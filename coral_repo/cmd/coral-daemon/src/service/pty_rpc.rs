//! Full-duplex interactive Linux PTY streaming over CRI gRPC (`RuntimeService::ExecStream`).
//! Works identically over Local, WSL2, and Remote daemon connections on the single CRI port.

use super::CoralDaemonService;
use coral_api::{ContainerState as CriContainerState, ExecStreamRequest, ExecStreamResponse};
use coral_runtime::pty::{PtyPair, attach_slave_stdio};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::pin::Pin;
use std::process::{Command as StdCommand, Stdio};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Status, Streaming};
use tracing::{debug, warn};

pub type ExecStreamOutput =
    Pin<Box<dyn tokio_stream::Stream<Item = Result<ExecStreamResponse, Status>> + Send + 'static>>;

impl CoralDaemonService {
    #[allow(clippy::result_large_err)]
    pub(crate) async fn handle_exec_stream(
        &self,
        mut inbound: Streaming<ExecStreamRequest>,
    ) -> Result<ExecStreamOutput, Status> {
        // First message initializes container target, command, and initial terminal size.
        let init = inbound
            .message()
            .await?
            .ok_or_else(|| Status::invalid_argument("exec stream closed before init message"))?;

        let cid = init.container_id.trim().to_string();
        if cid.is_empty() {
            return Err(Status::invalid_argument("container_id is required"));
        }

        let (cid, is_running) = {
            let store = self.store.read().await;
            let resolved = store
                .resolve_container_id(&cid)
                .ok_or_else(|| Status::not_found(format!("container '{cid}' not found")))?;
            let ctr = store
                .containers
                .get(&resolved)
                .ok_or_else(|| Status::not_found(format!("container '{cid}' not found")))?;
            (resolved, ctr.state == CriContainerState::ContainerRunning)
        };
        if !is_running {
            return Err(Status::failed_precondition(format!(
                "container '{cid}' is not running — PTY streaming requires a running container"
            )));
        }

        let cmd = if init.cmd.is_empty() {
            vec!["/bin/sh".into()]
        } else {
            init.cmd
        };
        let cols = if init.cols == 0 { 80 } else { init.cols.min(500) as u16 };
        let rows = if init.rows == 0 { 24 } else { init.rows.min(200) as u16 };

        let pty = PtyPair::open()
            .map_err(|e| Status::internal(format!("allocate daemon PTY: {e}")))?;
        set_pty_winsize(pty.master.as_raw_fd(), cols, rows);

        let slave_fd: RawFd = pty.slave.as_raw_fd();
        let mut proc_cmd = StdCommand::new(&self.cfg.coral_bin);
        proc_cmd
            .arg("--root")
            .arg(self.runtime_root())
            .arg("exec")
            .arg("--local")
            .arg("-it")
            .arg("-e")
            .arg("TERM=xterm-256color")
            .arg(&cid)
            .arg("--")
            .args(&cmd)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        // Attach the allocated PTY slave as the controlling terminal of `coral exec -it`.
        unsafe {
            use std::os::unix::process::CommandExt;
            proc_cmd.pre_exec(move || {
                attach_slave_stdio(slave_fd)
                    .map_err(|e| std::io::Error::other(e.to_string()))
            });
        }

        let mut child = proc_cmd
            .spawn()
            .map_err(|e| Status::internal(format!("spawn PTY exec process: {e}")))?;
        drop(pty.slave);

        let dup_raw = unsafe { libc::fcntl(pty.master.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
        if dup_raw < 0 {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Status::internal("dup PTY master fd failed"));
        }
        let write_fd = unsafe { OwnedFd::from_raw_fd(dup_raw) };
        let read_fd = pty.master;

        // Forward any initial stdin bytes bundled in the init message.
        if !init.stdin.is_empty() {
            write_all_pty(write_fd.as_raw_fd(), &init.stdin);
        }

        let (tx, rx) = mpsc::channel::<Result<ExecStreamResponse, Status>>(64);
        let child_pid = child.id();

        // Inbound task: client keystrokes & resize events -> PTY master.
        let writer_handle = tokio::spawn(async move {
            let wfd = write_fd;
            while let Ok(Some(msg)) = inbound.message().await {
                if msg.resize && msg.cols > 0 && msg.rows > 0 {
                    set_pty_winsize(
                        wfd.as_raw_fd(),
                        msg.cols.min(500) as u16,
                        msg.rows.min(200) as u16,
                    );
                }
                if !msg.stdin.is_empty() {
                    write_all_pty(wfd.as_raw_fd(), &msg.stdin);
                }
            }
            debug!(container_id = %cid, pid = child_pid, "PTY inbound stream closed");
            unsafe {
                let _ = libc::kill(child_pid as i32, libc::SIGHUP);
            }
        });

        // Outbound blocking reader: PTY master -> gRPC stream -> final exit frame.
        tokio::task::spawn_blocking(move || {
            let rfd = read_fd;
            let mut buf = [0u8; 4096];
            loop {
                let n = unsafe { libc::read(rfd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
                if n > 0 {
                    let chunk = buf[..n as usize].to_vec();
                    if tx
                        .blocking_send(Ok(ExecStreamResponse {
                            data: chunk,
                            exited: false,
                            exit_code: 0,
                        }))
                        .is_err()
                    {
                        let _ = child.kill();
                        break;
                    }
                } else if n == 0 {
                    break;
                } else {
                    let err = std::io::Error::last_os_error();
                    if err.kind() == std::io::ErrorKind::Interrupted {
                        continue;
                    }
                    // EIO is Linux's standard EOF signal when the PTY slave closes.
                    break;
                }
            }

            writer_handle.abort();
            let exit_code = match child.wait() {
                Ok(st) => st.code().unwrap_or(1),
                Err(e) => {
                    warn!(err = %e, "wait on PTY exec child failed");
                    1
                }
            };
            let _ = tx.blocking_send(Ok(ExecStreamResponse {
                data: Vec::new(),
                exited: true,
                exit_code,
            }));
        });

        Ok(Box::pin(ReceiverStream::new(rx)))
    }
}

fn set_pty_winsize(fd: RawFd, cols: u16, rows: u16) {
    let ws = libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    unsafe {
        let _ = libc::ioctl(fd, libc::TIOCSWINSZ, &ws);
    }
}

fn write_all_pty(fd: RawFd, mut data: &[u8]) {
    while !data.is_empty() {
        let n = unsafe { libc::write(fd, data.as_ptr().cast(), data.len()) };
        if n <= 0 {
            let err = std::io::Error::last_os_error();
            if err.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            break;
        }
        data = &data[n as usize..];
    }
}
