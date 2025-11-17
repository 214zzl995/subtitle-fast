pub mod chamfer_edge;
pub mod hybrid_mask;
pub mod spectral_hash;
pub mod structural_dssim;

pub use chamfer_edge::ChamferEdgeComparator;
pub use hybrid_mask::HybridMaskComparator;
pub use spectral_hash::SpectralHashComparator;
pub use structural_dssim::StructuralDssimComparator;

use crate::pipeline::{ComparisonReport, FeatureBlob};
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
