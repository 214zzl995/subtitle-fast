use std::error::Error;
use std::path::{Path, PathBuf};
use std::time::Instant;

use indicatif::{ProgressBar, ProgressStyle};
use subtitle_fast_decoder::{Backend, Configuration};
use tokio_stream::StreamExt;

const INPUT_VIDEO: &str = "./demo/video1_30s.mp4";

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let input_path = PathBuf::from(INPUT_VIDEO);
    if !input_path.exists() {
        return Err(format!("input file {:?} does not exist", input_path).into());
    }

    let mut backends = Configuration::available_backends();
    // Skip mock backend; we only care about real decoders here.
    backends.retain(|b| !matches!(b, Backend::Mock));

    if backends.is_empty() {
        return Err(
            "no decoder backend is compiled; enable a backend feature such as backend-ffmpeg"
                .into(),
        );
    }

    let mut results = Vec::new();

    for backend in backends {
        println!(
            "\nRunning decoder benchmark for backend='{}'...",
            backend.as_str()
        );
        match run_backend_bench(&input_path, backend).await {
            Ok((frames, avg_ms)) => {
                results.push((backend, frames, avg_ms));
            }
            Err(err) => {
                eprintln!("backend '{}' failed: {err}", backend.as_str());
            }
        }
    }

    if results.is_empty() {
        return Err("no backends produced benchmark results".into());
    }

    println!("\nDecoder benchmark summary over input {:?}:", input_path);
    for (backend, frames, avg_ms) in results {
        println!(
            "  {:>12}: frames={} avg={avg_ms:.3}ms/frame",
            backend.as_str(),
            frames,
        );
    }

    Ok(())
}

async fn run_backend_bench(
    input_path: &Path,
    backend: Backend,
) -> Result<(u64, f64), Box<dyn Error>> {
    let config = Configuration {
        backend,
        input: Some(input_path.to_path_buf()),
        channel_capacity: None,
    };

    let provider = config.create_provider()?;
    let total_frames = provider.total_frames();

    let progress = total_frames.map(|total| {
        let style = ProgressStyle::with_template(
            "{spinner:.green} [{elapsed_precise}] {prefix:>10.cyan.bold} \
{bar:40.cyan/blue} {pos:>4}/{len:4} frames avg={msg}ms",
        )
        .unwrap()
        .progress_chars("█▉▊▋▌▍▎▏  ");
        let bar = ProgressBar::new(total);
        bar.set_style(style);
        bar.set_prefix(backend.as_str().to_string());
        bar.set_message("0.000");
        bar
    });

    let mut stream = provider.into_stream();
    let mut processed = 0u64;
    let bench_start = Instant::now();

    while let Some(item) = stream.next().await {
        match item {
            Ok(_frame) => {
                processed += 1;

                if let Some(ref bar) = progress {
                    bar.inc(1);
                    let elapsed = bench_start.elapsed();
                    let avg_ms = elapsed.as_secs_f64() * 1000.0 / processed as f64;
                    bar.set_message(format!("{avg_ms:.3}"));
                }
            }
            Err(err) => {
                return Err(err.into());
            }
        }
    }

    if let Some(bar) = progress {
        bar.finish_with_message("done");
    }

    if processed == 0 {
        return Err("no frames decoded".into());
    }

    let total = bench_start.elapsed();
    let avg_ms = total.as_secs_f64() * 1000.0 / processed as f64;

    Ok((processed, avg_ms))
}
