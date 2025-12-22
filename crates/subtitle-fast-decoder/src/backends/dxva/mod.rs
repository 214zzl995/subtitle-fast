#[cfg(all(target_os = "windows", feature = "backend-dxva"))]
use crate::core::{DynFrameProvider, FrameError, FrameResult, FrameStream, FrameStreamProvider};

#[cfg(all(target_os = "windows", feature = "backend-dxva"))]
use crate::core::{VideoFrame, spawn_stream_from_channel};

#[cfg(all(target_os = "windows", feature = "backend-dxva"))]
#[allow(unexpected_cfgs)]
mod platform {
    use super::*;
    use std::ffi::{CStr, CString, c_char, c_void};
    use std::path::{Path, PathBuf};
    use std::ptr;
    use std::slice;
    use std::time::Duration;
    use tokio::sync::mpsc::Sender;

    const BACKEND_NAME: &str = "dxva";
    const DEFAULT_CHANNEL_CAPACITY: usize = 16;

    #[repr(C)]
    struct CDxvaProbeResult {
        has_value: bool,
        value: u64,
        duration_seconds: f64,
        fps: f64,
        width: u32,
        height: u32,
        error: *mut c_char,
    }

    #[repr(C)]
    struct CDxvaFrame {
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

    type CDxvaFrameCallback = unsafe extern "C" fn(*const CDxvaFrame, *mut c_void) -> bool;

    #[allow(improper_ctypes)]
    unsafe extern "C" {
        fn dxva_probe_total_frames(path: *const c_char, result: *mut CDxvaProbeResult) -> bool;
        fn dxva_decode(
            path: *const c_char,
            callback: CDxvaFrameCallback,
            context: *mut c_void,
            out_error: *mut *mut c_char,
        ) -> bool;
        fn dxva_string_free(ptr: *mut c_char);
    }

    pub struct DxvaProvider {
        input: PathBuf,
        metadata: crate::core::VideoMetadata,
        channel_capacity: usize,
    }

    impl DxvaProvider {
        pub fn open<P: AsRef<Path>>(path: P, channel_capacity: Option<usize>) -> FrameResult<Self> {
            let path = path.as_ref();
            if !path.exists() {
                return Err(FrameError::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("input file {} does not exist", path.display()),
                )));
            }
            let metadata = probe_video_metadata(path)?;
            let capacity = channel_capacity.unwrap_or(DEFAULT_CHANNEL_CAPACITY).max(1);
            Ok(Self {
                input: path.to_path_buf(),
                metadata,
                channel_capacity: capacity,
            })
        }
    }

    impl FrameStreamProvider for DxvaProvider {
        fn metadata(&self) -> crate::core::VideoMetadata {
            self.metadata
        }

        fn into_stream(self: Box<Self>) -> FrameStream {
            let provider = *self;
            let capacity = provider.channel_capacity;
            spawn_stream_from_channel(capacity, move |tx| {
                if let Err(err) = decode_dxva(provider.input.clone(), tx.clone()) {
                    let _ = tx.blocking_send(Err(err));
                }
            })
        }
    }

    fn decode_dxva(path: PathBuf, tx: Sender<FrameResult<VideoFrame>>) -> FrameResult<()> {
        let c_path = cstring_from_path(&path)?;
        let mut context = DecodeContext::new(tx);
        let mut error_ptr: *mut c_char = ptr::null_mut();
        let ok = unsafe {
            dxva_decode(
                c_path.as_ptr(),
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
        let mut result = CDxvaProbeResult {
            has_value: false,
            value: 0,
            error: ptr::null_mut(),
        };
        let ok = unsafe { dxva_probe_total_frames(c_path.as_ptr(), &mut result) };
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
        unsafe { dxva_string_free(ptr) };
        Some(message)
    }

    struct DecodeContext {
        tx: Sender<FrameResult<VideoFrame>>,
    }

    impl DecodeContext {
        fn new(tx: Sender<FrameResult<VideoFrame>>) -> Self {
            Self { tx }
        }

        fn send_frame(&self, frame: VideoFrame) -> bool {
            self.tx.blocking_send(Ok(frame)).is_ok()
        }

        fn send_error(&self, error: FrameError) {
            let _ = self.tx.blocking_send(Err(error));
        }
    }

    unsafe extern "C" fn handle_frame(frame: *const CDxvaFrame, context: *mut c_void) -> bool {
        if frame.is_null() || context.is_null() {
            return false;
        }
        let frame = unsafe { &*frame };
        let context = unsafe { &*(context as *mut DecodeContext) };
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

    pub fn boxed_dxva<P: AsRef<Path>>(
        path: P,
        channel_capacity: Option<usize>,
    ) -> FrameResult<DynFrameProvider> {
        Ok(Box::new(DxvaProvider::open(path, channel_capacity)?))
    }
}

#[cfg(not(target_os = "windows"))]
mod platform {
    use crate::{DynFrameProvider, FrameError, FrameResult, FrameStream, FrameStreamProvider};
    use std::path::Path;

    pub struct DxvaProvider;

    impl DxvaProvider {
        pub fn open<P: AsRef<Path>>(
            _path: P,
            _channel_capacity: Option<usize>,
        ) -> FrameResult<Self> {
            Err(FrameError::unsupported("dxva"))
        }
    }

    impl FrameStreamProvider for DxvaProvider {
        fn into_stream(self: Box<Self>) -> FrameStream {
            panic!("DXVA backend is only available on Windows builds");
        }
    }

    pub fn boxed_dxva<P: AsRef<Path>>(
        _path: P,
        _channel_capacity: Option<usize>,
    ) -> FrameResult<DynFrameProvider> {
        Err(FrameError::unsupported("dxva"))
    }
}

pub use platform::{DxvaProvider, boxed_dxva};
