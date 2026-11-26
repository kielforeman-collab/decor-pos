# decor-pos — multi-stage build for `df-server` and `df-api`.
#
# Build (from the repo root):
#   docker build -t decor-pos:latest .
#
# The image carries both binaries; pick one with the container command:
#   df-server edge --db sqlite:/data/edge.db --store-id <uuid> \
#       --listen 0.0.0.0:8080
#   df-api --db sqlite:/data/edge.db --listen 0.0.0.0:3000
# See docker-compose.yml for the ready-made stack and
# docs/self-hosting.md for the full guide.
#
# Build-time network: the builder fetches crates.io dependencies
# (Cargo.lock pins exact versions; `--locked` enforces it). The
# runtime image has no network requirement beyond the services
# themselves. No live database is needed at build time
# (SQLX_OFFLINE=true in .cargo/config.toml; sqlx uses
# runtime-checked queries only).

# ---- builder ---------------------------------------------------------
# Rust 1.99: the toolchain the workspace is developed and tested
# against (workspace `rust-version` floor is 1.85, but the code uses
# let-chains, stabilized in 1.88 — do not build with anything older
# than 1.88). If the `1.99` tag does not exist yet on Docker Hub,
# use the closest newer `rust:<version>-bookworm` tag.
FROM rust:1.99-bookworm AS builder

WORKDIR /build

# Manifests first so dependency fetching layers cache well.
COPY Cargo.toml Cargo.lock ./
COPY .cargo .cargo
COPY crates crates
# The Tauri desktop app is a workspace member, so its manifest must
# be present for workspace resolution even though we do not build it.
COPY apps/pos-desktop/src-tauri apps/pos-desktop/src-tauri

RUN cargo build --release --locked -p df-server -p df-api

# ---- runtime ---------------------------------------------------------
FROM debian:bookworm-slim

# curl only: used by the HEALTHCHECK / compose healthchecks against
# GET /health. ca-certificates for the (future) live Stripe/EDI
# endpoints — no outbound calls are made by the v1.0 binaries.
RUN apt-get update \
    && apt-get install -y --no-install-recommends curl ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Run as an unprivileged user; /data is the SQLite volume.
RUN useradd -m -u 10001 decor \
    && mkdir -p /data \
    && chown decor:decor /data

COPY --from=builder /build/target/release/df-server /usr/local/bin/df-server
COPY --from=builder /build/target/release/df-api /usr/local/bin/df-api

USER decor
WORKDIR /data

# df-server edge listens on 8080, df-api on 3000 (both overridden by
# --listen; the compose file maps them to the host).
EXPOSE 8080 3000

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD curl -fsS http://127.0.0.1:8080/health || exit 1

# Default: the store edge daemon. Override the command for the API:
#   command: ["df-api", "--db", "sqlite:/data/edge.db",
#             "--listen", "0.0.0.0:3000", ...]
CMD ["df-server", "edge", "--db", "sqlite:/data/edge.db", \
     "--listen", "0.0.0.0:8080"]
