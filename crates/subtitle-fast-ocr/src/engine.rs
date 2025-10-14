use crate::error::OcrError;
use crate::request::OcrRequest;
use crate::response::OcrResponse;

/// Common interface for all OCR engines.
pub trait OcrEngine: Send + Sync {
    fn name(&self) -> &'static str;

    fn warm_up(&self) -> Result<(), OcrError> {
        Ok(())
    }

    fn recognize(&self, request: &OcrRequest<'_>) -> Result<OcrResponse, OcrError>;
}

/// Placeholder OCR engine used while a real backend is not wired.
#[derive(Debug, Default)]
pub struct NoopOcrEngine;

impl OcrEngine for NoopOcrEngine {
    fn name(&self) -> &'static str {
        "noop"
    }

    fn recognize(&self, _: &OcrRequest<'_>) -> Result<OcrResponse, OcrError> {
        Ok(OcrResponse::empty())
    }
}
