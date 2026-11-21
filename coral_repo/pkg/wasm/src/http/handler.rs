//! Per-request Wasm sandbox instantiation and host import wiring.

use super::{WasmHttpHandler, WasmHttpOptions, WasmHttpRequest, WasmHttpResponse};
use crate::wasi::{WasiContext, register_wasi_preview1};
use anyhow::{Context, Result, bail};
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;
use wasmi::{Caller, Config, Engine, Linker, Module, Store, StoreLimitsBuilder};

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
        let (mut status, mut headers, body) = super::response::parse_cgi_or_raw_response(&raw_stdout);
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
