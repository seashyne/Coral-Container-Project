//! Rootfs packaging into a gzipped OCI layer.

use anyhow::{Context, Result};
use flate2::Compression;
use flate2::write::GzEncoder;
use sha2::{Digest, Sha256};
use std::io::{self, Write};
use std::path::Path;

struct HashingWriter<W: Write> {
    inner: W,
    hasher: Sha256,
}

impl<W: Write> HashingWriter<W> {
    fn new(inner: W) -> Self {
        Self {
            inner,
            hasher: Sha256::new(),
        }
    }

    fn finish(self) -> (W, String) {
        (self.inner, format!("sha256:{:x}", self.hasher.finalize()))
    }
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.hasher.update(&buf[..n]);
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

pub(super) fn pack_rootfs_layer_gzip(rootfs: &Path) -> Result<(Vec<u8>, String, String)> {
    let gz = GzEncoder::new(Vec::new(), Compression::default());
    let hw = HashingWriter::new(gz);
    let mut tar_builder = tar::Builder::new(hw);
    tar_builder.follow_symlinks(false);
    tar_builder
        .append_dir_all(".", rootfs)
        .with_context(|| format!("tar rootfs '{}'", rootfs.display()))?;
    let hw = tar_builder.into_inner()?;
    let (gz, diff_id) = hw.finish();
    let compressed = gz.finish().context("finish gzip layer stream")?;
    let layer_digest = sha256_digest(&compressed);
    Ok((compressed, diff_id, layer_digest))
}

pub(super) fn sha256_digest(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    format!("sha256:{:x}", h.finalize())
}

pub(super) fn host_oci_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        "arm" => "arm",
        "riscv64" => "riscv64",
        other => other,
    }
}
