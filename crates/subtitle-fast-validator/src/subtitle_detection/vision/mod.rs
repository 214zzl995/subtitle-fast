use std::ffi::{CStr, c_char};
use std::slice;

use super::{
    DetectionRegion, RoiConfig, SubtitleDetectionConfig, SubtitleDetectionError,
    SubtitleDetectionResult, SubtitleDetector,
};
use subtitle_fast_types::PlaneFrame;

#[derive(Debug, Clone)]
pub struct VisionTextDetector {
    config: SubtitleDetectionConfig,
    roi: RoiRect,
    required_bytes: usize,
}

#[repr(C)]
struct CVisionRegion {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    confidence: f32,
}

#[repr(C)]
struct CVisionResult {
    regions: *mut CVisionRegion,
    count: usize,
    error: *mut c_char,
}

unsafe extern "C" {
    fn vision_detect_text_regions(
        data: *const u8,
        width: usize,
        height: usize,
        stride: usize,
        roi_x: f32,
        roi_y: f32,
        roi_width: f32,
        roi_height: f32,
    ) -> CVisionResult;

    fn vision_result_destroy(result: CVisionResult);
}

struct OwnedVisionResult {
    raw: CVisionResult,
}

impl OwnedVisionResult {
    fn new(raw: CVisionResult) -> Self {
        Self { raw }
    }

    fn error_message(&self) -> Option<String> {
        if self.raw.error.is_null() {
            None
        } else {
            Some(unsafe {
                CStr::from_ptr(self.raw.error)
                    .to_string_lossy()
                    .into_owned()
            })
        }
    }

    fn regions(&self) -> &[CVisionRegion] {
        if self.raw.count == 0 || self.raw.regions.is_null() {
            &[]
        } else {
            unsafe { slice::from_raw_parts(self.raw.regions, self.raw.count) }
        }
    }
}

impl Drop for OwnedVisionResult {
    fn drop(&mut self) {
        unsafe {
            vision_result_destroy(CVisionResult {
                regions: self.raw.regions,
                count: self.raw.count,
                error: self.raw.error,
            });
        }
        self.raw.regions = std::ptr::null_mut();
        self.raw.error = std::ptr::null_mut();
        self.raw.count = 0;
    }
}

#[derive(Debug, Clone, Copy)]
struct RoiRect {
    x: usize,
    y: usize,
    width: usize,
    height: usize,
}

impl VisionTextDetector {
    pub fn new(config: SubtitleDetectionConfig) -> Result<Self, SubtitleDetectionError> {
        let required_bytes = config
            .stride
            .checked_mul(config.frame_height)
            .unwrap_or(usize::MAX);
        if required_bytes == usize::MAX {
            return Err(SubtitleDetectionError::InsufficientData {
                data_len: 0,
                required: required_bytes,
            });
        }
        let roi = compute_roi_rect(config.frame_width, config.frame_height, config.roi)?;
        Ok(Self {
            config,
            roi,
            required_bytes,
        })
    }
}

impl SubtitleDetector for VisionTextDetector {
    fn ensure_available(config: &SubtitleDetectionConfig) -> Result<(), SubtitleDetectionError> {
        let _ = VisionTextDetector::new(config.clone())?;
        Ok(())
    }

    fn detect(
        &self,
        frame: &PlaneFrame,
    ) -> Result<SubtitleDetectionResult, SubtitleDetectionError> {
        let y_plane = frame.data();
        if y_plane.len() < self.required_bytes {
            return Err(SubtitleDetectionError::InsufficientData {
                data_len: y_plane.len(),
                required: self.required_bytes,
            });
        }

        let raw = unsafe {
            vision_detect_text_regions(
                y_plane.as_ptr(),
                self.config.frame_width,
                self.config.frame_height,
                self.config.stride,
                self.config.roi.x,
                self.config.roi.y,
                self.config.roi.width,
                self.config.roi.height,
            )
        };
        let owned = OwnedVisionResult::new(raw);
        if let Some(message) = owned.error_message() {
            return Err(SubtitleDetectionError::Vision(message));
        }

        let mut regions = Vec::new();
        let mut max_score = 0.0f32;
        for region in owned.regions() {
            if let Some(clipped) = clip_region(
                region,
                self.roi,
                self.config.frame_width,
                self.config.frame_height,
            ) {
                max_score = max_score.max(clipped.score);
                regions.push(clipped);
            }
        }

        let has_subtitle = !regions.is_empty();
        let result = SubtitleDetectionResult {
            has_subtitle,
            max_score,
            regions,
        };

        Ok(result)
    }
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

    let width = (end_x - start_x) as usize;
    let height = (end_y - start_y) as usize;
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

fn clip_region(
    region: &CVisionRegion,
    roi: RoiRect,
    frame_width: usize,
    frame_height: usize,
) -> Option<DetectionRegion> {
    if region.width <= 0.0 || region.height <= 0.0 {
        return None;
    }

    let frame_w = frame_width as f32;
    let frame_h = frame_height as f32;
    let roi_x1 = roi.x as f32;
    let roi_y1 = roi.y as f32;
    let roi_x2 = roi_x1 + roi.width as f32;
    let roi_y2 = roi_y1 + roi.height as f32;

    let x1 = region.x.max(0.0);
    let y1 = region.y.max(0.0);
    let x2 = (region.x + region.width).min(frame_w);
    let y2 = (region.y + region.height).min(frame_h);

    let ix1 = x1.max(roi_x1);
    let iy1 = y1.max(roi_y1);
    let ix2 = x2.min(roi_x2);
    let iy2 = y2.min(roi_y2);

    if ix2 <= ix1 || iy2 <= iy1 {
        return None;
    }

    Some(DetectionRegion {
        x: ix1,
        y: iy1,
        width: ix2 - ix1,
        height: iy2 - iy1,
        score: region.confidence.max(0.0),
    })
}
