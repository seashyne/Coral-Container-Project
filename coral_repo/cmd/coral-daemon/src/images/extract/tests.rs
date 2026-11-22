//! Tests for OCI layer extraction.

use super::*;
use flate2::Compression;
use flate2::write::GzEncoder;
use std::io::Write;
use tempfile::tempdir;

#[test]
fn extracts_gzip_layer_with_merged_usr_symlink_and_whiteouts() {
    let dir = tempdir().unwrap();
    let rootfs = dir.path().join("rootfs");
    fs::create_dir_all(&rootfs).unwrap();

    let layer1 = dir.path().join("layer1.tar.gz");
    {
        let f = File::create(&layer1).unwrap();
        let enc = GzEncoder::new(f, Compression::fast());
        let mut builder = tar::Builder::new(enc);

        let mut hdr = tar::Header::new_gnu();
        hdr.set_entry_type(EntryType::Symlink);
        hdr.set_size(0);
        hdr.set_mode(0o777);
        hdr.set_cksum();
        builder.append_link(&mut hdr, "bin", "/usr/bin").unwrap();

        let data = b"hello coral";
        let mut hdr = tar::Header::new_gnu();
        hdr.set_size(data.len() as u64);
        hdr.set_mode(0o755);
        hdr.set_cksum();
        builder.append_data(&mut hdr, "bin/hello", &data[..]).unwrap();

        let old = b"old";
        let mut hdr = tar::Header::new_gnu();
        hdr.set_size(old.len() as u64);
        hdr.set_mode(0o644);
        hdr.set_cksum();
        builder
            .append_data(&mut hdr, "etc/old.conf", &old[..])
            .unwrap();

        builder.into_inner().unwrap().finish().unwrap();
    }
    extract_layer(&layer1, &rootfs, "application/vnd.oci.image.layer.v1.tar+gzip").unwrap();
    assert_eq!(
        fs::read_to_string(rootfs.join("usr/bin/hello")).unwrap(),
        "hello coral"
    );
    assert!(rootfs.join("etc/old.conf").exists());

    let layer2 = dir.path().join("layer2.tar.gz");
    {
        let f = File::create(&layer2).unwrap();
        let enc = GzEncoder::new(f, Compression::fast());
        let mut builder = tar::Builder::new(enc);
        let mut hdr = tar::Header::new_gnu();
        hdr.set_size(0);
        hdr.set_mode(0o644);
        hdr.set_cksum();
        builder
            .append_data(&mut hdr, "etc/.wh.old.conf", io::empty())
            .unwrap();
        builder.into_inner().unwrap().finish().unwrap();
    }
    extract_layer(&layer2, &rootfs, "application/vnd.oci.image.layer.v1.tar+gzip").unwrap();
    assert!(!rootfs.join("etc/old.conf").exists());
}

/// Regression test: the zstd path must not fail when the tar reader stops early.
///
/// The tar reader stops at the end-of-archive marker, so the decompressor still
/// has bytes left to write. If its stdout is closed at that moment it dies
/// with SIGPIPE, and the extraction reports `zstd decompression failed` even
/// though the archive was perfectly valid. This showed up in CI as an
/// intermittent `coral load` failure on real image archives.
#[test]
fn extracts_zstd_layer_when_the_archive_keeps_data_after_the_end_marker() {
    if which_zstd().is_none() {
        eprintln!("skipping: the zstd binary is not installed");
        return;
    }

    let dir = tempdir().unwrap();
    let rootfs = dir.path().join("rootfs");
    fs::create_dir_all(&rootfs).unwrap();

    // A valid tar with one entry, the end-of-archive marker, and then a large
    // block of incompressible trailing data. The trailing data guarantees the
    // decompressor still has output pending when the tar reader stops.
    let raw = dir.path().join("layer.tar");
    {
        let f = File::create(&raw).unwrap();
        let mut builder = tar::Builder::new(f);
        let data = b"hello coral";
        let mut hdr = tar::Header::new_gnu();
        hdr.set_size(data.len() as u64);
        hdr.set_mode(0o755);
        hdr.set_cksum();
        builder.append_data(&mut hdr, "hello.txt", &data[..]).unwrap();
        builder.finish().unwrap();
        drop(builder);

        // Append the trailing payload after the end-of-archive marker.
        let mut file = fs::OpenOptions::new().append(true).open(&raw).unwrap();
        let chunk = vec![0xA5u8; 1 << 20];
        for _ in 0..32 {
            file.write_all(&chunk).unwrap();
        }
    }

    let compressed = dir.path().join("layer.tar.zst");
    compress_with_zstd(&raw, &compressed);

    extract_layer(
        &compressed,
        &rootfs,
        "application/vnd.oci.image.layer.v1.tar+zstd",
    )
    .expect("extracting a valid zstd layer must not fail");

    assert_eq!(
        fs::read_to_string(rootfs.join("hello.txt")).unwrap(),
        "hello coral"
    );
}

/// Returns the path of the `zstd` binary, or `None` when it is not installed.
fn which_zstd() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join("zstd"))
        .find(|candidate| candidate.is_file())
}

/// Compresses `input` into `output` with the `zstd` binary.
fn compress_with_zstd(input: &Path, output: &Path) {
    let status = std::process::Command::new("zstd")
        .args(["-q", "-f", "-o"])
        .arg(output)
        .arg(input)
        .status()
        .expect("spawn zstd");
    assert!(status.success(), "zstd compression failed");
}
