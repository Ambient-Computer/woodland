//! Somewhere to run a step.

use std::pin::pin;
use std::task::{Context, Poll, Waker};

/// Runs a future to completion on this thread. A store in memory
/// answers at once, so nothing here ever waits. An application with a
/// store that does wait uses its own runtime.
pub(crate) fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let mut context = Context::from_waker(Waker::noop());
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
            return output;
        }
    }
}
