use std::pin::Pin;

use futures_core::Stream;
use futures_util::stream::unfold;
use tokio::sync::mpsc::{self, Sender};

pub use subtitle_fast_types::{YPlaneError, YPlaneFrame, YPlaneResult};

pub type YPlaneStream = Pin<Box<dyn Stream<Item = YPlaneResult<YPlaneFrame>> + Send>>;

pub type DynYPlaneProvider = Box<dyn YPlaneStreamProvider>;

pub trait YPlaneStreamProvider: Send + 'static {
    fn total_frames(&self) -> Option<u64> {
        None
    }

    fn into_stream(self: Box<Self>) -> YPlaneStream;
}

pub fn spawn_stream_from_channel(
    capacity: usize,
    task: impl FnOnce(Sender<YPlaneResult<YPlaneFrame>>) + Send + 'static,
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
