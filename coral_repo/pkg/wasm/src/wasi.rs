//! WASI Preview 1 (`wasi_snapshot_preview1`) host ABI implementation for `coral-wasm`,
//! providing isolated stdout/stderr capture, args, environment variables, clock, random,
//! and clean `proc_exit` handling without exposing host filesystem or network privileges.

use std::collections::VecDeque;
use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};
use wasmi::{Caller, Error as TrapError, Linker, Memory, StoreLimits};

// WASI Preview 1 errno constants.
const ERRNO_SUCCESS: i32 = 0;
const ERRNO_BADF: i32 = 8;
const ERRNO_INVAL: i32 = 28;
const ERRNO_NOSYS: i32 = 52;

/// Host state stored inside a `wasmi::Store<WasiContext>`.
pub struct WasiContext {
    pub args: Vec<String>,
    pub envs: Vec<String>,
    pub stdin: VecDeque<u8>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub passthrough_stdio: bool,
    pub exit_code: Option<i32>,
    pub limits: StoreLimits,
}

/// Registers core `wasi_snapshot_preview1` imports on `linker`.
pub fn register_wasi_preview1(linker: &mut Linker<WasiContext>) -> Result<(), TrapError> {
    linker.func_wrap(
        "wasi_snapshot_preview1",
        "fd_write",
        |mut caller: Caller<'_, WasiContext>,
         fd: i32,
         iovs_ptr: i32,
         iovs_len: i32,
         nwritten_ptr: i32|
         -> Result<i32, TrapError> {
            let Some(mem) = get_memory(&mut caller) else {
                return Ok(ERRNO_INVAL);
            };
            if fd != 1 && fd != 2 {
                return Ok(ERRNO_BADF);
            }
            let mut total_written: u32 = 0;
            let mut chunks: Vec<Vec<u8>> = Vec::new();
            for i in 0..iovs_len.max(0) as u32 {
                let entry_off = (iovs_ptr as u32).saturating_add(i.saturating_mul(8)) as usize;
                let mut hdr = [0u8; 8];
                if mem.read(&caller, entry_off, &mut hdr).is_err() {
                    return Ok(ERRNO_INVAL);
                }
                let buf_ptr = u32::from_le_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]) as usize;
                let buf_len = u32::from_le_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]) as usize;
                if buf_len == 0 {
                    continue;
                }
                // Cap single write slice to 4 MiB to protect host memory.
                let safe_len = buf_len.min(4 * 1024 * 1024);
                let mut slice = vec![0u8; safe_len];
                if mem.read(&caller, buf_ptr, &mut slice).is_err() {
                    return Ok(ERRNO_INVAL);
                }
                total_written = total_written.saturating_add(safe_len as u32);
                chunks.push(slice);
            }

            {
                let ctx = caller.data_mut();
                for data in chunks {
                    if fd == 1 {
                        if ctx.passthrough_stdio {
                            let _ = std::io::stdout().write_all(&data);
                            let _ = std::io::stdout().flush();
                        }
                        ctx.stdout.extend_from_slice(&data);
                    } else {
                        if ctx.passthrough_stdio {
                            let _ = std::io::stderr().write_all(&data);
                            let _ = std::io::stderr().flush();
                        }
                        ctx.stderr.extend_from_slice(&data);
                    }
                }
            }

            if mem
                .write(&mut caller, nwritten_ptr as usize, &total_written.to_le_bytes())
                .is_err()
            {
                return Ok(ERRNO_INVAL);
            }
            Ok(ERRNO_SUCCESS)
        },
    )?;

    linker.func_wrap(
        "wasi_snapshot_preview1",
        "fd_read",
        |mut caller: Caller<'_, WasiContext>,
         fd: i32,
         iovs_ptr: i32,
         iovs_len: i32,
         nread_ptr: i32|
         -> Result<i32, TrapError> {
            let Some(mem) = get_memory(&mut caller) else {
                return Ok(ERRNO_INVAL);
            };
            if fd != 0 {
                return Ok(ERRNO_BADF);
            }
            let mut total_read: u32 = 0;
            for i in 0..iovs_len.max(0) as u32 {
                let entry_off = (iovs_ptr as u32).saturating_add(i.saturating_mul(8)) as usize;
                let mut hdr = [0u8; 8];
                if mem.read(&caller, entry_off, &mut hdr).is_err() {
                    return Ok(ERRNO_INVAL);
                }
                let buf_ptr = u32::from_le_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]) as usize;
                let buf_len = u32::from_le_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]) as usize;
                let mut tmp = Vec::with_capacity(buf_len.min(65536));
                {
                    let stdin = &mut caller.data_mut().stdin;
                    for _ in 0..buf_len.min(65536) {
                        if let Some(b) = stdin.pop_front() {
                            tmp.push(b);
                        } else {
                            break;
                        }
                    }
                }
                if !tmp.is_empty() {
                    if mem.write(&mut caller, buf_ptr, &tmp).is_err() {
                        return Ok(ERRNO_INVAL);
                    }
                    total_read = total_read.saturating_add(tmp.len() as u32);
                }
            }
            let _ = mem.write(&mut caller, nread_ptr as usize, &total_read.to_le_bytes());
            Ok(ERRNO_SUCCESS)
        },
    )?;

    linker.func_wrap(
        "wasi_snapshot_preview1",
        "args_sizes_get",
        |mut caller: Caller<'_, WasiContext>, argc_ptr: i32, buf_size_ptr: i32| -> i32 {
            let Some(mem) = get_memory(&mut caller) else {
                return ERRNO_INVAL;
            };
            let argc = caller.data().args.len() as u32;
            let buf_sz: u32 = caller
                .data()
                .args
                .iter()
                .map(|a| (a.len() + 1) as u32)
                .sum();
            if mem.write(&mut caller, argc_ptr as usize, &argc.to_le_bytes()).is_err()
                || mem
                    .write(&mut caller, buf_size_ptr as usize, &buf_sz.to_le_bytes())
                    .is_err()
            {
                return ERRNO_INVAL;
            }
            ERRNO_SUCCESS
        },
    )?;

    linker.func_wrap(
        "wasi_snapshot_preview1",
        "args_get",
        |mut caller: Caller<'_, WasiContext>, argv_ptr: i32, argv_buf_ptr: i32| -> i32 {
            let Some(mem) = get_memory(&mut caller) else {
                return ERRNO_INVAL;
            };
            let args = caller.data().args.clone();
            write_string_table(&mut caller, &mem, argv_ptr as u32, argv_buf_ptr as u32, &args)
        },
    )?;

    linker.func_wrap(
        "wasi_snapshot_preview1",
        "environ_sizes_get",
        |mut caller: Caller<'_, WasiContext>, count_ptr: i32, buf_size_ptr: i32| -> i32 {
            let Some(mem) = get_memory(&mut caller) else {
                return ERRNO_INVAL;
            };
            let count = caller.data().envs.len() as u32;
            let buf_sz: u32 = caller
                .data()
                .envs
                .iter()
                .map(|e| (e.len() + 1) as u32)
                .sum();
            if mem.write(&mut caller, count_ptr as usize, &count.to_le_bytes()).is_err()
                || mem
                    .write(&mut caller, buf_size_ptr as usize, &buf_sz.to_le_bytes())
                    .is_err()
            {
                return ERRNO_INVAL;
            }
            ERRNO_SUCCESS
        },
    )?;

    linker.func_wrap(
        "wasi_snapshot_preview1",
        "environ_get",
        |mut caller: Caller<'_, WasiContext>, env_ptr: i32, env_buf_ptr: i32| -> i32 {
            let Some(mem) = get_memory(&mut caller) else {
                return ERRNO_INVAL;
            };
            let envs = caller.data().envs.clone();
            write_string_table(&mut caller, &mem, env_ptr as u32, env_buf_ptr as u32, &envs)
        },
    )?;

    linker.func_wrap(
        "wasi_snapshot_preview1",
        "clock_time_get",
        |mut caller: Caller<'_, WasiContext>,
         _clock_id: i32,
         _precision: i64,
         time_ptr: i32|
         -> i32 {
            let Some(mem) = get_memory(&mut caller) else {
                return ERRNO_INVAL;
            };
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0);
            if mem.write(&mut caller, time_ptr as usize, &nanos.to_le_bytes()).is_err() {
                return ERRNO_INVAL;
            }
            ERRNO_SUCCESS
        },
    )?;

    linker.func_wrap(
        "wasi_snapshot_preview1",
        "random_get",
        |mut caller: Caller<'_, WasiContext>, buf_ptr: i32, buf_len: i32| -> i32 {
            let Some(mem) = get_memory(&mut caller) else {
                return ERRNO_INVAL;
            };
            let len = buf_len.clamp(0, 1024 * 1024) as usize;
            let mut bytes = vec![0u8; len];
            // Read OS CSPRNG from /dev/urandom, falling back to monotonic PRNG if unavailable.
            if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
                use std::io::Read;
                let _ = f.read_exact(&mut bytes);
            } else {
                let mut seed = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_nanos() as u64)
                    .unwrap_or(0x9e37_79b9_7f4a_7c15);
                for b in &mut bytes {
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    *b = seed as u8;
                }
            }
            if mem.write(&mut caller, buf_ptr as usize, &bytes).is_err() {
                return ERRNO_INVAL;
            }
            ERRNO_SUCCESS
        },
    )?;

    linker.func_wrap(
        "wasi_snapshot_preview1",
        "fd_fdstat_get",
        |mut caller: Caller<'_, WasiContext>, fd: i32, stat_ptr: i32| -> i32 {
            let Some(mem) = get_memory(&mut caller) else {
                return ERRNO_INVAL;
            };
            if !(0..=2).contains(&fd) {
                return ERRNO_BADF;
            }
            // WASI fdstat struct is 24 bytes: filetype=2 (character_device), flags=0, rights=0.
            let mut stat = [0u8; 24];
            stat[0] = 2;
            if mem.write(&mut caller, stat_ptr as usize, &stat).is_err() {
                return ERRNO_INVAL;
            }
            ERRNO_SUCCESS
        },
    )?;

    linker.func_wrap(
        "wasi_snapshot_preview1",
        "fd_close",
        |_caller: Caller<'_, WasiContext>, fd: i32| -> i32 {
            if (0..=2).contains(&fd) {
                ERRNO_SUCCESS
            } else {
                ERRNO_BADF
            }
        },
    )?;

    linker.func_wrap(
        "wasi_snapshot_preview1",
        "fd_prestat_get",
        |_caller: Caller<'_, WasiContext>, _fd: i32, _prestat_ptr: i32| -> i32 { ERRNO_BADF },
    )?;

    linker.func_wrap(
        "wasi_snapshot_preview1",
        "fd_prestat_dir_name",
        |_caller: Caller<'_, WasiContext>, _fd: i32, _path: i32, _path_len: i32| -> i32 {
            ERRNO_BADF
        },
    )?;

    linker.func_wrap(
        "wasi_snapshot_preview1",
        "fd_seek",
        |_caller: Caller<'_, WasiContext>,
         _fd: i32,
         _offset: i64,
         _whence: i32,
         _newoffset: i32|
         -> i32 { ERRNO_NOSYS },
    )?;

    linker.func_wrap(
        "wasi_snapshot_preview1",
        "proc_exit",
        |mut caller: Caller<'_, WasiContext>, code: i32| -> Result<(), TrapError> {
            caller.data_mut().exit_code = Some(code);
            Err(TrapError::i32_exit(code))
        },
    )?;

    Ok(())
}

fn get_memory(caller: &mut Caller<'_, WasiContext>) -> Option<Memory> {
    caller.get_export("memory")?.into_memory()
}

fn write_string_table(
    caller: &mut Caller<'_, WasiContext>,
    mem: &Memory,
    mut ptr_off: u32,
    mut buf_off: u32,
    items: &[String],
) -> i32 {
    for item in items {
        if mem
            .write(&mut *caller, ptr_off as usize, &buf_off.to_le_bytes())
            .is_err()
        {
            return ERRNO_INVAL;
        }
        ptr_off = ptr_off.saturating_add(4);
        let mut bytes = item.as_bytes().to_vec();
        bytes.push(0);
        if mem.write(&mut *caller, buf_off as usize, &bytes).is_err() {
            return ERRNO_INVAL;
        }
        buf_off = buf_off.saturating_add(bytes.len() as u32);
    }
    ERRNO_SUCCESS
}
