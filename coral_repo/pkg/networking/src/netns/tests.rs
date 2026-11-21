//! Unit tests for port-mapping parsing and cross-namespace socket bridging.

use super::*;

#[test]
fn parses_port_mappings() {
    assert_eq!(
        PortMapping::parse("8080:80").unwrap(),
        PortMapping {
            bind_ip: "0.0.0.0".into(),
            host_port: 8080,
            container_port: 80,
            protocol: PortProtocol::Tcp,
        }
    );
    assert_eq!(
        PortMapping::parse("127.0.0.1:9000:3000/udp").unwrap(),
        PortMapping {
            bind_ip: "127.0.0.1".into(),
            host_port: 9000,
            container_port: 3000,
            protocol: PortProtocol::Udp,
        }
    );
    assert_eq!(
        PortMapping::parse("5353/UDP").unwrap(),
        PortMapping {
            bind_ip: "0.0.0.0".into(),
            host_port: 5353,
            container_port: 5353,
            protocol: PortProtocol::Udp,
        }
    );
    assert!(PortMapping::parse("80:80/sctp").is_err());
    assert!(PortMapping::parse("bad").is_err());
    assert!(PortMapping::parse("0:80").is_err());
}

#[test]
fn connect_container_udp_bridges_into_isolated_user_and_net_namespace() {
    use std::net::UdpSocket;

    let mut ready_pipe = [-1i32; 2];
    assert_eq!(unsafe { libc::pipe(ready_pipe.as_mut_ptr()) }, 0);

    let pid = unsafe { libc::fork() };
    assert!(pid >= 0);
    if pid == 0 {
        unsafe {
            libc::close(ready_pipe[0]);
            if libc::unshare(libc::CLONE_NEWUSER | libc::CLONE_NEWNET) != 0 {
                libc::_exit(1);
            }
        }
        if bring_loopback_up().is_err() {
            unsafe { libc::_exit(2) };
        }
        let Ok(server) = UdpSocket::bind("127.0.0.1:9090") else {
            unsafe { libc::_exit(3) };
        };
        unsafe {
            let _ = libc::write(ready_pipe[1], b"R".as_ptr().cast(), 1);
            libc::close(ready_pipe[1]);
        }
        let mut buf = [0u8; 32];
        if let Ok((n, peer)) = server.recv_from(&mut buf)
            && &buf[..n] == b"UDP-PING"
        {
            let _ = server.send_to(b"UDP-PONG-NETNS", peer);
        }
        unsafe { libc::_exit(0) };
    }

    unsafe {
        libc::close(ready_pipe[1]);
    }
    let mut sig = [0u8; 1];
    let n = unsafe { libc::read(ready_pipe[0], sig.as_mut_ptr().cast(), 1) };
    unsafe {
        libc::close(ready_pipe[0]);
    }
    assert_eq!(n, 1);
    assert_eq!(sig[0], b'R');

    let udp = connect_container_udp(Some(pid), 9090).expect("connect UDP into netns");
    udp.set_read_timeout(Some(std::time::Duration::from_secs(2)))
        .unwrap();
    udp.send(b"UDP-PING").unwrap();
    let mut reply = [0u8; 64];
    let rn = udp.recv(&mut reply).unwrap();
    assert_eq!(&reply[..rn], b"UDP-PONG-NETNS");

    let mut status = 0;
    unsafe {
        let _ = libc::waitpid(pid, &raw mut status, 0);
    }
}

#[test]
fn connect_container_port_bridges_into_isolated_user_and_net_namespace() {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    let mut ready_pipe = [-1i32; 2];
    assert_eq!(unsafe { libc::pipe(ready_pipe.as_mut_ptr()) }, 0);

    let pid = unsafe { libc::fork() };
    assert!(pid >= 0);
    if pid == 0 {
        unsafe {
            libc::close(ready_pipe[0]);
            if libc::unshare(libc::CLONE_NEWUSER | libc::CLONE_NEWNET) != 0 {
                libc::_exit(1);
            }
        }
        if bring_loopback_up().is_err() {
            unsafe { libc::_exit(2) };
        }
        let Ok(listener) = TcpListener::bind("127.0.0.1:8080") else {
            unsafe { libc::_exit(3) };
        };
        unsafe {
            let _ = libc::write(ready_pipe[1], b"R".as_ptr().cast(), 1);
            libc::close(ready_pipe[1]);
        }
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 4];
            let _ = stream.read_exact(&mut buf);
            if &buf == b"PING" {
                let _ = stream.write_all(b"PONG-FROM-NETNS");
            }
        }
        unsafe { libc::_exit(0) };
    }

    unsafe {
        libc::close(ready_pipe[1]);
    }
    let mut sig = [0u8; 1];
    let n = unsafe { libc::read(ready_pipe[0], sig.as_mut_ptr().cast(), 1) };
    unsafe {
        libc::close(ready_pipe[0]);
    }
    assert_eq!(n, 1);
    assert_eq!(sig[0], b'R');

    let mut stream = connect_container_port(Some(pid), 8080).expect("connect into netns");
    stream.write_all(b"PING").unwrap();
    let mut reply = String::new();
    stream.read_to_string(&mut reply).unwrap();
    assert_eq!(reply, "PONG-FROM-NETNS");

    let mut status = 0;
    unsafe {
        let _ = libc::waitpid(pid, &raw mut status, 0);
    }
}
