//! WASI Serverless HTTP Runtime (`coral wasm serve`) — pre-compiles a `.wasm`/`.wat`
//! module once and instantiates a fresh, fuel- and memory-capped WASI sandbox per HTTP
//! request in microseconds.
//!
//! Supports:
//! - Standard WASI Preview 1 HTTP/CGI bridge (`REQUEST_METHOD`, `REQUEST_URI`, `PATH_INFO`,
//!   `QUERY_STRING`, `HTTP_*` env vars, request body on `stdin`, optional `Status:` /
//!   `Content-Type:` response headers on `stdout`).
//! - `wasi:http` / `coral_http` host imports (`coral_http::set_status`, `coral_http::set_header`).

mod handler;
mod response;
mod server;
#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};
use wasmi::{Engine, Module};

pub use response::parse_cgi_or_raw_response;
pub use server::serve_wasm_http;

/// Parsed incoming HTTP request delivered to a Wasm handler instance.
#[derive(Debug, Clone)]
pub struct WasmHttpRequest {
    pub method: String,
    pub path: String,
    pub query: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// Response & per-request execution telemetry produced by a Wasm handler instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WasmHttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub fuel_consumed: u64,
    pub duration_usec: u64,
    pub trapped: Option<String>,
}

/// Resource limits for each isolated per-request Wasm HTTP sandbox.
#[derive(Debug, Clone)]
pub struct WasmHttpOptions {
    pub fuel_limit: u64,
    pub memory_limit_mb: usize,
    pub extra_envs: Vec<String>,
}

impl Default for WasmHttpOptions {
    fn default() -> Self {
        Self {
            fuel_limit: 50_000_000,
            memory_limit_mb: 32,
            extra_envs: Vec::new(),
        }
    }
}

/// Pre-compiled WebAssembly HTTP microservice ready to handle concurrent requests with
/// fresh per-request sandbox isolation.
pub struct WasmHttpHandler {
    engine: Engine,
    module: Module,
    opts: WasmHttpOptions,
}
