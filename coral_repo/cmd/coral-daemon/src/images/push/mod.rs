//! OCI Distribution v1.1 / Docker Registry v2 image pusher (`coral push`).

mod auth;
mod blob;
mod layer;
mod model;
mod pusher;
#[cfg(test)]
mod tests;

pub use model::PushResult;
pub use pusher::push_image;
pub(crate) use auth::fetch_bearer_token_with_creds;

pub(super) const MEDIA_OCI_MANIFEST: &str = "application/vnd.oci.image.manifest.v1+json";
pub(super) const MEDIA_OCI_CONFIG: &str = "application/vnd.oci.image.config.v1+json";
pub(super) const MEDIA_OCI_LAYER_GZIP: &str = "application/vnd.oci.image.layer.v1.tar+gzip";
