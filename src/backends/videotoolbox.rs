#![cfg(feature = "backend-videotoolbox")]

use std::collections::BTreeMap;
use std::ffi::{CStr, CString};
use std::os::raw::c_void;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use objc::rc::StrongPtr;
use objc::runtime::{BOOL, NO, Object, YES};
use objc::{class, msg_send};
use rayon::ThreadPool;

use crate::core::{
    DynYPlaneProvider, YPlaneError, YPlaneFrame, YPlaneResult, YPlaneStream, YPlaneStreamProvider,
    spawn_stream_from_channel,
};

#[cfg(target_os = "macos")]
mod platform {
    use super::*;

    use core_foundation::base::{CFRelease, CFTypeRef};
    use core_foundation_sys::array::{CFArrayGetCount, CFArrayGetValueAtIndex, CFArrayRef};
    use core_foundation_sys::boolean::{CFBooleanGetValue, CFBooleanRef};
    use core_foundation_sys::dictionary::{CFDictionaryGetValue, CFDictionaryRef};

    #[link(name = "AVFoundation", kind = "framework")]
    extern "C" {}

    #[link(name = "CoreMedia", kind = "framework")]
    extern "C" {
        fn CMSampleBufferGetImageBuffer(buffer: CMSampleBufferRef) -> CVPixelBufferRef;
        fn CMSampleBufferGetPresentationTimeStamp(buffer: CMSampleBufferRef) -> CMTime;
        fn CMSampleBufferGetSampleAttachmentsArray(
            buffer: CMSampleBufferRef,
            create_if_necessary: BOOL,
        ) -> CFArrayRef;
        static kCMSampleAttachmentKey_NotSync: CFTypeRef;
    }

    #[link(name = "CoreVideo", kind = "framework")]
    extern "C" {
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

    #[derive(Clone, Copy)]
    struct PendingSample {
        sample: CMSampleBufferRef,
        pts: CMTime,
    }

    pub struct VideoToolboxProvider {
        input: PathBuf,
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
            Ok(Self {
                input: path.to_path_buf(),
            })
        }

        fn available_parallelism() -> usize {
            std::thread::available_parallelism()
                .map(|p| p.get())
                .unwrap_or(2)
                .max(1)
        }

        fn build_pool() -> YPlaneResult<ThreadPool> {
            rayon::ThreadPoolBuilder::new()
                .thread_name(|idx| format!("vt-worker-{idx}"))
                .num_threads(Self::available_parallelism())
                .build()
                .map_err(|err| {
                    YPlaneError::backend_failure(
                        "videotoolbox",
                        format!("failed to build worker pool: {err}"),
                    )
                })
        }
    }

    impl YPlaneStreamProvider for VideoToolboxProvider {
        fn into_stream(self: Box<Self>) -> YPlaneStream {
            let path = self.input.clone();
            spawn_stream_from_channel(32, move |tx| unsafe {
                let pool: *mut Object = msg_send![class!(NSAutoreleasePool), new];
                if let Err(err) = decode_videotoolbox(path, tx) {
                    let _ = tx.blocking_send(Err(err));
                }
                let _: () = msg_send![pool, drain];
            })
        }
    }

    fn decode_videotoolbox(
        path: PathBuf,
        tx: tokio::sync::mpsc::Sender<YPlaneResult<YPlaneFrame>>,
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

            let pixel_format = StrongPtr::new(
                msg_send![class!(NSNumber), numberWithUnsignedInt:PIXEL_FORMAT_NV12],
            );
            let keys = [k_cv_pixel_buffer_pixel_format_type_key()];
            let values = [*pixel_format];
            let settings: *mut Object = msg_send![class!(NSDictionary),
                dictionaryWithObjects:values.as_ptr()
                forKeys:keys.as_ptr()
                count:keys.len()
            ];
            let settings = StrongPtr::new(settings);

            let mut error: *mut Object = std::ptr::null_mut();
            let reader: *mut Object =
                msg_send![class!(AVAssetReader), assetReaderWithAsset:*asset error:&mut error];
            if reader.is_null() {
                return Err(vt_error("failed to create AVAssetReader", error));
            }
            let reader = StrongPtr::new(reader);

            let output: *mut Object = msg_send![class!(AVAssetReaderTrackOutput),
                assetReaderTrackOutputWithTrack:track
                outputSettings:*settings
            ];
            if output.is_null() {
                return Err(YPlaneError::backend_failure(
                    "videotoolbox",
                    "failed to create track output",
                ));
            }
            let output = StrongPtr::new(output);

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

            let thread_pool = VideoToolboxProvider::build_pool()?;
            let (chunk_tx, chunk_rx) = mpsc::channel();

            let mut chunk_index = 0usize;
            let mut next_chunk = 0usize;
            let mut chunk_samples = Vec::new();
            let mut ordered = BTreeMap::<usize, Vec<YPlaneResult<YPlaneFrame>>>::new();

            loop {
                let sample: *mut Object = msg_send![*output, copyNextSampleBuffer];
                if sample.is_null() {
                    let status: i32 = msg_send![*reader, status];
                    if status == AVAssetReaderStatus::Completed as i32 {
                        break;
                    } else if status == AVAssetReaderStatus::Failed as i32 {
                        let err_obj: *mut Object = msg_send![*reader, error];
                        release_samples(std::mem::take(&mut chunk_samples));
                        return Err(vt_error("videotoolbox reader failed", err_obj));
                    } else if status == AVAssetReaderStatus::Cancelled as i32 {
                        release_samples(std::mem::take(&mut chunk_samples));
                        return Err(YPlaneError::backend_failure(
                            "videotoolbox",
                            "videotoolbox reader was cancelled",
                        ));
                    } else {
                        continue;
                    }
                }

                let sample_buf = sample as CMSampleBufferRef;
                let is_keyframe = is_sync_sample(sample_buf);

                if is_keyframe && !chunk_samples.is_empty() {
                    flush_chunk(
                        &thread_pool,
                        chunk_index,
                        std::mem::take(&mut chunk_samples),
                        chunk_tx.clone(),
                    );
                    chunk_index += 1;
                }

                let pts = CMSampleBufferGetPresentationTimeStamp(sample_buf);
                chunk_samples.push(PendingSample {
                    sample: sample_buf,
                    pts,
                });
            }

            if !chunk_samples.is_empty() {
                flush_chunk(&thread_pool, chunk_index, chunk_samples, chunk_tx.clone());
            }
            drop(chunk_tx);

            for (index, frames) in chunk_rx {
                ordered.insert(index, frames);
                while let Some(frames) = ordered.remove(&next_chunk) {
                    for frame in frames {
                        match frame {
                            Ok(frame) => {
                                if tx.blocking_send(Ok(frame)).is_err() {
                                    return Ok(());
                                }
                            }
                            Err(err) => {
                                let _ = tx.blocking_send(Err(err));
                                return Ok(());
                            }
                        }
                    }
                    next_chunk += 1;
                }
            }
        }
        Ok(())
    }

    fn flush_chunk(
        pool: &ThreadPool,
        chunk_index: usize,
        samples: Vec<PendingSample>,
        chunk_tx: mpsc::Sender<(usize, Vec<YPlaneResult<YPlaneFrame>>)>,
    ) {
        pool.spawn_fifo(move || {
            let mut results = Vec::with_capacity(samples.len());
            for sample in samples {
                unsafe {
                    let result = sample_to_frame(sample.sample, sample.pts);
                    CFRelease(sample.sample as CFTypeRef);
                    results.push(result);
                }
            }
            let _ = chunk_tx.send((chunk_index, results));
        });
    }

    fn release_samples(samples: Vec<PendingSample>) {
        for sample in samples {
            unsafe {
                CFRelease(sample.sample as CFTypeRef);
            }
        }
    }

    fn sample_to_frame(sample: CMSampleBufferRef, pts: CMTime) -> YPlaneResult<YPlaneFrame> {
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
            let mut buffer = vec![0u8; len];
            std::ptr::copy_nonoverlapping(base, buffer.as_mut_ptr(), len);

            CVPixelBufferUnlockBaseAddress(pixel_buffer, PIXEL_BUFFER_LOCK_READ_ONLY);

            let timestamp = cm_time_to_duration(pts);
            YPlaneFrame::from_owned(width, height, stride, timestamp, buffer)
        }
    }

    fn nsstring_from_path(path: &Path) -> YPlaneResult<*mut Object> {
        let cstring = CString::new(path.to_string_lossy().as_bytes()).map_err(|err| {
            YPlaneError::backend_failure("videotoolbox", format!("invalid path encoding: {err}"))
        })?;
        unsafe {
            let string: *mut Object =
                msg_send![class!(NSString), stringWithUTF8String:cstring.as_ptr()];
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
            let message = nsstring_to_string(description).unwrap_or_else(|| context.to_string());
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

    #[link(name = "CoreVideo", kind = "framework")]
    extern "C" {
        static kCVPixelBufferPixelFormatTypeKey: CFStringRef;
    }

    fn av_media_type_video() -> CFStringRef {
        unsafe { AVMediaTypeVideo }
    }

    #[link(name = "AVFoundation", kind = "framework")]
    extern "C" {
        static AVMediaTypeVideo: CFStringRef;
    }

    fn is_sync_sample(sample: CMSampleBufferRef) -> bool {
        unsafe {
            let attachments = CMSampleBufferGetSampleAttachmentsArray(sample, NO);
            if attachments.is_null() {
                return true;
            }
            if CFArrayGetCount(attachments) == 0 {
                return true;
            }
            let dict = CFArrayGetValueAtIndex(attachments, 0) as CFDictionaryRef;
            if dict.is_null() {
                return true;
            }
            let key = kCMSampleAttachmentKey_NotSync;
            let value = CFDictionaryGetValue(dict, key as *const _);
            if value.is_null() {
                true
            } else {
                CFBooleanGetValue(value as CFBooleanRef) == 0
            }
        }
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

    #[repr(i32)]
    enum AVAssetReaderStatus {
        Unknown = 0,
        Reading = 1,
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
