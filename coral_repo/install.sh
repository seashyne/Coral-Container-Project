#!/usr/bin/env bash
set -euo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN_DIR="${CORAL_INSTALL_DIR:-$HOME/.local/bin}"
STATE_DIR="${XDG_STATE_HOME:-$HOME/.local/state}/coral"
IMAGES_DIR="${CORAL_IMAGES_DIR:-$STATE_DIR/images}"
VOLUMES_DIR="${CORAL_VOLUMES_DIR:-$STATE_DIR/volumes}"
SYSTEMD_USER_DIR="$HOME/.config/systemd/user"

echo "============================================================"
echo "  Coral Container Runtime v2.3 — System Installer (Linux)   "
echo "============================================================"
echo "  Host architecture: $(uname -m)"
echo "  (x86_64 and arm64 are supported; the KVM MicroVM and the"
echo "   AI Agent sandbox require an x86_64 host)"

# 1. Ensure Alpine 3.20 minirootfs is prepared
"$REPO_DIR/scripts/setup-rootfs.sh"

# 2. Build optimized release binaries
echo "[1/4] Compiling coral & coral-daemon (--release)..."
cd "$REPO_DIR"
cargo build --release -p coral-cli -p coral-daemon

# 3. Install binaries to ~/.local/bin (stopping daemon briefly if running so binary isn't busy)
echo "[2/4] Installing binaries to $BIN_DIR..."
mkdir -p "$BIN_DIR" "$IMAGES_DIR" "$VOLUMES_DIR"
if command -v systemctl >/dev/null 2>&1; then
    systemctl --user stop coral-daemon.service 2>/dev/null || true
fi
install -m 0755 "$REPO_DIR/target/release/coral" "$BIN_DIR/coral"
install -m 0755 "$REPO_DIR/target/release/coral-daemon" "$BIN_DIR/coral-daemon"

# 4. Seed default base images (alpine:3.20, alpine:latest, scratch) into user image store
echo "[3/4] Seeding default Linux base images into $IMAGES_DIR..."
ALPINE_DEST="$IMAGES_DIR/alpine-3.20/rootfs"
if [ ! -d "$ALPINE_DEST/bin" ]; then
    mkdir -p "$ALPINE_DEST"
    cp -a "$REPO_DIR/rootfs/." "$ALPINE_DEST/"
fi
"$BIN_DIR/coral" --images-dir "$IMAGES_DIR" pull alpine:3.20 --local --rootfs "$ALPINE_DEST" >/dev/null
"$BIN_DIR/coral" --images-dir "$IMAGES_DIR" pull alpine:latest --local --rootfs "$ALPINE_DEST" >/dev/null
"$BIN_DIR/coral" --images-dir "$IMAGES_DIR" pull scratch --local >/dev/null

# 5. Install and start systemd --user service for coral-daemon
echo "[4/4] Configuring systemd user service (coral-daemon.service)..."
mkdir -p "$SYSTEMD_USER_DIR"
cat > "$SYSTEMD_USER_DIR/coral-daemon.service" <<EOF
[Unit]
Description=Coral Container Runtime CRI v1 gRPC Daemon
After=network.target

[Service]
Type=simple
ExecStart=$BIN_DIR/coral-daemon --listen 127.0.0.1:50051 --coral-bin $BIN_DIR/coral --state-dir $STATE_DIR/daemon --images-dir $IMAGES_DIR --default-rootfs $ALPINE_DEST
Restart=on-failure
RestartSec=2
Environment=RUST_LOG=info

[Install]
WantedBy=default.target
EOF

if command -v systemctl >/dev/null 2>&1; then
    systemctl --user daemon-reload
    systemctl --user enable --now coral-daemon.service
    sleep 0.5
fi

echo ""
echo "Installed successfully!"
echo "  coral CLI:     $BIN_DIR/coral"
echo "  coral-daemon:  $BIN_DIR/coral-daemon"
echo "  Images store:  $IMAGES_DIR"
echo "  Volumes store: $VOLUMES_DIR"
echo ""
"$BIN_DIR/coral" version
