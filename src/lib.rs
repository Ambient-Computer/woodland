use wasm_bindgen::prelude::*;

mod counter;
mod oplog;

mod console {
    use super::*;

    #[wasm_bindgen]
    extern "C" {
        #[wasm_bindgen(js_namespace = console)]
        pub fn info(s: &str);
    }
}

#[wasm_bindgen(start)]
pub fn main() -> () {
    let counter = counter::Counter::new();
    let mut log = oplog::OperationLog::new();
    let changes = log.commit(vec![
        counter.increment(1),
        counter.decrement(1),
        counter.set(10),
    ]);

    console::info(format!("Changes: {:?}", changes).as_ref());
}
