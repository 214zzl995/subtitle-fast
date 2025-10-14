use std::path::PathBuf;

use crate::output::error::OutputError;
use crate::output::types::{FrameJsonRecord, SegmentJsonRecord};
use crate::settings::JsonDumpSettings;
use tokio::fs;

pub(crate) struct JsonOutput {
    directory: PathBuf,
    segments_filename: String,
    frames_filename: String,
    pretty: bool,
}

impl JsonOutput {
    pub(crate) fn new(settings: JsonDumpSettings) -> Self {
        Self {
            directory: settings.dir,
            segments_filename: settings.segments_filename,
            frames_filename: settings.frames_filename,
            pretty: settings.pretty,
        }
    }

    pub(crate) async fn write(
        &self,
        frames: &[FrameJsonRecord],
        segments: &[SegmentJsonRecord],
    ) -> Result<(), OutputError> {
        let frames_path = self.directory.join(&self.frames_filename);
        let segments_path = self.directory.join(&self.segments_filename);

        write_json(frames_path, frames, self.pretty).await?;
        write_json(segments_path, segments, self.pretty).await?;
        Ok(())
    }
}

async fn write_json<T>(path: PathBuf, data: &T, pretty: bool) -> Result<(), OutputError>
where
    T: serde::Serialize + ?Sized,
{
    let encoded = if pretty {
        serde_json::to_vec_pretty(data)?
    } else {
        serde_json::to_vec(data)?
    };
    fs::write(path, encoded).await?;
    Ok(())
}
