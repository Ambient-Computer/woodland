build: build-wasm

build-cargo:
  cargo build --target=wasm32-unknown-unknown

build-wasm: build-cargo
  wasm-bindgen --target=experimental-nodejs-module --out-dir=wasm-lib target/wasm32-unknown-unknown/debug/woodland.wasm
