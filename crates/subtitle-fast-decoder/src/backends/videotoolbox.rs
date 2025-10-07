#![cfg(feature = "backend-videotoolbox")]

use std::ffi::{CStr, CString};
use std::fs::File;
use std::io::BufReader;
use std::os::raw::c_void;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::sync::mpsc;

use objc::rc::StrongPtr;
use objc::runtime::{BOOL, NO, Object, YES};
use objc::{class, msg_send, sel, sel_impl};

use crate::core::{
    DynYPlaneProvider, YPlaneError, YPlaneFrame, YPlaneResult, YPlaneStream, YPlaneStreamProvider,
    spawn_stream_from_channel,
};

#[cfg(target_os = "macos")]
#[allow(unexpected_cfgs)]
mod platform {
    use super::*;

    use core_foundation::base::{CFRelease, CFTypeRef};
    use mp4::{Mp4Reader, TrackType};

    #[allow(improper_ctypes)]
    #[link(name = "AVFoundation", kind = "framework")]
    unsafe extern "C" {}

    #[allow(improper_ctypes)]
    #[link(name = "CoreMedia", kind = "framework")]
    unsafe extern "C" {
        fn CMSampleBufferGetImageBuffer(buffer: CMSampleBufferRef) -> CVPixelBufferRef;
        fn CMSampleBufferGetPresentationTimeStamp(buffer: CMSampleBufferRef) -> CMTime;
    }

    #[allow(improper_ctypes)]
    #[link(name = "CoreVideo", kind = "framework")]
    unsafe extern "C" {
        fn CVPixelBufferLockBaseAddress(buffer: CVPixelBufferRef, flags: u64) -> i32;
        fn CVPixelBufferUnlockBaseAddress(buffer: CVPixelBufferRef, flags: u64) -> i32;
        fn CVPixelBufferGetPlaneCount(buffer: CVPixelBufferRef) -> usize;
        fn CVPixelBufferGetBaseAddressOfPlane(
            buffer: CVPixelBufferRef,
            plane_index: usize,
        ) -> *mut c_void;
        fn CVPixelBufferGetBytesPerRowOfPlane(
            buffer: CVPixelBufferRef,
            plane_index: usize,
        ) -> usize;
        fn CVPixelBufferGetWidthOfPlane(buffer: CVPixelBufferRef, plane_index: usize) -> usize;
        fn CVPixelBufferGetHeightOfPlane(buffer: CVPixelBufferRef, plane_index: usize) -> usize;
    }

    type CFStringRef = *const Object;
    type CMSampleBufferRef = *mut __CMSampleBuffer;
    type CVPixelBufferRef = *mut __CVPixelBuffer;

    #[repr(C)]
    struct __CMSampleBuffer;
    #[repr(C)]
    struct __CVPixelBuffer;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CMTime {
        value: i64,
        timescale: i32,
        flags: u32,
        epoch: i64,
    }

    const PIXEL_FORMAT_NV12: u32 = 875_704_438; // kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange
    const PIXEL_BUFFER_LOCK_READ_ONLY: u64 = 0x0000_0001;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CMTimeRange {
        start: CMTime,
        duration: CMTime,
    }

    pub struct VideoToolboxProvider {
        input: PathBuf,
        total_frames: Option<u64>,
    }

    impl VideoToolboxProvider {
        pub fn open<P: AsRef<Path>>(path: P) -> YPlaneResult<Self> {
            let path = path.as_ref();
            if !path.exists() {
                return Err(YPlaneError::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("input file {} does not exist", path.display()),
                )));
            }
            let total_frames = Self::probe_total_frames(path)?;
            Ok(Self {
                input: path.to_path_buf(),
                total_frames,
            })
        }

        const RESULT_CHANNEL_CAPACITY: usize = 16;

        fn probe_total_frames(path: &Path) -> YPlaneResult<Option<u64>> {
            if let Some(total) = Self::probe_total_frames_mp4(path)? {
                return Ok(Some(total));
            }
            unsafe { Self::probe_total_frames_avfoundation(path) }
        }

        fn probe_total_frames_mp4(path: &Path) -> YPlaneResult<Option<u64>> {
            let file = File::open(path)?;
            let size = file.metadata()?.len();
            let reader = BufReader::new(file);
            let mut reader = match Mp4Reader::read_header(reader, size) {
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

            let mut available: u64 = 0;
            for sample_id in 1..=count {
                let sample = reader.read_sample(track_id, sample_id).map_err(|err| {
                    YPlaneError::backend_failure(
                        "videotoolbox",
                        format!("failed to read MP4 sample {sample_id}: {err}"),
                    )
                })?;
                if sample.is_some() {
                    available = available.saturating_add(1);
                }
            }

            if available == 0 {
                Ok(None)
            } else {
                Ok(Some(available))
            }
        }

        unsafe fn probe_total_frames_avfoundation(path: &Path) -> YPlaneResult<Option<u64>> {
            let ns_path = unsafe { StrongPtr::new(nsstring_from_path(path)?) };
            let url: *mut Object = msg_send![class!(NSURL), fileURLWithPath:*ns_path];
            if url.is_null() {
                return Err(YPlaneError::backend_failure(
                    "videotoolbox",
                    format!("failed to create file URL for {}", path.display()),
                ));
            }

            let asset: *mut Object = msg_send![class!(AVURLAsset), alloc];
            let asset: *mut Object = msg_send![asset,
                initWithURL:url
                options:std::ptr::null_mut::<c_void>() as *mut Object
            ];
            if asset.is_null() {
                return Err(YPlaneError::backend_failure(
                    "videotoolbox",
                    "failed to open AVURLAsset",
                ));
            }
            let asset = unsafe { StrongPtr::new(asset) };

            let media_type: CFStringRef = av_media_type_video();
            let tracks: *mut Object = msg_send![*asset, tracksWithMediaType:media_type];
            if tracks.is_null() {
                return Err(YPlaneError::backend_failure(
                    "videotoolbox",
                    "asset contains no video tracks",
                ));
            }
            let track: *mut Object = msg_send![tracks, firstObject];
            if track.is_null() {
                return Err(YPlaneError::backend_failure(
                    "videotoolbox",
                    "asset contains no primary video track",
                ));
            }

            let time_range: CMTimeRange = msg_send![track, timeRange];
            let duration_seconds = match cm_time_to_seconds(time_range.duration) {
                Some(value) if value.is_finite() && value > 0.0 => value,
                _ => return Ok(None),
            };

            let nominal_frame_rate: f64 = msg_send![track, nominalFrameRate];
            let fps = if nominal_frame_rate.is_finite() && nominal_frame_rate > 0.0 {
                Some(nominal_frame_rate)
            } else {
                let min_frame_duration: CMTime = msg_send![track, minFrameDuration];
                cm_time_to_seconds(min_frame_duration).and_then(|seconds| {
                    if seconds > 0.0 {
                        Some(1.0 / seconds)
                    } else {
                        None
                    }
                })
            };

            let fps = match fps {
                Some(value) if value.is_finite() && value > 0.0 => value,
                _ => return Ok(None),
            };

            let total = (duration_seconds * fps).round();
            if total.is_sign_negative() || !total.is_finite() || total <= 0.0 {
                Ok(None)
            } else {
                Ok(Some(total as u64))
            }
        }
    }

    impl YPlaneStreamProvider for VideoToolboxProvider {
        fn total_frames(&self) -> Option<u64> {
            self.total_frames
        }

        fn into_stream(self: Box<Self>) -> YPlaneStream {
            let path = self.input.clone();
            spawn_stream_from_channel(Self::RESULT_CHANNEL_CAPACITY, move |tx| unsafe {
                let pool: *mut Object = msg_send![class!(NSAutoreleasePool), new];
                if let Err(err) = decode_videotoolbox(path.clone(), tx.clone()) {
                    let _ = tx.blocking_send(Err(err));
                }
                let _: () = msg_send![pool, drain];
            })
        }
    }

    fn decode_videotoolbox(
        path: PathBuf,
        tx: mpsc::Sender<YPlaneResult<YPlaneFrame>>,
    ) -> YPlaneResult<()> {
        unsafe {
            let ns_path = StrongPtr::new(nsstring_from_path(&path)?);
            let url: *mut Object = msg_send![class!(NSURL), fileURLWithPath:*ns_path];
            if url.is_null() {
                return Err(YPlaneError::backend_failure(
                    "videotoolbox",
                    format!("failed to create file URL for {}", path.display()),
                ));
            }

            let asset: *mut Object = msg_send![class!(AVURLAsset), alloc];
            let asset: *mut Object = msg_send![asset, initWithURL:url options:std::ptr::null_mut::<c_void>() as *mut Object];
            let asset = StrongPtr::new(asset);

            let media_type: CFStringRef = av_media_type_video();
            let tracks: *mut Object = msg_send![*asset, tracksWithMediaType:media_type];
            if tracks.is_null() {
                return Err(YPlaneError::backend_failure(
                    "videotoolbox",
                    "asset contains no video tracks",
                ));
            }
            let track: *mut Object = msg_send![tracks, firstObject];
            if track.is_null() {
                return Err(YPlaneError::backend_failure(
                    "videotoolbox",
                    "asset contains no primary video track",
                ));
            }

            let pixel_format_obj: *mut Object = msg_send![class!(NSNumber), alloc];
            if pixel_format_obj.is_null() {
                return Err(YPlaneError::backend_failure(
                    "videotoolbox",
                    "failed to allocate NSNumber for pixel format",
                ));
            }
            let pixel_format_obj: *mut Object =
                msg_send![pixel_format_obj, initWithUnsignedInt:PIXEL_FORMAT_NV12];
            if pixel_format_obj.is_null() {
                return Err(YPlaneError::backend_failure(
                    "videotoolbox",
                    "failed to create pixel format NSNumber",
                ));
            }
            let pixel_format = StrongPtr::new(pixel_format_obj);
            let keys = [k_cv_pixel_buffer_pixel_format_type_key()];
            let values = [*pixel_format];
            let settings_obj: *mut Object = msg_send![class!(NSDictionary), alloc];
            if settings_obj.is_null() {
                return Err(YPlaneError::backend_failure(
                    "videotoolbox",
                    "failed to create output settings dictionary",
                ));
            }
            let settings_obj: *mut Object = msg_send![settings_obj,
                initWithObjects:values.as_ptr()
                forKeys:keys.as_ptr()
                count:keys.len()
            ];
            if settings_obj.is_null() {
                return Err(YPlaneError::backend_failure(
                    "videotoolbox",
                    "failed to initialize output settings dictionary",
                ));
            }
            let settings = StrongPtr::new(settings_obj);

            let mut error: *mut Object = std::ptr::null_mut();
            let reader_obj: *mut Object = msg_send![class!(AVAssetReader), alloc];
            if reader_obj.is_null() {
                return Err(YPlaneError::backend_failure(
                    "videotoolbox",
                    "failed to allocate AVAssetReader",
                ));
            }
            let reader_obj: *mut Object =
                msg_send![reader_obj, initWithAsset:*asset error:&mut error];
            if reader_obj.is_null() {
                return Err(vt_error("failed to create AVAssetReader", error));
            }
            let reader = StrongPtr::new(reader_obj);

            let output_obj: *mut Object = msg_send![class!(AVAssetReaderTrackOutput), alloc];
            if output_obj.is_null() {
                return Err(YPlaneError::backend_failure(
                    "videotoolbox",
                    "failed to allocate AVAssetReaderTrackOutput",
                ));
            }
            let output_obj: *mut Object = msg_send![output_obj,
                initWithTrack:track
                outputSettings:*settings
            ];
            if output_obj.is_null() {
                return Err(YPlaneError::backend_failure(
                    "videotoolbox",
                    "failed to create track output",
                ));
            }
            let output = StrongPtr::new(output_obj);
            let responds: BOOL =
                msg_send![*output, respondsToSelector: sel!(setAlwaysCopiesSampleData:)];
            if responds == YES {
                let _: () = msg_send![*output, setAlwaysCopiesSampleData:NO];
            }

            let can_add: BOOL = msg_send![*reader, canAddOutput:*output];
            if can_add != YES {
                return Err(YPlaneError::backend_failure(
                    "videotoolbox",
                    "asset reader refused track output",
                ));
            }
            let _: () = msg_send![*reader, addOutput:*output];

            let started: BOOL = msg_send![*reader, startReading];
            if started != YES {
                let err_obj: *mut Object = msg_send![*reader, error];
                return Err(vt_error("failed to start reading", err_obj));
            }

            let mut next_sample_index: u64 = 0;

            loop {
                let sample: *mut Object = msg_send![*output, copyNextSampleBuffer];
                if sample.is_null() {
                    let status: i32 = msg_send![*reader, status];
                    if status == AVAssetReaderStatus::Completed as i32 {
                        break;
                    } else if status == AVAssetReaderStatus::Failed as i32 {
                        let err_obj: *mut Object = msg_send![*reader, error];
                        return Err(vt_error("videotoolbox reader failed", err_obj));
                    } else if status == AVAssetReaderStatus::Cancelled as i32 {
                        return Err(YPlaneError::backend_failure(
                            "videotoolbox",
                            "videotoolbox reader was cancelled",
                        ));
                    } else {
                        continue;
                    }
                }

                let sample_buf = sample as CMSampleBufferRef;
                let pts = CMSampleBufferGetPresentationTimeStamp(sample_buf);
                let index = next_sample_index;
                next_sample_index = next_sample_index.saturating_add(1);

                let frame_result = sample_to_frame(sample_buf, pts, index);
                CFRelease(sample as CFTypeRef);

                match frame_result {
                    Ok(frame) => {
                        if tx.blocking_send(Ok(frame)).is_err() {
                            return Ok(());
                        }
                    }
                    Err(err) => {
                        return Err(err);
                    }
                }
            }
        }
        Ok(())
    }

    fn sample_to_frame(
        sample: CMSampleBufferRef,
        pts: CMTime,
        index: u64,
    ) -> YPlaneResult<YPlaneFrame> {
        unsafe {
            let pixel_buffer = CMSampleBufferGetImageBuffer(sample);
            if pixel_buffer.is_null() {
                return Err(YPlaneError::backend_failure(
                    "videotoolbox",
                    "sample buffer missing pixel buffer",
                ));
            }
            if CVPixelBufferGetPlaneCount(pixel_buffer) == 0 {
                return Err(YPlaneError::backend_failure(
                    "videotoolbox",
                    "expected planar pixel buffer for Y plane extraction",
                ));
            }

            let lock_status =
                CVPixelBufferLockBaseAddress(pixel_buffer, PIXEL_BUFFER_LOCK_READ_ONLY);
            if lock_status != 0 {
                return Err(YPlaneError::backend_failure(
                    "videotoolbox",
                    format!("failed to lock pixel buffer: status {lock_status}"),
                ));
            }

            let base = CVPixelBufferGetBaseAddressOfPlane(pixel_buffer, 0) as *const u8;
            let stride = CVPixelBufferGetBytesPerRowOfPlane(pixel_buffer, 0);
            let width = CVPixelBufferGetWidthOfPlane(pixel_buffer, 0) as u32;
            let height = CVPixelBufferGetHeightOfPlane(pixel_buffer, 0) as u32;
            let len = stride.checked_mul(height as usize).ok_or_else(|| {
                YPlaneError::backend_failure(
                    "videotoolbox",
                    "calculated stride overflow for Y plane",
                )
            })?;
            let buffer = std::slice::from_raw_parts(base, len).to_vec();

            CVPixelBufferUnlockBaseAddress(pixel_buffer, PIXEL_BUFFER_LOCK_READ_ONLY);

            let timestamp = cm_time_to_duration(pts);
            let frame = YPlaneFrame::from_owned(width, height, stride, timestamp, buffer)?;
            Ok(frame.with_frame_index(Some(index)))
        }
    }

    fn nsstring_from_path(path: &Path) -> YPlaneResult<*mut Object> {
        let cstring = CString::new(path.to_string_lossy().as_bytes()).map_err(|err| {
            YPlaneError::backend_failure("videotoolbox", format!("invalid path encoding: {err}"))
        })?;
        unsafe {
            let string: *mut Object = msg_send![class!(NSString), alloc];
            if string.is_null() {
                return Err(YPlaneError::backend_failure(
                    "videotoolbox",
                    "failed to allocate NSString",
                ));
            }
            let string: *mut Object = msg_send![string, initWithUTF8String:cstring.as_ptr()];
            if string.is_null() {
                Err(YPlaneError::backend_failure(
                    "videotoolbox",
                    "failed to convert path to NSString",
                ))
            } else {
                Ok(string)
            }
        }
    }

    fn vt_error(context: &str, error: *mut Object) -> YPlaneError {
        if error.is_null() {
            return YPlaneError::backend_failure("videotoolbox", context);
        }
        unsafe {
            let description: *mut Object = msg_send![error, localizedDescription];
            let domain: *mut Object = msg_send![error, domain];
            let code: i64 = msg_send![error, code];
            let reason: *mut Object = msg_send![error, localizedFailureReason];
            let suggestion: *mut Object = msg_send![error, localizedRecoverySuggestion];
            let mut message = nsstring_to_string(description)
                .map(|desc| format!("{context}: {desc}"))
                .unwrap_or_else(|| context.to_string());
            if let Some(domain) = nsstring_to_string(domain) {
                message = format!("{message} (domain={domain} code={code})");
            }
            if let Some(reason) = nsstring_to_string(reason) {
                message = format!("{message} reason={reason}");
            }
            if let Some(suggestion) = nsstring_to_string(suggestion) {
                message = format!("{message} suggestion={suggestion}");
            }
            YPlaneError::backend_failure("videotoolbox", message)
        }
    }

    fn nsstring_to_string(ns_string: *mut Object) -> Option<String> {
        if ns_string.is_null() {
            return None;
        }
        unsafe {
            let cstr: *const i8 = msg_send![ns_string, UTF8String];
            if cstr.is_null() {
                None
            } else {
                Some(CStr::from_ptr(cstr).to_string_lossy().into_owned())
            }
        }
    }

    fn k_cv_pixel_buffer_pixel_format_type_key() -> *mut Object {
        unsafe { kCVPixelBufferPixelFormatTypeKey as *mut Object }
    }

    #[allow(improper_ctypes)]
    #[link(name = "CoreVideo", kind = "framework")]
    unsafe extern "C" {
        static kCVPixelBufferPixelFormatTypeKey: CFStringRef;
    }

    fn av_media_type_video() -> CFStringRef {
        unsafe { AVMediaTypeVideo }
    }

    #[allow(improper_ctypes)]
    #[link(name = "AVFoundation", kind = "framework")]
    unsafe extern "C" {
        static AVMediaTypeVideo: CFStringRef;
    }

    fn cm_time_to_duration(time: CMTime) -> Option<Duration> {
        if time.timescale <= 0 {
            return None;
        }
        let timescale = time.timescale as i128;
        if timescale == 0 {
            return None;
        }
        let value = time.value as i128;
        let nanos = value.checked_mul(1_000_000_000)?.checked_div(timescale)?;
        if nanos < 0 {
            None
        } else {
            Some(Duration::from_nanos(nanos as u64))
        }
    }

    fn cm_time_to_seconds(time: CMTime) -> Option<f64> {
        cm_time_to_duration(time).map(|duration| duration.as_secs_f64())
    }

    #[repr(i32)]
    enum AVAssetReaderStatus {
        _Unknown = 0,
        _Reading = 1,
        Completed = 2,
        Failed = 3,
        Cancelled = 4,
    }

    pub fn boxed_videotoolbox<P: AsRef<Path>>(path: P) -> YPlaneResult<DynYPlaneProvider> {
        VideoToolboxProvider::open(path).map(|provider| Box::new(provider) as DynYPlaneProvider)
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::*;

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

    pub fn boxed_videotoolbox<P: AsRef<Path>>(_path: P) -> YPlaneResult<DynYPlaneProvider> {
        Err(YPlaneError::unsupported("videotoolbox"))
    }
}

pub use platform::{VideoToolboxProvider, boxed_videotoolbox};
