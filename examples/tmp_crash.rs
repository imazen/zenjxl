use jxl::api::{JxlDecoder, JxlDecoderOptions, ProcessingResult, states};
fn run(data: &[u8], scan: bool) -> String {
    let o = JxlDecoderOptions::default().with_scan_frames_only(scan);
    let mut input: &[u8] = data;
    let d = JxlDecoder::<states::Initialized>::new(o);
    let mut d = match d.process(&mut input) { Ok(ProcessingResult::Complete { result }) => result, Ok(_) => return "needs more (header)".into(), Err(e) => return format!("err {e}") };
    while d.has_more_frames() {
        let w = match d.process(&mut input) { Ok(ProcessingResult::Complete { result }) => result, Ok(_) => return "needs more".into(), Err(e) => return format!("err {e}") };
        d = match w.skip_frame(&mut input) { Ok(ProcessingResult::Complete { result }) => result, Ok(_) => return "needs more".into(), Err(e) => return format!("err {e}") };
    }
    "ok".into()
}
fn main() {
    let data = std::fs::read(std::env::args().nth(1).unwrap()).unwrap();
    for scan in [false, true] {
        let r = std::panic::catch_unwind(|| run(&data, scan));
        println!("scan={scan}: {:?}", r.map_err(|_| "PANIC"));
    }
    let r = std::panic::catch_unwind(|| zenjxl::decode(&data, None, &[]).map(|_| ()).map_err(|e| e.to_string()));
    println!("zenjxl::decode: {:?}", r.map_err(|_| "PANIC"));
}
