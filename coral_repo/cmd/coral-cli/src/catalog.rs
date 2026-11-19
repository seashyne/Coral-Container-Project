use coral_api::Image;

pub struct BaseImageEntry {
    pub name: &'static str,
    pub image_ref: &'static str,
    pub match_tags: &'static [&'static str],
    pub approx_size: &'static str,
    pub pkg_mgr: &'static str,
    pub libc: &'static str,
    pub best_for: &'static str,
}

pub const BASE_CATALOG: &[BaseImageEntry] = &[
    BaseImageEntry {
        name: "Alpine Linux",
        image_ref: "alpine:3.20",
        match_tags: &["alpine:3.20", "alpine:latest"],
        approx_size: "~5 MB",
        pkg_mgr: "apk",
        libc: "musl",
        best_for: "Ultra-lightweight containers, microservices, Go/Rust apps",
    },
    BaseImageEntry {
        name: "Debian Slim",
        image_ref: "debian:bookworm-slim",
        match_tags: &["debian:bookworm-slim"],
        approx_size: "~35-75 MB",
        pkg_mgr: "apt",
        libc: "glibc",
        best_for: "General production apps needing high compatibility",
    },
    BaseImageEntry {
        name: "Ubuntu",
        image_ref: "ubuntu:24.04",
        match_tags: &["ubuntu:24.04", "ubuntu:latest"],
        approx_size: "~75-80 MB",
        pkg_mgr: "apt",
        libc: "glibc",
        best_for: "Broad software availability, LTS stability, familiar tooling",
    },
    BaseImageEntry {
        name: "BusyBox",
        image_ref: "busybox:musl",
        match_tags: &["busybox:musl", "busybox:latest"],
        approx_size: "~1-5 MB",
        pkg_mgr: "None",
        libc: "musl",
        best_for: "Bare-bones shell scripts and static binaries",
    },
    BaseImageEntry {
        name: "Rocky Linux",
        image_ref: "rockylinux:9-minimal",
        match_tags: &["rockylinux:9-minimal", "rockylinux/rockylinux:9-minimal"],
        approx_size: "~50-110 MB",
        pkg_mgr: "microdnf",
        libc: "glibc",
        best_for: "Enterprise workloads requiring RHEL binary compatibility",
    },
    BaseImageEntry {
        name: "Red Hat UBI",
        image_ref: "registry.access.redhat.com/ubi9/ubi-minimal:latest",
        match_tags: &[
            "registry.access.redhat.com/ubi9/ubi-minimal:latest",
            "ubi9/ubi-minimal:latest",
        ],
        approx_size: "~40-100 MB",
        pkg_mgr: "microdnf",
        libc: "glibc",
        best_for: "Official Red Hat Universal Base Image (RHEL compatible)",
    },
    BaseImageEntry {
        name: "scratch",
        image_ref: "scratch",
        match_tags: &["scratch", "scratch:latest"],
        approx_size: "0 MB",
        pkg_mgr: "None",
        libc: "None",
        best_for: "Empty starting point for statically compiled binaries",
    },
];

pub fn format_bytes(bytes: u64) -> String {
    if bytes == 0 {
        return "0 B".to_string();
    }
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut val = bytes as f64;
    let mut idx = 0usize;
    while val >= 1024.0 && idx < UNITS.len() - 1 {
        val /= 1024.0;
        idx += 1;
    }
    if idx == 0 || val >= 100.0 {
        format!("{val:.0} {}", UNITS[idx])
    } else {
        format!("{val:.1} {}", UNITS[idx])
    }
}

pub fn print_catalog(installed: &[Image], source_label: &str) {
    println!("Popular Linux Base Images ({source_label}):\n");
    println!(
        "{:<13} {:<51} {:<10} {:<9} {:<6} STATUS",
        "BASE IMAGE", "IMAGE REF", "SIZE", "PKG MGR", "LIBC"
    );
    println!("{}", "-".repeat(108));

    for entry in BASE_CATALOG {
        let matched = installed
            .iter()
            .find(|img| img.repo_tags.iter().any(|t| entry.match_tags.contains(&t.as_str())));
        let status = match matched {
            Some(img) => format!("installed ({})", format_bytes(img.size)),
            None => "available".to_string(),
        };
        println!(
            "{:<13} {:<51} {:<10} {:<9} {:<6} {}",
            entry.name, entry.image_ref, entry.approx_size, entry.pkg_mgr, entry.libc, status
        );
        println!("              ↳ {}", entry.best_for);
    }

    println!("\nPull any image:  coral pull <IMAGE REF>");
    println!("Run a container: coral run --image <IMAGE REF> -- /bin/sh -c \"cat /etc/os-release\"");
}
