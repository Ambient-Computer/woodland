use wasm_bindgen::prelude::*;

mod console;
mod counter;
mod oplog;

#[wasm_bindgen(start)]
pub fn main() -> () {
    let mut counter = counter::Counter::new();
    let mut log = oplog::OperationLog::new();

    log.commit(vec![
        counter.increment(1),
        counter.set(10),
        counter.decrement(1),
        counter.increment(4),
    ]);

    log.derive(&mut counter);

    console::info(format!("Value: {:?}", counter.0).as_ref());
}
