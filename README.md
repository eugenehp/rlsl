# rlsl

[![CI](https://github.com/eugenehp/rlsl/actions/workflows/ci.yml/badge.svg)](https://github.com/eugenehp/rlsl/actions/workflows/ci.yml)
[![License: GPL-3.0](https://img.shields.io/badge/License-GPLv3-blue.svg)](LICENSE)

Rust rewrite of the [Real Life Streaming Layer](https://github.com/sccn/liblsl) (liblsl) core library.

Full protocol 1.00 + 1.10 compatibility, 162/162 C ABI symbols, Python bindings, WASM support,
and a modern recording/visualization toolchain — all in pure Rust.

## Workspace Layout

```
rlsl/
├── crates/
│   ├── rlsl/       Pure Rust LSL library (rlib)
│   ├── rlsl-sys/        C ABI shared library (liblsl.dylib/so/dll)
│   ├── rlsl-cli/        Unified CLI tool (`rrlsl list`, `rrlsl gen`, `rrlsl bench`)
│   ├── rlsl-rec/        Recording engine + TUI recorder (XDF/Parquet, HDF5 optional)
│   ├── rrlsl-rec-gui/    eGUI recorder with live viewer
│   ├── rlsl-py/         Python bindings (PyO3 + numpy)
│   ├── rlsl-wasm/       WebSocket bridge + WASM browser client
│   ├── rlsl-gen/        Synthetic signal generator
│   ├── rlsl-bench/      Throughput/latency benchmark
│   ├── rlsl-convert/    Offline format converter (XDF↔Parquet↔CSV)
│   ├── rlsl-fuzz/       Fuzz testing targets
│   └── exg/            XDF writer + NumericSample trait
├── docs/               Architecture documentation
└── deny.toml           cargo-deny license/dep auditing
```

## Quick Start

### Prerequisites (optional, for faster builds)

```sh
# Install mold linker (Linux — 5-10x faster linking)
sudo apt install mold        # Ubuntu/Debian
brew install mold             # macOS (linuxbrew)

# Install sccache (shared compilation cache)
cargo install sccache
```

The project auto-detects mold via `.cargo/config.toml` on Linux targets.
sccache is configured as `rustc-wrapper` in CI via environment variables.

```sh
# Build everything
cargo build

# Run tests
cargo test

# Run benchmarks
cargo bench

# Unified CLI
cargo run -p rlsl-cli -- list              # discover streams
cargo run -p rlsl-cli -- gen --srate 256   # generate signals
cargo run -p rlsl-cli -- bench             # run benchmarks

# Build C shared library
cargo build -p rlsl-sys
# → target/debug/liblsl.{dylib,so,dll}

# Build Python wheel
pip install maturin
maturin develop -m crates/rlsl-py/Cargo.toml
```

## Rust API

```rust
use rlsl::prelude::*;

// Create an outlet
let info = StreamInfo::new("MyStream", "EEG", 8, 250.0, ChannelFormat::Float32, "src1");
let outlet = StreamOutlet::new(&info, 0, 360);

// Push a sample
outlet.push_sample_f(&[1.0; 8], 0.0, true);
```

### Compatibility with liblsl

rlsl speaks liblsl's protocol and is checked against liblsl 1.18 in both directions; what it
adds — compressed feeds, replay for late consumers — is negotiated with headers a liblsl peer
ignores. The C library (`rlsl-sys`) is a drop-in for liblsl and behaves like it.

The Rust crate keeps three defaults of its own, for the sake of programs written against it:
`local_clock()` counts from the start of the process, `time_correction()` returns the remote
clock minus the local one, and `PROC_CLOCKSYNC` adds what the caller last measured. For liblsl's
behaviour instead — the machine clock, liblsl's sign, automatic synchronisation — build with

```toml
rlsl = { version = "0.1.0", features = ["liblsl-defaults"] }
```

or choose each at run time (`LSL_CLOCK=system`, `clock_offset()`, `set_clock_sync(true)`).

Where multicast does not get through (many Wi-Fi and institutional networks, anything across a
router), name the other hosts, as with liblsl:

```ini
# lsl_api.cfg (or LSL_KNOWN_PEERS=recorder.lab,10.0.0.204)
[lab]
KnownPeers = {recorder.lab, 10.0.0.204}
```

### Optional: what a late consumer missed

A stream that has just appeared sends its first samples before any recorder has found it. An
outlet can keep its last seconds for consumers that ask (marker streams do by default):

```rust
outlet.set_history(10.0);        // keep 10 s (irregular streams: already the default)

let inlet = StreamInlet::new(&info, 360, 0, true);
inlet.set_replay(10.0);          // before open_stream: send me what I missed
inlet.open_stream(10.0)?;
```

### Optional: compressed feeds

With the cargo feature `compression` (off by default; it adds the `zstd` C library), an inlet
can ask an rlsl outlet for a compressed feed — about 70 % of the bytes for an EEG stream, bit
for bit the same values:

```toml
rlsl = { version = "0.1.0", features = ["compression"] }
```

```rust
let inlet = StreamInlet::new(&info, 360, 0, true);
inlet.set_compression(true);     // before open_stream / the first pull
inlet.open_stream(10.0)?;
println!("compressed: {}", inlet.is_compressed());
```

An outlet that cannot compress (liblsl, an older rlsl, a build without the feature) sends the
plain protocol, and an inlet that does not ask gets exactly the bytes it always got.

### Optional: HDF5 recordings

`rlsl-rec` records XDF and Parquet out of the box. With the cargo feature `hdf5` (off by
default) it can also write one `.h5` file per recording, through
[`hdf5-no-c`](https://crates.io/crates/hdf5-no-c), a pure-Rust HDF5 implementation, so there's
no libhdf5 to install:

```bash
cargo install rlsl-rec --features hdf5   # in the TUI, press `f` until the format is HDF5
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
rlsl list                          # discover streams on the network
rlsl list --json                   # JSON output
rlsl list -w                       # continuously watch
rlsl gen --name EEG --channels 32  # generate synthetic EEG
rlsl bench --srate 10000           # throughput benchmark
rlsl version                       # show version info
```

## Docker

```sh
# Build image
docker build -t rlsl .

# List streams (host network for UDP multicast)
docker run --rm --net=host rrlsl list

# Record streams
docker run --rm --net=host -v ./data:/data rlsl rlsl-rec -o /data/recording.xdf

# Generate test signals
docker run --rm --net=host rlsl rlsl-gen --channels 8 --srate 250
```

## Examples

```sh
cargo run -p rlsl --example multi_stream       # multiple simultaneous streams
cargo run -p rlsl --example markers            # event markers (string streams)
cargo run -p rlsl --example signal_quality     # real-time quality monitoring
cargo run -p rlsl --example send_data          # basic sender
cargo run -p rlsl --example receive_data       # basic receiver
```

## Testing

```sh
cargo test                      # all tests
cargo test -p rlsl          # core unit + integration tests
cargo bench                     # criterion benchmarks
cargo bench -- serialize_110    # specific benchmark group

# Fuzz testing (requires nightly)
cd crates/rlsl-fuzz
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
