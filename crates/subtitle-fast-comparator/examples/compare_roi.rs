use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use subtitle_fast_comparator::{
    ComparatorFactory, ComparatorKind, ComparatorSettings, PreprocessSettings,
    pipeline::{ops::percentile, ops::sobel_magnitude, preprocess::extract_masked_patch},
};
use subtitle_fast_decoder::YPlaneFrame;
use subtitle_fast_validator::subtitle_detection::RoiConfig;

const DEFAULT_TARGET: u8 = 235;
const DEFAULT_DELTA: u8 = 12;
const GRID_STEP: usize = 2;
const MAX_POINTS: usize = 800;
const YUV_A_PATH: &str = "./demo/decoder/yuv/00010.yuv";
const YUV_B_PATH: &str = "./demo/decoder/yuv/00010.yuv";
const ROI_JSON_PATH: &str = "./demo/validator/projection/00010.json";
const COMPARATOR: ComparatorKind = ComparatorKind::SparseChamfer;

#[derive(Debug, Deserialize)]
struct FrameInfo {
    width: usize,
    height: usize,
}

#[derive(Debug, Deserialize, Clone, Copy)]
struct LumaBand {
    target: u8,
    delta: u8,
}

#[derive(Debug, Deserialize, Clone)]
struct Region {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

#[derive(Debug, Deserialize)]
struct RoiDump {
    frame: FrameInfo,
    #[serde(default)]
    regions: Vec<Region>,
    #[serde(default)]
    roi: Option<Region>,
    #[serde(default)]
    luma_band: Option<LumaBand>,
}

#[derive(Debug)]
struct RoiEntry {
    description: String,
    roi: RoiConfig,
}

#[derive(Debug)]
struct RoiSelection {
    frame_width: usize,
    frame_height: usize,
    luma_band: LumaBand,
    regions: Vec<RoiEntry>,
}

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

fn load_frame(path: &Path, width: usize, height: usize) -> Result<YPlaneFrame, Box<dyn Error>> {
    let data = fs::read(path)?;
    let expected = width
        .checked_mul(height)
        .ok_or("frame dimensions overflowed when computing length")?;
    if data.len() != expected {
        return Err(format!(
            "unexpected Y plane length for {:?}: got {} bytes expected {} ({}x{})",
            path,
            data.len(),
            expected,
            width,
            height
        )
        .into());
    }
    Ok(YPlaneFrame::from_owned(
        width as u32,
        height as u32,
        width,
        None,
        data,
    )?)
}

fn load_rois(path: &Path) -> Result<RoiSelection, Box<dyn Error>> {
    let bytes = fs::read(path)?;
    let dump: RoiDump = serde_json::from_slice(&bytes)?;
    pick_rois(dump).map_err(Into::into)
}

fn pick_rois(dump: RoiDump) -> Result<RoiSelection, String> {
    if dump.frame.width == 0 || dump.frame.height == 0 {
        return Err("frame dimensions in JSON are zero".into());
    }
    let band = dump.luma_band.unwrap_or(LumaBand {
        target: DEFAULT_TARGET,
        delta: DEFAULT_DELTA,
    });
    let mut regions = Vec::new();
    if !dump.regions.is_empty() {
        for (idx, region) in dump.regions.iter().enumerate() {
            regions.push(RoiEntry {
                roi: normalize_region(region, &dump.frame, false),
                description: format!("regions[{idx}]"),
            });
        }
    }
    if regions.is_empty() {
        if let Some(roi) = dump.roi {
            regions.push(RoiEntry {
                roi: normalize_region(&roi, &dump.frame, true),
                description: "roi".into(),
            });
        }
    }
    if regions.is_empty() {
        return Err("no regions available in JSON and no 'roi' fallback".into());
    }
    Ok(RoiSelection {
        frame_width: dump.frame.width,
        frame_height: dump.frame.height,
        luma_band: band,
        regions,
    })
}

fn normalize_region(region: &Region, frame: &FrameInfo, already_normalized: bool) -> RoiConfig {
    if already_normalized {
        return RoiConfig {
            x: region.x,
            y: region.y,
            width: region.width,
            height: region.height,
        };
    }
    let frame_w = frame.width.max(1) as f32;
    let frame_h = frame.height.max(1) as f32;
    RoiConfig {
        x: region.x / frame_w,
        y: region.y / frame_h,
        width: region.width / frame_w,
        height: region.height / frame_h,
    }
}

fn mask_stats(
    frame: &YPlaneFrame,
    roi: &RoiConfig,
    settings: PreprocessSettings,
) -> Option<(usize, usize, f32, f32)> {
    let patch = extract_masked_patch(frame, roi, settings)?;
    let total = patch.len();
    if total == 0 {
        return None;
    }
    let on = patch.mask.iter().filter(|&&m| m >= 0.5).count();
    let mut min = 1.0f32;
    let mut max = 0.0f32;
    for &v in &patch.original {
        min = min.min(v);
        max = max.max(v);
    }
    let magnitude = sobel_magnitude(&patch.original, patch.width, patch.height);
    let mut masked_values = Vec::new();
    for (idx, &m) in magnitude.iter().enumerate() {
        if patch.mask[idx] >= 0.5 {
            masked_values.push(m);
        }
    }
    let threshold = if !masked_values.is_empty() {
        percentile(&masked_values, 0.7)
    } else {
        percentile(&magnitude, 0.7)
    };
    let mut sampled = 0usize;
    for y in (0..patch.height).step_by(2) {
        for x in (0..patch.width).step_by(2) {
            let idx = y * patch.width + x;
            if patch.mask[idx] >= 0.5 && magnitude[idx] >= threshold {
                sampled += 1;
                if sampled >= 800 {
                    break;
                }
            }
        }
        if sampled >= 800 {
            break;
        }
    }
    println!(
        "    edges: masked_pixels={} threshold={:.4} sampled_points={}",
        masked_values.len(),
        threshold,
        sampled
    );
    Some((on, total, min, max))
}

struct FeatureDiag {
    mask_on: usize,
    mask_total: usize,
    edge_count: usize,
    sampled_points: usize,
}

fn debug_features(
    frame: &YPlaneFrame,
    roi: &RoiConfig,
    settings: PreprocessSettings,
) -> Option<FeatureDiag> {
    let patch = extract_masked_patch(frame, roi, settings)?;
    if patch.len() < 16 {
        return None;
    }
    let base: Vec<u8> = patch
        .mask
        .iter()
        .map(|&v| if v >= 0.5 { 1 } else { 0 })
        .collect();
    if base.is_empty() {
        return None;
    }
    let mut mask = base.clone();
    mask = erode(&mask, patch.width, patch.height, 1);
    mask = dilate(&mask, patch.width, patch.height, 1);
    mask = dilate(&mask, patch.width, patch.height, 1);
    mask = erode(&mask, patch.width, patch.height, 1);
    if !mask.iter().any(|&v| v > 0) {
        mask = base;
    }
    let mask_on = mask.iter().filter(|&&v| v > 0).count();
    let mask_total = mask.len();

    let magnitude = sobel_magnitude(&patch.original, patch.width, patch.height);
    let mut masked_values = Vec::new();
    for (idx, &m) in magnitude.iter().enumerate() {
        if mask[idx] > 0 {
            masked_values.push(m);
        }
    }
    let threshold = if !masked_values.is_empty() {
        percentile(&masked_values, 0.7)
    } else {
        percentile(&magnitude, 0.7)
    };
    let mut edge_count = 0usize;
    let mut sampled = 0usize;
    for y in (0..patch.height).step_by(GRID_STEP) {
        for x in (0..patch.width).step_by(GRID_STEP) {
            let idx = y * patch.width + x;
            if mask[idx] > 0 && magnitude[idx] >= threshold {
                edge_count += 1;
                sampled += 1;
                if sampled >= MAX_POINTS {
                    break;
                }
            }
        }
        if sampled >= MAX_POINTS {
            break;
        }
    }
    Some(FeatureDiag {
        mask_on,
        mask_total,
        edge_count,
        sampled_points: sampled,
    })
}

fn erode(mask: &[u8], width: usize, height: usize, iterations: usize) -> Vec<u8> {
    let mut current = mask.to_vec();
    let mut next = vec![0u8; mask.len()];
    for _ in 0..iterations {
        for y in 0..height {
            for x in 0..width {
                let mut value = 1u8;
                'outer: for ky in y.saturating_sub(1)..=(y + 1).min(height - 1) {
                    for kx in x.saturating_sub(1)..=(x + 1).min(width - 1) {
                        if current[ky * width + kx] == 0 {
                            value = 0;
                            break 'outer;
                        }
                    }
                }
                next[y * width + x] = value;
            }
        }
        current.copy_from_slice(&next);
    }
    current
}

fn dilate(mask: &[u8], width: usize, height: usize, iterations: usize) -> Vec<u8> {
    let mut current = mask.to_vec();
    let mut next = vec![0u8; mask.len()];
    for _ in 0..iterations {
        for y in 0..height {
            for x in 0..width {
                let mut value = 0u8;
                'outer: for ky in y.saturating_sub(1)..=(y + 1).min(height - 1) {
                    for kx in x.saturating_sub(1)..=(x + 1).min(width - 1) {
                        if current[ky * width + kx] > 0 {
                            value = 1;
                            break 'outer;
                        }
                    }
                }
                next[y * width + x] = value;
            }
        }
        current.copy_from_slice(&next);
    }
    current
}
