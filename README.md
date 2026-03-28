# lsl-rs

[![CI](https://github.com/eugenehp/lsl-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/eugenehp/lsl-rs/actions/workflows/ci.yml)
[![License: GPL-3.0](https://img.shields.io/badge/License-GPLv3-blue.svg)](LICENSE)

Rust rewrite of the [Lab Streaming Layer](https://github.com/sccn/liblsl) (liblsl) core library.

Full protocol 1.00 + 1.10 compatibility, 162/162 C ABI symbols, Python bindings, WASM support,
and a modern recording/visualization toolchain — all in pure Rust.

## Workspace Layout

```
lsl-rs/
├── crates/
│   ├── lsl-core/       Pure Rust LSL library (rlib)
│   ├── lsl-sys/        C ABI shared library (liblsl.dylib/so/dll)
│   ├── lsl-cli/        Unified CLI tool (`lsl list`, `lsl gen`, `lsl bench`)
│   ├── lsl-rec/        Recording engine + TUI recorder (XDF/Parquet/HDF5)
│   ├── lsl-rec-gui/    eGUI recorder with live viewer
│   ├── lsl-py/         Python bindings (PyO3 + numpy)
│   ├── lsl-wasm/       WebSocket bridge + WASM browser client
│   ├── lsl-gen/        Synthetic signal generator
│   ├── lsl-bench/      Throughput/latency benchmark
│   ├── lsl-convert/    Offline format converter (XDF↔Parquet↔CSV)
│   ├── lsl-fuzz/       Fuzz testing targets
│   └── exg/            XDF writer + NumericSample trait
├── docs/               Architecture documentation
└── deny.toml           cargo-deny license/dep auditing
```

## Quick Start

```sh
# Build everything
cargo build

# Run tests
cargo test

# Run benchmarks
cargo bench

# Unified CLI
cargo run -p lsl-cli -- list              # discover streams
cargo run -p lsl-cli -- gen --srate 256   # generate signals
cargo run -p lsl-cli -- bench             # run benchmarks

# Build C shared library
cargo build -p lsl-sys
# → target/debug/liblsl.{dylib,so,dll}

# Build Python wheel
pip install maturin
maturin develop -m crates/lsl-py/Cargo.toml
```

## Rust API

```rust
use lsl_core::prelude::*;

// Create an outlet
let info = StreamInfo::new("MyStream", "EEG", 8, 250.0, ChannelFormat::Float32, "src1");
let outlet = StreamOutlet::new(&info, 0, 360);

// Push a sample
outlet.push_sample_f(&[1.0; 8], 0.0, true);
```

## Python API

```python
import pylsl

info = pylsl.StreamInfo("MyStream", "EEG", 8, 250.0, pylsl.CF_FLOAT32, "src1")
outlet = pylsl.StreamOutlet(info)
outlet.push_sample([1.0] * 8)

streams = pylsl.resolve_streams(timeout=2.0)
inlet = pylsl.StreamInlet(streams[0])
sample, timestamp = inlet.pull_sample()
chunk, timestamps = inlet.pull_chunk()  # → numpy arrays!
```

## CLI Tool

```sh
lsl list                          # discover streams on the network
lsl list --json                   # JSON output
lsl list -w                       # continuously watch
lsl gen --name EEG --channels 32  # generate synthetic EEG
lsl bench --srate 10000           # throughput benchmark
lsl version                       # show version info
```

## Docker

```sh
# Build image
docker build -t lsl-rs .

# List streams (host network for UDP multicast)
docker run --rm --net=host lsl-rs list

# Record streams
docker run --rm --net=host -v ./data:/data lsl-rs lsl-rec -o /data/recording.xdf

# Generate test signals
docker run --rm --net=host lsl-rs lsl-gen --channels 8 --srate 250
```

## Examples

```sh
cargo run -p lsl-core --example multi_stream       # multiple simultaneous streams
cargo run -p lsl-core --example markers            # event markers (string streams)
cargo run -p lsl-core --example signal_quality     # real-time quality monitoring
cargo run -p lsl-core --example send_data          # basic sender
cargo run -p lsl-core --example receive_data       # basic receiver
```

## Testing

```sh
cargo test                      # all tests
cargo test -p lsl-core          # core unit + integration tests
cargo bench                     # criterion benchmarks
cargo bench -- serialize_110    # specific benchmark group

# Fuzz testing (requires nightly)
cd crates/lsl-fuzz
cargo +nightly fuzz run fuzz_sample_110 -- -max_total_time=300
cargo +nightly fuzz run fuzz_query_match -- -max_total_time=300
```

## Architecture

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for detailed documentation including:
- Crate dependency graph
- Threading model
- Protocol wire formats
- Discovery and data streaming flows

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines on:
- Development setup
- Code style and commit conventions
- Pull request process
- Testing requirements

## Security

See [SECURITY.md](SECURITY.md) for vulnerability reporting.

**Note:** LSL is designed for local network use. It does not provide authentication
or encryption. Do not expose LSL streams to untrusted networks.

## License

GPLv3 — see [LICENSE](LICENSE)
