//! Comparator crate entry point with flat, easy-to-import modules.

pub mod comparators;
pub mod factory;
pub mod pipeline;

pub use comparators::{BitsetCoverComparator, SparseChamferComparator, SubtitleComparator};
pub use factory::{ComparatorFactory, ComparatorKind, ComparatorSettings};
pub use pipeline::{ComparisonReport, FeatureBlob, PreprocessSettings, ReportMetric};

#[cfg(test)]
mod tests;
