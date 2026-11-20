//! WASI Serverless HTTP Runtime (`coral wasm serve`) — pre-compiles a `.wasm`/`.wat`
//! module once and instantiates a fresh, fuel- and memory-capped WASI sandbox per HTTP
//! request in microseconds.
//!
//! Supports:
//! - Standard WASI Preview 1 HTTP/CGI bridge (`REQUEST_METHOD`, `REQUEST_URI`, `PATH_INFO`,
//!   `QUERY_STRING`, `HTTP_*` env vars, request body on `stdin`, optional `Status:` /
//!   `Content-Type:` response headers on `stdout`).
//! - `wasi:http` / `coral_http` host imports (`coral_http::set_status`, `coral_http::set_header`).

use crate::wasi::{WasiContext, register_wasi_preview1};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use wasmi::{Caller, Config, Engine, Linker, Module, Store, StoreLimitsBuilder};

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

impl WasmHttpHandler {
    /// Compiles and validates a `.wasm` binary or `.wat` text module once.
    pub fn from_bytes(source: &[u8], opts: WasmHttpOptions) -> Result<Self> {
        let mut cfg = Config::default();
        if opts.fuel_limit > 0 {
            cfg.consume_fuel(true);
        }
        let engine = Engine::new(&cfg);
        let module = Module::new(&engine, source).context("compile Wasm HTTP handler module")?;
        Ok(Self {
            engine,
            module,
            opts,
        })
    }

    /// Instantiates a fresh isolated Wasm sandbox for a single HTTP request.
    pub fn handle(&self, req: &WasmHttpRequest) -> Result<WasmHttpResponse> {
        let start = Instant::now();
        let mem_bytes = self.opts.memory_limit_mb.max(1).saturating_mul(1024 * 1024);
        let limits = StoreLimitsBuilder::new()
            .memory_size(mem_bytes)
            .instances(4)
            .tables(8)
            .build();

        let full_uri = if req.query.is_empty() {
            req.path.clone()
        } else {
            format!("{}?{}", req.path, req.query)
        };

        let mut envs = Vec::with_capacity(self.opts.extra_envs.len() + req.headers.len() + 6);
        envs.push(format!("REQUEST_METHOD={}", req.method));
        envs.push(format!("REQUEST_URI={full_uri}"));
        envs.push(format!("PATH_INFO={}", req.path));
        envs.push(format!("QUERY_STRING={}", req.query));
        envs.push(format!("CONTENT_LENGTH={}", req.body.len()));
        envs.push("SERVER_PROTOCOL=HTTP/1.1".into());
        for (k, v) in &req.headers {
            let env_key = k.to_ascii_uppercase().replace('-', "_");
            envs.push(format!("HTTP_{env_key}={v}"));
        }
        envs.extend(self.opts.extra_envs.iter().cloned());

        let ctx = WasiContext {
            args: vec!["handler.wasm".into(), req.method.clone(), full_uri],
            envs,
            stdin: VecDeque::from(req.body.clone()),
            stdout: Vec::new(),
            stderr: Vec::new(),
            passthrough_stdio: false,
            exit_code: None,
            limits,
        };

        let mut store = Store::new(&self.engine, ctx);
        store.limiter(|s| &mut s.limits);
        if self.opts.fuel_limit > 0 {
            store.set_fuel(self.opts.fuel_limit)?;
        }

        // Shared mutable response metadata updated by optional `coral_http` / `wasi:http` imports.
        let host_status = Arc::new(std::sync::Mutex::new(None::<u16>));
        let host_headers = Arc::new(std::sync::Mutex::new(Vec::<(String, String)>::new()));

        let mut linker = Linker::<WasiContext>::new(&self.engine);
        register_wasi_preview1(&mut linker)?;

        let status_ref = host_status.clone();
        linker.func_wrap("coral_http", "set_status", move |code: i32| {
            if let Ok(mut g) = status_ref.lock()
                && (100..=599).contains(&code)
            {
                *g = Some(code as u16);
            }
        })?;

        let hdr_ref = host_headers.clone();
        linker.func_wrap(
            "coral_http",
            "set_header",
            move |caller: Caller<'_, WasiContext>,
                  k_ptr: i32,
                  k_len: i32,
                  v_ptr: i32,
                  v_len: i32|
                  -> i32 {
                let Some(mem) = caller.get_export("memory").and_then(|e| e.into_memory()) else {
                    return -1;
                };
                let data = mem.data(&caller);
                let (kp, kl, vp, vl) = (k_ptr as usize, k_len as usize, v_ptr as usize, v_len as usize);
                let Some(kb) = data.get(kp..kp.saturating_add(kl)) else {
                    return -1;
                };
                let Some(vb) = data.get(vp..vp.saturating_add(vl)) else {
                    return -1;
                };
                if let Ok(mut g) = hdr_ref.lock()
                    && g.len() < 64
                {
                    g.push((
                        String::from_utf8_lossy(kb).into_owned(),
                        String::from_utf8_lossy(vb).into_owned(),
                    ));
                }
                0
            },
        )?;

        let instance = linker.instantiate_and_start(&mut store, &self.module)?;
        let call_res = if let Ok(f) = instance.get_typed_func::<(), ()>(&store, "wasi_http_handle") {
            f.call(&mut store, ()).map(|()| 0)
        } else if let Ok(f) = instance.get_typed_func::<(), ()>(&store, "_start") {
            f.call(&mut store, ()).map(|()| 0)
        } else if let Ok(f) = instance.get_typed_func::<(), i32>(&store, "_start") {
            f.call(&mut store, ())
        } else if let Ok(f) = instance.get_typed_func::<(), ()>(&store, "main") {
            f.call(&mut store, ()).map(|()| 0)
        } else {
            bail!("Wasm HTTP module must export `wasi_http_handle`, `_start`, or `main`");
        };

        let fuel_consumed = if self.opts.fuel_limit > 0 {
            self.opts.fuel_limit.saturating_sub(store.get_fuel().unwrap_or(0))
        } else {
            0
        };
        let duration_usec = start.elapsed().as_micros() as u64;

        let mut trapped = None;
        if let Err(err) = call_res
            && err.i32_exit_status().is_none()
            && store.data().exit_code.is_none()
        {
            trapped = Some(err.to_string());
        }

        if let Some(trap_msg) = trapped {
            return Ok(WasmHttpResponse {
                status: 500,
                headers: vec![("Content-Type".into(), "text/plain; charset=utf-8".into())],
                body: format!("coral-wasm trap: {trap_msg}\n").into_bytes(),
                fuel_consumed,
                duration_usec,
                trapped: Some(trap_msg),
            });
        }

        let raw_stdout = store.data().stdout.clone();
        let (mut status, mut headers, body) = parse_cgi_or_raw_response(&raw_stdout);
        if let Ok(g) = host_status.lock()
            && let Some(explicit) = *g
        {
            status = explicit;
        }
        if let Ok(g) = host_headers.lock() {
            for (k, v) in g.iter() {
                headers.push((k.clone(), v.clone()));
            }
        }
        if !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("content-type")) {
            headers.push(("Content-Type".into(), "text/plain; charset=utf-8".into()));
        }

        Ok(WasmHttpResponse {
            status,
            headers,
            body,
            fuel_consumed,
            duration_usec,
            trapped: None,
        })
    }
}

/// Starts an async HTTP/1.1 serverless host backed by a pre-compiled `WasmHttpHandler`.
pub async fn serve_wasm_http(
    listener: TcpListener,
    handler: Arc<WasmHttpHandler>,
    mut shutdown: oneshot::Receiver<()>,
) -> Result<()> {
    loop {
        tokio::select! {
            _ = &mut shutdown => {
                break;
            }
            accepted = listener.accept() => {
                let (stream, _) = accepted?;
                let h = handler.clone();
                tokio::spawn(async move {
                    let _ = handle_http_connection(stream, h).await;
                });
            }
        }
    }
    Ok(())
}

async fn handle_http_connection(mut stream: TcpStream, handler: Arc<WasmHttpHandler>) -> Result<()> {
    let req = read_http1_request(&mut stream).await?;
    let resp = tokio::task::spawn_blocking(move || handler.handle(&req)).await??;
    write_http1_response(&mut stream, &resp).await
}

async fn read_http1_request(stream: &mut TcpStream) -> Result<WasmHttpRequest> {
    let mut buf = Vec::with_capacity(4096);
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            bail!("client closed connection before sending HTTP headers");
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = find_double_crlf(&buf) {
            break pos;
        }
        if buf.len() > 64 * 1024 {
            bail!("HTTP header exceeds 64 KiB limit");
        }
    };

    let head_str = String::from_utf8_lossy(&buf[..header_end]);
    let mut lines = head_str.lines();
    let req_line = lines.next().unwrap_or("GET / HTTP/1.1");
    let mut parts = req_line.split_whitespace();
    let method = parts.next().unwrap_or("GET").to_string();
    let raw_target = parts.next().unwrap_or("/");
    let (path, query) = match raw_target.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (raw_target.to_string(), String::new()),
    };

    let mut headers = Vec::new();
    let mut content_len = 0usize;
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            let key = k.trim().to_string();
            let val = v.trim().to_string();
            if key.eq_ignore_ascii_case("content-length") {
                content_len = val.parse().unwrap_or(0).min(4 * 1024 * 1024);
            }
            headers.push((key, val));
        }
    }

    let mut body = buf[header_end + 4..].to_vec();
    while body.len() < content_len {
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(content_len);

    Ok(WasmHttpRequest {
        method,
        path,
        query,
        headers,
        body,
    })
}

async fn write_http1_response(stream: &mut TcpStream, resp: &WasmHttpResponse) -> Result<()> {
    let reason = match resp.status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        500 => "Internal Server Error",
        _ => "OK",
    };

    let mut out = format!(
        "HTTP/1.1 {} {}\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         X-Powered-By: coral-wasm/2.1\r\n\
         X-Coral-Wasm-Duration-Us: {}\r\n\
         X-Coral-Wasm-Fuel: {}\r\n",
        resp.status,
        reason,
        resp.body.len(),
        resp.duration_usec,
        resp.fuel_consumed,
    );
    for (k, v) in &resp.headers {
        let clean_k = k.replace(['\r', '\n'], "");
        let clean_v = v.replace(['\r', '\n'], "");
        out.push_str(&format!("{clean_k}: {clean_v}\r\n"));
    }
    out.push_str("\r\n");

    stream.write_all(out.as_bytes()).await?;
    stream.write_all(&resp.body).await?;
    stream.flush().await?;
    Ok(())
}

fn find_double_crlf(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

/// Parses optional CGI-style headers (`Status: 201`, `Content-Type: application/json`, blank line)
/// from Wasm stdout; falls back to `200 OK` raw body if no header block is present.
fn parse_cgi_or_raw_response(raw: &[u8]) -> (u16, Vec<(String, String)>, Vec<u8>) {
    let sep = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|p| (p, 4))
        .or_else(|| raw.windows(2).position(|w| w == b"\n\n").map(|p| (p, 2)));

    let Some((pos, sep_len)) = sep else {
        return (200, Vec::new(), raw.to_vec());
    };
    let head = String::from_utf8_lossy(&raw[..pos]);
    let mut status = 200u16;
    let mut headers = Vec::new();
    let mut valid_cgi = false;

    for line in head.lines() {
        let Some((k, v)) = line.split_once(':') else {
            return (200, Vec::new(), raw.to_vec());
        };
        let key = k.trim();
        let val = v.trim();
        if key.is_empty() || key.contains(' ') {
            return (200, Vec::new(), raw.to_vec());
        }
        valid_cgi = true;
        if key.eq_ignore_ascii_case("status") {
            if let Some(code) = val.split_whitespace().next().and_then(|s| s.parse::<u16>().ok()) {
                status = code;
            }
        } else {
            headers.push((key.to_string(), val.to_string()));
        }
    }

    if valid_cgi {
        (status, headers, raw[pos + sep_len..].to_vec())
    } else {
        (200, Vec::new(), raw.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn serves_wasi_http_requests_with_per_request_sandbox_isolation() {
        // Wasm module using `coral_http::set_status` + WASI `fd_write` CGI headers & JSON body!
        let wat = r#"
        (module
          (import "wasi_snapshot_preview1" "fd_write"
            (func $fd_write (param i32 i32 i32 i32) (result i32)))
          (import "coral_http" "set_status"
            (func $set_status (param i32)))
          (memory (export "memory") 1)
          (data (i32.const 16) "Content-Type: application/json\n\n{\"ok\":true,\"engine\":\"coral-wasm\"}")
          (func (export "wasi_http_handle")
            (call $set_status (i32.const 201))
            (i32.store (i32.const 0) (i32.const 16))
            (i32.store (i32.const 4) (i32.const 65))
            (drop (call $fd_write (i32.const 1) (i32.const 0) (i32.const 1) (i32.const 128)))
          )
        )
        "#;

        let handler = Arc::new(
            WasmHttpHandler::from_bytes(wat.as_bytes(), WasmHttpOptions::default()).unwrap(),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = oneshot::channel();
        let srv = tokio::spawn(serve_wasm_http(listener, handler, rx));

        let mut client = TcpStream::connect(addr).await.unwrap();
        client
            .write_all(b"GET /hello?x=1 HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        let mut resp = String::new();
        client.read_to_string(&mut resp).await.unwrap();

        let _ = tx.send(());
        let _ = srv.await;

        assert!(resp.starts_with("HTTP/1.1 201 Created\r\n"), "resp={resp}");
        assert!(resp.contains("X-Powered-By: coral-wasm/2.1"));
        assert!(resp.contains("X-Coral-Wasm-Duration-Us:"));
        assert!(resp.contains("Content-Type: application/json"));
        assert!(resp.ends_with("{\"ok\":true,\"engine\":\"coral-wasm\"}"));
    }
}
