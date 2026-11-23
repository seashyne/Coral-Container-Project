//! Container lifecycle commands (`run --daemon`, `ps`, `logs`, `exec`, `stop`, `rm`, `diff`)
//! over CRI v1 `RuntimeService` gRPC.

mod lifecycle;
mod lifecycle_ext;
mod logs;
mod run;
mod stats;

pub use lifecycle::{cmd_diff_via_daemon, cmd_exec, cmd_ps, cmd_rm, cmd_stop};
pub use lifecycle_ext::{cmd_pause, cmd_rename, cmd_top, cmd_unpause, cmd_update, cmd_wait};
pub use logs::cmd_logs;
pub use run::{DaemonRunOptions, cmd_run_via_daemon};
pub use stats::{cmd_stats, cmd_stats_all};
