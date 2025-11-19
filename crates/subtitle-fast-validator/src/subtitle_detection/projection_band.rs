use std::cmp;
use std::ops::Range;

const BYTE_BITS: usize = 8;

use super::{
    DetectionRegion, LumaBandConfig, MIN_REGION_HEIGHT_PX, MIN_REGION_WIDTH_PX, RoiConfig,
    SubtitleDetectionConfig, SubtitleDetectionError, SubtitleDetectionResult, SubtitleDetector,
    log_region_debug,
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

#[derive(Clone)]
struct PackedMask {
    width: usize,
    height: usize,
    stride: usize,
    data: Vec<u8>,
}

impl PackedMask {
    fn new(width: usize, height: usize) -> Self {
        let stride = (width + (BYTE_BITS - 1)) / BYTE_BITS;
        let data = vec![0u8; stride.saturating_mul(height)];
        Self {
            width,
            height,
            stride,
            data,
        }
    }

    fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    fn row(&self, y: usize) -> &[u8] {
        let offset = y * self.stride;
        &self.data[offset..offset + self.stride]
    }

    fn row_mut(&mut self, y: usize) -> &mut [u8] {
        let offset = y * self.stride;
        &mut self.data[offset..offset + self.stride]
    }

    fn row_iter(&self, y: usize) -> BitIter<'_> {
        BitIter::new(self.row(y), self.width)
    }

    fn set_bit(&mut self, x: usize, y: usize) {
        let idx = y * self.stride + x / BYTE_BITS;
        let mask = 1u8 << (x % BYTE_BITS);
        self.data[idx] |= mask;
    }

    fn fill_range(&mut self, y: usize, start: usize, end: usize) {
        if y >= self.height {
            return;
        }
        let width = self.width;
        let start = start.min(width);
        let end = end.min(width);
        if start >= end {
            return;
        }

        let start_byte = start / BYTE_BITS;
        let end_byte = (end - 1) / BYTE_BITS;
        let row_offset = y * self.stride;

        if start_byte == end_byte {
            let bits = ((1u16 << (end - start)) - 1) << (start % BYTE_BITS);
            self.data[row_offset + start_byte] |= bits as u8;
            return;
        }

        let start_mask = (!0u8) << (start % BYTE_BITS);
        let end_mask = if end % BYTE_BITS == 0 {
            0xFF
        } else {
            (1u16 << (end % BYTE_BITS)) as u8 - 1
        };
        self.data[row_offset + start_byte] |= start_mask;
        self.data[row_offset + end_byte] |= end_mask;

        if end_byte > start_byte + 1 {
            let middle = &mut self.data[row_offset + start_byte + 1..row_offset + end_byte];
            middle.fill(0xFF);
        }
    }

    fn fill_column_range(&mut self, x: usize, start_y: usize, end_y: usize) {
        if x >= self.width {
            return;
        }
        let end_y = end_y.min(self.height);
        let start_y = start_y.min(end_y);
        for y in start_y..end_y {
            self.set_bit(x, y);
        }
    }

    fn count_ones_row(&self, y: usize) -> usize {
        if self.width == 0 {
            return 0;
        }
        let row = self.row(y);
        let mut total = 0usize;
        let remainder = self.width % BYTE_BITS;
        for (i, &byte) in row.iter().enumerate() {
            let masked = if remainder != 0 && i + 1 == row.len() {
                byte & ((1u16 << remainder) as u8 - 1)
            } else {
                byte
            };
            total += masked.count_ones() as usize;
        }
        total
    }
}

struct BitIter<'a> {
    bytes: &'a [u8],
    bit_len: usize,
    byte_idx: usize,
    current: u16,
    base: usize,
}

impl<'a> BitIter<'a> {
    fn new(bytes: &'a [u8], bit_len: usize) -> Self {
        Self {
            bytes,
            bit_len,
            byte_idx: 0,
            current: 0,
            base: 0,
        }
    }

    fn load_next(&mut self) -> bool {
        if self.byte_idx >= self.bytes.len() {
            return false;
        }
        let mut byte = self.bytes[self.byte_idx] as u16;
        let bits_left = self
            .bit_len
            .saturating_sub(self.byte_idx.saturating_mul(BYTE_BITS));
        if bits_left < BYTE_BITS {
            let mask = (1u16 << bits_left) - 1;
            byte &= mask;
        }
        self.base = self.byte_idx * BYTE_BITS;
        self.byte_idx += 1;
        self.current = byte;
        true
    }
}

impl<'a> Iterator for BitIter<'a> {
    type Item = usize;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.current != 0 {
                let tz = self.current.trailing_zeros() as usize;
                let idx = self.base + tz;
                self.current &= self.current - 1;
                if idx < self.bit_len {
                    return Some(idx);
                }
            }
            if !self.load_next() {
                return None;
            }
        }
    }
}
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

    fn threshold_mask(&self, data: &[u8]) -> PackedMask {
        threshold_mask(self.roi, data, self.config.stride, self.config.luma_band)
    }

    fn find_candidates(&self, mask: &PackedMask) -> Vec<RegionCandidate> {
        let width = self.roi.width;
        let height = self.roi.height;
        if width == 0 || height == 0 {
            return Vec::new();
        }
        let mut row_density = vec![0f32; height];
        let mut total_density = 0f32;
        let width_f = width.max(1) as f32;
        for y in 0..height {
            let ones = mask.count_ones_row(y);
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
            let mut band_candidates = analyze_band(mask, start..end);
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
        gap_bridge_horizontal(&mut mask, H_GAP);
        gap_bridge_vertical(&mut mask, V_GAP);
        let mut local_candidates = self.find_candidates(&mask);
        if local_candidates.is_empty() {
            local_candidates = rle_candidates(&mask);
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

fn analyze_band(mask: &PackedMask, band: Range<usize>) -> Vec<RegionCandidate> {
    let width = mask.width;
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
        let mut row_first = None;
        let mut row_last = None;
        let mut iter = mask.row_iter(row);
        while let Some(x) = iter.next() {
            column_counts[x] = column_counts[x].saturating_add(1);
            if row_first.is_none() {
                row_first = Some(x);
            }
            row_last = Some(x + 1);
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

fn threshold_mask(roi: RoiRect, data: &[u8], stride: usize, params: LumaBandConfig) -> PackedMask {
    let mut mask = PackedMask::new(roi.width, roi.height);
    if mask.is_empty() {
        return mask;
    }
    let lo = params.target.saturating_sub(params.delta);
    let hi = params.target.saturating_add(params.delta);

    #[cfg(target_arch = "x86_64")]
    let has_sse2 = std::arch::is_x86_feature_detected!("sse2");

    for row in 0..roi.height {
        let src_offset = (roi.y + row) * stride + roi.x;
        let src = &data[src_offset..src_offset + roi.width];
        let dst = mask.row_mut(row);

        #[cfg(target_arch = "x86_64")]
        unsafe {
            if has_sse2 {
                let consumed = threshold_pack_row_sse2(src, dst, lo, hi);
                let byte_offset = consumed / BYTE_BITS;
                if consumed < roi.width {
                    threshold_pack_row_scalar(&src[consumed..], &mut dst[byte_offset..], lo, hi);
                }
                continue;
            }
        }

        threshold_pack_row_scalar(src, dst, lo, hi);
    }

    mask
}

fn threshold_pack_row_scalar(src: &[u8], dst: &mut [u8], lo: u8, hi: u8) {
    let mut byte = 0u8;
    let mut bit_idx = 0usize;
    let mut dst_idx = 0usize;
    for &value in src {
        if value >= lo && value <= hi {
            byte |= 1 << bit_idx;
        }
        bit_idx += 1;
        if bit_idx == BYTE_BITS {
            dst[dst_idx] = byte;
            dst_idx += 1;
            bit_idx = 0;
            byte = 0;
        }
    }
    if bit_idx != 0 {
        dst[dst_idx] = byte;
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2")]
unsafe fn threshold_pack_row_sse2(src: &[u8], dst: &mut [u8], lo: u8, hi: u8) -> usize {
    use std::arch::x86_64::{
        __m128i, _mm_and_si128, _mm_cmpeq_epi8, _mm_loadu_si128, _mm_max_epu8, _mm_min_epu8,
        _mm_movemask_epi8, _mm_set1_epi8,
    };

    let lo_vec = _mm_set1_epi8(lo as i8);
    let hi_vec = _mm_set1_epi8(hi as i8);
    let mut x = 0usize;
    let mut out_idx = 0usize;
    while x + 16 <= src.len() {
        let pixels = _mm_loadu_si128(src.as_ptr().add(x) as *const __m128i);
        let ge_lo = _mm_cmpeq_epi8(pixels, _mm_max_epu8(pixels, lo_vec));
        let le_hi = _mm_cmpeq_epi8(pixels, _mm_min_epu8(pixels, hi_vec));
        let mask_vec = _mm_and_si128(ge_lo, le_hi);
        let bits = _mm_movemask_epi8(mask_vec) as u32;
        dst[out_idx] = (bits & 0xFF) as u8;
        if out_idx + 1 < dst.len() {
            dst[out_idx + 1] = ((bits >> BYTE_BITS) & 0xFF) as u8;
        }
        x += 16;
        out_idx += 2;
    }
    x
}

fn rle_candidates(mask: &PackedMask) -> Vec<RegionCandidate> {
    let stats = connected_components(mask);
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

fn connected_components(mask: &PackedMask) -> Vec<ComponentStats> {
    let width = mask.width;
    let height = mask.height;
    if width == 0 || height == 0 {
        return Vec::new();
    }
    let mut runs = Vec::new();
    let mut row_offsets = vec![0usize; height + 1];
    for y in 0..height {
        row_offsets[y] = runs.len();
        let mut iter = mask.row_iter(y);
        let mut current: Option<RowRun> = None;
        while let Some(x) = iter.next() {
            match &mut current {
                Some(run) if x == run.end => {
                    run.end += 1;
                }
                Some(run) => {
                    runs.push(*run);
                    *run = RowRun {
                        y,
                        start: x,
                        end: x + 1,
                    };
                }
                None => {
                    current = Some(RowRun {
                        y,
                        start: x,
                        end: x + 1,
                    });
                }
            }
        }
        if let Some(run) = current.take() {
            runs.push(run);
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

fn gap_bridge_horizontal(mask: &mut PackedMask, gap: usize) {
    if gap == 0 || mask.width == 0 {
        return;
    }
    for y in 0..mask.height {
        let hits: Vec<usize> = mask.row_iter(y).collect();
        let Some(mut prev) = hits.first().copied() else {
            continue;
        };
        for &curr in hits.iter().skip(1) {
            let span = curr.saturating_sub(prev).saturating_sub(1);
            if span <= gap {
                mask.fill_range(y, prev + 1, curr);
            }
            prev = curr;
        }
    }
}

fn gap_bridge_vertical(mask: &mut PackedMask, gap: usize) {
    if gap == 0 || mask.height == 0 {
        return;
    }
    let width = mask.width;
    let mut last_hit: Vec<Option<usize>> = vec![None; width];
    for y in 0..mask.height {
        let hits: Vec<usize> = mask.row_iter(y).collect();
        for x in hits {
            if let Some(prev_y) = last_hit[x] {
                let span = y.saturating_sub(prev_y).saturating_sub(1);
                if span <= gap {
                    mask.fill_column_range(x, prev_y + 1, y);
                }
            }
            last_hit[x] = Some(y);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn rows_to_mask(rows: &[&[u8]]) -> PackedMask {
        let height = rows.len();
        let width = rows.first().map(|r| r.len()).unwrap_or(0);
        let mut mask = PackedMask::new(width, height);
        for (y, row) in rows.iter().enumerate() {
            for (x, &v) in row.iter().enumerate() {
                if v != 0 {
                    mask.set_bit(x, y);
                }
            }
        }
        mask
    }

    fn mask_to_rows(mask: &PackedMask) -> Vec<Vec<u8>> {
        let mut rows = Vec::new();
        for y in 0..mask.height {
            let mut row = vec![0u8; mask.width];
            for idx in mask.row_iter(y) {
                row[idx] = 1;
            }
            rows.push(row);
        }
        rows
    }

    #[test]
    fn horizontal_gap_bridge_fills_short_spans() {
        let mut mask = rows_to_mask(&[&[1, 0, 0, 1], &[0, 0, 0, 0]]);
        gap_bridge_horizontal(&mut mask, 2);
        let rows = mask_to_rows(&mask);
        assert_eq!(rows[0], vec![1, 1, 1, 1]);
        assert_eq!(rows[1], vec![0, 0, 0, 0]);
    }

    #[test]
    fn vertical_gap_bridge_respects_limit() {
        let mut mask = rows_to_mask(&[&[1, 0], &[0, 0], &[1, 0]]);
        gap_bridge_vertical(&mut mask, 1);
        let rows = mask_to_rows(&mask);
        assert_eq!(rows, vec![vec![1, 0], vec![1, 0], vec![1, 0]]);

        let mut mask = rows_to_mask(&[&[1, 0], &[0, 0], &[0, 0], &[1, 0]]);
        gap_bridge_vertical(&mut mask, 1);
        let rows = mask_to_rows(&mask);
        assert_eq!(rows, vec![vec![1, 0], vec![0, 0], vec![0, 0], vec![1, 0]]);
    }
}
