use std::collections::HashMap;
use std::env;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use subtitle_fast_comparator::{
    ComparatorFactory, ComparatorKind, ComparatorSettings, PreprocessSettings,
};

#[path = "common/roi_examples.rs"]
mod roi_examples;
use roi_examples::{RoiEntry, RoiSelection, debug_features, load_frame, load_rois, mask_stats};

const DEFAULT_YUV_DIR: &str = "./demo/decoder/yuv";
const DEFAULT_ROI_DIR: &str = "./demo/validator/projection";
const COMPARATOR: ComparatorKind = ComparatorKind::SparseChamfer;

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = env::args().skip(1);
    let yuv_dir = PathBuf::from(args.next().unwrap_or_else(|| DEFAULT_YUV_DIR.to_string()));
    let roi_dir = PathBuf::from(args.next().unwrap_or_else(|| DEFAULT_ROI_DIR.to_string()));

    let mut roi_files: Vec<PathBuf> = fs::read_dir(&roi_dir)?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("json"))
        .collect();
    roi_files.sort();

    if roi_files.len() < 2 {
        return Err(format!(
            "need at least two ROI JSON files in {:?} to compare adjacent frames",
            roi_dir
        )
        .into());
    }

    let mut selections = Vec::new();
    for path in &roi_files {
        let selection = load_rois(path)?;
        selections.push((path.clone(), selection));
    }

    let mut segments: HashMap<String, SegmentAccumulator> = HashMap::new();

    println!("Comparator      : {}", COMPARATOR.as_str());
    println!("YUV dir         : {:?}", yuv_dir);
    println!("ROI JSON dir    : {:?}", roi_dir);

    for idx in 0..selections.len() - 1 {
        let (json_path, selection) = &selections[idx];
        let (_, next_selection) = &selections[idx + 1];

        let yuv_a = frame_path(json_path, selection, &yuv_dir);
        let yuv_b = frame_path(&roi_files[idx + 1], next_selection, &yuv_dir);

        if !yuv_a.exists() || !yuv_b.exists() {
            println!(
                "[{}] skipped: missing YUV (current exists={}, next exists={})",
                json_path_file(json_path),
                yuv_a.exists(),
                yuv_b.exists()
            );
            continue;
        }

        let frame_a = match load_frame(&yuv_a, selection.frame_width, selection.frame_height) {
            Ok(frame) => frame,
            Err(err) => {
                println!(
                    "[{}] skipped: failed to load {:?} ({err})",
                    json_path_file(json_path),
                    yuv_a
                );
                continue;
            }
        };
        let frame_b = match load_frame(&yuv_b, selection.frame_width, selection.frame_height) {
            Ok(frame) => frame,
            Err(err) => {
                println!(
                    "[{}] skipped: failed to load {:?} ({err})",
                    json_path_file(json_path),
                    yuv_b
                );
                continue;
            }
        };

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

        let start_label = frame_label(&yuv_a);
        let end_label = frame_label(&yuv_b);
        println!(
            "[{} -> {}] luma target={}, delta={}",
            start_label, end_label, preprocess.target, preprocess.delta
        );

        for entry in &selection.regions {
            let acc = segments.entry(entry.description.clone()).or_default();
            match compare_roi(entry, &frame_a, &frame_b, preprocess, comparator.as_ref()) {
                Some(true) => acc.extend(&start_label, &end_label),
                Some(false) => acc.finish_current(),
                None => acc.finish_current(),
            }
        }
    }

    for acc in segments.values_mut() {
        acc.finish_current();
    }

    println!("Segments:");
    if segments.is_empty() {
        println!("  (no ROI segments detected)");
    } else {
        let mut keys: Vec<_> = segments.keys().cloned().collect();
        keys.sort();
        for key in keys {
            let acc = &segments[&key];
            if acc.segments.is_empty() {
                println!("  [{}] none", key);
            } else {
                for seg in &acc.segments {
                    println!("  [{}] {} -> {}", key, seg.start, seg.end);
                }
            }
        }
    }

    Ok(())
}

fn compare_roi(
    entry: &RoiEntry,
    frame_a: &subtitle_fast_decoder::YPlaneFrame,
    frame_b: &subtitle_fast_decoder::YPlaneFrame,
    settings: PreprocessSettings,
    comparator: &dyn subtitle_fast_comparator::SubtitleComparator,
) -> Option<bool> {
    let Some(feature_a) = comparator.extract(frame_a, &entry.roi) else {
        println!(
            "    [{}] skipped: failed to extract features from first frame (ROI may be empty)",
            entry.description
        );
        if let Some((on, total, min, max)) = mask_stats(frame_a, &entry.roi, settings) {
            println!(
                "        mask coverage={on}/{total} ({:.2}%), luma min/max={:.3}/{:.3}",
                on as f32 * 100.0 / total as f32,
                min,
                max
            );
        }
        if let Some(diag) = debug_features(frame_a, &entry.roi, settings) {
            println!(
                "        mask(after morph)={}/{} edges={} sampled_points={}",
                diag.mask_on, diag.mask_total, diag.edge_count, diag.sampled_points
            );
        }
        return None;
    };
    let Some(feature_b) = comparator.extract(frame_b, &entry.roi) else {
        println!(
            "    [{}] skipped: failed to extract features from second frame (ROI may be empty)",
            entry.description
        );
        if let Some((on, total, min, max)) = mask_stats(frame_b, &entry.roi, settings) {
            println!(
                "        mask coverage={on}/{total} ({:.2}%), luma min/max={:.3}/{:.3}",
                on as f32 * 100.0 / total as f32,
                min,
                max
            );
        }
        if let Some(diag) = debug_features(frame_b, &entry.roi, settings) {
            println!(
                "        mask(after morph)={}/{} edges={} sampled_points={}",
                diag.mask_on, diag.mask_total, diag.edge_count, diag.sampled_points
            );
        }
        return None;
    };

    let report = comparator.compare(&feature_a, &feature_b);
    println!(
        "    [{}] similarity={:.4} same_segment={}",
        entry.description, report.similarity, report.same_segment
    );
    Some(report.same_segment)
}

fn frame_path(json_path: &Path, selection: &RoiSelection, yuv_dir: &Path) -> PathBuf {
    if let Some(source) = selection.source_file_name() {
        return yuv_dir.join(source);
    }
    if let Some(stem) = json_path.file_stem().and_then(|s| s.to_str()) {
        return yuv_dir.join(format!("{stem}.yuv"));
    }
    yuv_dir.to_path_buf()
}

fn json_path_file(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

fn frame_label(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

#[derive(Default)]
struct SegmentAccumulator {
    active: Option<Segment>,
    segments: Vec<Segment>,
}

impl SegmentAccumulator {
    fn extend(&mut self, start: &str, end: &str) {
        if let Some(active) = &mut self.active {
            active.end = end.to_string();
        } else {
            self.active = Some(Segment {
                start: start.to_string(),
                end: end.to_string(),
            });
        }
    }

    fn finish_current(&mut self) {
        if let Some(active) = self.active.take() {
            self.segments.push(active);
        }
    }
}

#[derive(Clone)]
struct Segment {
    start: String,
    end: String,
}
