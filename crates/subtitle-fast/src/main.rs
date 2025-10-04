use std::path::PathBuf;

use subtitle_fast_decoder::{Configuration, YPlaneError};
use tokio_stream::StreamExt;

fn usage() {
    println!("usage: subtitle-fast <video-path>");
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), YPlaneError> {
    let mut args = std::env::args_os();
    let _ = args.next();
    let input = match args.next() {
        Some(flag) if flag == "--help" || flag == "-h" => {
            usage();
            return Ok(());
        }
        Some(path) => PathBuf::from(path),
        None => {
            usage();
            return Ok(());
        }
    };

    let mut config = Configuration::from_env().unwrap_or_default();
    config.input = Some(input);
    let provider = config.create_provider()?;
    let mut stream = provider.into_stream();

    while let Some(frame) = stream.next().await {
        frame?;
    }

    Ok(())
}
