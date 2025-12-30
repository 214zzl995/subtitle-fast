use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use tokio::sync::mpsc::Sender;

use crate::core::{
    DecoderController, DecoderProvider, DecoderResult, FrameStream, SeekInfo, SeekReceiver,
    VideoFrame, spawn_stream_from_channel,
};

pub struct MockProvider {
    _input: Option<PathBuf>,
    width: u32,
    height: u32,
    stride: usize,
    frame_count: usize,
    frame_interval: Duration,
    channel_capacity: usize,
    start_frame: u64,
}

impl MockProvider {
    const DEFAULT_CHANNEL_CAPACITY: usize = 8;

    fn emit_frames(&self, tx: Sender<DecoderResult<VideoFrame>>, mut seek_rx: SeekReceiver) {
        let start_index = self.start_frame.min(self.frame_count as u64) as usize;
        for index in start_index..self.frame_count {
            drain_seek_requests(&mut seek_rx);
            if tx.is_closed() {
                break;
            }
            let mut buffer = vec![0u8; self.stride * self.height as usize];
            for (row, chunk) in buffer.chunks_mut(self.stride).enumerate() {
                let value = ((row + index) % 256) as u8;
                chunk.fill(value);
            }
            let uv_rows = (self.height as usize + 1) / 2;
            let uv_stride = self.stride;
            let uv_plane = vec![128u8; uv_stride * uv_rows];
            let timestamp = Some(Duration::from_millis((index * 16) as u64));
            let frame = VideoFrame::from_nv12_owned(
                self.width,
                self.height,
                self.stride,
                uv_stride,
                timestamp,
                buffer,
                uv_plane,
            )
            .map(|frame| frame.with_frame_index(Some(index as u64)));
            if tx.blocking_send(frame).is_err() {
                break;
            }
            if !self.frame_interval.is_zero() {
                thread::sleep(self.frame_interval);
            }
        }
    }
}

impl DecoderProvider for MockProvider {
    fn new(config: &crate::config::Configuration) -> DecoderResult<Self> {
        let capacity = config.channel_capacity.map(|n| n.get()).unwrap_or(Self::DEFAULT_CHANNEL_CAPACITY);
        Ok(Self {
            _input: config.input.clone(),
            width: 640,
            height: 360,
            stride: 640,
            frame_count: 120,
            frame_interval: Duration::from_millis(4),
            channel_capacity: capacity.max(1),
            start_frame: config.start_frame.unwrap_or(0),
        })
    }

    fn metadata(&self) -> crate::core::VideoMetadata {
        use crate::core::VideoMetadata;

        VideoMetadata {
            duration: Some(Duration::from_secs_f64((self.frame_count as f64) * 0.016)),
            fps: Some(60.0),
            width: Some(self.width),
            height: Some(self.height),
            total_frames: Some(self.frame_count as u64),
        }
    }

    fn open(self: Box<Self>) -> (DecoderController, FrameStream) {
        let provider = *self;
        let capacity = provider.channel_capacity;
        let (controller, seek_rx) = DecoderController::new();
        let stream = spawn_stream_from_channel(capacity, move |tx| {
            provider.emit_frames(tx, seek_rx);
        });
        (controller, stream)
    }
}

fn drain_seek_requests(seek_rx: &mut SeekReceiver) {
    if !seek_rx.has_changed().unwrap_or(false) {
        return;
    }
    if let Some(info) = *seek_rx.borrow_and_update() {
        handle_seek_request(info);
    }
}

fn handle_seek_request(_info: SeekInfo) {
    todo!("mock seek handling is not implemented yet");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DynDecoderProvider;
    use tokio_stream::StreamExt;

    #[tokio::test(flavor = "multi_thread")]
    async fn mock_backend_emits_frames() {
        let config = crate::config::Configuration {
            backend: crate::config::Backend::Mock,
            input: None,
            channel_capacity: None,
            output_format: crate::config::OutputFormat::Nv12,
            start_frame: None,
        };
        let decoder = Box::new(MockProvider::new(&config).unwrap()) as DynDecoderProvider;
        let metadata = decoder.metadata();
        let (_controller, mut stream) = decoder.open();
        assert_eq!(metadata.total_frames, Some(120));
        let frame = stream.next().await.unwrap().unwrap();
        assert_eq!(frame.width(), 640);
        assert_eq!(frame.height(), 360);
        assert_eq!(frame.data().len(), 640 * 360);
        assert_eq!(frame.uv_plane().len(), 640 * 180);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mock_backend_honors_start_frame() {
        let config = crate::config::Configuration {
            backend: crate::config::Backend::Mock,
            input: None,
            channel_capacity: None,
            output_format: crate::config::OutputFormat::Nv12,
            start_frame: Some(10),
        };
        let decoder = Box::new(MockProvider::new(&config).unwrap()) as DynDecoderProvider;
        let (_controller, mut stream) = decoder.open();
        let frame = stream.next().await.unwrap().unwrap();
        assert_eq!(frame.frame_index(), Some(10));
    }
}
