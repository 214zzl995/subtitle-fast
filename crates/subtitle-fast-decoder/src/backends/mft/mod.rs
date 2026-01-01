#[cfg(all(target_os = "windows", feature = "backend-mft"))]
use crate::core::{
    DecoderController, DecoderError, DecoderProvider, DecoderResult, FrameStream, SeekInfo,
    SeekMode, SeekReceiver,
};

#[cfg(all(target_os = "windows", feature = "backend-mft"))]
use crate::core::{VideoFrame, spawn_stream_from_channel};

#[cfg(all(target_os = "windows", feature = "backend-mft"))]
#[allow(unexpected_cfgs)]
mod platform {
    use super::*;
    use std::ffi::{CStr, CString, c_char, c_void};
    use std::path::{Path, PathBuf};
    use std::ptr;
    use std::slice;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;
    use tokio::sync::mpsc::Sender;

    const BACKEND_NAME: &str = "mft";
    const DEFAULT_CHANNEL_CAPACITY: usize = 16;

    #[repr(C)]
    struct CMftProbeResult {
        has_value: bool,
        value: u64,
        duration_seconds: f64,
        fps: f64,
        width: u32,
        height: u32,
        error: *mut c_char,
    }

    #[repr(C)]
    struct CMftFrame {
        y_data: *const u8,
        y_len: usize,
        y_stride: usize,
        uv_data: *const u8,
        uv_len: usize,
        uv_stride: usize,
        width: u32,
        height: u32,
        pts_seconds: f64,
        dts_seconds: f64,
        index: u64,
    }

    type CMftFrameCallback = unsafe extern "C" fn(*const CMftFrame, *mut c_void) -> bool;
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CMftSeekRequest {
        position_seconds: f64,
        start_frame: u64,
    }

    type CMftSeekCallback = unsafe extern "C" fn(*mut c_void, *mut CMftSeekRequest) -> i32;

    const SEEK_ACTION_CONTINUE: i32 = 0;
    const SEEK_ACTION_STOP: i32 = 1;
    const SEEK_ACTION_SEEK: i32 = 2;

    #[allow(improper_ctypes)]
    unsafe extern "C" {
        fn mft_probe_total_frames(path: *const c_char, result: *mut CMftProbeResult) -> bool;
        fn mft_decode(
            path: *const c_char,
            has_start_frame: bool,
            start_frame: u64,
            callback: CMftFrameCallback,
            context: *mut c_void,
            seek_callback: CMftSeekCallback,
            out_error: *mut *mut c_char,
        ) -> bool;
        fn mft_string_free(ptr: *mut c_char);
    }

    pub struct MftProvider {
        input: PathBuf,
        metadata: crate::core::VideoMetadata,
        channel_capacity: usize,
        start_frame: Option<u64>,
    }

    impl MftProvider {}

    impl DecoderProvider for MftProvider {
        fn new(config: &crate::config::Configuration) -> DecoderResult<Self> {
            let path = config.input.as_ref().ok_or_else(|| {
                DecoderError::configuration("MFT backend requires SUBFAST_INPUT to be set")
            })?;
            if !path.exists() {
                return Err(DecoderError::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("input file {} does not exist", path.display()),
                )));
            }
            let metadata = probe_video_metadata(path)?;
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
            let fps = provider.metadata.fps;
            let controller = DecoderController::new();
            let seek_rx = controller.seek_receiver();
            let serial = controller.serial_handle();
            let stream = spawn_stream_from_channel(capacity, move |tx| {
                if let Err(err) = decode_mft(
                    provider.input.clone(),
                    tx.clone(),
                    start_frame,
                    seek_rx,
                    serial,
                    fps,
                ) {
                    let _ = tx.blocking_send(Err(err));
                }
            });
            Ok((controller, stream))
        }
    }

    fn decode_mft(
        path: PathBuf,
        tx: Sender<DecoderResult<VideoFrame>>,
        start_frame: Option<u64>,
        seek_rx: SeekReceiver,
        serial: Arc<AtomicU64>,
        fps: Option<f64>,
    ) -> DecoderResult<()> {
        let c_path = cstring_from_path(&path)?;
        let mut context = DecodeContext::new(tx, seek_rx, serial, fps);
        let mut error_ptr: *mut c_char = ptr::null_mut();
        let (has_start_frame, start_frame) = match start_frame {
            Some(value) => (true, value),
            None => (false, 0),
        };
        let ok = unsafe {
            mft_decode(
                c_path.as_ptr(),
                has_start_frame,
                start_frame,
                handle_frame,
                &mut context as *mut _ as *mut c_void,
                poll_seek_requests,
                &mut error_ptr,
            )
        };
        let bridge_error = take_bridge_string(error_ptr);
        if let Some(err) = context.take_seek_error() {
            return Err(err);
        }
        if context.is_closed() {
            return Ok(());
        }
        if !ok {
            let message = bridge_error.unwrap_or_else(|| "decode failed".to_string());
            return Err(DecoderError::backend_failure(BACKEND_NAME, message));
        }
        if let Some(message) = bridge_error {
            if !message.is_empty() {
                return Err(DecoderError::backend_failure(BACKEND_NAME, message));
            }
        }
        Ok(())
    }

    fn probe_video_metadata(path: &Path) -> DecoderResult<crate::core::VideoMetadata> {
        use crate::core::VideoMetadata;

        let c_path = cstring_from_path(path)?;
        let mut result = CMftProbeResult {
            has_value: false,
            value: 0,
            duration_seconds: 0.0,
            fps: 0.0,
            width: 0,
            height: 0,
            error: ptr::null_mut(),
        };
        let ok = unsafe { mft_probe_total_frames(c_path.as_ptr(), &mut result) };
        let bridge_error = take_bridge_string(result.error);
        if !ok {
            let message = bridge_error.unwrap_or_else(|| "probe failed".to_string());
            return Err(DecoderError::backend_failure(BACKEND_NAME, message));
        }
        if let Some(message) = bridge_error {
            if !message.is_empty() {
                return Err(DecoderError::backend_failure(BACKEND_NAME, message));
            }
        }

        let mut metadata = VideoMetadata::new();
        if result.has_value {
            metadata.total_frames = Some(result.value);
        }
        if result.duration_seconds.is_finite() && result.duration_seconds > 0.0 {
            metadata.duration = Some(Duration::from_secs_f64(result.duration_seconds));
        }
        if result.fps.is_finite() && result.fps > 0.0 {
            metadata.fps = Some(result.fps);
        }
        if result.width > 0 {
            metadata.width = Some(result.width);
        }
        if result.height > 0 {
            metadata.height = Some(result.height);
        }

        Ok(metadata)
    }

    fn cstring_from_path(path: &Path) -> DecoderResult<CString> {
        CString::new(path.to_string_lossy().as_bytes()).map_err(|err| {
            DecoderError::backend_failure(BACKEND_NAME, format!("invalid path encoding: {err}"))
        })
    }

    fn take_bridge_string(ptr: *mut c_char) -> Option<String> {
        if ptr.is_null() {
            return None;
        }
        let message = unsafe { CStr::from_ptr(ptr).to_string_lossy().into_owned() };
        unsafe { mft_string_free(ptr) };
        Some(message)
    }

    struct DecodeContext {
        tx: Sender<DecoderResult<VideoFrame>>,
        seek_rx: SeekReceiver,
        serial: Arc<AtomicU64>,
        current_serial: u64,
        pending_drop: Option<DropUntil>,
        seek_error: Option<DecoderError>,
        closed: bool,
        fps: Option<f64>,
    }

    impl DecodeContext {
        fn new(
            tx: Sender<DecoderResult<VideoFrame>>,
            seek_rx: SeekReceiver,
            serial: Arc<AtomicU64>,
            fps: Option<f64>,
        ) -> Self {
            let current_serial = serial.load(Ordering::SeqCst);
            Self {
                tx,
                seek_rx,
                serial,
                current_serial,
                pending_drop: None,
                seek_error: None,
                closed: false,
                fps,
            }
        }

        fn is_closed(&self) -> bool {
            self.closed || self.tx.is_closed()
        }

        fn apply_drop(&mut self, drop_until: Option<DropUntil>) {
            self.pending_drop = drop_until;
        }

        fn take_seek_error(&mut self) -> Option<DecoderError> {
            self.seek_error.take()
        }

        fn send_frame(&mut self, frame: VideoFrame) -> bool {
            if self.tx.blocking_send(Ok(frame)).is_ok() {
                true
            } else {
                self.closed = true;
                false
            }
        }

        fn send_error(&mut self, error: DecoderError) {
            let _ = self.tx.blocking_send(Err(error));
            self.closed = true;
        }

        fn should_skip_frame(&mut self, index: u64, pts: Option<Duration>) -> bool {
            let Some(drop_until) = self.pending_drop else {
                return false;
            };
            let keep = match drop_until {
                DropUntil::Frame(target) => index >= target,
                DropUntil::Timestamp(target) => pts.map(|value| value >= target).unwrap_or(true),
            };
            if keep {
                self.pending_drop = None;
                false
            } else {
                true
            }
        }
    }

    unsafe extern "C" fn handle_frame(frame: *const CMftFrame, context: *mut c_void) -> bool {
        if frame.is_null() || context.is_null() {
            return false;
        }
        let frame = unsafe { &*frame };
        let context = unsafe { &mut *(context as *mut DecodeContext) };
        if context.is_closed() {
            return false;
        }
        if frame.y_data.is_null() || frame.uv_data.is_null() {
            context.send_error(DecoderError::backend_failure(
                BACKEND_NAME,
                "NV12 plane pointer is null",
            ));
            return false;
        }
        let y_data = unsafe { slice::from_raw_parts(frame.y_data, frame.y_len) };
        let uv_data = unsafe { slice::from_raw_parts(frame.uv_data, frame.uv_len) };
        let pts = if frame.pts_seconds.is_finite() && frame.pts_seconds >= 0.0 {
            Some(Duration::from_secs_f64(frame.pts_seconds))
        } else {
            None
        };
        let dts = if frame.dts_seconds.is_finite() && frame.dts_seconds >= 0.0 {
            Some(Duration::from_secs_f64(frame.dts_seconds))
        } else {
            None
        };
        let index = pts
            .and_then(|pts| index_from_pts(pts, context.fps))
            .or(Some(frame.index));
        if context.should_skip_frame(index.unwrap_or(frame.index), pts) {
            return true;
        }
        match VideoFrame::from_nv12_owned(
            frame.width,
            frame.height,
            frame.y_stride,
            frame.uv_stride,
            pts,
            dts,
            y_data.to_vec(),
            uv_data.to_vec(),
        ) {
            Ok(frame_value) => {
                let frame_value = frame_value
                    .with_index(index)
                    .with_serial(context.current_serial);
                context.send_frame(frame_value)
            }
            Err(err) => {
                context.send_error(err);
                false
            }
        }
    }

    unsafe extern "C" fn poll_seek_requests(
        context: *mut c_void,
        out_request: *mut CMftSeekRequest,
    ) -> i32 {
        if context.is_null() {
            return SEEK_ACTION_STOP;
        }
        let context = unsafe { &mut *(context as *mut DecodeContext) };
        if context.is_closed() {
            return SEEK_ACTION_STOP;
        }
        if !context.seek_rx.has_changed().unwrap_or(false) {
            return SEEK_ACTION_CONTINUE;
        }
        let Some(info) = *context.seek_rx.borrow_and_update() else {
            return SEEK_ACTION_CONTINUE;
        };
        context.current_serial = context.serial.load(Ordering::SeqCst);
        match compute_seek_plan(info, context.fps) {
            Ok(plan) => {
                context.apply_drop(plan.drop_until);
                if !out_request.is_null() {
                    unsafe { *out_request = plan.request };
                }
                SEEK_ACTION_SEEK
            }
            Err(err) => {
                context.seek_error = Some(err);
                SEEK_ACTION_STOP
            }
        }
    }

    #[derive(Clone, Copy)]
    enum DropUntil {
        Frame(u64),
        Timestamp(Duration),
    }

    #[derive(Clone, Copy)]
    struct SeekPlan {
        request: CMftSeekRequest,
        drop_until: Option<DropUntil>,
    }

    fn compute_seek_plan(info: SeekInfo, fps: Option<f64>) -> DecoderResult<SeekPlan> {
        match info {
            SeekInfo::Frame { frame, mode } => {
                let fps = fps.ok_or_else(|| {
                    DecoderError::configuration(
                        "mft backend requires frame rate metadata to seek by frame",
                    )
                })?;
                if !(fps.is_finite() && fps > 0.0) {
                    return Err(DecoderError::configuration(
                        "mft backend requires frame rate metadata to seek by frame",
                    ));
                }
                let seconds = frame as f64 / fps;
                if !seconds.is_finite() || seconds.is_sign_negative() {
                    return Err(DecoderError::configuration("invalid seek timestamp"));
                }
                Ok(SeekPlan {
                    request: CMftSeekRequest {
                        position_seconds: seconds,
                        start_frame: frame,
                    },
                    drop_until: match mode {
                        SeekMode::Fast => None,
                        SeekMode::Accurate => Some(DropUntil::Frame(frame)),
                    },
                })
            }
            SeekInfo::Time { position, mode } => {
                let fps = fps.ok_or_else(|| {
                    DecoderError::configuration(
                        "mft backend requires frame rate metadata to seek by time",
                    )
                })?;
                if !(fps.is_finite() && fps > 0.0) {
                    return Err(DecoderError::configuration(
                        "mft backend requires frame rate metadata to seek by time",
                    ));
                }
                let seconds = position.as_secs_f64();
                if !seconds.is_finite() || seconds.is_sign_negative() {
                    return Err(DecoderError::configuration("invalid seek timestamp"));
                }
                let raw_frame = seconds * fps;
                if !raw_frame.is_finite() || raw_frame.is_sign_negative() {
                    return Err(DecoderError::configuration("invalid seek timestamp"));
                }
                let frame = match mode {
                    SeekMode::Fast => raw_frame.round(),
                    SeekMode::Accurate => raw_frame.floor(),
                };
                if frame < 0.0 || frame > u64::MAX as f64 {
                    return Err(DecoderError::configuration("seek frame is out of range"));
                }
                Ok(SeekPlan {
                    request: CMftSeekRequest {
                        position_seconds: seconds,
                        start_frame: frame as u64,
                    },
                    drop_until: match mode {
                        SeekMode::Fast => None,
                        SeekMode::Accurate => Some(DropUntil::Timestamp(position)),
                    },
                })
            }
        }
    }

    fn index_from_pts(pts: Duration, fps: Option<f64>) -> Option<u64> {
        let fps = fps?;
        if !(fps.is_finite() && fps > 0.0) {
            return None;
        }
        let seconds = pts.as_secs_f64();
        if !seconds.is_finite() || seconds.is_sign_negative() {
            return None;
        }
        let index = (seconds * fps).round();
        if index.is_finite() && index >= 0.0 && index <= u64::MAX as f64 {
            Some(index as u64)
        } else {
            None
        }
    }
}

#[cfg(not(target_os = "windows"))]
mod platform {
    use crate::{
        DecoderController, DecoderError, DecoderProvider, DecoderResult, DynDecoderProvider,
        FrameStream,
    };
    use std::path::Path;

    pub struct MftProvider;

    impl MftProvider {
        pub fn open<P: AsRef<Path>>(
            _path: P,
            _channel_capacity: Option<usize>,
            _start_frame: Option<u64>,
        ) -> DecoderResult<Self> {
            Err(DecoderError::unsupported("mft"))
        }
    }

    impl DecoderProvider for MftProvider {
        fn open(self: Box<Self>) -> DecoderResult<(DecoderController, FrameStream)> {
            Err(DecoderError::unsupported("mft"))
        }
    }
}

pub use platform::MftProvider;
