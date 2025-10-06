#![cfg(feature = "backend-ffmpeg")]

use std::path::{Path, PathBuf};
use std::time::Duration;

use ffmpeg::util::error::{EAGAIN, EWOULDBLOCK};
use ffmpeg_next as ffmpeg;
use tokio::sync::mpsc;

use crate::core::{
    DynYPlaneProvider, YPlaneError, YPlaneFrame, YPlaneResult, YPlaneStream, YPlaneStreamProvider,
    spawn_stream_from_channel,
};

const BACKEND_NAME: &str = "ffmpeg";

pub struct FfmpegProvider {
    input: PathBuf,
    channel_capacity: usize,
    total_frames: Option<u64>,
}

impl FfmpegProvider {
    pub fn open<P: AsRef<Path>>(path: P) -> YPlaneResult<Self> {
        let path = path.as_ref();
        if !path.exists() {
            return Err(YPlaneError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("input file {} does not exist", path.display()),
            )));
        }
        ffmpeg::init()
            .map_err(|err| YPlaneError::backend_failure(BACKEND_NAME, err.to_string()))?;
        let total_frames = probe_total_frames(path)?;
        Ok(Self {
            input: path.to_path_buf(),
            channel_capacity: 8,
            total_frames,
        })
    }

    fn decode_loop(&self, tx: mpsc::Sender<YPlaneResult<YPlaneFrame>>) -> YPlaneResult<()> {
        let mut ictx = ffmpeg::format::input(&self.input)
            .map_err(|err| YPlaneError::backend_failure(BACKEND_NAME, err.to_string()))?;
        let input_stream = ictx
            .streams()
            .best(ffmpeg::media::Type::Video)
            .ok_or_else(|| YPlaneError::backend_failure(BACKEND_NAME, "no video stream found"))?;
        let stream_index = input_stream.index();
        let time_base = input_stream.time_base();

        let context = ffmpeg::codec::context::Context::from_parameters(input_stream.parameters())
            .map_err(|err| YPlaneError::backend_failure(BACKEND_NAME, err.to_string()))?;
        let mut decoder = context
            .decoder()
            .video()
            .map_err(|err| YPlaneError::backend_failure(BACKEND_NAME, err.to_string()))?;

        let target_format = ffmpeg::format::pixel::Pixel::YUV420P;
        let mut scaler = ffmpeg::software::scaling::context::Context::get(
            decoder.format(),
            decoder.width(),
            decoder.height(),
            target_format,
            decoder.width(),
            decoder.height(),
            ffmpeg::software::scaling::flag::Flags::FAST_BILINEAR,
        )
        .map_err(|err| YPlaneError::backend_failure(BACKEND_NAME, err.to_string()))?;

        let mut decoded = ffmpeg::util::frame::Video::empty();
        let mut converted = ffmpeg::util::frame::Video::empty();

        let mut drain = |decoder: &mut ffmpeg::decoder::Video| -> YPlaneResult<()> {
            loop {
                match decoder.receive_frame(&mut decoded) {
                    Ok(_) => {
                        scaler.run(&decoded, &mut converted).map_err(|err| {
                            YPlaneError::backend_failure(BACKEND_NAME, err.to_string())
                        })?;
                        converted.set_pts(decoded.pts());
                        let frame = frame_from_converted(&converted, time_base)?;
                        if tx.blocking_send(Ok(frame)).is_err() {
                            break;
                        }
                    }
                    Err(err) => {
                        if is_retryable_error(&err) || matches!(err, ffmpeg::Error::Eof) {
                            break;
                        }
                        return Err(YPlaneError::backend_failure(BACKEND_NAME, err.to_string()));
                    }
                }
            }
            Ok(())
        };

        for (stream, packet) in ictx.packets() {
            if stream.index() != stream_index {
                continue;
            }
            if let Err(err) = decoder.send_packet(&packet) {
                if !is_retryable_error(&err) {
                    return Err(YPlaneError::backend_failure(BACKEND_NAME, err.to_string()));
                }
            }
            drain(&mut decoder)?;
        }

        decoder
            .send_eof()
            .map_err(|err| YPlaneError::backend_failure(BACKEND_NAME, err.to_string()))?;
        drain(&mut decoder)?;
        Ok(())
    }
}

impl YPlaneStreamProvider for FfmpegProvider {
    fn total_frames(&self) -> Option<u64> {
        self.total_frames
    }

    fn into_stream(self: Box<Self>) -> YPlaneStream {
        let provider = *self;
        let capacity = provider.channel_capacity;
        spawn_stream_from_channel(capacity, move |tx| {
            let result = provider.decode_loop(tx.clone());
            if let Err(err) = result {
                let _ = tx.blocking_send(Err(err));
            }
        })
    }
}

fn frame_from_converted(
    frame: &ffmpeg::util::frame::Video,
    time_base: ffmpeg::Rational,
) -> YPlaneResult<YPlaneFrame> {
    let plane = frame.data(0);
    let stride = frame.stride(0) as usize;
    let width = frame.width();
    let height = frame.height();
    let mut buffer = Vec::with_capacity(stride * height as usize);
    for row in 0..height as usize {
        let offset = row * stride;
        buffer.extend_from_slice(&plane[offset..offset + stride]);
    }
    let timestamp = frame.pts().map(|pts| {
        let seconds = pts as f64 * f64::from(time_base);
        Duration::from_secs_f64(seconds)
    });
    YPlaneFrame::from_owned(width, height, stride, timestamp, buffer)
}

fn is_retryable_error(error: &ffmpeg::Error) -> bool {
    matches!(
        error,
        ffmpeg::Error::Other { errno }
            if *errno == EAGAIN || *errno == EWOULDBLOCK
    )
}

fn probe_total_frames(path: &Path) -> YPlaneResult<Option<u64>> {
    let ictx = ffmpeg::format::input(path)
        .map_err(|err| YPlaneError::backend_failure(BACKEND_NAME, err.to_string()))?;
    let stream = ictx
        .streams()
        .best(ffmpeg::media::Type::Video)
        .ok_or_else(|| YPlaneError::backend_failure(BACKEND_NAME, "no video stream found"))?;
    Ok(estimate_stream_total_frames(&stream))
}

fn estimate_stream_total_frames(stream: &ffmpeg::format::stream::Stream) -> Option<u64> {
    let frames = stream.frames();
    if frames > 0 {
        return Some(frames as u64);
    }

    let duration = stream.duration();
    if duration <= 0 {
        return None;
    }
    let time_base = stream.time_base();
    let avg_frame_rate = stream.avg_frame_rate();
    let seconds = (duration as f64) * f64::from(time_base);
    if !seconds.is_finite() || seconds <= 0.0 {
        return None;
    }
    let fps = f64::from(avg_frame_rate);
    if !fps.is_finite() || fps <= 0.0 {
        return None;
    }
    let total = (seconds * fps).round();
    if total.is_sign_negative() || !total.is_finite() {
        None
    } else {
        Some(total as u64)
    }
}

pub fn boxed_ffmpeg<P: AsRef<Path>>(path: P) -> YPlaneResult<DynYPlaneProvider> {
    Ok(Box::new(FfmpegProvider::open(path)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_returns_error() {
        let result = FfmpegProvider::open("/tmp/nonexistent-file.mp4");
        assert!(result.is_err());
    }
}
