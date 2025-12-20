#[cfg(all(target_os = "windows", feature = "backend-dxva"))]
use crate::core::{
    DynYPlaneProvider, PlaneFrame, PlaneStreamHandle, SeekControl, SeekPosition, YPlaneError,
    YPlaneResult, YPlaneStreamProvider, spawn_stream_from_channel,
};

#[cfg(all(target_os = "windows", feature = "backend-dxva"))]
use crate::core::RawFrameFormat as OutputFormat;

#[cfg(all(target_os = "windows", feature = "backend-dxva"))]
use subtitle_fast_types::RawFrame;

#[cfg(all(target_os = "windows", feature = "backend-dxva"))]
use std::sync::mpsc as std_mpsc;

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
        error: *mut c_char,
    }

    #[repr(C)]
    struct CDxvaFrame {
        data: *const u8,
        data_len: usize,
        width: u32,
        height: u32,
        stride: usize,
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

    enum SeekTarget {
        Time(Duration),
        Frame(u64),
    }

    struct SeekRequest {
        target: SeekTarget,
        respond_to: std_mpsc::Sender<YPlaneResult<SeekPosition>>,
    }

    struct DxvaSeeker {
        tx: std_mpsc::Sender<SeekRequest>,
    }

    impl SeekControl for DxvaSeeker {
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

    pub struct DxvaProvider {
        input: PathBuf,
        total_frames: Option<u64>,
        channel_capacity: usize,
        output_format: OutputFormat,
    }

    impl DxvaProvider {
        pub fn open<P: AsRef<Path>>(
            path: P,
            channel_capacity: Option<usize>,
            output_format: OutputFormat,
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
                output_format,
            })
        }
    }

    impl YPlaneStreamProvider for DxvaProvider {
        fn total_frames(&self) -> Option<u64> {
            self.total_frames
        }

        fn into_stream(self: Box<Self>) -> PlaneStreamHandle {
            let provider = *self;
            let capacity = provider.channel_capacity;
            let (seek_tx, seek_rx) = std_mpsc::channel();
            let stream = spawn_stream_from_channel(capacity, move |tx| {
                run_loop(provider.input.clone(), provider.output_format, tx, seek_rx);
            });
            PlaneStreamHandle::new(stream, Box::new(DxvaSeeker { tx: seek_tx }))
        }
    }

    fn run_loop(
        path: PathBuf,
        output_format: OutputFormat,
        tx: Sender<YPlaneResult<PlaneFrame>>,
        mut seek_rx: std_mpsc::Receiver<SeekRequest>,
    ) {
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

            let mut context =
                DecodeContext::new(tx.clone(), output_format, pending.take(), seek_rx);
            let result = decode_dxva(path.clone(), &mut context);
            seek_rx = context.take_seek_rx();
            pending = context.pending.take();

            if let Some(interrupt) = context.interrupt.take() {
                pending = Some(interrupt);
                continue;
            }

            if let Some(pending) = pending.take() {
                let _ = pending.respond_to.send(Err(YPlaneError::configuration(
                    "seek target not reached before end of stream",
                )));
            }

            if let Err(err) = result {
                let _ = tx.blocking_send(Err(err));
            }
            break;
        }
    }

    fn decode_dxva(path: PathBuf, context: &mut DecodeContext) -> YPlaneResult<()> {
        let c_path = cstring_from_path(&path)?;
        let mut error_ptr: *mut c_char = ptr::null_mut();
        let ok = unsafe {
            dxva_decode(
                c_path.as_ptr(),
                handle_frame,
                context as *mut _ as *mut c_void,
                &mut error_ptr,
            )
        };
        let bridge_error = take_bridge_string(error_ptr);
        if !ok {
            if context.interrupt.is_some() {
                return Ok(());
            }
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
        let mut result = CDxvaProbeResult {
            has_value: false,
            value: 0,
            error: ptr::null_mut(),
        };
        let ok = unsafe { dxva_probe_total_frames(c_path.as_ptr(), &mut result) };
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
        unsafe { dxva_string_free(ptr) };
        Some(message)
    }

    struct DecodeContext {
        tx: Sender<YPlaneResult<PlaneFrame>>,
        output_format: OutputFormat,
        pending: Option<SeekRequest>,
        interrupt: Option<SeekRequest>,
        seek_rx: Option<std_mpsc::Receiver<SeekRequest>>,
    }

    impl DecodeContext {
        fn new(
            tx: Sender<YPlaneResult<PlaneFrame>>,
            output_format: OutputFormat,
            pending: Option<SeekRequest>,
            seek_rx: std_mpsc::Receiver<SeekRequest>,
        ) -> Self {
            Self {
                tx,
                output_format,
                pending,
                interrupt: None,
                seek_rx: Some(seek_rx),
            }
        }

        fn take_seek_rx(&mut self) -> std_mpsc::Receiver<SeekRequest> {
            self.seek_rx.take().expect("seek receiver")
        }

        fn poll_seek(&mut self) {
            let Some(seek_rx) = self.seek_rx.as_ref() else {
                return;
            };
            if let Ok(request) = seek_rx.try_recv() {
                if let Some(prev) = self.pending.take() {
                    let _ = prev.respond_to.send(Err(YPlaneError::configuration(
                        "seek superseded by a newer request",
                    )));
                }
                self.interrupt = Some(request);
            }
        }

        fn handle_frame(&mut self, frame: PlaneFrame) -> bool {
            self.poll_seek();
            if self.interrupt.is_some() {
                return false;
            }
            if let Some(pending) = &self.pending {
                if frame_matches(&frame, &pending.target) {
                    let position = SeekPosition {
                        timestamp: frame.timestamp(),
                        frame_index: frame.frame_index(),
                    };
                    let _ = pending.respond_to.send(Ok(position));
                    self.pending = None;
                } else {
                    return true;
                }
            }
            self.tx.blocking_send(Ok(frame)).is_ok()
        }

        fn send_error(&self, error: YPlaneError) {
            let _ = self.tx.blocking_send(Err(error));
        }
    }

    unsafe extern "C" fn handle_frame(frame: *const CDxvaFrame, context: *mut c_void) -> bool {
        if frame.is_null() || context.is_null() {
            return false;
        }
        let frame = unsafe { &*frame };
        let context = unsafe { &mut *(context as *mut DecodeContext) };

        if frame.data.is_null() {
            context.send_error(YPlaneError::backend_failure(
                BACKEND_NAME,
                "frame missing pixel data",
            ));
            return false;
        }

        let data = unsafe { slice::from_raw_parts(frame.data, frame.data_len) };
        let raw = match raw_from_nv12(
            frame.width,
            frame.height,
            frame.stride,
            data,
            context.output_format,
        ) {
            Ok(raw) => raw,
            Err(err) => {
                context.send_error(err);
                return false;
            }
        };

        let timestamp = if frame.timestamp_seconds.is_sign_negative() {
            None
        } else {
            Some(Duration::from_secs_f64(frame.timestamp_seconds))
        };

        let plane_frame = match PlaneFrame::from_raw(frame.width, frame.height, timestamp, raw) {
            Ok(frame_value) => frame_value.with_frame_index(Some(frame.frame_index)),
            Err(err) => {
                context.send_error(err);
                return false;
            }
        };

        context.handle_frame(plane_frame)
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

    fn raw_from_nv12(
        width: u32,
        height: u32,
        stride: usize,
        data: &[u8],
        output_format: OutputFormat,
    ) -> YPlaneResult<RawFrame> {
        let y_len = stride
            .checked_mul(height as usize)
            .ok_or_else(|| YPlaneError::backend_failure(BACKEND_NAME, "stride overflow"))?;
        if data.len() < y_len {
            return Err(YPlaneError::backend_failure(
                BACKEND_NAME,
                "incomplete NV12 buffer",
            ));
        }
        let chroma_height = ((height as usize) + 1) / 2;
        let uv_len = stride
            .checked_mul(chroma_height)
            .ok_or_else(|| YPlaneError::backend_failure(BACKEND_NAME, "stride overflow"))?;
        let y = &data[..y_len];
        let uv = if data.len() >= y_len + uv_len {
            &data[y_len..y_len + uv_len]
        } else {
            &[]
        };

        match output_format {
            OutputFormat::Y => Ok(RawFrame::Y {
                stride,
                data: y.to_vec().into(),
            }),
            OutputFormat::NV12 => {
                if uv.is_empty() {
                    return Err(YPlaneError::backend_failure(
                        BACKEND_NAME,
                        "NV12 output requires UV plane data",
                    ));
                }
                Ok(RawFrame::NV12 {
                    y_stride: stride,
                    uv_stride: stride,
                    y: y.to_vec().into(),
                    uv: uv.to_vec().into(),
                })
            }
            OutputFormat::NV21 => {
                if uv.is_empty() {
                    return Err(YPlaneError::backend_failure(
                        BACKEND_NAME,
                        "NV21 output requires UV plane data",
                    ));
                }
                let mut vu = uv.to_vec();
                for chunk in vu.chunks_exact_mut(2) {
                    chunk.swap(0, 1);
                }
                Ok(RawFrame::NV21 {
                    y_stride: stride,
                    vu_stride: stride,
                    y: y.to_vec().into(),
                    vu: vu.into(),
                })
            }
            OutputFormat::I420 => {
                if uv.is_empty() {
                    return Err(YPlaneError::backend_failure(
                        BACKEND_NAME,
                        "I420 output requires UV plane data",
                    ));
                }
                let chroma_width = ((width as usize) + 1) / 2;
                let (u, v) = nv12_to_i420(uv, stride, chroma_width, chroma_height);
                Ok(RawFrame::I420 {
                    y_stride: stride,
                    u_stride: chroma_width,
                    v_stride: chroma_width,
                    y: y.to_vec().into(),
                    u: u.into(),
                    v: v.into(),
                })
            }
            OutputFormat::YUYV | OutputFormat::UYVY => {
                if uv.is_empty() {
                    return Err(YPlaneError::backend_failure(
                        BACKEND_NAME,
                        "packed output requires UV plane data",
                    ));
                }
                let packed = nv12_to_packed(
                    y,
                    uv,
                    stride,
                    width as usize,
                    height as usize,
                    output_format,
                );
                match output_format {
                    OutputFormat::YUYV => Ok(RawFrame::YUYV {
                        stride: width as usize * 2,
                        data: packed.into(),
                    }),
                    OutputFormat::UYVY => Ok(RawFrame::UYVY {
                        stride: width as usize * 2,
                        data: packed.into(),
                    }),
                    _ => unreachable!(),
                }
            }
        }
    }

    fn nv12_to_i420(
        uv: &[u8],
        uv_stride: usize,
        chroma_width: usize,
        chroma_height: usize,
    ) -> (Vec<u8>, Vec<u8>) {
        let mut u = vec![0u8; chroma_width * chroma_height];
        let mut v = vec![0u8; chroma_width * chroma_height];
        for row in 0..chroma_height {
            let row_offset = row * uv_stride;
            for col in 0..chroma_width {
                let uv_index = row_offset + col * 2;
                if uv_index + 1 >= uv.len() {
                    continue;
                }
                let idx = row * chroma_width + col;
                u[idx] = uv[uv_index];
                v[idx] = uv[uv_index + 1];
            }
        }
        (u, v)
    }

    fn nv12_to_packed(
        y: &[u8],
        uv: &[u8],
        y_stride: usize,
        width: usize,
        height: usize,
        format: OutputFormat,
    ) -> Vec<u8> {
        let packed_stride = width * 2;
        let chroma_width = (width + 1) / 2;
        let mut out = vec![0u8; packed_stride * height];
        for row in 0..height {
            let y_row = row * y_stride;
            let uv_row = (row / 2) * y_stride;
            for col in 0..chroma_width {
                let x = col * 2;
                let y0 = y.get(y_row + x).copied().unwrap_or(0);
                let y1 = y.get(y_row + x + 1).copied().unwrap_or(y0);
                let uv_index = uv_row + col * 2;
                let u = uv.get(uv_index).copied().unwrap_or(128);
                let v = uv.get(uv_index + 1).copied().unwrap_or(128);
                let out_index = row * packed_stride + x * 2;
                if out_index + 3 >= out.len() {
                    continue;
                }
                match format {
                    OutputFormat::YUYV => {
                        out[out_index] = y0;
                        out[out_index + 1] = u;
                        out[out_index + 2] = y1;
                        out[out_index + 3] = v;
                    }
                    OutputFormat::UYVY => {
                        out[out_index] = u;
                        out[out_index + 1] = y0;
                        out[out_index + 2] = v;
                        out[out_index + 3] = y1;
                    }
                    _ => {}
                }
            }
        }
        out
    }

    pub fn boxed_dxva<P: AsRef<Path>>(
        path: P,
        channel_capacity: Option<usize>,
        output_format: OutputFormat,
    ) -> YPlaneResult<DynYPlaneProvider> {
        Ok(Box::new(DxvaProvider::open(
            path,
            channel_capacity,
            output_format,
        )?))
    }
}

#[cfg(not(target_os = "windows"))]
mod platform {
    use crate::{
        DynYPlaneProvider, PlaneStreamHandle, RawFrameFormat, YPlaneError, YPlaneResult,
        YPlaneStreamProvider,
    };
    use std::path::Path;

    pub struct DxvaProvider;

    impl DxvaProvider {
        pub fn open<P: AsRef<Path>>(
            _path: P,
            _channel_capacity: Option<usize>,
            _output_format: RawFrameFormat,
        ) -> YPlaneResult<Self> {
            Err(YPlaneError::unsupported("dxva"))
        }
    }

    impl YPlaneStreamProvider for DxvaProvider {
        fn into_stream(self: Box<Self>) -> PlaneStreamHandle {
            panic!("DXVA backend is only available on Windows builds");
        }
    }

    pub fn boxed_dxva<P: AsRef<Path>>(
        _path: P,
        _channel_capacity: Option<usize>,
        _output_format: RawFrameFormat,
    ) -> YPlaneResult<DynYPlaneProvider> {
        Err(YPlaneError::unsupported("dxva"))
    }
}

pub use platform::{DxvaProvider, boxed_dxva};
