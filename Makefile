# Coral Container Project — Unified Workspace Makefile

.PHONY: help build test clippy verify install clean desktop-install desktop-dev desktop-build desktop-check

help:
	@echo "Coral Container Project targets:"
	@echo "  make build          - Build coral & coral-daemon (debug)"
	@echo "  make test           - Run coral_repo workspace tests"
	@echo "  make clippy         - Run strict Clippy (-D warnings) on coral_repo"
	@echo "  make verify         - Run full coral_repo/verify.sh gate"
	@echo "  make install        - Build release & install coral/coral-daemon locally"
	@echo "  make desktop-dev    - Launch Coral Desktop (Tauri dev)"
	@echo "  make desktop-build  - Build Coral Desktop installers"
	@echo "  make desktop-check  - Run cargo check & clippy on desktop/src-tauri"

build:
	cd coral_repo && cargo build --workspace

test:
	cd coral_repo && cargo test --workspace

clippy:
	cd coral_repo && cargo clippy --workspace --all-targets -- -D warnings

verify:
	cd coral_repo && ./verify.sh

install:
	cd coral_repo && ./install.sh

clean:
	cd coral_repo && cargo clean
	cd desktop/src-tauri && cargo clean

desktop-install:
	cd desktop && npm install

desktop-dev:
	cd desktop && npm run dev

desktop-build:
	cd desktop && npm run build

desktop-check:
	cd desktop/src-tauri && cargo check && cargo clippy --all-targets -- -D warnings
