mod chamfer_edge;
mod factory;
mod feature;
mod hybrid_mask;
mod ops;
mod preprocess;
mod spectral_hash;
mod structural_dssim;

pub use crate::factory::{ComparatorFactory, ComparatorKind, ComparatorSettings};
pub use crate::feature::{ComparisonReport, FeatureBlob, ReportMetric};

use subtitle_fast_decoder::YPlaneFrame;
use subtitle_fast_validator::subtitle_detection::RoiConfig;

/// Trait implemented by all subtitle comparators.
pub trait SubtitleComparator: Send + Sync {
    /// Stable comparator name used for logging and diagnostics.
    fn name(&self) -> &'static str;

    /// Extracts comparator-specific features from the provided ROI.
    fn extract(&self, frame: &YPlaneFrame, roi: &RoiConfig) -> Option<FeatureBlob>;

    /// Compares two feature blobs and produces a similarity report.
    fn compare(&self, reference: &FeatureBlob, candidate: &FeatureBlob) -> ComparisonReport;
}

pub(crate) use chamfer_edge::ChamferEdgeComparator;
pub(crate) use hybrid_mask::HybridMaskComparator;
pub(crate) use spectral_hash::SpectralHashComparator;
pub(crate) use structural_dssim::StructuralDssimComparator;

#[cfg(test)]
mod tests;
