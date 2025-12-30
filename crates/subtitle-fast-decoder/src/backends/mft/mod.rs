#[cfg(all(target_os = "windows", feature = "backend-mft"))]
use crate::core::{
    DecoderController, FrameError, FrameResult, FrameStream, FrameStreamProvider,
    SeekInfo, SeekReceiver,
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
        timestamp_seconds: f64,
        frame_index: u64,
    }

    type CMftFrameCallback = unsafe extern "C" fn(*const CMftFrame, *mut c_void) -> bool;

    #[allow(improper_ctypes)]
    unsafe extern "C" {
        fn mft_probe_total_frames(path: *const c_char, result: *mut CMftProbeResult) -> bool;
        fn mft_decode(
            path: *const c_char,
            has_start_frame: bool,
            start_frame: u64,
            callback: CMftFrameCallback,
            context: *mut c_void,
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

    impl MftProvider {
    }

    impl FrameStreamProvider for MftProvider {
        fn new(config: &crate::config::Configuration) -> FrameResult<Self> {
            let path = config.input.as_ref().ok_or_else(|| {
                FrameError::configuration("MFT backend requires SUBFAST_INPUT to be set")
            })?;
            if !path.exists() {
                return Err(FrameError::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("input file {} does not exist", path.display()),
                )));
            }
            let metadata = probe_video_metadata(path)?;
            let capacity = config.channel_capacity.map(|n| n.get()).unwrap_or(DEFAULT_CHANNEL_CAPACITY).max(1);
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

        fn open(self: Box<Self>) -> (DecoderController, FrameStream) {
            let provider = *self;
            let capacity = provider.channel_capacity;
            let start_frame = provider.start_frame;
            let (controller, seek_rx) = DecoderController::new();
            let stream = spawn_stream_from_channel(capacity, move |tx| {
                if let Err(err) =
                    decode_mft(provider.input.clone(), tx.clone(), start_frame, seek_rx)
                {
                    let _ = tx.blocking_send(Err(err));
                }
            });
            (controller, stream)
        }
    }

    fn decode_mft(
        path: PathBuf,
        tx: Sender<FrameResult<VideoFrame>>,
        start_frame: Option<u64>,
        seek_rx: SeekReceiver,
    ) -> FrameResult<()> {
        let c_path = cstring_from_path(&path)?;
        let mut context = DecodeContext::new(tx, seek_rx);
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
                &mut error_ptr,
            )
        };
        let bridge_error = take_bridge_string(error_ptr);
        if !ok {
            let message = bridge_error.unwrap_or_else(|| "decode failed".to_string());
            return Err(FrameError::backend_failure(BACKEND_NAME, message));
        }
        if let Some(message) = bridge_error {
            if !message.is_empty() {
                return Err(FrameError::backend_failure(BACKEND_NAME, message));
            }
        }
        Ok(())
    }

    fn probe_video_metadata(path: &Path) -> FrameResult<crate::core::VideoMetadata> {
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
            return Err(FrameError::backend_failure(BACKEND_NAME, message));
        }
        if let Some(message) = bridge_error {
            if !message.is_empty() {
                return Err(FrameError::backend_failure(BACKEND_NAME, message));
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

    fn cstring_from_path(path: &Path) -> FrameResult<CString> {
        CString::new(path.to_string_lossy().as_bytes()).map_err(|err| {
            FrameError::backend_failure(BACKEND_NAME, format!("invalid path encoding: {err}"))
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
        tx: Sender<FrameResult<VideoFrame>>,
        seek_rx: SeekReceiver,
    }

    impl DecodeContext {
        fn new(tx: Sender<FrameResult<VideoFrame>>, seek_rx: SeekReceiver) -> Self {
            Self { tx, seek_rx }
        }

        fn send_frame(&self, frame: VideoFrame) -> bool {
            self.tx.blocking_send(Ok(frame)).is_ok()
        }

        fn send_error(&self, error: FrameError) {
            let _ = self.tx.blocking_send(Err(error));
        }

        fn drain_seek_requests(&mut self) {
            if !self.seek_rx.has_changed().unwrap_or(false) {
                return;
            }
            if let Some(info) = *self.seek_rx.borrow_and_update() {
                handle_seek_request(info);
            }
        }
    }

    unsafe extern "C" fn handle_frame(frame: *const CMftFrame, context: *mut c_void) -> bool {
        if frame.is_null() || context.is_null() {
            return false;
        }
        let frame = unsafe { &*frame };
        let context = unsafe { &mut *(context as *mut DecodeContext) };
        context.drain_seek_requests();
        if frame.y_data.is_null() || frame.uv_data.is_null() {
            context.send_error(FrameError::backend_failure(
                BACKEND_NAME,
                "NV12 plane pointer is null",
            ));
            return false;
        }
        let y_data = unsafe { slice::from_raw_parts(frame.y_data, frame.y_len) };
        let uv_data = unsafe { slice::from_raw_parts(frame.uv_data, frame.uv_len) };
        let timestamp = if frame.timestamp_seconds.is_sign_negative() {
            None
        } else {
            Some(Duration::from_secs_f64(frame.timestamp_seconds))
        };
        match VideoFrame::from_nv12_owned(
            frame.width,
            frame.height,
            frame.y_stride,
            frame.uv_stride,
            timestamp,
            y_data.to_vec(),
            uv_data.to_vec(),
        ) {
            Ok(frame_value) => {
                let frame_value = frame_value.with_frame_index(Some(frame.frame_index));
                context.send_frame(frame_value)
            }
            Err(err) => {
                context.send_error(err);
                false
            }
        }
    }

    fn handle_seek_request(_info: SeekInfo) {
        todo!("mft seek handling is not implemented yet");
    }
}

#[cfg(not(target_os = "windows"))]
mod platform {
    use crate::{
        DecoderController, DynFrameProvider, FrameError, FrameResult, FrameStream,
        FrameStreamProvider,
    };
    use std::path::Path;

    pub struct MftProvider;

    impl MftProvider {
        pub fn open<P: AsRef<Path>>(
            _path: P,
            _channel_capacity: Option<usize>,
            _start_frame: Option<u64>,
        ) -> FrameResult<Self> {
            Err(FrameError::unsupported("mft"))
        }
    }

    impl FrameStreamProvider for MftProvider {
        fn open(self: Box<Self>) -> (DecoderController, FrameStream) {
            panic!("MFT backend is only available on Windows builds");
        }
    }
}

pub use platform::MftProvider;
