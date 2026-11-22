#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ROOTFS_DIR="${ROOT_DIR}/rootfs"
ALPINE_VERSION="3.20.3"
HOST_ARCH="$(uname -m)"
case "${HOST_ARCH}" in
    x86_64|amd64)  ARCH="x86_64" ;;
    aarch64|arm64) ARCH="aarch64" ;;
    *)             ARCH="${HOST_ARCH}" ;;
esac
TARBALL="alpine-minirootfs-${ALPINE_VERSION}-${ARCH}.tar.gz"
URL="https://dl-cdn.alpinelinux.org/alpine/v3.20/releases/${ARCH}/${TARBALL}"

mkdir -p "${ROOTFS_DIR}"

if [[ ! -x "${ROOTFS_DIR}/bin/sh" ]]; then
    echo "[coral] Downloading Alpine Linux minirootfs (${ALPINE_VERSION})..."
    curl -fsSL "${URL}" -o "${ROOTFS_DIR}/${TARBALL}"
    tar -xzf "${ROOTFS_DIR}/${TARBALL}" -C "${ROOTFS_DIR}"
    rm -f "${ROOTFS_DIR}/${TARBALL}"
fi

if [[ ! -f "${ROOTFS_DIR}/hello.txt" ]]; then
    echo "Hello from Coral Container Runtime!" > "${ROOTFS_DIR}/hello.txt"
fi

echo "[coral] Alpine rootfs is ready at ${ROOTFS_DIR}"
