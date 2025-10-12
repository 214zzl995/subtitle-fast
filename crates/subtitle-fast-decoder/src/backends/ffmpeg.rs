#![cfg(feature = "backend-ffmpeg")]

use std::path::{Path, PathBuf};
use std::time::Duration;

use ffmpeg::util::error::{EAGAIN, EWOULDBLOCK};
use ffmpeg_next as ffmpeg;

use crate::core::{
    DynYPlaneProvider, YPlaneError, YPlaneFrame, YPlaneResult, YPlaneStream, YPlaneStreamProvider,
    spawn_stream_from_channel,
};
use tokio::sync::mpsc::Sender;

const BACKEND_NAME: &str = "ffmpeg";
const FILTER_SPEC: &str =
    "crop=iw:ih*0.125:0:ih*0.875,scale=256:-2:flags=fast_bilinear,format=nv12";
const DEFAULT_CHANNEL_CAPACITY: usize = 8;

struct VideoFilterPipeline {
    _graph: ffmpeg::filter::Graph,
    source_ctx: *mut ffmpeg::ffi::AVFilterContext,
    sink_ctx: *mut ffmpeg::ffi::AVFilterContext,
    filtered: ffmpeg::util::frame::Video,
    frame_rate: Option<(i32, i32)>,
    next_fallback_index: u64,
}

impl VideoFilterPipeline {
    fn new(
        decoder: &ffmpeg::decoder::Video,
        stream: &ffmpeg::format::stream::Stream,
    ) -> YPlaneResult<Self> {
        let mut graph = ffmpeg::filter::Graph::new();

        let (time_base_num, time_base_den) = sanitize_rational(stream.time_base(), (1, 1));
        let (sar_num, sar_den) = sanitize_rational(decoder.aspect_ratio(), (1, 1));
        let frame_rate = optional_positive_rational(stream.avg_frame_rate());
        let pixel_format = decoder.format();
        let pixel_format_name = pixel_format
            .descriptor()
            .map(|descriptor| descriptor.name())
            .unwrap_or("yuv420p");
        let mut args = format!(
            "video_size={}x{}:pix_fmt={}:time_base={}/{}:pixel_aspect={}/{}",
            decoder.width(),
            decoder.height(),
            pixel_format_name,
            time_base_num,
            time_base_den,
            sar_num,
            sar_den
        );
        if let Some((num, den)) = frame_rate {
            args.push_str(&format!(":frame_rate={}/{}", num, den));
        }

        graph
            .add(
                &ffmpeg::filter::find("buffer").ok_or_else(|| {
                    YPlaneError::backend_failure(BACKEND_NAME, "ffmpeg buffer filter not found")
                })?,
                "in",
                &args,
            )
            .map_err(|err| YPlaneError::backend_failure(BACKEND_NAME, err.to_string()))?;

        graph
            .add(
                &ffmpeg::filter::find("buffersink").ok_or_else(|| {
                    YPlaneError::backend_failure(BACKEND_NAME, "ffmpeg buffersink filter not found")
                })?,
                "out",
                "",
            )
            .map_err(|err| YPlaneError::backend_failure(BACKEND_NAME, err.to_string()))?;

        graph
            .output("in", 0)
            .and_then(|parser| parser.input("out", 0))
            .and_then(|parser| parser.parse(FILTER_SPEC))
            .map_err(|err| YPlaneError::backend_failure(BACKEND_NAME, err.to_string()))?;

        graph
            .validate()
            .map_err(|err| YPlaneError::backend_failure(BACKEND_NAME, err.to_string()))?;

        let source_ctx = {
            let mut context = graph.get("in").ok_or_else(|| {
                YPlaneError::backend_failure(BACKEND_NAME, "failed to access buffer source context")
            })?;
            unsafe { context.as_mut_ptr() }
        };
        let sink_ctx = {
            let mut context = graph.get("out").ok_or_else(|| {
                YPlaneError::backend_failure(BACKEND_NAME, "failed to access buffersink context")
            })?;
            unsafe { context.as_mut_ptr() }
        };
        let filtered = ffmpeg::util::frame::Video::empty();

        Ok(Self {
            _graph: graph,
            source_ctx,
            sink_ctx,
            filtered,
            frame_rate,
            next_fallback_index: 0,
        })
    }

    fn push(&mut self, frame: &ffmpeg::util::frame::Video) -> YPlaneResult<()> {
        unsafe {
            let mut context = ffmpeg::filter::context::Context::wrap(self.source_ctx);
            let mut src = ffmpeg::filter::context::Source::wrap(&mut context);
            src.add(frame)
                .map_err(|err| YPlaneError::backend_failure(BACKEND_NAME, err.to_string()))?;
        }
        Ok(())
    }

    fn flush(&mut self) -> YPlaneResult<()> {
        unsafe {
            let mut context = ffmpeg::filter::context::Context::wrap(self.source_ctx);
            let mut src = ffmpeg::filter::context::Source::wrap(&mut context);
            src.flush()
                .map_err(|err| YPlaneError::backend_failure(BACKEND_NAME, err.to_string()))?;
        }
        Ok(())
    }

    fn drain(
        &mut self,
        fallback_time_base: ffmpeg::Rational,
        tx: &Sender<YPlaneResult<YPlaneFrame>>,
    ) -> YPlaneResult<()> {
        unsafe {
            let mut context = ffmpeg::filter::context::Context::wrap(self.sink_ctx);
            let mut sink = ffmpeg::filter::context::Sink::wrap(&mut context);
            let sink_time_base = {
                let tb = sink.time_base();
                if is_invalid_time_base(tb) {
                    fallback_time_base
                } else {
                    tb
                }
            };

            loop {
                match sink.frame(&mut self.filtered) {
                    Ok(()) => {
                        let frame = frame_from_converted(
                            &self.filtered,
                            sink_time_base,
                            self.frame_rate,
                            &mut self.next_fallback_index,
                        )?;
                        ffmpeg::ffi::av_frame_unref(self.filtered.as_mut_ptr());
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
        }
        Ok(())
    }
}

pub struct FfmpegProvider {
    input: PathBuf,
    channel_capacity: usize,
    total_frames: Option<u64>,
}

impl FfmpegProvider {
    pub fn open<P: AsRef<Path>>(path: P, channel_capacity: Option<usize>) -> YPlaneResult<Self> {
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
        let capacity = channel_capacity.unwrap_or(DEFAULT_CHANNEL_CAPACITY).max(1);
        Ok(Self {
            input: path.to_path_buf(),
            channel_capacity: capacity,
            total_frames,
        })
    }

    fn decode_loop(&self, tx: Sender<YPlaneResult<YPlaneFrame>>) -> YPlaneResult<()> {
        let mut ictx = ffmpeg::format::input(&self.input)
            .map_err(|err| YPlaneError::backend_failure(BACKEND_NAME, err.to_string()))?;
        let input_stream = ictx
            .streams()
            .best(ffmpeg::media::Type::Video)
            .ok_or_else(|| YPlaneError::backend_failure(BACKEND_NAME, "no video stream found"))?;
        let stream_index = input_stream.index();
        let time_base = input_stream.time_base();

        let mut context =
            ffmpeg::codec::context::Context::from_parameters(input_stream.parameters())
                .map_err(|err| YPlaneError::backend_failure(BACKEND_NAME, err.to_string()))?;
        let mut threading = ffmpeg::codec::threading::Config::default();
        threading.kind = ffmpeg::codec::threading::Type::Frame;
        context.set_threading(threading);
        let mut decoder = context
            .decoder()
            .video()
            .map_err(|err| YPlaneError::backend_failure(BACKEND_NAME, err.to_string()))?;

        let mut filter = VideoFilterPipeline::new(&decoder, &input_stream)?;
        let mut decoded = ffmpeg::util::frame::Video::empty();

        for (stream, packet) in ictx.packets() {
            if stream.index() != stream_index {
                continue;
            }
            if let Err(err) = decoder.send_packet(&packet) {
                if !is_retryable_error(&err) {
                    return Err(YPlaneError::backend_failure(BACKEND_NAME, err.to_string()));
                }
            }
            drain_decoder(&mut decoder, &mut decoded, &mut filter, time_base, &tx)?;
        }

        decoder
            .send_eof()
            .map_err(|err| YPlaneError::backend_failure(BACKEND_NAME, err.to_string()))?;
        drain_decoder(&mut decoder, &mut decoded, &mut filter, time_base, &tx)?;
        filter.flush()?;
        filter.drain(time_base, &tx)?;
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

fn drain_decoder(
    decoder: &mut ffmpeg::decoder::Video,
    decoded: &mut ffmpeg::util::frame::Video,
    filter: &mut VideoFilterPipeline,
    fallback_time_base: ffmpeg::Rational,
    tx: &Sender<YPlaneResult<YPlaneFrame>>,
) -> YPlaneResult<()> {
    loop {
        match decoder.receive_frame(decoded) {
            Ok(_) => {
                filter.push(decoded)?;
                unsafe {
                    ffmpeg::ffi::av_frame_unref(decoded.as_mut_ptr());
                }
                filter.drain(fallback_time_base, tx)?;
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
}

fn frame_from_converted(
    frame: &ffmpeg::util::frame::Video,
    time_base: ffmpeg::Rational,
    frame_rate: Option<(i32, i32)>,
    next_fallback_index: &mut u64,
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
    let frame_index = compute_frame_index(frame, time_base, frame_rate, next_fallback_index);
    let frame = YPlaneFrame::from_owned(width, height, stride, timestamp, buffer)?;
    Ok(frame.with_frame_index(frame_index))
}

fn compute_frame_index(
    frame: &ffmpeg::util::frame::Video,
    time_base: ffmpeg::Rational,
    frame_rate: Option<(i32, i32)>,
    next_fallback_index: &mut u64,
) -> Option<u64> {
    let pts = frame.timestamp().or_else(|| frame.pts());
    if let Some(pts) = pts {
        if let Some((num, den)) = frame_rate {
            if num > 0 && den > 0 {
                let time_base_seconds = f64::from(time_base);
                let fps = num as f64 / den as f64;
                let seconds = pts as f64 * time_base_seconds;
                let index = (seconds * fps).round();
                if index.is_finite() && index >= 0.0 {
                    let value = index as u64;
                    *next_fallback_index = value.saturating_add(1);
                    return Some(value);
                }
            }
        }
        if pts >= 0 {
            let value = pts as u64;
            *next_fallback_index = value.saturating_add(1);
            return Some(value);
        }
    }

    let value = *next_fallback_index;
    *next_fallback_index = next_fallback_index.saturating_add(1);
    Some(value)
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

fn sanitize_rational(value: ffmpeg::Rational, default: (i32, i32)) -> (i32, i32) {
    let num = value.numerator();
    let den = value.denominator();
    if num <= 0 || den <= 0 {
        default
    } else {
        (num, den)
    }
}

fn optional_positive_rational(value: ffmpeg::Rational) -> Option<(i32, i32)> {
    let num = value.numerator();
    let den = value.denominator();
    if num > 0 && den > 0 {
        Some((num, den))
    } else {
        None
    }
}

fn is_invalid_time_base(value: ffmpeg::Rational) -> bool {
    let num = value.numerator();
    let den = value.denominator();
    num <= 0 || den <= 0
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

pub fn boxed_ffmpeg<P: AsRef<Path>>(
    path: P,
    channel_capacity: Option<usize>,
) -> YPlaneResult<DynYPlaneProvider> {
    Ok(Box::new(FfmpegProvider::open(path, channel_capacity)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_returns_error() {
        let result = FfmpegProvider::open("/tmp/nonexistent-file.mp4", None);
        assert!(result.is_err());
    }
}