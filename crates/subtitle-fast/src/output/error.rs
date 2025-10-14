use std::fmt;

#[derive(Debug)]
pub enum OutputError {
    Io(std::io::Error),
    Encode(image::ImageError),
    Json(serde_json::Error),
}

impl fmt::Display for OutputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OutputError::Io(err) => write!(f, "I/O error: {err}"),
            OutputError::Encode(err) => write!(f, "encoding error: {err}"),
            OutputError::Json(err) => write!(f, "JSON error: {err}"),
        }
    }
}

impl std::error::Error for OutputError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            OutputError::Io(err) => Some(err),
            OutputError::Encode(err) => Some(err),
            OutputError::Json(err) => Some(err),
        }
    }
}

impl From<std::io::Error> for OutputError {
    fn from(value: std::io::Error) -> Self {
        OutputError::Io(value)
    }
}

impl From<image::ImageError> for OutputError {
    fn from(value: image::ImageError) -> Self {
        OutputError::Encode(value)
    }
}

impl From<serde_json::Error> for OutputError {
    fn from(value: serde_json::Error) -> Self {
        OutputError::Json(value)
    }
}
