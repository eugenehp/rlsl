//! Feature `mdns`: an outlet is announced by DNS-SD, the announcement says what the stream is,
//! it is withdrawn with the outlet — and resolving returns what it always returned.
#![cfg(feature = "mdns")]

use rlsl::prelude::*;
use rlsl::types::ChannelFormat;
use std::time::Duration;

#[test]
fn an_outlet_is_announced_and_found() {
    let name = format!("rlsl-test-mdns-{}", std::process::id());
    let info = StreamInfo::new(
        &name,
        "EEG",
        3,
        250.0,
        ChannelFormat::Float32,
        "mdns-test-device",
    );
    let outlet = StreamOutlet::new(&info, 0, 360);
    let uid = outlet.info().uid();

    let ours = |found: &[rlsl::mdns::Announced]| found.iter().find(|a| a.uid == uid).cloned();
    let seen =
        ours(&rlsl::mdns::browse(Duration::from_secs(6))).expect("the outlet's announcement");
    assert_eq!(
        (
            seen.name.as_str(),
            seen.stream_type.as_str(),
            seen.source_id.as_str()
        ),
        (name.as_str(), "EEG", "mdns-test-device")
    );
    assert_eq!(
        seen.port,
        outlet.info().v4data_port(),
        "the service port is the data port"
    );
    assert!(
        !seen.addresses.is_empty() && seen.host.ends_with(".local."),
        "{seen:?}"
    );

    // resolving: the stream, as ever (the announcement only adds a place to ask)
    let found = rlsl::resolver::resolve_by_property("name", &name, 1, 5.0);
    assert_eq!(found.len(), 1);
    assert_eq!((found[0].uid(), found[0].channel_count()), (uid.clone(), 3));
    assert!(
        rlsl::resolver::known_peers()
            .iter()
            .any(|p| seen.addresses.iter().any(|a| &a.to_string() == p)),
        "the announcing machine is asked directly from now on"
    );

    // gone with the outlet
    drop(outlet);
    std::thread::sleep(Duration::from_millis(1500));
    assert!(
        ours(&rlsl::mdns::browse(Duration::from_secs(3))).is_none(),
        "the announcement was withdrawn"
    );
}
