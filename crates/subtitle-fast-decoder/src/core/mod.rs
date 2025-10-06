use std::fmt;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use futures_core::Stream;
use futures_util::stream::unfold;
use thiserror::Error;
use tokio::sync::mpsc::{self, Sender};

pub type YPlaneStream = Pin<Box<dyn Stream<Item = YPlaneResult<YPlaneFrame>> + Send>>;

pub type DynYPlaneProvider = Box<dyn YPlaneStreamProvider>;

pub type YPlaneResult<T> = Result<T, YPlaneError>;

pub trait YPlaneStreamProvider: Send + 'static {
    fn total_frames(&self) -> Option<u64> {
        None
    }

    fn into_stream(self: Box<Self>) -> YPlaneStream;
}

#[derive(Clone)]
pub struct YPlaneFrame {
    width: u32,
    height: u32,
    stride: usize,
    frame_index: Option<u64>,
    timestamp: Option<Duration>,
    data: Arc<[u8]>,

}

impl fmt::Debug for YPlaneFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("YPlaneFrame")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("stride", &self.stride)
            .field("timestamp", &self.timestamp)
            .field("bytes", &self.data.len())
            .field("frame_index", &self.frame_index)
            .finish()
    }
}

impl YPlaneFrame {
    pub fn from_owned(
        width: u32,
        height: u32,
        stride: usize,
        timestamp: Option<Duration>,
        data: Vec<u8>,
    ) -> YPlaneResult<Self> {
        let required =
            stride
                .checked_mul(height as usize)
                .ok_or_else(|| YPlaneError::InvalidFrame {
                    reason: "calculated Y plane length overflowed".into(),
                })?;
        if data.len() < required {
            return Err(YPlaneError::InvalidFrame {
                reason: format!(
                    "insufficient Y plane bytes: got {} expected at least {}",
                    data.len(),
                    required
                ),
            });
        }
        Ok(Self {
            width,
            height,
            stride,
            timestamp,
            data: Arc::from(data.into_boxed_slice()),
            frame_index: None,
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn stride(&self) -> usize {
        self.stride
    }

    pub fn timestamp(&self) -> Option<Duration> {
        self.timestamp
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }

    pub fn frame_index(&self) -> Option<u64> {
        self.frame_index
    }

    pub fn with_frame_index(mut self, index: Option<u64>) -> Self {
        self.frame_index = index;
        self
    }

    pub fn set_frame_index(&mut self, index: Option<u64>) {
        self.frame_index = index;
    }
}

#[derive(Debug, Error)]
pub enum YPlaneError {
    #[error("backend {backend} is not supported in this build")]
    Unsupported { backend: &'static str },

    #[error("{backend} backend failed: {message}")]
    BackendFailure {
        backend: &'static str,
        message: String,
    },

    #[error("configuration error: {message}")]
    Configuration { message: String },

    #[error("invalid frame: {reason}")]
    InvalidFrame { reason: String },

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

impl YPlaneError {
    pub fn unsupported(backend: &'static str) -> Self {
        Self::Unsupported { backend }
    }

    pub fn backend_failure(backend: &'static str, message: impl Into<String>) -> Self {
        Self::BackendFailure {
            backend,
            message: message.into(),
        }
    }

    pub fn configuration(message: impl Into<String>) -> Self {
        Self::Configuration {
            message: message.into(),
        }
    }
}

pub fn spawn_stream_from_channel(
    capacity: usize,
    task: impl FnOnce(Sender<YPlaneResult<YPlaneFrame>>) + Send + 'static,
) -> YPlaneStream {
    let (tx, rx) = mpsc::channel(capacity);
    tokio::task::spawn_blocking(move || task(tx));
    let stream = unfold(rx, |mut receiver| async {
        match receiver.recv().await {
            Some(item) => Some((item, receiver)),
            None => None,
        }
    });
    Box::pin(stream)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_stream::StreamExt;

    #[tokio::test(flavor = "multi_thread")]
    async fn frame_metadata_accessors_work() {
        let frame =
            YPlaneFrame::from_owned(4, 2, 4, Some(Duration::from_millis(10)), vec![0; 8]).unwrap();
        assert_eq!(frame.width(), 4);
        assert_eq!(frame.height(), 2);
        assert_eq!(frame.stride(), 4);
        assert_eq!(frame.timestamp(), Some(Duration::from_millis(10)));
        assert_eq!(frame.data().len(), 8);
        assert_eq!(frame.frame_index(), None);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn spawn_stream_from_channel_pushes_values() {
        let stream = spawn_stream_from_channel(2, move |tx| {
            tx.blocking_send(Ok(
                YPlaneFrame::from_owned(2, 2, 2, None, vec![1, 2, 3, 4]).unwrap()
            ))
            .unwrap();
        });
        let mut stream = stream;
        let frame = stream.next().await.unwrap().unwrap();
        assert_eq!(frame.data(), &[1, 2, 3, 4]);
    }
}
