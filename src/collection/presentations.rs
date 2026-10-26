//! Presentations through the native JPEG XL decoder.
//! This path retains the input and scans visible presentations, not raw layers.
use super::{Control, Failure, NativeState as Error, located};
use alloc::{boxed::Box, string::String, sync::Arc, vec::Vec};
use jxl::api::{
    JxlDataFormat, JxlDecoder, JxlDecoderOptions, JxlOutputBuffer, JxlPixelFormat,
    ProcessingResult, VisibleFrameInfo, states,
};
use zencodec::collection::{
    Collection, CollectionError, Composition, ImageDescription as Description, ImageId,
    ImageRequest, Origin, RegionRequest, SelectionError,
};
use zencodec::image::ImageOutput;
use zencodec::storage::{OwnedStorage, StorageBudget};

pub(super) struct JxlItems<'a> {
    data: &'a [u8],
    remaining: &'a [u8],
    scanner: Option<JxlDecoder<states::WithImageInfo>>,
    description: Description,
    origin: Origin,
    visible: Vec<VisibleFrameInfo>,
    limits: Limits,
    failed: Option<DiscoveryFailure>,
    control: Control,
}

/// Separate selected-item storage from the native work needed to discover it.
/// Each selected decode creates a fresh native decoder with this same bound;
/// these are not shared charges across the collection's independent decodes.
#[derive(Clone, Copy, Debug)]
pub(super) struct Limits {
    pub(super) images: usize,
    pub(super) frame_headers: u64,
}
#[derive(Clone, Debug)]
struct DiscoveryFailure(Arc<dyn core::error::Error + Send + Sync>);
impl core::fmt::Display for DiscoveryFailure {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.0.fmt(f)
    }
}
impl core::error::Error for DiscoveryFailure {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        Some(self.0.as_ref())
    }
}

pub(super) fn complete<T, U>(result: ProcessingResult<T, U>) -> Result<T, Failure> {
    match result {
        ProcessingResult::Complete { result } => Ok(result),
        ProcessingResult::NeedsMoreInput { .. } => Err(zencodec::At::wrap(
            crate::JxlError::UnexpectedEof("incomplete JXL input".into()),
        )
        .into()),
    }
}

fn options() -> JxlDecoderOptions {
    JxlDecoderOptions::default()
        .with_adjust_orientation(false)
        .with_dither_u8(false)
}

pub(super) fn controlled_options(control: &Control, limits: Limits) -> JxlDecoderOptions {
    let mut options = options();
    options.limits.max_frame_headers = Some(limits.frame_headers);
    options.with_stop(control.token())
}

pub(super) fn describe_header(info: &jxl::api::JxlBasicInfo) -> Result<Description, Failure> {
    let description = Description::new((
        u32::try_from(info.coded_size.0)?,
        u32::try_from(info.coded_size.1)?,
    ))
    .with_orientation(
        zenpixels::Orientation::from_exif(info.intrinsic_orientation as u8)
            .ok_or_else(|| Error::other("invalid stored JXL orientation"))?,
    );
    description.validate()?;
    Ok(description)
}

/// Raw native channel bytes, with explicit row layout. f32 samples are compared
/// bitwise, including NaNs; this is not a color conversion or display encoding.
#[derive(Debug)]
pub(super) struct Plane {
    pub(super) bytes: PlaneBytes,
    pub(super) width: usize,
    pub(super) height: usize,
    pub(super) channels: usize,
    pub(super) stride: usize,
}

#[derive(Debug)]
pub(super) enum PlaneBytes {
    Ordinary(Vec<u32>),
    Admitted(OwnedStorage<u32>),
}
impl PlaneBytes {
    fn new(size: usize, storage: Option<&StorageBudget>) -> Result<Self, Failure> {
        // A byte vector need not have f32 alignment. Keep aligned words as the
        // backing allocation and borrow their bytes without changing ownership.
        if !size.is_multiple_of(4) {
            return Err(Error::other("native F32 plane has a partial sample").into());
        }
        let words = size / 4;
        if let Some(storage) = storage {
            return Ok(Self::Admitted(storage.filled(words, 0xa5a5_a5a5u32)?));
        }
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(words)?;
        bytes.resize(words, 0xa5a5_a5a5);
        Ok(Self::Ordinary(bytes))
    }
    pub(super) fn as_slice(&self) -> &[u8] {
        bytemuck::cast_slice(match self {
            Self::Ordinary(bytes) => bytes,
            Self::Admitted(bytes) => bytes.as_slice(),
        })
    }
    fn as_mut_slice(&mut self) -> &mut [u8] {
        bytemuck::cast_slice_mut(match self {
            Self::Ordinary(bytes) => bytes,
            Self::Admitted(bytes) => bytes.as_mut_slice(),
        })
    }
}

impl<'a> JxlItems<'a> {
    /// The view is fixed before discovery: flattened presentations and exposed
    /// layers have different cardinalities, so changing it at decode is unsound.
    pub(super) fn open_controlled(
        data: &'a [u8],
        composition: Composition,
        limits: Limits,
        control: Control,
    ) -> Result<Self, Failure> {
        if composition != Composition::Composited {
            return Err(SelectionError::UnsupportedComposition.into());
        }
        control.check()?;
        let mut remaining = data;
        let decoder =
            JxlDecoder::new(controlled_options(&control, limits).with_scan_frames_only(true));
        let scanner = complete(decoder.process(&mut remaining)?)?;
        let description = describe_header(scanner.basic_info())?;
        control.check()?;
        Ok(Self {
            data,
            remaining,
            scanner: Some(scanner),
            description,
            origin: Origin::default(),
            visible: Vec::new(),
            limits,
            failed: None,
            control,
        })
    }

    fn discover(&mut self) -> Result<Option<ImageId>, Failure> {
        self.check_failed()?;
        match self.next_inner() {
            Ok(item) => Ok(item),
            Err(cause) => {
                let error = DiscoveryFailure(Arc::from(cause));
                self.failed = Some(error.clone());
                self.scanner = None;
                Err(Box::new(error))
            }
        }
    }

    fn check_failed(&self) -> Result<(), Failure> {
        self.failed
            .as_ref()
            .map_or(Ok(()), |e| Err(Box::new(e.clone())))
    }

    fn next_inner(&mut self) -> Result<Option<ImageId>, Failure> {
        self.control.check()?;
        let Some(scanner) = self.scanner.as_ref() else {
            return Ok(None);
        };
        if !scanner.has_more_frames() {
            self.scanner = None;
            return Ok(None);
        }
        if self.visible.len() >= self.limits.images {
            return Err(SelectionError::DiscoveryLimit.into());
        }
        self.visible.try_reserve_exact(1)?;
        let scanner = self.scanner.take().expect("checked above");
        let frame = complete(scanner.process(&mut self.remaining)?)?;
        self.control.check()?;
        let scanner = complete(frame.skip_frame(&mut self.remaining)?)?;
        self.control.check()?;
        let native = scanner
            .scanned_frames()
            .last()
            .ok_or_else(|| Error::other("native scan returned no visible presentation"))?
            .clone();
        if native.index != self.visible.len() {
            return Err(Error::other("native visible index is not sequential").into());
        }
        let image = self.origin.image(native.index as u64);
        self.visible.push(native.clone());
        self.scanner = Some(scanner);
        Ok(Some(image))
    }

    fn decode_image(&self, image: &ImageId) -> Result<ImageOutput, Failure> {
        // Composition is fixed at open; presentations render spot contributions.
        let render = true;
        let native_options =
            controlled_options(&self.control, self.limits).with_render_spot_colors(render);
        let (mut decoder, mut input) = self.selected_decoder(image, native_options)?;
        let index = usize::try_from(self.origin.key(image)?)?;
        let selected = &self.visible[index];
        let info = decoder.basic_info().clone();
        let embedded_color = decoder.embedded_color_profile().clone();
        let mut extra_details = Vec::new();
        extra_details.try_reserve_exact(info.extra_channels.len())?;
        for index in 0..info.extra_channels.len() {
            self.control.check()?;
            extra_details.push(
                decoder
                    .extra_channel_details(index)
                    .ok_or_else(|| Error::other("native extra-channel details unavailable"))?,
            );
        }
        let mut format = decoder.current_pixel_format().clone();
        if info
            .extra_channels
            .iter()
            .any(|ec| ec.ec_type == jxl::api::ExtraChannel::Alpha)
            && !format.color_type.has_alpha()
        {
            format.color_type = format
                .color_type
                .add_alpha()
                .ok_or_else(|| Error::other("packed color cannot carry required alpha"))?;
            decoder.set_pixel_format(format.clone());
        }
        let profile = decoder.output_color_profile().clone();
        // PixelSlice requires whole-pixel strides, not just sample alignment.
        // We own these buffers, so request one pixel of padding. The native
        // 13-byte padding stress path remains in the native qualification harness.
        let padding = format.color_type.samples_per_pixel() * 4;
        let decoded = decode_next_details(
            decoder,
            &mut input,
            padding,
            self.control.output_storage.as_ref(),
        )?;
        self.control.check()?;
        let timing = if let Some(animation) = &info.animation {
            let ticks = self.visible[..index].iter().try_fold(
                0i64,
                |sum, frame| -> Result<i64, Failure> {
                    self.control.check()?;
                    sum.checked_add(i64::from(frame.duration_ticks))
                        .ok_or_else(|| Error::other("JXL timestamp overflow").into())
                },
            )?;
            let base =
                zencodec::time::TimeBase::new(animation.tps_denominator, animation.tps_numerator)?;
            Some(
                zencodec::frame::FrameTiming::at(zencodec::time::Timestamp::new(ticks, base))
                    .with_duration_ticks(u64::from(self.visible[index].duration_ticks)),
            )
        } else {
            None
        };
        let image = super::output::adopt(
            decoded.planes,
            super::output::JxlSourceFacts {
                header: info,
                embedded_color,
                frame_index: index,
                frame_name: selected.name.clone(),
                is_last: selected.is_last,
                frame_timecode: decoded.timecode,
                extra_details,
            },
            &format,
            &profile,
            timing,
            render,
            None,
        )?;
        self.control.check()?;
        Ok(image)
    }

    fn selected_decoder(
        &self,
        image: &ImageId,
        native_options: JxlDecoderOptions,
    ) -> Result<(JxlDecoder<states::WithImageInfo>, &[u8]), Failure> {
        let key = self.origin.key(image)?;
        self.check_failed()?;
        self.control.check()?;
        let index = usize::try_from(key)?;
        let frame = self
            .visible
            .get(index)
            .ok_or_else(|| Error::other("unknown image"))?;
        let target = frame
            .seek_target
            .ok_or_else(|| Error::other("native source has no direct seek target"))?;
        let (mut decoder, _) = pixel_decoder(self.data, native_options)?;
        self.control.check()?;
        decoder.try_start_new_frame(target)?;
        let start = usize::try_from(target.decode_start_file_offset)?;
        let input = self
            .data
            .get(start..)
            .ok_or_else(|| Error::other("native seek outside input"))?;
        Ok((decoder, input))
    }
}

pub(super) fn pixel_decoder(
    data: &[u8],
    options: JxlDecoderOptions,
) -> Result<(JxlDecoder<states::WithImageInfo>, &[u8]), Failure> {
    let mut input = data;
    let mut decoder = complete(JxlDecoder::new(options).process(&mut input)?)?;
    let format = decoder.current_pixel_format().clone();
    decoder.set_pixel_format(JxlPixelFormat {
        color_type: format.color_type,
        color_data_format: Some(JxlDataFormat::f32()),
        extra_channel_format: format
            .extra_channel_format
            .iter()
            .map(|_| Some(JxlDataFormat::f32()))
            .collect(),
    });
    Ok((decoder, input))
}

pub(super) struct DecodedNext {
    pub(super) planes: Vec<Plane>,
    pub(super) timecode: Option<u32>,
    pub(super) layer: jxl::api::JxlLayerInfo,
    pub(super) name: String,
}

pub(super) fn decode_next_details(
    decoder: JxlDecoder<states::WithImageInfo>,
    input: &mut &[u8],
    padding: usize,
    output_storage: Option<&StorageBudget>,
) -> Result<DecodedNext, Failure> {
    let format = decoder.current_pixel_format().clone();
    let frame = complete(decoder.process(input)?)?;
    let header = frame.frame_header();
    let (width, height) = header.size;
    let layer = frame.frame_layer_info();
    let mut planes = Vec::new();
    planes.try_reserve_exact(1 + format.extra_channel_format.len())?;
    for channels in core::iter::once(format.color_type.samples_per_pixel()).chain(
        format
            .extra_channel_format
            .iter()
            .filter(|v| v.is_some())
            .map(|_| 1),
    ) {
        let stride = width
            .checked_mul(channels)
            .and_then(|v| v.checked_mul(4))
            .and_then(|v| v.checked_add(padding))
            .ok_or_else(|| Error::other("row overflow"))?;
        let size = stride
            .checked_mul(height)
            .ok_or_else(|| Error::other("plane overflow"))?;
        let bytes = PlaneBytes::new(size, output_storage)?;
        planes.push(Plane {
            bytes,
            width,
            height,
            channels,
            stride,
        });
    }
    let timecode = frame.frame_timecode();
    let mut buffers: Vec<_> = planes
        .iter_mut()
        .map(|p| {
            JxlOutputBuffer::new_with_stride(
                p.bytes.as_mut_slice(),
                p.height,
                p.width * p.channels * 4,
                p.stride,
            )
        })
        .collect();
    let _decoder = complete(frame.process(input, &mut buffers)?)?;
    Ok(DecodedNext {
        planes,
        timecode,
        layer,
        name: header.name,
    })
}

pub(super) struct JxlCursor {
    owner: ImageId,
    index: usize,
}

impl Collection for JxlItems<'_> {
    fn describe(&mut self, image: &ImageId) -> Result<Description, zencodec::At<CollectionError>> {
        let describe = || -> Result<Description, Failure> {
            let index = usize::try_from(self.origin.key(image)?)?;
            self.check_failed()?;
            self.control.check()?;
            self.visible
                .get(index)
                .ok_or_else(|| Error::other("unknown presentation"))?;
            self.control.check()?;
            Ok(self.description)
        };
        describe().map_err(located)
    }

    type Cursor = JxlCursor;
    fn view(&self) -> Composition {
        Composition::Composited
    }
    fn images(&mut self) -> Result<JxlCursor, zencodec::At<CollectionError>> {
        self.check_failed().map_err(located)?;
        self.control.check().map_err(located)?;
        Ok(JxlCursor {
            owner: self.origin.image(u64::MAX),
            index: 0,
        })
    }
    fn children(&mut self, image: &ImageId) -> Result<JxlCursor, zencodec::At<CollectionError>> {
        self.origin.key(image).map_err(|e| located(e.into()))?;
        self.check_failed().map_err(located)?;
        self.control.check().map_err(located)?;
        Err(located(SelectionError::UnsupportedChildren.into()))
    }
    fn next(
        &mut self,
        cursor: &mut JxlCursor,
    ) -> Result<Option<ImageId>, zencodec::At<CollectionError>> {
        self.origin
            .key(&cursor.owner)
            .map_err(|_| located(SelectionError::ForeignCursor.into()))?;
        self.check_failed().map_err(located)?;
        self.control.check().map_err(located)?;
        let image = if cursor.index < self.visible.len() {
            Some(self.origin.image(cursor.index as u64))
        } else {
            self.discover().map_err(located)?
        };
        if image.is_some() {
            self.control.check().map_err(located)?;
            cursor.index += 1;
        }
        Ok(image)
    }
    fn decode(
        &mut self,
        image: &ImageId,
        request: &ImageRequest,
    ) -> Result<ImageOutput, zencodec::At<CollectionError>> {
        self.origin.key(image).map_err(|e| located(e.into()))?;
        if request.region != RegionRequest::Full {
            return Err(located(SelectionError::UnsupportedRegion.into()));
        }
        self.decode_image(image).map_err(located)
    }
}
