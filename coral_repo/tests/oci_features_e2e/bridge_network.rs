//! Isolated bridge NetNS and same-internal-port forwarding.

use super::*;

#[test]
fn bridge_network_isolates_netns_and_forwards_same_internal_port() {
    let rootfs = alpine_rootfs();
    let coral = ensure_coral_binary();
    let work = tempdir().unwrap();
    let state_root = work.path().join("state");
    let images_dir = work.path().join("images");

    Command::new(coral)
        .arg("--images-dir")
        .arg(&images_dir)
        .args(["pull", "alpine:3.20", "--local", "--rootfs"])
        .arg(&rootfs)
        .status()
        .unwrap();

    let host_port_1 = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let host_port_2 = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };

    // Both containers bind the EXACT SAME internal container port (8080) inside their own
    // isolated `--network bridge` (CLONE_NEWNET) namespaces at the same time!
    let mut ctr1 = Command::new(coral)
        .arg("--root")
        .arg(&state_root)
        .arg("--images-dir")
        .arg(&images_dir)
        .args([
            "run",
            "--network",
            "bridge",
            "--image",
            "alpine:3.20",
            "--name",
            "bridge-web-1",
            "-p",
            &format!("127.0.0.1:{host_port_1}:8080"),
            "--",
            "/bin/sh",
            "-c",
            "echo BRIDGE_NODE_ONE | nc -l -p 8080 -w 3",
        ])
        .spawn()
        .unwrap();

    let mut ctr2 = Command::new(coral)
        .arg("--root")
        .arg(&state_root)
        .arg("--images-dir")
        .arg(&images_dir)
        .args([
            "run",
            "--network",
            "bridge",
            "--image",
            "alpine:3.20",
            "--name",
            "bridge-web-2",
            "-p",
            &format!("127.0.0.1:{host_port_2}:8080"),
            "--",
            "/bin/sh",
            "-c",
            "echo BRIDGE_NODE_TWO | nc -l -p 8080 -w 3",
        ])
        .spawn()
        .unwrap();

    let read_from_port = |port: u16, expected: &str| -> String {
        let mut buf = String::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Ok(mut s) = std::net::TcpStream::connect(("127.0.0.1", port)) {
                use std::io::Read;
                let _ = s.set_read_timeout(Some(Duration::from_millis(500)));
                let _ = s.read_to_string(&mut buf);
                if buf.contains(expected) {
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(40));
        }
        buf
    };

    let out1 = read_from_port(host_port_1, "BRIDGE_NODE_ONE");
    let out2 = read_from_port(host_port_2, "BRIDGE_NODE_TWO");

    let _ = ctr1.wait();
    let _ = ctr2.wait();

    assert!(out1.contains("BRIDGE_NODE_ONE"), "ctr1 output: {out1:?}");
    assert!(out2.contains("BRIDGE_NODE_TWO"), "ctr2 output: {out2:?}");
}
