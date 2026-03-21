# Multi-stage build for nautilus CLI using cargo-chef for dependency caching.
# Usage:
#   docker build -t nautilus-cli .
#   docker run --rm -v $(pwd):/work -w /work nautilus-cli init-ci
#   docker run --rm -v $(pwd):/work -v ~/.sui:/root/.sui -w /work nautilus-cli deploy-contract

# ── Stage 1: cargo-chef planner ──────────────────────────────────────
FROM lukemathwalker/cargo-chef:latest-rust-1 AS chef
WORKDIR /app

# ── Stage 2: capture dependency graph ────────────────────────────────
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# ── Stage 3: build dependencies (cached unless Cargo.toml/lock change)
FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json --features sui
COPY . .
RUN cargo build --release --features sui -p nautilus-cli

# ── Stage 4: minimal runtime (same Debian as cargo-chef to match glibc)
FROM debian:trixie-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
      ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/nautilus /usr/local/bin/nautilus
ENTRYPOINT ["nautilus"]
