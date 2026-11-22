//! Dockerfile build engine (`coral build` & Coral Desktop Image Builder).
//!
//! Supports Multi-Stage Dockerfiles (`FROM ... AS <stage>`, `COPY --from=<stage|index|image>`,
//! `--target <stage>`, `--build-arg KEY=VAL`), `FROM`, `ARG`, `ENV`, `WORKDIR`, `COPY`, `ADD`,
//! `RUN`, `CMD`, `ENTRYPOINT`, `EXPOSE`, `LABEL`, `USER`, `VOLUME`, executing `RUN` steps inside
//! isolated Coral containers.

mod build;
mod buildkit;
mod cache;
mod context;
pub mod dag;
mod dag_exec;
mod dockerfile;
pub mod dockerignore;
mod exec;
mod step_copy;
mod step_run;
mod steps;
#[cfg(test)]
mod tests;

pub use build::{BuildOptions, build_image};
