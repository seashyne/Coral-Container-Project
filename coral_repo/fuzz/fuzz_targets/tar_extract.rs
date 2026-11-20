#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.is_empty() {
        return;
    }
    let media_type = match data[0] % 3 {
        0 => "application/vnd.oci.image.layer.v1.tar+gzip",
        1 => "application/vnd.oci.image.layer.v1.tar+zstd",
        _ => "application/vnd.oci.image.layer.v1.tar",
    };
    if let Ok(dir) = tempfile::tempdir() {
        let blob = dir.path().join("layer.blob");
        let rootfs = dir.path().join("rootfs");
        if std::fs::write(&blob, &data[1..]).is_ok() {
            let _ = coral_daemon::images::extract::extract_layer(&blob, &rootfs, media_type);
        }
    }
});
