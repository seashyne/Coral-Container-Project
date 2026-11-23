#!/usr/bin/env bash
set -euo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION="${1:-2.8.0}"
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
# The state directories default to the per-user XDG paths below, so a fresh
# install works without any drop-in. Override them with a user drop-in in
# ~/.config/systemd/user/coral-daemon.service.d/override.conf.
ExecStart=/usr/bin/coral-daemon --listen 127.0.0.1:50051 --coral-bin /usr/bin/coral --state-dir %h/.local/state/coral/daemon --images-dir %h/.local/state/coral/images
Restart=on-failure
RestartSec=2
Environment=RUST_LOG=info

[Install]
WantedBy=default.target
EOF

# Post-install smoke check: verify the installed binaries actually run and
# that the daemon can answer a version query (backlog item #5).
cat > "$PKG_ROOT/DEBIAN/postinst" <<'EOF'
#!/bin/sh
set -e

case "$1" in
    configure)
        echo "coral-runtime: running the post-install smoke check..."
        if ! /usr/bin/coral --version >/dev/null 2>&1; then
            echo "coral-runtime: ERROR: /usr/bin/coral --version failed" >&2
            exit 1
        fi
        echo "coral-runtime: coral $(/usr/bin/coral --version 2>&1 | head -1)"

        if ! /usr/bin/coral-daemon --help >/dev/null 2>&1; then
            echo "coral-runtime: ERROR: /usr/bin/coral-daemon --help failed" >&2
            exit 1
        fi
        echo "coral-runtime: coral-daemon responds"

        echo "coral-runtime: start the daemon with: systemctl --user enable --now coral-daemon.service"
        ;;
    abort-upgrade|abort-remove|abort-deconfigure)
        ;;
    *)
        echo "postinst called with unknown argument '$1'" >&2
        exit 1
        ;;
esac

exit 0
EOF
chmod 0755 "$PKG_ROOT/DEBIAN/postinst"

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
 .
 The OCI engine, the CRI daemon, CNI networking, image management, signing
 and SBOM all work on both x86_64 and arm64. The hardware KVM MicroVM and
 the AI Agent sandbox are x86_64-only.
EOF

DEB_FILE="$OUT_DIR/coral-runtime_${VERSION}_${ARCH}.deb"
dpkg-deb --build --root-owner-group "$PKG_ROOT" "$DEB_FILE"
echo "[coral-deb] Built package: $DEB_FILE"
