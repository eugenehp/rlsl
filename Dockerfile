# ── Builder stage ─────────────────────────────────────────────────────
FROM rust:latest AS builder

# Install mold linker for fast linking
RUN apt-get update && apt-get install -y --no-install-recommends mold \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build
COPY . .

# Build release binaries (excluding GUI — no display in container)
RUN cargo build --release \
    -p rlsl-rec \
    -p rlsl-sys \
    -p rlsl-cli \
    -p rlsl-gen \
    -p rlsl-bench \
    -p rlsl-convert \
    && cargo build --release -p rlsl-wasm --features bridge --bin rlsl-bridge

# ── Runtime stage (must match builder's glibc version) ───────────────
FROM debian:sid-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Copy binaries
COPY --from=builder /build/target/release/rlsl /usr/local/bin/rlsl
COPY --from=builder /build/target/release/rlsl-rec /usr/local/bin/rlsl-rec
COPY --from=builder /build/target/release/rlsl-gen /usr/local/bin/rlsl-gen
COPY --from=builder /build/target/release/rlsl-bench /usr/local/bin/rlsl-bench
COPY --from=builder /build/target/release/rlsl-convert /usr/local/bin/rlsl-convert
COPY --from=builder /build/target/release/rlsl-bridge /usr/local/bin/rlsl-bridge

# Copy shared library
COPY --from=builder /build/target/release/liblsl.so /usr/local/lib/liblsl.so
RUN ldconfig

# Data directory for recordings
VOLUME /data
WORKDIR /data

# Default: show available streams
ENTRYPOINT ["rlsl"]
CMD ["list"]

# ── Labels ───────────────────────────────────────────────────────────
LABEL org.opencontainers.image.title="rlsl"
LABEL org.opencontainers.image.description="Real Life Streaming Layer (Rust) — recorder, generator, bridge"
LABEL org.opencontainers.image.source="https://github.com/eugenehp/rlsl"
LABEL org.opencontainers.image.licenses="GPL-3.0-only"
