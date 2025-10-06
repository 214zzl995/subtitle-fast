#![cfg(feature = "backend-openh264")]

use std::env;
use std::path::PathBuf;

use subtitle_fast_decoder::{Backend, Configuration};
use tokio_stream::StreamExt;

#[tokio::test(flavor = "multi_thread")]
async fn openh264_backend_decodes_stream() {
    let asset = match env::var("SUBFAST_TEST_ASSET") {
        Ok(value) => PathBuf::from(value),
        Err(_) => {
            eprintln!("skipping openh264 backend test - SUBFAST_TEST_ASSET not set");
            return;
        }
    };

    let mut config = Configuration::default();
    config.backend = Backend::OpenH264;
    config.input = Some(asset);
    let provider = match config.create_provider() {
        Ok(provider) => provider,
        Err(err) => {
            panic!("failed to initialize openh264 backend: {err:?}");
        }
    };

    let total_frames = provider.total_frames();
    let mut stream = provider.into_stream();
    let frame = stream
        .next()
        .await
        .expect("openh264 backend should produce at least one frame");
    let frame = frame.expect("frame decoding should succeed");
    assert!(frame.width() > 0);
    assert!(frame.height() > 0);
    if let Some(total) = total_frames {
        assert!(
            total > 0,
            "openh264 backend should report positive frame count"
        );
    }
}
