//! Test-only support: the cast of devices, and in-memory stand-ins for the
//! network and storage that a real application would provide.

pub(crate) mod cluster;
pub(crate) mod peers;
pub(crate) mod store;

use std::pin::pin;
use std::task::{Context, Poll, Waker};

/// Runs a future to completion on this thread.
///
/// Nothing in tests waits for anything outside the test, so a future
/// that isn't ready has only to be asked again.
pub(crate) fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let mut context = Context::from_waker(Waker::noop());
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
            return output;
        }
    }
}
