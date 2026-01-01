use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use ffmpeg::format::Pixel;
use ffmpeg::media::Type;
use ffmpeg::software::scaling::{context::Context as Scaler, flag::Flags as ScaleFlags};
use ffmpeg::util::error::{EAGAIN, EWOULDBLOCK};
use ffmpeg_next as ffmpeg;
use tokio::sync::mpsc::Sender;

use crate::core::{
    DecoderController, DecoderError, DecoderProvider, DecoderResult, FrameStream, SeekInfo,
    SeekMode, SeekReceiver, VideoFrame, spawn_stream_from_channel,
};

const BACKEND_NAME: &str = "ffmpeg";
const DEFAULT_CHANNEL_CAPACITY: usize = 8;

pub struct FFmpegProvider {
    input: PathBuf,
    metadata: crate::core::VideoMetadata,
    channel_capacity: usize,
    start_frame: Option<u64>,
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
        let metadata = probe_metadata(path)?;
        let capacity = config
            .channel_capacity
            .map(|n| n.get())
            .unwrap_or(DEFAULT_CHANNEL_CAPACITY)
            .max(1);
        Ok(Self {
            input: path.to_path_buf(),
            metadata,
            channel_capacity: capacity,
            start_frame: config.start_frame,
        })
    }

    fn metadata(&self) -> crate::core::VideoMetadata {
        self.metadata
    }

    fn open(self: Box<Self>) -> DecoderResult<(DecoderController, FrameStream)> {
        let provider = *self;
        let capacity = provider.channel_capacity;
        let start_frame = provider.start_frame;
        let controller = DecoderController::new();
        let seek_rx = controller.seek_receiver();
        let serial = controller.serial_handle();
        let stream = spawn_stream_from_channel(capacity, move |tx| {
            if let Err(err) = decode_ffmpeg(
                provider.input.clone(),
                start_frame,
                tx.clone(),
                seek_rx,
                serial,
            ) {
                let _ = tx.blocking_send(Err(err));
            }
        });
        Ok((controller, stream))
    }
}

struct DecodeState {
    stream_index: usize,
    time_base: ffmpeg::Rational,
    frame_rate: Option<(i32, i32)>,
    next_index: u64,
    pending_drop: Option<DropUntil>,
    scaler: Option<Scaler>,
    source_format: Option<Pixel>,
    converted: ffmpeg::util::frame::Video,
}

#[derive(Clone, Copy)]
enum DropUntil {
    Timestamp(i64),
    Frame(u64),
}

#[derive(Clone, Copy)]
enum DrainOutcome {
    Continue,
    Seeked,
    Closed,
}

fn decode_ffmpeg(
    input: PathBuf,
    start_frame: Option<u64>,
    tx: Sender<DecoderResult<VideoFrame>>,
    mut seek_rx: SeekReceiver,
    serial: Arc<AtomicU64>,
) -> DecoderResult<()> {
    let mut ictx = ffmpeg::format::input(&input)
        .map_err(|err| DecoderError::backend_failure(BACKEND_NAME, err.to_string()))?;
    let stream = ictx
        .streams()
        .best(Type::Video)
        .ok_or_else(|| DecoderError::backend_failure(BACKEND_NAME, "no video stream found"))?;
    let stream_index = stream.index();
    let time_base = stream.time_base();
    let frame_rate = stream_frame_rate(&stream);

    let mut context = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
        .map_err(|err| DecoderError::backend_failure(BACKEND_NAME, err.to_string()))?;
    let mut threading = ffmpeg::codec::threading::Config::default();
    threading.kind = ffmpeg::codec::threading::Type::Frame;
    context.set_threading(threading);
    let mut decoder = context
        .decoder()
        .video()
        .map_err(|err| DecoderError::backend_failure(BACKEND_NAME, err.to_string()))?;

    let mut state = DecodeState {
        stream_index,
        time_base,
        frame_rate,
        next_index: 0,
        pending_drop: None,
        scaler: None,
        source_format: None,
        converted: ffmpeg::util::frame::Video::empty(),
    };

    if let Some(start_frame) = start_frame {
        if state.frame_rate.is_some() {
            let _ = perform_seek(
                SeekInfo::Frame {
                    frame: start_frame,
                    mode: SeekMode::Fast,
                },
                &mut ictx,
                &mut decoder,
                &mut state,
            )?;
            state.next_index = start_frame;
        }
    }

    let mut decoded = ffmpeg::util::frame::Video::empty();
    let mut current_serial = serial.load(Ordering::SeqCst);

    loop {
        if tx.is_closed() {
            return Ok(());
        }
        if let Some(info) = take_seek(&mut seek_rx) {
            state.pending_drop = perform_seek(info, &mut ictx, &mut decoder, &mut state)?;
            current_serial = serial.load(Ordering::SeqCst);
            continue;
        }
        let next_packet = {
            let mut packets = ictx.packets();
            packets
                .next()
                .map(|(stream, packet)| (stream.index(), packet))
        };
        let Some((stream_index, packet)) = next_packet else {
            break;
        };
        if stream_index != state.stream_index {
            continue;
        }
        match decoder.send_packet(&packet) {
            Ok(()) => {}
            Err(err) if is_retryable_error(&err) => {}
            Err(err) => {
                return Err(DecoderError::backend_failure(BACKEND_NAME, err.to_string()));
            }
        }
        match drain_decoder(
            &mut decoder,
            &mut decoded,
            &mut state,
            start_frame,
            current_serial,
            &tx,
            &mut seek_rx,
            &mut ictx,
        )? {
            DrainOutcome::Continue => {}
            DrainOutcome::Seeked => {
                current_serial = serial.load(Ordering::SeqCst);
            }
            DrainOutcome::Closed => return Ok(()),
        }
    }

    decoder
        .send_eof()
        .map_err(|err| DecoderError::backend_failure(BACKEND_NAME, err.to_string()))?;
    let _ = drain_decoder(
        &mut decoder,
        &mut decoded,
        &mut state,
        start_frame,
        current_serial,
        &tx,
        &mut seek_rx,
        &mut ictx,
    )?;
    Ok(())
}

fn drain_decoder(
    decoder: &mut ffmpeg::decoder::Video,
    decoded: &mut ffmpeg::util::frame::Video,
    state: &mut DecodeState,
    start_frame: Option<u64>,
    current_serial: u64,
    tx: &Sender<DecoderResult<VideoFrame>>,
    seek_rx: &mut SeekReceiver,
    ictx: &mut ffmpeg::format::context::Input,
) -> DecoderResult<DrainOutcome> {
    loop {
        if let Some(info) = take_seek(seek_rx) {
            state.pending_drop = perform_seek(info, ictx, decoder, state)?;
            return Ok(DrainOutcome::Seeked);
        }
        match decoder.receive_frame(decoded) {
            Ok(()) => {
                let pts_value = frame_pts(decoded);
                let dts_value = frame_dts(decoded);
                let pts = pts_value.and_then(|value| timestamp_from_pts(value, state.time_base));
                let dts = dts_value.and_then(|value| timestamp_from_pts(value, state.time_base));
                let frame_index = frame_index_from_pts(
                    pts_value,
                    state.frame_rate,
                    state.time_base,
                    &mut state.next_index,
                );

                if let Some(drop) = state.pending_drop {
                    let keep = match drop {
                        DropUntil::Timestamp(target) => {
                            pts_value.map(|value| value >= target).unwrap_or(true)
                        }
                        DropUntil::Frame(target) => {
                            frame_index.map(|value| value >= target).unwrap_or(true)
                        }
                    };
                    if keep {
                        state.pending_drop = None;
                    } else {
                        unsafe { ffmpeg::ffi::av_frame_unref(decoded.as_mut_ptr()) };
                        continue;
                    }
                }

                if let (Some(start_frame), Some(index)) = (start_frame, frame_index) {
                    if index < start_frame {
                        unsafe { ffmpeg::ffi::av_frame_unref(decoded.as_mut_ptr()) };
                        continue;
                    }
                }

                ensure_scaler(state, decoded)?;
                let frame = build_frame(&state.converted, pts, dts, frame_index, current_serial)?;
                unsafe { ffmpeg::ffi::av_frame_unref(decoded.as_mut_ptr()) };
                if tx.blocking_send(Ok(frame)).is_err() {
                    return Ok(DrainOutcome::Closed);
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
    Ok(DrainOutcome::Continue)
}

fn ensure_scaler(
    state: &mut DecodeState,
    decoded: &ffmpeg::util::frame::Video,
) -> DecoderResult<()> {
    let width = decoded.width();
    let height = decoded.height();
    let format = decoded.format();
    let needs_rebuild = state.scaler.is_none()
        || state.source_format != Some(format)
        || state.converted.width() != width
        || state.converted.height() != height;

    if needs_rebuild {
        state.scaler = Some(
            Scaler::get(
                format,
                width,
                height,
                Pixel::NV12,
                width,
                height,
                ScaleFlags::BILINEAR,
            )
            .map_err(|err| DecoderError::backend_failure(BACKEND_NAME, err.to_string()))?,
        );
        state.source_format = Some(format);
        unsafe {
            state.converted.alloc(Pixel::NV12, width, height);
        }
    }

    if let Some(ref mut scaler) = state.scaler {
        scaler
            .run(decoded, &mut state.converted)
            .map_err(|err| DecoderError::backend_failure(BACKEND_NAME, err.to_string()))?;
    }
    Ok(())
}

fn build_frame(
    converted: &ffmpeg::util::frame::Video,
    pts: Option<Duration>,
    dts: Option<Duration>,
    frame_index: Option<u64>,
    serial: u64,
) -> DecoderResult<VideoFrame> {
    let width = converted.width();
    let height = converted.height();
    let y_stride = converted.stride(0);
    let uv_stride = converted.stride(1);
    let y_plane = copy_plane(converted.data(0), y_stride, height as usize, "Y")?;
    let uv_rows = (height as usize + 1) / 2;
    let uv_plane = copy_plane(converted.data(1), uv_stride, uv_rows, "UV")?;
    VideoFrame::from_nv12_owned(
        width, height, y_stride, uv_stride, pts, dts, y_plane, uv_plane,
    )
    .map(|frame| frame.with_serial(serial).with_index(frame_index))
}

fn perform_seek(
    info: SeekInfo,
    ictx: &mut ffmpeg::format::context::Input,
    decoder: &mut ffmpeg::decoder::Video,
    state: &mut DecodeState,
) -> DecoderResult<Option<DropUntil>> {
    let target = seek_target(info, state.time_base, state.frame_rate)?;
    let flags = match target.mode {
        SeekMode::Fast => ffmpeg::ffi::AVSEEK_FLAG_ANY,
        SeekMode::Accurate => 0,
    };

    unsafe {
        let fmt_ctx = ictx.as_mut_ptr();
        if !fmt_ctx.is_null() {
            ffmpeg::ffi::avformat_flush(fmt_ctx);
            let stream_index = state.stream_index as i32;
            let result = ffmpeg::ffi::avformat_seek_file(
                fmt_ctx,
                stream_index,
                i64::MIN,
                target.timestamp,
                i64::MAX,
                flags,
            );
            if result < 0 {
                let fallback =
                    ffmpeg::ffi::av_seek_frame(fmt_ctx, stream_index, target.timestamp, flags);
                if fallback < 0 {
                    return Err(DecoderError::backend_failure(
                        BACKEND_NAME,
                        format!(
                            "ffmpeg seek failed (avformat_seek_file={result}, av_seek_frame={fallback})"
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

    if let Some(frame) = target.frame {
        state.next_index = frame;
    } else if let Some((num, den)) = state.frame_rate {
        if num > 0 && den > 0 {
            let fps = num as f64 / den as f64;
            let estimate = (target.seconds * fps).round();
            if estimate.is_finite() && estimate >= 0.0 {
                state.next_index = estimate as u64;
            }
        }
    }

    let drop = match target.mode {
        SeekMode::Fast => None,
        SeekMode::Accurate => match target.frame {
            Some(frame) => Some(DropUntil::Frame(frame)),
            None => Some(DropUntil::Timestamp(target.timestamp)),
        },
    };
    Ok(drop)
}

struct SeekTarget {
    timestamp: i64,
    seconds: f64,
    frame: Option<u64>,
    mode: SeekMode,
}

fn seek_target(
    info: SeekInfo,
    time_base: ffmpeg::Rational,
    frame_rate: Option<(i32, i32)>,
) -> DecoderResult<SeekTarget> {
    let (seconds, mode, frame) = match info {
        SeekInfo::Time { position, mode } => (position.as_secs_f64(), mode, None),
        SeekInfo::Frame { frame, mode } => {
            let (num, den) = frame_rate.ok_or_else(|| {
                DecoderError::configuration(
                    "ffmpeg backend requires frame rate metadata to seek by frame",
                )
            })?;
            if num <= 0 || den <= 0 {
                return Err(DecoderError::configuration(
                    "ffmpeg backend requires a valid frame rate to seek by frame",
                ));
            }
            let fps = num as f64 / den as f64;
            (frame as f64 / fps, mode, Some(frame))
        }
    };

    if !seconds.is_finite() || seconds.is_sign_negative() {
        return Err(DecoderError::configuration("invalid seek timestamp"));
    }

    let Some(time_base_seconds) = time_base_seconds(time_base) else {
        return Err(DecoderError::backend_failure(
            BACKEND_NAME,
            "ffmpeg time base is invalid".to_string(),
        ));
    };

    let target = seconds / time_base_seconds;
    if !target.is_finite() || target < i64::MIN as f64 || target > i64::MAX as f64 {
        return Err(DecoderError::configuration(
            "seek timestamp is out of range",
        ));
    }

    Ok(SeekTarget {
        timestamp: target.round() as i64,
        seconds,
        frame,
        mode,
    })
}

fn frame_index_from_pts(
    pts: Option<i64>,
    frame_rate: Option<(i32, i32)>,
    time_base: ffmpeg::Rational,
    next_index: &mut u64,
) -> Option<u64> {
    if let Some(value) = pts {
        if let Some((num, den)) = frame_rate {
            if num > 0 && den > 0 {
                if let Some(time_base_seconds) = time_base_seconds(time_base) {
                    let fps = num as f64 / den as f64;
                    let seconds = value as f64 * time_base_seconds;
                    let index = (seconds * fps).round();
                    if index.is_finite() && index >= 0.0 {
                        let value = index as u64;
                        *next_index = value.saturating_add(1);
                        return Some(value);
                    }
                }
            }
        }

        if value >= 0 {
            let value = value as u64;
            *next_index = value.saturating_add(1);
            return Some(value);
        }
    }

    let value = *next_index;
    *next_index = next_index.saturating_add(1);
    Some(value)
}

fn frame_pts(frame: &ffmpeg::util::frame::Video) -> Option<i64> {
    frame.timestamp().or_else(|| frame.pts())
}

fn frame_dts(frame: &ffmpeg::util::frame::Video) -> Option<i64> {
    let dts = frame.packet().dts;
    if dts == ffmpeg::ffi::AV_NOPTS_VALUE {
        None
    } else {
        Some(dts)
    }
}

fn timestamp_from_pts(value: i64, time_base: ffmpeg::Rational) -> Option<Duration> {
    let seconds = value as f64 * time_base_seconds(time_base)?;
    if seconds.is_finite() && seconds >= 0.0 {
        Some(Duration::from_secs_f64(seconds))
    } else {
        None
    }
}

fn time_base_seconds(value: ffmpeg::Rational) -> Option<f64> {
    let seconds = f64::from(value);
    if seconds.is_finite() && seconds > 0.0 {
        Some(seconds)
    } else {
        None
    }
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

fn take_seek(seek_rx: &mut SeekReceiver) -> Option<SeekInfo> {
    if !seek_rx.has_changed().unwrap_or(false) {
        return None;
    }
    *seek_rx.borrow_and_update()
}

fn is_retryable_error(error: &ffmpeg::Error) -> bool {
    matches!(
        error,
        ffmpeg::Error::Other { errno }
            if *errno == EAGAIN || *errno == EWOULDBLOCK
    )
}

fn stream_frame_rate(stream: &ffmpeg::format::stream::Stream) -> Option<(i32, i32)> {
    let avg = stream.avg_frame_rate();
    let num = avg.numerator();
    let den = avg.denominator();
    if num > 0 && den > 0 {
        return Some((num, den));
    }
    let rate = stream.rate();
    let num = rate.numerator();
    let den = rate.denominator();
    if num > 0 && den > 0 {
        Some((num, den))
    } else {
        None
    }
}

fn probe_metadata(path: &Path) -> DecoderResult<crate::core::VideoMetadata> {
    use crate::core::VideoMetadata;

    let ictx = ffmpeg::format::input(path)
        .map_err(|err| DecoderError::backend_failure(BACKEND_NAME, err.to_string()))?;
    let stream = ictx
        .streams()
        .best(Type::Video)
        .ok_or_else(|| DecoderError::backend_failure(BACKEND_NAME, "no video stream found"))?;
    let time_base = stream.time_base();
    let duration = match (stream.duration(), time_base_seconds(time_base)) {
        (ticks, Some(seconds)) if ticks > 0 => {
            let duration = ticks as f64 * seconds;
            if duration.is_finite() && duration > 0.0 {
                Some(Duration::from_secs_f64(duration))
            } else {
                None
            }
        }
        _ => None,
    };

    let fps = stream_frame_rate(&stream).map(|(num, den)| num as f64 / den as f64);

    let context = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
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
