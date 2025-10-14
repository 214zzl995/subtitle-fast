use std::ffi::CStr;
use std::ptr;
use std::slice;

use crate::{OcrEngine, OcrError, OcrRegion, OcrRequest, OcrResponse, OcrText};

#[repr(C)]
#[derive(Clone, Copy)]
struct CVisionOcrRect {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

#[repr(C)]
struct CVisionOcrText {
    rect: CVisionOcrRect,
    confidence: f32,
    text: *mut std::os::raw::c_char,
}

#[repr(C)]
struct CVisionOcrResult {
    texts: *mut CVisionOcrText,
    count: usize,
    error: *mut std::os::raw::c_char,
}

unsafe extern "C" {
    fn vision_recognize_text(
        data: *const u8,
        width: usize,
        height: usize,
        stride: usize,
        regions: *const CVisionOcrRect,
        regions_count: usize,
    ) -> CVisionOcrResult;

    fn vision_ocr_result_destroy(result: CVisionOcrResult);
}

struct OwnedVisionOcrResult {
    raw: CVisionOcrResult,
}

impl OwnedVisionOcrResult {
    fn new(raw: CVisionOcrResult) -> Self {
        Self { raw }
    }

    fn error_message(&self) -> Option<String> {
        if self.raw.error.is_null() {
            None
        } else {
            Some(
                unsafe { CStr::from_ptr(self.raw.error) }
                    .to_string_lossy()
                    .into_owned(),
            )
        }
    }

    fn texts(&self) -> &[CVisionOcrText] {
        if self.raw.count == 0 || self.raw.texts.is_null() {
            &[]
        } else {
            unsafe { slice::from_raw_parts(self.raw.texts, self.raw.count) }
        }
    }
}

impl Drop for OwnedVisionOcrResult {
    fn drop(&mut self) {
        unsafe {
            vision_ocr_result_destroy(CVisionOcrResult {
                texts: self.raw.texts,
                count: self.raw.count,
                error: self.raw.error,
            });
        }
        self.raw.texts = ptr::null_mut();
        self.raw.error = ptr::null_mut();
        self.raw.count = 0;
    }
}

#[derive(Debug, Default)]
pub struct VisionOcrEngine;

impl VisionOcrEngine {
    pub fn new() -> Result<Self, OcrError> {
        Ok(Self)
    }
}

impl OcrEngine for VisionOcrEngine {
    fn name(&self) -> &'static str {
        "macos_vision"
    }

    fn recognize(&self, request: &OcrRequest<'_>) -> Result<OcrResponse, OcrError> {
        let plane = request.plane();
        let width = plane.width() as usize;
        let height = plane.height() as usize;
        let stride = plane.stride();

        let data = plane.data();
        if data.is_empty() {
            return Ok(OcrResponse::empty());
        }

        let regions = request.regions();
        let mut ffi_regions = Vec::with_capacity(regions.len());
        for region in regions {
            ffi_regions.push(CVisionOcrRect {
                x: region.x,
                y: region.y,
                width: region.width,
                height: region.height,
            });
        }

        let (regions_ptr, regions_count) = if ffi_regions.is_empty() {
            (std::ptr::null(), 0)
        } else {
            (ffi_regions.as_ptr(), ffi_regions.len())
        };

        let raw = unsafe {
            vision_recognize_text(
                data.as_ptr(),
                width,
                height,
                stride,
                regions_ptr,
                regions_count,
            )
        };

        let owned = OwnedVisionOcrResult::new(raw);
        if let Some(message) = owned.error_message() {
            return Err(OcrError::backend(message));
        }

        let mut texts = Vec::with_capacity(owned.texts().len());
        for entry in owned.texts() {
            if entry.text.is_null() {
                continue;
            }
            let text = unsafe { CStr::from_ptr(entry.text) }
                .to_string_lossy()
                .into_owned();
            if text.trim().is_empty() {
                continue;
            }

            let region = entry.rect;
            let mut ocr_text = OcrText::new(
                OcrRegion::new(region.x, region.y, region.width, region.height),
                text,
            );

            if entry.confidence.is_finite() && entry.confidence >= 0.0 {
                ocr_text = ocr_text.with_confidence(entry.confidence);
            }

            texts.push(ocr_text);
        }

        Ok(OcrResponse::new(texts))
    }
}
