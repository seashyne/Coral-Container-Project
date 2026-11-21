//! Async HTTP/1.1 serverless host for pre-compiled Wasm handlers.

use super::{WasmHttpHandler, WasmHttpRequest, WasmHttpResponse};
use anyhow::{Result, bail};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;

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
