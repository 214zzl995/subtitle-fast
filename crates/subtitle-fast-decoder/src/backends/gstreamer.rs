#![cfg(feature = "backend-gstreamer")]

use std::path::{Path, PathBuf};
use std::time::Duration;

use gstreamer as gst;
use gstreamer::ClockTime;
use gstreamer::MessageView;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use gstreamer_video as gst_video;
use tokio::sync::mpsc::Sender;

use crate::core::{
    DynYPlaneProvider, YPlaneError, YPlaneFrame, YPlaneResult, YPlaneStream, YPlaneStreamProvider,
    spawn_stream_from_channel,
};

const BACKEND_NAME: &str = "gstreamer";

pub struct GStreamerProvider {
    input: PathBuf,
    channel_capacity: usize,
}

impl GStreamerProvider {
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
            channel_capacity: 8,
        })
    }

    fn run(&self, tx: Sender<YPlaneResult<YPlaneFrame>>) -> YPlaneResult<()> {
        gst::init().map_err(|err| backend_error(err.to_string()))?;
        let pipeline = gst::Pipeline::new();
        let src = gst::ElementFactory::make("filesrc")
            .build()
            .map_err(|err| backend_error(format!("failed to create filesrc element: {err}")))?;
        src.set_property("location", self.input.to_string_lossy().as_ref());

        let decodebin = gst::ElementFactory::make("decodebin")
            .build()
            .map_err(|err| backend_error(format!("failed to create decodebin element: {err}")))?;
        let convert = gst::ElementFactory::make("videoconvert")
            .build()
            .map_err(|err| {
                backend_error(format!("failed to create videoconvert element: {err}"))
            })?;
        let caps = gst::Caps::builder("video/x-raw")
            .field("format", &"I420")
            .build();
        let appsink = gst_app::AppSink::builder()
            .caps(&caps)
            .drop(true)
            .max_buffers(8)
            .sync(false)
            .build();

        pipeline
            .add_many([
                &src,
                &decodebin,
                &convert,
                appsink.upcast_ref::<gst::Element>(),
            ])
            .map_err(|err| backend_error(err.to_string()))?;
        gst::Element::link_many([&src, &decodebin])
            .map_err(|err| backend_error(err.to_string()))?;
        convert
            .link(appsink.upcast_ref::<gst::Element>())
            .map_err(|err| backend_error(err.to_string()))?;

        let convert_clone = convert.clone();
        decodebin.connect_pad_added(move |_dbin, pad| {
            let Some(sink_pad) = convert_clone.static_pad("sink") else {
                return;
            };
            if sink_pad.is_linked() {
                return;
            }
            let _ = pad.link(&sink_pad);
        });

        let result = (|| {
            pipeline
                .set_state(gst::State::Playing)
                .map_err(|err| backend_error(format!("failed to set pipeline state: {err:?}")))?;

            let bus = pipeline
                .bus()
                .ok_or_else(|| backend_error("pipeline missing bus"))?;
            let mut frame_index: u64 = 0;
            loop {
                match appsink.pull_sample() {
                    Ok(sample) => {
                        drain_bus_errors(&bus)?;
                        let frame = frame_from_sample(&sample)?
                            .with_frame_index(Some(frame_index));
                        frame_index = frame_index.saturating_add(1);
                        if tx.blocking_send(Ok(frame)).is_err() {
                            break;
                        }
                    }
                    Err(err) => {
                        drain_bus_errors(&bus)?;
                        if appsink.is_eos() {
                            break;
                        }
                        return Err(backend_error(err.to_string()));
                    }
                }
            }
            Ok(())
        })();

        pipeline
            .set_state(gst::State::Null)
            .map_err(|err| backend_error(format!("failed to stop pipeline: {err:?}")))?;
        result
    }
}

impl YPlaneStreamProvider for GStreamerProvider {
    fn into_stream(self: Box<Self>) -> YPlaneStream {
        let provider = *self;
        let capacity = provider.channel_capacity;
        spawn_stream_from_channel(capacity, move |tx| {
            if let Err(err) = provider.run(tx.clone()) {
                let _ = tx.blocking_send(Err(err));
            }
        })
    }
}

fn drain_bus_errors(bus: &gst::Bus) -> YPlaneResult<()> {
    while let Some(msg) =
        bus.timed_pop_filtered(ClockTime::from_mseconds(0), &[gst::MessageType::Error])
    {
        if let MessageView::Error(err) = msg.view() {
            return Err(backend_error(err.error().to_string()));
        }
    }
    Ok(())
}

fn frame_from_sample(sample: &gst::Sample) -> YPlaneResult<YPlaneFrame> {
    let buffer = sample
        .buffer()
        .ok_or_else(|| backend_error("appsink sample missing buffer"))?;
    let caps = sample
        .caps()
        .ok_or_else(|| backend_error("appsink sample missing caps"))?;
    let info =
        gst_video::VideoInfo::from_caps(&caps).map_err(|err| backend_error(err.to_string()))?;
    let map = buffer
        .map_readable()
        .map_err(|err| backend_error(err.to_string()))?;
    let stride = info.stride()[0] as usize;
    let height = info.height() as usize;
    let width = info.width() as u32;
    let plane_size = stride * height;
    let data = map.as_slice();
    if data.len() < plane_size {
        return Err(backend_error(format!(
            "incomplete Y plane: have {} expected {}",
            data.len(),
            plane_size
        )));
    }
    let mut plane = Vec::with_capacity(plane_size);
    plane.extend_from_slice(&data[..plane_size]);
    let timestamp = buffer.pts().map(|ts| Duration::from_nanos(ts.nseconds()));
    YPlaneFrame::from_owned(width, info.height() as u32, stride, timestamp, plane)
}

fn backend_error(message: impl Into<String>) -> YPlaneError {
    YPlaneError::backend_failure(BACKEND_NAME, message)
}

pub fn boxed_gstreamer<P: AsRef<Path>>(path: P) -> YPlaneResult<DynYPlaneProvider> {
    Ok(Box::new(GStreamerProvider::open(path)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_returns_error() {
        let result = GStreamerProvider::open("/tmp/nonexistent-file.mp4");
        assert!(result.is_err());
    }
}
