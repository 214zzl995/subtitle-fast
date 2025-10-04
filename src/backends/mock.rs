use std::time::Duration;

use tokio::sync::mpsc;
use tokio::time::{Instant, sleep};
use tokio_stream::wrappers::ReceiverStream;

use crate::core::{
    DynYPlaneProvider, YPlaneFrame, YPlaneResult, YPlaneStream, YPlaneStreamProvider,
};

#[derive(Debug, Clone)]
pub struct MockProvider {
    frame_count: usize,
    width: u32,
    height: u32,
    stride: usize,
    interval: Duration,
}

impl Default for MockProvider {
    fn default() -> Self {
        Self {
            frame_count: 30,
            width: 640,
            height: 360,
            stride: 640,
            interval: Duration::from_millis(16),
        }
    }
}

impl MockProvider {
    pub fn new(
        frame_count: usize,
        width: u32,
        height: u32,
        stride: usize,
        interval: Duration,
    ) -> Self {
        Self {
            frame_count,
            width,
            height,
            stride,
            interval,
        }
    }

    fn generate_frame(&self, index: usize) -> YPlaneFrame {
        let mut data = vec![0u8; self.stride * self.height as usize];
        for row in 0..self.height as usize {
            let offset = row * self.stride;
            let value = ((index + row) % 256) as u8;
            data[offset..offset + self.width as usize].fill(value);
        }
        let timestamp = self.interval.checked_mul(index as u32);
        YPlaneFrame::from_owned(self.width, self.height, self.stride, timestamp, data)
            .expect("mock frame construction should not fail")
    }
}

impl YPlaneStreamProvider for MockProvider {
    fn into_stream(self: Box<Self>) -> YPlaneStream {
        let provider = *self;
        let (tx, rx) = mpsc::channel::<YPlaneResult<YPlaneFrame>>(provider.frame_count.min(8));
        tokio::spawn(async move {
            let mut next_instant = Instant::now();
            for index in 0..provider.frame_count {
                let frame = provider.generate_frame(index);
                if Instant::now() < next_instant {
                    sleep(next_instant - Instant::now()).await;
                }
                next_instant += provider.interval;
                if tx.send(Ok(frame)).await.is_err() {
                    break;
                }
            }
        });
        Box::pin(ReceiverStream::new(rx))
    }
}

pub fn boxed_mock() -> DynYPlaneProvider {
    Box::new(MockProvider::default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_stream::StreamExt;

    #[tokio::test(flavor = "multi_thread")]
    async fn mock_stream_yields_requested_frames() {
        let provider = MockProvider::new(3, 4, 4, 4, Duration::from_millis(1));
        let mut stream = Box::new(provider) as DynYPlaneProvider;
        let mut stream = stream.into_stream();
        let mut frames = Vec::new();
        while let Some(frame) = stream.next().await {
            frames.push(frame.unwrap());
        }
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[0].width(), 4);
        assert_eq!(frames[0].data()[0], 0);
        assert_eq!(frames[1].data()[0], 1);
        assert_eq!(frames[2].data()[0], 2);
    }
}
