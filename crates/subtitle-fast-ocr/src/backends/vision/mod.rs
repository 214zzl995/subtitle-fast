use std::ffi::{CStr, CString};
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
        languages: *const *const std::os::raw::c_char,
        languages_count: usize,
        auto_detect_language: bool,
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

#[derive(Debug, Clone)]
pub struct VisionOcrConfig {
    pub languages: Vec<String>,
    pub auto_detect_language: bool,
}

impl Default for VisionOcrConfig {
    fn default() -> Self {
        Self {
            languages: Vec::new(),
            auto_detect_language: true,
        }
    }
}

#[derive(Debug)]
pub struct VisionOcrEngine {
    languages: Vec<CString>,
    auto_detect_language: bool,
}

impl VisionOcrEngine {
    pub fn new() -> Result<Self, OcrError> {
        Self::with_config(VisionOcrConfig::default())
    }

    pub fn with_config(config: VisionOcrConfig) -> Result<Self, OcrError> {
        if !config.auto_detect_language && config.languages.is_empty() {
            return Err(OcrError::backend(
                "vision OCR auto language detection disabled but no languages provided",
            ));
        }

        let mut languages = Vec::with_capacity(config.languages.len());
        for value in config.languages {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                continue;
            }
            let cstr = CString::new(trimmed).map_err(|_| {
                OcrError::backend(
                    "vision OCR language contains interior null byte and cannot be used",
                )
            })?;
            if !languages.iter().any(|existing| existing == &cstr) {
                languages.push(cstr);
            }
        }
        Ok(Self {
            languages,
            auto_detect_language: config.auto_detect_language,
        })
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
            (ptr::null(), 0)
        } else {
            (ffi_regions.as_ptr(), ffi_regions.len())
        };

        let language_ptrs: Vec<*const std::os::raw::c_char> =
            self.languages.iter().map(|lang| lang.as_ptr()).collect();
        let (languages_ptr, languages_count) = if language_ptrs.is_empty() {
            (ptr::null(), 0)
        } else {
            (language_ptrs.as_ptr(), language_ptrs.len())
        };

        let raw = unsafe {
            vision_recognize_text(
                data.as_ptr(),
                width,
                height,
                stride,
                regions_ptr,
                regions_count,
                languages_ptr,
                languages_count,
                self.auto_detect_language,
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
