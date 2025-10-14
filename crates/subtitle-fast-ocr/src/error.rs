use thiserror::Error;

#[derive(Debug, Error)]
pub enum OcrError {
    #[error("plane data length {provided} is smaller than stride * height ({required})")]
    InsufficientPlaneData { provided: usize, required: usize },
    #[error(
        "plane dimensions overflowed while validating stride * height (stride={stride}, height={height})"
    )]
    PlaneOverflow { stride: usize, height: u32 },
    #[error("backend error: {message}")]
    Backend { message: String },
}

impl OcrError {
    pub fn backend(message: impl Into<String>) -> Self {
        Self::Backend {
            message: message.into(),
        }
    }
}
