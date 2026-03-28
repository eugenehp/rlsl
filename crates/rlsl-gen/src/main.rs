//! `rlsl-gen` — Synthetic LSL signal generator.
//!
//! ```text
//! Usage:
//!   rlsl-gen [OPTIONS]
//!
//! Options:
//!   --name NAME        Stream name (default: "LslGen")
//!   --type TYPE        Content type (default: "EEG")
//!   --channels N       Number of channels (default: 8)
//!   --srate HZ         Sample rate (default: 250)
//!   --waveform WAVE    sine|square|noise|chirp|counter|sawtooth (default: sine)
//!   --freq HZ          Waveform frequency (default: 10.0)
//!   --amplitude A      Waveform amplitude (default: 100.0)
//! ```

use rlsl::prelude::*;
use std::time::Duration;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();

    let get = |key: &str, default: &str| -> String {
        args.iter()
            .position(|a| a == key)
            .and_then(|i| args.get(i + 1))
            .map(|s| s.clone())
            .unwrap_or_else(|| default.to_string())
    };

    let name = get("--name", "LslGen");
    let type_ = get("--type", "EEG");
    let nch: u32 = get("--channels", "8").parse()?;
    let srate: f64 = get("--srate", "250").parse()?;
    let waveform = get("--waveform", "sine");
    let freq: f64 = get("--freq", "10.0").parse()?;
    let amplitude: f64 = get("--amplitude", "100.0").parse()?;

    let info = StreamInfo::new(
        &name,
        &type_,
        nch,
        srate,
        ChannelFormat::Float32,
        "rlsl-gen",
    );
    let outlet = StreamOutlet::new(&info, 0, 360);

    eprintln!("🎵 rlsl-gen streaming:");
    eprintln!("   name={}, type={}, {}ch, {}Hz", name, type_, nch, srate);
    eprintln!(
        "   waveform={}, freq={}Hz, amplitude={}",
        waveform, freq, amplitude
    );
    eprintln!("   TCP port {}, Ctrl-C to stop", info.v4data_port());

    let interval = Duration::from_secs_f64(1.0 / srate);
    let mut sample_idx: u64 = 0;
    let mut sample = vec![0.0f32; nch as usize];

    loop {
        let t = sample_idx as f64 / srate;

        for ch in 0..nch as usize {
            // Phase-offset each channel slightly
            let phase = 2.0 * std::f64::consts::PI * freq * t + ch as f64 * 0.5;

            let value = match waveform.as_str() {
                "sine" => amplitude * phase.sin(),
                "square" => amplitude * phase.sin().signum(),
                "sawtooth" => amplitude * (2.0 * ((freq * t + ch as f64 * 0.1) % 1.0) - 1.0),
                "noise" => {
                    amplitude * (pseudo_random(sample_idx * nch as u64 + ch as u64) * 2.0 - 1.0)
                }
                "chirp" => {
                    // Frequency sweeps from freq to freq*4 over 10 seconds
                    let sweep = freq * (1.0 + 3.0 * (t % 10.0) / 10.0);
                    amplitude * (2.0 * std::f64::consts::PI * sweep * t).sin()
                }
                "counter" => (sample_idx * nch as u64 + ch as u64) as f64,
                _ => amplitude * phase.sin(),
            };

            sample[ch] = value as f32;
        }

        outlet.push_sample_f(&sample, 0.0, true);
        sample_idx += 1;
        std::thread::sleep(interval);
    }
}

/// Simple deterministic pseudo-random (not crypto) for noise waveform.
fn pseudo_random(seed: u64) -> f64 {
    let x = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    (x >> 33) as f64 / (1u64 << 31) as f64
}
