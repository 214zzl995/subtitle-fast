use std::cmp;
use std::env;
use std::ops::Range;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::{
    log_region_debug, DetectionRegion, LumaBandConfig, RoiConfig, SubtitleDetectionConfig,
    SubtitleDetectionError, SubtitleDetectionResult, SubtitleDetector, MIN_REGION_HEIGHT_PX,
    MIN_REGION_WIDTH_PX,
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
const PROJECTION_PERF_ENV: &str = "PROJECTION_PERF";
static PROJECTION_PERF_STATS: OnceLock<Mutex<ProjectionPerfStats>> = OnceLock::new();
static PROJECTION_PERF_GUARD: OnceLock<ProjectionPerfGuard> = OnceLock::new();

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
        threshold_mask(self.roi, data, self.config.stride, self.config.luma_band)
    }

    fn find_candidates(&self, mask: &[u8]) -> Vec<RegionCandidate> {
        let width = self.roi.width;
        let height = self.roi.height;
        if width == 0 || height == 0 {
            return Vec::new();
        }
        let mut row_density = vec![0f32; height];
        let mut total_density = 0f32;
        let width_f = width.max(1) as f32;
        for y in 0..height {
            let row = &mask[y * width..(y + 1) * width];
            let ones = count_ones_row(row);
            row_density[y] = ones as f32 / width_f;
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
        let perf_start = projection_perf_enabled().then(Instant::now);
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
        let result = SubtitleDetectionResult {
            has_subtitle: !regions.is_empty(),
            max_score: regions.first().map(|r| r.score).unwrap_or(0.0),
            regions,
        };
        if let Some(start) = perf_start {
            projection_perf_record(start.elapsed());
        }
        Ok(result)
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

fn projection_perf_enabled() -> bool {
    env::var_os(PROJECTION_PERF_ENV).is_some()
}

fn projection_perf_record(duration: Duration) {
    if !projection_perf_enabled() {
        return;
    }
    PROJECTION_PERF_GUARD.get_or_init(|| ProjectionPerfGuard);
    let stats_lock =
        PROJECTION_PERF_STATS.get_or_init(|| Mutex::new(ProjectionPerfStats::default()));
    if let Ok(mut stats) = stats_lock.lock() {
        stats.frames += 1;
        stats.total += duration;
    }
}

#[derive(Default)]
struct ProjectionPerfStats {
    total: Duration,
    frames: u64,
}

struct ProjectionPerfGuard;

impl Drop for ProjectionPerfGuard {
    fn drop(&mut self) {
        if !projection_perf_enabled() {
            return;
        }
        if let Some(stats_lock) = PROJECTION_PERF_STATS.get() {
            if let Ok(stats) = stats_lock.lock() {
                if stats.frames > 0 {
                    let avg_ms = (stats.total.as_secs_f64() * 1000.0) / stats.frames as f64;
                    eprintln!(
                        "[projection][perf] frames={} avg={:.3}ms",
                        stats.frames, avg_ms
                    );
                }
            }
        }
    }
}

fn count_ones_row(row: &[u8]) -> usize {
    let mut total = 0usize;
    let mut chunks = row.chunks_exact(16);
    for chunk in &mut chunks {
        let arr: [u8; 16] = chunk.try_into().unwrap();
        let value = u128::from_le_bytes(arr);
        total += value.count_ones() as usize;
    }
    for &byte in chunks.remainder() {
        total += (byte != 0) as usize;
    }
    total
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
        accumulate_columns(row_slice, &mut column_counts);
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

fn threshold_mask(roi: RoiRect, data: &[u8], stride: usize, params: LumaBandConfig) -> Vec<u8> {
    let mut mask = vec![0u8; roi.width * roi.height];
    if mask.is_empty() {
        return mask;
    }
    let lo = params.target.saturating_sub(params.delta);
    let hi = params.target.saturating_add(params.delta);

    #[cfg(target_arch = "x86_64")]
    {
        if std::arch::is_x86_feature_detected!("sse2") {
            unsafe {
                threshold_mask_sse2(data, stride, roi, lo, hi, &mut mask);
            }
            return mask;
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        if std::arch::is_aarch64_feature_detected!("neon") {
            unsafe {
                threshold_mask_neon(data, stride, roi, lo, hi, &mut mask);
            }
            return mask;
        }
    }

    threshold_mask_scalar(data, stride, roi, lo, hi, &mut mask);
    mask
}

fn threshold_mask_scalar(
    data: &[u8],
    stride: usize,
    roi: RoiRect,
    lo: u8,
    hi: u8,
    mask: &mut [u8],
) {
    for row in 0..roi.height {
        let src_offset = (roi.y + row) * stride + roi.x;
        let dst_offset = row * roi.width;
        let src = &data[src_offset..src_offset + roi.width];
        let dst = &mut mask[dst_offset..dst_offset + roi.width];
        threshold_row(src, dst, lo, hi);
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2")]
unsafe fn threshold_mask_sse2(
    data: &[u8],
    stride: usize,
    roi: RoiRect,
    lo: u8,
    hi: u8,
    mask: &mut [u8],
) {
    use std::arch::x86_64::{
        __m128i, _mm_and_si128, _mm_cmpeq_epi8, _mm_loadu_si128, _mm_max_epu8, _mm_min_epu8,
        _mm_set1_epi8, _mm_storeu_si128,
    };

    let lo_vec = _mm_set1_epi8(lo as i8);
    let hi_vec = _mm_set1_epi8(hi as i8);
    let ones = _mm_set1_epi8(1);
    let width = roi.width;

    for row in 0..roi.height {
        let src_ptr = data.as_ptr().add((roi.y + row) * stride + roi.x);
        let dst_ptr = mask.as_mut_ptr().add(row * width);
        let mut x = 0usize;
        while x + 16 <= width {
            let pixels = _mm_loadu_si128(src_ptr.add(x) as *const __m128i);
            let ge_lo = _mm_cmpeq_epi8(pixels, _mm_max_epu8(pixels, lo_vec));
            let le_hi = _mm_cmpeq_epi8(pixels, _mm_min_epu8(pixels, hi_vec));
            let mut mask_vec = _mm_and_si128(ge_lo, le_hi);
            mask_vec = _mm_and_si128(mask_vec, ones);
            _mm_storeu_si128(dst_ptr.add(x) as *mut __m128i, mask_vec);
            x += 16;
        }
        if x < width {
            let rem = width - x;
            let src_tail = std::slice::from_raw_parts(src_ptr.add(x), rem);
            let dst_tail = std::slice::from_raw_parts_mut(dst_ptr.add(x), rem);
            threshold_row(src_tail, dst_tail, lo, hi);
        }
    }
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
unsafe fn threshold_mask_neon(
    data: &[u8],
    stride: usize,
    roi: RoiRect,
    lo: u8,
    hi: u8,
    mask: &mut [u8],
) {
    use std::arch::aarch64::{
        uint8x16_t, vandq_u8, vceqq_u8, vdupq_n_u8, vld1q_u8, vmaxq_u8, vminq_u8, vst1q_u8,
    };

    let lo_vec: uint8x16_t = vdupq_n_u8(lo);
    let hi_vec: uint8x16_t = vdupq_n_u8(hi);
    let ones: uint8x16_t = vdupq_n_u8(1);
    let width = roi.width;

    for row in 0..roi.height {
        let src_ptr = data.as_ptr().add((roi.y + row) * stride + roi.x);
        let dst_ptr = mask.as_mut_ptr().add(row * width);
        let mut x = 0usize;
        while x + 16 <= width {
            let pixels = vld1q_u8(src_ptr.add(x));
            let ge_lo = vceqq_u8(pixels, vmaxq_u8(pixels, lo_vec));
            let le_hi = vceqq_u8(pixels, vminq_u8(pixels, hi_vec));
            let mask_vec = vandq_u8(vandq_u8(ge_lo, le_hi), ones);
            vst1q_u8(dst_ptr.add(x), mask_vec);
            x += 16;
        }
        if x < width {
            let rem = width - x;
            let src_tail = std::slice::from_raw_parts(src_ptr.add(x), rem);
            let dst_tail = std::slice::from_raw_parts_mut(dst_ptr.add(x), rem);
            threshold_row(src_tail, dst_tail, lo, hi);
        }
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

fn accumulate_columns(row: &[u8], counts: &mut [u16]) {
    if row.is_empty() {
        return;
    }
    #[cfg(target_arch = "x86_64")]
    {
        if std::arch::is_x86_feature_detected!("sse2") {
            unsafe {
                accumulate_columns_sse2(row, counts);
            }
            return;
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        if std::arch::is_aarch64_feature_detected!("neon") {
            unsafe {
                accumulate_columns_neon(row, counts);
            }
            return;
        }
    }
    accumulate_columns_scalar(row, counts);
}

fn accumulate_columns_scalar(row: &[u8], counts: &mut [u16]) {
    for (dst, &value) in counts.iter_mut().zip(row.iter()) {
        *dst = dst.saturating_add(value as u16);
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2")]
unsafe fn accumulate_columns_sse2(row: &[u8], counts: &mut [u16]) {
    use std::arch::x86_64::{
        __m128i, _mm_add_epi16, _mm_cvtepu8_epi16, _mm_loadu_si128, _mm_srli_si128,
        _mm_storeu_si128,
    };
    let len = row.len();
    let mut x = 0usize;
    while x + 16 <= len {
        let pixels = _mm_loadu_si128(row.as_ptr().add(x) as *const __m128i);
        let low = _mm_cvtepu8_epi16(pixels);
        let high = _mm_cvtepu8_epi16(_mm_srli_si128(pixels, 8));
        let dst_ptr = counts.as_mut_ptr().add(x) as *mut __m128i;
        let dst_hi_ptr = counts.as_mut_ptr().add(x + 8) as *mut __m128i;
        let sum_low = _mm_add_epi16(_mm_loadu_si128(dst_ptr), low);
        let sum_high = _mm_add_epi16(_mm_loadu_si128(dst_hi_ptr), high);
        _mm_storeu_si128(dst_ptr, sum_low);
        _mm_storeu_si128(dst_hi_ptr, sum_high);
        x += 16;
    }
    if x < len {
        let remaining = len - x;
        let tail_src = std::slice::from_raw_parts(row.as_ptr().add(x), remaining);
        let tail_dst = std::slice::from_raw_parts_mut(counts.as_mut_ptr().add(x), remaining);
        for (dst, &value) in tail_dst.iter_mut().zip(tail_src.iter()) {
            *dst = dst.saturating_add(value as u16);
        }
    }
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
unsafe fn accumulate_columns_neon(row: &[u8], counts: &mut [u16]) {
    use std::arch::aarch64::{
        uint16x8_t, uint8x16_t, vaddq_u16, vget_high_u8, vget_low_u8, vld1q_u16, vld1q_u8,
        vmovl_u8, vst1q_u16,
    };
    let len = row.len();
    let mut x = 0usize;
    while x + 16 <= len {
        let pixels: uint8x16_t = vld1q_u8(row.as_ptr().add(x));
        let low: uint16x8_t = vmovl_u8(vget_low_u8(pixels));
        let high: uint16x8_t = vmovl_u8(vget_high_u8(pixels));
        let dst_ptr = counts.as_mut_ptr().add(x);
        let curr_low = vld1q_u16(dst_ptr);
        let curr_high = vld1q_u16(dst_ptr.add(8));
        vst1q_u16(dst_ptr, vaddq_u16(curr_low, low));
        vst1q_u16(dst_ptr.add(8), vaddq_u16(curr_high, high));
        x += 16;
    }
    if x < len {
        let remaining = len - x;
        let tail_src = std::slice::from_raw_parts(row.as_ptr().add(x), remaining);
        let tail_dst = std::slice::from_raw_parts_mut(counts.as_mut_ptr().add(x), remaining);
        for (dst, &value) in tail_dst.iter_mut().zip(tail_src.iter()) {
            *dst = dst.saturating_add(value as u16);
        }
    }
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
