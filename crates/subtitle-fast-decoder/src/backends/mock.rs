use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use tokio::sync::mpsc::Sender;

use crate::core::{
    DynYPlaneProvider, YPlaneFrame, YPlaneResult, YPlaneStream, YPlaneStreamProvider,
    spawn_stream_from_channel,
};

pub struct MockProvider {
    _input: Option<PathBuf>,
    width: u32,
    height: u32,
    stride: usize,
    frame_count: usize,
    frame_interval: Duration,
    channel_capacity: usize,
}

impl MockProvider {
    const DEFAULT_CHANNEL_CAPACITY: usize = 8;

    pub fn open(input: Option<PathBuf>, channel_capacity: Option<usize>) -> Self {
        let capacity = channel_capacity.unwrap_or(Self::DEFAULT_CHANNEL_CAPACITY);
        Self {
            _input: input,
            width: 640,
            height: 360,
            stride: 640,
            frame_count: 120,
            frame_interval: Duration::from_millis(4),
            channel_capacity: capacity.max(1),
        }
    }

    fn emit_frames(&self, tx: Sender<YPlaneResult<YPlaneFrame>>) {
        for index in 0..self.frame_count {
            if tx.is_closed() {
                break;
            }
            let mut buffer = vec![0u8; self.stride * self.height as usize];
            for (row, chunk) in buffer.chunks_mut(self.stride).enumerate() {
                let value = ((row + index) % 256) as u8;
                chunk.fill(value);
            }
            let timestamp = Some(Duration::from_millis((index * 16) as u64));
            let frame =
                YPlaneFrame::from_owned(self.width, self.height, self.stride, timestamp, buffer);
            if tx.blocking_send(frame).is_err() {
                break;
            }
            if !self.frame_interval.is_zero() {
                thread::sleep(self.frame_interval);
            }
        }
    }
}

impl YPlaneStreamProvider for MockProvider {
    fn total_frames(&self) -> Option<u64> {
        Some(self.frame_count as u64)
    }

    fn into_stream(self: Box<Self>) -> YPlaneStream {
        let provider = *self;
        let capacity = provider.channel_capacity;
        spawn_stream_from_channel(capacity, move |tx| {
            provider.emit_frames(tx);
        })
    }
}

pub fn boxed_mock(
    path: Option<PathBuf>,
    channel_capacity: Option<usize>,
) -> YPlaneResult<DynYPlaneProvider> {
    Ok(Box::new(MockProvider::open(path, channel_capacity)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_stream::StreamExt;

    #[tokio::test(flavor = "multi_thread")]
    async fn mock_backend_emits_frames() {
        let provider = boxed_mock(None, None).unwrap();
        let total = provider.total_frames();
        let mut stream = provider.into_stream();
        assert_eq!(total, Some(120));
        let frame = stream.next().await.unwrap().unwrap();
        assert_eq!(frame.width(), 640);
        assert_eq!(frame.height(), 360);
        assert_eq!(frame.data().len(), 640 * 360);
    }
}
