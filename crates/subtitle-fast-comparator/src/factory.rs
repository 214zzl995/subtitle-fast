use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

use crate::comparators::{BitsetCoverComparator, SparseChamferComparator, SubtitleComparator};
use crate::pipeline::PreprocessSettings;

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum ComparatorKind {
    BitsetCover,
    SparseChamfer,
}

impl ComparatorKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ComparatorKind::BitsetCover => "bitset-cover",
            ComparatorKind::SparseChamfer => "sparse-chamfer",
        }
    }
}

#[derive(Debug)]
pub struct ComparatorKindParseError(pub String);

impl fmt::Display for ComparatorKindParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown comparator '{}'", self.0)
    }
}

impl std::error::Error for ComparatorKindParseError {}

impl FromStr for ComparatorKind {
    type Err = ComparatorKindParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let lower = s.trim().to_ascii_lowercase();
        match lower.as_str() {
            "bitset-cover" => Ok(ComparatorKind::BitsetCover),
            "sparse-chamfer" => Ok(ComparatorKind::SparseChamfer),
            _ => Err(ComparatorKindParseError(lower)),
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub struct ComparatorSettings {
    pub kind: ComparatorKind,
    pub target: u8,
    pub delta: u8,
}

impl ComparatorSettings {
    fn preprocess(&self) -> PreprocessSettings {
        PreprocessSettings {
            target: self.target,
            delta: self.delta,
        }
    }
}

pub struct ComparatorFactory {
    settings: ComparatorSettings,
}

impl ComparatorFactory {
    pub fn new(settings: ComparatorSettings) -> Self {
        Self { settings }
    }

    pub fn build(&self) -> Arc<dyn SubtitleComparator> {
        let preprocess = self.settings.preprocess();
        match self.settings.kind {
            ComparatorKind::BitsetCover => Arc::new(BitsetCoverComparator::new(preprocess)),
            ComparatorKind::SparseChamfer => Arc::new(SparseChamferComparator::new(preprocess)),
        }
    }
}
