#![no_main]
use coral_wasm::http::{WasmHttpOptions, WasmHttpHandler, parse_cgi_or_raw_response};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = parse_cgi_or_raw_response(data);
    if data.len() <= 64 * 1024 {
        let _ = WasmHttpHandler::from_bytes(
            data,
            WasmHttpOptions {
                fuel_limit: 50_000,
                memory_limit_mb: 2,
                extra_envs: vec![],
            },
        );
    }
});
