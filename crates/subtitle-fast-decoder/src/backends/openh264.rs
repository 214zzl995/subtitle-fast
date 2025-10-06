#![cfg(feature = "backend-openh264")]

use std::collections::VecDeque;
use std::fs;
use std::fs::File;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::time::Duration;

use mp4::{MediaType, Mp4Reader, Mp4Track, TrackType};
use openh264::Error as OpenH264Error;
use openh264::decoder::Decoder;
use openh264::formats::YUVSource;
use openh264::nal_units;
use rayon::spawn;

use crate::core::{
    DynYPlaneProvider, YPlaneError, YPlaneFrame, YPlaneResult, YPlaneStream, YPlaneStreamProvider,
    spawn_stream_from_channel,
};
use tokio::sync::mpsc::{self, Sender, error::TryRecvError};

const BACKEND_NAME: &str = "openh264";
const CHUNK_RESULT_CAPACITY: usize = 64;

pub struct OpenH264Provider {
    input: PathBuf,
    total_frames: Option<u64>,
}

impl OpenH264Provider {
    pub fn open<P: AsRef<Path>>(path: P) -> YPlaneResult<Self> {
        let path = path.as_ref();
        if !path.exists() {
            return Err(YPlaneError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("input file {} does not exist", path.display()),
            )));
        }
        let total_frames = probe_total_frames(path)?;
        Ok(Self {
            input: path.to_path_buf(),
            total_frames,
        })
    }

    fn decode_loop(&self, tx: Sender<YPlaneResult<YPlaneFrame>>) -> YPlaneResult<()> {
        let data = fs::read(&self.input)?;
        match decode_mp4_stream(data.as_slice(), tx.clone()) {
            Ok(()) => Ok(()),
            Err(mp4_err) => {
                if looks_like_annexb(&data) {
                    decode_annexb_stream(data.as_slice(), tx)
                } else {
                    Err(mp4_err)
                }
            }
        }
    }
}

impl YPlaneStreamProvider for OpenH264Provider {
    fn total_frames(&self) -> Option<u64> {
        self.total_frames
    }

    fn into_stream(self: Box<Self>) -> YPlaneStream {
        let provider = *self;
        spawn_stream_from_channel(32, move |tx| {
            if let Err(err) = provider.decode_loop(tx.clone()) {
                let _ = tx.blocking_send(Err(err));
            }
        })
    }
}

fn convert_frame(
    image: &openh264::decoder::DecodedYUV<'_>,
    timestamp: Option<Duration>,
) -> YPlaneResult<YPlaneFrame> {
    let (width, height) = image.dimensions();
    let stride = image.strides().0;
    let plane = image.y();
    let mut buffer = Vec::with_capacity(stride * height);
    let plane_len = plane.len();
    for row in 0..height {
        let offset = row * stride;
        let end = offset + stride;
        if end <= plane_len {
            buffer.extend_from_slice(&plane[offset..end]);
        } else if offset < plane_len {
            buffer.extend_from_slice(&plane[offset..plane_len]);
            break;
        } else {
            break;
        }
    }
    if buffer.len() < stride * height {
        buffer.resize(stride * height, 0);
    }
    debug_assert_eq!(buffer.len(), stride * height);
    YPlaneFrame::from_owned(width as u32, height as u32, stride, timestamp, buffer)
}

fn map_openh264_error(err: OpenH264Error) -> YPlaneError {
    YPlaneError::backend_failure(BACKEND_NAME, err.to_string())
}

fn looks_like_annexb(data: &[u8]) -> bool {
    data.windows(4).any(|w| matches!(w, [0, 0, 0, 1]))
        || data.windows(3).any(|w| matches!(w, [0, 0, 1]))
}

fn probe_total_frames(path: &Path) -> YPlaneResult<Option<u64>> {
    let file = File::open(path)?;
    let size = file.metadata()?.len();

    let reader = match Mp4Reader::read_header(file, size) {
        Ok(reader) => reader,
        Err(_) => return Ok(None),
    };

    let track_id = reader
        .tracks()
        .iter()
        .find_map(
            |(id, track)| match (track.track_type(), track.media_type()) {
                (Ok(TrackType::Video), Ok(MediaType::H264)) => Some(*id),
                _ => None,
            },
        )
        .ok_or_else(|| {
            YPlaneError::backend_failure(
                BACKEND_NAME,
                "MP4 file does not contain an H.264 video track",
            )
        })?;

    let sample_count = reader.sample_count(track_id).map_err(|err| {
        YPlaneError::backend_failure(
            BACKEND_NAME,
            format!("failed to query MP4 sample count: {err}"),
        )
    })?;

    Ok(Some(sample_count.into()))
}

fn decode_annexb_stream(data: &[u8], tx: Sender<YPlaneResult<YPlaneFrame>>) -> YPlaneResult<()> {
    let mut decoder = Decoder::new().map_err(map_openh264_error)?;
    let mut frame_index: u64 = 0;
    for packet in nal_units(data) {
        match decoder.decode(packet) {
            Ok(Some(image)) => {
                let timestamp = Some(Duration::from_secs_f64(frame_index as f64 / 30.0));
                frame_index = frame_index.saturating_add(1);
                let frame = convert_frame(&image, timestamp)?;
                if tx.blocking_send(Ok(frame)).is_err() {
                    return Ok(());
                }
            }
            Ok(None) => {}
            Err(err) => {
                return Err(map_openh264_error(err));
            }
        }
    }

    for image in decoder.flush_remaining().map_err(map_openh264_error)? {
        let timestamp = Some(Duration::from_secs_f64(frame_index as f64 / 30.0));
        frame_index = frame_index.saturating_add(1);
        let frame = convert_frame(&image, timestamp)?;
        if tx.blocking_send(Ok(frame)).is_err() {
            return Ok(());
        }
    }

    Ok(())
}

fn decode_mp4_stream(data: &[u8], tx: Sender<YPlaneResult<YPlaneFrame>>) -> YPlaneResult<()> {
    let size = u64::try_from(data.len()).map_err(|err| {
        YPlaneError::backend_failure(
            BACKEND_NAME,
            format!("MP4 input too large to decode ({err})"),
        )
    })?;
    let cursor = Cursor::new(data);
    let mut reader = Mp4Reader::read_header(cursor, size).map_err(|err| {
        YPlaneError::backend_failure(BACKEND_NAME, format!("failed to parse MP4 header: {err}"))
    })?;

    let track_id = reader
        .tracks()
        .iter()
        .find_map(
            |(id, track)| match (track.track_type(), track.media_type()) {
                (Ok(TrackType::Video), Ok(MediaType::H264)) => Some(*id),
                _ => None,
            },
        )
        .ok_or_else(|| {
            YPlaneError::backend_failure(
                BACKEND_NAME,
                "MP4 file does not contain an H.264 video track",
            )
        })?;

    let track = reader.tracks().get(&track_id).ok_or_else(|| {
        YPlaneError::backend_failure(BACKEND_NAME, "failed to fetch selected MP4 track")
    })?;

    let mut converter = Mp4BitstreamConverter::from_track(track)?;
    let timescale = track.timescale();
    let sample_count = reader.sample_count(track_id).map_err(|err| {
        YPlaneError::backend_failure(
            BACKEND_NAME,
            format!("failed to query MP4 sample count: {err}"),
        )
    })?;

    let (chunk_tx, mut chunk_rx) =
        mpsc::channel::<(usize, Vec<YPlaneResult<YPlaneFrame>>)>(CHUNK_RESULT_CAPACITY);
    let mut ordered = OrderedChunks::new();
    let mut chunk_samples = Vec::new();
    let mut converted = Vec::new();
    let mut chunk_index = 0usize;

    for sample_id in 1..=sample_count {
        if drain_ready_chunks(&mut chunk_rx, &mut ordered, &tx)? {
            return Ok(());
        }

        let Some(sample) = reader.read_sample(track_id, sample_id).map_err(|err| {
            YPlaneError::backend_failure(
                BACKEND_NAME,
                format!("failed to read MP4 sample {sample_id}: {err}"),
            )
        })?
        else {
            continue;
        };

        let is_keyframe = sample.is_sync;

        if is_keyframe && !chunk_samples.is_empty() {
            flush_chunk(
                chunk_index,
                std::mem::take(&mut chunk_samples),
                chunk_tx.clone(),
            );
            chunk_index += 1;
        }

        converter.convert_sample(sample.bytes.as_ref(), &mut converted)?;

        let (sample_timestamp, sample_duration) = if timescale > 0 {
            let ts = Duration::from_secs_f64(sample.start_time as f64 / timescale as f64);
            let dur = Duration::from_secs_f64(sample.duration as f64 / timescale as f64);
            (Some(ts), Some(dur))
        } else {
            (None, None)
        };

        chunk_samples.push(ChunkSample::new(
            &converted,
            sample_timestamp,
            sample_duration,
        ));
    }

    if !chunk_samples.is_empty() {
        flush_chunk(chunk_index, chunk_samples, chunk_tx.clone());
    }
    drop(chunk_tx);

    if drain_ready_chunks(&mut chunk_rx, &mut ordered, &tx)? {
        return Ok(());
    }
    while let Some((index, frames)) = chunk_rx.blocking_recv() {
        if handle_chunk(index, frames, &mut ordered, &tx)? {
            return Ok(());
        }
    }
    flush_ordered(&mut ordered, &tx)?;
    Ok(())
}

fn flush_chunk(
    chunk_index: usize,
    samples: Vec<ChunkSample>,
    chunk_tx: mpsc::Sender<(usize, Vec<YPlaneResult<YPlaneFrame>>)>,
) {
    spawn(move || {
        let results = decode_chunk(samples);
        let _ = chunk_tx.blocking_send((chunk_index, results));
    });
}

fn decode_chunk(samples: Vec<ChunkSample>) -> Vec<YPlaneResult<YPlaneFrame>> {
    let mut frames = Vec::new();
    let mut decoder = match Decoder::new() {
        Ok(decoder) => decoder,
        Err(err) => return vec![Err(map_openh264_error(err))],
    };
    let mut next_frame_timestamp: Option<Duration> = None;
    let mut frame_duration_hint: Option<Duration> = None;

    for sample in samples {
        match decoder.decode(sample.data.as_slice()) {
            Ok(Some(image)) => match convert_frame(&image, sample.timestamp) {
                Ok(frame) => frames.push(Ok(frame)),
                Err(err) => {
                    frames.push(Err(err));
                    return frames;
                }
            },
            Ok(None) => {}
            Err(err) => {
                frames.push(Err(map_openh264_error(err)));
                return frames;
            }
        }

        if let (Some(ts), Some(dur)) = (sample.timestamp, sample.duration) {
            next_frame_timestamp = Some(ts + dur);
            frame_duration_hint = Some(dur);
        }
    }

    match decoder.flush_remaining() {
        Ok(images) => {
            for image in images {
                let timestamp = next_frame_timestamp;
                match convert_frame(&image, timestamp) {
                    Ok(frame) => frames.push(Ok(frame)),
                    Err(err) => {
                        frames.push(Err(err));
                        return frames;
                    }
                }
                if let (Some(ts), Some(dur)) = (timestamp, frame_duration_hint) {
                    next_frame_timestamp = Some(ts + dur);
                }
            }
        }
        Err(err) => frames.push(Err(map_openh264_error(err))),
    }

    frames
}

fn drain_ready_chunks(
    chunk_rx: &mut mpsc::Receiver<(usize, Vec<YPlaneResult<YPlaneFrame>>)>,
    ordered: &mut OrderedChunks,
    tx: &Sender<YPlaneResult<YPlaneFrame>>,
) -> YPlaneResult<bool> {
    loop {
        match chunk_rx.try_recv() {
            Ok((index, frames)) => {
                if handle_chunk(index, frames, ordered, tx)? {
                    return Ok(true);
                }
            }
            Err(TryRecvError::Empty) => break,
            Err(TryRecvError::Disconnected) => break,
        }
    }
    flush_ordered(ordered, tx)
}

fn handle_chunk(
    index: usize,
    frames: Vec<YPlaneResult<YPlaneFrame>>,
    ordered: &mut OrderedChunks,
    tx: &Sender<YPlaneResult<YPlaneFrame>>,
) -> YPlaneResult<bool> {
    ordered.insert(index, frames);
    flush_ordered(ordered, tx)
}

fn flush_ordered(
    ordered: &mut OrderedChunks,
    tx: &Sender<YPlaneResult<YPlaneFrame>>,
) -> YPlaneResult<bool> {
    while let Some(frames) = ordered.pop_ready() {
        for frame in frames {
            match frame {
                Ok(frame) => {
                    if tx.blocking_send(Ok(frame)).is_err() {
                        return Ok(true);
                    }
                }
                Err(err) => {
                    let _ = tx.blocking_send(Err(err));
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

struct OrderedChunks {
    next_index: usize,
    pending: VecDeque<Option<Vec<YPlaneResult<YPlaneFrame>>>>,
}

impl OrderedChunks {
    fn new() -> Self {
        Self {
            next_index: 0,
            pending: VecDeque::new(),
        }
    }

    fn insert(&mut self, index: usize, frames: Vec<YPlaneResult<YPlaneFrame>>) {
        if index < self.next_index {
            return;
        }
        let relative = index - self.next_index;
        if relative >= self.pending.len() {
            self.pending.resize_with(relative + 1, || None);
        }
        self.pending[relative] = Some(frames);
    }

    fn pop_ready(&mut self) -> Option<Vec<YPlaneResult<YPlaneFrame>>> {
        if let Some(slot) = self.pending.front_mut() {
            if let Some(frames) = slot.take() {
                self.pending.pop_front();
                self.next_index = self.next_index.saturating_add(1);
                return Some(frames);
            }
        }
        None
    }
}

struct ChunkSample {
    data: Vec<u8>,
    timestamp: Option<Duration>,
    duration: Option<Duration>,
}

impl ChunkSample {
    fn new(data: &[u8], timestamp: Option<Duration>, duration: Option<Duration>) -> Self {
        Self {
            data: data.to_vec(),
            timestamp,
            duration,
        }
    }
}

struct Mp4BitstreamConverter {
    length_size: u8,
    sps: Vec<Vec<u8>>,
    pps: Vec<Vec<u8>>,
    prefix_emitted: bool,
}

impl Mp4BitstreamConverter {
    fn from_track(track: &Mp4Track) -> YPlaneResult<Self> {
        let avc1 = track
            .trak
            .mdia
            .minf
            .stbl
            .stsd
            .avc1
            .as_ref()
            .ok_or_else(|| {
                YPlaneError::backend_failure(
                    BACKEND_NAME,
                    "video track is missing AVC1 configuration",
                )
            })?;
        let avcc = &avc1.avcc;
        let length_size = avcc.length_size_minus_one + 1;
        if !(1..=4).contains(&length_size) {
            return Err(YPlaneError::backend_failure(
                BACKEND_NAME,
                "unsupported H.264 length size in MP4 stream",
            ));
        }
        let sps: Vec<Vec<u8>> = avcc
            .sequence_parameter_sets
            .iter()
            .map(|nal| nal.bytes.clone())
            .collect();
        let pps: Vec<Vec<u8>> = avcc
            .picture_parameter_sets
            .iter()
            .map(|nal| nal.bytes.clone())
            .collect();
        if sps.is_empty() || pps.is_empty() {
            return Err(YPlaneError::backend_failure(
                BACKEND_NAME,
                "MP4 stream is missing SPS/PPS parameter sets",
            ));
        }
        Ok(Self {
            length_size,
            sps,
            pps,
            prefix_emitted: false,
        })
    }

    fn convert_sample(&mut self, sample: &[u8], out: &mut Vec<u8>) -> YPlaneResult<()> {
        out.clear();
        if !self.prefix_emitted {
            self.write_parameter_sets(out);
            self.prefix_emitted = true;
        }

        let mut stream = sample;
        let length_size = self.length_size as usize;
        while stream.len() >= length_size {
            let mut nal_size = 0usize;
            for _ in 0..length_size {
                nal_size = (nal_size << 8) | stream[0] as usize;
                stream = &stream[1..];
            }
            if nal_size == 0 || nal_size > stream.len() {
                return Err(YPlaneError::backend_failure(
                    BACKEND_NAME,
                    "corrupt MP4 sample encountered while extracting NAL units",
                ));
            }
            let nal = &stream[..nal_size];
            let nal_type = nal[0] & 0x1F;
            if nal_type == 5 {
                self.write_parameter_sets(out);
            }
            out.extend_from_slice(&[0, 0, 0, 1]);
            out.extend_from_slice(nal);
            stream = &stream[nal_size..];
        }

        Ok(())
    }

    fn write_parameter_sets(&self, out: &mut Vec<u8>) {
        for sps in &self.sps {
            out.extend_from_slice(&[0, 0, 0, 1]);
            out.extend_from_slice(sps);
        }
        for pps in &self.pps {
            out.extend_from_slice(&[0, 0, 0, 1]);
            out.extend_from_slice(pps);
        }
    }
}

pub fn boxed_openh264<P: AsRef<Path>>(path: P) -> YPlaneResult<DynYPlaneProvider> {
    Ok(Box::new(OpenH264Provider::open(path)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_returns_error() {
        let result = OpenH264Provider::open("/tmp/nonexistent-file.mp4");
        assert!(result.is_err());
    }
}
