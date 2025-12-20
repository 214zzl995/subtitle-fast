use std::path::{Path, PathBuf};
use std::sync::mpsc as std_mpsc;
use std::time::Duration;

use ffmpeg::util::error::{EAGAIN, EWOULDBLOCK};
use ffmpeg_next as ffmpeg;

use crate::core::{
    DynYPlaneProvider, PlaneFrame, PlaneStreamHandle, RawFrameFormat, SeekControl, SeekPosition,
    YPlaneError, YPlaneResult, YPlaneStreamProvider, spawn_stream_from_channel,
};
use subtitle_fast_types::RawFrame;
use tokio::sync::mpsc::Sender;

const BACKEND_NAME: &str = "ffmpeg";
const DEFAULT_CHANNEL_CAPACITY: usize = 8;

enum SeekTarget {
    Time(Duration),
    Frame(u64),
}

struct SeekRequest {
    target: SeekTarget,
    respond_to: std_mpsc::Sender<YPlaneResult<SeekPosition>>,
}

struct FfmpegSeeker {
    tx: std_mpsc::Sender<SeekRequest>,
}

impl SeekControl for FfmpegSeeker {
    fn seek_to_time(&self, timestamp: Duration) -> YPlaneResult<SeekPosition> {
        let (tx, rx) = std_mpsc::channel();
        self.tx
            .send(SeekRequest {
                target: SeekTarget::Time(timestamp),
                respond_to: tx,
            })
            .map_err(|_| YPlaneError::backend_failure(BACKEND_NAME, "seek channel closed"))?;
        rx.recv().unwrap_or_else(|_| {
            Err(YPlaneError::backend_failure(
                BACKEND_NAME,
                "seek response failed",
            ))
        })
    }

    fn seek_to_frame(&self, frame_index: u64) -> YPlaneResult<SeekPosition> {
        let (tx, rx) = std_mpsc::channel();
        self.tx
            .send(SeekRequest {
                target: SeekTarget::Frame(frame_index),
                respond_to: tx,
            })
            .map_err(|_| YPlaneError::backend_failure(BACKEND_NAME, "seek channel closed"))?;
        rx.recv().unwrap_or_else(|_| {
            Err(YPlaneError::backend_failure(
                BACKEND_NAME,
                "seek response failed",
            ))
        })
    }
}

enum DecodeOutcome {
    Completed,
    Interrupted(SeekRequest),
}

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
        output_format: RawFrameFormat,
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

        let filter_spec = filter_spec_for_format(output_format);
        graph
            .output("in", 0)
            .and_then(|parser| parser.input("out", 0))
            .and_then(|parser| parser.parse(filter_spec))
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
        output_format: RawFrameFormat,
        mut on_frame: impl FnMut(PlaneFrame) -> YPlaneResult<bool>,
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
                            output_format,
                            &mut self.next_fallback_index,
                        )?;
                        ffmpeg::ffi::av_frame_unref(self.filtered.as_mut_ptr());
                        if !on_frame(frame)? {
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
    output_format: RawFrameFormat,
}

impl FfmpegProvider {
    pub fn open<P: AsRef<Path>>(
        path: P,
        channel_capacity: Option<usize>,
        output_format: RawFrameFormat,
    ) -> YPlaneResult<Self> {
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
            output_format,
        })
    }

    fn decode_loop(
        &self,
        tx: Sender<YPlaneResult<PlaneFrame>>,
        seek_rx: std_mpsc::Receiver<SeekRequest>,
    ) -> YPlaneResult<()> {
        let mut pending: Option<SeekRequest> = None;
        loop {
            if let Ok(request) = seek_rx.try_recv() {
                if let Some(prev) = pending.take() {
                    let _ = prev.respond_to.send(Err(YPlaneError::configuration(
                        "seek superseded by a newer request",
                    )));
                }
                pending = Some(request);
            }

            let outcome = decode_once(
                &self.input,
                self.output_format,
                tx.clone(),
                &seek_rx,
                pending.take(),
            )?;

            match outcome {
                DecodeOutcome::Completed => break,
                DecodeOutcome::Interrupted(request) => {
                    pending = Some(request);
                }
            }
        }
        Ok(())
    }
}

struct PendingSeek {
    request: SeekRequest,
}

fn decode_once(
    path: &Path,
    output_format: RawFrameFormat,
    tx: Sender<YPlaneResult<PlaneFrame>>,
    seek_rx: &std_mpsc::Receiver<SeekRequest>,
    pending: Option<SeekRequest>,
) -> YPlaneResult<DecodeOutcome> {
    let mut ictx = ffmpeg::format::input(path)
        .map_err(|err| YPlaneError::backend_failure(BACKEND_NAME, err.to_string()))?;
    let input_stream = ictx
        .streams()
        .best(ffmpeg::media::Type::Video)
        .ok_or_else(|| YPlaneError::backend_failure(BACKEND_NAME, "no video stream found"))?;
    let stream_index = input_stream.index();
    let time_base = input_stream.time_base();

    let mut context = ffmpeg::codec::context::Context::from_parameters(input_stream.parameters())
        .map_err(|err| YPlaneError::backend_failure(BACKEND_NAME, err.to_string()))?;
    let mut threading = ffmpeg::codec::threading::Config::default();
    threading.kind = ffmpeg::codec::threading::Type::Frame;
    context.set_threading(threading);
    let mut decoder = context
        .decoder()
        .video()
        .map_err(|err| YPlaneError::backend_failure(BACKEND_NAME, err.to_string()))?;

    let mut filter = VideoFilterPipeline::new(&decoder, &input_stream, output_format)?;
    let mut decoded = ffmpeg::util::frame::Video::empty();
    let mut pending = pending.map(|request| PendingSeek { request });
    let mut interrupt: Option<SeekRequest> = None;

    for (stream, packet) in ictx.packets() {
        if let Ok(request) = seek_rx.try_recv() {
            if let Some(prior) = pending.take() {
                let _ = prior
                    .request
                    .respond_to
                    .send(Err(YPlaneError::configuration(
                        "seek superseded by a newer request",
                    )));
            }
            interrupt = Some(request);
            break;
        }

        if stream.index() != stream_index {
            continue;
        }

        match decoder.send_packet(&packet) {
            Ok(_) => {}
            Err(err) if is_retryable_error(&err) => {}
            Err(err) => {
                return Err(YPlaneError::backend_failure(BACKEND_NAME, err.to_string()));
            }
        }
        drain_decoder(
            &mut decoder,
            &mut decoded,
            &mut filter,
            time_base,
            output_format,
            &tx,
            &mut pending,
            seek_rx,
            &mut interrupt,
        )?;
        if interrupt.is_some() {
            break;
        }
    }

    if interrupt.is_some() {
        return Ok(DecodeOutcome::Interrupted(
            interrupt.expect("interrupt set"),
        ));
    }

    decoder
        .send_eof()
        .map_err(|err| YPlaneError::backend_failure(BACKEND_NAME, err.to_string()))?;
    drain_decoder(
        &mut decoder,
        &mut decoded,
        &mut filter,
        time_base,
        output_format,
        &tx,
        &mut pending,
        seek_rx,
        &mut interrupt,
    )?;
    if interrupt.is_some() {
        return Ok(DecodeOutcome::Interrupted(
            interrupt.expect("interrupt set"),
        ));
    }
    filter.flush()?;
    filter.drain(time_base, output_format, |frame| {
        handle_frame(frame, &tx, &mut pending, seek_rx, &mut interrupt)
    })?;

    if interrupt.is_some() {
        return Ok(DecodeOutcome::Interrupted(
            interrupt.expect("interrupt set"),
        ));
    }

    if let Some(pending) = pending.take() {
        let _ = pending
            .request
            .respond_to
            .send(Err(YPlaneError::configuration(
                "seek target not reached before end of stream",
            )));
    }

    Ok(DecodeOutcome::Completed)
}

fn handle_frame(
    frame: PlaneFrame,
    tx: &Sender<YPlaneResult<PlaneFrame>>,
    pending: &mut Option<PendingSeek>,
    seek_rx: &std_mpsc::Receiver<SeekRequest>,
    interrupt: &mut Option<SeekRequest>,
) -> YPlaneResult<bool> {
    if let Ok(request) = seek_rx.try_recv() {
        if let Some(prior) = pending.take() {
            let _ = prior
                .request
                .respond_to
                .send(Err(YPlaneError::configuration(
                    "seek superseded by a newer request",
                )));
        }
        *interrupt = Some(request);
        return Ok(false);
    }

    if let Some(pending_seek) = pending {
        if frame_matches(&frame, &pending_seek.request.target) {
            let position = SeekPosition {
                timestamp: frame.timestamp(),
                frame_index: frame.frame_index(),
            };
            let _ = pending_seek.request.respond_to.send(Ok(position));
            *pending = None;
        } else {
            return Ok(true);
        }
    }

    Ok(tx.blocking_send(Ok(frame)).is_ok())
}

fn frame_matches(frame: &PlaneFrame, target: &SeekTarget) -> bool {
    match target {
        SeekTarget::Frame(index) => frame.frame_index() == Some(*index),
        SeekTarget::Time(timestamp) => frame
            .timestamp()
            .map(|ts| ts >= *timestamp)
            .unwrap_or(false),
    }
}

impl YPlaneStreamProvider for FfmpegProvider {
    fn total_frames(&self) -> Option<u64> {
        self.total_frames
    }

    fn into_stream(self: Box<Self>) -> PlaneStreamHandle {
        let provider = *self;
        let capacity = provider.channel_capacity;
        let (seek_tx, seek_rx) = std_mpsc::channel();
        let stream = spawn_stream_from_channel(capacity, move |tx| {
            let result = provider.decode_loop(tx.clone(), seek_rx);
            if let Err(err) = result {
                let _ = tx.blocking_send(Err(err));
            }
        });
        PlaneStreamHandle::new(stream, Box::new(FfmpegSeeker { tx: seek_tx }))
    }
}

fn drain_decoder(
    decoder: &mut ffmpeg::decoder::Video,
    decoded: &mut ffmpeg::util::frame::Video,
    filter: &mut VideoFilterPipeline,
    fallback_time_base: ffmpeg::Rational,
    output_format: RawFrameFormat,
    tx: &Sender<YPlaneResult<PlaneFrame>>,
    pending: &mut Option<PendingSeek>,
    seek_rx: &std_mpsc::Receiver<SeekRequest>,
    interrupt: &mut Option<SeekRequest>,
) -> YPlaneResult<()> {
    loop {
        match decoder.receive_frame(decoded) {
            Ok(_) => {
                filter.push(decoded)?;
                unsafe {
                    ffmpeg::ffi::av_frame_unref(decoded.as_mut_ptr());
                }
                filter.drain(fallback_time_base, output_format, |frame| {
                    handle_frame(frame, tx, pending, seek_rx, interrupt)
                })?;
                if interrupt.is_some() {
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
}

fn frame_from_converted(
    frame: &ffmpeg::util::frame::Video,
    time_base: ffmpeg::Rational,
    frame_rate: Option<(i32, i32)>,
    output_format: RawFrameFormat,
    next_fallback_index: &mut u64,
) -> YPlaneResult<PlaneFrame> {
    let width = frame.width();
    let height = frame.height();
    let timestamp = frame.pts().map(|pts| {
        let seconds = pts as f64 * f64::from(time_base);
        Duration::from_secs_f64(seconds)
    });
    let frame_index = compute_frame_index(frame, time_base, frame_rate, next_fallback_index);

    let raw = match output_format {
        RawFrameFormat::Y => {
            let stride = frame.stride(0);
            let data = copy_plane(frame.data(0), stride, height as usize);
            RawFrame::Y {
                stride,
                data: data.into(),
            }
        }
        RawFrameFormat::NV12 => {
            let y_stride = frame.stride(0);
            let uv_stride = frame.stride(1);
            let y = copy_plane(frame.data(0), y_stride, height as usize);
            let uv = copy_plane(frame.data(1), uv_stride, chroma_height(height));
            RawFrame::NV12 {
                y_stride,
                uv_stride,
                y: y.into(),
                uv: uv.into(),
            }
        }
        RawFrameFormat::NV21 => {
            let y_stride = frame.stride(0);
            let vu_stride = frame.stride(1);
            let y = copy_plane(frame.data(0), y_stride, height as usize);
            let vu = copy_plane(frame.data(1), vu_stride, chroma_height(height));
            RawFrame::NV21 {
                y_stride,
                vu_stride,
                y: y.into(),
                vu: vu.into(),
            }
        }
        RawFrameFormat::I420 => {
            let y_stride = frame.stride(0);
            let u_stride = frame.stride(1);
            let v_stride = frame.stride(2);
            let y = copy_plane(frame.data(0), y_stride, height as usize);
            let u = copy_plane(frame.data(1), u_stride, chroma_height(height));
            let v = copy_plane(frame.data(2), v_stride, chroma_height(height));
            RawFrame::I420 {
                y_stride,
                u_stride,
                v_stride,
                y: y.into(),
                u: u.into(),
                v: v.into(),
            }
        }
        RawFrameFormat::YUYV => {
            let stride = frame.stride(0);
            let data = copy_plane(frame.data(0), stride, height as usize);
            RawFrame::YUYV {
                stride,
                data: data.into(),
            }
        }
        RawFrameFormat::UYVY => {
            let stride = frame.stride(0);
            let data = copy_plane(frame.data(0), stride, height as usize);
            RawFrame::UYVY {
                stride,
                data: data.into(),
            }
        }
    };

    let frame = PlaneFrame::from_raw(width, height, timestamp, raw)?;
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
        if let Some((num, den)) = frame_rate.filter(|(num, den)| *num > 0 && *den > 0) {
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

fn chroma_height(height: u32) -> usize {
    ((height as usize) + 1) / 2
}

fn copy_plane(data: &[u8], stride: usize, rows: usize) -> Vec<u8> {
    let mut buffer = Vec::with_capacity(stride.saturating_mul(rows));
    for row in 0..rows {
        let offset = row.saturating_mul(stride);
        buffer.extend_from_slice(&data[offset..offset + stride]);
    }
    buffer
}

fn filter_spec_for_format(format: RawFrameFormat) -> &'static str {
    match format {
        RawFrameFormat::Y => "format=nv12",
        RawFrameFormat::NV12 => "format=nv12",
        RawFrameFormat::NV21 => "format=nv21",
        RawFrameFormat::I420 => "format=yuv420p",
        RawFrameFormat::YUYV => "format=yuyv422",
        RawFrameFormat::UYVY => "format=uyvy422",
    }
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
    output_format: RawFrameFormat,
) -> YPlaneResult<DynYPlaneProvider> {
    Ok(Box::new(FfmpegProvider::open(
        path,
        channel_capacity,
        output_format,
    )?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_returns_error() {
        let result = FfmpegProvider::open("/tmp/nonexistent-file.mp4", None, RawFrameFormat::Y);
        assert!(result.is_err());
    }
}
