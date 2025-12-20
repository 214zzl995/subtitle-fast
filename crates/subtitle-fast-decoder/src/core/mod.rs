use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use futures_core::Stream;
use futures_util::stream::unfold;
use tokio::sync::mpsc::{self, Sender};

pub use subtitle_fast_types::{PlaneFrame, RawFrame, RawFrameFormat, YPlaneError, YPlaneResult};

pub type YPlaneStream = Pin<Box<dyn Stream<Item = YPlaneResult<PlaneFrame>> + Send>>;

pub type DynYPlaneProvider = Box<dyn YPlaneStreamProvider>;

#[derive(Debug, Clone, Copy)]
pub struct SeekPosition {
    pub timestamp: Option<Duration>,
    pub frame_index: Option<u64>,
}

pub trait YPlaneStreamProvider: Send + 'static {
    fn total_frames(&self) -> Option<u64> {
        None
    }

    fn into_stream(self: Box<Self>) -> PlaneStreamHandle;
}

pub trait SeekControl: Send + Sync {
    fn seek_to_time(&self, timestamp: Duration) -> YPlaneResult<SeekPosition>;
    fn seek_to_frame(&self, frame_index: u64) -> YPlaneResult<SeekPosition>;
}

pub struct PlaneStreamHandle {
    stream: YPlaneStream,
    seeker: Box<dyn SeekControl>,
}

impl PlaneStreamHandle {
    pub fn new(stream: YPlaneStream, seeker: Box<dyn SeekControl>) -> Self {
        Self { stream, seeker }
    }

    pub fn seek_to_time(&self, timestamp: Duration) -> YPlaneResult<SeekPosition> {
        self.seeker.seek_to_time(timestamp)
    }

    pub fn seek_to_frame(&self, frame_index: u64) -> YPlaneResult<SeekPosition> {
        self.seeker.seek_to_frame(frame_index)
    }
}

impl Stream for PlaneStreamHandle {
    type Item = YPlaneResult<PlaneFrame>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.stream.as_mut().poll_next(cx)
    }
}

pub fn spawn_stream_from_channel(
    capacity: usize,
    task: impl FnOnce(Sender<YPlaneResult<PlaneFrame>>) + Send + 'static,
) -> YPlaneStream {
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
        let frame =
            PlaneFrame::from_owned(4, 2, 4, Some(Duration::from_millis(10)), vec![0; 8]).unwrap();
        assert_eq!(frame.width(), 4);
        assert_eq!(frame.height(), 2);
        assert_eq!(frame.timestamp(), Some(Duration::from_millis(10)));
        let plane = frame.y_plane().expect("Y plane");
        assert_eq!(plane.stride, 4);
        assert_eq!(plane.data.len(), 8);
        assert_eq!(frame.frame_index(), None);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn spawn_stream_from_channel_pushes_values() {
        let stream = spawn_stream_from_channel(2, move |tx| {
            tx.blocking_send(Ok(
                PlaneFrame::from_owned(2, 2, 2, None, vec![1, 2, 3, 4]).unwrap()
            ))
            .unwrap();
        });
        let mut stream = stream;
        let frame = stream.next().await.unwrap().unwrap();
        let plane = frame.y_plane().expect("Y plane");
        assert_eq!(plane.data, &[1, 2, 3, 4]);
    }
}
