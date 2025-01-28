use wasm_bindgen::prelude::*;

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
    console::info("Hello, world!");
}
