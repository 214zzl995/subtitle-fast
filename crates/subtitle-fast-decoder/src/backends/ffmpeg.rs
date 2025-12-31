use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use ffmpeg::util::error::{EAGAIN, EWOULDBLOCK};
use ffmpeg::util::mathematics::rescale::{Rescale, TIME_BASE};
use ffmpeg_next as ffmpeg;
use futures_util::stream::unfold;
use parking_lot::Mutex;

use crate::core::{
    DecoderController, DecoderError, DecoderProvider, DecoderResult, FrameStream, VideoFrame,
};
use tokio::sync::mpsc::Sender;

const BACKEND_NAME: &str = "ffmpeg";
const FILTER_SPEC: &str = "format=nv12";
const DEFAULT_CHANNEL_CAPACITY: usize = 8;

struct VideoFilterPipeline {
    _graph: ffmpeg::filter::Graph,
    source_ctx: *mut ffmpeg::ffi::AVFilterContext,
    sink_ctx: *mut ffmpeg::ffi::AVFilterContext,
    filtered: ffmpeg::util::frame::Video,
    frame_rate: Option<(i32, i32)>,
    next_fallback_index: u64,
    start_frame: Option<u64>,
    serial: Arc<AtomicU64>,
}

impl VideoFilterPipeline {
    fn new(
        time_base: ffmpeg::Rational,
        frame_rate: Option<(i32, i32)>,
        frame: &ffmpeg::util::frame::Video,
        start_frame: Option<u64>,
        serial: Arc<AtomicU64>,
    ) -> DecoderResult<Self> {
        let mut graph = ffmpeg::filter::Graph::new();

        let (time_base_num, time_base_den) = sanitize_rational(time_base, (1, 1));
        let (sar_num, sar_den) = sanitize_rational(frame.aspect_ratio(), (1, 1));
        let pixel_format = frame.format();
        let pixel_format_name = pixel_format
            .descriptor()
            .map(|descriptor| descriptor.name())
            .unwrap_or("yuv420p");
        let mut args = format!(
            "video_size={}x{}:pix_fmt={}:time_base={}/{}:pixel_aspect={}/{}",
            frame.width(),
            frame.height(),
            pixel_format_name,
            time_base_num,
            time_base_den,
            sar_num,
            sar_den
        );
        if let Some((num, den)) = frame_rate {
            args.push_str(&format!(":frame_rate={}/{}", num, den));
        }
        if let Some(colorspace) = colorspace_name(frame.color_space()) {
            args.push_str(&format!(":colorspace={colorspace}"));
        }
        if let Some(range) = color_range_name(frame.color_range()) {
            args.push_str(&format!(":range={range}"));
        }

        graph
            .add(
                &ffmpeg::filter::find("buffer").ok_or_else(|| {
                    DecoderError::backend_failure(BACKEND_NAME, "ffmpeg buffer filter not found")
                })?,
                "in",
                &args,
            )
            .map_err(|err| DecoderError::backend_failure(BACKEND_NAME, err.to_string()))?;

        graph
            .add(
                &ffmpeg::filter::find("buffersink").ok_or_else(|| {
                    DecoderError::backend_failure(
                        BACKEND_NAME,
                        "ffmpeg buffersink filter not found",
                    )
                })?,
                "out",
                "",
            )
            .map_err(|err| DecoderError::backend_failure(BACKEND_NAME, err.to_string()))?;

        graph
            .output("in", 0)
            .and_then(|parser| parser.input("out", 0))
            .and_then(|parser| parser.parse(FILTER_SPEC))
            .map_err(|err| DecoderError::backend_failure(BACKEND_NAME, err.to_string()))?;

        graph
            .validate()
            .map_err(|err| DecoderError::backend_failure(BACKEND_NAME, err.to_string()))?;

        let source_ctx = {
            let mut context = graph.get("in").ok_or_else(|| {
                DecoderError::backend_failure(
                    BACKEND_NAME,
                    "failed to access buffer source context",
                )
            })?;
            unsafe { context.as_mut_ptr() }
        };
        let sink_ctx = {
            let mut context = graph.get("out").ok_or_else(|| {
                DecoderError::backend_failure(BACKEND_NAME, "failed to access buffersink context")
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
            start_frame,
            serial,
        })
    }

    fn push(&mut self, frame: &ffmpeg::util::frame::Video) -> DecoderResult<()> {
        unsafe {
            let mut context = ffmpeg::filter::context::Context::wrap(self.source_ctx);
            let mut src = ffmpeg::filter::context::Source::wrap(&mut context);
            src.add(frame)
                .map_err(|err| DecoderError::backend_failure(BACKEND_NAME, err.to_string()))?;
        }
        Ok(())
    }

    fn flush(&mut self) -> DecoderResult<()> {
        unsafe {
            let mut context = ffmpeg::filter::context::Context::wrap(self.source_ctx);
            let mut src = ffmpeg::filter::context::Source::wrap(&mut context);
            src.flush()
                .map_err(|err| DecoderError::backend_failure(BACKEND_NAME, err.to_string()))?;
        }
        Ok(())
    }

    fn drain(
        &mut self,
        fallback_time_base: ffmpeg::Rational,
        tx: &Sender<DecoderResult<VideoFrame>>,
    ) -> DecoderResult<()> {
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
                        let serial = self.serial.load(Ordering::SeqCst);
                        let frame = frame_from_converted(
                            &self.filtered,
                            sink_time_base,
                            self.frame_rate,
                            &mut self.next_fallback_index,
                            serial,
                        )?;
                        ffmpeg::ffi::av_frame_unref(self.filtered.as_mut_ptr());
                        if let Some(start_frame) = self.start_frame
                            && frame
                                .frame_index()
                                .map_or(false, |index| index < start_frame)
                        {
                            continue;
                        }
                        if tx.blocking_send(Ok(frame)).is_err() {
                            break;
                        }
                    }
                    Err(err) => {
                        if is_retryable_error(&err) || matches!(err, ffmpeg::Error::Eof) {
                            break;
                        }
                        return Err(DecoderError::backend_failure(BACKEND_NAME, err.to_string()));
                    }
                }
            }
        }
        Ok(())
    }
}

struct FFmpegHandles {
    ictx: Arc<Mutex<ffmpeg::format::context::Input>>,
    decoder: Arc<Mutex<ffmpeg::decoder::Video>>,
    stream_index: usize,
    time_base: ffmpeg::Rational,
    frame_rate: Option<(i32, i32)>,
    tx: Sender<DecoderResult<VideoFrame>>,
    #[allow(dead_code)]
    abort_decode: AtomicBool,
    #[allow(dead_code)]
    seeking: AtomicBool,
}

pub struct FFmpegProvider {
    input: PathBuf,
    channel_capacity: usize,
    metadata: crate::core::VideoMetadata,
    start_frame: Option<u64>,
    handles: Option<FFmpegHandles>,
    serial: Option<Arc<AtomicU64>>,
}

impl FFmpegProvider {
    fn init_handles(&mut self, tx: Sender<DecoderResult<VideoFrame>>) -> DecoderResult<()> {
        if self.handles.is_some() {
            return Ok(());
        }

        let mut ictx = ffmpeg::format::input(&self.input)
            .map_err(|err| DecoderError::backend_failure(BACKEND_NAME, err.to_string()))?;
        let frame_rate = {
            let input_stream =
                ictx.streams()
                    .best(ffmpeg::media::Type::Video)
                    .ok_or_else(|| {
                        DecoderError::backend_failure(BACKEND_NAME, "no video stream found")
                    })?;
            stream_frame_rate(&input_stream)
        };

        if let Some(start_frame) = self.start_frame.filter(|value| *value > 0) {
            let frame_rate = frame_rate.ok_or_else(|| {
                DecoderError::configuration(
                    "ffmpeg backend requires frame rate metadata to seek to start_frame",
                )
            })?;
            seek_to_start_frame(&mut ictx, start_frame, frame_rate)?;
        }

        let (stream_index, time_base, frame_rate, parameters) = {
            let input_stream =
                ictx.streams()
                    .best(ffmpeg::media::Type::Video)
                    .ok_or_else(|| {
                        DecoderError::backend_failure(BACKEND_NAME, "no video stream found")
                    })?;
            (
                input_stream.index(),
                input_stream.time_base(),
                stream_frame_rate(&input_stream),
                input_stream.parameters(),
            )
        };

        let mut context = ffmpeg::codec::context::Context::from_parameters(parameters)
            .map_err(|err| DecoderError::backend_failure(BACKEND_NAME, err.to_string()))?;
        let mut threading = ffmpeg::codec::threading::Config::default();
        threading.kind = ffmpeg::codec::threading::Type::Frame;
        context.set_threading(threading);
        let decoder = context
            .decoder()
            .video()
            .map_err(|err| DecoderError::backend_failure(BACKEND_NAME, err.to_string()))?;

        self.handles = Some(FFmpegHandles {
            ictx: Arc::new(Mutex::new(ictx)),
            decoder: Arc::new(Mutex::new(decoder)),
            stream_index,
            time_base,
            frame_rate,
            tx,
            abort_decode: AtomicBool::new(false),
            seeking: AtomicBool::new(false),
        });

        Ok(())
    }

    fn decode_loop(&mut self) -> DecoderResult<()> {
        let serial = self
            .serial
            .as_ref()
            .ok_or_else(|| {
                DecoderError::backend_failure(BACKEND_NAME, "ffmpeg serial handle not set")
            })
            .map(Arc::clone)?;
        let handles = self.handles.as_mut().ok_or_else(|| {
            DecoderError::backend_failure(BACKEND_NAME, "ffmpeg backend was not initialized")
        })?;
        let stream_index = handles.stream_index;
        let time_base = handles.time_base;
        let frame_rate = handles.frame_rate;
        let tx = handles.tx.clone();
        let mut ictx = handles.ictx.lock();
        let mut decoder = handles.decoder.lock();

        let mut filter: Option<VideoFilterPipeline> = None;
        let mut decoded = ffmpeg::util::frame::Video::empty();

        for (stream, packet) in ictx.packets() {
            if stream.index() != stream_index {
                continue;
            }
            match decoder.send_packet(&packet) {
                Ok(_) => {}
                Err(err) if is_retryable_error(&err) => {}
                Err(err) => {
                    return Err(DecoderError::backend_failure(BACKEND_NAME, err.to_string()));
                }
            }
            drain_decoder(
                &mut *decoder,
                &mut decoded,
                &mut filter,
                time_base,
                frame_rate,
                self.start_frame,
                &tx,
                &serial,
            )?;
        }

        decoder
            .send_eof()
            .map_err(|err| DecoderError::backend_failure(BACKEND_NAME, err.to_string()))?;
        drain_decoder(
            &mut *decoder,
            &mut decoded,
            &mut filter,
            time_base,
            frame_rate,
            self.start_frame,
            &tx,
            &serial,
        )?;
        if let Some(filter) = filter.as_mut() {
            filter.flush()?;
            filter.drain(time_base, &tx)?;
        }
        Ok(())
    }

    #[allow(dead_code)]
    fn interrupt_and_seek(
        &self,
        target_ts: i64,
        flags: i32,
        filter: Option<&mut Option<VideoFilterPipeline>>,
    ) -> DecoderResult<()> {
        let handles = self.handles.as_ref().ok_or_else(|| {
            DecoderError::backend_failure(BACKEND_NAME, "ffmpeg backend was not initialized")
        })?;
        handles.abort_decode.store(true, Ordering::SeqCst);
        handles.seeking.store(true, Ordering::SeqCst);

        let mut ictx = handles.ictx.lock();
        let mut decoder = handles.decoder.lock();

        unsafe {
            let fmt_ctx = ictx.as_mut_ptr();
            if !fmt_ctx.is_null() {
                ffmpeg::ffi::avformat_flush(fmt_ctx);
                let stream_index = handles.stream_index as i32;
                let seek_result = ffmpeg::ffi::avformat_seek_file(
                    fmt_ctx,
                    stream_index,
                    i64::MIN,
                    target_ts,
                    i64::MAX,
                    flags,
                );
                if seek_result < 0 {
                    let fallback =
                        ffmpeg::ffi::av_seek_frame(fmt_ctx, stream_index, target_ts, flags);
                    if fallback < 0 {
                        return Err(DecoderError::backend_failure(
                            BACKEND_NAME,
                            format!(
                                "ffmpeg seek failed (avformat_seek_file={seek_result}, av_seek_frame={fallback})",
                            ),
                        ));
                    }
                }
            }

            let codec_ctx = decoder.as_mut_ptr();
            if !codec_ctx.is_null() {
                ffmpeg::ffi::avcodec_flush_buffers(codec_ctx);
            }
        }

        if let Some(filter) = filter {
            *filter = None;
        }

        Ok(())
    }
}

impl DecoderProvider for FFmpegProvider {
    fn new(config: &crate::config::Configuration) -> DecoderResult<Self> {
        let path = config
            .input
            .as_ref()
            .ok_or_else(|| DecoderError::configuration("FFmpeg backend requires SUBFAST_INPUT"))?;
        if !path.exists() {
            return Err(DecoderError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("input file {} does not exist", path.display()),
            )));
        }
        ffmpeg::init()
            .map_err(|err| DecoderError::backend_failure(BACKEND_NAME, err.to_string()))?;
        let metadata = probe_video_metadata(path)?;
        let capacity = config
            .channel_capacity
            .map(|n| n.get())
            .unwrap_or(DEFAULT_CHANNEL_CAPACITY)
            .max(1);
        Ok(Self {
            input: path.to_path_buf(),
            channel_capacity: capacity,
            metadata,
            start_frame: config.start_frame,
            handles: None,
            serial: None,
        })
    }

    fn metadata(&self) -> crate::core::VideoMetadata {
        self.metadata
    }

    fn open(self: Box<Self>) -> DecoderResult<(DecoderController, FrameStream)> {
        let mut provider = *self;
        let capacity = provider.channel_capacity;
        let (tx, rx) = tokio::sync::mpsc::channel(capacity);

        let controller = DecoderController::new();
        provider.serial = Some(controller.serial_handle());
        provider.init_handles(tx.clone())?;

        let stream = {
            let stream = unfold(rx, |mut receiver| async {
                receiver.recv().await.map(|item| (item, receiver))
            });
            Box::pin(stream)
        };

        tokio::task::spawn_blocking(move || {
            let result = provider.decode_loop();
            if let Err(err) = result {
                let _ = tx.blocking_send(Err(err));
            }
        });
        Ok((controller, stream))
    }
}

fn drain_decoder(
    decoder: &mut ffmpeg::decoder::Video,
    decoded: &mut ffmpeg::util::frame::Video,
    filter: &mut Option<VideoFilterPipeline>,
    fallback_time_base: ffmpeg::Rational,
    frame_rate: Option<(i32, i32)>,
    start_frame: Option<u64>,
    tx: &Sender<DecoderResult<VideoFrame>>,
    serial: &Arc<AtomicU64>,
) -> DecoderResult<()> {
    loop {
        match decoder.receive_frame(decoded) {
            Ok(_) => {
                if filter.is_none() {
                    let new_filter = VideoFilterPipeline::new(
                        fallback_time_base,
                        frame_rate,
                        decoded,
                        start_frame,
                        Arc::clone(serial),
                    )
                    .map_err(|err| {
                        unsafe {
                            ffmpeg::ffi::av_frame_unref(decoded.as_mut_ptr());
                        }
                        err
                    })?;
                    *filter = Some(new_filter);
                }
                if let Some(filter) = filter.as_mut() {
                    filter.push(decoded)?;
                    unsafe {
                        ffmpeg::ffi::av_frame_unref(decoded.as_mut_ptr());
                    }
                    filter.drain(fallback_time_base, tx)?;
                }
            }
            Err(err) => {
                if is_retryable_error(&err) || matches!(err, ffmpeg::Error::Eof) {
                    break;
                }
                return Err(DecoderError::backend_failure(BACKEND_NAME, err.to_string()));
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
    serial: u64,
) -> DecoderResult<VideoFrame> {
    let width = frame.width();
    let height = frame.height();
    let y_stride = frame.stride(0);
    let uv_stride = frame.stride(1);
    let y_plane = copy_plane(frame.data(0), y_stride, height as usize, "Y")?;
    let uv_rows = (height as usize + 1) / 2;
    let uv_plane = copy_plane(frame.data(1), uv_stride, uv_rows, "UV")?;
    let timestamp = frame.pts().map(|pts| {
        let seconds = pts as f64 * f64::from(time_base);
        Duration::from_secs_f64(seconds)
    });
    let frame_index = compute_frame_index(frame, time_base, frame_rate, next_fallback_index);
    let frame = VideoFrame::from_nv12_owned(
        width, height, y_stride, uv_stride, timestamp, y_plane, uv_plane,
    )?;
    Ok(frame.with_serial(serial).with_frame_index(frame_index))
}

fn copy_plane(plane: &[u8], stride: usize, rows: usize, label: &str) -> DecoderResult<Vec<u8>> {
    if stride == 0 && rows > 0 {
        return Err(DecoderError::InvalidFrame {
            reason: format!("NV12 {label} plane stride is zero"),
        });
    }
    let required = stride
        .checked_mul(rows)
        .ok_or_else(|| DecoderError::InvalidFrame {
            reason: format!("calculated NV12 {label} plane length overflowed"),
        })?;
    if plane.len() < required {
        return Err(DecoderError::InvalidFrame {
            reason: format!(
                "insufficient NV12 {label} plane bytes: got {} expected at least {}",
                plane.len(),
                required
            ),
        });
    }
    Ok(plane[..required].to_vec())
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

fn stream_frame_rate(stream: &ffmpeg::format::stream::Stream) -> Option<(i32, i32)> {
    optional_positive_rational(stream.avg_frame_rate())
        .or_else(|| optional_positive_rational(stream.rate()))
}

fn seek_to_start_frame(
    ictx: &mut ffmpeg::format::context::Input,
    start_frame: u64,
    frame_rate: (i32, i32),
) -> DecoderResult<()> {
    let (num, den) = frame_rate;
    let start_frame = i64::try_from(start_frame)
        .map_err(|_| DecoderError::configuration("start_frame is too large for ffmpeg seeking"))?;
    let frame_time_base = ffmpeg::Rational::new(den, num);
    let timestamp = start_frame.rescale(frame_time_base, TIME_BASE);
    ictx.seek(timestamp, ..)
        .map_err(|err| DecoderError::backend_failure(BACKEND_NAME, err.to_string()))?;
    Ok(())
}

fn is_retryable_error(error: &ffmpeg::Error) -> bool {
    matches!(
        error,
        ffmpeg::Error::Other { errno }
            if *errno == EAGAIN || *errno == EWOULDBLOCK
    )
}

fn probe_video_metadata(path: &Path) -> DecoderResult<crate::core::VideoMetadata> {
    use crate::core::VideoMetadata;

    let ictx = ffmpeg::format::input(path)
        .map_err(|err| DecoderError::backend_failure(BACKEND_NAME, err.to_string()))?;
    let input_stream = ictx
        .streams()
        .best(ffmpeg::media::Type::Video)
        .ok_or_else(|| DecoderError::backend_failure(BACKEND_NAME, "no video stream found"))?;

    let time_base = input_stream.time_base();
    let duration = if !is_invalid_time_base(time_base) {
        let duration_ticks = input_stream.duration();
        if duration_ticks > 0 {
            let seconds = duration_ticks as f64 * f64::from(time_base);
            if seconds.is_finite() && seconds > 0.0 {
                Some(Duration::from_secs_f64(seconds))
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    let fps = optional_positive_rational(input_stream.avg_frame_rate())
        .map(|(num, den)| num as f64 / den as f64);

    let context = ffmpeg::codec::context::Context::from_parameters(input_stream.parameters())
        .map_err(|err| DecoderError::backend_failure(BACKEND_NAME, err.to_string()))?;
    let decoder = context
        .decoder()
        .video()
        .map_err(|err| DecoderError::backend_failure(BACKEND_NAME, err.to_string()))?;
    let width = decoder.width() as u32;
    let height = decoder.height() as u32;

    let mut metadata = VideoMetadata::new();
    if width > 0 {
        metadata.width = Some(width);
    }
    if height > 0 {
        metadata.height = Some(height);
    }
    metadata.duration = duration;
    metadata.fps = fps;
    metadata.total_frames = metadata.calculate_total_frames();

    Ok(metadata)
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

fn colorspace_name(space: ffmpeg::color::Space) -> Option<&'static str> {
    match space {
        ffmpeg::color::Space::Unspecified | ffmpeg::color::Space::Reserved => None,
        _ => space.name(),
    }
}

fn color_range_name(range: ffmpeg::color::Range) -> Option<&'static str> {
    range.name()
}

fn is_invalid_time_base(value: ffmpeg::Rational) -> bool {
    let num = value.numerator();
    let den = value.denominator();
    num <= 0 || den <= 0
}
