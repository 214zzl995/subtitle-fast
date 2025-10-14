use std::pin::Pin;

pub mod detection;
pub mod sampler;
pub mod sorter;
pub mod subtitle_gen;

use futures_util::Stream;

pub struct StageInput<I> {
    pub stream: Pin<Box<dyn Stream<Item = I> + Send>>,
    pub total_frames: Option<u64>,
}

pub struct StageOutput<O> {
    pub stream: Pin<Box<dyn Stream<Item = O> + Send>>,
    pub total_frames: Option<u64>,
}

pub trait PipelineStage<I>: Send + 'static {
    type Output;

    #[allow(dead_code)]
    fn name(&self) -> &'static str;

    fn apply(self: Box<Self>, input: StageInput<I>) -> StageOutput<Self::Output>;
}
