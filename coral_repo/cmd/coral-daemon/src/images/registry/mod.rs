//! Online OCI Registry v2 HTTP client: Bearer token authentication, multi-arch manifest
//! index resolution (`linux/amd64` / `linux/arm64`), and SHA-256 verified layer streaming.

mod auth;
pub(crate) mod layer_cache;
pub mod layer_store;
mod manifest;
mod pull;
mod util;

pub use layer_store::load_image_layers_manifest;
pub use pull::{pull_image, pull_image_for_platform};
