use std::path::PathBuf;
use subtitle_fast_decoder::{Backend, Configuration};

const VIDEO_FILE: &str = "demo/video1_30s.mp4";
const BACKEND: Backend = Backend::FFmpeg;

#[tokio::main]
async fn main() {
    let input_path = PathBuf::from(VIDEO_FILE);

    let config = Configuration {
        backend: BACKEND,
        input: Some(input_path),
        channel_capacity: None,
    };

    match config.create_provider() {
        Ok(provider) => {
            let metadata = provider.metadata();

            println!("Backend: {}", BACKEND.as_str());
            println!("Duration: {:?}", metadata.duration);
            println!("FPS: {:?}", metadata.fps);
            println!("Width: {:?}", metadata.width);
            println!("Height: {:?}", metadata.height);
            println!("Total Frames: {:?}", metadata.total_frames);
        }
        Err(err) => {
            eprintln!("Failed to create provider: {}", err);
        }
    }
}
