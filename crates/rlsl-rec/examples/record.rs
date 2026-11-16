//! Record streams without the TUI, for scripts and remote machines:
//!
//! ```text
//! cargo run -p rlsl-rec --example record -- <output.xdf> <seconds> [query]
//! ```
//!
//! Records every stream matching `query` (all streams if none is given) for `seconds`. With
//! the `ntp` feature, `RLSL_REC_CLOCK` (`in-band`, `companion`, `mesh`) chooses how the clock
//! measurements that go with the streams are recorded; see `ClockSyncOptions::from_env`.

use rlsl::resolver;
use rlsl_rec::recording::{Recording, RecordingFormat};
use std::time::Duration;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let (Some(file), Some(seconds)) = (args.get(1), args.get(2)) else {
        anyhow::bail!("usage: record <output.xdf> <seconds> [query]");
    };
    let seconds: f64 = seconds.parse()?;
    let query = args.get(3).cloned().unwrap_or_default();
    let mut streams = if query.is_empty() {
        resolver::resolve_all(2.0)
    } else {
        resolver::resolve_by_predicate(&query, 0, 2.0)
    };
    // the companion clock streams are added by the recording itself when asked for
    if std::env::var("RLSL_REC_CLOCK").is_ok_and(|w| w.contains("companion")) {
        streams.retain(|s| s.type_() != "ClockSync");
    }
    anyhow::ensure!(!streams.is_empty(), "no streams found");
    for s in &streams {
        eprintln!(
            "recording {} [{}] from {}",
            s.name(),
            s.type_(),
            s.hostname()
        );
    }
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let rec = Recording::start_with_format(file, &streams, RecordingFormat::Xdf)?;
        tokio::time::sleep(Duration::from_secs_f64(seconds)).await;
        let n = rec
            .state
            .sample_count
            .load(std::sync::atomic::Ordering::Relaxed);
        rec.stop().await;
        eprintln!("saved {file}: {n} samples");
        anyhow::Ok(())
    })
}
