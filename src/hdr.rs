//! HDR content-description metadata carried beside the native JXL tone mapping.
//!
//! The optional `hCdR` box follows the encoder's ST 2086 / CTA-861 layout.
//! The 24-byte mastering block and the 4-byte CLL block may appear alone or
//! together, so an absent field and an explicitly measured zero stay distinct.

use zenpixels::HdrMetadata;
#[cfg(feature = "decode")]
use zenpixels::{ContentLightLevel, hdr::MasteringDisplay};

#[cfg(all(feature = "encode", feature = "zencodec"))]
pub(crate) fn encode(hdr: HdrMetadata) -> Option<([u8; 28], usize)> {
    let mut payload = [0; 28];
    let mut len = 0;
    if let Some(mastering) = hdr.mastering {
        let xy = mastering
            .primaries_xy
            .into_iter()
            .flatten()
            .chain(mastering.white_point_xy);
        for (dst, value) in payload[..16].as_chunks_mut::<2>().0.iter_mut().zip(xy) {
            if !(0.0..=1.0).contains(&value) {
                return None;
            }
            *dst = ((f64::from(value) * 50_000.0 + 0.5) as u16).to_be_bytes();
        }
        for (dst, value) in payload[16..24]
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip([mastering.max_luminance, mastering.min_luminance])
        {
            let units = f64::from(value) * 10_000.0 + 0.5;
            if !(0.0..=f64::from(u32::MAX)).contains(&units) {
                return None;
            }
            *dst = (units as u32).to_be_bytes();
        }
        len = 24;
    }
    if let Some(cll) = hdr.content_light_level {
        payload[len..len + 2].copy_from_slice(&cll.max_content_light_level.to_be_bytes());
        payload[len + 2..len + 4].copy_from_slice(&cll.max_frame_average_light_level.to_be_bytes());
        len += 4;
    }
    Some((payload, len))
}

#[cfg(feature = "decode")]
fn decode(payload: &[u8]) -> Option<HdrMetadata> {
    let (mastering, cll) = match payload.len() {
        4 => (None, payload),
        24 | 28 => {
            let (master, cll) = payload.split_at(24);
            let xy =
                |i: usize| f32::from(u16::from_be_bytes([master[i], master[i + 1]])) / 50_000.0;
            let nits = |i: usize| {
                (f64::from(u32::from_be_bytes(master[i..i + 4].try_into().unwrap())) / 10_000.0)
                    as f32
            };
            let mastering = MasteringDisplay::new(
                [[xy(0), xy(2)], [xy(4), xy(6)], [xy(8), xy(10)]],
                [xy(12), xy(14)],
                nits(16),
                nits(20),
            );
            (Some(mastering), cll)
        }
        _ => return None,
    };
    let cll = match cll {
        [a, b, c, d] => Some(ContentLightLevel::new(
            u16::from_be_bytes([*a, *b]),
            u16::from_be_bytes([*c, *d]),
        )),
        _ => None,
    };
    Some(HdrMetadata {
        content_light_level: cll,
        mastering,
    })
}

/// Read a bounded HDR box without scanning or allocating codestream payloads.
#[cfg(feature = "decode")]
pub(crate) fn from_container(data: &[u8]) -> Option<HdrMetadata> {
    let mut boxes = data.strip_prefix(b"\0\0\0\x0cJXL \r\n\x87\n")?;
    while let Some((head, _)) = boxes.split_first_chunk::<8>() {
        let size = u32::from_be_bytes(head[..4].try_into().unwrap());
        let (len, header) = match size {
            0 => (boxes.len(), 8),
            1 => {
                let extended = boxes.get(8..)?.first_chunk::<8>()?;
                (usize::try_from(u64::from_be_bytes(*extended)).ok()?, 16)
            }
            n => (n as usize, 8),
        };
        let payload = boxes.get(header..len)?;
        if &head[4..] == b"hCdR"
            && let Some(hdr) = decode(payload)
        {
            return Some(hdr);
        }
        boxes = boxes.get(len..)?;
    }
    None
}
