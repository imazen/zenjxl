//! Generate an encoded/rewritten pair for independent libjxl comparison.
use std::path::PathBuf;
use zencodec::metadata_rewrite::RewriteRequest;

fn main() {
    let directory = PathBuf::from(std::env::args_os().nth(1).expect("output directory"));
    std::fs::create_dir_all(&directory).unwrap();
    let pixels: Vec<u8> = (0..16 * 16 * 3)
        .map(|i| ((i * 13 + i / 17) % 256) as u8)
        .collect();
    let encoded = zenjxl::LosslessConfig::new()
        .with_effort(3)
        .encode(&pixels, 16, 16, zenjxl::PixelLayout::Rgb8)
        .unwrap();
    let mut input = if encoded.starts_with(b"\xff\x0a") {
        let mut container = b"\0\0\0\x0cJXL \r\n\x87\n".to_vec();
        add_box(&mut container, b"ftyp", b"jxl \0\0\0\0jxl ");
        add_box(&mut container, b"jxlc", &encoded);
        container
    } else {
        encoded
    };
    add_box(&mut input, b"xml ", b"<private>SECRET</private>");
    let output = zenjxl::metadata::rewrite(&RewriteRequest::new(&input))
        .unwrap()
        .to_vec()
        .unwrap();
    assert!(!output.windows(6).any(|b| b == b"SECRET"));
    let before = zenjxl::decode(&input, None, &[]).unwrap();
    let after = zenjxl::decode(&output, None, &[]).unwrap();
    assert_eq!(
        before.pixels.as_contiguous_bytes(),
        after.pixels.as_contiguous_bytes()
    );
    assert_eq!(before.info.icc_profile, after.info.icc_profile);
    std::fs::write(directory.join("before.jxl"), input).unwrap();
    std::fs::write(directory.join("after.jxl"), output).unwrap();
}
fn add_box(output: &mut Vec<u8>, kind: &[u8; 4], payload: &[u8]) {
    output.extend_from_slice(&(payload.len() as u32 + 8).to_be_bytes());
    output.extend_from_slice(kind);
    output.extend_from_slice(payload);
}
