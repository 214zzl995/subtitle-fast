#[cfg(all(target_os = "windows", feature = "backend-mft"))]
use crate::core::{
    DynYPlaneProvider, YPlaneError, YPlaneResult, YPlaneStream, YPlaneStreamProvider,
};

#[cfg(all(target_os = "windows", feature = "backend-mft"))]
use crate::core::{YPlaneFrame, spawn_stream_from_channel};

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
        error: *mut c_char,
    }

    #[repr(C)]
    struct CMftFrame {
        data: *const u8,
        data_len: usize,
        width: u32,
        height: u32,
        stride: usize,
        timestamp_seconds: f64,
        frame_index: u64,
    }

    type CMftFrameCallback = unsafe extern "C" fn(*const CMftFrame, *mut c_void) -> bool;

    #[allow(improper_ctypes)]
    unsafe extern "C" {
        fn mft_probe_total_frames(path: *const c_char, result: *mut CMftProbeResult) -> bool;
        fn mft_decode(
            path: *const c_char,
            callback: CMftFrameCallback,
            context: *mut c_void,
            out_error: *mut *mut c_char,
        ) -> bool;
        fn mft_string_free(ptr: *mut c_char);
    }

    pub struct MftProvider {
        input: PathBuf,
        total_frames: Option<u64>,
        channel_capacity: usize,
    }

    impl MftProvider {
        pub fn open<P: AsRef<Path>>(
            path: P,
            channel_capacity: Option<usize>,
        ) -> YPlaneResult<Self> {
            let path = path.as_ref();
            if !path.exists() {
                return Err(YPlaneError::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("input file {} does not exist", path.display()),
                )));
            }
            let total_frames = probe_total_frames(path)?;
            let capacity = channel_capacity.unwrap_or(DEFAULT_CHANNEL_CAPACITY).max(1);
            Ok(Self {
                input: path.to_path_buf(),
                total_frames,
                channel_capacity: capacity,
            })
        }
    }

    impl YPlaneStreamProvider for MftProvider {
        fn total_frames(&self) -> Option<u64> {
            self.total_frames
        }

        fn into_stream(self: Box<Self>) -> YPlaneStream {
            let provider = *self;
            let capacity = provider.channel_capacity;
            spawn_stream_from_channel(capacity, move |tx| {
                if let Err(err) = decode_mft(provider.input.clone(), tx.clone()) {
                    let _ = tx.blocking_send(Err(err));
                }
            })
        }
    }

    fn decode_mft(path: PathBuf, tx: Sender<YPlaneResult<YPlaneFrame>>) -> YPlaneResult<()> {
        let c_path = cstring_from_path(&path)?;
        let mut context = DecodeContext::new(tx);
        let mut error_ptr: *mut c_char = ptr::null_mut();
        let ok = unsafe {
            mft_decode(
                c_path.as_ptr(),
                handle_frame,
                &mut context as *mut _ as *mut c_void,
                &mut error_ptr,
            )
        };
        let bridge_error = take_bridge_string(error_ptr);
        if !ok {
            let message = bridge_error.unwrap_or_else(|| "decode failed".to_string());
            return Err(YPlaneError::backend_failure(BACKEND_NAME, message));
        }
        if let Some(message) = bridge_error {
            if !message.is_empty() {
                return Err(YPlaneError::backend_failure(BACKEND_NAME, message));
            }
        }
        Ok(())
    }

    fn probe_total_frames(path: &Path) -> YPlaneResult<Option<u64>> {
        let c_path = cstring_from_path(path)?;
        let mut result = CMftProbeResult {
            has_value: false,
            value: 0,
            error: ptr::null_mut(),
        };
        let ok = unsafe { mft_probe_total_frames(c_path.as_ptr(), &mut result) };
        let bridge_error = take_bridge_string(result.error);
        if !ok {
            let message = bridge_error.unwrap_or_else(|| "probe failed".to_string());
            return Err(YPlaneError::backend_failure(BACKEND_NAME, message));
        }
        if let Some(message) = bridge_error {
            if !message.is_empty() {
                return Err(YPlaneError::backend_failure(BACKEND_NAME, message));
            }
        }
        Ok(if result.has_value {
            Some(result.value)
        } else {
            None
        })
    }

    fn cstring_from_path(path: &Path) -> YPlaneResult<CString> {
        CString::new(path.to_string_lossy().as_bytes()).map_err(|err| {
            YPlaneError::backend_failure(BACKEND_NAME, format!("invalid path encoding: {err}"))
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
        tx: Sender<YPlaneResult<YPlaneFrame>>,
    }

    impl DecodeContext {
        fn new(tx: Sender<YPlaneResult<YPlaneFrame>>) -> Self {
            Self { tx }
        }

        fn send_frame(&self, frame: YPlaneFrame) -> bool {
            self.tx.blocking_send(Ok(frame)).is_ok()
        }

        fn send_error(&self, error: YPlaneError) {
            let _ = self.tx.blocking_send(Err(error));
        }
    }

    unsafe extern "C" fn handle_frame(frame: *const CMftFrame, context: *mut c_void) -> bool {
        if frame.is_null() || context.is_null() {
            return false;
        }
        let frame = unsafe { &*frame };
        let context = unsafe { &*(context as *mut DecodeContext) };
        let expected_bytes = match frame.stride.checked_mul(frame.height as usize) {
            Some(bytes) => bytes,
            None => {
                context.send_error(YPlaneError::backend_failure(
                    BACKEND_NAME,
                    "stride overflow when calculating plane length",
                ));
                return false;
            }
        };
        let data = unsafe { slice::from_raw_parts(frame.data, frame.data_len) };
        if data.len() < expected_bytes {
            context.send_error(YPlaneError::backend_failure(
                BACKEND_NAME,
                format!(
                    "incomplete Y plane: have {} expected {} bytes",
                    data.len(),
                    expected_bytes
                ),
            ));
            return false;
        }
        let mut plane = Vec::with_capacity(expected_bytes);
        plane.extend_from_slice(&data[..expected_bytes]);
        let timestamp = if frame.timestamp_seconds.is_sign_negative() {
            None
        } else {
            Some(Duration::from_secs_f64(frame.timestamp_seconds))
        };
        match YPlaneFrame::from_owned(frame.width, frame.height, frame.stride, timestamp, plane) {
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

    pub fn boxed_mft<P: AsRef<Path>>(
        path: P,
        channel_capacity: Option<usize>,
    ) -> YPlaneResult<DynYPlaneProvider> {
        Ok(Box::new(MftProvider::open(path, channel_capacity)?))
    }
}

#[cfg(not(target_os = "windows"))]
mod platform {
    use crate::{DynYPlaneProvider, YPlaneError, YPlaneResult, YPlaneStream, YPlaneStreamProvider};
    use std::path::Path;

    pub struct MftProvider;

    impl MftProvider {
        pub fn open<P: AsRef<Path>>(
            _path: P,
            _channel_capacity: Option<usize>,
        ) -> YPlaneResult<Self> {
            Err(YPlaneError::unsupported("mft"))
        }
    }

    impl YPlaneStreamProvider for MftProvider {
        fn into_stream(self: Box<Self>) -> YPlaneStream {
            panic!("MFT backend is only available on Windows builds");
        }
    }

    pub fn boxed_mft<P: AsRef<Path>>(
        _path: P,
        _channel_capacity: Option<usize>,
    ) -> YPlaneResult<DynYPlaneProvider> {
        Err(YPlaneError::unsupported("mft"))
    }
}

pub use platform::{MftProvider, boxed_mft};
