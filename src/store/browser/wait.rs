//! Waiting for the browser to call back.

use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::{IdbRequest, IdbTransaction};

#[derive(Debug)]
struct Slot<T> {
    answer: Option<T>,
    waker: Option<Waker>,
}

/// Something a callback will say, once.
#[derive(Debug)]
pub(super) struct Answer<T>(Rc<RefCell<Slot<T>>>);

/// The callback's end of an [`Answer`].
#[derive(Debug)]
pub(super) struct Give<T>(Rc<RefCell<Slot<T>>>);

impl<T> Clone for Give<T> {
    fn clone(&self) -> Self {
        Self(Rc::clone(&self.0))
    }
}

pub(super) fn answer<T>() -> (Answer<T>, Give<T>) {
    let slot = Rc::new(RefCell::new(Slot {
        answer: None,
        waker: None,
    }));

    (Answer(Rc::clone(&slot)), Give(slot))
}

impl<T> Give<T> {
    /// Only the first answer counts.
    pub(super) fn give(&self, answer: T) {
        let mut slot = self.0.borrow_mut();
        if slot.answer.is_none() {
            slot.answer = Some(answer);
        }
        if let Some(waker) = slot.waker.take() {
            waker.wake();
        }
    }
}

impl<T> Future for Answer<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<T> {
        let mut slot = self.0.borrow_mut();
        if let Some(answer) = slot.answer.take() {
            return Poll::Ready(answer);
        }
        slot.waker = Some(context.waker().clone());

        Poll::Pending
    }
}

type Callback = Closure<dyn FnMut()>;

/// Whether a request succeeded, once it's done either. The callbacks
/// are in place from the moment this is made, not from when it's
/// first waited on.
pub(super) struct Settled {
    request: IdbRequest,
    answer: Answer<bool>,
    _callbacks: [Callback; 2],
}

impl std::fmt::Debug for Settled {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Settled").finish_non_exhaustive()
    }
}

pub(super) fn settled(request: IdbRequest) -> Settled {
    let (answer, give) = answer();
    let succeeded = {
        let give = give.clone();
        Callback::new(move || give.give(true))
    };
    let failed = Callback::new(move || give.give(false));
    request.set_onsuccess(Some(succeeded.as_ref().unchecked_ref()));
    request.set_onerror(Some(failed.as_ref().unchecked_ref()));

    Settled {
        request,
        answer,
        _callbacks: [succeeded, failed],
    }
}

impl Settled {
    /// The request, once it's done. `None` if it failed.
    pub(super) async fn done(self) -> Option<IdbRequest> {
        let Self {
            request,
            answer,
            _callbacks,
        } = self;
        let succeeded = answer.await;
        request.set_onsuccess(None);
        request.set_onerror(None);

        succeeded.then_some(request)
    }
}

/// Whether a transaction committed, once it has or can't. `then` runs
/// inside the browser's own callback if it did, before anything that
/// was asked of the database afterwards is answered.
pub(super) async fn committed(transaction: &IdbTransaction, then: impl FnOnce() + 'static) -> bool {
    let (answer, give) = answer();
    let mut then = Some(then);
    let completed = {
        let give = give.clone();
        Callback::new(move || {
            if let Some(then) = then.take() {
                then();
            }
            give.give(true);
        })
    };
    let failed = Callback::new(move || give.give(false));
    transaction.set_oncomplete(Some(completed.as_ref().unchecked_ref()));
    transaction.set_onabort(Some(failed.as_ref().unchecked_ref()));
    transaction.set_onerror(Some(failed.as_ref().unchecked_ref()));

    let committed = answer.await;
    transaction.set_oncomplete(None);
    transaction.set_onabort(None);
    transaction.set_onerror(None);

    committed
}
