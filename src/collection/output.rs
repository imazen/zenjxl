//! Interpret actual requested output, not merely the embedded source profile.
use super::presentations::Plane;
use super::{Failure, NativeState as Error};
use alloc::{boxed::Box, string::String, sync::Arc, vec::Vec};
use jxl::api::{
    ExtraChannel, JxlBasicInfo, JxlBitDepth, JxlColorProfile, JxlColorType, JxlExtraChannelDetails,
    JxlPixelFormat,
};
use zencodec::collection::ImageId;
use zencodec::image::{
    Auxiliary, AuxiliaryKind, ColorContribution, ImageGeometry, ImageInterpretation, ImageOutput,
    OutputError, PackedImageOwner, Rect, SourcePrecision, ValueSlice,
};
use zenpixels::{
    AlphaMode, ChannelLayout, ChannelType, ColorContext, ColorPrimaries, Orientation,
    PixelDescriptor, PixelFormat, PixelSlice, TransferFunction,
};

/// Retain native source facts without treating them as an output profile or
/// as metadata approved for passthrough.
#[non_exhaustive]
pub struct JxlSourceFacts {
    /// Native source declaration, not the returned pixels' output profile.
    pub header: JxlBasicInfo,
    /// Embedded color declaration, retained separately from output ICC.
    pub embedded_color: JxlColorProfile,
    /// Index in this opened view; not a portable identity or reference slot.
    pub frame_index: usize,
    /// Untrusted source name; not metadata approved for passthrough.
    pub frame_name: String,
    /// Native end-of-sequence marker for the selected presentation or layer.
    pub is_last: bool,
    /// Optional source timecode, separate from the rational presentation clock.
    pub frame_timecode: Option<u32>,
    /// Source precision and role parameters for each native extra channel.
    pub extra_details: Vec<JxlExtraChannelDetails>,
}

/// Frozen with the actual local samples. Reference slots are scoped by image's
/// opened-source identity; they are not collection item indices or portable IDs.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct JxlRawLayer {
    /// Identity from the opened uncomposited view.
    pub image: ImageId,
    /// Stored blend/reference and geometry facts for these exact local samples.
    pub native: jxl::api::JxlLayerInfo,
}

pub(super) fn adopt(
    planes: Vec<Plane>,
    source: JxlSourceFacts,
    format: &JxlPixelFormat,
    profile: &JxlColorProfile,
    timing: Option<zencodec::frame::FrameTiming>,
    spot_colors_rendered: bool,
    layer: Option<JxlRawLayer>,
) -> Result<ImageOutput, Failure> {
    let info = &source.header;
    let layout = match format.color_type {
        JxlColorType::Grayscale => ChannelLayout::Gray,
        JxlColorType::GrayscaleAlpha => ChannelLayout::GrayAlpha,
        JxlColorType::Rgb => ChannelLayout::Rgb,
        JxlColorType::Rgba => ChannelLayout::Rgba,
        JxlColorType::Bgra => ChannelLayout::Bgra,
        JxlColorType::Cmyk => ChannelLayout::Cmyk,
        JxlColorType::Bgr => return Err(Error::other("BGR interpretation is unqualified").into()),
    };
    let alpha = if layout.has_alpha() {
        let channel = info
            .extra_channels
            .iter()
            .find(|ec| ec.ec_type == ExtraChannel::Alpha)
            .ok_or_else(|| Error::other("output alpha has no source declaration"))?;
        Some(alpha_mode(channel.alpha_associated))
    } else {
        None
    };
    let icc = match profile {
        JxlColorProfile::Icc(bytes) => bytes.clone(),
        JxlColorProfile::Simple(encoding) => encoding
            .maybe_create_profile()?
            .ok_or_else(|| Error::other("native output color cannot be represented by ICC"))?,
    };
    let mut planes = planes.into_iter();
    let color = planes
        .next()
        .ok_or_else(|| Error::other("missing native color plane"))?;
    let (width, height) = (u32::try_from(color.width)?, u32::try_from(color.height)?);
    let physical_format = if layout == ChannelLayout::Cmyk {
        // Native JXL values have reversed ink polarity. An ICC profile alone
        // does not declare this; preserve it in the converter's format input.
        PixelFormat::CmykF32Inverted
    } else {
        PixelFormat::from_parts(ChannelType::F32, layout, alpha)
            .ok_or_else(|| Error::other("native output layout has no pixel format"))?
    };
    let descriptor = PixelDescriptor::from_pixel_format(physical_format)
        .with_alpha(alpha)
        .with_transfer(TransferFunction::Unknown)
        .with_primaries(ColorPrimaries::Unknown);
    let context = Arc::new(ColorContext::from_icc(Arc::<[u8]>::from(icc)));
    let mut extras = Vec::new();
    extras.try_reserve_exact(info.extra_channels.len())?;
    let mut extra_planes = Vec::new();
    extra_planes.try_reserve_exact(info.extra_channels.len())?;
    if source.extra_details.len() != info.extra_channels.len() {
        return Err(Error::other("extra-channel interpretation count differs from source").into());
    }
    for (ec, details) in info.extra_channels.iter().zip(&source.extra_details) {
        let plane = planes
            .next()
            .ok_or_else(|| Error::other("missing requested native extra channel"))?;
        if plane.channels != 1 {
            return Err(Error::other("extra plane has multiple channels").into());
        }
        let kind = match ec.ec_type {
            ExtraChannel::Alpha => AuxiliaryKind::Alpha(alpha_mode(ec.alpha_associated)),
            ExtraChannel::Depth => AuxiliaryKind::Depth,
            ExtraChannel::SpotColor => AuxiliaryKind::SpotColor {
                linear_rgba: details
                    .spot_color
                    .ok_or_else(|| Error::other("spot channel is missing source RGBA"))?,
                contribution: if spot_colors_rendered {
                    ColorContribution::Applied
                } else {
                    ColorContribution::Separate
                },
            },
            ExtraChannel::SelectionMask => AuxiliaryKind::SelectionMask,
            ExtraChannel::Black => AuxiliaryKind::Black { inverted: true },
            ExtraChannel::CFA => AuxiliaryKind::Cfa {
                channel: details
                    .cfa_channel
                    .ok_or_else(|| Error::other("CFA channel is missing source index"))?,
            },
            ExtraChannel::Thermal => AuxiliaryKind::Thermal,
            other => AuxiliaryKind::Unknown(other as u32),
        };
        extras.push(Auxiliary {
            kind,
            name: ec.name.clone(),
            source_precision: match details.bit_depth {
                JxlBitDepth::Int { bits_per_sample } => SourcePrecision::Integer {
                    bits: bits_per_sample,
                    signed: false,
                },
                JxlBitDepth::Float {
                    bits_per_sample,
                    exponent_bits_per_sample,
                } => SourcePrecision::Float {
                    bits: bits_per_sample,
                    exponent_bits: exponent_bits_per_sample,
                },
            },
            source_dim_shift: ec.dim_shift,
        });
        // Validate the addressable public dimensions before publishing the owner.
        u32::try_from(plane.width)?;
        u32::try_from(plane.height)?;
        extra_planes.push(plane);
    }
    if planes.next().is_some() {
        return Err(Error::other("unidentified native extra plane").into());
    }
    let canvas = layer
        .as_ref()
        .map_or(info.coded_size, |layer| layer.native.size);
    let geometry = ImageGeometry {
        canvas_width: u32::try_from(canvas.0)?,
        canvas_height: u32::try_from(canvas.1)?,
        region: Rect {
            x: 0,
            y: 0,
            width,
            height,
        },
    };
    let orientation = Orientation::from_exif(info.orientation as u8)
        .ok_or_else(|| Error::other("invalid JXL orientation"))?;
    let mut interpretation = ImageInterpretation::packed(geometry, orientation);
    interpretation.timing = timing;
    if let Some(layer) = &layer {
        interpretation.placement = Some(zencodec::image::Placement {
            canvas: (
                u32::try_from(info.coded_size.0)?,
                u32::try_from(info.coded_size.1)?,
            ),
            origin: (
                i64::from(layer.native.origin.0),
                i64::from(layer.native.origin.1),
            ),
        });
    }
    interpretation.source_details.insert(source);
    if let Some(layer) = layer {
        interpretation.source_details.insert(layer);
    }
    ImageOutput::from_packed_owner(
        Packed {
            color,
            extras: extra_planes,
            descriptor,
            context,
        },
        extras,
        interpretation,
    )
    .map_err(Into::into)
}

// Own the original buffers, including any admission reservations. There is no
// Vec extraction/copy between native reconstruction and shared publication.
struct Packed {
    color: Plane,
    extras: Vec<Plane>,
    descriptor: PixelDescriptor,
    context: Arc<ColorContext>,
}
impl PackedImageOwner for Packed {
    fn pixels(&self) -> Result<PixelSlice<'_>, zencodec::At<OutputError>> {
        PixelSlice::new(
            self.color.bytes.as_slice(),
            self.color.width as u32,
            self.color.height as u32,
            self.color.stride,
            self.descriptor,
        )
        .map(|p| p.with_color_context(self.context.clone()))
        .map_err(|e| {
            zencodec::At::wrap(OutputError::Mapping(zencodec::frame::MapError::Backend(
                Box::new(e),
            )))
        })
    }
    fn auxiliary(&self, index: usize) -> Result<Option<ValueSlice<'_>>, zencodec::At<OutputError>> {
        self.extras
            .get(index)
            .map(|plane| {
                ValueSlice::new(
                    plane.bytes.as_slice(),
                    plane.width as u32,
                    plane.height as u32,
                    plane.stride,
                    ChannelType::F32,
                )
            })
            .transpose()
    }
}

fn alpha_mode(associated: bool) -> AlphaMode {
    if associated {
        AlphaMode::Premultiplied
    } else {
        AlphaMode::Straight
    }
}
