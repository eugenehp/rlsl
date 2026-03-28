# Contributing to lsl-rs

Thank you for your interest in contributing to lsl-rs! This document provides
guidelines and information for contributors.

## Table of Contents

- [Code of Conduct](#code-of-conduct)
- [Getting Started](#getting-started)
- [Development Setup](#development-setup)
- [Code Style](#code-style)
- [Pull Request Process](#pull-request-process)
- [Testing](#testing)
- [Architecture Overview](#architecture-overview)
- [Reporting Issues](#reporting-issues)

## Code of Conduct

This project follows the [Rust Code of Conduct](https://www.rust-lang.org/policies/code-of-conduct).
Please be respectful and constructive in all interactions.

## Getting Started

1. **Fork** the repository on GitHub
2. **Clone** your fork locally:
   ```sh
   git clone https://github.com/<your-username>/lsl-rs.git
   cd lsl-rs
   ```
3. **Create a branch** for your work:
   ```sh
   git checkout -b feature/my-feature
   ```

## Development Setup

### Prerequisites

- **Rust** 1.75+ (stable) — install via [rustup](https://rustup.rs)
- **Python 3.9+** (optional, for `lsl-py` bindings)
- **wasm-pack** (optional, for `lsl-wasm` builds)

### Building

```sh
# Build entire workspace
cargo build

# Build specific crate
cargo build -p lsl-core

# Build C shared library (liblsl)
cargo build -p lsl-sys

# Build Python wheel (requires maturin)
pip install maturin
maturin develop -m crates/lsl-py/Cargo.toml

# Build WASM package
cd crates/lsl-wasm
wasm-pack build --target web --features wasm --no-default-features
```

### Running Tests

```sh
# All tests
cargo test

# Specific crate
cargo test -p lsl-core

# With output
cargo test -- --nocapture

# Benchmarks
cargo bench
```

## Code Style

- **Formatting**: Run `cargo fmt --all` before committing
- **Linting**: Run `cargo clippy --all-targets` and fix all warnings
- **Documentation**: All public items must have doc comments (`///`)
- **Error handling**: Use `anyhow::Result` in binaries, custom errors in libraries
- **Unsafe code**: Minimize; document safety invariants when required (only in `lsl-sys`)
- **Dependencies**: Prefer workspace dependencies; discuss new deps in the PR

### Naming Conventions

- Follow standard Rust naming (snake_case for functions/variables, CamelCase for types)
- Match liblsl naming where possible for API parity (e.g., `StreamInfo`, `StreamOutlet`)
- Prefix protocol-specific code with the version (e.g., `serialize_110`, `deserialize_100`)

### Commit Messages

Use [Conventional Commits](https://www.conventionalcommits.org/):

```
feat(core): add IPv6 multicast support
fix(inlet): handle connection reset during recovery
test(sample): add fuzz targets for protocol deserialization
docs: update architecture diagram
ci: add Windows ARM64 to build matrix
```

## Pull Request Process

1. **Ensure CI passes**: formatting, clippy, tests on all platforms
2. **Write tests**: new features need tests; bug fixes need regression tests
3. **Update documentation**: update README, CHANGELOG, and rustdoc as needed
4. **Keep PRs focused**: one feature or fix per PR
5. **Describe the change**: explain what, why, and any breaking changes
6. **Rebase on main**: keep a clean, linear history

### PR Checklist

- [ ] `cargo fmt --all -- --check` passes
- [ ] `cargo clippy --all-targets -- -D warnings` passes
- [ ] `cargo test` passes
- [ ] New public APIs have doc comments
- [ ] CHANGELOG.md updated (under `[Unreleased]`)
- [ ] Breaking changes documented

## Testing

### Test Categories

| Category | Location | Description |
|----------|----------|-------------|
| Unit tests | `#[cfg(test)]` in source files | Module-level tests |
| Integration | `crates/*/tests/` | Cross-module tests |
| E2E | `crates/lsl-rec/tests/` | Full recording pipeline |
| Fuzz | `crates/lsl-fuzz/` | Protocol/parser fuzzing |
| Benchmarks | `benches/` | Criterion micro-benchmarks |
| Interop | `tests/interop/` | C liblsl compatibility |

### Writing Tests

- Use descriptive test names: `test_float32_roundtrip_protocol_110`
- Test edge cases: empty data, maximum values, malformed input
- Network tests should use `localhost` with random ports
- Mark slow tests with `#[ignore]` and run with `cargo test -- --ignored`

## Architecture Overview

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for detailed architecture documentation.

The workspace is organized as:

```
lsl-core          ← Pure Rust library (protocol, networking, types)
  ↑
lsl-sys           ← C ABI shim (extern "C" functions → liblsl.dylib)
lsl-rec           ← Recording engine + TUI
lsl-rec-gui       ← eGUI recorder
lsl-py            ← Python bindings (PyO3)
lsl-wasm          ← WebSocket bridge + WASM client
lsl-cli           ← Unified CLI tool
lsl-gen           ← Signal generator
lsl-bench         ← Benchmarks
lsl-convert       ← Format converter
exg               ← XDF writer + sample traits
```

## Reporting Issues

- **Bugs**: Include OS, Rust version, reproduction steps, and expected vs. actual behavior
- **Features**: Describe the use case and proposed API
- **Security**: See [SECURITY.md](SECURITY.md) — do NOT file public issues for vulnerabilities

## License

By contributing, you agree that your contributions will be licensed under the GNU General Public License v3.0.
