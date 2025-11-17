use std::error::Error;
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use png::{BitDepth, ColorType, Encoder};
use subtitle_fast_decoder::{Backend, Configuration, YPlaneFrame};
use tokio_stream::StreamExt;

const SAMPLE_FREQUENCY_HZ: f32 = 7.0;
const DECODER_BACKEND: Backend = Backend::VideoToolbox;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let repo_root = repo_root();
    let dump_root = repo_root.join("demo").join("decoder_dump");
    let yuv_dir = dump_root.join("yuv");
    let png_dir = dump_root.join("png");
    fs::create_dir_all(&yuv_dir)?;
    fs::create_dir_all(&png_dir)?;

    let input_path = parse_input_path(&repo_root)?;

    let available = Configuration::available_backends();
    if available.is_empty() {
        return Err(
            "no decoder backend is compiled; enable a backend feature such as backend-ffmpeg"
                .into(),
        );
    }

    if !available.contains(&DECODER_BACKEND) {
        return Err(format!(
            "decoder backend '{}' is not compiled in this build",
            DECODER_BACKEND.as_str()
        )
        .into());
    }
    let backend = DECODER_BACKEND;

    let took = SystemTime::now();
    let mut config = Configuration::default();
    config.backend = backend;
    config.input = Some(input_path.clone());
    config.channel_capacity = None;
    let provider = config.create_provider()?;

    write_metadata(&dump_root, &input_path, backend)?;
    println!("Decoding frames from {:?}", input_path);
    println!("Writing YUV files to {:?}", yuv_dir);
    println!("Writing PNG files to {:?}", png_dir);

    let mut stream = provider.into_stream();
    let mut processed = 0u64;
    while let Some(frame) = stream.next().await {
        match frame {
            Ok(frame) => {
                let ordinal = frame.frame_index().unwrap_or(processed);
                processed += 1;
                write_frame_yuv(&frame, &yuv_dir, ordinal)?;
                write_frame_png(&frame, &png_dir, ordinal)?;
                if processed % 25 == 0 {
                    println!("dumped {processed} frames...");
                }
            }
            Err(err) => {
                eprintln!("failed to decode frame: {err}");
                break;
            }
        }
    }
    let elapsed = took.elapsed().unwrap_or_else(|_| Duration::from_secs(0));
    println!(
        "Wrote {processed} frames to {:?} in {:.2?}",
        dump_root, elapsed
    );
    Ok(())
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .to_path_buf()
}

fn parse_input_path(repo_root: &Path) -> Result<PathBuf, Box<dyn Error>> {
    let mut args = std::env::args();
    let _bin = args.next();
    let input = match args.next() {
        Some(path) => PathBuf::from(path),
        None => {
            return Err(
                "usage: cargo run -p subtitle-fast-decoder --example decoder_dump --features backend-ffmpeg -- <input-video>"
                    .into(),
            );
        }
    };
    let path = if input.is_relative() {
        repo_root.join(input)
    } else {
        input
    };
    if !path.exists() {
        return Err(Box::new(io::Error::new(
            io::ErrorKind::NotFound,
            format!("input file {:?} does not exist", path),
        )));
    }
    Ok(path)
}

fn write_metadata(root: &Path, input: &Path, backend: Backend) -> Result<(), io::Error> {
    let mut file = File::create(root.join("decoder_dump.txt"))?;
    writeln!(file, "input={}", input.display())?;
    writeln!(file, "backend={}", backend.as_str())?;
    writeln!(file, "sample_frequency_hz={}", SAMPLE_FREQUENCY_HZ)?;
    writeln!(file, "generated_at={}", timestamp())?;
    Ok(())
}

fn timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn write_frame_yuv(frame: &YPlaneFrame, dir: &Path, index: u64) -> Result<(), io::Error> {
    let file = dir.join(format!("{index:05}.yuv"));
    let data = flatten_y(frame);
    fs::write(file, data)
}

fn write_frame_png(frame: &YPlaneFrame, dir: &Path, index: u64) -> Result<(), io::Error> {
    let width = frame.width();
    let height = frame.height();
    let file = File::create(dir.join(format!("{index:05}.png")))?;
    let writer = BufWriter::new(file);
    let mut encoder = Encoder::new(writer, width, height);
    encoder.set_color(ColorType::Grayscale);
    encoder.set_depth(BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    let data = flatten_y(frame);
    writer.write_image_data(&data)?;
    Ok(())
}

fn flatten_y(frame: &YPlaneFrame) -> Vec<u8> {
    let width = frame.width() as usize;
    let height = frame.height() as usize;
    let stride = frame.stride();
    let data = frame.data();
    let mut out = Vec::with_capacity(width * height);
    for row in 0..height {
        let start = row * stride;
        let end = (start + width).min(data.len());
        if end <= start {
            break;
        }
        out.extend_from_slice(&data[start..end]);
        if end - start < width {
            // Unexpected short row, bail early to avoid repeating data.
            break;
        }
    }
    out
}
