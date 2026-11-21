//! Unit tests for the WASI serverless HTTP host.

use super::*;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;

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
