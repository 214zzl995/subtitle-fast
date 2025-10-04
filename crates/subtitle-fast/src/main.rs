use std::ffi::OsString;
use std::path::PathBuf;
use std::str::FromStr;

use subtitle_fast_decoder::{Backend, Configuration, YPlaneError};
use tokio_stream::StreamExt;

fn usage() {
    println!("usage: subtitle-fast [--backend <name>] <video-path>");
    println!("       subtitle-fast --list-backends");
    print_available_backends();
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), YPlaneError> {
    let mut args = std::env::args_os();
    let _ = args.next();
    let mut backend_override = None;
    let mut input_path = None;

    while let Some(arg) = args.next() {
        if arg == "--help" || arg == "-h" {
            usage();
            return Ok(());
        } else if arg == "--list-backends" {
            print_available_backends();
            return Ok(());
        } else if arg == "--backend" || arg == "-b" {
            let value = args
                .next()
                .ok_or_else(|| YPlaneError::configuration("--backend requires a backend name"))?;
            backend_override = Some(parse_backend(value)?);
        } else if let Some(value) = arg
            .to_str()
            .and_then(|s| s.strip_prefix("--backend="))
            .map(|s| s.to_owned())
        {
            backend_override = Some(parse_backend(OsString::from(value))?);
        } else if input_path.is_none() {
            input_path = Some(PathBuf::from(arg));
        } else {
            return Err(YPlaneError::configuration(
                "multiple input paths provided; only one is supported",
            ));
        }
    }

    let input = match input_path {
        Some(path) => path,
        None => {
            usage();
            return Ok(());
        }
    };

    let mut config = Configuration::from_env().unwrap_or_default();
    if let Some(backend) = backend_override {
        config.backend = backend;
    }
    config.input = Some(input);

    let available = Configuration::available_backends();
    if available.is_empty() {
        return Err(YPlaneError::configuration(
            "no decoding backend available; rebuild with a backend feature such as \"backend-ffmpeg\"",
        ));
    }
    if !available.contains(&config.backend) {
        return Err(YPlaneError::unsupported(config.backend.as_str()));
    }

    let provider = config.create_provider()?;
    let mut stream = provider.into_stream();

    while let Some(frame) = stream.next().await {
        frame?;
    }

    Ok(())
}

fn parse_backend(value: OsString) -> Result<Backend, YPlaneError> {
    let value = value
        .into_string()
        .map_err(|_| YPlaneError::configuration("backend name must be valid UTF-8"))?;
    Backend::from_str(&value)
}

fn print_available_backends() {
    let names: Vec<&'static str> = Configuration::available_backends()
        .iter()
        .map(Backend::as_str)
        .collect();
    if names.is_empty() {
        println!("available backends: (none compiled)");
    } else {
        println!("available backends: {}", names.join(", "));
    }
}
