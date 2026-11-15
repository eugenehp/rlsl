# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.0.9] — 2026-11-15

**Nothing here changes what an existing Rust program gets.** The clock, the sign of
`time_correction()` and what the clock-sync flag does are as in every release so far. liblsl's
behaviour is opt-in for the Rust crate — all at once with the cargo feature `liblsl-defaults`,
or one by one at run time — while the C library (`rlsl-sys`, the liblsl drop-in) now behaves
like liblsl by itself. Everything rlsl adds on top — compression, replay for late consumers,
HDF5 recordings — is optional and invisible to a liblsl peer: without it the bytes on the
network are liblsl's.

Checked against liblsl 1.18.0 (pylsl 1.18.7) in both directions — rlsl outlet to liblsl inlet
and liblsl outlet to rlsl inlet, the latter also with compression and replay asked for:
discovery, stream info and description, all seven sample formats value for value, time stamps,
and the time service.

### Changed

- **Outlets put their services on the ports of the LSL range** (`base_port` … `base_port +
  port_range`: 16572–16603), where liblsl puts its own, falling back to any free port when the
  range is taken (`allow_random_ports`). They used to take any free port. Nothing in the API
  changes; this is what lets a peer find a stream by asking a host directly (below), and a
  firewall be opened for LSL.
- **The C library behaves like liblsl** (`rlsl-sys`): its clock is the machine's monotonic
  clock, so `lsl_local_clock` and the time stamps of its samples are comparable with those of
  liblsl programs on the same machine (a liblsl inlet measures an offset of 0.00002 s to it;
  with the per-process clock it measured the outlet's uptime); `lsl_time_correction` returns
  what liblsl's does — the seconds to add to the stream's time stamps — and reports
  `lsl_timeout_error` when the outlet does not answer; and an inlet with the clock-sync flag
  synchronises by itself. A program written for liblsl needs this; `clock = process` in
  `lsl_api.cfg` / `LSL_CLOCK=process` keeps the old clock.

### Added

- **liblsl's defaults for the Rust crate, opt-in: cargo feature `liblsl-defaults`.** With it,
  `local_clock()` is the machine's monotonic clock (the one liblsl reads, the same in every
  process), `time_correction()` returns what liblsl's does (the seconds to *add* to a stream's
  time stamps; rlsl's own returns the opposite), and `PROC_CLOCKSYNC` synchronises by itself.
  Without the feature each is available on its own, as before: `LSL_CLOCK=system` /
  `clock::set_clock_source`, `clock_offset()`, `set_clock_sync(true)`.

- **Streams are found without multicast: `KnownPeers`.** Discovery by multicast and broadcast
  does not cross routers and is blocked on many Wi-Fi and institutional networks; there, streams
  could not be found at all. As in liblsl, hosts listed in `known_peers = {a, b}` in
  `lsl_api.cfg` (`KnownPeers` is read too), in `LSL_KNOWN_PEERS=a,b`, or added while running
  with `resolver::add_known_peer(host)`, are asked for their streams directly on every port of
  the LSL range. Checked between two machines with multicast off: not found without the peer,
  found and received with it.
- **A consumer that connects late can have what it missed** (opt-in on both sides, negotiated,
  invisible to liblsl): an outlet keeps the last seconds of what it sent
  (`StreamOutlet::set_history(seconds)`; irregular streams — markers — keep 10 s by default,
  regular ones nothing), and an inlet that asks (`StreamInlet::set_replay(seconds)`, `replay =`
  in `lsl_api.cfg`, `LSL_REPLAY`) is sent those samples, each with its own time stamp, ahead of
  the live ones; `replay_granted()` says how far back it starts. Without this the first second
  or so of a stream that has just appeared is lost to every recorder, because it takes that
  long to find it: of 200 markers sent from the moment a stream had its first consumer, a
  recorder joining as fast as it could missed 13–14; with replay, none.
- **HDF5 recordings** (`rlsl-rec`, cargo feature `hdf5`, off by default): `RecordingFormat::Hdf5`
  writes one `.h5` file with a group per stream — time stamps, data in the stream's own type
  (chunked, extendable, shuffle + deflate), channel labels, clock offsets and the stream's
  description as attributes — through `hdf5-no-c`, a pure-Rust HDF5 implementation: no libhdf5
  to install. Read back with h5py. (`Hdf5RecordingWriter` used to be an alias of the Parquet
  writer.)
- **The rest of the chunk API**: `StreamOutlet::push_chunk_i8` / `push_chunk_str`,
  `StreamInlet::pull_chunk_i16` / `pull_chunk_i8` / `pull_chunk_str`.
- **`ContinuousResolver` honours `forget_after`** — a stream stays in `results()` until it has
  been silent that long, instead of vanishing with one missed answer — and reports changes:
  `take_new()` and `take_lost()`.

- **`rlsl-iroh`: `stream-zstd` compression** (`Compression::StreamZstd`, wire mode 6, opt-in):
  each sample is coded against the ones before it — XOR with the previous sample, value bytes
  grouped by significance, one Zstandard stream flushed per chunk — through the new stateful
  `compress::ChunkEncoder` / `ChunkDecoder`. On a real 32-channel, 500 Hz float32 EEG recording
  sent one sample per chunk it puts 70 % of the raw bytes on the wire, where `lz4`, `zstd1`,
  `zstd3`, `snappy` and `delta-lz4` put 103 % (a single sample offers them nothing, and the
  frame costs 4 bytes). No added latency. Both ends must know the mode; the default (`none`)
  and the other modes are unchanged on the wire.

- **Compression between inlets and outlets on the network** (cargo feature `compression`, off
  by default): an inlet that asks — `StreamInlet::set_compression(true)`, or
  `compression = stream-zstd` in `lsl_api.cfg` / `LSL_COMPRESSION=stream-zstd` — gets its feed
  from an rlsl outlet built with the feature as `stream-zstd` frames (the codec now lives in
  `rlsl::compress`). 1500 samples of 32-channel float32 EEG: 205 774 bytes plain, 139 384
  compressed (68 %), every value bit for bit. It is negotiated with one extra request header:
  an outlet that does not know it (older rlsl, a build without the feature, liblsl) feeds the
  plain protocol, an inlet that does not ask gets exactly the bytes it always got, and string
  streams stay plain. `StreamInlet::is_compressed()` tells which.

- **Smaller Parquet recordings, on request**: `rlsl-convert --compact`, and for `rlsl-rec`
  `RLSL_PARQUET_COMPACT=1` (or `Recording::start_with_parquet_options` /
  `ParquetRecordingWriter::new_with_options`), write float32 columns and the timestamp
  byte-stream-split and integer columns delta-packed (standard Parquet encodings, readable by
  any reader), still ZSTD. Two minutes of a real 32-channel EEG recording: float32
  7.3 → 4.7 MB, integer counts 6.8 → 4.0 MB; double64 columns keep the default, which nothing
  beat. Not the default, because it does not always win: on a quiet signal with few distinct
  values float32 files come out larger (5.2 → 6.6 MB). Without the option the files are
  written exactly as before.
- **`StreamOutlet::dropped_samples()`**: how many samples the outlet could not deliver because
  a consumer fell further behind than its buffer holds (logged once per consumer when it
  starts).

- **`clock::UnixOffset` and `clock::unix_offset_now()`**: the local clock on the Unix timeline.
  `local_clock()` is monotonic and Unix time is a disciplined wall clock; the two drift apart
  (54 ms in an hour on one measured machine), so a recorder that turns LSL time stamps into
  wall-clock time with an offset taken once is that far out an hour later. `UnixOffset`
  re-measures the offset every 5 s and follows it at no more than 500 ppm, so mapped time
  stamps stay evenly spaced; a wall clock that is set is followed at once.

### Fixed

- **`rlsl-rec` did not record string streams — markers.** A stream of strings was opened and
  nothing of it written, in either format. They are recorded now (XDF string samples, a string
  column in Parquet, a string dataset in HDF5), each sample with its own time stamp; an XDF
  recording of 40 markers reads back with pyxdf.
- **`rlsl-rec` recorded streams without their description**: it wrote the header from the
  resolved stream, which carries none, so channel labels and everything else an outlet says
  about itself were missing from XDF headers and Parquet columns were named `ch0`, `ch1`, ….
  The outlet's full description is fetched and written.
- **A panic could cross the C API** (`rlsl-sys`): none of the 122 exported functions caught
  one, and unwinding out of an `extern "C"` function aborts the host program. Each now returns
  "nothing" (null, zero) and reports the error.
- **An outlet fed a consumer that had asked for another stream.** A request names the stream it
  wants by its UID; the outlet ignored that. An inlet that lost its stream and came back to the
  address it knew was then given the samples of whatever outlet had since taken the port — seen
  on a recorder following a marker stream: three markers of an unrelated stream recorded as its
  own. A request for another stream is now refused (`404`), as liblsl refuses it, and the inlet
  looks for its stream again.
- **A stream found by asking its service port had no time service**: that answer carried the
  stream's description from before its ports were known (service ports 0). Harmless while
  services sat on random ports nobody asked; with the LSL port range it would have broken clock
  synchronisation for streams found on the same machine or through `KnownPeers`.
- **Protocol 1.00 is liblsl's.** What rlsl called protocol 1.00 was a form of its own: a liblsl
  peer limited to 1.00 (an old one, or one set to it) got no samples from an rlsl outlet, and an
  rlsl inlet could not open its stream. Both ends now speak liblsl's "portable archive" — the
  feed's header and description, every number with its size, class information before the first
  sample — and negotiate it the way liblsl does (`Data-Protocol-Version` in the answer, or a
  request without a version). The bytes match liblsl 1.18's; checked in both directions with
  liblsl set to protocol 100.
- **Byte order and subnormals are honoured in the handshake**: an inlet reads a feed its outlet
  declares big-endian (`Byte-Order: 4321`) correctly, and an outlet flushes subnormal numbers to
  zero for a consumer that says its machine has none (`Supports-Subnormals: 0`).
- **No socket, no crash**: an outlet on a machine without IPv4, or with every allowed port
  taken, used to panic; it now has no IPv4 service and says so. Likewise an inlet that cannot
  start its receiver.

- **A reader that fell 360 samples behind lost its stream.** `max_buflen` / `max_buffered` were
  taken as a number of samples instead of seconds of data (hundreds of samples for an irregular
  stream), and an outlet whose consumer queue passed that mark ended the consumer's feed — at
  500 Hz, 0.7 s of delay anywhere between the outlet and the reader. The buffers now hold what
  the parameter says (360 s by default), and a queue that does fill up drops its oldest samples
  while the stream goes on, as in liblsl. Inlet buffers are no longer allocated in full up
  front.
- **`rlsl-rec` wrote XDF clock offsets with the wrong sign.** A ClockOffset chunk holds the
  value a reader adds to the stream's time stamps; the recorder wrote the opposite (+100 for an
  outlet whose clock is 100 s ahead, where readers such as pyxdf expect -100), and a made-up 0
  when the outlet did not answer. It now writes `clock_offset()` and nothing when there is no
  answer. Recordings made with earlier versions carry the reversed sign.
- **`rlsl-rec` did not record 8-bit integer streams** (nothing pulled them). They are recorded
  now; the core gained `push_sample_i8` / `pull_sample_i8`.
- **The outlet's own `max_buffered` was ignored.** It now caps what a consumer can ask the
  outlet to hold for it.
- **`have_consumers()` kept counting consumers that had gone** until the next sample was pushed.
- **`rlsl-rec` rounded 64-bit integers in Parquet recordings**: integer streams were buffered as
  doubles, so values beyond 2^53 were stored changed. They are buffered as integers now
  (`ToF64` gained a provided `to_i64`).

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
