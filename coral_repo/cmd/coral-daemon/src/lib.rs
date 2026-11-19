//! Coral CRI v1 gRPC Daemon library.

pub mod builder;
pub mod compose;
pub mod images;
pub mod service;

pub use service::{CoralDaemonConfig, CoralDaemonService};
