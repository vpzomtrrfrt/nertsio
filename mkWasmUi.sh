#!/bin/bash
cargo build --target wasm32-unknown-unknown -p nertsio_ui --release

mkdir -p target/wasm32-unknown-unknown/wbindgen
wasm-bindgen --target web --out-dir target/wasm32-unknown-unknown/wbindgen/ target/wasm32-unknown-unknown/release/nertsio_ui.wasm

mkdir -p target/wasm32-unknown-unknown/dist
cp target/wasm32-unknown-unknown/wbindgen/nertsio_ui.js target/wasm32-unknown-unknown/dist/
cp target/wasm32-unknown-unknown/wbindgen/nertsio_ui_bg.wasm target/wasm32-unknown-unknown/dist/
cp ~/.cargo/registry/src/github.com-1ecc6299db9ec823/macroquad-0.3.10/js/mq_js_bundle.js target/wasm32-unknown-unknown/dist/
cp misc/index.html target/wasm32-unknown-unknown/dist/

sed -i "s/import \* as __wbg_star0 from 'env';//" target/wasm32-unknown-unknown/dist/nertsio_ui.js
sed -i "s/let wasm;/let wasm; export const set_wasm = (w) => wasm = w;/" target/wasm32-unknown-unknown/dist/nertsio_ui.js
sed -i "s/imports\['env'\] = __wbg_star0;/return imports.wbg\;/" target/wasm32-unknown-unknown/dist/nertsio_ui.js
