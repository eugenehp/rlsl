//! Example: Create multiple LSL outlets simultaneously.
//!
//! Demonstrates creating EEG, EMG, and Marker streams in one process.
//!
//! Run with: cargo run --example multi_stream

use rlsl::prelude::*;
use std::time::Duration;

fn main() -> anyhow::Result<()> {
    // ── EEG stream (32 channels, 256 Hz) ──
    let eeg_info = StreamInfo::new("EEG", "EEG", 32, 256.0, ChannelFormat::Float32, "eeg_src");
    let eeg_desc = eeg_info.desc();
    let channels = eeg_desc.append_child("channels");
    for ch in ["Fp1", "Fp2", "F3", "F4", "C3", "C4", "P3", "P4"] {
        let c = channels.append_child("channel");
        c.append_child_value("label", ch);
        c.append_child_value("unit", "microvolts");
        c.append_child_value("type", "EEG");
    }
    let eeg_outlet = StreamOutlet::new(&eeg_info, 0, 360);
    eprintln!(
        "✓ EEG outlet: 32ch × 256Hz, port {}",
        eeg_info.v4data_port()
    );

    // ── EMG stream (4 channels, 2000 Hz) ──
    let emg_info = StreamInfo::new("EMG", "EMG", 4, 2000.0, ChannelFormat::Float32, "emg_src");
    let emg_outlet = StreamOutlet::new(&emg_info, 0, 360);
    eprintln!(
        "✓ EMG outlet: 4ch × 2000Hz, port {}",
        emg_info.v4data_port()
    );

    // ── Marker stream (1 channel, irregular) ──
    let marker_info = StreamInfo::new(
        "Markers",
        "Markers",
        1,
        IRREGULAR_RATE,
        ChannelFormat::String,
        "marker_src",
    );
    let marker_outlet = StreamOutlet::new(&marker_info, 0, 0);
    eprintln!(
        "✓ Marker outlet: 1ch, irregular, port {}",
        marker_info.v4data_port()
    );
    eprintln!();
    eprintln!("Streaming... Ctrl-C to stop.");

    // ── Push data ──
    let mut eeg_data = vec![0.0f32; 32];
    let mut emg_data = vec![0.0f32; 4];
    let eeg_interval = Duration::from_secs_f64(1.0 / 256.0);
    let mut sample_idx: u64 = 0;

    loop {
        // EEG: sine waves at different frequencies per channel
        let t = sample_idx as f64 / 256.0;
        for (ch, value) in eeg_data.iter_mut().enumerate() {
            let freq = 8.0 + ch as f64 * 0.5; // 8–24 Hz
            *value = (100.0 * (2.0 * std::f64::consts::PI * freq * t).sin()) as f32;
        }
        eeg_outlet.push_sample_f(&eeg_data, 0.0, true);

        // EMG: push 2000/256 ≈ 8 samples per EEG sample
        for (ch, value) in emg_data.iter_mut().enumerate() {
            *value = (50.0 * (2.0 * std::f64::consts::PI * 150.0 * t + ch as f64).sin()) as f32;
        }
        emg_outlet.push_sample_f(&emg_data, 0.0, true);

        // Markers: send event every ~2 seconds
        if sample_idx.is_multiple_of(512) && sample_idx > 0 {
            let event = format!("event_{}", sample_idx / 512);
            marker_outlet.push_sample_str(std::slice::from_ref(&event), 0.0, true);
            eprintln!("  → marker: {}", event);
        }

        sample_idx += 1;
        std::thread::sleep(eeg_interval);
    }
}
