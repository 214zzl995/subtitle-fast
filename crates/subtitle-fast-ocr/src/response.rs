use crate::region::OcrRegion;

/// OCR result for a single region.
#[derive(Debug, Clone)]
pub struct OcrText {
    pub region: OcrRegion,
    pub text: String,
    pub confidence: Option<f32>,
}

impl OcrText {
    pub fn new(region: OcrRegion, text: String) -> Self {
        Self {
            region,
            text,
            confidence: None,
        }
    }

    pub fn with_confidence(mut self, value: f32) -> Self {
        self.confidence = Some(value);
        self
    }
}

/// Collection of OCR results.
#[derive(Debug, Clone)]
pub struct OcrResponse {
    pub texts: Vec<OcrText>,
}

impl OcrResponse {
    pub fn new(texts: Vec<OcrText>) -> Self {
        Self { texts }
    }

    pub fn empty() -> Self {
        Self { texts: Vec::new() }
    }
}
