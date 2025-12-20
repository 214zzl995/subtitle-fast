use std::error::Error;
use std::fs;
use std::path::Path;

use serde::Deserialize;
use subtitle_fast_comparator::PreprocessSettings;
use subtitle_fast_comparator::pipeline::{
    ops::percentile, ops::percentile_in_place, ops::sobel_magnitude,
    preprocess::extract_masked_patch,
};
use subtitle_fast_types::{PlaneFrame, RoiConfig};

pub const DEFAULT_TARGET: u8 = 235;
pub const DEFAULT_DELTA: u8 = 12;
#[allow(dead_code)]
const GRID_STEP: usize = 2;
#[allow(dead_code)]
const MAX_POINTS: usize = 800;

#[derive(Debug, Deserialize, Clone, Copy)]
pub struct LumaBand {
    pub target: u8,
    pub delta: u8,
}

#[derive(Debug, Deserialize)]
struct FrameInfo {
    width: usize,
    height: usize,
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
    #[serde(default)]
    source: Option<String>,
}

#[derive(Debug)]
pub struct RoiEntry {
    #[allow(dead_code)]
    pub description: String,
    pub roi: RoiConfig,
}

#[derive(Debug)]
pub struct RoiSelection {
    pub frame_width: usize,
    pub frame_height: usize,
    pub luma_band: LumaBand,
    pub regions: Vec<RoiEntry>,
    #[allow(dead_code)]
    pub source: Option<String>,
}

impl RoiSelection {
    #[allow(dead_code)]
    pub fn source_file_name(&self) -> Option<&str> {
        self.source.as_deref()
    }
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct FeatureDiag {
    pub mask_on: usize,
    pub mask_total: usize,
    pub edge_count: usize,
    pub sampled_points: usize,
}

pub fn load_frame(path: &Path, width: usize, height: usize) -> Result<PlaneFrame, Box<dyn Error>> {
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
    Ok(PlaneFrame::from_owned(
        width as u32,
        height as u32,
        width,
        None,
        data,
    )?)
}

pub fn load_rois(path: &Path) -> Result<RoiSelection, Box<dyn Error>> {
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
    if regions.is_empty()
        && let Some(roi) = dump.roi
    {
        regions.push(RoiEntry {
            roi: normalize_region(&roi, &dump.frame, true),
            description: "roi".into(),
        });
    }
    if regions.is_empty() {
        return Err("no regions available in JSON and no 'roi' fallback".into());
    }
    Ok(RoiSelection {
        frame_width: dump.frame.width,
        frame_height: dump.frame.height,
        luma_band: band,
        regions,
        source: dump.source,
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

#[allow(dead_code)]
pub fn mask_stats(
    frame: &PlaneFrame,
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
    let mut masked_values = Vec::with_capacity(magnitude.len());
    for (idx, &m) in magnitude.iter().enumerate() {
        if patch.mask[idx] >= 0.5 {
            masked_values.push(m);
        }
    }
    let threshold = if !masked_values.is_empty() {
        percentile_in_place(&mut masked_values, 0.7)
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

#[allow(dead_code)]
pub fn debug_features(
    frame: &PlaneFrame,
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
    let mut masked_values = Vec::with_capacity(magnitude.len());
    for (idx, &m) in magnitude.iter().enumerate() {
        if mask[idx] > 0 {
            masked_values.push(m);
        }
    }
    let threshold = if !masked_values.is_empty() {
        percentile_in_place(&mut masked_values, 0.7)
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

#[allow(dead_code)]
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

#[allow(dead_code)]
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
