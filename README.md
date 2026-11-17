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
adds — compressed feeds, replay for late consumers, clock records — is negotiated with headers a
liblsl peer ignores. The C library (`rlsl-sys`) is a drop-in for liblsl and behaves like it, and
the Python module like pylsl (`time_correction()` included, since 0.1.1).

The Rust crate keeps three defaults of its own, for the sake of programs written against it:
`local_clock()` counts from the start of the process, `time_correction()` returns the remote
clock minus the local one, and `PROC_CLOCKSYNC` adds what the caller last measured. For liblsl's
behaviour instead — the machine clock, liblsl's sign, automatic synchronisation — build with

```toml
rlsl = { version = "0.1.2", features = ["liblsl-defaults"] }
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
rlsl = { version = "0.1.2", features = ["compression"] }
```

```rust
let inlet = StreamInlet::new(&info, 360, 0, true);
inlet.set_compression(true);     // before open_stream / the first pull
inlet.open_stream(10.0)?;
println!("compressed: {}", inlet.is_compressed());
```

An outlet that cannot compress (liblsl, an older rlsl, a build without the feature) sends the
plain protocol, and an inlet that does not ask gets exactly the bytes it always got.

### Optional: NTP server and client

With the cargo feature `ntp` (off by default; no extra dependencies) rlsl speaks standard NTP
(SNTPv4) as well as LSL's own clock synchronisation. Serve the recording machine's clock to
devices that only know NTP, or ask any NTP server where this machine's clock stands — including
the offset from `local_clock()` to the server's UTC, to put LSL time stamps on absolute time:

```toml
rlsl = { version = "0.1.2", features = ["ntp"] }
```

```rust
use rlsl::ntp::{self, NtpServer};

let server = NtpServer::start("0.0.0.0:123".parse()?)?;   // stops when dropped

let s = ntp::sync("pool.ntp.org", 4, 5.0)?;               // best of 4 probes
println!("offset {:+.6} s, delay {:.6} s", s.offset, s.delay);
let utc = s.unix_time(rlsl::clock::local_clock());
```

To follow clocks over time, `ClockTracker` keeps a window of measurements and reports the mean,
min/max, jitter (standard deviation) and drift in ppm. `ClockMesh` does this across a group of
machines: each node serves NTP and measures every peer it knows, and the nodes swap their tables
(over TCP, on the same port number), so every node holds the full who-is-how-far-from-whom
matrix, including pairs it doesn't measure itself:

```rust
use rlsl::ntp::{ClockMesh, ClockMeshConfig};

let mesh = ClockMesh::start(ClockMeshConfig {
    peers: vec!["10.0.0.172".into(), "10.0.0.204".into()],   // port 12300 by default
    ..ClockMeshConfig::default()
})?;
for row in mesh.matrix() {            // also: mesh.matrix_json(), mesh.history(peer)
    println!("{} → {}: {:+.3} ms, drift {:?} ppm", row.from, row.to,
             row.stats.mean_offset * 1e3, row.stats.drift_ppm);
}
```

The mesh measures two clocks per peer: the wall clocks (NTP) and the monotonic clocks LSL time
stamps are taken on (`ClockKind::Mono`, LSL's own time probe). Only the latter correct LSL data,
and only for outlets that time-stamp with the machine clock (liblsl does; rlsl with
`LSL_CLOCK=system` or the `liblsl-defaults` feature).

#### Clock measurements with the data, for offline correction

An outlet passes its mesh's measurements on with the stream, and a recorder keeps them in the
same file as the data — pick any combination:

```rust
use rlsl::ntp::ClockTransport;

outlet.attach_clock_mesh(&mesh, ClockTransport::BOTH);   // or IN_BAND / COMPANION

// 1. in-band: inside the stream's own connection (rlsl inlets that ask; liblsl sees nothing)
inlet.set_clock_records(true);                          // before open_stream
let records = inlet.pull_clock_records();               // Vec<ClockRecord>, history first

// 2. companion: a stream "<name> (clock)", type ClockSync, source id "<source_id>#clock",
//    that any LSL recorder (LabRecorder too) can record
```

`rlsl-rec` (feature `ntp`) records them, chosen with `RLSL_REC_CLOCK=in-band,companion,mesh`:
in-band records become a `ClockSync` stream next to their data stream; companion streams are
added to the recording; and with `mesh` (and `RLSL_REC_CLOCK_PEERS`) the recorder measures the
source machines itself and writes those offsets as the streams' XDF clock offsets — checked
against LSL's own probe, which is used instead when they disagree. Each stream's footer says
which (`<clock_offsets><from_clock_mesh>`/`<from_lsl_probe>`).

Offline, `ClockFit::from_records(&records, "msi", "mm4", ClockKind::Mono)` fits a
delay-weighted line (offset and drift) to the records; add `fit.offset_at(t)` to a time stamp
of `msi` to put it on `mm4`'s clock. Each record is `from, to, clock, at, offset, delay,
at_unix`: `to`'s clock is `offset` seconds ahead of `from`'s at time `at` on `from`'s clock.

The CLI has it behind the same feature (`ntp serve`, `ntp query`, `ntp track <server>`,
`ntp mesh --peer host1,host2`, and `gen --clock-peers host1,host2` for a test signal that
carries its measurements): `cargo run -p rlsl-cli --features ntp -- ntp serve
--port 12300` and `... -- ntp query pool.ntp.org`.

### Optional: HDF5 recordings

`rlsl-rec` records XDF and Parquet out of the box. With the cargo feature `hdf5` (off by
default) it can also write one `.h5` file per recording, through
[`hdf5-no-c`](https://crates.io/crates/hdf5-no-c), a pure-Rust HDF5 implementation, so there's
no libhdf5 to install:

```bash
cargo install rlsl-rec --features hdf5   # in the TUI, press `f` until the format is HDF5
```

## Discovery by Bonjour / Avahi (feature `mdns`)

```toml
rlsl = { version = "0.1", features = ["mdns"] }
```

Every outlet is then also announced by DNS-SD as a `_lsl._tcp` service (`dns-sd -B _lsl._tcp`,
`avahi-browse -r _lsl._tcp` show it, with the stream's name, type and source id in its TXT
record), and a resolver also asks the machines that announce streams, directly. Streams are
found on networks that carry mDNS but not LSL's own multicast groups; what a resolver returns
is the same either way, and peers without the feature — liblsl included — notice nothing.
`LSL_MDNS=0` switches it off at run time. Like LSL's own discovery it stays on the local
link: across routers use an mDNS reflector, or `KnownPeers`.

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
