//! Container lifecycle commands (`run --daemon`, `ps`, `logs`, `exec`, `stop`, `rm`, `diff`)
//! over CRI v1 `RuntimeService` gRPC.

mod lifecycle;
mod logs;
mod run;
mod stats;

pub use lifecycle::{cmd_diff_via_daemon, cmd_exec, cmd_ps, cmd_rm, cmd_stop};
pub use logs::cmd_logs;
pub use run::{DaemonRunOptions, cmd_run_via_daemon};
pub use stats::cmd_stats;
