# ── Builder stage ─────────────────────────────────────────────────────
FROM rust:1.85-bookworm AS builder

WORKDIR /build
COPY . .

# Build release binaries (excluding GUI — no display in container)
RUN cargo build --release \
    -p lsl-rec \
    -p lsl-sys \
    -p lsl-cli \
    -p lsl-gen \
    -p lsl-bench \
    -p lsl-convert \
    && cargo build --release -p lsl-wasm --features bridge --bin lsl-bridge

# ── Runtime stage ────────────────────────────────────────────────────
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Copy binaries
COPY --from=builder /build/target/release/lsl /usr/local/bin/lsl
COPY --from=builder /build/target/release/lsl-rec /usr/local/bin/lsl-rec
COPY --from=builder /build/target/release/lsl-gen /usr/local/bin/lsl-gen
COPY --from=builder /build/target/release/lsl-bench /usr/local/bin/lsl-bench
COPY --from=builder /build/target/release/lsl-convert /usr/local/bin/lsl-convert
COPY --from=builder /build/target/release/lsl-bridge /usr/local/bin/lsl-bridge

# Copy shared library
COPY --from=builder /build/target/release/liblsl.so /usr/local/lib/liblsl.so
RUN ldconfig

# Data directory for recordings
VOLUME /data
WORKDIR /data

# Default: show available streams
ENTRYPOINT ["lsl"]
CMD ["list"]

# ── Labels ───────────────────────────────────────────────────────────
LABEL org.opencontainers.image.title="lsl-rs"
LABEL org.opencontainers.image.description="Lab Streaming Layer (Rust) — recorder, generator, bridge"
LABEL org.opencontainers.image.source="https://github.com/eugenehp/lsl-rs"
LABEL org.opencontainers.image.licenses="MIT"
