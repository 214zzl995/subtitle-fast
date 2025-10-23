pub mod detection;
pub mod sampler;
pub mod sorter;
pub mod subtitle_gen;

use std::pin::Pin;

use futures_util::Stream;

pub struct StreamBundle<T> {
    pub stream: Pin<Box<dyn Stream<Item = T> + Send>>,
    pub total_frames: Option<u64>,
}

impl<T> StreamBundle<T> {
    pub fn new(stream: Pin<Box<dyn Stream<Item = T> + Send>>, total_frames: Option<u64>) -> Self {
        Self {
            stream,
            total_frames,
        }
    }
}
