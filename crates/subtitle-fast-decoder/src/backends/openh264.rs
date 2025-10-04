#![cfg(feature = "backend-openh264")]

use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use openh264::Error as OpenH264Error;
use openh264::decoder::Decoder;
use openh264::formats::YUVSource;
use openh264::nal_units;
use tokio::sync::mpsc;

use crate::core::{
    DynYPlaneProvider, YPlaneError, YPlaneFrame, YPlaneResult, YPlaneStream, YPlaneStreamProvider,
    spawn_stream_from_channel,
};

const BACKEND_NAME: &str = "openh264";

pub struct OpenH264Provider {
    input: PathBuf,
    worker_count: usize,
    channel_capacity: usize,
}

impl OpenH264Provider {
    pub fn open<P: AsRef<Path>>(path: P) -> YPlaneResult<Self> {
        let path = path.as_ref();
        if !path.exists() {
            return Err(YPlaneError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("input file {} does not exist", path.display()),
            )));
        }
        let worker_count = thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .max(1);
        Ok(Self {
            input: path.to_path_buf(),
            worker_count,
            channel_capacity: worker_count * 4,
        })
    }

    fn decode_loop(&self, tx: mpsc::Sender<YPlaneResult<YPlaneFrame>>) -> YPlaneResult<()> {
        let data = fs::read(&self.input)?;
        let gops = chunk_annexb_by_idr(&data);
        if gops.is_empty() {
            return Err(YPlaneError::backend_failure(
                BACKEND_NAME,
                "input stream did not contain decodable GOPs",
            ));
        }

        let total = gops.len();
        let mut results: Vec<Option<YPlaneResult<Vec<YPlaneFrame>>>> = vec![None; total];
        let mut handles: VecDeque<thread::JoinHandle<(usize, YPlaneResult<Vec<YPlaneFrame>>)>> =
            VecDeque::new();
        let mut tasks = gops.into_iter().enumerate();
        let limit = self.worker_count.max(1);

        loop {
            while handles.len() < limit {
                if let Some((index, chunk)) = tasks.next() {
                    handles.push_back(thread::spawn(move || (index, decode_chunk(chunk))));
                } else {
                    break;
                }
            }

            if handles.is_empty() {
                break;
            }

            let handle = handles
                .pop_front()
                .expect("handles must not be empty when scheduling results");
            let (index, result) = handle.join().map_err(|_| {
                YPlaneError::backend_failure(BACKEND_NAME, "OpenH264 worker panicked")
            })?;
            results[index] = Some(result);
        }

        for entry in results.into_iter() {
            let outcome = entry.expect("all GOP results must be populated");
            match outcome {
                Ok(frames) => {
                    for frame in frames {
                        if tx.blocking_send(Ok(frame)).is_err() {
                            return Ok(());
                        }
                    }
                }
                Err(err) => {
                    let _ = tx.blocking_send(Err(err));
                    return Ok(());
                }
            }
        }

        Ok(())
    }
}

impl YPlaneStreamProvider for OpenH264Provider {
    fn into_stream(self: Box<Self>) -> YPlaneStream {
        let provider = *self;
        let capacity = provider.channel_capacity;
        spawn_stream_from_channel(capacity, move |tx| {
            if let Err(err) = provider.decode_loop(tx.clone()) {
                let _ = tx.blocking_send(Err(err));
            }
        })
    }
}

fn decode_chunk(chunk: Vec<u8>) -> YPlaneResult<Vec<YPlaneFrame>> {
    let mut decoder = Decoder::new().map_err(map_openh264_error)?;
    let mut frames = Vec::new();
    for packet in nal_units(chunk.as_slice()) {
        match decoder.decode(packet) {
            Ok(Some(image)) => {
                frames.push(convert_frame(&image)?);
            }
            Ok(None) => {}
            Err(err) => {
                return Err(map_openh264_error(err));
            }
        }
    }
    for image in decoder.flush_remaining().map_err(map_openh264_error)? {
        frames.push(convert_frame(&image)?);
    }
    Ok(frames)
}

fn convert_frame(image: &openh264::decoder::DecodedYUV<'_>) -> YPlaneResult<YPlaneFrame> {
    let (width, height) = image.dimensions();
    let stride = image.strides().0;
    let plane = image.y();
    let mut buffer = Vec::with_capacity(stride * height);
    let plane_len = plane.len();
    for row in 0..height {
        let offset = row * stride;
        let end = offset + stride;
        if end <= plane_len {
            buffer.extend_from_slice(&plane[offset..end]);
        } else if offset < plane_len {
            buffer.extend_from_slice(&plane[offset..plane_len]);
            break;
        } else {
            break;
        }
    }
    if buffer.len() < stride * height {
        buffer.resize(stride * height, 0);
    }
    debug_assert_eq!(buffer.len(), stride * height);
    let timestamp = Some(Duration::from_millis(image.timestamp().as_millis()));
    YPlaneFrame::from_owned(width as u32, height as u32, stride, timestamp, buffer)
}

fn map_openh264_error(err: OpenH264Error) -> YPlaneError {
    YPlaneError::backend_failure(BACKEND_NAME, err.to_string())
}

fn chunk_annexb_by_idr(data: &[u8]) -> Vec<Vec<u8>> {
    let mut groups = Vec::new();
    let mut current = Vec::new();
    for unit in nal_units(data) {
        if is_idr_nal(unit) && !current.is_empty() {
            groups.push(std::mem::take(&mut current));
        }
        current.extend_from_slice(unit);
    }
    if !current.is_empty() {
        groups.push(current);
    }
    groups
}

fn is_idr_nal(unit: &[u8]) -> bool {
    nal_unit_type(unit).map(|ty| ty == 5).unwrap_or(false)
}

fn nal_unit_type(unit: &[u8]) -> Option<u8> {
    if unit.len() < 4 {
        return None;
    }
    for idx in 0..unit.len().saturating_sub(3) {
        if unit[idx] == 0 && unit[idx + 1] == 0 {
            if unit[idx + 2] == 1 {
                return unit.get(idx + 3).map(|value| value & 0x1F);
            }
            if idx + 4 < unit.len() && unit[idx + 2] == 0 && unit[idx + 3] == 1 {
                return unit.get(idx + 4).map(|value| value & 0x1F);
            }
        }
    }
    None
}

pub fn boxed_openh264<P: AsRef<Path>>(path: P) -> YPlaneResult<DynYPlaneProvider> {
    Ok(Box::new(OpenH264Provider::open(path)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_returns_error() {
        let result = OpenH264Provider::open("/tmp/nonexistent-file.mp4");
        assert!(result.is_err());
    }

    #[test]
    fn nal_unit_type_detects_idr() {
        let frame = vec![0, 0, 0, 1, 0x65, 0x88, 0x84];
        assert!(is_idr_nal(&frame));
    }
}
