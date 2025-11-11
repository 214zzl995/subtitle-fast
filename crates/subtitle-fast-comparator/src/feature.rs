use std::any::Any;
use std::sync::Arc;

/// Type-erased container for per-comparator feature data.
#[derive(Clone)]
pub struct FeatureBlob {
    tag: &'static str,
    payload: Arc<dyn Any + Send + Sync>,
}

impl FeatureBlob {
    pub fn new<T>(tag: &'static str, data: T) -> Self
    where
        T: Any + Send + Sync + 'static,
    {
        Self {
            tag,
            payload: Arc::new(data),
        }
    }

    pub fn tag(&self) -> &'static str {
        self.tag
    }

    pub(crate) fn downcast<T>(&self, expected: &'static str) -> Option<Arc<T>>
    where
        T: Any + Send + Sync + 'static,
    {
        if self.tag != expected {
            return None;
        }
        self.payload.clone().downcast::<T>().ok()
    }
}

/// Individual metric emitted by a comparator.
#[derive(Debug, Clone)]
pub struct ReportMetric {
    pub name: &'static str,
    pub value: f32,
}

impl ReportMetric {
    pub fn new(name: &'static str, value: f32) -> Self {
        Self { name, value }
    }
}

/// Comparison output returned to the segmentation stage.
#[derive(Debug, Clone)]
pub struct ComparisonReport {
    pub similarity: f32,
    pub same_segment: bool,
    pub details: Vec<ReportMetric>,
}

impl ComparisonReport {
    pub fn new(similarity: f32, same_segment: bool) -> Self {
        Self {
            similarity,
            same_segment,
            details: Vec::new(),
        }
    }

    pub fn with_details(similarity: f32, same_segment: bool, details: Vec<ReportMetric>) -> Self {
        Self {
            similarity,
            same_segment,
            details,
        }
    }
}
