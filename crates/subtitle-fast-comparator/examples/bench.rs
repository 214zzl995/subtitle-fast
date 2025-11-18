use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use subtitle_fast_comparator::{
    ComparatorFactory, ComparatorKind, ComparatorSettings, PreprocessSettings,
};

#[path = "common/roi_examples.rs"]
mod roi_examples;

use roi_examples::{load_frame, load_rois};

const YUV_DIR: &str = "./demo/decoder/yuv";
const ROI_DIR: &str = "./demo/validator/projection";
const COMPARATORS: &[ComparatorKind] = &[ComparatorKind::SparseChamfer];

fn main() -> Result<(), Box<dyn Error>> {
    let json_files = collect_roi_json(ROI_DIR)?;
    if json_files.is_empty() {
        return Err("no ROI JSON files found for benchmark".into());
    }

    let style = ProgressStyle::with_template(
        "{spinner:.green} [{elapsed_precise}] {prefix:>10.magenta.bold} \
{bar:40.magenta/blue} {pos:>4}/{len:4} frames avg={msg}ms",
    )?
    .progress_chars("█▉▊▋▌▍▎▏  ");
    let multi = MultiProgress::new();

    let mut handles = Vec::new();

    for &kind in COMPARATORS {
        let json_files = json_files.clone();
        let bar = multi.add(ProgressBar::new(json_files.len() as u64));
        bar.set_style(style.clone());
        bar.set_prefix(kind.as_str().to_string());
        bar.set_message("0.000");

        let handle = thread::spawn(move || -> Result<(ComparatorKind, u64, f64), String> {
            match run_comparator_bench(&json_files, kind, bar) {
                Ok((pairs, avg_ms)) => Ok((kind, pairs, avg_ms)),
                Err(err) => Err(format!("comparator '{}' failed: {err}", kind.as_str())),
            }
        });

        handles.push(handle);
    }

    let mut results = Vec::new();
    for handle in handles {
        match handle.join() {
            Ok(Ok((kind, pairs, avg_ms))) => {
                results.push((kind, pairs, avg_ms));
            }
            Ok(Err(err)) => {
                eprintln!("comparator worker failed: {err}");
            }
            Err(_) => {
                eprintln!("comparator worker panicked");
            }
        }
    }

    if results.is_empty() {
        return Err("no comparator benchmark results produced".into());
    }

    println!(
        "\nComparator benchmark summary over ROI data in {:?}:",
        ROI_DIR
    );
    for (kind, pairs, avg_ms) in results {
        println!(
            "  {:>16}: comparisons={} avg={avg_ms:.3}ms/comparison",
            kind.as_str(),
            pairs
        );
    }

    Ok(())
}

fn run_comparator_bench(
    json_files: &[PathBuf],
    kind: ComparatorKind,
    bar: ProgressBar,
) -> Result<(u64, f64), Box<dyn Error>> {
    let mut total_pairs = 0u64;
    let mut total_time = Duration::from_secs(0);

    for json_path in json_files {
        let stem = json_path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or("failed to read JSON file name")?;
        let yuv_path = PathBuf::from(YUV_DIR).join(format!("{stem}.yuv"));
        if !yuv_path.exists() {
            eprintln!("skipping frame {stem}: missing YUV at {:?}", yuv_path);
            bar.inc(1);
            continue;
        }

        let selection = load_rois(json_path)?;
        let preprocess = PreprocessSettings {
            target: selection.luma_band.target,
            delta: selection.luma_band.delta,
        };
        let comparator = ComparatorFactory::new(ComparatorSettings {
            kind,
            target: preprocess.target,
            delta: preprocess.delta,
        })
        .build();

        let frame = load_frame(&yuv_path, selection.frame_width, selection.frame_height)?;

        for entry in &selection.regions {
            let Some(feature) = comparator.extract(&frame, &entry.roi) else {
                continue;
            };
            let start = Instant::now();
            let _report = comparator.compare(&feature, &feature);
            let elapsed = start.elapsed();
            total_time += elapsed;
            total_pairs += 1;
        }

        bar.inc(1);
        if total_pairs > 0 {
            let avg_ms = total_time.as_secs_f64() * 1000.0 / total_pairs as f64;
            bar.set_message(format!("{avg_ms:.3}"));
        }
    }

    bar.finish_with_message("done");

    if total_pairs == 0 {
        return Err("no comparisons performed in benchmark".into());
    }

    let avg_ms = total_time.as_secs_f64() * 1000.0 / total_pairs as f64;
    Ok((total_pairs, avg_ms))
}

fn collect_roi_json(dir: &str) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(Path::new(dir))? {
        let path = entry?.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}
