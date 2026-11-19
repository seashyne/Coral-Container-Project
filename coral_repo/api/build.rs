use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=proto/cri.proto");
    println!("cargo:rerun-if-env-changed=PROTOC");

    // Use system PROTOC if configured; otherwise fall back to vendored protoc binary.
    if std::env::var_os("PROTOC").is_none() {
        let protoc_path: PathBuf = protoc_bin_vendored::protoc_bin_path()?;
        // SAFETY: build scripts run single-threaded before compiling crates.
        unsafe {
            std::env::set_var("PROTOC", &protoc_path);
        }
    }

    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .compile_protos(&["proto/cri.proto"], &["proto"])?;

    Ok(())
}
