//! Multi-container stack specification and parser for `coral compose`
//! (`coral-compose.yml` / `docker-compose.yml`).
//!
//! Supports `.env` & host variable interpolation (`${VAR:-default}`), `env_file`,
//! separate `entrypoint` & `command`, Multi-Stage `build` (`context`, `dockerfile`, `target`, `args`),
//! `depends_on` conditions (`service_started`, `service_healthy`, `service_completed_successfully`),
//! `ports` (TCP & UDP), `volumes`, `networks`, `profiles`, `labels`, `user`, `privileged`,
//! `restart`, `healthcheck`, and topological ordering.

mod deps;
mod launch;
mod model;
mod parse;
#[cfg(test)]
mod tests;
mod text;
mod util;
mod yaml;
mod yaml_props;

pub use model::{
    ComposeDependency, ComposeLaunchSpec, ComposeService, ComposeSpec, DependsCondition,
};
pub use util::expand_compose_vars;
