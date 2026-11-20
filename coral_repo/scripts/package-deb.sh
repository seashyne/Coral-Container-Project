#!/usr/bin/env bash
set -euo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION="${1:-2.1.0}"
ARCH="$(dpkg --print-architecture 2>/dev/null || echo amd64)"
OUT_DIR="$REPO_DIR/target/packages"
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

echo "[coral-deb] Building release binaries..."
cd "$REPO_DIR"
cargo build --release -p coral-cli -p coral-daemon

PKG_ROOT="$STAGE/coral-runtime_${VERSION}_${ARCH}"
mkdir -p \
  "$PKG_ROOT/DEBIAN" \
  "$PKG_ROOT/usr/bin" \
  "$PKG_ROOT/usr/lib/systemd/user" \
  "$PKG_ROOT/usr/share/coral" \
  "$OUT_DIR"

install -m 0755 "$REPO_DIR/target/release/coral" "$PKG_ROOT/usr/bin/coral"
install -m 0755 "$REPO_DIR/target/release/coral-daemon" "$PKG_ROOT/usr/bin/coral-daemon"
install -m 0755 "$REPO_DIR/scripts/setup-rootfs.sh" "$PKG_ROOT/usr/share/coral/setup-rootfs.sh"

cat > "$PKG_ROOT/usr/lib/systemd/user/coral-daemon.service" <<'EOF'
[Unit]
Description=Coral Container Runtime CRI v1 gRPC Daemon
After=network.target

[Service]
Type=simple
ExecStart=/usr/bin/coral-daemon --listen 127.0.0.1:50051 --coral-bin /usr/bin/coral
Restart=on-failure
RestartSec=2
Environment=RUST_LOG=info

[Install]
WantedBy=default.target
EOF

cat > "$PKG_ROOT/DEBIAN/control" <<EOF
Package: coral-runtime
Version: ${VERSION}
Section: admin
Priority: optional
Architecture: ${ARCH}
Maintainer: Coral Container Project <https://github.com/seashyne/Coral-Container-Project>
Recommends: slirp4netns | passt
Description: Memory-safe Rust OCI, WebAssembly, and KVM MicroVM Container Runtime
 Coral provides a fast, rootless-first Linux container runtime (coral) and
 Kubernetes CRI v1 gRPC daemon (coral-daemon) with OverlayFS CoW, Dockerfile
 builds with step cache, Compose stacks, OCI registry push/pull, live PTY
 streaming, WASI Preview 1 WebAssembly execution (coral-wasm), and hardware
 KVM 64-bit MicroVM / AI Agent Sandboxing (coral-vmm).
EOF

DEB_FILE="$OUT_DIR/coral-runtime_${VERSION}_${ARCH}.deb"
dpkg-deb --build --root-owner-group "$PKG_ROOT" "$DEB_FILE"
echo "[coral-deb] Built package: $DEB_FILE"
