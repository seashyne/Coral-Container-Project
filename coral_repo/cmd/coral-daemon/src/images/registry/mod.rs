//! Online OCI Registry v2 HTTP client: Bearer token authentication, multi-arch manifest
//! index resolution (`linux/amd64` / `linux/arm64`), and SHA-256 verified layer streaming.

mod auth;
mod manifest;
mod pull;
mod util;

pub use pull::pull_image;
