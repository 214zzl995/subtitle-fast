use subtitle_fast_decoder::{Backend, Configuration, FrameError, OutputFormat};

#[test]
fn handle_output_rejects_non_videotoolbox_backend() {
    let config = Configuration {
        backend: Backend::Mock,
        input: None,
        channel_capacity: None,
        output_format: OutputFormat::CVPixelBuffer,
        start_frame: None,
    };

    let err = match config.create_provider() {
        Ok(_) => panic!("expected output format validation to fail"),
        Err(err) => err,
    };

    match err {
        FrameError::Configuration { message } => {
            assert!(message.contains("videotoolbox"));
            assert!(message.contains(OutputFormat::CVPixelBuffer.as_str()));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}
