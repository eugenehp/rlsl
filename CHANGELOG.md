# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.0.8] — 2026-11-14

### Added

- **64-bit chunk API**: `StreamOutlet::push_chunk_d` / `push_chunk_i64` (and `push_chunk_i32` /
  `push_chunk_i16`), `StreamInlet::pull_chunk_i64` (exact over the whole `i64` range, which
  `pull_chunk_d` is not above 2^53), `pull_chunk_i32` and `pull_chunk_f`. Double and 64-bit
  integer streams could already be pushed and pulled sample by sample; `tests/sixty_four_bit.rs`
  now checks them bit for bit across the network, by sample and by chunk.
- **`--format` for the generators**: `rlsl-gen` and `rlsl gen` take
  `--format float32|double64|int64|int32|int16|int8` (default `float32`).

### Refactored

- The typed `push_sample_*` / `pull_sample_*` / `push_chunk_*` / `pull_chunk_*` methods share
  one implementation each instead of a copy per type.

### Added (opt-in; defaults are unchanged)

Nothing below changes what existing code gets: `local_clock()`, `time_correction()` and the
`PROC_CLOCKSYNC` flag behave exactly as in 0.0.7 unless asked otherwise.

- **Machine clock**: `clock::system_clock()` reads the machine's monotonic clock, the same in
  every process (liblsl's `steady_clock`: `CLOCK_MONOTONIC` on Linux, `CLOCK_MONOTONIC_RAW` on
  macOS, the performance counter on Windows). `clock::set_clock_source(ClockSource::System)`,
  `clock = system` in `lsl_api.cfg` or `LSL_CLOCK=system` make it the source of `local_clock()`,
  so that time stamps of programs on one machine are directly comparable. The default remains
  seconds since the process first read the clock.
- **`StreamInlet::clock_offset()`** (and `time_receiver::clock_offset`): the number to *add* to a
  stream's time stamps to get local time — liblsl's `time_correction()` — or `None` when the
  outlet does not answer. `time_correction()` keeps its historical sign (remote minus local,
  the opposite) and its `0.0` on no answer; its documentation now says so.
- **`StreamInlet::set_clock_sync(true)`**: with `PROC_CLOCKSYNC`, the inlet estimates its clock
  offset before the first sample is stamped and renews it every `time_update_interval` (2 s),
  so time stamps come out on the local clock with no manual call. Without it the flag adds
  what the last `time_correction()` / `clock_offset()` call stored, as before.

### Changed

- **Inlets connect to the address the resolver saw**, IPv4 first, before guessing loopback. They
  used to try IPv6 first, which for a stream on another machine is a link-local address without
  its interface. Streams that connected before still do.

### Fixed

- **Resolved streams could lack a usable address**: an outlet answers a query over IPv4 and
  IPv6, and only the first reply was kept — when the IPv6 one won, the stream had no IPv4
  address at all, and connections (data, `LSL:fullinfo`) to its link-local IPv6 address failed.
  Replies for one stream are now merged, and a resolve that already has enough streams waits up
  to 100 ms for their other address.
- **`cargo clippy --workspace --all-targets -- -D warnings` failed** (clippy 1.99 lints in
  `rlsl-gen`, `rlsl-bench`, `rlsl-wasm`, `rlsl-iroh` tests, `rlsl` tests and examples).
- **`rlsl-convert` read every XDF stream as float32**: double and 64-bit integer streams (8 bytes
  per value), 16- and 8-bit integers and string streams were parsed four bytes at a time and came
  out as garbage. Each stream is now read in the width of its `channel_format`, integers are kept
  as integers (no rounding above 2^53) with integer / string Parquet columns, and a chunk cut
  short ends at its last complete sample. 8-byte sample counts are accepted. float32 streams —
  the only ones that worked — convert exactly as before (Float64 columns).

## [0.0.7] — 2026-11-08

### Fixed
- Outlet TCP server: the stream feed of each consumer now runs on the blocking pool. Its wait
  for samples used to block a shared async worker without yielding (indefinitely while no
  samples were pushed), so with consumers connected the outlet could stop answering
  `LSL:fullinfo` / `LSL:shortinfo` requests and new connections
- `StreamInlet::get_fullinfo` fetches the outlet's full info (`LSL:fullinfo`) like liblsl's
  `lsl_get_fullinfo`, so the description reaches inlets; the result is cached per inlet and the
  resolver's short info is returned if the outlet cannot be reached
- `xml_unescape` decodes in a single pass (`&amp;lt;` stays the text `&lt;`) and handles numeric
  character references

### Added
- `StreamInfo::from_fullinfo_message` (header fields plus the `<desc>` tree)
- `XmlNode::append_parsed`: parse an XML fragment (liblsl / pugixml style, indented or not) into
  child elements
- `StreamInlet::info()`: the stream info the inlet was created from, without network access
  (`rlsl-sys` uses it for the channel count on every pull)

## [0.0.6] — 2026-10-26

### Security
- `rlsl-iroh`: bump `zstd` 0.13 → 0.14 (RUSTSEC-2026-0391, plus `zstd-safe` 7.x advisories
  RUSTSEC-2026-0395..0406)
- `rlsl-rec`, `rlsl-convert`: bump `arrow` / `parquet` 54 → 60 to drop the transitive `zstd` 0.13
- Refresh transitive deps for `rustls`, `rustls-webpki`, `webpki-roots`, `webpki-root-certs`,
  `h2`, `quick-xml`, `crossbeam-epoch`, `ctutils`, `webbrowser` advisories
- `deny.toml`: ignore RUSTSEC-2026-0192 (`ttf-parser` unmaintained, via egui); drop stale
  RUSTSEC-2023-0089 ignore

### Changed
- Use `as_chunks` / `slice::fill` to satisfy clippy 1.99 lints

## [0.0.3] — 2026-03-28

### Added
- **`rlsl-iroh` crate** — tunnel LSL streams over iroh peer-to-peer QUIC connections
  - Source/sink architecture: streams forwarded transparently, re-published as local LSL outlets
  - Zero data loss guarantee via reliable QUIC streams, unbounded channels, 32K-sample inlet buffer
  - 6 compression codecs: none, lz4, zstd (L1/L3), snappy, delta-lz4
  - Low-latency QUIC transport tuning (immediate ACKs, zero ACK delay, 1ms initial RTT)
  - Optional lossy datagram mode behind `lossy-datagrams` feature flag
  - In-process benchmark (`rlsl-iroh bench`) with zero-loss assertion
  - Continuous stream discovery (`--continuous`)
  - Connection tickets for easy node-id exchange
  - 23 tests (8 unit, 9 protocol roundtrip, 6 end-to-end iroh tunnel)
- **Benchmark suite** in `figures/`
  - 43-configuration sweep: codecs × channels × sample rates × formats
  - 8 auto-generated charts (codec latency, channel/rate scaling, format comparison, etc.)
  - `run_benchmarks.sh` + `plot_benchmarks.py` for reproducibility
  - All 43 runs: 0.00% data loss, ~52µs mean latency at 10 MB/s throughput

## [Unreleased]

### Added
- `rlsl-cli` — unified CLI tool combining gen, record, convert, bench, and list commands
- `rlsl-fuzz` — fuzz testing targets for protocol parsers and XML DOM
- `docs/ARCHITECTURE.md` — detailed architecture documentation with crate dependency graph
- `CONTRIBUTING.md` — contribution guidelines
- `SECURITY.md` — vulnerability disclosure policy
- `CHANGELOG.md` — this file
- `Dockerfile` — container image for rlsl-rec and rlsl-gen
- Comprehensive unit tests for `sample`, `xml_dom`, `postproc`, `signal_quality`, `clock`,
  `send_buffer`, `config`, and `stream_info` modules
- Criterion benchmarks for sample serialization, XML DOM operations, and push/pull throughput
- Workspace-level examples: `multi_stream.rs`, `markers.rs`, `recording.rs`
- `cargo-deny` configuration for license and dependency auditing
- Cross-platform CI matrix (Linux x86_64, macOS aarch64, Windows x86_64)
- npm publish workflow for WASM package

## [0.1.0] — 2026-03-28

### Added
- **rlsl**: Pure-Rust LSL implementation with full protocol 1.00 + 1.10 support
  - `StreamInfo` — stream metadata with XPath-like query matching
  - `StreamOutlet` — publish data on the network
  - `StreamInlet` — receive data with automatic recovery
  - UDP multicast/broadcast discovery (IPv4 + IPv6 dual-stack)
  - TCP data streaming with protocol handshake
  - NTP-like time correction probing
  - Timestamp post-processing (dejitter, clocksync, monotonize)
  - Mutable XML DOM for `<desc>` metadata
  - Signal quality metrics (SNR, jitter, dropout detection)
  - Config file loading (`lsl_api.cfg`)
- **rlsl-sys**: C ABI shared library — 162/162 `extern "C"` symbols, drop-in for liblsl
- **exg**: XDF file writer and `NumericSample` trait
- **rlsl-rec**: Recording engine with XDF, Parquet, and HDF5 output + ratatui TUI
- **rrlsl-rec-gui**: eGUI recorder with live signal viewer and stream inspector
- **rlsl-py**: Python bindings via PyO3 + numpy (StreamInfo, StreamOutlet, StreamInlet, resolver)
- **rlsl-wasm**: WebSocket bridge server + WASM browser client
- **rlsl-gen**: Synthetic signal generator (sine, square, noise, chirp, sawtooth, counter)
- **rlsl-bench**: Throughput and latency benchmarking tool
- **rlsl-convert**: Offline format converter (XDF → Parquet, XDF → CSV, Parquet → CSV)
- CI workflows for lint, test (Linux/macOS/Windows), WASM, and Python wheels
- Release workflow with GitHub Releases + PyPI publishing

### Known Gaps
- mDNS/Bonjour discovery not implemented (LSL uses UDP multicast as primary discovery)

[Unreleased]: https://github.com/eugenehp/rlsl/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/eugenehp/rlsl/releases/tag/v0.1.0
