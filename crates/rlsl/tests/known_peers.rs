//! Finding streams without multicast: outlets put their services on the ports of the LSL range,
//! and a resolver asks those ports of the hosts it knows (`KnownPeers`) directly.

use rlsl::prelude::*;
use rlsl::types::ChannelFormat;

/// Run by `streams_are_found_with_multicast_off` in a child process whose configuration has no
/// multicast or broadcast address at all.
#[test]
fn resolve_for_parent() {
    let Ok(name) = std::env::var("RLSL_TEST_RESOLVE") else {
        return;
    };
    assert!(rlsl::config::CONFIG.multicast_addresses.is_empty());
    let found = rlsl::resolver::resolve_by_property("name", &name, 1, 5.0);
    println!(
        "FOUND={} PEERS={:?}",
        found.len(),
        rlsl::resolver::known_peers()
    );
}

#[test]
fn outlets_use_the_lsl_port_range() {
    let info = StreamInfo::new(
        "rlsl-test-ports",
        "Test",
        1,
        0.0,
        ChannelFormat::Float32,
        "ports",
    );
    let outlet = StreamOutlet::new(&info, 0, 360);
    let info = outlet.info();
    let cfg = &rlsl::config::CONFIG;
    let range = cfg.base_port..cfg.base_port + cfg.port_range;
    // (other tests and programs may hold part of the range: then any free port is taken, and
    // that is what `allow_random_ports` is for)
    let in_range = range.contains(&info.v4service_port()) && range.contains(&info.v4data_port());
    let range_busy = (cfg.base_port..cfg.base_port + cfg.port_range)
        .filter(|p| std::net::UdpSocket::bind(("0.0.0.0", *p)).is_err())
        .count()
        > 8;
    assert!(
        in_range || range_busy,
        "service on UDP {} / TCP {}, outside {range:?}",
        info.v4service_port(),
        info.v4data_port()
    );
}

#[test]
fn streams_are_found_with_multicast_off() {
    let name = format!("rlsl-test-unicast-{}", std::process::id());
    let info = StreamInfo::new(&name, "Test", 1, 0.0, ChannelFormat::Float32, "unicast");
    let _outlet = StreamOutlet::new(&info, 0, 360);
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["resolve_for_parent", "--exact", "--nocapture"])
        .env("RLSL_TEST_RESOLVE", &name)
        // nothing to shout at: only hosts asked by name can answer
        .env("LSL_MULTICAST_ADDRESSES", "")
        .env("LSL_KNOWN_PEERS", "{localhost, 127.0.0.1}")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("FOUND=1"), "{text}");
    assert!(
        text.contains(r#"PEERS=["localhost", "127.0.0.1"]"#),
        "{text}"
    );
}

#[test]
fn peers_can_be_added_while_running() {
    let before = rlsl::resolver::known_peers().len();
    rlsl::resolver::add_known_peer(" lab-recorder.local ");
    rlsl::resolver::add_known_peer("lab-recorder.local");
    rlsl::resolver::add_known_peer("");
    let peers = rlsl::resolver::known_peers();
    assert_eq!(peers.len(), before + 1);
    assert!(peers.contains(&"lab-recorder.local".to_string()));
}

/// Whichever way a stream is found — by multicast or by asking its service port directly — it
/// is described the same, service ports included (the direct answer used to carry the
/// description from before the ports were known: a stream found that way had no time service).
#[test]
fn a_stream_found_by_asking_directly_has_its_service_ports() {
    use std::net::UdpSocket;
    use std::time::Duration;
    let info = StreamInfo::new(
        "rlsl-test-direct",
        "Test",
        1,
        0.0,
        ChannelFormat::Float32,
        "direct",
    );
    let outlet = StreamOutlet::new(&info, 0, 360);
    let (service, data) = (outlet.info().v4service_port(), outlet.info().v4data_port());
    assert!(service != 0 && data != 0);

    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let query = format!(
        "LSL:shortinfo\r\nname='rlsl-test-direct'\r\n{} 42\r\n",
        socket.local_addr().unwrap().port()
    );
    socket
        .send_to(query.as_bytes(), ("127.0.0.1", service))
        .unwrap();
    let mut buf = [0u8; 65536];
    let (n, _) = socket
        .recv_from(&mut buf)
        .expect("an answer from the service port");
    let reply = String::from_utf8_lossy(&buf[..n]).to_string();
    assert!(reply.starts_with("42\r\n"), "{reply}");
    assert!(
        reply.contains(&format!("<v4service_port>{service}</v4service_port>")),
        "{reply}"
    );
    assert!(
        reply.contains(&format!("<v4data_port>{data}</v4data_port>")),
        "{reply}"
    );
}
