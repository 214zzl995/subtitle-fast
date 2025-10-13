use std::ffi::{c_char, CStr};
use std::slice;

use super::{
    resolve_roi, DetectionRegion, PixelRect, SubtitleDetectionConfig, SubtitleDetectionError,
    SubtitleDetectionResult, SubtitleDetector,
};

#[derive(Debug, Clone)]
pub struct VisionTextDetector {
    config: SubtitleDetectionConfig,
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

extern "C" {
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
        let _ = resolve_roi(config.frame_width, config.frame_height, config.roi, None)?;
        Ok(Self {
            config,
            required_bytes,
        })
    }
}

impl SubtitleDetector for VisionTextDetector {
    fn ensure_available(config: &SubtitleDetectionConfig) -> Result<(), SubtitleDetectionError> {
        let _ = VisionTextDetector::new(config.clone())?;
        Ok(())
    }

    fn detect(&self, y_plane: &[u8]) -> Result<SubtitleDetectionResult, SubtitleDetectionError> {
        if y_plane.len() < self.required_bytes {
            return Err(SubtitleDetectionError::InsufficientData {
                data_len: y_plane.len(),
                required: self.required_bytes,
            });
        }

        let roi_cfg = self.config.roi;
        let roi_rect = resolve_roi(
            self.config.frame_width,
            self.config.frame_height,
            roi_cfg,
            None,
        )?;
        let raw = unsafe {
            vision_detect_text_regions(
                y_plane.as_ptr(),
                self.config.frame_width,
                self.config.frame_height,
                self.config.stride,
                roi_cfg.x,
                roi_cfg.y,
                roi_cfg.width,
                roi_cfg.height,
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
                roi_rect,
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

fn clip_region(
    region: &CVisionRegion,
    roi: PixelRect,
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
    let roi_x2 = (roi.x + roi.width) as f32;
    let roi_y2 = (roi.y + roi.height) as f32;

    let x0 = region.x.max(roi_x1).max(0.0);
    let y0 = region.y.max(roi_y1).max(0.0);
    let x1 = (region.x + region.width).min(roi_x2).min(frame_w);
    let y1 = (region.y + region.height).min(roi_y2).min(frame_h);

    if x1 <= x0 || y1 <= y0 {
        return None;
    }

    Some(DetectionRegion {
        x: x0,
        y: y0,
        width: x1 - x0,
        height: y1 - y0,
        score: region.confidence.max(0.0),
    })
}
