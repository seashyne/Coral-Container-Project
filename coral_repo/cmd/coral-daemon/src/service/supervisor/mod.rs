//! Container execution supervisor: live log streaming to disk while running,
//! Auto-Restart Policies (`always`, `unless-stopped`, `on-failure[:N]`), and
//! background Healthcheck probes (`coral.health.cmd`).

mod health;
mod log_writer;
mod policy;
mod run;
mod spawn;
#[cfg(test)]
mod tests;

pub use log_writer::LogRotationPolicy;
pub use policy::RestartPolicy;
pub(crate) use spawn::{SupervisorLaunch, spawn_container_supervisor};
