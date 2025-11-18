use std::collections::BTreeMap;
use std::error::Error;
use std::fs::{self, File};
use std::io::BufWriter;
use std::path::PathBuf;

use indicatif::{ProgressBar, ProgressStyle};
use serde::Serialize;
use serde_json::to_writer_pretty;
use subtitle_fast_comparator::{
    ComparatorFactory, ComparatorKind, ComparatorSettings, PreprocessSettings,
};

#[path = "common/roi_examples.rs"]
mod roi_examples;

use roi_examples::{load_frame, load_rois};

const YUV_DIR: &str = "./demo/decoder/yuv";
const ROI_DIR: &str = "./demo/validator/projection";
const OUTPUT_DIR: &str = "./demo/comparator";
const DUMP_FILE: &str = "./demo/comparator/comparator_dump.json";
const MAX_FRAMES: usize = 100;
const COMPARATOR: ComparatorKind = ComparatorKind::SparseChamfer;

#[derive(Serialize)]
struct RoiResultDump {
    description: String,
    similarity: f32,
    same_segment: bool,
    metrics: BTreeMap<String, f32>,
}

#[derive(Serialize)]
struct FramePairDump {
    prev_frame: String,
    curr_frame: String,
    skipped: Option<String>,
    roi_results: Vec<RoiResultDump>,
}

#[derive(Serialize)]
struct ComparatorDump {
    comparator: String,
    yuv_dir: String,
    roi_dir: String,
    frame_pairs: usize,
    pairs: Vec<FramePairDump>,
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut frames = collect_yuv_frames(YUV_DIR)?;
    if frames.len() < 2 {
        return Err("need at least two YUV frames for comparison".into());
    }

    if frames.len() > MAX_FRAMES {
        frames.truncate(MAX_FRAMES);
    }

    let total_pairs = frames.len() - 1;
    let progress = ProgressBar::new(total_pairs as u64);
    let style = ProgressStyle::with_template(
        "{spinner:.green} [{elapsed_precise}] {prefix:>10.cyan.bold} \
{bar:40.cyan/blue} {pos:>4}/{len:4} frame-pairs",
    )?
    .progress_chars("█▉▊▋▌▍▎▏  ");
    progress.set_style(style);
    progress.set_prefix(COMPARATOR.as_str().to_string());

    let mut pairs = Vec::new();

    for pair_idx in 1..frames.len() {
        let prev_frame_path = &frames[pair_idx - 1];
        let curr_frame_path = &frames[pair_idx];

        let prev_name = prev_frame_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        let curr_name = curr_frame_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();

        let stem = curr_frame_path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or("failed to read frame file name")?;
        let roi_path = PathBuf::from(ROI_DIR).join(format!("{stem}.json"));

        if !roi_path.exists() {
            pairs.push(FramePairDump {
                prev_frame: prev_name,
                curr_frame: curr_name,
                skipped: Some(format!("missing ROI JSON at {:?}", roi_path)),
                roi_results: Vec::new(),
            });
            progress.inc(1);
            continue;
        }

        let selection = load_rois(&roi_path)?;
        let preprocess = PreprocessSettings {
            target: selection.luma_band.target,
            delta: selection.luma_band.delta,
        };
        let comparator = ComparatorFactory::new(ComparatorSettings {
            kind: COMPARATOR,
            target: preprocess.target,
            delta: preprocess.delta,
        })
        .build();

        let frame_a = load_frame(
            prev_frame_path,
            selection.frame_width,
            selection.frame_height,
        )?;
        let frame_b = load_frame(
            curr_frame_path,
            selection.frame_width,
            selection.frame_height,
        )?;

        let mut roi_results = Vec::new();
        for entry in &selection.regions {
            let Some(feature_a) = comparator.extract(&frame_a, &entry.roi) else {
                continue;
            };
            let Some(feature_b) = comparator.extract(&frame_b, &entry.roi) else {
                continue;
            };

            let report = comparator.compare(&feature_a, &feature_b);
            let metrics = report
                .details
                .iter()
                .map(|metric| (metric.name.to_string(), metric.value))
                .collect::<BTreeMap<_, _>>();
            roi_results.push(RoiResultDump {
                description: entry.description.clone(),
                similarity: report.similarity,
                same_segment: report.same_segment,
                metrics,
            });
        }

        pairs.push(FramePairDump {
            prev_frame: prev_name,
            curr_frame: curr_name,
            skipped: None,
            roi_results,
        });

        progress.inc(1);
    }

    progress.finish_with_message("done");

    fs::create_dir_all(OUTPUT_DIR)?;
    let file = File::create(DUMP_FILE)?;
    let writer = BufWriter::new(file);

    let dump = ComparatorDump {
        comparator: COMPARATOR.as_str().to_string(),
        yuv_dir: YUV_DIR.to_string(),
        roi_dir: ROI_DIR.to_string(),
        frame_pairs: total_pairs,
        pairs,
    };

    to_writer_pretty(writer, &dump)?;

    Ok(())
}

fn collect_yuv_frames(dir: &str) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let mut frames = Vec::new();
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("yuv") {
            frames.push(path);
        }
    }
    frames.sort();
    Ok(frames)
}
