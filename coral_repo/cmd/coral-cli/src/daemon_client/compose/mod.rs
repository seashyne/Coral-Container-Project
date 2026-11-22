//! `coral compose` CLI commands (`up`, `down`, `ps`, `logs`, `build`, `start`, `stop`,
//! `restart`, `exec`, `config`) managing multi-container stacks defined in
//! `coral-compose.yml` / `docker-compose.yml` via `coral-daemon` CRI v1.

mod common;
mod lifecycle;
mod logs;
mod ps;
mod up;
mod watch;

pub use lifecycle::{
    cmd_compose_down, cmd_compose_exec, cmd_compose_restart, cmd_compose_start, cmd_compose_stop,
};
pub use logs::cmd_compose_logs;
pub use ps::{cmd_compose_config, cmd_compose_ps};
pub use up::{cmd_compose_build, cmd_compose_up};
pub use watch::cmd_compose_watch;
