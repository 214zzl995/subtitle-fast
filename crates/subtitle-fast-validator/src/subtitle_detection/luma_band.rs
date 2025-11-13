use std::cmp::{self, Ordering};
use std::sync::Mutex;

#[cfg(target_arch = "aarch64")]
use std::arch::is_aarch64_feature_detected;
#[cfg(target_arch = "x86_64")]
use std::arch::is_x86_feature_detected;
#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::{
    __m128i, __m256i, _mm256_and_si256, _mm256_andnot_si256, _mm256_cmpgt_epi8, _mm256_loadu_si256,
    _mm256_set1_epi8, _mm256_storeu_si256, _mm256_xor_si256, _mm_loadu_si128, _mm_max_epu8,
    _mm_min_epu8, _mm_set1_epi8, _mm_storeu_si128,
};

#[cfg(feature = "detector-parallel")]
use rayon::prelude::*;

use super::{
    DetectionRegion, GapFillMode, LumaBandConfig, RoiConfig, SubtitleDetectionConfig,
    SubtitleDetectionError, SubtitleDetectionResult, SubtitleDetector,
};
use subtitle_fast_decoder::YPlaneFrame;

// Minimum connected-component area kept as a subtitle candidate.
const MIN_AREA: usize = 300;
// Reject components that occupy more than this fraction of the frame.
const MAX_AREA_RATIO: f32 = 0.35;
// Lower bound on width / height of candidate rectangles.
const MIN_ASPECT_RATIO: f32 = 2.0;
// Subdivision factor for evaluating fill variance.
const VMR_K: usize = 4;
// Max distance between vertical centers to consider the same line.
const Y_MERGE_TOL: usize = 10;
// Required vertical overlap ratio to force a same-line merge.
const Y_OVERLAP_RATIO: f32 = 0.30;
// IoU threshold when merging horizontally adjacent boxes.
const IOU_MERGE: f32 = 0.15;
// Allowable horizontal gap when merging neighboring boxes.
const NEAR_GAP: usize = 16;
// Clamp the number of boxes returned to the pipeline.
const MAX_OUTPUT_REGIONS: usize = 4;
const MASK_DENSITY_THRESHOLD: f32 = 0.0015;
const MASK_POPULATION_RATIO: f32 = 0.0004;
const MIN_MASK_POPULATION: usize = 128;
const ROW_DENSITY_THRESHOLD: f32 = 0.2;
const MIN_BAND_ROWS: usize = 1;
const MIN_ROW_BAND_PX: usize = 12;
const HORIZONTAL_GAP: usize = 100;
const VERTICAL_GAP: usize = 10;
const ROW_PROJECTION_ENABLED: bool = true;
const COMPONENTS_FALLBACK_ENABLED: bool = true;
const GAP_FILL_MODE: GapFillMode = GapFillMode::Distance;

#[derive(Clone, Copy)]
struct RoiRect {
    x: usize,
    y: usize,
    width: usize,
    height: usize,
}

#[derive(Clone, Copy)]
struct RectCandidate {
    x: usize,
    y: usize,
    width: usize,
    height: usize,
}

#[derive(Clone, Copy)]
struct RowRun {
    start: usize,
    end: usize,
    row: usize,
    label: u32,
}

#[derive(Clone, Copy)]
struct RawPtr<T>(*mut T);

unsafe impl<T: Send> Send for RawPtr<T> {}
unsafe impl<T: Send> Sync for RawPtr<T> {}

impl<T: Send> RawPtr<T> {
    unsafe fn write(&self, idx: usize, value: T) {
        *self.0.add(idx) = value;
    }
}

#[derive(Clone, Copy)]
struct RawConstPtr<T>(*const T);

unsafe impl<T: Sync> Send for RawConstPtr<T> {}
unsafe impl<T: Sync> Sync for RawConstPtr<T> {}

impl<T: Sync + Copy> RawConstPtr<T> {
    unsafe fn read(&self, idx: usize) -> T {
        *self.0.add(idx)
    }
}

#[derive(Default)]
struct DetectorWorkspace {
    mask: Vec<u8>,
    integral: Vec<u32>,
    row_prefix: Vec<u32>,
    row_sums: Vec<u32>,
    left_edges: Vec<usize>,
    right_edges: Vec<usize>,
    labels: Vec<u32>,
    runs: Vec<RowRun>,
    run_offsets: Vec<usize>,
    morph_tmp: Vec<u8>,
    morph_prefix: Vec<u8>,
    morph_suffix: Vec<u8>,
    horiz_dist: Vec<u16>,
    morph_line: Vec<u8>,
}

impl DetectorWorkspace {
    fn ensure_capacity(&mut self, width: usize, height: usize) {
        let roi_pixels = width.saturating_mul(height);
        if self.mask.len() < roi_pixels {
            self.mask.resize(roi_pixels, 0);
        }
        if self.integral.len() < (width + 1).saturating_mul(height + 1) {
            self.integral
                .resize((width + 1).saturating_mul(height + 1), 0);
        }
        if self.row_prefix.len() < roi_pixels {
            self.row_prefix.resize(roi_pixels, 0);
        }
        if self.row_sums.len() < height {
            self.row_sums.resize(height, 0);
        }
        if self.left_edges.len() < height {
            self.left_edges.resize(height, width);
        }
        if self.right_edges.len() < height {
            self.right_edges.resize(height, 0);
        }
        if self.labels.len() < roi_pixels {
            self.labels.resize(roi_pixels, 0);
        }
        if self.run_offsets.len() < height + 1 {
            self.run_offsets.resize(height + 1, 0);
        }
        if self.morph_tmp.len() < roi_pixels {
            self.morph_tmp.resize(roi_pixels, 0);
        }
        let max_line = width.max(height);
        if self.morph_prefix.len() < max_line {
            self.morph_prefix.resize(max_line, 0);
        }
        if self.morph_suffix.len() < max_line {
            self.morph_suffix.resize(max_line, 0);
        }
        if self.horiz_dist.len() < roi_pixels {
            self.horiz_dist.resize(roi_pixels, 0);
        }
        if self.morph_line.len() < max_line {
            self.morph_line.resize(max_line, 0);
        }
    }

    fn detection_buffers(&mut self, width: usize, height: usize) -> MainBuffers<'_> {
        MainBuffers {
            mask: &mut self.mask[..width * height],
            integral: &mut self.integral[..(width + 1) * (height + 1)],
            row_prefix: &mut self.row_prefix[..width * height],
            row_sums: &mut self.row_sums[..height],
            left_edges: &mut self.left_edges[..height],
            right_edges: &mut self.right_edges[..height],
            labels: &mut self.labels[..width * height],
            horiz: &mut self.horiz_dist[..width * height],
            runs: &mut self.runs,
            offsets: &mut self.run_offsets[..height + 1],
            morph_tmp: &mut self.morph_tmp[..width * height],
            morph_prefix: &mut self.morph_prefix[..width.max(height)],
            morph_suffix: &mut self.morph_suffix[..width.max(height)],
            morph_line: &mut self.morph_line[..width.max(height)],
        }
    }
}

struct MainBuffers<'a> {
    mask: &'a mut [u8],
    integral: &'a mut [u32],
    row_prefix: &'a mut [u32],
    row_sums: &'a mut [u32],
    left_edges: &'a mut [usize],
    right_edges: &'a mut [usize],
    labels: &'a mut [u32],
    horiz: &'a mut [u16],
    runs: &'a mut Vec<RowRun>,
    offsets: &'a mut [usize],
    morph_tmp: &'a mut [u8],
    morph_prefix: &'a mut [u8],
    morph_suffix: &'a mut [u8],
    morph_line: &'a mut [u8],
}

pub struct LumaBandDetector {
    config: SubtitleDetectionConfig,
    roi: RoiRect,
    required_len: usize,
    workspace: Mutex<DetectorWorkspace>,
}

impl LumaBandDetector {
    pub fn new(config: SubtitleDetectionConfig) -> Result<Self, SubtitleDetectionError> {
        let required_len = required_len(&config)?;
        let roi = compute_roi_rect(config.frame_width, config.frame_height, config.roi)?;
        Ok(Self {
            config,
            roi,
            required_len,
            workspace: Mutex::new(DetectorWorkspace::default()),
        })
    }
}

impl SubtitleDetector for LumaBandDetector {
    fn ensure_available(config: &SubtitleDetectionConfig) -> Result<(), SubtitleDetectionError> {
        required_len(config)?;
        let _ = compute_roi_rect(config.frame_width, config.frame_height, config.roi)?;
        Ok(())
    }

    fn detect(
        &self,
        frame: &YPlaneFrame,
    ) -> Result<SubtitleDetectionResult, SubtitleDetectionError> {
        let y_plane = frame.data();
        if y_plane.len() < self.required_len {
            return Err(SubtitleDetectionError::InsufficientData {
                data_len: y_plane.len(),
                required: self.required_len,
            });
        }

        if self.roi.width == 0 || self.roi.height == 0 {
            return Ok(SubtitleDetectionResult::empty());
        }

        let mut workspace = self.workspace.lock().expect("workspace poisoned");
        workspace.ensure_capacity(self.roi.width, self.roi.height);
        let MainBuffers {
            mask,
            integral,
            row_prefix,
            row_sums,
            left_edges,
            right_edges,
            labels,
            horiz,
            runs,
            offsets,
            morph_tmp,
            morph_prefix,
            morph_suffix,
            morph_line,
        } = workspace.detection_buffers(self.roi.width, self.roi.height);
        mask.fill(0);

        threshold_mask(
            y_plane,
            self.config.stride,
            self.roi,
            &self.config.luma_band,
            mask,
        );

        let roi_area = (self.roi.width * self.roi.height).max(1);
        let mask_sum = mask_population(mask);
        if sparse_mask_should_skip(mask_sum, roi_area) {
            return Ok(SubtitleDetectionResult::empty());
        }

        bridge_mask(
            mask,
            self.roi.width,
            self.roi.height,
            horiz,
            labels,
            morph_tmp,
            morph_prefix,
            morph_suffix,
            morph_line,
        );

        let mask_view: &[u8] = mask;

        let mut rects = Vec::new();
        if ROW_PROJECTION_ENABLED {
            rects.extend(row_projection_candidates(
                mask_view,
                self.roi.width,
                self.roi.height,
                row_sums,
                left_edges,
                right_edges,
            ));
        }

        if rects.is_empty() && COMPONENTS_FALLBACK_ENABLED {
            rects.extend(rle_component_candidates(
                mask_view,
                self.roi.width,
                self.roi.height,
                runs,
                offsets,
            ));
        }

        if rects.is_empty() {
            return Ok(SubtitleDetectionResult::empty());
        }

        build_integral(
            mask_view,
            self.roi.width,
            self.roi.height,
            integral,
            row_prefix,
        );

        let frame_area = self
            .config
            .frame_width
            .saturating_mul(self.config.frame_height) as f32;
        let max_rect_area = frame_area * MAX_AREA_RATIO;

        let mut candidates = Vec::new();
        for rect in rects {
            if rect.width == 0 || rect.height == 0 {
                continue;
            }
            let area = rect.width * rect.height;
            if area < MIN_AREA {
                continue;
            }
            if (area as f32) > max_rect_area {
                continue;
            }
            let aspect = rect.width as f32 / rect.height.max(1) as f32;
            if aspect < MIN_ASPECT_RATIO {
                continue;
            }
            let (fill, vmr, score) = evaluate_region(
                integral,
                self.roi.width,
                rect.x,
                rect.y,
                rect.width,
                rect.height,
                VMR_K,
            );
            candidates.push(Candidate {
                x: rect.x,
                y: rect.y,
                width: rect.width,
                height: rect.height,
                _fill: fill,
                _vmr: vmr,
                score,
            });
        }

        if candidates.is_empty() {
            return Ok(SubtitleDetectionResult::empty());
        }

        let mut merged = merge_line_candidates(candidates, integral, self.roi.width);
        if merged.is_empty() {
            return Ok(SubtitleDetectionResult::empty());
        }

        merged.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(Ordering::Equal));

        let mut regions = Vec::new();
        for cand in merged.iter().take(MAX_OUTPUT_REGIONS) {
            regions.push(DetectionRegion {
                x: (cand.x + self.roi.x) as f32,
                y: (cand.y + self.roi.y) as f32,
                width: cand.width as f32,
                height: cand.height as f32,
                score: 1.0,
            });
        }

        Ok(SubtitleDetectionResult {
            has_subtitle: !regions.is_empty(),
            max_score: if regions.is_empty() { 0.0 } else { 1.0 },
            regions,
        })
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

    let width = (end_x - start_x).max(0) as usize;
    let height = (end_y - start_y).max(0) as usize;
    if width == 0 || height == 0 {
        return Err(SubtitleDetectionError::EmptyRoi);
    }

    Ok(RoiRect {
        x: start_x as usize,
        y: start_y as usize,
        width,
        height,
    })
}

fn threshold_mask(
    data: &[u8],
    stride: usize,
    roi: RoiRect,
    params: &LumaBandConfig,
    mask: &mut [u8],
) {
    if mask.is_empty() {
        return;
    }

    let lo = params.target.saturating_sub(params.delta);
    let hi = params.target.saturating_add(params.delta);

    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") {
            unsafe {
                threshold_mask_avx2(data, stride, roi, lo, hi, mask);
            }
            return;
        }
        if is_x86_feature_detected!("sse2") {
            unsafe {
                threshold_mask_sse2(data, stride, roi, lo, hi, mask);
            }
            return;
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        if is_aarch64_feature_detected!("neon") {
            unsafe {
                threshold_mask_neon(data, stride, roi, lo, hi, mask);
            }
            return;
        }
    }

    threshold_mask_scalar(data, stride, roi, lo, hi, mask);
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
        threshold_mask_scalar_row(src, dst, lo, hi);
    }
}

#[inline(always)]
fn threshold_mask_scalar_row(src: &[u8], dst: &mut [u8], lo: u8, hi: u8) {
    for (value, out) in src.iter().zip(dst.iter_mut()) {
        *out = u8::from(*value >= lo && *value <= hi);
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn threshold_mask_avx2(
    data: &[u8],
    stride: usize,
    roi: RoiRect,
    lo: u8,
    hi: u8,
    mask: &mut [u8],
) {
    let width = roi.width;
    let bias = _mm256_set1_epi8(-128);
    let lo_vec = _mm256_xor_si256(_mm256_set1_epi8(lo as i8), bias);
    let hi_vec = _mm256_xor_si256(_mm256_set1_epi8(hi as i8), bias);
    let ones = _mm256_set1_epi8(1);
    let all = _mm256_set1_epi8(-1);

    for row in 0..roi.height {
        let src_ptr = data.as_ptr().add((roi.y + row) * stride + roi.x);
        let dst_ptr = mask.as_mut_ptr().add(row * width);
        let mut x = 0usize;
        while x + 32 <= width {
            let pixels =
                _mm256_xor_si256(_mm256_loadu_si256(src_ptr.add(x) as *const __m256i), bias);
            let ge_lo = _mm256_andnot_si256(_mm256_cmpgt_epi8(lo_vec, pixels), all);
            let le_hi = _mm256_andnot_si256(_mm256_cmpgt_epi8(pixels, hi_vec), all);
            let mask_vec = _mm256_and_si256(_mm256_and_si256(ge_lo, le_hi), ones);
            _mm256_storeu_si256(dst_ptr.add(x) as *mut __m256i, mask_vec);
            x += 32;
        }
        if x < width {
            let remaining = width - x;
            let src_tail = std::slice::from_raw_parts(src_ptr.add(x), remaining);
            let dst_tail = std::slice::from_raw_parts_mut(dst_ptr.add(x), remaining);
            threshold_mask_scalar_row(src_tail, dst_tail, lo, hi);
        }
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
    let width = roi.width;
    let lo_vec = _mm_set1_epi8(lo as i8);
    let hi_vec = _mm_set1_epi8(hi as i8);
    let ones = _mm_set1_epi8(1);

    for row in 0..roi.height {
        let src_ptr = data.as_ptr().add((roi.y + row) * stride + roi.x);
        let dst_ptr = mask.as_mut_ptr().add(row * width);
        let mut x = 0usize;
        while x + 16 <= width {
            let pixels = _mm_loadu_si128(src_ptr.add(x) as *const __m128i);
            let ge_lo = _mm_cmpeq_epi8(pixels, _mm_max_epu8(pixels, lo_vec));
            let le_hi = _mm_cmpeq_epi8(pixels, _mm_min_epu8(pixels, hi_vec));
            let mask_vec = _mm_and_si128(_mm_and_si128(ge_lo, le_hi), ones);
            _mm_storeu_si128(dst_ptr.add(x) as *mut __m128i, mask_vec);
            x += 16;
        }
        if x < width {
            let remaining = width - x;
            let src_tail = std::slice::from_raw_parts(src_ptr.add(x), remaining);
            let dst_tail = std::slice::from_raw_parts_mut(dst_ptr.add(x), remaining);
            threshold_mask_scalar_row(src_tail, dst_tail, lo, hi);
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
        while x + 32 <= width {
            let pixels0 = vld1q_u8(src_ptr.add(x));
            let pixels1 = vld1q_u8(src_ptr.add(x + 16));
            let mask0 = vandq_u8(
                vandq_u8(
                    vceqq_u8(pixels0, vmaxq_u8(pixels0, lo_vec)),
                    vceqq_u8(pixels0, vminq_u8(pixels0, hi_vec)),
                ),
                ones,
            );
            let mask1 = vandq_u8(
                vandq_u8(
                    vceqq_u8(pixels1, vmaxq_u8(pixels1, lo_vec)),
                    vceqq_u8(pixels1, vminq_u8(pixels1, hi_vec)),
                ),
                ones,
            );
            vst1q_u8(dst_ptr.add(x), mask0);
            vst1q_u8(dst_ptr.add(x + 16), mask1);
            x += 32;
        }
        while x + 16 <= width {
            let pixels = vld1q_u8(src_ptr.add(x));
            let mask_vec = vandq_u8(
                vandq_u8(
                    vceqq_u8(pixels, vmaxq_u8(pixels, lo_vec)),
                    vceqq_u8(pixels, vminq_u8(pixels, hi_vec)),
                ),
                ones,
            );
            vst1q_u8(dst_ptr.add(x), mask_vec);
            x += 16;
        }
        if x < width {
            let remaining = width - x;
            let src_tail = std::slice::from_raw_parts(src_ptr.add(x), remaining);
            let dst_tail = std::slice::from_raw_parts_mut(dst_ptr.add(x), remaining);
            threshold_mask_scalar_row(src_tail, dst_tail, lo, hi);
        }
    }
}

fn mask_population(mask: &[u8]) -> usize {
    #[cfg(feature = "detector-parallel")]
    {
        return mask
            .par_chunks(1024)
            .map(|chunk| chunk.iter().map(|&b| b as usize).sum::<usize>())
            .sum();
    }

    #[cfg(not(feature = "detector-parallel"))]
    {
        mask.iter().map(|&b| b as usize).sum()
    }
}

fn sparse_mask_should_skip(mask_population: usize, roi_area: usize) -> bool {
    let area = roi_area.max(1);
    let density = mask_population as f32 / area as f32;
    let min_population = min_mask_population(area);
    // Skip only when both the relative density and absolute bright-pixel count
    // indicate an empty frame to keep sparse-but-real bands alive.
    density < MASK_DENSITY_THRESHOLD && mask_population < min_population
}

fn min_mask_population(roi_area: usize) -> usize {
    let scaled = ((roi_area as f32) * MASK_POPULATION_RATIO).ceil() as usize;
    cmp::max(MIN_MASK_POPULATION, scaled)
}

fn bridge_mask(
    mask: &mut [u8],
    width: usize,
    height: usize,
    horiz: &mut [u16],
    labels: &mut [u32],
    morph_plane: &mut [u8],
    morph_prefix: &mut [u8],
    morph_suffix: &mut [u8],
    morph_line: &mut [u8],
) {
    match GAP_FILL_MODE {
        GapFillMode::Distance => {
            dp_bridge_horizontal(mask, width, height, HORIZONTAL_GAP, horiz);
            dp_bridge_vertical(mask, width, height, VERTICAL_GAP, labels);
        }
        GapFillMode::Closing => {
            vhgw_close_rows(
                mask,
                width,
                height,
                HORIZONTAL_GAP,
                morph_plane,
                morph_prefix,
                morph_suffix,
                morph_line,
            );
            vhgw_close_cols(
                mask,
                width,
                height,
                VERTICAL_GAP,
                morph_plane,
                morph_prefix,
                morph_suffix,
                morph_line,
            );
        }
    }
}

fn dp_bridge_horizontal(
    mask: &mut [u8],
    width: usize,
    height: usize,
    gap: usize,
    scratch: &mut [u16],
) {
    if gap == 0 || width == 0 {
        return;
    }
    let buf = &mut scratch[..width * height];
    buf.fill(0);
    let gap_u16 = gap.min(u16::MAX as usize) as u16;

    #[cfg(feature = "detector-parallel")]
    {
        let mask_rows: &[u8] = mask;
        mask_rows
            .par_chunks(width)
            .zip(buf.par_chunks_mut(width))
            .for_each(|(src, dst)| horizontal_left_pass(src, dst, gap_u16));
    }

    #[cfg(not(feature = "detector-parallel"))]
    {
        for (row, row_data) in mask.chunks(width).enumerate() {
            let buf_row = &mut buf[row * width..(row + 1) * width];
            horizontal_left_pass(row_data, buf_row, gap_u16);
        }
    }

    #[cfg(feature = "detector-parallel")]
    {
        let buf_rows: &[u16] = buf;
        mask.par_chunks_mut(width)
            .zip(buf_rows.par_chunks(width))
            .for_each(|(row, buf_row)| horizontal_right_apply(row, buf_row, gap, gap_u16));
    }

    #[cfg(not(feature = "detector-parallel"))]
    {
        for (row, row_data) in mask.chunks_mut(width).enumerate() {
            let buf_row = &buf[row * width..(row + 1) * width];
            horizontal_right_apply(row_data, buf_row, gap, gap_u16);
        }
    }
}

fn horizontal_left_pass(row: &[u8], buf: &mut [u16], gap: u16) {
    let mut last = 0u16;
    for (value, slot) in row.iter().zip(buf.iter_mut()) {
        if *value != 0 {
            last = 0;
            *slot = 0;
        } else {
            last = last.saturating_add(1).min(gap);
            *slot = last;
        }
    }
}

fn horizontal_right_apply(row: &mut [u8], buf: &[u16], gap: usize, gap_u16: u16) {
    let mut last = 0u16;
    for idx in (0..row.len()).rev() {
        if row[idx] != 0 {
            last = 0;
            continue;
        }
        last = last.saturating_add(1).min(gap_u16);
        let left = buf[idx];
        if left != 0 && last != 0 && (left as usize + last as usize) <= gap {
            row[idx] = 1;
        }
    }
}

fn dp_bridge_vertical(
    mask: &mut [u8],
    width: usize,
    height: usize,
    gap: usize,
    labels: &mut [u32],
) {
    if gap == 0 || height == 0 {
        return;
    }
    let buf = &mut labels[..width * height];
    buf.fill(0);

    #[cfg(feature = "detector-parallel")]
    {
        let mask_ptr = RawPtr(mask.as_mut_ptr());
        let buf_ptr = RawPtr(buf.as_mut_ptr());
        (0..width).into_par_iter().for_each(move |col| unsafe {
            vertical_column_pass(mask_ptr, buf_ptr, width, height, gap, col);
        });
    }

    #[cfg(not(feature = "detector-parallel"))]
    {
        for col in 0..width {
            unsafe {
                vertical_column_pass(
                    RawPtr(mask.as_mut_ptr()),
                    RawPtr(buf.as_mut_ptr()),
                    width,
                    height,
                    gap,
                    col,
                );
            }
        }
    }
}

unsafe fn vertical_column_pass(
    mask_ptr: RawPtr<u8>,
    buf_ptr: RawPtr<u32>,
    width: usize,
    height: usize,
    gap: usize,
    col: usize,
) {
    let gap_u32 = gap.min(u32::MAX as usize) as u32;
    let mut last = 0u32;
    for y in 0..height {
        let idx = y * width + col;
        let pixel = mask_ptr.0.add(idx);
        if *pixel != 0 {
            last = 0;
            *buf_ptr.0.add(idx) = 0;
        } else {
            last = last.saturating_add(1).min(gap_u32);
            *buf_ptr.0.add(idx) = last;
        }
    }
    last = 0;
    for y in (0..height).rev() {
        let idx = y * width + col;
        let pixel = mask_ptr.0.add(idx);
        if *pixel != 0 {
            last = 0;
            continue;
        }
        last = last.saturating_add(1).min(gap_u32);
        let up = *buf_ptr.0.add(idx);
        if up != 0 && last != 0 && (up as usize + last as usize) <= gap {
            *pixel = 1;
        }
    }
}

fn vhgw_close_rows(
    mask: &mut [u8],
    width: usize,
    height: usize,
    gap: usize,
    tmp_plane: &mut [u8],
    prefix: &mut [u8],
    suffix: &mut [u8],
    line: &mut [u8],
) {
    if gap == 0 || width == 0 {
        return;
    }
    let plane = width * height;
    let tmp = &mut tmp_plane[..plane];
    tmp.fill(0);
    let prefix_slice = &mut prefix[..width];
    let suffix_slice = &mut suffix[..width];
    let line_buf = &mut line[..width];
    for (src, dst) in mask.chunks(width).zip(tmp.chunks_mut(width)) {
        vhgw_close_line(src, dst, prefix_slice, suffix_slice, line_buf, gap);
    }

    mask.copy_from_slice(tmp);
}

fn vhgw_close_cols(
    mask: &mut [u8],
    width: usize,
    height: usize,
    gap: usize,
    tmp_plane: &mut [u8],
    prefix: &mut [u8],
    suffix: &mut [u8],
    line: &mut [u8],
) {
    if gap == 0 || height == 0 {
        return;
    }
    let plane = width * height;
    let tmp = &mut tmp_plane[..plane];
    let prefix_slice = &mut prefix[..height];
    let suffix_slice = &mut suffix[..height];
    let line_buf = &mut line[..height];
    tmp.copy_from_slice(mask);
    let mut column = vec![0u8; height];
    let mut out = vec![0u8; height];

    for col in 0..width {
        for y in 0..height {
            column[y] = tmp[y * width + col];
        }
        vhgw_close_line(&column, &mut out, prefix_slice, suffix_slice, line_buf, gap);
        for y in 0..height {
            mask[y * width + col] = out[y];
        }
    }
}

fn vhgw_close_line(
    src: &[u8],
    dst: &mut [u8],
    prefix: &mut [u8],
    suffix: &mut [u8],
    scratch: &mut [u8],
    gap: usize,
) {
    if src.is_empty() {
        return;
    }
    vhgw_line_op(src, prefix, suffix, gap, true, scratch);
    vhgw_line_op(scratch, prefix, suffix, gap, false, dst);
}

fn vhgw_line_op(
    src: &[u8],
    prefix: &mut [u8],
    suffix: &mut [u8],
    gap: usize,
    is_dilation: bool,
    dst: &mut [u8],
) {
    let window = gap.max(1);
    let len = src.len();
    let combine = if is_dilation { u8::max } else { u8::min };

    let mut i = 0usize;
    while i < len {
        if i % window == 0 {
            prefix[i] = src[i];
        } else {
            prefix[i] = combine(prefix[i - 1], src[i]);
        }
        i += 1;
    }

    if let Some(last) = len.checked_sub(1) {
        let mut j = last as isize;
        while j >= 0 {
            let idx = j as usize;
            if idx == last || ((idx + 1) % window == 0) {
                suffix[idx] = src[idx];
            } else {
                suffix[idx] = combine(suffix[idx + 1], src[idx]);
            }
            j -= 1;
        }

        for idx in 0..len {
            let end = cmp::min(idx + window - 1, last);
            let left = prefix[end];
            let right = suffix[idx];
            dst[idx] = combine(left, right);
        }
    }
}

fn row_projection_candidates(
    mask: &[u8],
    width: usize,
    height: usize,
    row_sums: &mut [u32],
    left_edges: &mut [usize],
    right_edges: &mut [usize],
) -> Vec<RectCandidate> {
    let mut rects = Vec::new();
    if width == 0 || height == 0 {
        return rects;
    }

    compute_row_stats(mask, width, row_sums, left_edges, right_edges);

    let min_sum = (ROW_DENSITY_THRESHOLD * width as f32).ceil() as u32;
    let min_rows = cmp::max(1, MIN_BAND_ROWS);

    let mut row = 0usize;
    while row < height {
        while row < height && row_sums[row] < min_sum {
            row += 1;
        }
        if row >= height {
            break;
        }
        let start = row;
        let mut left = width;
        let mut right = 0usize;
        while row < height && row_sums[row] >= min_sum {
            left = left.min(left_edges[row]);
            right = right.max(right_edges[row]);
            row += 1;
        }
        let band_height = row - start;
        if band_height >= min_rows && right > left {
            let (top, bottom) = expand_row_band(start, row, height);
            rects.push(RectCandidate {
                x: left,
                y: top,
                width: right - left,
                height: bottom.saturating_sub(top),
            });
        }
    }

    rects
}

fn expand_row_band(mut top: usize, mut bottom: usize, height: usize) -> (usize, usize) {
    if bottom <= top {
        bottom = cmp::min(top + 1, height);
    }
    if bottom <= top {
        return (top, bottom);
    }
    while bottom - top < MIN_ROW_BAND_PX {
        let mut expanded = false;
        if top > 0 {
            top -= 1;
            expanded = true;
        }
        if bottom < height && bottom - top < MIN_ROW_BAND_PX {
            bottom += 1;
            expanded = true;
        }
        if !expanded {
            break;
        }
    }
    (top, bottom.min(height))
}

fn compute_row_stats(
    mask: &[u8],
    width: usize,
    row_sums: &mut [u32],
    left_edges: &mut [usize],
    right_edges: &mut [usize],
) {
    #[cfg(feature = "detector-parallel")]
    {
        let mask_rows: &[u8] = mask;
        row_sums
            .par_iter_mut()
            .zip(left_edges.par_iter_mut())
            .zip(right_edges.par_iter_mut())
            .enumerate()
            .for_each(|(row, ((sum_slot, left_slot), right_slot))| {
                let data = &mask_rows[row * width..(row + 1) * width];
                let mut sum = 0u32;
                let mut left = width;
                let mut right = 0usize;
                for (idx, &value) in data.iter().enumerate() {
                    if value != 0 {
                        left = left.min(idx);
                        right = right.max(idx + 1);
                    }
                    sum += value as u32;
                }
                *sum_slot = sum;
                *left_slot = left;
                *right_slot = right;
            });
        return;
    }

    #[cfg(not(feature = "detector-parallel"))]
    {
        for (row, data) in mask.chunks(width).enumerate() {
            let mut sum = 0u32;
            let mut left = width;
            let mut right = 0usize;
            for (idx, &value) in data.iter().enumerate() {
                if value != 0 {
                    left = left.min(idx);
                    right = right.max(idx + 1);
                }
                sum += value as u32;
            }
            row_sums[row] = sum;
            left_edges[row] = left;
            right_edges[row] = right;
        }
    }
}

fn rle_component_candidates(
    mask: &[u8],
    width: usize,
    height: usize,
    runs: &mut Vec<RowRun>,
    offsets: &mut [usize],
) -> Vec<RectCandidate> {
    let mut rects = Vec::new();
    if width == 0 || height == 0 {
        return rects;
    }

    runs.clear();

    let mut cursor = 0usize;
    for row in 0..height {
        offsets[row] = cursor;
        let row_data = &mask[row * width..(row + 1) * width];
        let mut x = 0usize;
        while x < width {
            while x < width && row_data[x] == 0 {
                x += 1;
            }
            if x >= width {
                break;
            }
            let start = x;
            while x < width && row_data[x] != 0 {
                x += 1;
            }
            runs.push(RowRun {
                start,
                end: x,
                row,
                label: 0,
            });
            cursor += 1;
        }
    }
    offsets[height] = cursor;

    if runs.is_empty() {
        return rects;
    }

    let mut dsu = DisjointSet::new();
    for run in runs.iter_mut() {
        run.label = dsu.make_set();
    }

    for row in 1..height {
        let mut prev = offsets[row - 1];
        let prev_end = offsets[row];
        let mut curr = offsets[row];
        let curr_end = offsets[row + 1];

        while prev < prev_end && curr < curr_end {
            let run_a = runs[prev];
            let run_b = runs[curr];
            if runs_touch(&run_a, &run_b) {
                dsu.union(run_a.label, run_b.label);
            }
            if run_a.end <= run_b.end {
                prev += 1;
            } else {
                curr += 1;
            }
        }
    }

    let mut stats = vec![None; dsu.len()];
    for run in runs.iter() {
        let root = dsu.find(run.label);
        let entry =
            stats[root as usize].get_or_insert_with(|| ComponentStats::new(run.start, run.row));
        let width = run.end.saturating_sub(run.start);
        entry.area += width;
        entry.min_x = entry.min_x.min(run.start);
        entry.max_x = entry.max_x.max(run.end.saturating_sub(1));
        entry.min_y = entry.min_y.min(run.row);
        entry.max_y = entry.max_y.max(run.row);
    }

    for comp in stats.into_iter().flatten() {
        rects.push(RectCandidate {
            x: comp.min_x,
            y: comp.min_y,
            width: comp.max_x.saturating_sub(comp.min_x) + 1,
            height: comp.max_y.saturating_sub(comp.min_y) + 1,
        });
    }

    rects
}

fn runs_touch(a: &RowRun, b: &RowRun) -> bool {
    let overlap = cmp::min(a.end, b.end).saturating_sub(cmp::max(a.start, b.start));
    if overlap > 0 {
        return true;
    }
    let gap = if a.end <= b.start {
        b.start - a.end
    } else {
        a.start - b.end
    };
    gap <= 1
}

fn build_integral(
    mask: &[u8],
    width: usize,
    height: usize,
    integral: &mut [u32],
    row_prefix: &mut [u32],
) {
    compute_horizontal_prefix(mask, width, row_prefix);
    accumulate_columns(row_prefix, width, height, integral);
}

fn compute_horizontal_prefix(mask: &[u8], width: usize, row_prefix: &mut [u32]) {
    #[cfg(feature = "detector-parallel")]
    {
        mask.par_chunks(width)
            .zip(row_prefix.par_chunks_mut(width))
            .for_each(|(src, dst)| {
                let mut acc = 0u32;
                for (idx, &value) in src.iter().enumerate() {
                    acc += value as u32;
                    dst[idx] = acc;
                }
            });
        return;
    }

    #[cfg(not(feature = "detector-parallel"))]
    {
        for (row, src) in mask.chunks(width).enumerate() {
            let dst = &mut row_prefix[row * width..(row + 1) * width];
            let mut acc = 0u32;
            for (idx, &value) in src.iter().enumerate() {
                acc += value as u32;
                dst[idx] = acc;
            }
        }
    }
}

fn accumulate_columns(row_prefix: &[u32], width: usize, height: usize, integral: &mut [u32]) {
    let stride = width + 1;
    integral.fill(0);

    #[cfg(feature = "detector-parallel")]
    {
        let integral_ptr = RawPtr(integral.as_mut_ptr());
        let prefix_ptr = RawConstPtr(row_prefix.as_ptr());
        (0..width).into_par_iter().for_each(move |col| {
            let mut acc = 0u32;
            for row in 0..height {
                let value = unsafe { prefix_ptr.read(row * width + col) };
                acc += value;
                let idx = (row + 1) * stride + (col + 1);
                unsafe { integral_ptr.write(idx, acc) };
            }
        });
        return;
    }

    #[cfg(not(feature = "detector-parallel"))]
    {
        for col in 0..width {
            let mut acc = 0u32;
            for row in 0..height {
                acc += row_prefix[row * width + col];
                let idx = (row + 1) * stride + (col + 1);
                integral[idx] = acc;
            }
        }
    }
}

fn rect_sum(integral: &[u32], width: usize, x0: usize, y0: usize, x1: usize, y1: usize) -> u32 {
    let stride = width + 1;
    let idx = |x: usize, y: usize| -> usize { y * stride + x };
    integral[idx(x1, y1)]
        .wrapping_sub(integral[idx(x0, y1)])
        .wrapping_sub(integral[idx(x1, y0)])
        .wrapping_add(integral[idx(x0, y0)])
}

fn evaluate_region(
    integral: &[u32],
    width: usize,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    vmr_k: usize,
) -> (f32, f32, f32) {
    let x1 = x + w;
    let y1 = y + h;
    let hits = rect_sum(integral, width, x, y, x1, y1) as f32;
    let area = (w * h) as f32 + 1e-6;
    let fill = hits / area;

    let mut vmr = 0.0f32;
    if w >= vmr_k && h >= vmr_k {
        let dx = cmp::max(w / vmr_k, 1);
        let dy = cmp::max(h / vmr_k, 1);
        let mut count = 0.0f32;
        let mut mean = 0.0f32;
        let mut m2 = 0.0f32;

        let mut yy = y;
        while yy < y1 {
            let yb = cmp::min(yy + dy, y1);
            let mut xx = x;
            while xx < x1 {
                let xb = cmp::min(xx + dx, x1);
                let value = rect_sum(integral, width, xx, yy, xb, yb) as f32;
                count += 1.0;
                let delta = value - mean;
                mean += delta / count;
                let delta2 = value - mean;
                m2 += delta * delta2;
                xx += dx;
            }
            yy += dy;
        }

        if count > 0.0 {
            let variance = if count <= 1.0 { 0.0 } else { m2 / count };
            vmr = variance / (mean + 1e-6);
        }
    }

    let score = fill - 0.1 * vmr;
    (fill, vmr, score)
}

#[derive(Clone)]
struct Candidate {
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    _fill: f32,
    _vmr: f32,
    score: f32,
}

fn merge_line_candidates(
    mut candidates: Vec<Candidate>,
    integral: &[u32],
    width: usize,
) -> Vec<Candidate> {
    if candidates.is_empty() {
        return Vec::new();
    }

    candidates.sort_by(|a, b| a.y.cmp(&b.y));
    let mut merged = Vec::with_capacity(candidates.len());
    let mut line_group = vec![candidates[0].clone()];

    for cand in candidates.into_iter().skip(1) {
        if same_line(line_group.last().unwrap(), &cand) {
            line_group.push(cand);
        } else {
            merged.extend(merge_group(line_group, integral, width));
            line_group = vec![cand];
        }
    }

    merged.extend(merge_group(line_group, integral, width));
    merged
}

fn same_line(a: &Candidate, b: &Candidate) -> bool {
    let cy1 = a.y + a.height / 2;
    let cy2 = b.y + b.height / 2;
    let diff = cy1.abs_diff(cy2);
    if diff <= Y_MERGE_TOL {
        return true;
    }
    let y0 = cmp::max(a.y, b.y);
    let y1 = cmp::min(a.y + a.height, b.y + b.height);
    if y1 <= y0 {
        return false;
    }
    let overlap = (y1 - y0) as f32;
    let min_height = cmp::min(a.height, b.height).max(1) as f32;
    (overlap / min_height) >= Y_OVERLAP_RATIO
}

fn merge_group(mut group: Vec<Candidate>, integral: &[u32], width: usize) -> Vec<Candidate> {
    if group.is_empty() {
        return Vec::new();
    }
    group.sort_by(|a, b| a.x.cmp(&b.x));
    let mut result = Vec::with_capacity(group.len());
    let mut iter = group.into_iter();
    let mut current = iter.next().unwrap();

    for candidate in iter {
        if should_merge(&current, &candidate) {
            current = merge_candidates(&current, &candidate, integral, width);
        } else {
            result.push(current);
            current = candidate;
        }
    }
    result.push(current);
    result
}

fn should_merge(a: &Candidate, b: &Candidate) -> bool {
    let overlap = candidate_iou(a, b);
    let near = b.x <= a.x + a.width + NEAR_GAP;
    overlap >= IOU_MERGE || near
}

fn candidate_iou(a: &Candidate, b: &Candidate) -> f32 {
    let x0 = cmp::max(a.x, b.x);
    let y0 = cmp::max(a.y, b.y);
    let x1 = cmp::min(a.x + a.width, b.x + b.width);
    let y1 = cmp::min(a.y + a.height, b.y + b.height);
    let inter_w = x1.saturating_sub(x0);
    let inter_h = y1.saturating_sub(y0);
    if inter_w == 0 || inter_h == 0 {
        return 0.0;
    }
    let intersection = (inter_w * inter_h) as f32;
    let area_a = (a.width * a.height) as f32;
    let area_b = (b.width * b.height) as f32;
    let union = area_a + area_b - intersection + 1e-6;
    intersection / union
}

fn merge_candidates(a: &Candidate, b: &Candidate, integral: &[u32], width: usize) -> Candidate {
    let x0 = cmp::min(a.x, b.x);
    let y0 = cmp::min(a.y, b.y);
    let x1 = cmp::max(a.x + a.width, b.x + b.width);
    let y1 = cmp::max(a.y + a.height, b.y + b.height);
    let new_width = x1.saturating_sub(x0);
    let new_height = y1.saturating_sub(y0);
    let (fill, vmr, score) = evaluate_region(integral, width, x0, y0, new_width, new_height, VMR_K);
    Candidate {
        x: x0,
        y: y0,
        width: new_width,
        height: new_height,
        _fill: fill,
        _vmr: vmr,
        score,
    }
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

#[derive(Default)]
struct DisjointSet {
    parent: Vec<u32>,
    rank: Vec<u8>,
}

impl DisjointSet {
    fn new() -> Self {
        Self {
            parent: vec![0],
            rank: vec![0],
        }
    }

    fn len(&self) -> usize {
        self.parent.len()
    }

    fn make_set(&mut self) -> u32 {
        let idx = self.parent.len() as u32;
        self.parent.push(idx);
        self.rank.push(0);
        idx
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparse_frames_with_too_few_pixels_are_skipped() {
        let area = 1920 * 824;
        assert!(sparse_mask_should_skip(200, area));
    }

    #[test]
    fn sparse_frames_with_enough_pixels_continue() {
        let area = 1920 * 824;
        // Density is below MASK_DENSITY_THRESHOLD, but population exceeds the scaled watermark.
        assert!(!sparse_mask_should_skip(1100, area));
    }

    #[test]
    fn small_roi_uses_absolute_floor() {
        let area = 200 * 100;
        let min_pop = min_mask_population(area);
        assert!(min_pop >= MIN_MASK_POPULATION);
        assert!(!sparse_mask_should_skip(min_pop + 10, area));
    }

    #[test]
    fn row_band_expands_to_min_height() {
        let (top, bottom) = super::expand_row_band(800, 804, 824);
        assert!(bottom - top >= MIN_ROW_BAND_PX);
        assert!(top <= 800);
        assert!(bottom >= 804);
    }
}
