//! Image tagging (`coral tag`), portable archive export (`coral save`),
//! and offline archive import (`coral load`) supporting `.tar`, `.tar.gz`, and `.tar.zst`.

use super::extract::extract_layer;
use super::reference::normalize_lookup_tag;
use super::{StoredImage, dir_size, find_image, remove_dir_force};
use anyhow::{Context, Result, anyhow, bail};
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

const ARCHIVE_META_FILE: &str = "coral-image.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ArchivedImageMeta {
    id: String,
    repo_tags: Vec<String>,
    size: u64,
}

/// Adds `target_tag` as an alias pointing to the image matching `source_query` in `images`.
pub fn tag_image(
    images: &mut [StoredImage],
    source_query: &str,
    target_tag: &str,
) -> Result<StoredImage> {
    let target_trimmed = target_tag.trim();
    if target_trimmed.is_empty() {
        bail!("target image tag must not be empty");
    }
    let normalized_target = normalize_lookup_tag(target_trimmed);

    let source_id = find_image(images, source_query)
        .ok_or_else(|| anyhow!("source image '{source_query}' not found"))?
        .id
        .clone();

    // Remove the target tag from any other image first so tags remain unique.
    for img in images.iter_mut() {
        if img.id != source_id {
            img.repo_tags
                .retain(|t| t != target_trimmed && normalize_lookup_tag(t) != normalized_target);
        }
    }

    let target_entry = images
        .iter_mut()
        .find(|img| img.id == source_id)
        .ok_or_else(|| anyhow!("source image '{source_query}' not found"))?;

    if !target_entry
        .repo_tags
        .iter()
        .any(|t| t == target_trimmed || normalize_lookup_tag(t) == normalized_target)
    {
        target_entry.repo_tags.push(target_trimmed.to_string());
    }

    Ok(target_entry.clone())
}

/// Exports an installed image matching `query` to a `.tar`, `.tar.gz`/`.tgz`, or `.tar.zst` archive.
/// Returns the size in bytes of the written archive file.
pub fn save_image_archive(
    images: &[StoredImage],
    query: &str,
    output_path: &Path,
) -> Result<u64> {
    let img = find_image(images, query)
        .ok_or_else(|| anyhow!("image '{query}' is not installed"))?;
    let rootfs = Path::new(&img.rootfs_path);
    if !rootfs.is_dir() {
        bail!(
            "rootfs directory '{}' for image '{query}' does not exist",
            rootfs.display()
        );
    }

    if let Some(parent) = output_path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("create parent dir '{}'", parent.display()))?;
    }

    let meta = ArchivedImageMeta {
        id: img.id.clone(),
        repo_tags: if img.repo_tags.is_empty() {
            vec![query.trim().to_string()]
        } else {
            img.repo_tags.clone()
        },
        size: img.size,
    };
    let meta_bytes = serde_json::to_vec_pretty(&meta)?;

    let out_name = output_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    if out_name.ends_with(".tar.gz") || out_name.ends_with(".tgz") {
        let f = File::create(output_path)
            .with_context(|| format!("create archive '{}'", output_path.display()))?;
        let enc = GzEncoder::new(f, Compression::default());
        write_rootfs_tar(enc, rootfs, &meta_bytes)?
            .finish()
            .context("finalize gzip archive")?;
    } else if out_name.ends_with(".tar.zst") || out_name.ends_with(".tzst") {
        let mut child = std::process::Command::new("zstd")
            .args(["-q", "-f", "-o"])
            .arg(output_path)
            .stdin(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .context("spawn 'zstd' to compress image archive")?;
        {
            let stdin = child
                .stdin
                .take()
                .ok_or_else(|| anyhow!("failed to open zstd stdin"))?;
            let _ = write_rootfs_tar(stdin, rootfs, &meta_bytes)?;
        }
        let out = child.wait_with_output().context("wait for zstd compression")?;
        if !out.status.success() {
            bail!(
                "zstd archive compression failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
    } else {
        let f = File::create(output_path)
            .with_context(|| format!("create archive '{}'", output_path.display()))?;
        let mut out = write_rootfs_tar(f, rootfs, &meta_bytes)?;
        out.flush()?;
    }

    let written = fs::metadata(output_path)
        .map(|m| m.len())
        .unwrap_or(0);
    Ok(written)
}

fn write_rootfs_tar<W: Write>(
    writer: W,
    rootfs: &Path,
    meta_bytes: &[u8],
) -> Result<W> {
    let mut builder = tar::Builder::new(writer);
    builder.follow_symlinks(false);

    let mut hdr = tar::Header::new_gnu();
    hdr.set_size(meta_bytes.len() as u64);
    hdr.set_mode(0o644);
    hdr.set_cksum();
    builder
        .append_data(&mut hdr, ARCHIVE_META_FILE, meta_bytes)
        .context("write coral-image.json to tar")?;

    builder
        .append_dir_all(".", rootfs)
        .with_context(|| format!("archive rootfs '{}'", rootfs.display()))?;

    builder.into_inner().context("finalize tar stream")
}

/// Imports an image archive (`.tar`, `.tar.gz`, `.tgz`, `.tar.zst`) created by `coral save`
/// (or a standard rootfs tarball) into `images_dir` and returns its `StoredImage` record.
pub fn load_image_archive(
    images_dir: &Path,
    input_path: &Path,
    override_tag: Option<&str>,
) -> Result<StoredImage> {
    if !input_path.is_file() {
        bail!("archive file '{}' does not exist", input_path.display());
    }

    let meta_opt = read_archive_meta(input_path).ok().flatten();
    let repo_tags: Vec<String> = if let Some(tag) = override_tag.map(str::trim).filter(|s| !s.is_empty()) {
        vec![tag.to_string()]
    } else if let Some(m) = &meta_opt
        && !m.repo_tags.is_empty()
    {
        m.repo_tags.clone()
    } else {
        let stem = input_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("imported")
            .trim_end_matches(".tar");
        vec![format!("{stem}:latest")]
    };

    let primary_tag = &repo_tags[0];
    let slug: String = primary_tag
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let dest_parent = images_dir.join(&slug);
    let dest_rootfs = dest_parent.join("rootfs");
    if dest_rootfs.exists() {
        remove_dir_force(&dest_rootfs)?;
    }
    fs::create_dir_all(&dest_rootfs)
        .with_context(|| format!("create loaded rootfs '{}'", dest_rootfs.display()))?;

    extract_layer(input_path, &dest_rootfs, "")
        .with_context(|| format!("extract image archive '{}'", input_path.display()))?;
    // Remove metadata marker if it was extracted at the root of the rootfs.
    let _ = fs::remove_file(dest_rootfs.join(ARCHIVE_META_FILE));

    let abs_rootfs = fs::canonicalize(&dest_rootfs).unwrap_or(dest_rootfs);
    let size = dir_size(&abs_rootfs);
    let id = if let Some(m) = meta_opt
        && override_tag.is_none()
    {
        m.id
    } else {
        let mut hasher = Sha256::new();
        hasher.update(format!("loaded:{}:{size}", repo_tags.join(",")).as_bytes());
        format!("sha256:{:x}", hasher.finalize())
    };

    Ok(StoredImage {
        id,
        repo_tags,
        size,
        rootfs_path: abs_rootfs.to_string_lossy().into_owned(),
        managed: true,
    })
}

fn read_archive_meta(input_path: &Path) -> Result<Option<ArchivedImageMeta>> {
    let mut f = File::open(input_path)?;
    let mut magic = [0u8; 4];
    let n = f.read(&mut magic)?;
    f.seek(SeekFrom::Start(0))?;

    if n >= 2 && magic[..2] == [0x1f, 0x8b] {
        read_meta_from_tar(GzDecoder::new(f))
    } else if n == 4 && magic == [0x28, 0xb5, 0x2f, 0xfd] {
        drop(f);
        let mut child = std::process::Command::new("zstd")
            .args(["-d", "-c"])
            .arg(input_path)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()?;
        let res = if let Some(stdout) = child.stdout.take() {
            read_meta_from_tar(stdout)
        } else {
            Ok(None)
        };
        let _ = child.kill();
        let _ = child.wait();
        res
    } else {
        read_meta_from_tar(f)
    }
}

fn read_meta_from_tar<R: Read>(reader: R) -> Result<Option<ArchivedImageMeta>> {
    let mut archive = tar::Archive::new(reader);
    for entry_res in archive.entries()? {
        let mut entry = entry_res?;
        let path = entry.path()?;
        if path.as_ref() == Path::new(ARCHIVE_META_FILE)
            || path.as_ref() == Path::new("./coral-image.json")
        {
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes)?;
            let parsed: ArchivedImageMeta = serde_json::from_slice(&bytes)?;
            return Ok(Some(parsed));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn tag_save_and_load_image_roundtrip_gzip_and_zstd() {
        let dir = tempdir().unwrap();
        let src_rootfs = dir.path().join("src-rootfs");
        fs::create_dir_all(src_rootfs.join("etc")).unwrap();
        fs::write(src_rootfs.join("etc/coral-release"), "Coral Linux 1.1\n").unwrap();

        let mut store = vec![StoredImage {
            id: "sha256:test-base-111".into(),
            repo_tags: vec!["coral-base:1.0".into()],
            size: 16,
            rootfs_path: src_rootfs.to_string_lossy().into_owned(),
            managed: false,
        }];

        // 1. Tag image
        let tagged = tag_image(&mut store, "coral-base:1.0", "coral-base:stable").unwrap();
        assert!(tagged.repo_tags.contains(&"coral-base:stable".to_string()));

        // 2. Save as .tar.gz and load into a fresh images_dir
        let gz_archive = dir.path().join("exported.tar.gz");
        let bytes_written = save_image_archive(&store, "coral-base:stable", &gz_archive).unwrap();
        assert!(bytes_written > 0);

        let dest_images = dir.path().join("dest-images");
        let loaded = load_image_archive(&dest_images, &gz_archive, None).unwrap();
        assert_eq!(loaded.id, "sha256:test-base-111");
        assert!(loaded.repo_tags.contains(&"coral-base:stable".to_string()));
        assert_eq!(
            fs::read_to_string(Path::new(&loaded.rootfs_path).join("etc/coral-release")).unwrap(),
            "Coral Linux 1.1\n"
        );

        // 3. Save as .tar.zst and load with override tag
        let zst_archive = dir.path().join("exported.tar.zst");
        save_image_archive(&store, "coral-base:1.0", &zst_archive).unwrap();
        let loaded_zst =
            load_image_archive(&dest_images, &zst_archive, Some("coral-zst:v2")).unwrap();
        assert_eq!(loaded_zst.repo_tags, vec!["coral-zst:v2".to_string()]);
        assert_eq!(
            fs::read_to_string(Path::new(&loaded_zst.rootfs_path).join("etc/coral-release"))
                .unwrap(),
            "Coral Linux 1.1\n"
        );
    }
}
