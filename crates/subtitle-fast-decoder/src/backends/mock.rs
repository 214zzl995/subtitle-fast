use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

use tokio::sync::mpsc::Sender;

use crate::core::{
    DecoderController, DecoderProvider, DecoderResult, FrameStream, SeekInfo, SeekMode,
    SeekReceiver, VideoFrame, spawn_stream_from_channel,
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
    const FPS: f64 = 60.0;

    fn emit_frames(
        &self,
        tx: Sender<DecoderResult<VideoFrame>>,
        mut seek_rx: SeekReceiver,
        serial: Arc<AtomicU64>,
    ) {
        let mut index = self.start_frame.min(self.frame_count as u64) as usize;
        let mut current_serial = serial.load(Ordering::SeqCst);
        let mut pending_drop: Option<DropUntil> = None;
        while index < self.frame_count {
            if let Some(plan) = drain_seek_requests(&mut seek_rx, &serial, &mut current_serial) {
                index = plan
                    .start_frame
                    .min(self.frame_count as u64)
                    .try_into()
                    .unwrap_or(self.frame_count);
                pending_drop = plan.drop_until;
            }
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
            let pts = Some(Duration::from_millis((index * 16) as u64));
            if should_skip_frame(&mut pending_drop, index as u64, pts) {
                index += 1;
                continue;
            }
            let frame = VideoFrame::from_nv12_owned(
                self.width,
                self.height,
                self.stride,
                uv_stride,
                pts,
                None,
                buffer,
                uv_plane,
            )
            .map(|frame| {
                frame
                    .with_index(Some(index as u64))
                    .with_serial(current_serial)
            });
            if tx.blocking_send(frame).is_err() {
                break;
            }
            if !self.frame_interval.is_zero() {
                thread::sleep(self.frame_interval);
            }
            index += 1;
        }
    }
}

impl DecoderProvider for MockProvider {
    fn new(config: &crate::config::Configuration) -> DecoderResult<Self> {
        let capacity = config
            .channel_capacity
            .map(|n| n.get())
            .unwrap_or(Self::DEFAULT_CHANNEL_CAPACITY);
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

    fn open(self: Box<Self>) -> DecoderResult<(DecoderController, FrameStream)> {
        let provider = *self;
        let capacity = provider.channel_capacity;
        let controller = DecoderController::new();
        let seek_rx = controller.seek_receiver();
        let serial = controller.serial_handle();
        let stream = spawn_stream_from_channel(capacity, move |tx| {
            provider.emit_frames(tx, seek_rx, serial);
        });
        Ok((controller, stream))
    }
}

fn drain_seek_requests(
    seek_rx: &mut SeekReceiver,
    serial: &AtomicU64,
    current_serial: &mut u64,
) -> Option<SeekPlan> {
    if !seek_rx.has_changed().unwrap_or(false) {
        return None;
    }
    if let Some(info) = *seek_rx.borrow_and_update() {
        *current_serial = serial.load(Ordering::SeqCst);
        return compute_seek_plan(info);
    }
    None
}

fn should_skip_frame(
    pending_drop: &mut Option<DropUntil>,
    frame_index: u64,
    pts: Option<Duration>,
) -> bool {
    let Some(drop_until) = *pending_drop else {
        return false;
    };
    let keep = match drop_until {
        DropUntil::Frame(target) => frame_index >= target,
        DropUntil::Timestamp(target) => pts.map(|value| value >= target).unwrap_or(true),
    };
    if keep {
        *pending_drop = None;
        false
    } else {
        true
    }
}

#[derive(Clone, Copy)]
enum DropUntil {
    Frame(u64),
    Timestamp(Duration),
}

#[derive(Clone, Copy)]
struct SeekPlan {
    start_frame: u64,
    drop_until: Option<DropUntil>,
}

fn compute_seek_plan(info: SeekInfo) -> Option<SeekPlan> {
    match info {
        SeekInfo::Frame { frame, mode } => Some(SeekPlan {
            start_frame: frame,
            drop_until: match mode {
                SeekMode::Fast => None,
                SeekMode::Accurate => Some(DropUntil::Frame(frame)),
            },
        }),
        SeekInfo::Time { position, mode } => {
            let seconds = position.as_secs_f64();
            if !seconds.is_finite() || seconds.is_sign_negative() {
                return None;
            }
            let raw_frame = seconds * MockProvider::FPS;
            if !raw_frame.is_finite() || raw_frame.is_sign_negative() {
                return None;
            }
            let frame = match mode {
                SeekMode::Fast => raw_frame.round(),
                SeekMode::Accurate => raw_frame.floor(),
            };
            if frame < 0.0 || frame > u64::MAX as f64 {
                return None;
            }
            Some(SeekPlan {
                start_frame: frame as u64,
                drop_until: match mode {
                    SeekMode::Fast => None,
                    SeekMode::Accurate => Some(DropUntil::Timestamp(position)),
                },
            })
        }
    }
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
        let (_controller, mut stream) = decoder.open().unwrap();
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
        let (_controller, mut stream) = decoder.open().unwrap();
        let frame = stream.next().await.unwrap().unwrap();
        assert_eq!(frame.index(), Some(10));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mock_backend_seek_by_frame_updates_serial() {
        let config = crate::config::Configuration {
            backend: crate::config::Backend::Mock,
            input: None,
            channel_capacity: None,
            output_format: crate::config::OutputFormat::Nv12,
            start_frame: None,
        };
        let decoder = Box::new(MockProvider::new(&config).unwrap()) as DynDecoderProvider;
        let (controller, mut stream) = decoder.open().unwrap();
        let _ = stream.next().await.unwrap().unwrap();
        let serial = controller
            .seek(SeekInfo::Frame {
                frame: 10,
                mode: SeekMode::Accurate,
            })
            .unwrap();
        let mut sought = None;
        for _ in 0..200 {
            let frame = stream.next().await.unwrap().unwrap();
            if frame.serial() == serial {
                sought = Some(frame);
                break;
            }
        }
        let frame = sought.expect("expected frame after seek");
        assert_eq!(frame.index(), Some(10));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mock_backend_seek_by_time_updates_serial() {
        let config = crate::config::Configuration {
            backend: crate::config::Backend::Mock,
            input: None,
            channel_capacity: None,
            output_format: crate::config::OutputFormat::Nv12,
            start_frame: None,
        };
        let decoder = Box::new(MockProvider::new(&config).unwrap()) as DynDecoderProvider;
        let (controller, mut stream) = decoder.open().unwrap();
        let _ = stream.next().await.unwrap().unwrap();
        let serial = controller
            .seek(SeekInfo::Time {
                position: Duration::from_secs(1),
                mode: SeekMode::Accurate,
            })
            .unwrap();
        let mut sought = None;
        for _ in 0..200 {
            let frame = stream.next().await.unwrap().unwrap();
            if frame.serial() == serial {
                sought = Some(frame);
                break;
            }
        }
        let frame = sought.expect("expected frame after seek");
        assert!(frame.index().unwrap_or(0) >= 60);
        assert!(frame.pts().unwrap_or_default() >= Duration::from_secs(1));
    }
}
