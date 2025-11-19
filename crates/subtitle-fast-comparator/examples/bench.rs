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
const COMPARATORS: &[ComparatorKind] =
    &[ComparatorKind::SparseChamfer, ComparatorKind::BitsetCover];

#[derive(Debug, Clone, Copy)]
struct BenchStats {
    frames: u64,
    comparisons: u64,
    avg_extract_ms: f64,
    avg_compare_ms: f64,
    avg_total_ms: f64,
    avg_frame_ms: f64,
}

fn main() -> Result<(), Box<dyn Error>> {
    let json_files = collect_roi_json(ROI_DIR)?;
    if json_files.is_empty() {
        return Err("no ROI JSON files found for benchmark".into());
    }

    let style = ProgressStyle::with_template(
        "{spinner:.green} [{elapsed_precise}] {prefix:>10.magenta.bold} \
{bar:40.magenta/blue} {pos:>4}/{len:4} frames {msg}",
    )?
    .progress_chars("█▉▊▋▌▍▎▏  ");
    let multi = MultiProgress::new();

    let mut handles = Vec::new();

    for &kind in COMPARATORS {
        let json_files = json_files.clone();
        let bar = multi.add(ProgressBar::new(json_files.len() as u64));
        bar.set_style(style.clone());
        bar.set_prefix(kind.as_str().to_string());
        bar.set_message("roi ext=0.000ms cmp=0.000ms tot=0.000ms");

        let handle = thread::spawn(move || -> Result<(ComparatorKind, BenchStats), String> {
            match run_comparator_bench(&json_files, kind, bar) {
                Ok(stats) => Ok((kind, stats)),
                Err(err) => Err(format!("comparator '{}' failed: {err}", kind.as_str())),
            }
        });

        handles.push(handle);
    }

    let mut results = Vec::new();
    for handle in handles {
        match handle.join() {
            Ok(Ok((kind, stats))) => {
                results.push((kind, stats));
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
    for (kind, stats) in results {
        println!(
            "  {:>16}: frames={} comparisons={} per_roi: ext={:.3}ms cmp={:.3}ms tot={:.3}ms per_frame_tot={:.3}ms",
            kind.as_str(),
            stats.frames,
            stats.comparisons,
            stats.avg_extract_ms,
            stats.avg_compare_ms,
            stats.avg_total_ms,
            stats.avg_frame_ms,
        );
    }

    Ok(())
}

fn run_comparator_bench(
    json_files: &[PathBuf],
    kind: ComparatorKind,
    bar: ProgressBar,
) -> Result<BenchStats, Box<dyn Error>> {
    let mut frames = 0u64;
    let mut total_pairs = 0u64;
    let mut total_extract = Duration::from_secs(0);
    let mut total_compare = Duration::from_secs(0);

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
        frames += 1;

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
            let start_extract = Instant::now();
            let Some(feature) = comparator.extract(&frame, &entry.roi) else {
                continue;
            };
            total_extract += start_extract.elapsed();

            let start = Instant::now();
            let _report = comparator.compare(&feature, &feature);
            total_compare += start.elapsed();
            total_pairs += 1;
        }

        bar.inc(1);
        if total_pairs > 0 {
            let avg_extract = total_extract.as_secs_f64() * 1000.0 / total_pairs as f64;
            let avg_compare = total_compare.as_secs_f64() * 1000.0 / total_pairs as f64;
            let avg_total = avg_extract + avg_compare;
            bar.set_message(format!(
                "roi ext={avg_extract:.3}ms cmp={avg_compare:.3}ms tot={avg_total:.3}ms"
            ));
        }
    }

    bar.finish_with_message("done");

    if total_pairs == 0 {
        return Err("no comparisons performed in benchmark".into());
    }

    let total_time = total_extract + total_compare;
    let avg_extract_ms = total_extract.as_secs_f64() * 1000.0 / total_pairs as f64;
    let avg_compare_ms = total_compare.as_secs_f64() * 1000.0 / total_pairs as f64;
    let avg_total_ms = avg_extract_ms + avg_compare_ms;
    let avg_frame_ms = if frames > 0 {
        total_time.as_secs_f64() * 1000.0 / frames as f64
    } else {
        0.0
    };

    Ok(BenchStats {
        frames,
        comparisons: total_pairs,
        avg_extract_ms,
        avg_compare_ms,
        avg_total_ms,
        avg_frame_ms,
    })
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
