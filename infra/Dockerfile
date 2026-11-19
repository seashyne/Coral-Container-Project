# --- Stage 1: Build ---
FROM rust:1-slim AS builder

RUN apt-get update && apt-get install -y pkg-config libssl-dev curl && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY coral_repo .

RUN cargo build --workspace --release

# --- Stage 2: Runtime ---
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y libssl3 ca-certificates curl && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=builder /app/target/release/coral /usr/local/bin/coral
COPY --from=builder /app/target/release/coral-daemon /usr/local/bin/coral-daemon
COPY coral_repo/config.json /app/config.json
COPY coral_repo/scripts /app/scripts
RUN /app/scripts/setup-rootfs.sh

EXPOSE 50051
CMD ["coral-daemon", "--listen", "0.0.0.0:50051", "--coral-bin", "/usr/local/bin/coral", "--default-rootfs", "/app/rootfs"]
