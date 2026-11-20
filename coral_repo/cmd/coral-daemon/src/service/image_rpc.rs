//! Kubernetes CRI v1 `ImageService` gRPC implementation for `CoralDaemonService`
//! (supporting OCI Registry v2 pull, local rootfs registration, Dockerfile build, and CoW commit).

use super::CoralDaemonService;
use crate::images::{self, StoredImage};
use coral_api::image_service_server::ImageService;
use coral_api::{
    Image, ImageSpec, ImageStatusRequest, ImageStatusResponse, ListImagesRequest,
    ListImagesResponse, PullImageRequest, PullImageResponse, RemoveImageRequest,
    RemoveImageResponse,
};
use coral_runtime::overlay::commit_overlay_to_rootfs;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;
use tonic::{Request, Response, Status};

#[tonic::async_trait]
impl ImageService for CoralDaemonService {
    async fn list_images(
        &self,
        _request: Request<ListImagesRequest>,
    ) -> Result<Response<ListImagesResponse>, Status> {
        let store = self.store.read().await;
        let mut images: Vec<Image> = store.images.iter().map(Image::from).collect();
        images.sort_by(|a, b| {
            let ta = a.repo_tags.first().map(String::as_str).unwrap_or(&a.id);
            let tb = b.repo_tags.first().map(String::as_str).unwrap_or(&b.id);
            ta.cmp(tb)
        });
        Ok(Response::new(ListImagesResponse { images }))
    }

    async fn image_status(
        &self,
        request: Request<ImageStatusRequest>,
    ) -> Result<Response<ImageStatusResponse>, Status> {
        let name = request
            .into_inner()
            .image
            .map(|i| i.image)
            .unwrap_or_default();
        let store = self.store.read().await;
        Ok(Response::new(ImageStatusResponse {
            image: images::find_image(&store.images, &name).map(Image::from),
        }))
    }

    async fn pull_image(
        &self,
        request: Request<PullImageRequest>,
    ) -> Result<Response<PullImageResponse>, Status> {
        let spec: ImageSpec = request
            .into_inner()
            .image
            .ok_or_else(|| Status::invalid_argument("image spec is required"))?;
        let image_name = spec.image.trim().to_string();
        if image_name.is_empty() {
            return Err(Status::invalid_argument("image name must not be empty"));
        }

        // 0a. Build image from a Dockerfile (`coral.dockerfile` annotation).
        if let Some(dockerfile) = spec.annotations.get("coral.dockerfile") {
            {
                let store = self.store.read().await;
                self.persist_images(&store.images);
            }
            let context_dir = spec
                .annotations
                .get("coral.context")
                .map(String::as_str)
                .filter(|s| !s.trim().is_empty())
                .map(std::path::Path::new);
            let no_cache = spec
                .annotations
                .get("coral.no_cache")
                .is_some_and(|v| v == "true" || v == "1");
            let stored = crate::builder::build_image(crate::builder::BuildOptions {
                tag: &image_name,
                dockerfile_content: dockerfile,
                context_dir,
                images_dir: &self.images_dir,
                state_dir: &self.cfg.state_dir,
                coral_bin: &self.cfg.coral_bin,
                no_cache,
            })
            .await
            .map_err(|e| Status::internal(format!("build image '{image_name}': {e:#}")))?;

            let image_ref = stored.id.clone();
            let mut store = self.store.write().await;
            store.images = images::load_index(&self.images_dir);
            let orphaned = images::upsert_image(&mut store.images, stored);
            for old_path in orphaned {
                self.cleanup_unused_managed_dir(&store.images, &old_path);
            }
            self.persist_images(&store.images);
            return Ok(Response::new(PullImageResponse { image_ref }));
        }

        // 0a-2. Tag an existing image (`coral.tag_source` annotation).
        if let Some(source_img) = spec.annotations.get("coral.tag_source") {
            let mut store = self.store.write().await;
            let tagged = images::tag_image(&mut store.images, source_img, &image_name)
                .map_err(|e| Status::not_found(format!("{e:#}")))?;
            self.persist_images(&store.images);
            return Ok(Response::new(PullImageResponse {
                image_ref: tagged.id,
            }));
        }

        // 0a-3. Save an existing image to a tar/tar.gz/tar.zst archive (`coral.save_archive` annotation).
        if let Some(out_path) = spec.annotations.get("coral.save_archive") {
            let store = self.store.read().await;
            let bytes = images::save_image_archive(
                &store.images,
                &image_name,
                std::path::Path::new(out_path),
            )
            .map_err(|e| Status::internal(format!("save image '{image_name}': {e:#}")))?;
            return Ok(Response::new(PullImageResponse {
                image_ref: format!("saved:{bytes}"),
            }));
        }

        // 0a-4. Load an image from a tar/tar.gz/tar.zst archive (`coral.load_archive` annotation).
        if let Some(in_path) = spec.annotations.get("coral.load_archive") {
            let override_tag = (image_name != "__archive__").then_some(image_name.as_str());
            let stored = images::load_image_archive(
                &self.images_dir,
                std::path::Path::new(in_path),
                override_tag,
            )
            .map_err(|e| Status::internal(format!("load image archive '{in_path}': {e:#}")))?;
            let image_ref = stored.repo_tags.first().cloned().unwrap_or_else(|| stored.id.clone());
            let mut store = self.store.write().await;
            let orphaned = images::upsert_image(&mut store.images, stored);
            for old_path in orphaned {
                self.cleanup_unused_managed_dir(&store.images, &old_path);
            }
            self.persist_images(&store.images);
            return Ok(Response::new(PullImageResponse { image_ref }));
        }

        // 0b. Commit a container's OverlayFS CoW upper layer + lower rootfs into a new image (`coral.commit_container`).
        if let Some(cid) = spec.annotations.get("coral.commit_container") {
            let (lower_rootfs, upperdir) = {
                let store = self.store.read().await;
                let resolved = store.resolve_container_id(cid).ok_or_else(|| {
                    Status::not_found(format!("container '{cid}' not found for commit"))
                })?;
                let ctr = store.containers.get(&resolved).ok_or_else(|| {
                    Status::not_found(format!("container '{cid}' not found for commit"))
                })?;
                (
                    ctr.rootfs_path.clone(),
                    ctr.bundle_dir.join("overlay/upper"),
                )
            };

            let slug: String = image_name
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                        c
                    } else {
                        '_'
                    }
                })
                .collect();
            let dest_rootfs = self.images_dir.join(&slug).join("rootfs");
            commit_overlay_to_rootfs(&lower_rootfs, &upperdir, &dest_rootfs).map_err(|e| {
                Status::internal(format!("commit container '{cid}' to '{image_name}': {e}"))
            })?;

            let abs_rootfs = fs::canonicalize(&dest_rootfs).unwrap_or(dest_rootfs);
            let size = images::dir_size(&abs_rootfs);
            let mut hasher = Sha256::new();
            hasher.update(format!("commit:{cid}:{image_name}:{size}").as_bytes());
            let id = format!("sha256:{:x}", hasher.finalize());

            let stored = StoredImage {
                id: id.clone(),
                repo_tags: vec![image_name],
                size,
                rootfs_path: abs_rootfs.to_string_lossy().into_owned(),
                managed: true,
            };

            let mut store = self.store.write().await;
            let orphaned = images::upsert_image(&mut store.images, stored);
            for old_path in orphaned {
                self.cleanup_unused_managed_dir(&store.images, &old_path);
            }
            self.persist_images(&store.images);
            return Ok(Response::new(PullImageResponse { image_ref: id }));
        }

        // 1. Offline local rootfs registration (explicit annotation or local directory path).
        let local_rootfs = if let Some(path) = spec.annotations.get("coral.rootfs") {
            Some(PathBuf::from(path))
        } else if PathBuf::from(&image_name).is_dir() {
            Some(PathBuf::from(&image_name))
        } else {
            None
        };

        let stored = if let Some(rootfs) = local_rootfs {
            images::register_local_rootfs(&image_name, &rootfs)
                .map_err(|e| Status::not_found(format!("{e:#}")))?
        } else {
            // 2. Online OCI Registry v2 pull (or empty `scratch` base image).
            images::pull_image(&image_name, &self.images_dir)
                .await
                .map_err(|e| Status::internal(format!("pull image '{image_name}': {e:#}")))?
        };

        let image_ref = stored.id.clone();
        let mut store = self.store.write().await;
        let orphaned = images::upsert_image(&mut store.images, stored);
        for old_path in orphaned {
            self.cleanup_unused_managed_dir(&store.images, &old_path);
        }
        self.persist_images(&store.images);

        Ok(Response::new(PullImageResponse { image_ref }))
    }

    async fn remove_image(
        &self,
        request: Request<RemoveImageRequest>,
    ) -> Result<Response<RemoveImageResponse>, Status> {
        let name = request
            .into_inner()
            .image
            .map(|i| i.image)
            .unwrap_or_default();

        let mut store = self.store.write().await;
        let (_, removed_managed) = images::remove_matching_images(&mut store.images, &name);
        for path in removed_managed {
            self.cleanup_unused_managed_dir(&store.images, &path);
        }
        self.persist_images(&store.images);

        Ok(Response::new(RemoveImageResponse {}))
    }

    async fn image_fs_info(
        &self,
        _request: Request<coral_api::ImageFsInfoRequest>,
    ) -> Result<Response<coral_api::ImageFsInfoResponse>, Status> {
        let now = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
        let used = images::dir_size(&self.images_dir);
        Ok(Response::new(coral_api::ImageFsInfoResponse {
            image_filesystems: vec![coral_api::FilesystemUsage {
                timestamp: now,
                fs_id: Some(coral_api::FilesystemIdentifier {
                    mountpoint: self.images_dir.display().to_string(),
                }),
                used_bytes: Some(coral_api::UInt64Value { value: used }),
                inodes_used: Some(coral_api::UInt64Value { value: 0 }),
            }],
        }))
    }
}
