//! Emit an animation for independent timing inspection.
use zencodec::animation::FrameDuration;
use zencodec::encode::{AnimationFrameEncoder as _, EncodeJob as _, EncoderConfig as _};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os().nth(1).expect("output path required");
    let mut encoder = zenjxl::JxlEncoderConfig::new()
        .with_lossless(true)
        .job()
        .animation_frame_encoder()?;
    for index in 0..3 {
        let pixels = zenpixels::PixelBuffer::<rgb::Rgb<u8>>::from_pixels(
            vec![rgb::Rgb::new(index * 40, 20, 60); 16 * 16],
            16,
            16,
        )?;
        encoder.push_frame_timed(pixels.as_slice().into(), FrameDuration::new(1, 30)?, None)?;
    }
    std::fs::write(path, encoder.finish(None)?.into_vec())?;
    Ok(())
}
