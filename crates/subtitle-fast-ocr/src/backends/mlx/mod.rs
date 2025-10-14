use std::ffi::{CStr, CString};
use std::ptr;

use crate::{OcrEngine, OcrError, OcrRegion, OcrRequest, OcrResponse, OcrText};

#[repr(C)]
#[derive(Clone, Copy)]
struct CMlxRegion {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

#[repr(C)]
struct CMlxText {
    rect: CMlxRegion,
    confidence: f32,
    text: *mut std::os::raw::c_char,
}

#[repr(C)]
struct CMlxResult {
    texts: *mut CMlxText,
    count: usize,
    error: *mut std::os::raw::c_char,
}

#[repr(C)]
struct CMlxContext {
    _private: [u8; 0],
}

unsafe extern "C" {
    fn mlx_vlm_create(model_path: *const std::os::raw::c_char) -> *mut CMlxContext;
    fn mlx_vlm_destroy(ctx: *mut CMlxContext);
    fn mlx_vlm_last_error() -> *const std::os::raw::c_char;
    fn mlx_vlm_recognize(
        ctx: *mut CMlxContext,
        data: *const u8,
        width: usize,
        height: usize,
        stride: usize,
        regions: *const CMlxRegion,
        regions_count: usize,
    ) -> CMlxResult;
    fn mlx_vlm_result_destroy(result: CMlxResult);
}

struct OwnedMlxResult {
    raw: CMlxResult,
}

impl OwnedMlxResult {
    fn new(raw: CMlxResult) -> Self {
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

    fn texts(&self) -> &[CMlxText] {
        if self.raw.count == 0 || self.raw.texts.is_null() {
            &[]
        } else {
            unsafe { std::slice::from_raw_parts(self.raw.texts, self.raw.count) }
        }
    }
}

impl Drop for OwnedMlxResult {
    fn drop(&mut self) {
        unsafe {
            mlx_vlm_result_destroy(CMlxResult {
                texts: self.raw.texts,
                count: self.raw.count,
                error: self.raw.error,
            });
        }
        self.raw.texts = ptr::null_mut();
        self.raw.count = 0;
        self.raw.error = ptr::null_mut();
    }
}

pub struct MlxVlmOcrEngine {
    ctx: *mut CMlxContext,
}

unsafe impl Send for MlxVlmOcrEngine {}
unsafe impl Sync for MlxVlmOcrEngine {}

impl Drop for MlxVlmOcrEngine {
    fn drop(&mut self) {
        if !self.ctx.is_null() {
            unsafe {
                mlx_vlm_destroy(self.ctx);
            }
            self.ctx = ptr::null_mut();
        }
    }
}

impl MlxVlmOcrEngine {
    pub fn new(model_path: impl AsRef<std::path::Path>) -> Result<Self, OcrError> {
        let path = model_path.as_ref();
        let c_path = CString::new(
            path.to_str()
                .ok_or_else(|| OcrError::backend("mlx_vlm model path contains invalid UTF-8"))?,
        )
        .map_err(|err| OcrError::backend(format!("invalid mlx_vlm model path: {err}")))?;

        let ctx = unsafe { mlx_vlm_create(c_path.as_ptr()) };
        if ctx.is_null() {
            return Err(fetch_bridge_error()
                .unwrap_or_else(|| OcrError::backend("failed to initialize mlx_vlm context")));
        }

        Ok(Self { ctx })
    }
}

impl OcrEngine for MlxVlmOcrEngine {
    fn name(&self) -> &'static str {
        "mlx_vlm"
    }

    fn recognize(&self, request: &OcrRequest<'_>) -> Result<OcrResponse, OcrError> {
        let plane = request.plane();
        if request.regions().is_empty() {
            return Ok(OcrResponse::empty());
        }

        let mut ffi_regions = Vec::with_capacity(request.regions().len());
        for region in request.regions() {
            ffi_regions.push(CMlxRegion {
                x: region.x,
                y: region.y,
                width: region.width,
                height: region.height,
            });
        }

        let raw = unsafe {
            mlx_vlm_recognize(
                self.ctx,
                plane.data().as_ptr(),
                plane.width() as usize,
                plane.height() as usize,
                plane.stride(),
                ffi_regions.as_ptr(),
                ffi_regions.len(),
            )
        };

        let owned = OwnedMlxResult::new(raw);
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

            let rect = entry.rect;
            let mut ocr_text = OcrText::new(
                OcrRegion::new(rect.x, rect.y, rect.width, rect.height),
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

fn fetch_bridge_error() -> Option<OcrError> {
    let ptr = unsafe { mlx_vlm_last_error() };
    if ptr.is_null() {
        None
    } else {
        Some(OcrError::backend(
            unsafe { CStr::from_ptr(ptr) }.to_string_lossy(),
        ))
    }
}
