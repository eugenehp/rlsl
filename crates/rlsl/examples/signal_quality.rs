//! Example: Monitor signal quality of an LSL stream.
//!
//! Connects to any EEG stream and reports quality metrics.
//!
//! Run with: cargo run --example signal_quality

use rlsl::prelude::*;
use rlsl::{resolver, signal_quality::SignalQuality};
fn main() -> anyhow::Result<()> {
    eprintln!("🔍 Looking for EEG streams...");
    let streams = resolver::resolve_by_property("type", "EEG", 1, 10.0);
    if streams.is_empty() {
        eprintln!("No EEG streams found. Run `lsl gen` in another terminal first.");
        std::process::exit(1);
    }

    let info = &streams[0];
    let nch = info.channel_count() as usize;
    let srate = info.nominal_srate();
    eprintln!(
        "✓ Found: {} [{}], {}ch × {}Hz",
        info.name(),
        info.type_(),
        nch,
        srate
    );

    let inlet = StreamInlet::new(info, 360, 0, true);
    inlet.open_stream(5.0).map_err(|e| anyhow::anyhow!(e))?;
    eprintln!("✓ Connected. Monitoring signal quality...");
    eprintln!();

    let mut sq = SignalQuality::new(srate, nch);
    let mut buf = vec![0.0f32; nch];
    let mut report_counter = 0u64;

    loop {
        match inlet.pull_sample_f(&mut buf, 0.1) {
            Ok(ts) if ts > 0.0 => {
                let vals: Vec<f64> = buf.iter().map(|v| *v as f64).collect();
                sq.update(ts, &vals);
                report_counter += 1;

                // Report every second
                if report_counter.is_multiple_of((srate as u64).max(1)) {
                    let snap = sq.snapshot();
                    eprintln!("  ─── Signal Quality Report ───");
                    eprintln!(
                        "  Effective rate: {:.1} Hz (nominal: {:.1} Hz)",
                        snap.effective_srate, srate
                    );
                    eprintln!("  Jitter:   {:.3} ms", snap.jitter_sec * 1000.0);
                    eprintln!(
                        "  Dropouts: {} ({:.2}%)",
                        snap.total_dropouts,
                        snap.dropout_rate * 100.0
                    );
                    eprintln!("  Samples:  {}", snap.total_samples);
                    if !snap.snr_db.is_empty() {
                        let avg_snr: f64 =
                            snap.snr_db.iter().filter(|v| v.is_finite()).sum::<f64>()
                                / snap.snr_db.iter().filter(|v| v.is_finite()).count().max(1)
                                    as f64;
                        eprintln!("  Avg SNR:  {:.1} dB", avg_snr);
                    }
                    eprintln!();
                }
            }
            _ => {}
        }
    }
}
