#!/usr/bin/env bash
# Coral Container Runtime — Official Linux Installer
# Usage: curl -fsSL https://seashyne.github.io/Coral-Container-Project/install.sh | bash
set -euo pipefail

REPO="seashyne/Coral-Container-Project"
VERSION="${CORAL_VERSION:-}"
BIN_DIR="${CORAL_INSTALL_DIR:-$HOME/.local/bin}"
STATE_DIR="${XDG_STATE_HOME:-$HOME/.local/state}/coral"
IMAGES_DIR="${CORAL_IMAGES_DIR:-$STATE_DIR/images}"
VOLUMES_DIR="${CORAL_VOLUMES_DIR:-$STATE_DIR/volumes}"
SYSTEMD_USER_DIR="$HOME/.config/systemd/user"

if [[ "$(uname -s)" != "Linux" ]]; then
  echo "[coral] Error: Coral runtime requires Linux (on Windows, run this inside WSL2)." >&2
  exit 1
fi

ARCH="$(uname -m)"
case "$ARCH" in
  x86_64|aarch64) ;;
  arm64) ARCH="aarch64" ;;
  *)
    echo "[coral] Error: Prebuilt binaries support x86_64 and aarch64 (detected: $ARCH)." >&2
    echo "[coral] Tip: Build from source with: git clone https://github.com/${REPO}.git && cd Coral-Container-Project/coral_repo && ./install.sh" >&2
    exit 1
    ;;
esac

if [[ -z "$VERSION" ]]; then
  VERSION="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" 2>/dev/null \
    | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n1 || true)"
  VERSION="${VERSION:-v2.6.0}"
fi

TARBALL="coral-${VERSION}-linux-${ARCH}.tar.gz"
BASE_URL="https://github.com/${REPO}/releases/download/${VERSION}"
TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

echo "============================================================"
echo "  Coral Container Runtime ${VERSION} — Web Installer (Linux)"
echo "============================================================"
echo "[1/4] Downloading ${TARBALL}..."
curl -fsSL "${BASE_URL}/${TARBALL}" -o "${TMP_DIR}/${TARBALL}"

if curl -fsSL "${BASE_URL}/SHA256SUMS.txt" -o "${TMP_DIR}/SHA256SUMS.txt" 2>/dev/null; then
  echo "[2/4] Verifying SHA-256 checksum..."
  EXPECTED="$(grep " ${TARBALL}$" "${TMP_DIR}/SHA256SUMS.txt" | awk '{print $1}' || true)"
  if [[ -n "$EXPECTED" ]] && command -v sha256sum >/dev/null 2>&1; then
    ACTUAL="$(sha256sum "${TMP_DIR}/${TARBALL}" | awk '{print $1}')"
    if [[ "$EXPECTED" != "$ACTUAL" ]]; then
      echo "[coral] Checksum mismatch! Expected ${EXPECTED}, got ${ACTUAL}" >&2
      exit 1
    fi
    echo "      SHA-256 verified OK."
  fi
else
  echo "[2/4] Skipping checksum verification (SHA256SUMS.txt not found)."
fi

echo "[3/4] Installing binaries to ${BIN_DIR}..."
tar -xzf "${TMP_DIR}/${TARBALL}" -C "${TMP_DIR}"
mkdir -p "$BIN_DIR" "$IMAGES_DIR" "$VOLUMES_DIR"
if command -v systemctl >/dev/null 2>&1; then
  systemctl --user stop coral-daemon.service 2>/dev/null || true
fi
install -m 0755 "${TMP_DIR}/coral/coral" "${BIN_DIR}/coral"
install -m 0755 "${TMP_DIR}/coral/coral-daemon" "${BIN_DIR}/coral-daemon"

ALPINE_DEST="${IMAGES_DIR}/alpine-3.20/rootfs"
if [[ ! -x "${ALPINE_DEST}/bin/sh" ]]; then
  echo "[coral] Downloading Alpine Linux 3.20 minirootfs (${ARCH})..."
  mkdir -p "$ALPINE_DEST"
  curl -fsSL "https://dl-cdn.alpinelinux.org/alpine/v3.20/releases/${ARCH}/alpine-minirootfs-3.20.3-${ARCH}.tar.gz" \
    | tar -xz -C "$ALPINE_DEST"
fi
"${BIN_DIR}/coral" --images-dir "$IMAGES_DIR" pull alpine:3.20 --local --rootfs "$ALPINE_DEST" >/dev/null
"${BIN_DIR}/coral" --images-dir "$IMAGES_DIR" pull alpine:latest --local --rootfs "$ALPINE_DEST" >/dev/null
"${BIN_DIR}/coral" --images-dir "$IMAGES_DIR" pull scratch --local >/dev/null

echo "[4/4] Configuring systemd user service (coral-daemon.service)..."
mkdir -p "$SYSTEMD_USER_DIR"
cat > "${SYSTEMD_USER_DIR}/coral-daemon.service" <<EOF
[Unit]
Description=Coral Container Runtime CRI v1 gRPC Daemon
After=network.target

[Service]
Type=simple
ExecStart=${BIN_DIR}/coral-daemon --listen 127.0.0.1:50051 --coral-bin ${BIN_DIR}/coral --state-dir ${STATE_DIR}/daemon --images-dir ${IMAGES_DIR} --default-rootfs ${ALPINE_DEST}
Restart=on-failure
RestartSec=2
Environment=RUST_LOG=info

[Install]
WantedBy=default.target
EOF

if command -v systemctl >/dev/null 2>&1; then
  systemctl --user daemon-reload || true
  systemctl --user enable --now coral-daemon.service || true
  sleep 0.5
fi

echo ""
echo "Coral ${VERSION} installed successfully!"
echo "  CLI:    ${BIN_DIR}/coral"
echo "  Daemon: ${BIN_DIR}/coral-daemon"
if [[ ":$PATH:" != *":${BIN_DIR}:"* ]]; then
  echo ""
  echo "Add ${BIN_DIR} to your PATH:"
  echo "  export PATH=\"${BIN_DIR}:\$PATH\""
fi
echo ""
"${BIN_DIR}/coral" version || true
