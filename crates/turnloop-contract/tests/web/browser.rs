#![cfg(all(target_arch = "wasm32", target_os = "unknown"))]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);
#[path = "web_contract.rs"]
mod web_contract;
