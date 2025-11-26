#[cfg(all(target_os = "macos", feature = "backend-videotoolbox"))]
use crate::core::{
    DynYPlaneProvider, YPlaneError, YPlaneResult, YPlaneStream, YPlaneStreamProvider,
};

#[cfg(target_os = "macos")]
use crate::core::{YPlaneFrame, spawn_stream_from_channel};

#[cfg(target_os = "macos")]
#[allow(unexpected_cfgs)]
mod platform {
    use super::*;

    use mp4::{Mp4Reader, TrackType};
    use std::ffi::{CStr, CString, c_char};
    use std::fs::File;
    use std::io::BufReader;
    use std::os::raw::c_void;
    use std::path::{Path, PathBuf};
    use std::ptr;
    use std::slice;
    use std::time::Duration;
    use tokio::sync::mpsc;

    #[repr(C)]
    struct CVTProbeResult {
        has_value: bool,
        value: u64,
        error: *mut c_char,
    }

    #[repr(C)]
    struct CVTFrame {
        data: *const u8,
        data_len: usize,
        width: u32,
        height: u32,
        stride: usize,
        timestamp_seconds: f64,
        frame_index: u64,
    }

    type CVTFrameCallback = unsafe extern "C" fn(*const CVTFrame, *mut c_void) -> bool;

    #[allow(improper_ctypes)]
    unsafe extern "C" {
        fn videotoolbox_probe_total_frames(
            path: *const c_char,
            result: *mut CVTProbeResult,
        ) -> bool;
        fn videotoolbox_decode(
            path: *const c_char,
            callback: CVTFrameCallback,
            context: *mut c_void,
            out_error: *mut *mut c_char,
        ) -> bool;
        fn videotoolbox_string_free(ptr: *mut c_char);
    }

    const DEFAULT_CHANNEL_CAPACITY: usize = 16;

    pub struct VideoToolboxProvider {
        input: PathBuf,
        total_frames: Option<u64>,
        channel_capacity: usize,
    }

    impl VideoToolboxProvider {
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

    fn probe_total_frames(path: &Path) -> YPlaneResult<Option<u64>> {
        match probe_total_frames_videotoolbox(path) {
            Ok(result) => Ok(result),
            Err(vt_err) => match probe_total_frames_mp4(path) {
                Ok(Some(value)) => Ok(Some(value)),
                Ok(None) => Err(vt_err),
                Err(mp4_err) => Err(YPlaneError::backend_failure(
                    "videotoolbox",
                    format!("{vt_err}; mp4 fallback failed: {mp4_err}"),
                )),
            },
        }
    }

    fn probe_total_frames_mp4(path: &Path) -> YPlaneResult<Option<u64>> {
        let file = File::open(path)?;
        let size = file.metadata()?.len();
        let reader = BufReader::new(file);
        let reader = match Mp4Reader::read_header(reader, size) {
            Ok(reader) => reader,
            Err(_) => return Ok(None),
        };

        let track_id = match reader
            .tracks()
            .iter()
            .find(|(_, track)| matches!(track.track_type(), Ok(TrackType::Video)))
        {
            Some((&id, _)) => id,
            None => return Ok(None),
        };

        let count = reader.sample_count(track_id).map_err(|err| {
            YPlaneError::backend_failure(
                "videotoolbox",
                format!("failed to query MP4 sample count: {err}"),
            )
        })?;

        if count == 0 {
            return Ok(None);
        }

        Ok(Some(count as u64))
    }

    fn probe_total_frames_videotoolbox(path: &Path) -> YPlaneResult<Option<u64>> {
        let c_path = cstring_from_path(path)?;
        let mut result = CVTProbeResult {
            has_value: false,
            value: 0,
            error: ptr::null_mut(),
        };
        let ok = unsafe { videotoolbox_probe_total_frames(c_path.as_ptr(), &mut result) };
        let error = take_bridge_string(result.error);
        if !ok {
            let message = error.unwrap_or_else(|| "videotoolbox probe failed".to_string());
            return Err(YPlaneError::backend_failure("videotoolbox", message));
        }
        if let Some(message) = error
            && !message.is_empty()
        {
            return Err(YPlaneError::backend_failure("videotoolbox", message));
        }
        if result.has_value {
            Ok(Some(result.value))
        } else {
            Ok(None)
        }
    }

    fn cstring_from_path(path: &Path) -> YPlaneResult<CString> {
        CString::new(path.to_string_lossy().as_bytes()).map_err(|err| {
            YPlaneError::backend_failure("videotoolbox", format!("invalid path encoding: {err}"))
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

    impl YPlaneStreamProvider for VideoToolboxProvider {
        fn total_frames(&self) -> Option<u64> {
            self.total_frames
        }

        fn into_stream(self: Box<Self>) -> YPlaneStream {
            let path = self.input.clone();
            let capacity = self.channel_capacity;
            spawn_stream_from_channel(capacity, move |tx| {
                if let Err(err) = decode_videotoolbox(path.clone(), tx.clone()) {
                    let _ = tx.blocking_send(Err(err));
                }
            })
        }
    }

    fn decode_videotoolbox(
        path: PathBuf,
        tx: mpsc::Sender<YPlaneResult<YPlaneFrame>>,
    ) -> YPlaneResult<()> {
        let c_path = cstring_from_path(&path)?;
        let mut context = Box::new(DecodeContext::new(tx));
        let mut error_ptr: *mut c_char = ptr::null_mut();
        let ok = unsafe {
            videotoolbox_decode(
                c_path.as_ptr(),
                frame_callback,
                (&mut *context) as *mut DecodeContext as *mut c_void,
                &mut error_ptr,
            )
        };
        drop(context);

        let error = take_bridge_string(error_ptr);
        if !ok {
            let message = error.unwrap_or_else(|| "videotoolbox decode failed".to_string());
            return Err(YPlaneError::backend_failure("videotoolbox", message));
        }
        if let Some(message) = error
            && !message.is_empty()
        {
            return Err(YPlaneError::backend_failure("videotoolbox", message));
        }
        Ok(())
    }

    struct DecodeContext {
        sender: mpsc::Sender<YPlaneResult<YPlaneFrame>>,
    }

    impl DecodeContext {
        fn new(sender: mpsc::Sender<YPlaneResult<YPlaneFrame>>) -> Self {
            Self { sender }
        }

        fn send(&self, message: YPlaneResult<YPlaneFrame>) -> bool {
            self.sender.blocking_send(message).is_ok()
        }
    }

    unsafe extern "C" fn frame_callback(frame: *const CVTFrame, ctx: *mut c_void) -> bool {
        if frame.is_null() || ctx.is_null() {
            return false;
        }
        let frame = unsafe { &*frame };
        let context = unsafe { &*(ctx as *const DecodeContext) };

        if frame.data.is_null() {
            let _ = context.send(Err(YPlaneError::backend_failure(
                "videotoolbox",
                "frame missing pixel data",
            )));
            return false;
        }

        let data = unsafe { slice::from_raw_parts(frame.data, frame.data_len) };
        let buffer = data.to_vec();

        let timestamp = if frame.timestamp_seconds.is_finite() && frame.timestamp_seconds >= 0.0 {
            Some(Duration::from_secs_f64(frame.timestamp_seconds))
        } else {
            None
        };

        let y_frame = match YPlaneFrame::from_owned(
            frame.width,
            frame.height,
            frame.stride,
            timestamp,
            buffer,
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

    pub fn boxed_videotoolbox<P: AsRef<Path>>(
        path: P,
        channel_capacity: Option<usize>,
    ) -> YPlaneResult<DynYPlaneProvider> {
        VideoToolboxProvider::open(path, channel_capacity)
            .map(|provider| Box::new(provider) as DynYPlaneProvider)
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::*;
    use std::path::Path;

    pub struct VideoToolboxProvider;

    impl VideoToolboxProvider {
        pub fn open<P: AsRef<Path>>(_path: P) -> YPlaneResult<Self> {
            Err(YPlaneError::unsupported("videotoolbox"))
        }
    }

    impl YPlaneStreamProvider for VideoToolboxProvider {
        fn into_stream(self: Box<Self>) -> YPlaneStream {
            panic!("VideoToolbox backend is only available on macOS builds");
        }
    }

    pub fn boxed_videotoolbox<P: AsRef<Path>>(
        _path: P,
        _channel_capacity: Option<usize>,
    ) -> YPlaneResult<DynYPlaneProvider> {
        Err(YPlaneError::unsupported("videotoolbox"))
    }
}

pub use platform::{VideoToolboxProvider, boxed_videotoolbox};
