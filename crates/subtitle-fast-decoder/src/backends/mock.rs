use std::path::PathBuf;
use std::sync::mpsc as std_mpsc;
use std::thread;
use std::time::Duration;

use tokio::sync::mpsc::Sender;

use crate::core::{
    DynYPlaneProvider, PlaneFrame, PlaneStreamHandle, RawFrameFormat, SeekControl, SeekPosition,
    YPlaneError, YPlaneResult, YPlaneStreamProvider, spawn_stream_from_channel,
};
use subtitle_fast_types::RawFrame;

pub struct MockProvider {
    _input: Option<PathBuf>,
    width: u32,
    height: u32,
    stride: usize,
    frame_count: usize,
    frame_interval: Duration,
    channel_capacity: usize,
    output_format: RawFrameFormat,
}

enum SeekTarget {
    Time(Duration),
    Frame(u64),
}

struct SeekRequest {
    target: SeekTarget,
    respond_to: std_mpsc::Sender<YPlaneResult<SeekPosition>>,
}

struct MockSeeker {
    tx: std_mpsc::Sender<SeekRequest>,
}

impl SeekControl for MockSeeker {
    fn seek_to_time(&self, timestamp: Duration) -> YPlaneResult<SeekPosition> {
        let (tx, rx) = std_mpsc::channel();
        self.tx
            .send(SeekRequest {
                target: SeekTarget::Time(timestamp),
                respond_to: tx,
            })
            .map_err(|_| YPlaneError::backend_failure("mock", "seek channel closed"))?;
        rx.recv()
            .unwrap_or_else(|_| Err(YPlaneError::backend_failure("mock", "seek response failed")))
    }

    fn seek_to_frame(&self, frame_index: u64) -> YPlaneResult<SeekPosition> {
        let (tx, rx) = std_mpsc::channel();
        self.tx
            .send(SeekRequest {
                target: SeekTarget::Frame(frame_index),
                respond_to: tx,
            })
            .map_err(|_| YPlaneError::backend_failure("mock", "seek channel closed"))?;
        rx.recv()
            .unwrap_or_else(|_| Err(YPlaneError::backend_failure("mock", "seek response failed")))
    }
}

impl MockProvider {
    const DEFAULT_CHANNEL_CAPACITY: usize = 8;
    const FRAME_TIME_MS: u64 = 16;

    pub fn open(
        input: Option<PathBuf>,
        channel_capacity: Option<usize>,
        output_format: RawFrameFormat,
    ) -> Self {
        let capacity = channel_capacity.unwrap_or(Self::DEFAULT_CHANNEL_CAPACITY);
        Self {
            _input: input,
            width: 640,
            height: 360,
            stride: 640,
            frame_count: 120,
            frame_interval: Duration::from_millis(4),
            channel_capacity: capacity.max(1),
            output_format,
        }
    }

    fn run_loop(
        &self,
        tx: Sender<YPlaneResult<PlaneFrame>>,
        seek_rx: std_mpsc::Receiver<SeekRequest>,
    ) {
        let mut next_index: usize = 0;
        let mut pending: Option<SeekRequest> = None;

        loop {
            while let Ok(request) = seek_rx.try_recv() {
                if let Some(prior) = pending.take() {
                    let _ = prior.respond_to.send(Err(YPlaneError::configuration(
                        "seek superseded by a newer request",
                    )));
                }
                pending = Some(request);
            }

            if let Some(request) = pending.take() {
                let (index, timestamp) = match request.target {
                    SeekTarget::Frame(index) => (index, Some(timestamp_for_index(index))),
                    SeekTarget::Time(timestamp) => {
                        let index = time_to_index(timestamp);
                        (index, Some(timestamp))
                    }
                };
                next_index = index as usize;
                let _ = request.respond_to.send(Ok(SeekPosition {
                    timestamp,
                    frame_index: Some(index),
                }));
            }

            if next_index >= self.frame_count {
                break;
            }

            let frame = match self.build_frame(next_index) {
                Ok(frame) => frame,
                Err(err) => {
                    let _ = tx.blocking_send(Err(err));
                    break;
                }
            };
            if tx.blocking_send(Ok(frame)).is_err() {
                break;
            }

            next_index = next_index.saturating_add(1);

            if !self.frame_interval.is_zero() {
                thread::sleep(self.frame_interval);
            }
        }
    }

    fn build_frame(&self, index: usize) -> YPlaneResult<PlaneFrame> {
        let width = self.width;
        let height = self.height;
        let stride = self.stride;
        let timestamp = Some(timestamp_for_index(index as u64));

        let raw = match self.output_format {
            RawFrameFormat::Y => {
                let mut buffer = vec![0u8; stride * height as usize];
                for (row, chunk) in buffer.chunks_mut(stride).enumerate() {
                    let value = ((row + index) % 256) as u8;
                    chunk.fill(value);
                }
                RawFrame::Y {
                    stride,
                    data: buffer.into(),
                }
            }
            RawFrameFormat::NV12 | RawFrameFormat::NV21 => {
                let chroma_height = ((height as usize) + 1) / 2;
                let mut y = vec![0u8; stride * height as usize];
                for (row, chunk) in y.chunks_mut(stride).enumerate() {
                    let value = ((row + index) % 256) as u8;
                    chunk.fill(value);
                }
                let uv = vec![128u8; stride * chroma_height];
                match self.output_format {
                    RawFrameFormat::NV12 => RawFrame::NV12 {
                        y_stride: stride,
                        uv_stride: stride,
                        y: y.into(),
                        uv: uv.into(),
                    },
                    RawFrameFormat::NV21 => RawFrame::NV21 {
                        y_stride: stride,
                        vu_stride: stride,
                        y: y.into(),
                        vu: uv.into(),
                    },
                    _ => unreachable!(),
                }
            }
            RawFrameFormat::I420 => {
                let chroma_width = ((width as usize) + 1) / 2;
                let chroma_height = ((height as usize) + 1) / 2;
                let mut y = vec![0u8; stride * height as usize];
                for (row, chunk) in y.chunks_mut(stride).enumerate() {
                    let value = ((row + index) % 256) as u8;
                    chunk.fill(value);
                }
                let u = vec![128u8; chroma_width * chroma_height];
                let v = vec![128u8; chroma_width * chroma_height];
                RawFrame::I420 {
                    y_stride: stride,
                    u_stride: chroma_width,
                    v_stride: chroma_width,
                    y: y.into(),
                    u: u.into(),
                    v: v.into(),
                }
            }
            RawFrameFormat::YUYV | RawFrameFormat::UYVY => {
                let packed_stride = width as usize * 2;
                let mut buffer = vec![0u8; packed_stride * height as usize];
                for row in 0..height as usize {
                    let y_value = ((row + index) % 256) as u8;
                    let line = &mut buffer[row * packed_stride..(row + 1) * packed_stride];
                    for pair in line.chunks_exact_mut(4) {
                        match self.output_format {
                            RawFrameFormat::YUYV => {
                                pair[0] = y_value;
                                pair[1] = 128;
                                pair[2] = y_value;
                                pair[3] = 128;
                            }
                            RawFrameFormat::UYVY => {
                                pair[0] = 128;
                                pair[1] = y_value;
                                pair[2] = 128;
                                pair[3] = y_value;
                            }
                            _ => {}
                        }
                    }
                }
                match self.output_format {
                    RawFrameFormat::YUYV => RawFrame::YUYV {
                        stride: packed_stride,
                        data: buffer.into(),
                    },
                    RawFrameFormat::UYVY => RawFrame::UYVY {
                        stride: packed_stride,
                        data: buffer.into(),
                    },
                    _ => unreachable!(),
                }
            }
        };

        PlaneFrame::from_raw(width, height, timestamp, raw)
            .map(|frame| frame.with_frame_index(Some(index as u64)))
    }
}

impl YPlaneStreamProvider for MockProvider {
    fn total_frames(&self) -> Option<u64> {
        Some(self.frame_count as u64)
    }

    fn into_stream(self: Box<Self>) -> PlaneStreamHandle {
        let provider = *self;
        let capacity = provider.channel_capacity;
        let (seek_tx, seek_rx) = std_mpsc::channel();
        let stream = spawn_stream_from_channel(capacity, move |tx| {
            provider.run_loop(tx, seek_rx);
        });
        PlaneStreamHandle::new(stream, Box::new(MockSeeker { tx: seek_tx }))
    }
}

fn timestamp_for_index(index: u64) -> Duration {
    Duration::from_millis(index.saturating_mul(MockProvider::FRAME_TIME_MS))
}

fn time_to_index(timestamp: Duration) -> u64 {
    let millis = timestamp.as_millis() as u64;
    millis / MockProvider::FRAME_TIME_MS
}

pub fn boxed_mock(
    path: Option<PathBuf>,
    channel_capacity: Option<usize>,
    output_format: RawFrameFormat,
) -> YPlaneResult<DynYPlaneProvider> {
    Ok(Box::new(MockProvider::open(
        path,
        channel_capacity,
        output_format,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_stream::StreamExt;

    #[tokio::test(flavor = "multi_thread")]
    async fn mock_backend_emits_frames() {
        let provider = boxed_mock(None, None, RawFrameFormat::Y).unwrap();
        let total = provider.total_frames();
        let mut stream = provider.into_stream();
        assert_eq!(total, Some(120));
        let frame = stream.next().await.unwrap().unwrap();
        assert_eq!(frame.width(), 640);
        assert_eq!(frame.height(), 360);
        let plane = frame.y_plane().expect("Y plane");
        assert_eq!(plane.data.len(), 640 * 360);
    }
}
