use std::pin::Pin;
use std::time::Duration;

use futures_core::Stream;
use futures_util::stream::unfold;
use tokio::sync::mpsc::{self, Sender};

pub use subtitle_fast_types::{FrameBuffer, FrameError, FrameResult, Nv12Buffer, VideoFrame};

pub type FrameStream = Pin<Box<dyn Stream<Item = FrameResult<VideoFrame>> + Send>>;

pub type DynFrameProvider = Box<dyn FrameStreamProvider>;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VideoMetadata {
    pub duration: Option<Duration>,
    pub fps: Option<f64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub total_frames: Option<u64>,
}

impl Default for VideoMetadata {
    fn default() -> Self {
        Self {
            duration: None,
            fps: None,
            width: None,
            height: None,
            total_frames: None,
        }
    }
}

impl VideoMetadata {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_duration_and_fps(duration: Duration, fps: f64) -> Self {
        Self {
            duration: Some(duration),
            fps: Some(fps),
            ..Default::default()
        }
    }

    pub fn duration_ms(&self) -> Option<f64> {
        self.duration.map(|d| d.as_secs_f64() * 1000.0)
    }

    pub fn calculate_total_frames(&self) -> Option<u64> {
        if let Some(total) = self.total_frames {
            return Some(total);
        }

        if let (Some(duration), Some(fps)) = (self.duration, self.fps) {
            let seconds = duration.as_secs_f64();
            let total = (seconds * fps).round();
            if total.is_finite() && total >= 0.0 {
                return Some(total as u64);
            }
        }

        None
    }
}

pub trait FrameStreamProvider: Send + 'static {
    fn metadata(&self) -> VideoMetadata {
        VideoMetadata::default()
    }

    fn into_stream(self: Box<Self>) -> FrameStream;
}

pub fn spawn_stream_from_channel(
    capacity: usize,
    task: impl FnOnce(Sender<FrameResult<VideoFrame>>) + Send + 'static,
) -> FrameStream {
    let (tx, rx) = mpsc::channel(capacity);
    tokio::task::spawn_blocking(move || task(tx));
    let stream = unfold(rx, |mut receiver| async {
        receiver.recv().await.map(|item| (item, receiver))
    });
    Box::pin(stream)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio_stream::StreamExt;

    #[tokio::test(flavor = "multi_thread")]
    async fn frame_metadata_accessors_work() {
        let frame = VideoFrame::from_nv12_owned(
            4,
            2,
            4,
            4,
            Some(Duration::from_millis(10)),
            vec![0; 8],
            vec![128; 4],
        )
        .unwrap();
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
            tx.blocking_send(Ok(VideoFrame::from_nv12_owned(
                2,
                2,
                2,
                2,
                None,
                vec![1, 2, 3, 4],
                vec![128; 2],
            )
            .unwrap()))
                .unwrap();
        });
        let mut stream = stream;
        let frame = stream.next().await.unwrap().unwrap();
        assert_eq!(frame.data(), &[1, 2, 3, 4]);
    }
}
