// Next-Gen presets: AI sandboxes, WASI templates, and connection mode hints.
const SANDBOX_PRESETS = {
  isolation: {
    image: "alpine:3.20",
    cmd: 'echo "Isolated UID=$(id -u) PID=$$ Host=$(hostname)" && uname -a && ls /',
    timeout: 5,
    mem: 64,
    net: false,
  },
  python: {
    image: "python:3.12-alpine",
    cmd: 'python3 -c "import os, sys; print(\'Python\', sys.version.split()[0], \'PID:\', os.getpid())"',
    timeout: 8,
    mem: 128,
    net: false,
  },
  offline: {
    image: "alpine:3.20",
    cmd: 'ip addr 2>&1 || ifconfig 2>&1; ping -c 1 1.1.1.1 2>&1 || echo "AIR_GAPPED_VERIFIED"',
    timeout: 5,
    mem: 64,
    net: false,
  },
};

const WASM_TEMPLATES = {
  hello: `(module
  (import "wasi_snapshot_preview1" "fd_write"
    (func $fd_write (param i32 i32 i32 i32) (result i32)))
  (memory (export "memory") 1)
  (data (i32.const 0) "\\10\\00\\00\\00\\17\\00\\00\\00")
  (data (i32.const 16) "Hello from Coral Wasm!\\n")
  (func (export "_start")
    (drop (call $fd_write (i32.const 1) (i32.const 0) (i32.const 1) (i32.const 64)))))`,
  fuel: `(module
  (import "wasi_snapshot_preview1" "fd_write"
    (func $fd_write (param i32 i32 i32 i32) (result i32)))
  (memory (export "memory") 1)
  (data (i32.const 0) "\\10\\00\\00\\00\\1e\\00\\00\\00")
  (data (i32.const 16) "Computed 100k loops in WASI!\\n")
  (func (export "_start") (local $i i32)
    (loop $L
      (local.set $i (i32.add (local.get $i) (i32.const 1)))
      (br_if $L (i32.lt_u (local.get $i) (i32.const 100000))))
    (drop (call $fd_write (i32.const 1) (i32.const 0) (i32.const 1) (i32.const 64)))))`,
};

const MODE_HINT_KEYS = {
  local: "settings.hintLocal",
  wsl2: "settings.hintWsl2",
  remote: "settings.hintRemote",
};
