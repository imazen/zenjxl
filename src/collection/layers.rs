//! Raw layers through the qualified native F32 path.
//! Selection replays from the start; it does not advertise efficient raw seeks.
use super::output::{JxlRawLayer, JxlSourceFacts, adopt};
use super::presentations::{
    Limits, complete, controlled_options, decode_next_details, describe_header, pixel_decoder,
};
use super::{Control, Failure, NativeState as Error, located};
use alloc::{boxed::Box, sync::Arc, vec::Vec};
use jxl::api::{ExtraChannel, JxlColorType, JxlDecoder, JxlLayerInfo, states};
use zencodec::At;
use zencodec::collection::{
    Collection, CollectionError, Composition, ImageDescription as Description, ImageId,
    ImageRequest, Origin, RegionRequest, SelectionError,
};
use zencodec::image::ImageOutput;

pub(super) struct JxlLayers<'a> {
    bytes: &'a [u8],
    remaining: &'a [u8],
    scanner: Option<JxlDecoder<states::WithImageInfo>>,
    description: Description,
    layers: Vec<JxlLayerInfo>,
    origin: Origin,
    limits: Limits,
    control: Control,
    failure: Option<FailedScan>,
}

#[derive(Clone, Debug)]
struct FailedScan(Arc<dyn core::error::Error + Send + Sync>);
impl core::fmt::Display for FailedScan {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.0.fmt(f)
    }
}
impl core::error::Error for FailedScan {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        Some(self.0.as_ref())
    }
}

pub(super) struct Cursor {
    owner: ImageId,
    index: usize,
}

impl<'a> JxlLayers<'a> {
    pub(super) fn open(bytes: &'a [u8], limits: Limits, control: Control) -> Result<Self, Failure> {
        control.check()?;
        let options = controlled_options(&control, limits)
            .with_coalescing(false)
            .with_render_spot_colors(false)
            .with_scan_frames_only(true);
        let mut remaining = bytes;
        let scanner = complete(JxlDecoder::new(options).process(&mut remaining)?)?;
        let description = describe_header(scanner.basic_info())?;
        control.check()?;
        Ok(Self {
            bytes,
            remaining,
            scanner: Some(scanner),
            description,
            layers: Vec::new(),
            origin: Origin::default(),
            limits,
            control,
            failure: None,
        })
    }

    fn check(&self) -> Result<(), Failure> {
        if let Some(failure) = &self.failure {
            return Err(Box::new(failure.clone()));
        }
        self.control.check()
    }

    fn discover(&mut self) -> Result<Option<ImageId>, Failure> {
        self.check()?;
        let Some(scanner) = self.scanner.as_ref() else {
            return Ok(None);
        };
        if !scanner.has_more_frames() {
            self.scanner = None;
            return Ok(None);
        }
        if self.layers.len() >= self.limits.images {
            return Err(SelectionError::DiscoveryLimit.into());
        }
        self.layers.try_reserve_exact(1)?;
        let frame = complete(self.scanner.take().unwrap().process(&mut self.remaining)?)?;
        self.control.check()?;
        let layer = frame.frame_layer_info();
        let scanner = complete(frame.skip_frame(&mut self.remaining)?)?;
        self.control.check()?;
        let image = self.origin.image(self.layers.len() as u64);
        self.layers.push(layer);
        self.scanner = Some(scanner);
        Ok(Some(image))
    }

    fn selected(&self, image: &ImageId) -> Result<ImageOutput, Failure> {
        let index = usize::try_from(self.origin.key(image)?)?;
        self.check()?;
        let selected = self
            .layers
            .get(index)
            .ok_or_else(|| Error::other("unknown raw layer"))?;
        let options = controlled_options(&self.control, self.limits)
            .with_coalescing(false)
            .with_render_spot_colors(false);
        let (mut decoder, mut input) = pixel_decoder(self.bytes, options)?;
        let info = decoder.basic_info().clone();
        let embedded_color = decoder.embedded_color_profile().clone();
        let mut extra_details = Vec::new();
        extra_details.try_reserve_exact(info.extra_channels.len())?;
        for channel in 0..info.extra_channels.len() {
            self.control.check()?;
            extra_details.push(
                decoder
                    .extra_channel_details(channel)
                    .ok_or_else(|| Error::other("missing native channel details"))?,
            );
        }
        let mut format = decoder.current_pixel_format().clone();
        if info
            .extra_channels
            .iter()
            .any(|e| e.ec_type == ExtraChannel::Black)
        {
            // CMY plus a separate K plane must not be mislabeled as RGB.
            format.color_type = JxlColorType::Cmyk;
        } else if info
            .extra_channels
            .iter()
            .any(|e| e.ec_type == ExtraChannel::Alpha)
            && !format.color_type.has_alpha()
        {
            format.color_type = format
                .color_type
                .add_alpha()
                .ok_or_else(|| Error::other("cannot interleave alpha"))?;
        }
        decoder.set_pixel_format(format.clone());
        let profile = decoder.output_color_profile().clone();
        for expected in &self.layers[..index] {
            self.control.check()?;
            let frame = complete(decoder.process(&mut input)?)?;
            if frame.frame_layer_info() != *expected {
                return Err(Error::other("raw-layer discovery/replay disagrees").into());
            }
            // Native skip still reconstructs references needed by later layers.
            decoder = complete(frame.skip_frame(&mut input)?)?;
        }
        self.control.check()?;
        let decoded = decode_next_details(
            decoder,
            &mut input,
            format.color_type.samples_per_pixel() * 4,
            self.control.output_storage.as_ref(),
        )?;
        self.control.check()?;
        if decoded.layer != *selected {
            return Err(Error::other("selected layer changed during replay").into());
        }
        let timing = if let Some(animation) = &info.animation {
            let ticks = self.layers[..index].iter().try_fold(0i64, |sum, layer| {
                sum.checked_add(i64::from(layer.duration_ticks))
                    .ok_or_else(|| Error::other("raw-layer timestamp overflow"))
            })?;
            let base =
                zencodec::time::TimeBase::new(animation.tps_denominator, animation.tps_numerator)?;
            Some(
                zencodec::frame::FrameTiming::at(zencodec::time::Timestamp::new(ticks, base))
                    .with_duration_ticks(u64::from(selected.duration_ticks)),
            )
        } else {
            None
        };
        let result = adopt(
            decoded.planes,
            JxlSourceFacts {
                header: info,
                embedded_color,
                frame_index: index,
                frame_name: decoded.name,
                is_last: selected.is_last,
                frame_timecode: decoded.timecode,
                extra_details,
            },
            &format,
            &profile,
            timing,
            false,
            Some(JxlRawLayer {
                image: image.clone(),
                native: decoded.layer,
            }),
        )?;
        self.control.check()?;
        Ok(result)
    }
}

impl Collection for JxlLayers<'_> {
    fn describe(&mut self, image: &ImageId) -> Result<Description, At<CollectionError>> {
        let describe = || -> Result<Description, Failure> {
            let index = usize::try_from(self.origin.key(image)?)?;
            self.check()?;
            let layer = self
                .layers
                .get(index)
                .ok_or_else(|| Error::other("unknown raw layer"))?;
            let mut description =
                Description::new((u32::try_from(layer.size.0)?, u32::try_from(layer.size.1)?))
                    .with_placement(zencodec::image::Placement {
                        canvas: self.description.stored_size,
                        origin: (i64::from(layer.origin.0), i64::from(layer.origin.1)),
                    });
            description.stored_orientation = self.description.stored_orientation;
            description.validate()?;
            self.check()?;
            Ok(description)
        };
        describe().map_err(located)
    }

    type Cursor = Cursor;
    fn view(&self) -> Composition {
        Composition::Uncomposited
    }
    fn images(&mut self) -> Result<Cursor, At<CollectionError>> {
        self.check().map_err(located)?;
        Ok(Cursor {
            owner: self.origin.image(u64::MAX),
            index: 0,
        })
    }
    fn children(&mut self, image: &ImageId) -> Result<Cursor, At<CollectionError>> {
        self.origin.key(image).map_err(|e| located(e.into()))?;
        self.check().map_err(located)?;
        Err(located(SelectionError::UnsupportedChildren.into()))
    }
    fn next(&mut self, cursor: &mut Cursor) -> Result<Option<ImageId>, At<CollectionError>> {
        self.origin
            .key(&cursor.owner)
            .map_err(|_| located(SelectionError::ForeignCursor.into()))?;
        self.check().map_err(located)?;
        let next = if cursor.index < self.layers.len() {
            Some(self.origin.image(cursor.index as u64))
        } else {
            match self.discover() {
                Ok(next) => next,
                Err(cause) => {
                    let failure = FailedScan(Arc::from(cause));
                    self.failure = Some(failure.clone());
                    self.scanner = None;
                    return Err(located(Box::new(failure)));
                }
            }
        };
        self.control.check().map_err(located)?;
        if next.is_some() {
            cursor.index += 1;
        }
        Ok(next)
    }
    fn decode(
        &mut self,
        image: &ImageId,
        request: &ImageRequest,
    ) -> Result<ImageOutput, At<CollectionError>> {
        self.origin.key(image).map_err(|e| located(e.into()))?;
        self.check().map_err(located)?;
        if request.region != RegionRequest::Full {
            return Err(located(SelectionError::UnsupportedRegion.into()));
        }
        self.selected(image).map_err(located)
    }
}
