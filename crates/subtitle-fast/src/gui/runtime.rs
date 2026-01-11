use std::future::Future;
use std::sync::OnceLock;

use tokio::runtime::Handle;
use tokio::task::JoinHandle;

static RUNTIME: OnceLock<Handle> = OnceLock::new();

pub fn init(handle: Handle) {
    let _ = RUNTIME.set(handle);
}

pub fn spawn<F>(future: F) -> Option<JoinHandle<F::Output>>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    RUNTIME.get().map(|handle| handle.spawn(future))
}
