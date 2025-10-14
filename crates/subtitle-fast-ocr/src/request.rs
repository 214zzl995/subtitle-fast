use crate::plane::LumaPlane;
use crate::region::OcrRegion;

/// OCR invocation metadata.
#[derive(Debug)]
pub struct OcrRequest<'a> {
    plane: LumaPlane<'a>,
    regions: &'a [OcrRegion],
}

impl<'a> OcrRequest<'a> {
    pub fn new(plane: LumaPlane<'a>, regions: &'a [OcrRegion]) -> Self {
        Self { plane, regions }
    }

    pub fn plane(&self) -> &LumaPlane<'a> {
        &self.plane
    }

    pub fn regions(&self) -> &'a [OcrRegion] {
        self.regions
    }
}
