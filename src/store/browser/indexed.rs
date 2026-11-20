//! `IndexedDB`, held to the store's contract.
//!
//! Every request a call makes is made before the call first waits, in
//! one transaction. `IndexedDB` closes a transaction whose owner waits
//! on anything else, and nothing here gives it the chance.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::fmt;
use std::rc::Rc;

use js_sys::{Array, Function, Object, Promise, Reflect, Uint8Array};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{
    IdbCursorDirection, IdbCursorWithValue, IdbDatabase, IdbFactory, IdbKeyRange, IdbObjectStore,
    IdbRequest, IdbTransaction,
};

use super::history::{Before, History, restore};
use super::wait::{answer, committed, settled};
use crate::store::{Entry, Order, Store, StoreError, View, Write};

/// The one object store everything is kept in.
const KEPT: &str = "woodland";

fn failed(error: &JsValue) -> StoreError {
    let message = Reflect::get(error, &"message".into()).ok();
    let message = message.and_then(|it| it.as_string());

    StoreError::new(message.unwrap_or_else(|| format!("{error:?}")))
}

fn bytes(bytes: &[u8]) -> JsValue {
    Uint8Array::from(bytes).into()
}

fn read(value: &JsValue) -> Vec<u8> {
    Uint8Array::new(value).to_vec()
}

fn global(name: &str) -> Option<JsValue> {
    let found = Reflect::get(&js_sys::global(), &name.into()).ok()?;

    (!found.is_undefined() && !found.is_null()).then_some(found)
}

/// A hold on a name, for as long as this lives. The browser lets it
/// go when the page does.
struct Owned {
    release: Rc<RefCell<Option<Function>>>,
    _granted: Closure<dyn FnMut(JsValue) -> JsValue>,
}

impl Drop for Owned {
    fn drop(&mut self) {
        if let Some(release) = self.release.borrow_mut().take()
            && let Err(error) = release.call0(&JsValue::UNDEFINED)
        {
            tracing::warn!(?error, "couldn't let go of a store");
        }
    }
}

/// Takes the name for this store alone, where the browser can say
/// whether another has it: another tab, or a worker. `None` where it
/// can't.
async fn own(name: &str) -> Result<Option<Owned>, StoreError> {
    let locks = global("navigator").and_then(|it| Reflect::get(&it, &"locks".into()).ok());
    let Some(locks) = locks.filter(|it| !it.is_undefined()) else {
        return Ok(None);
    };
    let request = Reflect::get(&locks, &"request".into());
    let request: Function = request
        .and_then(JsCast::dyn_into)
        .map_err(|it| failed(&it))?;

    let (granted, give) = answer();
    let release = Rc::new(RefCell::new(None));
    let callback = {
        let release = Rc::clone(&release);
        Closure::<dyn FnMut(JsValue) -> JsValue>::new(move |lock: JsValue| {
            if lock.is_null() {
                give.give(false);
                return JsValue::UNDEFINED;
            }

            // The name is held until this is resolved.
            let held = Promise::new(&mut |resolve, _| {
                *release.borrow_mut() = Some(resolve);
            });
            give.give(true);

            held.into()
        })
    };
    let options = Object::new();
    Reflect::set(&options, &"ifAvailable".into(), &true.into()).map_err(|it| failed(&it))?;
    let name = format!("woodland/{name}");
    request
        .call3(&locks, &name.into(), &options, callback.as_ref())
        .map_err(|it| failed(&it))?;

    if !granted.await {
        return Err(StoreError::busy("another store has it open"));
    }

    Ok(Some(Owned {
        release,
        _granted: callback,
    }))
}

/// A store in the browser's own database, `IndexedDB`.
///
/// It's kept by the browser for the site, under a name. The browser
/// may take it back: see what the library does about a store that's
/// [lost].
///
/// [lost]: crate::store#losing-a-store
pub struct BrowserStore {
    database: IdbDatabase,
    history: Rc<RefCell<History>>,
    _owned: Option<Owned>,
}

impl fmt::Debug for BrowserStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BrowserStore")
            .field("name", &self.database.name())
            .finish_non_exhaustive()
    }
}

impl Drop for BrowserStore {
    fn drop(&mut self) {
        self.database.close();
    }
}

impl BrowserStore {
    /// Opens the store called `name`, making it if it isn't there.
    ///
    /// What it makes in the origin is one `IndexedDB` database called
    /// `name`, at version 1, and while it's open a Web Lock called
    /// `woodland/` and then `name`. Pick a name nothing else in the
    /// origin uses for a database.
    ///
    /// # Errors
    ///
    /// If another store has it open, in this tab, another, or a
    /// worker: that's [`Busy`]. If the origin already has a database
    /// of that name at a later version, which is something else's:
    /// that's [`Mismatched`]. Or if the browser won't open it.
    ///
    /// [`Busy`]: crate::StoreFault::Busy
    /// [`Mismatched`]: crate::StoreFault::Mismatched
    pub async fn open(name: &str) -> Result<Self, StoreError> {
        let owned = own(name).await?;
        let factory = global("indexedDB").and_then(|it| it.dyn_into::<IdbFactory>().ok());
        let factory = factory.ok_or_else(|| StoreError::new("there's no IndexedDB here"))?;
        let request = factory.open_with_u32(name, 1).map_err(|it| failed(&it))?;

        // A view reads the object store, so it has to be there from
        // the start.
        let create = {
            let request = request.clone();
            Closure::<dyn FnMut()>::new(move || {
                let database = request.result().and_then(JsCast::dyn_into::<IdbDatabase>);
                let made = database.and_then(|it| it.create_object_store(KEPT));
                if let Err(error) = made {
                    tracing::error!(?error, "couldn't make a store");
                }
            })
        };
        request.set_onupgradeneeded(Some(create.as_ref().unchecked_ref()));
        let opened = settled(request.clone().into()).done().await;
        request.set_onupgradeneeded(None);

        // The name is the origin's to share. A database something else
        // made under it, at a later version, isn't opened at this one.
        let refusal = || match request.error() {
            Ok(Some(error)) if error.name() == "VersionError" => StoreError::mismatched(
                "the origin has a database of that name that isn't a store: use another name",
            ),
            _ => StoreError::new("the browser wouldn't open it"),
        };
        let database = opened.ok_or_else(refusal)?;
        let database = database.result().and_then(JsCast::dyn_into::<IdbDatabase>);

        Ok(Self {
            database: database.map_err(|it| failed(&it))?,
            history: Rc::default(),
            _owned: owned,
        })
    }

    /// A transaction that isn't done until it's on disk.
    fn writing(&self) -> Result<IdbTransaction, JsValue> {
        let options = Object::new();
        Reflect::set(&options, &"durability".into(), &"strict".into())?;
        let open: Function = Reflect::get(&self.database, &"transaction".into())?.dyn_into()?;
        let begun = open.call3(&self.database, &KEPT.into(), &"readwrite".into(), &options)?;

        begun.dyn_into()
    }

    /// Asks for everything in `batch`, and first, if a view is still
    /// about, for what it replaces.
    fn ask(
        &self,
        transaction: &IdbTransaction,
        batch: &[Write],
    ) -> Result<Vec<(Vec<u8>, IdbRequest)>, JsValue> {
        let kept = transaction.object_store(KEPT)?;
        let key = |write: &Write| match write {
            Write::Put { key, .. } | Write::Delete { key } => key.clone(),
        };
        let keys: BTreeSet<Vec<u8>> = batch.iter().map(key).collect();
        let watched = self.history.borrow().watched();
        let mut replaced = Vec::new();
        for key in keys.into_iter().filter(|_| watched) {
            let request = kept.get(&bytes(&key))?;
            replaced.push((key, request));
        }

        for write in batch {
            match write {
                Write::Put { key, value } => kept.put_with_key(&bytes(value), &bytes(key))?,
                Write::Delete { key } => kept.delete(&bytes(key))?,
            };
        }

        Ok(replaced)
    }
}

impl Store for BrowserStore {
    type View = BrowserView;

    /// The browser doesn't say what its database has freed, and
    /// gives it back when it sees fit: there's nothing to tell and
    /// nothing to ask for.
    fn unreturned(&self) -> impl Future<Output = Result<u64, StoreError>> {
        std::future::ready(Ok(0))
    }

    fn reclaim(&mut self) -> impl Future<Output = Result<(), StoreError>> {
        std::future::ready(Ok(()))
    }

    async fn apply(&mut self, batch: Vec<Write>) -> Result<(), StoreError> {
        let transaction = self.writing().map_err(|it| failed(&it))?;
        let replaced = match self.ask(&transaction, &batch) {
            Ok(replaced) => replaced,
            Err(error) => {
                // Some of it may have been asked for. None of it is to
                // happen.
                if let Err(error) = transaction.abort() {
                    tracing::warn!(?error, "couldn't take a batch back");
                }

                return Err(failed(&error));
            }
        };

        // What the batch replaced is on record before anything can
        // read what it wrote.
        let history = Rc::clone(&self.history);
        let record = move || {
            let held = |(key, request): (Vec<u8>, IdbRequest)| {
                let held = request.result().ok().filter(|it| !it.is_undefined());

                (key, held.as_ref().map(read))
            };
            let before: Before = replaced.into_iter().map(held).collect();
            history.borrow_mut().applied(before);
        };
        if committed(&transaction, record).await {
            return Ok(());
        }

        let reason = transaction.error().map(|it| it.message());

        Err(StoreError::new(reason.unwrap_or_else(|| {
            "the browser didn't keep a batch".into()
        })))
    }

    fn view(&self) -> BrowserView {
        BrowserView {
            database: self.database.clone(),
            history: Rc::clone(&self.history),
            generation: self.history.borrow_mut().hold(),
        }
    }
}

/// A [`BrowserStore`] at one moment.
pub struct BrowserView {
    database: IdbDatabase,
    history: Rc<RefCell<History>>,
    generation: u64,
}

impl fmt::Debug for BrowserView {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BrowserView")
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

impl Clone for BrowserView {
    fn clone(&self) -> Self {
        Self {
            database: self.database.clone(),
            history: Rc::clone(&self.history),
            generation: self.history.borrow_mut().hold_at(self.generation),
        }
    }
}

impl Drop for BrowserView {
    fn drop(&mut self) {
        self.history.borrow_mut().release(self.generation);
    }
}

/// Reads rows off a cursor as the browser hands them over, up to
/// `limit` of them.
fn walk(request: IdbRequest, limit: usize) -> impl Future<Output = Result<Vec<Entry>, StoreError>> {
    let (walked, give) = answer();
    let rows = Rc::new(RefCell::new(Vec::new()));
    let step = {
        let (rows, request) = (Rc::clone(&rows), request.clone());
        let give = give.clone();
        Closure::<dyn FnMut()>::new(move || {
            let cursor = request.result().ok().filter(|it| !it.is_null());
            let cursor = cursor.and_then(|it| it.dyn_into::<IdbCursorWithValue>().ok());
            let Some(cursor) = cursor else {
                return give.give(Ok(()));
            };

            let row = cursor
                .key()
                .and_then(|key| Ok((read(&key), read(&cursor.value()?))));
            let more = row.and_then(|row| {
                rows.borrow_mut().push(row);
                if rows.borrow().len() < limit {
                    cursor.continue_()?;
                    return Ok(true);
                }

                Ok(false)
            });
            match more {
                Ok(true) => {}
                Ok(false) => give.give(Ok(())),
                Err(error) => give.give(Err(failed(&error))),
            }
        })
    };
    let halt = Closure::<dyn FnMut()>::new(move || {
        give.give(Err(StoreError::new("the browser stopped a read")));
    });
    request.set_onsuccess(Some(step.as_ref().unchecked_ref()));
    request.set_onerror(Some(halt.as_ref().unchecked_ref()));

    async move {
        let walked = walked.await;
        request.set_onsuccess(None);
        request.set_onerror(None);
        drop((step, halt));
        walked?;

        Ok(rows.take())
    }
}

impl BrowserView {
    fn kept(&self) -> Result<IdbObjectStore, JsValue> {
        self.database.transaction_with_str(KEPT)?.object_store(KEPT)
    }

    /// The first `limit` entries of a range as the database has it
    /// now.
    async fn now(
        &self,
        start: &[u8],
        end: Option<&[u8]>,
        order: Order,
        limit: usize,
    ) -> Result<Vec<Entry>, StoreError> {
        let count = u32::try_from(limit).unwrap_or(u32::MAX);
        let asked = (|| {
            let kept = self.kept()?;
            let range = match end {
                Some(end) => IdbKeyRange::bound_with_lower_open_and_upper_open(
                    &bytes(start),
                    &bytes(end),
                    false,
                    true,
                )?,
                None => IdbKeyRange::lower_bound(&bytes(start))?,
            };

            // Everything at once is quicker than a row at a time, and
            // only goes one way.
            Ok::<_, JsValue>(match order {
                Order::Ascending => {
                    let keys = kept.get_all_keys_with_key_and_limit(&range, count)?;
                    let values = kept.get_all_with_key_and_limit(&range, count)?;

                    Ok((settled(keys), settled(values)))
                }

                Order::Descending => Err(walk(
                    kept.open_cursor_with_range_and_direction(&range, IdbCursorDirection::Prev)?,
                    limit,
                )),
            })
        })();

        let (keys, values) = match asked.map_err(|it| failed(&it))? {
            Ok(both) => both,
            Err(walking) => return walking.await,
        };
        let (keys, values) = (keys.done().await, values.done().await);
        let (Some(keys), Some(values)) = (keys, values) else {
            return Err(StoreError::new("the browser stopped a read"));
        };
        let list = |request: IdbRequest| request.result().map(|it| Array::from(&it));
        let keys = list(keys).map_err(|it| failed(&it))?;
        let values = list(values).map_err(|it| failed(&it))?;

        Ok(keys
            .iter()
            .zip(values.iter())
            .map(|(key, value)| (read(&key), read(&value)))
            .collect())
    }
}

impl View for BrowserView {
    async fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
        let asked = self.kept().and_then(|kept| kept.get(&bytes(key)));
        let asked = settled(asked.map_err(|it| failed(&it))?);
        let Some(request) = asked.done().await else {
            return Err(StoreError::new("the browser stopped a read"));
        };

        let now = request.result().map_err(|it| failed(&it))?;
        let now = (!now.is_undefined()).then(|| read(&now));

        Ok(self.history.borrow().was(self.generation, key, now))
    }

    async fn scan(
        &self,
        start: &[u8],
        end: Option<&[u8]>,
        order: Order,
        limit: usize,
    ) -> Result<Vec<Entry>, StoreError> {
        if limit == 0 || end.is_some_and(|end| end <= start) {
            return Ok(Vec::new());
        }

        // Whatever was written since this view was taken may have put
        // entries in the page that it has to give up, so the database
        // is asked for that many more. A batch can land while it's
        // answering, and then it's asked again.
        loop {
            let spare = self
                .history
                .borrow()
                .were(self.generation, start, end)
                .len();
            let reach = limit.saturating_add(spare);
            let now = self.now(start, end, order, reach).await?;
            let since = self.history.borrow().were(self.generation, start, end);
            if since.len() <= spare || now.len() < reach {
                return Ok(restore(now, since, order, limit));
            }
        }
    }
}
