use std::collections::BTreeMap;
use std::pin::Pin;

use futures_util::{Stream, StreamExt};

use super::StreamBundle;
use subtitle_fast_decoder::{YPlaneFrame, YPlaneResult};

pub struct FrameSorter;

impl FrameSorter {
    pub fn new() -> Self {
        Self
    }

    pub fn attach(
        self,
        input: StreamBundle<YPlaneResult<YPlaneFrame>>,
    ) -> StreamBundle<YPlaneResult<YPlaneFrame>> {
        let StreamBundle {
            stream,
            total_frames,
        } = input;

        let state = SorterState {
            upstream: stream,
            pool: FramePool::default(),
            finished: false,
        };

        let stream = Box::pin(futures_util::stream::unfold(state, SorterState::next));
        StreamBundle::new(stream, total_frames)
    }
}

struct SorterState {
    upstream: Pin<Box<dyn Stream<Item = YPlaneResult<YPlaneFrame>> + Send>>,
    pool: FramePool,
    finished: bool,
}

impl SorterState {
    async fn next(mut state: SorterState) -> Option<(YPlaneResult<YPlaneFrame>, SorterState)> {
        loop {
            if let Some(frame) = state.pool.pop_next() {
                return Some((Ok(frame), state));
            }

            if state.finished {
                return None;
            }

            match state.upstream.as_mut().next().await {
                Some(Ok(frame)) => {
                    state.pool.insert(frame);
                }
                Some(Err(err)) => {
                    state.finished = true;
                    return Some((Err(err), state));
                }
                None => {
                    state.finished = true;
                    if let Some(frame) = state.pool.pop_next() {
                        return Some((Ok(frame), state));
                    }
                    return None;
                }
            }
        }
    }
}

#[derive(Default)]
struct FramePool {
    pending: BTreeMap<u64, YPlaneFrame>,
    fallback_index: u64,
}

impl FramePool {
    fn insert(&mut self, frame: YPlaneFrame) {
        let key = frame.frame_index().unwrap_or_else(|| {
            let key = self.fallback_index;
            self.fallback_index = self.fallback_index.saturating_add(1);
            key
        });

        self.pending.entry(key).or_insert(frame);
    }

    fn pop_next(&mut self) -> Option<YPlaneFrame> {
        let key = self.pending.keys().next().copied()?;
        self.pending.remove(&key)
    }
}
