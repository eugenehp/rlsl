//! `LSL:fullinfo`: the description reaches inlets, and the outlet answers while consumers stream.

use rlsl::inlet::StreamInlet;
use rlsl::outlet::StreamOutlet;
use rlsl::stream_info::StreamInfo;
use rlsl::types::ChannelFormat;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn unique(name: &str) -> String {
    format!(
        "{name}-{}-{}",
        std::process::id(),
        rlsl::clock::local_clock().to_bits() % 1_000_000
    )
}

fn described_info(name: &str) -> StreamInfo {
    let info = StreamInfo::new(name, "EEG", 3, 500.0, ChannelFormat::Float32, "src-1");
    let desc = info.desc();
    desc.append_child_value("manufacturer", "Neuro & <Co>");
    for label in ["Fp1", "Fp2", "Cz"] {
        let ch = desc.append_child("channel");
        ch.append_child_value("name", label);
        ch.append_child_value("unit", "microvolts");
        ch.append_child_value("type", "EEG");
    }
    desc.append_child("acquisition")
        .append_child_value("model", "EN32");
    info
}

fn resolve(name: &str) -> StreamInfo {
    let found = rlsl::resolver::resolve_by_property("name", name, 1, 5.0);
    found.into_iter().next().expect("stream not resolved")
}

/// The raw `LSL:fullinfo` reply of the outlet behind `info` (empty if none within `timeout`).
fn raw_fullinfo(info: &StreamInfo, timeout: Duration) -> String {
    let host = if info.v4address().is_empty() {
        "127.0.0.1".to_string()
    } else {
        info.v4address()
    };
    let addr = format!("{host}:{}", info.v4data_port());
    let mut c =
        std::net::TcpStream::connect_timeout(&addr.parse().unwrap(), timeout).expect("connect");
    c.set_read_timeout(Some(timeout)).unwrap();
    c.write_all(b"LSL:fullinfo\r\n").unwrap();
    let mut xml = String::new();
    let mut buf = [0u8; 4096];
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        match c.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                xml += &String::from_utf8_lossy(&buf[..n]);
                if xml.contains("</info>") {
                    break;
                }
            }
        }
    }
    xml
}

#[test]
fn inlet_fullinfo_carries_the_description() {
    let name = unique("rlsl-fullinfo-desc");
    let outlet = StreamOutlet::new(&described_info(&name), 0, 360);
    let resolved = resolve(&name);
    // like liblsl, the resolver's short info has an empty description
    assert!(resolved.desc().child("channel").is_empty());
    let inlet = StreamInlet::new(&resolved, 360, 0, true);
    let full = inlet.get_fullinfo(5.0);
    let desc = full.desc();
    assert_eq!(desc.child_value("manufacturer"), "Neuro & <Co>");
    let mut names = Vec::new();
    let mut ch = desc.child("channel");
    while !ch.is_empty() {
        names.push(ch.child_value("name"));
        assert_eq!(ch.child_value("unit"), "microvolts");
        ch = ch.next_sibling_named("channel");
    }
    assert_eq!(names, ["Fp1", "Fp2", "Cz"]);
    assert_eq!(desc.child("acquisition").child_value("model"), "EN32");
    assert_eq!(full.name(), name);
    assert_eq!(full.channel_count(), 3);
    assert_eq!(full.uid(), resolved.uid());
    drop(outlet);
}

#[test]
fn fullinfo_is_answered_while_many_consumers_stream() {
    let name = unique("rlsl-fullinfo-busy");
    let outlet = Arc::new(StreamOutlet::new(&described_info(&name), 0, 360));
    let stop = Arc::new(AtomicBool::new(false));
    let pusher = {
        let (o, s) = (outlet.clone(), stop.clone());
        std::thread::spawn(move || {
            while !s.load(Ordering::Relaxed) {
                o.push_sample_f(&[1.0, 2.0, 3.0], 0.0, true);
                std::thread::sleep(Duration::from_micros(500));
            }
        })
    };
    let resolved = resolve(&name);
    // more consumers than the shared runtime has worker threads
    let inlets: Vec<StreamInlet> = (0..8)
        .map(|_| {
            let i = StreamInlet::new(&resolved, 360, 0, true);
            i.open_stream(5.0).expect("open_stream");
            i
        })
        .collect();
    std::thread::sleep(Duration::from_millis(500));
    let t = Instant::now();
    let xml = raw_fullinfo(&resolved, Duration::from_secs(3));
    let took = t.elapsed();
    stop.store(true, Ordering::Relaxed);
    pusher.join().unwrap();
    assert!(
        xml.contains("<channel><name>Fp1</name>"),
        "no fullinfo reply within {took:?}: {} bytes",
        xml.len()
    );
    // the consumers kept receiving
    let mut buf = [0f32; 3];
    for i in &inlets {
        assert!(i.pull_sample_f(&mut buf, 2.0).is_ok());
    }
}

#[test]
fn fullinfo_is_answered_while_consumers_wait_for_data() {
    // connected inlets on an outlet that pushes nothing yet (the stream-feed tasks only wait)
    let name = unique("rlsl-fullinfo-idle");
    let outlet = StreamOutlet::new(&described_info(&name), 0, 360);
    let resolved = resolve(&name);
    let inlets: Vec<StreamInlet> = (0..2)
        .map(|_| {
            let i = StreamInlet::new(&resolved, 360, 0, true);
            i.open_stream(5.0).expect("open_stream");
            i
        })
        .collect();
    std::thread::sleep(Duration::from_millis(300));
    let t = Instant::now();
    let xml = raw_fullinfo(&resolved, Duration::from_secs(3));
    assert!(
        xml.contains("<channel><name>Fp1</name>"),
        "no fullinfo reply within {:?}: {} bytes",
        t.elapsed(),
        xml.len()
    );
    drop(inlets);
    drop(outlet);
}

#[test]
fn fullinfo_message_parses_liblsl_style_xml() {
    // pugixml (liblsl) indents with tabs and may write attributes, comments and empty elements
    let xml = "<?xml version=\"1.0\"?>\n<info>\n\t<name>X</name>\n\t<type>EEG</type>\n\t<channel_count>2</channel_count>\n\t<channel_format>float32</channel_format>\n\t<source_id>s</source_id>\n\t<nominal_srate>250</nominal_srate>\n\t<version>1.1000000000000001</version>\n\t<created_at>12.5</created_at>\n\t<uid>u-1</uid>\n\t<session_id>default</session_id>\n\t<hostname>h</hostname>\n\t<v4address />\n\t<v4data_port>16572</v4data_port>\n\t<v4service_port>16572</v4service_port>\n\t<v6address />\n\t<v6data_port>0</v6data_port>\n\t<v6service_port>0</v6service_port>\n\t<desc>\n\t\t<!-- made by hand -->\n\t\t<manufacturer>A &amp;lt; B</manufacturer>\n\t\t<channels>\n\t\t\t<channel kind=\"eeg\">\n\t\t\t\t<label>Fp1</label>\n\t\t\t\t<name>n&#233;</name>\n\t\t\t</channel>\n\t\t\t<channel><label><![CDATA[a<b]]></label><empty/></channel>\n\t\t</channels>\n\t</desc>\n</info>\n";
    let info = StreamInfo::from_fullinfo_message(xml).expect("parse");
    assert_eq!(info.name(), "X");
    assert_eq!(info.channel_count(), 2);
    assert_eq!(info.uid(), "u-1");
    let desc = info.desc();
    assert_eq!(desc.child_value("manufacturer"), "A &lt; B");
    let chans = desc.child("channels");
    let c1 = chans.child("channel");
    assert_eq!(c1.child_value("label"), "Fp1");
    assert_eq!(c1.child_value("name"), "né");
    let c2 = c1.next_sibling_named("channel");
    assert_eq!(c2.child_value("label"), "a<b");
    assert!(!c2.child("empty").is_empty());
    // the header's <name> is not confused with the description's
    assert_eq!(info.name(), "X");
}

#[test]
fn fullinfo_message_roundtrips_to_fullinfo_message() {
    let info = described_info("roundtrip");
    let back = StreamInfo::from_fullinfo_message(&info.to_fullinfo_message()).expect("parse");
    assert_eq!(back.desc().to_xml(), info.desc().to_xml());
}
