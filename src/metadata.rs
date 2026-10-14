//! Bounded JPEG XL metadata rewriting without pixel decoding.
// Container planner migrated from zencodecs at aa21cba0.
use alloc::{borrow::Cow, vec::Vec};
use core::ops::Range;
use zencodec::metadata_rewrite::{Action, Compression, Error, Request};
use zenmeta::rewrite::{Context as ScrubRequest, PlanBuilder as Plan};

/// Plan a metadata rewrite while borrowing unchanged image data.
///
/// Requires the default-off `metadata-rewrite` feature. Codestream color and
/// HDR signaling remain opaque; unsupported transformations return an error.
pub fn rewrite<'a>(request: &Request<'a>) -> Result<zencodec::metadata_rewrite::Plan<'a>, Error> {
    let context = ScrubRequest::new(request)?;
    if !request.input().starts_with(b"\xff\x0a")
        && !request.input().starts_with(b"\0\0\0\x0cJXL \r\n\x87\n")
    {
        return Err(Error::Malformed("JXL signature"));
    }
    plan(&context)?.finish()
}

fn plan<'a>(req: &ScrubRequest<'a>) -> Result<Plan<'a>, Error> {
    let mut out = Plan::new();
    if matches!(req.policy().fields().icc, zencodec::IccRetention::Drop) {
        return Err(Error::Unsupported(
            "JXL codestream color cannot be stripped without rewriting it",
        ));
    }
    if req.normalize_icc() {
        return Err(Error::Unsupported(
            "JXL codestream ICC normalization needs a codestream rewrite",
        ));
    }
    if req.input().starts_with(b"\xff\x0a") {
        out.record(
            req,
            0..req.input().len(),
            "JXL codestream (color/HDR opaque)",
            Action::OpaqueImageData,
            Some(Cow::Borrowed(req.input())),
        )?;
        return Ok(out);
    }
    out.keep(req, 0..12, "JXL signature")?;
    let mut pos = 12;
    let mut ftyp = false;
    let mut stream = false;
    let mut parts = 0u32;
    let mut last = false;
    let mut exif = false;
    let mut xmp = false;
    let mut gainmap = false;
    let mut level = false;
    while pos < req.input().len() {
        req.check()?;
        let size = be32(&req.input()[pos..])?;
        let kind: [u8; 4] = req
            .input()
            .get(pos + 4..pos + 8)
            .ok_or(Error::Malformed("JXL box header"))?
            .try_into()
            .unwrap();
        let header = if size == 1 { 16 } else { 8 };
        let len = match size {
            0 => req.input().len() - pos,
            1 => usize::try_from(u64::from_be_bytes(
                req.input()
                    .get(pos + 8..pos + 16)
                    .ok_or(Error::Malformed("JXL extended box size"))?
                    .try_into()
                    .unwrap(),
            ))
            .map_err(|_| Error::Limit(zencodec::LimitKind::OutputSize))?,
            n => n as usize,
        };
        if len < header {
            return Err(Error::Malformed("JXL box size below header"));
        }
        let end = pos
            .checked_add(len)
            .filter(|&n| n <= req.input().len())
            .ok_or(Error::Malformed("truncated JXL box"))?;
        let data = &req.input()[pos + header..end];
        let range = pos..end;
        if pos == 12 && kind != *b"ftyp" {
            return Err(Error::Malformed("JXL ftyp must follow signature"));
        }
        match &kind {
            b"JXL " => return Err(Error::Malformed("duplicate JXL signature")),
            b"ftyp" => {
                if ftyp
                    || data.len() < 12
                    || !data.len().is_multiple_of(4)
                    || &data[..4] != b"jxl "
                    || !data[8..].as_chunks::<4>().0.iter().any(|s| s == b"jxl ")
                {
                    return Err(Error::Malformed("JXL file type"));
                }
                ftyp = true;
                out.keep(req, range, "JXL file type")?;
            }
            b"jxll" => {
                if level || stream || parts > 0 || data.len() != 1 || !matches!(data[0], 5 | 10) {
                    return Err(Error::Malformed("JXL level"));
                }
                level = true;
                out.keep(req, range, "JXL level")?;
            }
            b"jxlc" => {
                if stream || parts > 0 || data.is_empty() {
                    return Err(Error::Malformed("duplicate/empty JXL codestream"));
                }
                stream = true;
                out.record(
                    req,
                    range.clone(),
                    "JXL codestream (color/HDR opaque)",
                    Action::OpaqueImageData,
                    Some(Cow::Borrowed(&req.input()[range])),
                )?;
            }
            b"jxlp" => {
                let index = be32(data)?;
                if stream || last || index & 0x7fff_ffff != parts || data.len() == 4 {
                    return Err(Error::Malformed("JXL partial codestream order"));
                }
                last = index & 0x8000_0000 != 0;
                parts += 1;
                out.record(
                    req,
                    range.clone(),
                    "JXL partial codestream (color/HDR opaque)",
                    Action::OpaqueImageData,
                    Some(Cow::Borrowed(&req.input()[range])),
                )?;
            }
            b"Exif" => {
                if exif {
                    return Err(Error::Malformed("duplicate JXL EXIF"));
                }
                exif = true;
                let bytes = filter_exif(req, data)?;
                out.record(
                    req,
                    range,
                    "EXIF",
                    Action::Rewrite,
                    Some(Cow::Owned(box_bytes(b"Exif", &bytes)?)),
                )?;
            }
            b"xml " => {
                if xmp {
                    return Err(Error::Malformed("duplicate JXL XMP"));
                }
                xmp = true;
                if let Some(bytes) = req.xmp(data)? {
                    out.record(
                        req,
                        range,
                        "XMP",
                        Action::Rewrite,
                        Some(Cow::Owned(box_bytes(b"xml ", &bytes)?)),
                    )?;
                } else {
                    out.remove(req, range, "XMP")?;
                }
            }
            b"jbrd" => remove_reconstruction(req, &mut out, range)?,
            b"brob" => {
                let inner: [u8; 4] = data
                    .get(..4)
                    .ok_or(Error::Malformed("JXL brob header"))?
                    .try_into()
                    .unwrap();
                match &inner {
                    b"jbrd" => remove_reconstruction(req, &mut out, range)?,
                    b"Exif" => {
                        if exif {
                            return Err(Error::Malformed("duplicate JXL EXIF"));
                        }
                        exif = true;
                        let expanded = req.decompress(Compression::Brotli, &data[4..])?;
                        let bytes = filter_exif(req, &expanded)?;
                        out.record(
                            req,
                            range,
                            "compressed EXIF",
                            Action::Rewrite,
                            Some(Cow::Owned(box_bytes(b"Exif", &bytes)?)),
                        )?;
                    }
                    b"xml " => {
                        if xmp {
                            return Err(Error::Malformed("duplicate JXL XMP"));
                        }
                        xmp = true;
                        if req.edit_xmp() {
                            let expanded = req.decompress(Compression::Brotli, &data[4..])?;
                            if let Some(bytes) = req.xmp(&expanded)? {
                                out.record(
                                    req,
                                    range,
                                    "compressed XMP",
                                    Action::Rewrite,
                                    Some(Cow::Owned(box_bytes(b"xml ", &bytes)?)),
                                )?;
                            } else {
                                out.remove(req, range, "compressed XMP")?;
                            }
                        } else {
                            out.remove(req, range, "compressed XMP")?;
                        }
                    }
                    b"jhgm" => return Err(Error::Unsupported("compressed JXL gain-map bundle")),
                    b"brob" | b"JXL " | b"ftyp" | b"jxlc" | b"jxlp" | b"jxll" => {
                        return Err(Error::Malformed("invalid compressed JXL box kind"));
                    }
                    _ => out.remove(req, range, "compressed private JXL box")?,
                }
            }
            b"jhgm" => {
                if gainmap {
                    return Err(Error::Malformed("duplicate JXL gain map"));
                }
                gainmap = true;
                validate_gain_map(req, data)?;
                out.record(
                    req,
                    range.clone(),
                    "JXL gain map (alternate ICC opaque)",
                    Action::OpaqueImageData,
                    Some(Cow::Borrowed(&req.input()[range])),
                )?;
            }
            _ => out.remove(req, range, "private/unknown JXL box")?,
        }
        pos = end;
    }
    if !ftyp || !(stream || parts > 0 && last) {
        return Err(Error::Malformed("incomplete JXL container"));
    }
    Ok(out)
}

fn remove_reconstruction<'a>(
    req: &ScrubRequest<'a>,
    out: &mut Plan<'a>,
    range: Range<usize>,
) -> Result<(), Error> {
    if !req.remove_reconstruction() {
        return Err(Error::Unsupported(
            "scrubbing requires explicit removal of JPEG reconstruction data",
        ));
    }
    out.record(
        req,
        range,
        "JPEG reconstruction data",
        Action::RemoveJpegReconstruction,
        None,
    )
}
fn filter_exif(req: &ScrubRequest<'_>, data: &[u8]) -> Result<Vec<u8>, Error> {
    req.packet(data.len())?;
    let start = 4usize
        .checked_add(be32(data)? as usize)
        .filter(|&n| n <= data.len())
        .ok_or(Error::Malformed("JXL EXIF TIFF offset"))?;
    let exif = req.exif(&data[start..])?;
    let tiff = exif.strip_prefix(b"Exif\0\0").unwrap_or(&exif);
    Ok([b"\0\0\0\0".as_slice(), tiff].concat())
}
fn validate_gain_map(req: &ScrubRequest<'_>, data: &[u8]) -> Result<(), Error> {
    if data.len() < 3 || data[0] != 0 {
        return Err(Error::Malformed("JXL gain-map version/header"));
    }
    let n = u16::from_be_bytes([data[1], data[2]]) as usize;
    req.packet(n)?;
    let metadata = data
        .get(3..3 + n)
        .ok_or(Error::Malformed("JXL gain-map parameters"))?;
    let params =
        zencodec::gainmap::parse_iso21496_fmt(metadata, zencodec::gainmap::Iso21496Format::JxlJhgm)
            .map_err(|_| Error::Malformed("JXL ISO gain-map parameters"))?;
    params
        .validate()
        .map_err(|_| Error::Malformed("JXL gain-map values"))?;
    let mut p = 3 + n;
    let color_size = *data
        .get(p)
        .ok_or(Error::Malformed("JXL alternate color header"))? as usize;
    p += 1 + color_size;
    let icc = be32(
        data.get(p..)
            .ok_or(Error::Malformed("JXL alternate ICC size"))?,
    )? as usize;
    req.packet(icc)?;
    p = p
        .checked_add(4)
        .and_then(|v| v.checked_add(icc))
        .ok_or(Error::Malformed("JXL alternate ICC length"))?;
    if !data.get(p..).is_some_and(|s| s.starts_with(b"\xff\x0a")) {
        return Err(Error::Malformed("JXL gain map must be a bare codestream"));
    }
    Ok(())
}
fn box_bytes(kind: &[u8; 4], data: &[u8]) -> Result<Vec<u8>, Error> {
    let size = data
        .len()
        .checked_add(8)
        .and_then(|n| u32::try_from(n).ok())
        .ok_or(Error::Limit(zencodec::LimitKind::MetadataSize))?;
    let mut out = Vec::new();
    out.try_reserve_exact(size as usize)
        .map_err(|_| Error::Allocation)?;
    out.extend_from_slice(&size.to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    Ok(out)
}

fn be32(bytes: &[u8]) -> Result<u32, Error> {
    Ok(u32::from_be_bytes(
        bytes
            .get(..4)
            .ok_or(Error::Malformed("truncated integer"))?
            .try_into()
            .unwrap(),
    ))
}
