#[cfg(all(target_os = "macos", feature = "backend-videotoolbox"))]
use crate::core::{DynFrameProvider, FrameError, FrameResult, FrameStream, FrameStreamProvider};

use crate::config::OutputFormat;
#[cfg(target_os = "macos")]
use crate::core::{VideoFrame, spawn_stream_from_channel};

#[cfg(target_os = "macos")]
#[allow(unexpected_cfgs)]
mod platform {
    use super::*;

    use std::ffi::{CStr, CString, c_char, c_void};
    use std::path::{Path, PathBuf};
    use std::ptr;
    use std::slice;
    use std::time::Duration;
    use tokio::sync::mpsc;

    #[repr(C)]
    struct CVTProbeResult {
        has_value: bool,
        value: u64,
        duration_seconds: f64,
        fps: f64,
        width: u32,
        height: u32,
        error: *mut c_char,
    }

    #[repr(C)]
    struct CVTFrame {
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

    type CVTFrameCallback = unsafe extern "C" fn(*const CVTFrame, *mut c_void) -> bool;

    #[repr(C)]
    struct CVTHandleFrame {
        pixel_buffer: *mut c_void,
        pixel_format: u32,
        width: u32,
        height: u32,
        timestamp_seconds: f64,
        frame_index: u64,
    }

    type CVTHandleFrameCallback = unsafe extern "C" fn(*const CVTHandleFrame, *mut c_void) -> bool;

    #[allow(improper_ctypes)]
    unsafe extern "C" {
        fn videotoolbox_probe_total_frames(
            path: *const c_char,
            result: *mut CVTProbeResult,
        ) -> bool;
        fn videotoolbox_decode(
            path: *const c_char,
            has_start_frame: bool,
            start_frame: u64,
            callback: CVTFrameCallback,
            context: *mut c_void,
            out_error: *mut *mut c_char,
        ) -> bool;
        fn videotoolbox_decode_handle(
            path: *const c_char,
            has_start_frame: bool,
            start_frame: u64,
            callback: CVTHandleFrameCallback,
            context: *mut c_void,
            out_error: *mut *mut c_char,
        ) -> bool;
        fn videotoolbox_string_free(ptr: *mut c_char);
    }

    const DEFAULT_CHANNEL_CAPACITY: usize = 16;

    pub struct VideoToolboxProvider {
        input: PathBuf,
        metadata: crate::core::VideoMetadata,
        channel_capacity: usize,
        output_format: OutputFormat,
        start_frame: Option<u64>,
    }

    impl VideoToolboxProvider {
        pub fn open<P: AsRef<Path>>(
            path: P,
            channel_capacity: Option<usize>,
            output_format: OutputFormat,
            start_frame: Option<u64>,
        ) -> FrameResult<Self> {
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
                output_format,
                start_frame,
            })
        }
    }

    fn probe_video_metadata(path: &Path) -> FrameResult<crate::core::VideoMetadata> {
        probe_metadata_videotoolbox(path)
    }

    fn probe_metadata_videotoolbox(path: &Path) -> FrameResult<crate::core::VideoMetadata> {
        use crate::core::VideoMetadata;

        let c_path = cstring_from_path(path)?;
        let mut result = CVTProbeResult {
            has_value: false,
            value: 0,
            duration_seconds: f64::NAN,
            fps: f64::NAN,
            width: 0,
            height: 0,
            error: ptr::null_mut(),
        };
        let ok = unsafe { videotoolbox_probe_total_frames(c_path.as_ptr(), &mut result) };
        let error = take_bridge_string(result.error);
        if !ok {
            let message = error.unwrap_or_else(|| "videotoolbox probe failed".to_string());
            return Err(FrameError::backend_failure("videotoolbox", message));
        }
        if let Some(message) = error
            && !message.is_empty()
        {
            return Err(FrameError::backend_failure("videotoolbox", message));
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
            FrameError::backend_failure("videotoolbox", format!("invalid path encoding: {err}"))
        })
    }

    fn take_bridge_string(ptr: *mut c_char) -> Option<String> {
        if ptr.is_null() {
            return None;
        }
        let message = unsafe { CStr::from_ptr(ptr).to_string_lossy().into_owned() };
        unsafe { videotoolbox_string_free(ptr) };
        Some(message)
    }

    unsafe extern "C" {
        fn CFRelease(cf: *const c_void);
    }

    unsafe extern "C" fn release_native_handle(handle: *mut c_void) {
        if !handle.is_null() {
            unsafe { CFRelease(handle as *const c_void) };
        }
    }

    impl FrameStreamProvider for VideoToolboxProvider {
        fn metadata(&self) -> crate::core::VideoMetadata {
            self.metadata
        }

        fn into_stream(self: Box<Self>) -> FrameStream {
            let path = self.input.clone();
            let capacity = self.channel_capacity;
            let output_format = self.output_format;
            let start_frame = self.start_frame;
            spawn_stream_from_channel(capacity, move |tx| {
                let result = match output_format {
                    OutputFormat::Nv12 => {
                        decode_videotoolbox_nv12(path.clone(), tx.clone(), start_frame)
                    }
                    OutputFormat::CVPixelBuffer => {
                        decode_videotoolbox_handle(path.clone(), tx.clone(), start_frame)
                    }
                };
                if let Err(err) = result {
                    let _ = tx.blocking_send(Err(err));
                }
            })
        }
    }

    fn decode_videotoolbox_nv12(
        path: PathBuf,
        tx: mpsc::Sender<FrameResult<VideoFrame>>,
        start_frame: Option<u64>,
    ) -> FrameResult<()> {
        let c_path = cstring_from_path(&path)?;
        let mut context = Box::new(DecodeContext::new(tx));
        let mut error_ptr: *mut c_char = ptr::null_mut();
        let (has_start_frame, start_frame) = match start_frame {
            Some(value) => (true, value),
            None => (false, 0),
        };
        let ok = unsafe {
            videotoolbox_decode(
                c_path.as_ptr(),
                has_start_frame,
                start_frame,
                frame_callback_nv12,
                (&mut *context) as *mut DecodeContext as *mut c_void,
                &mut error_ptr,
            )
        };
        drop(context);

        let error = take_bridge_string(error_ptr);
        if !ok {
            let message = error.unwrap_or_else(|| "videotoolbox decode failed".to_string());
            return Err(FrameError::backend_failure("videotoolbox", message));
        }
        if let Some(message) = error
            && !message.is_empty()
        {
            return Err(FrameError::backend_failure("videotoolbox", message));
        }
        Ok(())
    }

    fn decode_videotoolbox_handle(
        path: PathBuf,
        tx: mpsc::Sender<FrameResult<VideoFrame>>,
        start_frame: Option<u64>,
    ) -> FrameResult<()> {
        let c_path = cstring_from_path(&path)?;
        let mut context = Box::new(DecodeContext::new(tx));
        let mut error_ptr: *mut c_char = ptr::null_mut();
        let (has_start_frame, start_frame) = match start_frame {
            Some(value) => (true, value),
            None => (false, 0),
        };
        let ok = unsafe {
            videotoolbox_decode_handle(
                c_path.as_ptr(),
                has_start_frame,
                start_frame,
                frame_callback_handle,
                (&mut *context) as *mut DecodeContext as *mut c_void,
                &mut error_ptr,
            )
        };
        drop(context);

        let error = take_bridge_string(error_ptr);
        if !ok {
            let message = error.unwrap_or_else(|| "videotoolbox handle decode failed".to_string());
            return Err(FrameError::backend_failure("videotoolbox", message));
        }
        if let Some(message) = error
            && !message.is_empty()
        {
            return Err(FrameError::backend_failure("videotoolbox", message));
        }
        Ok(())
    }

    struct DecodeContext {
        sender: mpsc::Sender<FrameResult<VideoFrame>>,
    }

    impl DecodeContext {
        fn new(sender: mpsc::Sender<FrameResult<VideoFrame>>) -> Self {
            Self { sender }
        }

        fn send(&self, message: FrameResult<VideoFrame>) -> bool {
            self.sender.blocking_send(message).is_ok()
        }
    }

    unsafe extern "C" fn frame_callback_nv12(frame: *const CVTFrame, ctx: *mut c_void) -> bool {
        if frame.is_null() || ctx.is_null() {
            return false;
        }
        let frame = unsafe { &*frame };
        let context = unsafe { &*(ctx as *const DecodeContext) };

        if frame.y_data.is_null() || frame.uv_data.is_null() {
            let _ = context.send(Err(FrameError::backend_failure(
                "videotoolbox",
                "frame missing pixel data",
            )));
            return false;
        }

        let y_data = unsafe { slice::from_raw_parts(frame.y_data, frame.y_len) };
        let uv_data = unsafe { slice::from_raw_parts(frame.uv_data, frame.uv_len) };

        let timestamp = if frame.timestamp_seconds.is_finite() && frame.timestamp_seconds >= 0.0 {
            Some(Duration::from_secs_f64(frame.timestamp_seconds))
        } else {
            None
        };

        let y_frame = match VideoFrame::from_nv12_owned(
            frame.width,
            frame.height,
            frame.y_stride,
            frame.uv_stride,
            timestamp,
            y_data.to_vec(),
            uv_data.to_vec(),
        ) {
            Ok(value) => value.with_frame_index(Some(frame.frame_index)),
            Err(err) => {
                let _ = context.send(Err(err));
                return false;
            }
        };

        if !context.send(Ok(y_frame)) {
            return false;
        }
        true
    }

    unsafe extern "C" fn frame_callback_handle(
        frame: *const CVTHandleFrame,
        ctx: *mut c_void,
    ) -> bool {
        if frame.is_null() {
            return false;
        }
        let frame = unsafe { &*frame };
        if ctx.is_null() {
            if !frame.pixel_buffer.is_null() {
                unsafe { release_native_handle(frame.pixel_buffer) };
            }
            return false;
        }
        let context = unsafe { &*(ctx as *const DecodeContext) };

        if frame.pixel_buffer.is_null() {
            let _ = context.send(Err(FrameError::backend_failure(
                "videotoolbox",
                "native frame missing pixel buffer handle",
            )));
            return false;
        }

        let timestamp = if frame.timestamp_seconds.is_finite() && frame.timestamp_seconds >= 0.0 {
            Some(Duration::from_secs_f64(frame.timestamp_seconds))
        } else {
            None
        };

        let native_frame = match VideoFrame::from_native_handle(
            frame.width,
            frame.height,
            timestamp,
            Some(frame.frame_index),
            "videotoolbox",
            frame.pixel_format,
            frame.pixel_buffer,
            release_native_handle,
        ) {
            Ok(value) => value,
            Err(err) => {
                unsafe { release_native_handle(frame.pixel_buffer) };
                let _ = context.send(Err(err));
                return false;
            }
        };

        if !context.send(Ok(native_frame)) {
            return false;
        }
        true
    }

    pub fn boxed_videotoolbox<P: AsRef<Path>>(
        path: P,
        channel_capacity: Option<usize>,
        output_format: OutputFormat,
        start_frame: Option<u64>,
    ) -> FrameResult<DynFrameProvider> {
        VideoToolboxProvider::open(path, channel_capacity, output_format, start_frame)
            .map(|provider| Box::new(provider) as DynFrameProvider)
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::*;
    use std::path::Path;

    pub struct VideoToolboxProvider;

    impl VideoToolboxProvider {
        pub fn open<P: AsRef<Path>>(
            _path: P,
            _channel_capacity: Option<usize>,
            _output_format: OutputFormat,
            _start_frame: Option<u64>,
        ) -> FrameResult<Self> {
            Err(FrameError::unsupported("videotoolbox"))
        }
    }

    impl FrameStreamProvider for VideoToolboxProvider {
        fn into_stream(self: Box<Self>) -> FrameStream {
            panic!("VideoToolbox backend is only available on macOS builds");
        }
    }

    pub fn boxed_videotoolbox<P: AsRef<Path>>(
        _path: P,
        _channel_capacity: Option<usize>,
        _output_format: OutputFormat,
        _start_frame: Option<u64>,
    ) -> FrameResult<DynFrameProvider> {
        Err(FrameError::unsupported("videotoolbox"))
    }
}

pub use platform::{VideoToolboxProvider, boxed_videotoolbox};
