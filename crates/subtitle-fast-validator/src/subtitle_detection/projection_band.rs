use std::cmp;
use std::ops::Range;

use super::{
    log_region_debug, DetectionRegion, RoiConfig, SubtitleDetectionConfig, SubtitleDetectionError,
    SubtitleDetectionResult, SubtitleDetector, MIN_REGION_HEIGHT_PX, MIN_REGION_WIDTH_PX,
};
use subtitle_fast_decoder::YPlaneFrame;

const ROW_DENSITY_THRESHOLD: f32 = 0.08;
const MIN_BAND_HEIGHT: usize = 8;
const MAX_BANDS: usize = 5;
const MIN_FILL: f32 = 0.25;
const H_GAP: usize = 80;
const V_GAP: usize = 12;
const BAND_SPLIT_MIN_GAP: usize = 32;
const BAND_SPLIT_GAP_RATIO: f32 = 0.2;

#[derive(Clone, Copy)]
struct RoiRect {
    x: usize,
    y: usize,
    width: usize,
    height: usize,
}

pub struct ProjectionBandDetector {
    config: SubtitleDetectionConfig,
    roi: RoiRect,
    required_len: usize,
}

impl ProjectionBandDetector {
    pub fn new(config: SubtitleDetectionConfig) -> Result<Self, SubtitleDetectionError> {
        let required_len = required_len(&config)?;
        let roi = compute_roi_rect(config.frame_width, config.frame_height, config.roi)?;
        Ok(Self {
            config,
            roi,
            required_len,
        })
    }

    fn threshold_mask(&self, data: &[u8]) -> Vec<u8> {
        let mut mask = vec![0u8; self.roi.width * self.roi.height];
        if mask.is_empty() {
            return mask;
        }
        let stride = self.config.stride;
        let params = self.config.luma_band;
        let lo = params.target.saturating_sub(params.delta);
        let hi = params.target.saturating_add(params.delta);
        for row in 0..self.roi.height {
            let src_offset = (self.roi.y + row) * stride + self.roi.x;
            let dst_offset = row * self.roi.width;
            let src = &data[src_offset..src_offset + self.roi.width];
            let dst = &mut mask[dst_offset..dst_offset + self.roi.width];
            threshold_row(src, dst, lo, hi);
        }
        mask
    }

    fn find_candidates(&self, mask: &[u8]) -> Vec<RegionCandidate> {
        let width = self.roi.width;
        let height = self.roi.height;
        if width == 0 || height == 0 {
            return Vec::new();
        }
        let mut row_density = vec![0f32; height];
        let mut total_density = 0f32;
        for y in 0..height {
            let row = &mask[y * width..(y + 1) * width];
            let ones = row.iter().filter(|&&b| b != 0).count();
            row_density[y] = ones as f32 / width.max(1) as f32;
            total_density += row_density[y];
        }
        let avg_density = total_density / height.max(1) as f32;
        let density_threshold = ROW_DENSITY_THRESHOLD.min(avg_density * 0.7).max(0.02);
        let mut candidates = Vec::new();
        let mut y = 0usize;
        while y < height {
            if row_density[y] < density_threshold {
                y += 1;
                continue;
            }
            let start = y;
            y += 1;
            while y < height && row_density[y] >= density_threshold {
                y += 1;
            }
            let end = y;
            if end - start < MIN_BAND_HEIGHT {
                continue;
            }
            let mut band_candidates = analyze_band(mask, width, start..end);
            candidates.append(&mut band_candidates);
        }
        candidates.sort_by(|a, b| {
            candidate_mass(b)
                .partial_cmp(&candidate_mass(a))
                .unwrap_or(cmp::Ordering::Equal)
        });
        candidates.truncate(MAX_BANDS);
        candidates
    }
}

impl SubtitleDetector for ProjectionBandDetector {
    fn ensure_available(config: &SubtitleDetectionConfig) -> Result<(), SubtitleDetectionError> {
        required_len(config).map(|_| ())
    }

    fn detect(
        &self,
        frame: &YPlaneFrame,
    ) -> Result<SubtitleDetectionResult, SubtitleDetectionError> {
        let data = frame.data();
        if data.len() < self.required_len {
            return Err(SubtitleDetectionError::InsufficientData {
                data_len: data.len(),
                required: self.required_len,
            });
        }
        let mut mask = self.threshold_mask(data);
        gap_bridge_horizontal(&mut mask, self.roi.width, self.roi.height, H_GAP);
        gap_bridge_vertical(&mut mask, self.roi.width, self.roi.height, V_GAP);
        let mut local_candidates = self.find_candidates(&mask);
        if local_candidates.is_empty() {
            local_candidates = rle_candidates(&mask, self.roi.width, self.roi.height);
        }
        if local_candidates.is_empty() {
            return Ok(SubtitleDetectionResult::empty());
        }
        let mut regions = Vec::new();
        for cand in local_candidates {
            let activation = candidate_mass(&cand);
            log_region_debug(
                "projection",
                "accept_region",
                cand.x,
                cand.y,
                cand.width,
                cand.height,
                activation,
            );
            regions.push(DetectionRegion {
                x: (cand.x + self.roi.x) as f32,
                y: (cand.y + self.roi.y) as f32,
                width: cand.width as f32,
                height: cand.height as f32,
                score: activation,
            });
        }
        Ok(SubtitleDetectionResult {
            has_subtitle: !regions.is_empty(),
            max_score: regions.first().map(|r| r.score).unwrap_or(0.0),
            regions,
        })
    }
}

#[derive(Clone)]
struct RegionCandidate {
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    score: f32,
}

fn candidate_mass(candidate: &RegionCandidate) -> f32 {
    let area = candidate.width.saturating_mul(candidate.height).max(1);
    candidate.score * area as f32
}

fn analyze_band(mask: &[u8], width: usize, band: Range<usize>) -> Vec<RegionCandidate> {
    let height = band.end.saturating_sub(band.start);
    if height == 0 || height < MIN_REGION_HEIGHT_PX {
        log_region_debug(
            "projection",
            "reject_band_short",
            0,
            band.start,
            width,
            height,
            0.0,
        );
        return Vec::new();
    }
    let mut min_x = width;
    let mut max_x = 0usize;
    let mut column_counts = vec![0u16; width];
    for row in band.clone() {
        let row_slice = &mask[row * width..(row + 1) * width];
        let mut row_first = None;
        let mut row_last = None;
        for (x, &value) in row_slice.iter().enumerate() {
            if value != 0 {
                column_counts[x] = column_counts[x].saturating_add(1);
                if row_first.is_none() {
                    row_first = Some(x);
                }
                row_last = Some(x + 1);
            }
        }
        if let Some(first) = row_first {
            min_x = min_x.min(first);
        }
        if let Some(last) = row_last {
            max_x = max_x.max(last);
        }
    }
    if min_x >= max_x {
        return Vec::new();
    }
    let band_width = max_x.saturating_sub(min_x);
    let split_gap = cmp::max(
        BAND_SPLIT_MIN_GAP,
        (band_width as f32 * BAND_SPLIT_GAP_RATIO).ceil() as usize,
    );
    let mut raw_segments = Vec::new();
    let mut x = min_x;
    while x < max_x {
        while x < max_x && column_counts[x] == 0 {
            x += 1;
        }
        if x >= max_x {
            break;
        }
        let start = x;
        while x < max_x && column_counts[x] != 0 {
            x += 1;
        }
        raw_segments.push(start..x);
    }
    if raw_segments.is_empty() {
        return Vec::new();
    }
    let mut merged_segments: Vec<Range<usize>> = Vec::new();
    for seg in raw_segments {
        if let Some(last) = merged_segments.last_mut() {
            let gap = seg.start.saturating_sub(last.end);
            if gap < split_gap {
                last.end = seg.end;
                continue;
            }
        }
        merged_segments.push(seg);
    }
    let mut candidates = Vec::new();
    for seg in merged_segments {
        let seg_width = seg.end.saturating_sub(seg.start);
        if seg_width == 0 {
            continue;
        }
        let mut seg_ones = 0usize;
        for x in seg.start..seg.end {
            seg_ones += column_counts[x] as usize;
        }
        let area = seg_width.saturating_mul(height);
        if area == 0 {
            continue;
        }
        let fill = seg_ones as f32 / area as f32;
        if fill < MIN_FILL {
            continue;
        }
        if seg_width < MIN_REGION_WIDTH_PX {
            log_region_debug(
                "projection",
                "reject_narrow_band",
                seg.start,
                band.start,
                seg_width,
                height,
                fill,
            );
            continue;
        }
        candidates.push(RegionCandidate {
            x: seg.start,
            y: band.start,
            width: seg_width,
            height,
            score: fill,
        });
        log_region_debug(
            "projection",
            "candidate_band",
            seg.start,
            band.start,
            seg_width,
            height,
            fill,
        );
    }
    candidates
}

fn threshold_row(src: &[u8], dst: &mut [u8], lo: u8, hi: u8) {
    for (value, out) in src.iter().zip(dst.iter_mut()) {
        *out = u8::from(*value >= lo && *value <= hi);
    }
}

fn rle_candidates(mask: &[u8], width: usize, height: usize) -> Vec<RegionCandidate> {
    let stats = connected_components(mask, width, height);
    let mut candidates = Vec::new();
    for comp in stats {
        let w = comp.max_x.saturating_sub(comp.min_x) + 1;
        let h = comp.max_y.saturating_sub(comp.min_y) + 1;
        let area = w * h;
        if area == 0 {
            continue;
        }
        if h < MIN_REGION_HEIGHT_PX {
            log_region_debug(
                "projection",
                "reject_short_component",
                comp.min_x,
                comp.min_y,
                w,
                h,
                0.0,
            );
            continue;
        }
        if w < MIN_REGION_WIDTH_PX {
            log_region_debug(
                "projection",
                "reject_narrow_component",
                comp.min_x,
                comp.min_y,
                w,
                h,
                0.0,
            );
            continue;
        }
        let fill = comp.area as f32 / area as f32;
        if fill < MIN_FILL {
            continue;
        }
        candidates.push(RegionCandidate {
            x: comp.min_x,
            y: comp.min_y,
            width: w,
            height: h,
            score: fill,
        });
        log_region_debug(
            "projection",
            "candidate_rle",
            comp.min_x,
            comp.min_y,
            w,
            h,
            fill,
        );
    }
    candidates
}

#[derive(Clone, Copy)]
struct ComponentStats {
    area: usize,
    min_x: usize,
    max_x: usize,
    min_y: usize,
    max_y: usize,
}

impl ComponentStats {
    fn new(x: usize, y: usize) -> Self {
        Self {
            area: 0,
            min_x: x,
            max_x: x,
            min_y: y,
            max_y: y,
        }
    }
}

#[derive(Clone, Copy)]
struct RowRun {
    y: usize,
    start: usize,
    end: usize,
}

#[derive(Default)]
struct RunDsu {
    parent: Vec<u32>,
    rank: Vec<u8>,
}

impl RunDsu {
    fn new(len: usize) -> Self {
        Self {
            parent: (0..len as u32).collect(),
            rank: vec![0; len],
        }
    }

    fn find(&mut self, x: u32) -> u32 {
        let idx = x as usize;
        let parent = self.parent[idx];
        if parent == x {
            return x;
        }
        let root = self.find(parent);
        self.parent[idx] = root;
        root
    }

    fn union(&mut self, a: u32, b: u32) {
        let mut root_a = self.find(a);
        let mut root_b = self.find(b);
        if root_a == root_b {
            return;
        }
        let rank_a = self.rank[root_a as usize];
        let rank_b = self.rank[root_b as usize];
        if rank_a < rank_b {
            std::mem::swap(&mut root_a, &mut root_b);
        }
        self.parent[root_b as usize] = root_a;
        if rank_a == rank_b {
            self.rank[root_a as usize] = rank_a + 1;
        }
    }
}

fn connected_components(mask: &[u8], width: usize, height: usize) -> Vec<ComponentStats> {
    if width == 0 || height == 0 {
        return Vec::new();
    }
    let mut runs = Vec::new();
    let mut row_offsets = vec![0usize; height + 1];
    for y in 0..height {
        row_offsets[y] = runs.len();
        let row = &mask[y * width..(y + 1) * width];
        let mut x = 0usize;
        while x < width {
            if row[x] == 0 {
                x += 1;
                continue;
            }
            let start = x;
            while x < width && row[x] != 0 {
                x += 1;
            }
            runs.push(RowRun { y, start, end: x });
        }
    }
    row_offsets[height] = runs.len();
    if runs.is_empty() {
        return Vec::new();
    }

    let mut dsu = RunDsu::new(runs.len());
    for y in 1..height {
        let prev_start = row_offsets[y - 1];
        let prev_end = row_offsets[y];
        let curr_start = row_offsets[y];
        let curr_end = row_offsets[y + 1];
        for curr_idx in curr_start..curr_end {
            let curr = runs[curr_idx];
            for prev_idx in prev_start..prev_end {
                let prev = runs[prev_idx];
                if prev.y + 1 == curr.y && prev.end > curr.start && curr.end > prev.start {
                    dsu.union(curr_idx as u32, prev_idx as u32);
                }
            }
        }
    }

    let mut stats = vec![None; runs.len()];
    for (idx, run) in runs.iter().enumerate() {
        let root = dsu.find(idx as u32) as usize;
        let entry = stats[root].get_or_insert_with(|| ComponentStats::new(run.start, run.y));
        entry.area += run.end - run.start;
        entry.min_x = entry.min_x.min(run.start);
        entry.max_x = entry.max_x.max(run.end.saturating_sub(1));
        entry.min_y = entry.min_y.min(run.y);
        entry.max_y = entry.max_y.max(run.y);
    }
    stats.into_iter().flatten().collect()
}

fn gap_bridge_horizontal(mask: &mut [u8], width: usize, height: usize, gap: usize) {
    if gap == 0 || width == 0 {
        return;
    }
    for y in 0..height {
        let row = &mut mask[y * width..(y + 1) * width];
        let mut x = 0usize;
        while x < width {
            if row[x] != 0 {
                x += 1;
                continue;
            }
            let mut span_end = x;
            while span_end < width && row[span_end] == 0 {
                span_end += 1;
            }
            let left_connected = x > 0 && row[x - 1] != 0;
            let right_connected = span_end < width && row[span_end] != 0;
            if left_connected && right_connected && (span_end - x) <= gap {
                for cell in &mut row[x..span_end] {
                    *cell = 1;
                }
            }
            x = span_end;
        }
    }
}

fn gap_bridge_vertical(mask: &mut [u8], width: usize, height: usize, gap: usize) {
    if gap == 0 || height == 0 {
        return;
    }
    for x in 0..width {
        let mut y = 0usize;
        while y < height {
            let idx = y * width + x;
            if mask[idx] != 0 {
                y += 1;
                continue;
            }
            let mut span_end = y;
            while span_end < height && mask[span_end * width + x] == 0 {
                span_end += 1;
            }
            let top_connected = y > 0 && mask[(y - 1) * width + x] != 0;
            let bottom_connected = span_end < height && mask[span_end * width + x] != 0;
            if top_connected && bottom_connected && (span_end - y) <= gap {
                for fill_y in y..span_end {
                    mask[fill_y * width + x] = 1;
                }
            }
            y = span_end;
        }
    }
}

fn required_len(config: &SubtitleDetectionConfig) -> Result<usize, SubtitleDetectionError> {
    config
        .stride
        .checked_mul(config.frame_height)
        .ok_or_else(|| SubtitleDetectionError::InsufficientData {
            data_len: 0,
            required: usize::MAX,
        })
}

fn compute_roi_rect(
    frame_width: usize,
    frame_height: usize,
    roi: RoiConfig,
) -> Result<RoiRect, SubtitleDetectionError> {
    let start_x = (roi.x * frame_width as f32).floor() as isize;
    let start_y = (roi.y * frame_height as f32).floor() as isize;
    let end_x = ((roi.x + roi.width) * frame_width as f32).ceil() as isize;
    let end_y = ((roi.y + roi.height) * frame_height as f32).ceil() as isize;

    let start_x = start_x.clamp(0, frame_width as isize);
    let start_y = start_y.clamp(0, frame_height as isize);
    let end_x = end_x.clamp(start_x, frame_width as isize);
    let end_y = end_y.clamp(start_y, frame_height as isize);

    if start_x == end_x || start_y == end_y {
        return Err(SubtitleDetectionError::EmptyRoi);
    }

    Ok(RoiRect {
        x: start_x as usize,
        y: start_y as usize,
        width: (end_x - start_x) as usize,
        height: (end_y - start_y) as usize,
    })
}
