use std::process::Command;

#[test]
fn test_poc_binary_runs_as_root() {
    if unsafe { libc::getuid() } != 0 {
        eprintln!("Skipping test as not root");
        return;
    }

    // Assume the binary is already built in target/debug/coral
    let status = Command::new("../target/debug/coral")
        .status()
        .expect("Failed to run coral binary");
    
    assert!(status.success());
}
