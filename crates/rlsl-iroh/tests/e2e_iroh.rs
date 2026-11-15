//! End-to-end integration test: source → iroh QUIC → sink → verify.
//!
//! Uses the bench infrastructure to create two in-process endpoints,
//! stream samples through the full pipeline, and assert zero loss.

use rlsl::prelude::ChannelFormat;
use rlsl_iroh::bench::{run_bench, BenchConfig};
use rlsl_iroh::compress::Compression;

async fn assert_zero_loss(config: BenchConfig) {
    let label = format!(
        "{}ch × {}Hz × {} ({})",
        config.channels,
        config.sample_rate,
        config.format.as_str(),
        config.compression.as_str(),
    );
    let results = run_bench(config)
        .await
        .unwrap_or_else(|_| panic!("bench failed: {}", label));
    assert_eq!(
        results.loss_pct, 0.0,
        "{}: expected 0% loss, got {:.4}%",
        label, results.loss_pct
    );
    assert!(results.received > 0, "{}: received 0 samples", label);
    assert_eq!(
        results.pushed, results.received,
        "{}: pushed {} != received {}",
        label, results.pushed, results.received
    );
}

#[tokio::test]
async fn zero_loss_float32_no_compression() {
    assert_zero_loss(BenchConfig {
        channels: 4,
        sample_rate: 500.0,
        duration_secs: 1.0,
        format: ChannelFormat::Float32,
        compression: Compression::None,
        ..Default::default()
    })
    .await;
}

#[tokio::test]
async fn zero_loss_float32_lz4() {
    assert_zero_loss(BenchConfig {
        channels: 4,
        sample_rate: 500.0,
        duration_secs: 1.0,
        format: ChannelFormat::Float32,
        compression: Compression::Lz4,
        ..Default::default()
    })
    .await;
}

#[tokio::test]
async fn zero_loss_float32_zstd() {
    assert_zero_loss(BenchConfig {
        channels: 4,
        sample_rate: 500.0,
        duration_secs: 1.0,
        format: ChannelFormat::Float32,
        compression: Compression::Zstd1,
        ..Default::default()
    })
    .await;
}

#[tokio::test]
async fn zero_loss_float32_delta_lz4() {
    assert_zero_loss(BenchConfig {
        channels: 4,
        sample_rate: 500.0,
        duration_secs: 1.0,
        format: ChannelFormat::Float32,
        compression: Compression::DeltaLz4,
        ..Default::default()
    })
    .await;
}

#[tokio::test]
async fn zero_loss_double64_snappy() {
    assert_zero_loss(BenchConfig {
        channels: 8,
        sample_rate: 250.0,
        duration_secs: 1.0,
        format: ChannelFormat::Double64,
        compression: Compression::Snappy,
        ..Default::default()
    })
    .await;
}

/// The stateful mode, through the real tunnel: every sample arrives, in order.
#[tokio::test]
async fn zero_loss_float32_stream_zstd() {
    assert_zero_loss(BenchConfig {
        channels: 32,
        sample_rate: 500.0,
        duration_secs: 1.0,
        format: ChannelFormat::Float32,
        compression: Compression::StreamZstd,
        ..Default::default()
    })
    .await;
}

#[tokio::test]
async fn zero_loss_int32_stream_zstd() {
    assert_zero_loss(BenchConfig {
        channels: 8,
        sample_rate: 250.0,
        duration_secs: 1.0,
        format: ChannelFormat::Int32,
        compression: Compression::StreamZstd,
        ..Default::default()
    })
    .await;
}

#[tokio::test]
async fn zero_loss_int32_zstd3() {
    assert_zero_loss(BenchConfig {
        channels: 8,
        sample_rate: 250.0,
        duration_secs: 1.0,
        format: ChannelFormat::Int32,
        compression: Compression::Zstd3,
        ..Default::default()
    })
    .await;
}
