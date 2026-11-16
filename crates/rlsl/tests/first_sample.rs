//! A consumer that is connected gets every sample pushed from then on: the outlet subscribes it
//! before sending the test patterns that tell it it is connected. (It used to subscribe it
//! after them, and a sample pushed in between went nowhere.)

use rlsl::prelude::*;

#[test]
fn sample_pushed_right_after_open_stream_arrives() {
    // one outlet, a new connection each round: the race is per connection (making and
    // dropping an outlet per round would test how fast sockets are released instead)
    let info = StreamInfo::new(
        "FirstSample",
        "Markers",
        1,
        0.0,
        ChannelFormat::String,
        "first-sample",
    );
    let outlet = StreamOutlet::new(&info, 0, 360);
    for k in 0..100 {
        let inlet = StreamInlet::new(outlet.info(), 360, 0, false);
        inlet.open_stream(5.0).unwrap();
        // no waiting for `have_consumers`: connected means subscribed (a time stamp of 0 would
        // mean "now")
        let stamp = 1000.0 + f64::from(k);
        outlet.push_sample_str(&[format!("first-{k}")], stamp, true);
        let (got, ts) = inlet.pull_sample_str(5.0).unwrap();
        assert_eq!(
            (got[0].as_str(), ts),
            (format!("first-{k}").as_str(), stamp),
            "round {k}"
        );
    }
}
