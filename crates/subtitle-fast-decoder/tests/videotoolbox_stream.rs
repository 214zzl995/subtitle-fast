#![cfg(all(feature = "backend-videotoolbox", target_os = "macos"))]

use std::env;
use std::path::PathBuf;

use subtitle_fast_decoder::{Backend, Configuration, RawFrameFormat};
use tokio_stream::StreamExt;

#[tokio::test(flavor = "multi_thread")]
async fn videotoolbox_backend_streams_frames() {
    let asset = match env::var("SUBFAST_TEST_ASSET") {
        Ok(value) => PathBuf::from(value),
        Err(_) => {
            eprintln!("skipping videotoolbox backend test - SUBFAST_TEST_ASSET not set");
            return;
        }
    };

    let config = Configuration {
        backend: Backend::VideoToolbox,
        input: Some(asset),
        output_format: RawFrameFormat::Y,
        ..Configuration::default()
    };

    let provider = match config.create_provider() {
        Ok(provider) => provider,
        Err(err) => {
            panic!("failed to initialize VideoToolbox backend: {err:?}");
        }
    };

    let total_frames = provider.total_frames();
    let mut stream = provider.into_stream();
    let frame = stream
        .next()
        .await
        .expect("videotoolbox backend should produce at least one frame");
    let frame = frame.expect("frame decoding should succeed");
    assert!(frame.width() > 0);
    assert!(frame.height() > 0);
    if let Some(total) = total_frames {
        assert!(
            total > 0,
            "videotoolbox backend should report positive frame count"
        );
    }
}
