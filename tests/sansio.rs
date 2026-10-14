//! Incremental (sans-I/O) decode through `dyn_incremental_decoder`: the
//! testkit's incremental contract on lossy and lossless encodes.
#![cfg(all(feature = "encode", feature = "decode", feature = "zencodec"))]

use zencodec::decode::DecoderConfig as _;
use zencodec::encode::{EncodeJob as _, EncoderConfig as _};
use zencodec_testkit as tk;
use zenjxl::{JxlDecoderConfig, JxlEncoderConfig};

#[test]
fn incremental_contract_holds() {
    assert!(JxlDecoderConfig::capabilities().incremental());
    for enc in [
        JxlEncoderConfig::new(),
        JxlEncoderConfig::new().with_lossless(true),
    ] {
        for img in [
            tk::TestImage::rgb8_gradient(37, 21),
            tk::TestImage::rgba8_gradient(64, 48),
        ] {
            let bytes = enc.clone().job().encode(img.as_slice()).unwrap();
            tk::check_incremental_capability_honesty(JxlDecoderConfig::new(), bytes.data())
                .unwrap();
        }
    }
}
