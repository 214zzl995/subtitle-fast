use std::error::Error;
use std::path::PathBuf;

use subtitle_fast_comparator::{
    ComparatorFactory, ComparatorKind, ComparatorSettings, PreprocessSettings,
};
#[path = "common/roi_examples.rs"]
mod roi_examples;
use roi_examples::{debug_features, load_frame, load_rois, mask_stats};

const YUV_A_PATH: &str = "./demo/decoder/yuv/00010.yuv";
const YUV_B_PATH: &str = "./demo/decoder/yuv/00010.yuv";
const ROI_JSON_PATH: &str = "./demo/validator/projection/00010.json";
const COMPARATOR: ComparatorKind = ComparatorKind::SparseChamfer;

fn main() -> Result<(), Box<dyn Error>> {
    let yuv_a = PathBuf::from(YUV_A_PATH);
    let yuv_b = PathBuf::from(YUV_B_PATH);
    let roi_json = PathBuf::from(ROI_JSON_PATH);

    let selections = load_rois(&roi_json)?;

    let frame_a = load_frame(&yuv_a, selections.frame_width, selections.frame_height)?;
    let frame_b = load_frame(&yuv_b, selections.frame_width, selections.frame_height)?;

    let preprocess = PreprocessSettings {
        target: selections.luma_band.target,
        delta: selections.luma_band.delta,
    };
    let comparator = ComparatorFactory::new(ComparatorSettings {
        kind: COMPARATOR,
        target: preprocess.target,
        delta: preprocess.delta,
    })
    .build();

    println!("Comparator      : {}", COMPARATOR.as_str());
    println!("YUV A           : {:?}", yuv_a);
    println!("YUV B           : {:?}", yuv_b);
    println!(
        "Luma band       : target={}, delta={} (from JSON)",
        preprocess.target, preprocess.delta
    );

    for entry in &selections.regions {
        let Some(feature_a) = comparator.extract(&frame_a, &entry.roi) else {
            println!(
                "[{}] skipped: failed to extract features from first frame (ROI may be empty)",
                entry.description
            );
            if let Some((on, total, min, max)) = mask_stats(&frame_a, &entry.roi, preprocess) {
                println!(
                    "    mask coverage={on}/{total} ({:.2}%), luma min/max={:.3}/{:.3}",
                    on as f32 * 100.0 / total as f32,
                    min,
                    max
                );
            }
            if let Some(diag) = debug_features(&frame_a, &entry.roi, preprocess) {
                println!(
                    "    mask(after morph)={}/{} edges={} sampled_points={}",
                    diag.mask_on, diag.mask_total, diag.edge_count, diag.sampled_points
                );
            }
            continue;
        };
        let Some(feature_b) = comparator.extract(&frame_b, &entry.roi) else {
            println!(
                "[{}] skipped: failed to extract features from second frame (ROI may be empty)",
                entry.description
            );
            if let Some((on, total, min, max)) = mask_stats(&frame_b, &entry.roi, preprocess) {
                println!(
                    "    mask coverage={on}/{total} ({:.2}%), luma min/max={:.3}/{:.3}",
                    on as f32 * 100.0 / total as f32,
                    min,
                    max
                );
            }
            if let Some(diag) = debug_features(&frame_b, &entry.roi, preprocess) {
                println!(
                    "    mask(after morph)={}/{} edges={} sampled_points={}",
                    diag.mask_on, diag.mask_total, diag.edge_count, diag.sampled_points
                );
            }
            continue;
        };
        let report = comparator.compare(&feature_a, &feature_b);
        println!(
            "[{}] ROI x={:.4}, y={:.4}, w={:.4}, h={:.4}",
            entry.description, entry.roi.x, entry.roi.y, entry.roi.width, entry.roi.height
        );
        println!(
            "    similarity: {:.4} (same_segment = {})",
            report.similarity, report.same_segment
        );
        if !report.details.is_empty() {
            for metric in &report.details {
                println!("    {:18}: {}", metric.name, format_metric(metric.value));
            }
        }
    }

    Ok(())
}

fn format_metric(value: f32) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else if value.abs() >= 10.0 {
        format!("{value:.2}")
    } else {
        format!("{value:.4}")
    }
}
