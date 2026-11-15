//! `rlsl` — pure-Rust Lab Streaming Layer implementation.
//!
//! This crate provides the Rust-native types and networking for LSL:
//!
//! * [`StreamInfo`](stream_info::StreamInfo) — stream metadata
//! * [`StreamOutlet`](outlet::StreamOutlet) — publish data on the network
//! * [`StreamInlet`](inlet::StreamInlet) — receive data from the network
//! * [`resolver`] — discover streams via UDP multicast / broadcast
//! * [`xml_dom`] — mutable XML tree for `<desc>` metadata

pub mod clock;
#[cfg(feature = "compression")]
pub mod compress;
pub mod config;
pub mod inlet;
pub mod outlet;
pub mod postproc;
pub mod resolver;
pub mod sample;
pub mod send_buffer;
pub mod signal_quality;
pub mod stream_info;
pub mod tcp_server;
pub mod time_receiver;
pub mod types;
pub mod udp_server;
pub mod xml_dom;

use once_cell::sync::Lazy;
use tokio::runtime::Runtime;

/// Shared tokio runtime used by outlet / UDP servers.
/// Inlet data-receiver threads create their own single-threaded runtimes to
/// avoid scheduling contention with the server accept-loops.
pub static RUNTIME: Lazy<Runtime> = Lazy::new(|| {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("failed to create rlsl tokio runtime")
});

/// Convenience re-exports.
pub mod prelude {
    pub use crate::clock::local_clock;
    pub use crate::inlet::StreamInlet;
    pub use crate::outlet::StreamOutlet;
    pub use crate::stream_info::StreamInfo;
    pub use crate::types::*;
}

// ── tests ────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::prelude::*;
    use std::time::Duration;

    #[test]
    fn in_process_loopback() {
        let info = StreamInfo::new(
            "TestLoopback",
            "EEG",
            4,
            250.0,
            ChannelFormat::Float32,
            "test_src_1",
        );
        let outlet = StreamOutlet::new(&info, 0, 360);
        std::thread::sleep(Duration::from_millis(100));

        let inlet = StreamInlet::new(&info, 360, 0, true);
        inlet.open_stream(10.0).unwrap();

        let data = [1.0f32, 2.0, 3.0, 4.0];
        outlet.push_sample_f(&data, 0.0, true);

        let mut buf = [0.0f32; 4];
        let ts = inlet.pull_sample_f(&mut buf, 5.0).unwrap();
        assert!(ts > 0.0);
        assert_eq!(buf, data);
    }

    #[test]
    fn xml_dom_operations() {
        let info = StreamInfo::new("XMLTest", "EEG", 2, 250.0, ChannelFormat::Float32, "");
        let desc = info.desc();

        let channels = desc.append_child("channels");
        let ch1 = channels.append_child("channel");
        ch1.append_child_value("label", "C3");
        ch1.append_child_value("unit", "microvolts");
        let ch2 = channels.append_child("channel");
        ch2.append_child_value("label", "C4");
        ch2.append_child_value("unit", "microvolts");

        let channels_read = desc.child("channels");
        assert!(!channels_read.is_empty());
        let ch1_read = channels_read.child("channel");
        assert_eq!(ch1_read.child_value("label"), "C3");
        assert_eq!(ch1_read.child_value("unit"), "microvolts");
        let ch2_read = ch1_read.next_sibling_named("channel");
        assert_eq!(ch2_read.child_value("label"), "C4");
    }

    #[test]
    fn query_matching_xpath() {
        let info = StreamInfo::new("MyEEG", "EEG", 8, 250.0, ChannelFormat::Float32, "src42");

        // Simple equality
        assert!(info.matches_query("name='MyEEG'"));
        assert!(info.matches_query("type='EEG'"));
        assert!(!info.matches_query("name='Other'"));

        // Conjunction
        assert!(info.matches_query("name='MyEEG' and type='EEG'"));
        assert!(!info.matches_query("name='MyEEG' and type='Markers'"));

        // Disjunction (or)
        assert!(info.matches_query("name='MyEEG' or name='Other'"));
        assert!(info.matches_query("name='Nope' or type='EEG'"));
        assert!(!info.matches_query("name='A' or name='B'"));

        // Inequality
        assert!(info.matches_query("name!='Other'"));
        assert!(!info.matches_query("name!='MyEEG'"));

        // Numeric comparisons
        assert!(info.matches_query("channel_count>4"));
        assert!(!info.matches_query("channel_count>10"));
        assert!(info.matches_query("channel_count>=8"));
        assert!(info.matches_query("channel_count<100"));
        assert!(info.matches_query("channel_count<=8"));
        assert!(!info.matches_query("channel_count<8"));
        assert!(info.matches_query("nominal_srate>100"));

        // starts-with
        assert!(info.matches_query("starts-with(name,'My')"));
        assert!(!info.matches_query("starts-with(name,'Oth')"));

        // contains
        assert!(info.matches_query("contains(name,'EEG')"));
        assert!(info.matches_query("contains(type,'EE')"));
        assert!(!info.matches_query("contains(name,'XYZ')"));

        // not(...)
        assert!(info.matches_query("not(name='Other')"));
        assert!(!info.matches_query("not(name='MyEEG')"));
        assert!(info.matches_query("not(contains(name,'ZZZ'))"));

        // Combined
        assert!(info
            .matches_query("starts-with(name,'My') and channel_count>4 and not(type='Markers')"));

        // Empty query
        assert!(info.matches_query(""));
    }

    #[test]
    fn protocol_100_serialization() {
        use crate::sample::Sample;
        use std::io::Cursor;

        let fmt = ChannelFormat::Float32;
        let nch = 4u32;

        // Create and serialize
        let mut sample = Sample::new(fmt, nch, 0.0);
        sample.timestamp = 123.456;
        sample.assign_f32(&[1.0, 2.0, 3.0, 4.0]);

        let mut buf = Vec::new();
        sample.serialize_100(&mut buf);

        // Protocol 1.00, as liblsl writes it: the tag (1 + 1 bytes), the time stamp (1 + 8) and
        // four floats (1 + 4 each, the size byte first)
        assert_eq!(buf.len(), 2 + 9 + 4 * 5);
        // the test pattern liblsl 1.18 sends first on a two-channel float32 stream
        let mut pattern = Sample::new(fmt, 2, 0.0);
        pattern.assign_test_pattern(4);
        let mut bytes = Vec::new();
        pattern.serialize_100(&mut bytes);
        assert_eq!(
            bytes,
            [
                0x01, 0x02, 0x08, 0xc9, 0x76, 0xbe, 0x9f, 0x0c, 0x24, 0xfe, 0x40, 0x04, 0x00, 0x00,
                0x80, 0x40, 0x04, 0x00, 0x00, 0xa0, 0xc0
            ]
        );

        // Deserialize
        let mut cursor = Cursor::new(&buf);
        let decoded = Sample::deserialize_100(&mut cursor, fmt, nch).unwrap();
        assert!((decoded.timestamp - 123.456).abs() < 1e-10);

        let mut out = [0.0f32; 4];
        decoded.retrieve_f32(&mut out);
        assert_eq!(out, [1.0, 2.0, 3.0, 4.0]);

        // String format
        let sfmt = ChannelFormat::String;
        let mut ssample = Sample::new(sfmt, 2, 0.0);
        ssample.timestamp = 99.0;
        ssample.assign_strings(&["hello".to_string(), "world".to_string()]);

        let mut sbuf = Vec::new();
        ssample.serialize_100(&mut sbuf);

        let mut scursor = Cursor::new(&sbuf);
        let sdecoded = Sample::deserialize_100(&mut scursor, sfmt, 2).unwrap();
        assert!((sdecoded.timestamp - 99.0).abs() < 1e-10);
        let strings = sdecoded.retrieve_strings();
        assert_eq!(strings, vec!["hello", "world"]);

        // Test pattern round-trip
        let mut tp = Sample::new(fmt, nch, 0.0);
        tp.assign_test_pattern(4);
        let mut tbuf = Vec::new();
        tp.serialize_100(&mut tbuf);
        let mut tcursor = Cursor::new(&tbuf);
        let tdecoded = Sample::deserialize_100(&mut tcursor, fmt, nch).unwrap();
        assert_eq!(tp, tdecoded);
    }
}
