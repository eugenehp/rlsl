//! A group of machines that measure each other's clocks and share what they measured.
//!
//! Every node runs an [`NtpServer`] and, on a TCP port with the same number, serves the table
//! of measurements it holds. Every few seconds it measures each peer it knows over NTP and
//! fetches the peer's table. A row of the table is one directed measurement — node A's
//! statistics of node B's clock — and a node keeps, for every pair, the row with the most
//! measurements behind it. So each node ends up with the whole matrix, including rows between
//! machines it does not talk to itself.
//!
//! The tables go over TCP rather than UDP so that a forged sender address cannot turn a small
//! request into a large answer sent to somebody else.

use super::{ClockFit, ClockKind, ClockRecord, ClockTracker, OffsetSample, OffsetStats};
use super::{NtpServer, NtpServerConfig};
use crate::clock::{local_clock, system_clock, unix_offset_now};
use parking_lot::Mutex;
use std::collections::{HashMap, VecDeque};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

/// The port a mesh node uses unless told otherwise (UDP for NTP, TCP for tables).
pub const DEFAULT_MESH_PORT: u16 = 12300;

const REQUEST: &str = "RLSL:clockmesh 2";
/// The most a table may take on the wire.
const MAX_TABLE_BYTES: u64 = 1 << 20;
/// How long a peer may take to send its table.
const TABLE_TIMEOUT: Duration = Duration::from_secs(2);

/// How a [`ClockMesh`] node runs.
#[derive(Debug, Clone)]
pub struct ClockMeshConfig {
    /// The name the node goes by in the matrix. Empty: `hostname:port`.
    pub node_id: String,
    /// Where to serve NTP (UDP) and the table (TCP). Port 0 picks one free for both.
    pub bind: SocketAddr,
    /// The other nodes, as `host[:port]` (port [`DEFAULT_MESH_PORT`] if none is given).
    pub peers: Vec<String>,
    /// How often each peer is measured.
    pub interval: Duration,
    /// NTP requests per measurement; the one with the smallest delay is kept.
    pub probes: usize,
    /// The longest one measurement of one peer may take, in seconds.
    pub timeout: f64,
    /// Measurements kept per peer.
    pub window: usize,
    /// How the node's NTP server describes itself.
    pub server: NtpServerConfig,
}

impl Default for ClockMeshConfig {
    fn default() -> Self {
        Self {
            node_id: String::new(),
            bind: SocketAddr::from(([0, 0, 0, 0], DEFAULT_MESH_PORT)),
            peers: Vec::new(),
            interval: Duration::from_secs(10),
            probes: 4,
            timeout: 2.0,
            window: 360,
            server: NtpServerConfig::default(),
        }
    }
}

/// One row of the matrix: what node `from` knows about the clock of node `to`.
#[derive(Debug, Clone, PartialEq)]
pub struct MeshEntry {
    pub from: String,
    pub to: String,
    /// Which clocks were compared: the monotonic ones LSL time stamps are on, or the wall
    /// clocks.
    pub clock: ClockKind,
    /// How many measurements `from` has made of `to` in all; the row with more wins.
    pub seq: u64,
    /// The statistics of `from`'s current window: offsets are how far `to` is ahead of `from`.
    pub stats: OffsetStats,
    /// Measured by this node itself, rather than learned from another.
    pub direct: bool,
    /// When this node last measured or learned the row, on its
    /// [`system_clock`](crate::clock::system_clock).
    pub updated: f64,
}

/// How measuring one configured peer goes.
#[derive(Debug, Clone)]
pub struct PeerStatus {
    /// The address it was configured with.
    pub addr: String,
    /// The node id it reported, once it has.
    pub id: Option<String>,
    /// The statistics of this node's measurements of its wall clock.
    pub stats: Option<OffsetStats>,
    /// The statistics of this node's measurements of its monotonic clock.
    pub mono_stats: Option<OffsetStats>,
    /// The address it was last reached at.
    pub ip: Option<IpAddr>,
    /// Why the last measurement or table fetch failed, if it did.
    pub last_error: Option<String>,
}

struct Peer {
    addr: String,
    id: Option<String>,
    ip: Option<IpAddr>,
    wall: ClockTracker,
    mono: ClockTracker,
    last_error: Option<String>,
    /// The address turned out to be this node.
    is_self: bool,
}

impl Peer {
    fn name(&self) -> &str {
        self.id.as_deref().unwrap_or(&self.addr)
    }

    fn tracker(&self, clock: ClockKind) -> &ClockTracker {
        match clock {
            ClockKind::Mono => &self.mono,
            ClockKind::Wall => &self.wall,
        }
    }
}

/// Something that wants each new measurement; `false` when it no longer does.
type Sink = Box<dyn Fn(&ClockRecord) -> bool + Send + Sync>;

/// Raw measurements a node keeps, for [`ClockMesh::records`] and new subscribers.
const RECORD_HISTORY: usize = 20_000;

struct Shared {
    node_id: String,
    config: ClockMeshConfig,
    peers: Mutex<Vec<Peer>>,
    learned: Mutex<HashMap<(String, String, ClockKind), MeshEntry>>,
    records: Mutex<VecDeque<ClockRecord>>,
    sinks: Mutex<Vec<Sink>>,
    shutdown: AtomicBool,
}

impl Shared {
    fn new(node_id: String, config: ClockMeshConfig) -> Shared {
        Shared {
            node_id,
            config,
            peers: Mutex::new(Vec::new()),
            learned: Mutex::new(HashMap::new()),
            records: Mutex::new(VecDeque::new()),
            sinks: Mutex::new(Vec::new()),
            shutdown: AtomicBool::new(false),
        }
    }

    fn publish(&self, record: ClockRecord) {
        // the history and the hand-out under one lock, as `subscribe` takes them
        let mut records = self.records.lock();
        self.sinks.lock().retain(|sink| sink(&record));
        if records.len() == RECORD_HISTORY {
            records.pop_front();
        }
        records.push_back(record);
    }
}

/// A node of a clock-measuring mesh: an NTP server, a table server and a thread measuring
/// the peers. Stops when dropped.
///
/// ```no_run
/// use rlsl::ntp::{ClockMesh, ClockMeshConfig};
///
/// let mesh = ClockMesh::start(ClockMeshConfig {
///     peers: vec!["10.0.0.2".into(), "10.0.0.3".into()],
///     ..ClockMeshConfig::default()
/// })?;
/// std::thread::sleep(std::time::Duration::from_secs(60));
/// for e in mesh.matrix() {
///     let s = e.stats;
///     println!("{} → {}: {:+.6} s ± {:.6}, drift {:?} ppm", e.from, e.to, s.mean_offset,
///         s.jitter, s.drift_ppm);
/// }
/// # Ok::<(), std::io::Error>(())
/// ```
pub struct ClockMesh {
    shared: Arc<Shared>,
    addr: SocketAddr,
    _ntp: NtpServer,
    threads: Vec<JoinHandle<()>>,
}

impl ClockMesh {
    pub fn start(config: ClockMeshConfig) -> io::Result<ClockMesh> {
        let (ntp, listener) = bind(config.bind, config.server.clone())?;
        let addr = ntp.local_addr();
        let node_id = if config.node_id.is_empty() {
            let host = hostname::get()
                .ok()
                .and_then(|h| h.into_string().ok())
                .unwrap_or_else(|| "localhost".into());
            format!("{host}:{}", addr.port())
        } else {
            config.node_id.clone()
        };
        let shared = Arc::new(Shared::new(clean_id(&node_id), config));
        for p in shared.config.peers.clone() {
            add_peer(&shared, &p);
        }

        let mut threads = Vec::new();
        listener.set_nonblocking(true)?;
        let s = shared.clone();
        threads.push(
            std::thread::Builder::new()
                .name("rlsl-clockmesh-table".into())
                .spawn(move || serve_tables(listener, &s))?,
        );
        let s = shared.clone();
        threads.push(
            std::thread::Builder::new()
                .name("rlsl-clockmesh-poll".into())
                .spawn(move || {
                    while !s.shutdown.load(Ordering::Relaxed) {
                        poll_round(&s);
                        let mut waited = Duration::ZERO;
                        while waited < s.config.interval && !s.shutdown.load(Ordering::Relaxed) {
                            let step = Duration::from_millis(100).min(s.config.interval - waited);
                            std::thread::sleep(step);
                            waited += step;
                        }
                    }
                })?,
        );
        Ok(ClockMesh {
            shared,
            addr,
            _ntp: ntp,
            threads,
        })
    }

    /// This node's name in the matrix.
    pub fn node_id(&self) -> &str {
        &self.shared.node_id
    }

    /// Where the node serves NTP (UDP) and its table (TCP).
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    /// Measure another node from now on (`host[:port]`); one already known is ignored.
    pub fn add_peer(&self, addr: &str) {
        add_peer(&self.shared, addr);
    }

    /// Measure every peer and fetch its table now, in this thread, besides the regular rounds.
    pub fn poll_now(&self) {
        poll_round(&self.shared);
    }

    /// How measuring each configured peer goes.
    pub fn peers(&self) -> Vec<PeerStatus> {
        self.shared
            .peers
            .lock()
            .iter()
            .filter(|p| !p.is_self)
            .map(|p| PeerStatus {
                addr: p.addr.clone(),
                id: p.id.clone(),
                stats: p.wall.stats(),
                mono_stats: p.mono.stats(),
                ip: p.ip,
                last_error: p.last_error.clone(),
            })
            .collect()
    }

    /// This node's measurements of a peer's clock (peer by address or node id) still in the
    /// window, oldest first.
    pub fn history(&self, peer: &str, clock: ClockKind) -> Vec<OffsetSample> {
        self.shared
            .peers
            .lock()
            .iter()
            .find(|p| p.addr == peer || p.id.as_deref() == Some(peer))
            .map(|p| p.tracker(clock).samples().copied().collect())
            .unwrap_or_default()
    }

    /// A line through this node's measurements of the monotonic clock of the peer last
    /// reached at `ip`: offset (peer ahead of this node) and drift, on this node's
    /// [`system_clock`](crate::clock::system_clock). `None` when no peer there has been
    /// measured.
    pub fn mono_fit_to(&self, ip: IpAddr) -> Option<ClockFit> {
        let peers = self.shared.peers.lock();
        let peer = peers.iter().find(|p| p.ip == Some(ip) && !p.is_self)?;
        ClockFit::fit(peer.mono.samples())
    }

    /// The raw measurements this node has made (the last 20 000), oldest first.
    pub fn records(&self) -> Vec<ClockRecord> {
        self.shared.records.lock().iter().cloned().collect()
    }

    /// Call `sink` with every measurement this node has kept and, from then on, with each new
    /// one, for as long as it returns `true`. It is called on the measuring thread: it should
    /// be quick.
    pub fn subscribe(&self, sink: impl Fn(&ClockRecord) -> bool + Send + Sync + 'static) {
        let records = self.shared.records.lock();
        for r in records.iter() {
            if !sink(r) {
                return;
            }
        }
        self.shared.sinks.lock().push(Box::new(sink));
    }

    /// Every row known — this node's own measurements and those learned from others — sorted
    /// by `from`, then `to`.
    pub fn matrix(&self) -> Vec<MeshEntry> {
        let mut rows = table(&self.shared);
        rows.sort_by(|a, b| (&a.from, &a.to, a.clock).cmp(&(&b.from, &b.to, b.clock)));
        rows
    }

    /// [`matrix`](Self::matrix) as JSON, for saving or for other programs.
    pub fn matrix_json(&self) -> String {
        let now = system_clock();
        let opt = |v: Option<f64>| v.map_or("null".to_string(), |v| v.to_string());
        let rows: Vec<String> = self
            .matrix()
            .iter()
            .map(|e| {
                let s = &e.stats;
                format!(
                    r#"{{"from":{},"to":{},"clock":"{}","seq":{},"direct":{},"age":{},"count":{},"span":{},"last_offset":{},"mean_offset":{},"min_offset":{},"max_offset":{},"jitter":{},"drift_ppm":{},"mean_delay":{},"min_delay":{},"max_delay":{}}}"#,
                    json_str(&e.from),
                    json_str(&e.to),
                    e.clock.as_str(),
                    e.seq,
                    e.direct,
                    (now - e.updated).max(0.0),
                    s.count,
                    s.span,
                    s.last_offset,
                    s.mean_offset,
                    s.min_offset,
                    s.max_offset,
                    s.jitter,
                    opt(s.drift_ppm),
                    s.mean_delay,
                    s.min_delay,
                    s.max_delay
                )
            })
            .collect();
        format!(
            r#"{{"node":{},"rows":[{}]}}"#,
            json_str(&self.shared.node_id),
            rows.join(",")
        )
    }

    /// Stop the node and wait for its threads.
    pub fn stop(self) {}
}

impl Drop for ClockMesh {
    fn drop(&mut self) {
        self.shared.shutdown.store(true, Ordering::Relaxed);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

/// The NTP server and the table listener, on the same port number.
fn bind(addr: SocketAddr, server: NtpServerConfig) -> io::Result<(NtpServer, TcpListener)> {
    if addr.port() != 0 {
        let ntp = NtpServer::start_with(addr, server)?;
        let listener = TcpListener::bind(addr)?;
        return Ok((ntp, listener));
    }
    // a free UDP port may be taken on TCP: try a few
    let mut last = io::Error::new(io::ErrorKind::AddrInUse, "no free port");
    for _ in 0..20 {
        let ntp = NtpServer::start_with(addr, server.clone())?;
        match TcpListener::bind(ntp.local_addr()) {
            Ok(listener) => return Ok((ntp, listener)),
            Err(e) => last = e,
        }
    }
    Err(last)
}

fn add_peer(shared: &Shared, addr: &str) {
    let addr = addr.trim();
    if addr.is_empty() {
        return;
    }
    let mut peers = shared.peers.lock();
    if !peers.iter().any(|p| p.addr == addr) {
        peers.push(Peer {
            addr: addr.to_string(),
            id: None,
            ip: None,
            wall: ClockTracker::new(shared.config.window),
            mono: ClockTracker::new(shared.config.window),
            last_error: None,
            is_self: false,
        });
    }
}

/// Measure every peer once and merge its table. No lock is held while waiting on the network.
fn poll_round(shared: &Shared) {
    let addrs: Vec<String> = shared
        .peers
        .lock()
        .iter()
        .filter(|p| !p.is_self)
        .map(|p| p.addr.clone())
        .collect();
    for addr in addrs {
        if shared.shutdown.load(Ordering::Relaxed) {
            return;
        }
        let target = with_default_port(&addr);
        let fetched = fetch_table(&target);
        let is_self = matches!(&fetched, Ok((id, _)) if *id == shared.node_id);
        let (probes, timeout) = (shared.config.probes, shared.config.timeout);
        let resolved = super::resolve(&target);
        let (wall, mono) = match (&resolved, is_self) {
            (_, true) => (None, None),
            (Err(e), false) => (Some(Err(io::Error::new(e.kind(), e.to_string()))), None),
            (Ok(sa), false) => (
                Some(super::sync(&sa.to_string(), probes, timeout)),
                Some(super::mono_query(*sa, probes, timeout)),
            ),
        };

        let mut new_records = Vec::new();
        if let Some(p) = shared.peers.lock().iter_mut().find(|p| p.addr == addr) {
            p.is_self = is_self;
            if let Ok(sa) = &resolved {
                p.ip = Some(sa.ip());
            }
            let mut errors = Vec::new();
            match &fetched {
                Ok((id, _)) => p.id = Some(id.clone()),
                Err(e) => errors.push(format!("table: {e}")),
            }
            let to = clean_id(p.name());
            // what to add to a system_clock() time to get Unix time
            let unix = unix_offset_now() - (system_clock() - local_clock());
            let record = |clock, at: f64, offset, delay| ClockRecord {
                from: shared.node_id.clone(),
                to: to.clone(),
                clock,
                at,
                offset,
                delay,
                at_unix: at + unix,
            };
            match wall {
                Some(Ok(s)) => {
                    // the measurement time on the monotonic clock, as for every record
                    let at = s.local_time - local_clock() + system_clock();
                    let r = record(ClockKind::Wall, at, s.offset, s.delay);
                    p.wall.push(r.sample());
                    new_records.push(r);
                }
                Some(Err(e)) => errors.push(format!("ntp: {e}")),
                None => {}
            }
            match mono {
                Some(Ok((offset, delay, at))) => {
                    let r = record(ClockKind::Mono, at, offset, delay);
                    p.mono.push(r.sample());
                    new_records.push(r);
                }
                Some(Err(e)) => errors.push(format!("lsl probe: {e}")),
                None => {}
            }
            p.last_error = (!errors.is_empty()).then(|| errors.join("; "));
        }
        for r in new_records {
            shared.publish(r);
        }
        if let Ok((_, rows)) = fetched {
            merge(shared, rows);
        }
    }
}

/// Keep the rows that are news: not this node's own, and backed by more measurements than the
/// row already held.
fn merge(shared: &Shared, rows: Vec<MeshEntry>) {
    let now = system_clock();
    let mut learned = shared.learned.lock();
    for mut row in rows {
        if row.from == shared.node_id {
            continue;
        }
        let key = (row.from.clone(), row.to.clone(), row.clock);
        if learned.get(&key).is_some_and(|old| old.seq >= row.seq) {
            continue;
        }
        row.direct = false;
        row.updated = now;
        learned.insert(key, row);
    }
}

/// This node's rows and the learned ones.
fn table(shared: &Shared) -> Vec<MeshEntry> {
    let mut rows: Vec<MeshEntry> = shared
        .peers
        .lock()
        .iter()
        .filter(|p| !p.is_self)
        .flat_map(|p| {
            [ClockKind::Mono, ClockKind::Wall].map(|clock| {
                let t = p.tracker(clock);
                Some(MeshEntry {
                    from: shared.node_id.clone(),
                    to: clean_id(p.name()),
                    clock,
                    seq: t.total_samples(),
                    stats: t.stats()?,
                    direct: true,
                    updated: t.last()?.at,
                })
            })
        })
        .flatten()
        .collect();
    rows.extend(shared.learned.lock().values().cloned());
    rows
}

// ── table service ────────────────────────────────────────────────────

fn serve_tables(listener: TcpListener, shared: &Shared) {
    while !shared.shutdown.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => {
                if let Err(e) = answer_table(stream, shared) {
                    log::debug!("clock mesh table request: {e}");
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                log::debug!("clock mesh accept: {e}");
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

fn answer_table(stream: TcpStream, shared: &Shared) -> io::Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(TABLE_TIMEOUT))?;
    stream.set_write_timeout(Some(TABLE_TIMEOUT))?;
    let mut line = String::new();
    BufReader::new((&stream).take(64)).read_line(&mut line)?;
    if line.trim_end() != REQUEST {
        return Ok(());
    }
    let mut out = encode_table(&shared.node_id, &table(shared));
    out.truncate(MAX_TABLE_BYTES as usize);
    (&stream).write_all(out.as_bytes())
}

fn fetch_table(addr: &str) -> io::Result<(String, Vec<MeshEntry>)> {
    let target = super::resolve(addr)?;
    let mut stream = TcpStream::connect_timeout(&target, TABLE_TIMEOUT)?;
    stream.set_read_timeout(Some(TABLE_TIMEOUT))?;
    stream.set_write_timeout(Some(TABLE_TIMEOUT))?;
    stream.write_all(format!("{REQUEST}\n").as_bytes())?;
    let mut text = String::new();
    stream.take(MAX_TABLE_BYTES).read_to_string(&mut text)?;
    decode_table(&text).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{addr} is not a clock mesh node"),
        )
    })
}

/// `RLSL:clockmesh 1<TAB>node` and then a line per row, fields separated by tabs.
fn encode_table(node_id: &str, rows: &[MeshEntry]) -> String {
    let mut out = format!("{REQUEST}\t{node_id}\n");
    for e in rows {
        let s = &e.stats;
        let drift = s.drift_ppm.map_or("-".to_string(), |d| d.to_string());
        out.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            e.from,
            e.to,
            e.clock.as_str(),
            e.seq,
            s.count,
            s.span,
            s.last_offset,
            s.mean_offset,
            s.min_offset,
            s.max_offset,
            s.jitter,
            drift,
            s.mean_delay,
            s.min_delay,
            s.max_delay
        ));
    }
    out
}

fn decode_table(text: &str) -> Option<(String, Vec<MeshEntry>)> {
    let mut lines = text.lines();
    let node_id = lines.next()?.strip_prefix(REQUEST)?.strip_prefix('\t')?;
    let rows = lines
        .filter_map(|line| {
            let all: Vec<&str> = line.split('\t').collect();
            if all.len() != 15 || all[0].is_empty() || all[1].is_empty() {
                return None;
            }
            let clock = ClockKind::parse(all[2])?;
            // the other fields, numbered as without the clock column
            let f: Vec<&str> = [all[0], all[1]]
                .into_iter()
                .chain(all[3..].iter().copied())
                .collect();
            let num = |i: usize| f[i].parse::<f64>().ok().filter(|v| v.is_finite());
            Some(MeshEntry {
                from: f[0].to_string(),
                to: f[1].to_string(),
                clock,
                seq: f[2].parse().ok()?,
                stats: OffsetStats {
                    count: f[3].parse().ok()?,
                    span: num(4)?,
                    last_offset: num(5)?,
                    mean_offset: num(6)?,
                    min_offset: num(7)?,
                    max_offset: num(8)?,
                    jitter: num(9)?,
                    drift_ppm: if f[10] == "-" { None } else { Some(num(10)?) },
                    mean_delay: num(11)?,
                    min_delay: num(12)?,
                    max_delay: num(13)?,
                },
                direct: false,
                updated: 0.0,
            })
        })
        .collect();
    Some((node_id.to_string(), rows))
}

/// A node id that fits in a table field.
fn clean_id(id: &str) -> String {
    id.chars()
        .map(|c| if c.is_control() { '_' } else { c })
        .collect()
}

fn with_default_port(addr: &str) -> String {
    use std::net::ToSocketAddrs;
    if addr.to_socket_addrs().is_ok() {
        addr.to_string()
    } else if addr.contains(':') && !addr.starts_with('[') {
        format!("[{addr}]:{DEFAULT_MESH_PORT}") // a bare IPv6 address
    } else {
        format!("{addr}:{DEFAULT_MESH_PORT}")
    }
}

fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats(offset: f64) -> OffsetStats {
        OffsetStats {
            count: 3,
            span: 20.0,
            last_offset: offset,
            mean_offset: offset,
            min_offset: offset - 1e-4,
            max_offset: offset + 1e-4,
            jitter: 5e-5,
            drift_ppm: Some(-3.25),
            mean_delay: 2e-3,
            min_delay: 1e-3,
            max_delay: 3e-3,
        }
    }

    fn entry(from: &str, to: &str, seq: u64, offset: f64) -> MeshEntry {
        MeshEntry {
            from: from.into(),
            to: to.into(),
            clock: ClockKind::Mono,
            seq,
            stats: stats(offset),
            direct: false,
            updated: 0.0,
        }
    }

    #[test]
    fn table_round_trip() {
        let mut no_drift = entry("b", "c", 1, -0.25);
        no_drift.stats.drift_ppm = None;
        let mut wall = entry("a", "b", 7, 0.012_345_678_9);
        wall.clock = ClockKind::Wall;
        let rows = vec![wall, entry("a", "b", 7, 0.012_345_678_9), no_drift];
        let text = encode_table("a", &rows);
        let (id, back) = decode_table(&text).unwrap();
        assert_eq!(id, "a");
        assert_eq!(back, rows);

        assert!(decode_table("HTTP/1.1 200 OK\n").is_none());
        // broken rows are skipped, not fatal
        let (_, rows) = decode_table(&format!("{REQUEST}\tx\nnonsense\n")).unwrap();
        assert!(rows.is_empty());
    }

    #[test]
    fn merge_keeps_the_better_backed_row() {
        let shared = Shared::new("me".into(), ClockMeshConfig::default());
        merge(
            &shared,
            vec![entry("a", "b", 5, 0.1), entry("me", "a", 99, 0.2)],
        );
        merge(&shared, vec![entry("a", "b", 4, 0.3)]); // older: ignored
        let rows = table(&shared);
        assert_eq!(rows.len(), 1, "own rows come only from own measurements");
        assert_eq!(rows[0].stats.last_offset, 0.1);
        merge(&shared, vec![entry("a", "b", 6, 0.4)]);
        assert_eq!(table(&shared)[0].stats.last_offset, 0.4);
    }

    #[test]
    fn default_port() {
        assert_eq!(with_default_port("10.0.0.1"), "10.0.0.1:12300");
        assert_eq!(with_default_port("10.0.0.1:9"), "10.0.0.1:9");
        assert_eq!(with_default_port("::1"), "[::1]:12300");
        assert_eq!(with_default_port("[::1]:9"), "[::1]:9");
    }

    #[test]
    fn json_escapes() {
        assert_eq!(json_str("a\"b\\c\u{1}"), r#""a\"b\\c\u0001""#);
    }

    /// Three nodes in a ring, each knowing one peer: after a few rounds every node holds all
    /// three measurements.
    #[test]
    fn ring_learns_the_whole_matrix() {
        let node = |id: &str| {
            ClockMesh::start(ClockMeshConfig {
                node_id: id.into(),
                bind: "127.0.0.1:0".parse().unwrap(),
                interval: Duration::from_secs(3600),
                probes: 2,
                timeout: 1.0,
                ..ClockMeshConfig::default()
            })
            .unwrap()
        };
        let (a, b, c) = (node("a"), node("b"), node("c"));
        a.add_peer(&b.local_addr().to_string());
        b.add_peer(&c.local_addr().to_string());
        c.add_peer(&a.local_addr().to_string());
        // a node listed as its own peer is recognised and left out
        a.add_peer(&a.local_addr().to_string());

        for _ in 0..3 {
            for n in [&a, &b, &c] {
                n.poll_now();
            }
        }
        for n in [&a, &b, &c] {
            let pairs: Vec<(String, String, ClockKind)> = n
                .matrix()
                .into_iter()
                .map(|e| (e.from, e.to, e.clock))
                .collect();
            let want = [("a", "b"), ("b", "c"), ("c", "a")]
                .into_iter()
                .flat_map(|(f, t)| {
                    [ClockKind::Mono, ClockKind::Wall].map(|k| (f.to_string(), t.to_string(), k))
                })
                .collect::<Vec<_>>();
            assert_eq!(pairs, want, "node {}", n.node_id());
        }
        let m = a.matrix();
        let (ab_mono, ab) = (&m[0], &m[1]);
        assert_eq!(
            (ab_mono.clock, ab.clock),
            (ClockKind::Mono, ClockKind::Wall)
        );
        assert!(ab.direct);
        assert!(ab.seq >= 3);
        assert!(ab.stats.mean_offset.abs() < 0.005, "{ab:?}");
        assert!(ab.stats.drift_ppm.is_some());
        // one machine: one monotonic clock, within what the delays allow
        let s = ab_mono.stats;
        assert!(
            s.mean_offset.abs() <= s.max_delay / 2.0 + 1e-4,
            "{ab_mono:?}"
        );
        assert!(!b.matrix()[0].direct);

        // raw measurements, kept and handed to subscribers, the past first
        let records = a.records();
        assert_eq!(records.len() as u64, ab.seq + ab_mono.seq);
        assert!(records.iter().all(|r| r.from == "a" && r.to == "b"));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s2 = seen.clone();
        a.subscribe(move |r| {
            s2.lock().push(r.clone());
            true
        });
        a.poll_now();
        assert_eq!(seen.lock().len(), records.len() + 2);
        assert_eq!(seen.lock()[..records.len()], records[..]);

        let fit = a.mono_fit_to(b.local_addr().ip()).unwrap();
        assert!(
            fit.offset_at(system_clock()).abs() <= s.max_delay + 1e-4,
            "{fit:?}"
        );

        let peers = a.peers();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].id.as_deref(), Some("b"));
        assert!(peers[0].last_error.is_none(), "{:?}", peers[0].last_error);
        assert_eq!(a.history("b", ClockKind::Wall).len() as u64, ab.seq + 1);
        assert!(a
            .matrix_json()
            .starts_with(r#"{"node":"a","rows":[{"from":"a","to":"b","clock":"mono""#));
    }

    #[test]
    fn unreachable_peer_reports_an_error() {
        let quiet = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let mesh = ClockMesh::start(ClockMeshConfig {
            node_id: "lonely".into(),
            bind: "127.0.0.1:0".parse().unwrap(),
            interval: Duration::from_secs(3600),
            probes: 1,
            timeout: 0.2,
            peers: vec![quiet.local_addr().unwrap().to_string()],
            ..ClockMeshConfig::default()
        })
        .unwrap();
        mesh.poll_now();
        let peers = mesh.peers();
        assert!(peers[0].last_error.is_some());
        assert!(mesh.matrix().is_empty());
    }
}
