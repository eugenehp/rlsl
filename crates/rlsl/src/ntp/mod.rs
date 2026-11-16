//! A small NTP server and client (SNTPv4, RFC 5905 / RFC 4330).
//!
//! LSL's own time synchronisation ([`time_receiver`](crate::time_receiver)) compares the clocks
//! of an inlet and an outlet. This module speaks standard NTP instead, for the two things LSL's
//! protocol cannot do:
//!
//! * [`NtpServer`] serves this machine's wall clock on UDP, so that devices which only know NTP
//!   (EEG amplifiers, phones, microcontrollers, a lab network without internet) can set their
//!   clocks from the machine that records.
//! * [`query`] / [`sync`] ask any NTP server — `pool.ntp.org`, a lab server, another rlsl — how
//!   far this machine's clock is from it. Besides the usual wall-clock offset, the answer gives
//!   [`NtpSample::lsl_offset`]: what to add to a [`local_clock`] time stamp to get the server's
//!   Unix time, measured on the monotonic clock, so LSL time stamps of several machines can be
//!   placed on one absolute time line.
//!
//! To follow clocks over time, [`ClockTracker`] keeps a window of measurements of one server
//! and summarises them — offset, jitter, drift in ppm — and [`ClockMesh`] does this between a
//! group of machines: each measures every peer it knows and shares its measurements, so every
//! node holds the whole matrix of who is how far from whom.
//!
//! ```no_run
//! use rlsl::ntp::{self, NtpServer};
//!
//! // serve this machine's time (port 123 needs privileges; any port works for rlsl clients)
//! let server = NtpServer::start("0.0.0.0:12300".parse().unwrap())?;
//!
//! // ask a server how far off we are
//! let sample = ntp::sync("127.0.0.1:12300", 4, 2.0)?;
//! println!("offset {:+.6} s, delay {:.6} s", sample.offset, sample.delay);
//! let utc = sample.unix_time(rlsl::clock::local_clock());
//! # drop(server);
//! # Ok::<(), std::io::Error>(())
//! ```

pub mod feed;
mod mesh;
mod record;
mod tracker;

pub use mesh::{ClockMesh, ClockMeshConfig, MeshEntry, PeerStatus, DEFAULT_MESH_PORT};
pub use record::{
    clock_source_id, clock_stream_info, ClockFit, ClockKind, ClockRecord, CLOCK_STREAM_TYPE,
};
pub use tracker::{ClockTracker, OffsetSample, OffsetStats};

/// How an outlet passes on the measurements of a [`ClockMesh`]
/// ([`StreamOutlet::attach_clock_mesh`](crate::outlet::StreamOutlet::attach_clock_mesh)).
/// Both may be on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ClockTransport {
    /// Inside the stream's own connection, to rlsl inlets that ask
    /// ([`StreamInlet::set_clock_records`](crate::inlet::StreamInlet::set_clock_records)).
    pub in_band: bool,
    /// As a companion stream of type [`CLOCK_STREAM_TYPE`] next to the data stream, which
    /// every LSL recorder, LabRecorder included, can record.
    pub companion: bool,
}

impl ClockTransport {
    pub const IN_BAND: ClockTransport = ClockTransport {
        in_band: true,
        companion: false,
    };
    pub const COMPANION: ClockTransport = ClockTransport {
        in_band: false,
        companion: true,
    };
    pub const BOTH: ClockTransport = ClockTransport {
        in_band: true,
        companion: true,
    };
}

use crate::clock::{local_clock, system_clock, unix_offset_now};
use std::io;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The standard NTP port.
pub const NTP_PORT: u16 = 123;

/// Seconds from the NTP epoch (1900-01-01) to the Unix epoch (1970-01-01).
const NTP_UNIX_DELTA: u64 = 2_208_988_800;

/// Size of an NTP packet without extension fields or MAC.
const PACKET_LEN: usize = 48;

const MODE_CLIENT: u8 = 3;
const MODE_SERVER: u8 = 4;
const MODE_BROADCAST: u8 = 5;

/// Leap indicator 3: the server's clock is not synchronised.
const LEAP_UNSYNCHRONISED: u8 = 3;

/// 2^-20 s ≈ 1 µs: the resolution the server claims for its clock.
const PRECISION: i8 = -20;

// ── timestamps ───────────────────────────────────────────────────────

/// An NTP timestamp from a Unix time in seconds.
fn ntp_from_unix(unix: f64) -> u64 {
    let secs = unix.floor();
    let frac = ((unix - secs) * 4_294_967_296.0) as u64;
    // modulo 2^32: era 1 (from 2036) wraps around, as on the wire
    let ntp_secs = (secs as i64 + NTP_UNIX_DELTA as i64) as u64 & 0xffff_ffff;
    (ntp_secs << 32) | frac.min(0xffff_ffff)
}

/// An NTP timestamp of a [`SystemTime`], without going through `f64`.
fn ntp_from_system(t: SystemTime) -> u64 {
    let d = t.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = (d.as_secs() + NTP_UNIX_DELTA) & 0xffff_ffff;
    let frac = (u64::from(d.subsec_nanos()) << 32) / 1_000_000_000;
    (secs << 32) | frac
}

/// A Unix time in seconds from an NTP timestamp. Times with the top bit clear are taken to be
/// in era 1 (2036–2172), as RFC 4330 suggests, so this is right from 1968 to 2104.
fn unix_from_ntp(ts: u64) -> f64 {
    let secs = ts >> 32;
    let secs = if secs & 0x8000_0000 == 0 {
        secs + (1 << 32)
    } else {
        secs
    };
    (secs as i64 - NTP_UNIX_DELTA as i64) as f64 + (ts & 0xffff_ffff) as f64 / 4_294_967_296.0
}

/// A duration in seconds as NTP's 16.16 short format.
fn short_from_secs(secs: f64) -> u32 {
    (secs.max(0.0) * 65536.0).min(u32::MAX as f64) as u32
}

fn secs_from_short(v: u32) -> f64 {
    v as f64 / 65536.0
}

// ── packet ───────────────────────────────────────────────────────────

/// The 48-byte NTP header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Packet {
    leap: u8,
    version: u8,
    mode: u8,
    stratum: u8,
    poll: i8,
    precision: i8,
    root_delay: u32,
    root_dispersion: u32,
    reference_id: [u8; 4],
    reference: u64,
    origin: u64,
    receive: u64,
    transmit: u64,
}

impl Packet {
    fn parse(buf: &[u8]) -> Option<Packet> {
        if buf.len() < PACKET_LEN {
            return None;
        }
        let u32_at = |i: usize| u32::from_be_bytes(buf[i..i + 4].try_into().unwrap());
        let u64_at = |i: usize| u64::from_be_bytes(buf[i..i + 8].try_into().unwrap());
        Some(Packet {
            leap: buf[0] >> 6,
            version: (buf[0] >> 3) & 0x7,
            mode: buf[0] & 0x7,
            stratum: buf[1],
            poll: buf[2] as i8,
            precision: buf[3] as i8,
            root_delay: u32_at(4),
            root_dispersion: u32_at(8),
            reference_id: buf[12..16].try_into().unwrap(),
            reference: u64_at(16),
            origin: u64_at(24),
            receive: u64_at(32),
            transmit: u64_at(40),
        })
    }

    fn encode(&self) -> [u8; PACKET_LEN] {
        let mut b = [0u8; PACKET_LEN];
        b[0] = (self.leap << 6) | ((self.version & 0x7) << 3) | (self.mode & 0x7);
        b[1] = self.stratum;
        b[2] = self.poll as u8;
        b[3] = self.precision as u8;
        b[4..8].copy_from_slice(&self.root_delay.to_be_bytes());
        b[8..12].copy_from_slice(&self.root_dispersion.to_be_bytes());
        b[12..16].copy_from_slice(&self.reference_id);
        b[16..24].copy_from_slice(&self.reference.to_be_bytes());
        b[24..32].copy_from_slice(&self.origin.to_be_bytes());
        b[32..40].copy_from_slice(&self.receive.to_be_bytes());
        b[40..48].copy_from_slice(&self.transmit.to_be_bytes());
        b
    }
}

// ── server ───────────────────────────────────────────────────────────

/// How an [`NtpServer`] describes itself to its clients.
#[derive(Debug, Clone)]
pub struct NtpServerConfig {
    /// The stratum announced (1–15). The server has no reference clock of its own, so the
    /// default is 10, as for the "local clock" of ntpd and chrony: clients use it, but prefer
    /// any server closer to a real reference.
    pub stratum: u8,
    /// The reference identifier: four ASCII characters for stratum 1 (`LOCL`, `GPS`, `PPS`…),
    /// the address of the upstream server above that. The default, `127.127.1.1`, is what
    /// chrony announces for its local clock.
    pub reference_id: [u8; 4],
    /// The root dispersion announced, in seconds: how far the served clock may be from true
    /// time.
    pub root_dispersion: f64,
}

impl Default for NtpServerConfig {
    fn default() -> Self {
        Self {
            stratum: 10,
            reference_id: [127, 127, 1, 1],
            root_dispersion: 0.010,
        }
    }
}

/// An NTP server answering with this machine's wall clock, on a thread of its own. It stops
/// when dropped.
pub struct NtpServer {
    addr: SocketAddr,
    shutdown: Arc<AtomicBool>,
    served: Arc<AtomicU64>,
    thread: Option<JoinHandle<()>>,
}

impl NtpServer {
    /// Serve on `addr` (`0.0.0.0:123` for the standard port, which needs privileges on most
    /// systems; port 0 picks a free one) with the default [`NtpServerConfig`].
    pub fn start(addr: SocketAddr) -> io::Result<NtpServer> {
        Self::start_with(addr, NtpServerConfig::default())
    }

    /// Serve on `addr` with the given configuration.
    pub fn start_with(addr: SocketAddr, config: NtpServerConfig) -> io::Result<NtpServer> {
        if !(1..=15).contains(&config.stratum) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "NTP stratum must be 1 to 15",
            ));
        }
        let socket = UdpSocket::bind(addr)?;
        // wake up now and then to see whether we have been stopped
        socket.set_read_timeout(Some(Duration::from_millis(200)))?;
        let addr = socket.local_addr()?;
        let shutdown = Arc::new(AtomicBool::new(false));
        let served = Arc::new(AtomicU64::new(0));
        let thread = {
            let (shutdown, served) = (shutdown.clone(), served.clone());
            std::thread::Builder::new()
                .name("rlsl-ntp-server".into())
                .spawn(move || serve(socket, config, &shutdown, &served))?
        };
        Ok(NtpServer {
            addr,
            shutdown,
            served,
            thread: Some(thread),
        })
    }

    /// The address the server listens on (with the port it got, if 0 was asked for).
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    /// How many requests have been answered.
    pub fn requests_served(&self) -> u64 {
        self.served.load(Ordering::Relaxed)
    }

    /// Stop serving and wait for the thread to end.
    pub fn stop(mut self) {
        self.shutdown_now();
    }

    fn shutdown_now(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for NtpServer {
    fn drop(&mut self) {
        self.shutdown_now();
    }
}

fn serve(socket: UdpSocket, config: NtpServerConfig, shutdown: &AtomicBool, served: &AtomicU64) {
    let started = ntp_from_system(SystemTime::now());
    let mut buf = [0u8; 1024];
    while !shutdown.load(Ordering::Relaxed) {
        let (len, peer) = match socket.recv_from(&mut buf) {
            Ok(r) => r,
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                continue
            }
            Err(e) => {
                // e.g. an ICMP "port unreachable" from an earlier reply, reported on Windows
                log::debug!("NTP server: {e}");
                continue;
            }
        };
        let receive_mono = system_clock();
        let receive = ntp_from_system(SystemTime::now());
        if let Some(reply) = answer_lsl_probe(&buf[..len], receive_mono) {
            let _ = socket.send_to(reply.as_bytes(), peer);
            continue;
        }
        if let Some(reply) = answer(&buf[..len], receive, started, &config) {
            if socket.send_to(&reply, peer).is_ok() {
                served.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

/// The answer to an LSL time probe (`LSL:timedata`, as an outlet's time service answers it)
/// received at `t1`, with the machine's monotonic clock: what LSL time stamps of liblsl, and of
/// rlsl with `LSL_CLOCK=system`, are taken on.
fn answer_lsl_probe(request: &[u8], t1: f64) -> Option<String> {
    let text = std::str::from_utf8(request).ok()?;
    let rest = text.strip_prefix("LSL:timedata")?;
    let mut parts = rest.split_whitespace();
    let wave_id: u64 = parts.next()?.parse().ok()?;
    let t0: f64 = parts.next()?.parse().ok()?;
    Some(format!(" {wave_id} {t0} {t1} {}", system_clock()))
}

/// Measure the monotonic clock ([`system_clock`]) of the machine at `addr` — an [`NtpServer`]
/// or an LSL outlet's time service — against this machine's, with LSL's time probe: the best
/// of `probes` round trips, within `timeout` seconds. Returns the seconds the remote clock is
/// ahead, the round-trip delay, and when (on this machine's [`system_clock`]).
pub fn mono_query(addr: SocketAddr, probes: usize, timeout: f64) -> io::Result<(f64, f64, f64)> {
    let socket = bind_for(addr)?;
    let deadline = Instant::now() + Duration::from_secs_f64(timeout.max(0.001));
    let mut best: Option<(f64, f64, f64)> = None;
    let mut buf = [0u8; 256];
    for wave in 0..probes.max(1) as u64 {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        let t0 = system_clock();
        socket.send_to(format!("LSL:timedata\r\n{wave} {t0}\r\n").as_bytes(), addr)?;
        let probe_deadline = Instant::now() + left.min(Duration::from_millis(500));
        loop {
            let left = probe_deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            socket.set_read_timeout(Some(left))?;
            let Ok((len, from)) = socket.recv_from(&mut buf) else {
                break;
            };
            let t3 = system_clock();
            if from.ip() != addr.ip() {
                continue;
            }
            let reply = std::str::from_utf8(&buf[..len]).unwrap_or("");
            let f: Vec<f64> = reply
                .split_whitespace()
                .filter_map(|v| v.parse().ok())
                .collect();
            // " <wave> <t0> <t1> <t2>", for this probe
            if f.len() != 4 || f[0] != wave as f64 || f[1] != t0 {
                continue;
            }
            let (t1, t2) = (f[2], f[3]);
            let delay = (t3 - t0) - (t2 - t1);
            let offset = ((t1 - t0) + (t2 - t3)) / 2.0;
            if delay >= 0.0 && best.is_none_or(|b| delay < b.1) {
                best = Some((offset, delay, (t0 + t3) / 2.0));
            }
            break;
        }
    }
    best.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::TimedOut,
            format!("no answer to LSL time probes from {addr}"),
        )
    })
}

/// The reply to a request received at `receive`, or `None` when it is not a client request.
/// The transmit time stamp is taken last, just before the reply is returned.
fn answer(
    request: &[u8],
    receive: u64,
    reference: u64,
    config: &NtpServerConfig,
) -> Option<[u8; PACKET_LEN]> {
    let req = Packet::parse(request)?;
    if req.mode != MODE_CLIENT || !(1..=4).contains(&req.version) {
        return None;
    }
    let mut reply = Packet {
        leap: 0,
        version: req.version,
        mode: MODE_SERVER,
        stratum: config.stratum,
        poll: req.poll.clamp(4, 17),
        precision: PRECISION,
        root_delay: 0,
        root_dispersion: short_from_secs(config.root_dispersion),
        reference_id: config.reference_id,
        reference,
        origin: req.transmit,
        receive,
        transmit: 0,
    };
    reply.transmit = ntp_from_system(SystemTime::now());
    Some(reply.encode())
}

// ── client ───────────────────────────────────────────────────────────

/// One exchange with an NTP server.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NtpSample {
    /// Seconds to add to this machine's wall clock to get the server's: positive when the
    /// server is ahead.
    pub offset: f64,
    /// Seconds to add to a [`local_clock`] time to get the server's Unix time. Unlike
    /// `offset`, measured against the monotonic clock LSL time stamps are on.
    pub lsl_offset: f64,
    /// The round-trip delay, without the time the server took to answer, in seconds.
    pub delay: f64,
    /// The server's stratum (1 = it has a reference clock).
    pub stratum: u8,
    /// The server's reference identifier (ASCII for stratum 1, an IPv4 address above).
    pub reference_id: [u8; 4],
    /// The server's estimate of its distance from true time: root delay / 2 + root
    /// dispersion, in seconds.
    pub root_distance: f64,
    /// The server's leap indicator: 0 none, 1 a leap second inserted at the end of the day,
    /// 2 one deleted.
    pub leap: u8,
    /// The server address that answered.
    pub server: SocketAddr,
    /// When the answer arrived, on this machine's [`local_clock`].
    pub local_time: f64,
}

impl NtpSample {
    /// A [`local_clock`] time (e.g. an LSL time stamp taken on this machine) as the server's
    /// Unix time.
    pub fn unix_time(&self, local_time: f64) -> f64 {
        local_time + self.lsl_offset
    }

    /// The reference identifier as text, as `ntpq` shows it.
    pub fn reference_id_string(&self) -> String {
        if self.stratum <= 1 {
            self.reference_id
                .iter()
                .take_while(|&&b| b != 0)
                .map(|&b| b as char)
                .collect()
        } else {
            std::net::Ipv4Addr::from(self.reference_id).to_string()
        }
    }
}

/// Resolve `server`, which may omit the port (`pool.ntp.org`, `10.0.0.5:123`, `[::1]:123`).
pub(crate) fn resolve(server: &str) -> io::Result<SocketAddr> {
    let with_port = |s: &str| (s, NTP_PORT).to_socket_addrs();
    let mut addrs = match server.to_socket_addrs() {
        Ok(a) => a.collect::<Vec<_>>(),
        // no port given; a bare IPv6 address may still come with brackets
        Err(_) => with_port(server.trim_start_matches('[').trim_end_matches(']'))?.collect(),
    };
    // prefer IPv4, which every NTP server speaks
    addrs.sort_by_key(|a| !a.is_ipv4());
    addrs.into_iter().next().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("{server}: no address found"),
        )
    })
}

/// Ask `server` (host or address, port 123 if none is given) for the time once, waiting at
/// most `timeout` seconds.
pub fn query(server: &str, timeout: f64) -> io::Result<NtpSample> {
    let addr = resolve(server)?;
    let socket = bind_for(addr)?;
    exchange(&socket, addr, Duration::from_secs_f64(timeout.max(0.001)))
}

/// Ask `server` `probes` times, a little apart, and return the answer with the smallest
/// delay — the one least disturbed by the network — or the last error if none came back.
/// `timeout` (seconds) bounds the whole.
pub fn sync(server: &str, probes: usize, timeout: f64) -> io::Result<NtpSample> {
    let addr = resolve(server)?;
    let socket = bind_for(addr)?;
    let deadline = Instant::now() + Duration::from_secs_f64(timeout.max(0.001));
    let mut best: Option<NtpSample> = None;
    let mut last_err = io::Error::new(io::ErrorKind::TimedOut, "no NTP probe was sent");
    for i in 0..probes.max(1) {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        match exchange(&socket, addr, left) {
            Ok(s) => {
                if best.is_none_or(|b| s.delay < b.delay) {
                    best = Some(s);
                }
            }
            // a server that says go away means it
            Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => return Err(e),
            Err(e) => last_err = e,
        }
        if i + 1 < probes {
            // public servers ask for at least 2 s between requests; a probe series is brief
            std::thread::sleep(Duration::from_millis(50).min(left));
        }
    }
    best.ok_or(last_err)
}

fn bind_for(addr: SocketAddr) -> io::Result<UdpSocket> {
    let local: SocketAddr = if addr.is_ipv4() {
        "0.0.0.0:0".parse().unwrap()
    } else {
        "[::]:0".parse().unwrap()
    };
    UdpSocket::bind(local)
}

fn exchange(socket: &UdpSocket, addr: SocketAddr, timeout: Duration) -> io::Result<NtpSample> {
    // local time stamps are taken on the monotonic clock and only turned into wall time for
    // the packet, so that a wall clock being slewed or set meanwhile does not show up
    let to_unix = unix_offset_now();
    let t1 = local_clock();
    let transmit = ntp_from_unix(t1 + to_unix);
    let request = Packet {
        version: 4,
        mode: MODE_CLIENT,
        transmit,
        ..Packet::default()
    };
    socket.send_to(&request.encode(), addr)?;

    let deadline = Instant::now() + timeout;
    let mut buf = [0u8; 1024];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("no answer from NTP server {addr}"),
            ));
        }
        socket.set_read_timeout(Some(left))?;
        let (len, from) = match socket.recv_from(&mut buf) {
            Ok(r) => r,
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                continue
            }
            Err(e) => return Err(e),
        };
        let t4 = local_clock();
        if from.ip() != addr.ip() {
            continue;
        }
        let Some(reply) = Packet::parse(&buf[..len]) else {
            continue;
        };
        // an answer to this request, not a stale or forged one
        if reply.origin != transmit || !matches!(reply.mode, MODE_SERVER | MODE_BROADCAST) {
            continue;
        }
        return sample_from(&reply, t1, t4, to_unix, from);
    }
}

/// The sample from a reply to a request sent at `t1` and received at `t4` ([`local_clock`]),
/// with `to_unix` the local clock's offset to the wall clock.
fn sample_from(
    reply: &Packet,
    t1: f64,
    t4: f64,
    to_unix: f64,
    server: SocketAddr,
) -> io::Result<NtpSample> {
    if reply.stratum == 0 {
        let code: String = reply.reference_id.iter().map(|&b| b as char).collect();
        return Err(io::Error::new(
            io::ErrorKind::ConnectionRefused,
            format!("NTP server {server} sent kiss-o'-death {code:?}"),
        ));
    }
    if reply.leap == LEAP_UNSYNCHRONISED || reply.stratum > 15 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("NTP server {server} is not synchronised"),
        ));
    }
    if reply.transmit == 0 || reply.receive == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("NTP server {server} sent an empty time stamp"),
        ));
    }
    let t2 = unix_from_ntp(reply.receive);
    let t3 = unix_from_ntp(reply.transmit);
    let delay = ((t4 - t1) - (t3 - t2)).max(0.0);
    let lsl_offset = ((t2 - t1) + (t3 - t4)) / 2.0;
    Ok(NtpSample {
        offset: lsl_offset - to_unix,
        lsl_offset,
        delay,
        stratum: reply.stratum,
        reference_id: reply.reference_id,
        root_distance: secs_from_short(reply.root_delay) / 2.0
            + secs_from_short(reply.root_dispersion),
        leap: reply.leap,
        server,
        local_time: t4,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_round_trip() {
        for unix in [
            0.0,
            1.0e9 + 0.25,
            1_763_200_000.123_456,
            2_085_978_496.5,
            3.0e9,
        ] {
            let back = unix_from_ntp(ntp_from_unix(unix));
            assert!((back - unix).abs() < 1e-6, "{unix} came back as {back}");
        }
        // the Unix epoch is 2 208 988 800 s after NTP's
        assert_eq!(ntp_from_unix(0.0), NTP_UNIX_DELTA << 32);
        // 2036-02-07 06:28:16 UTC: NTP era 1 starts, the seconds wrap to 0
        assert_eq!(ntp_from_unix(2_085_978_496.0), 0);
    }

    #[test]
    fn system_time_matches_float_path() {
        let now = SystemTime::now();
        let unix = now.duration_since(UNIX_EPOCH).unwrap().as_secs_f64();
        let a = unix_from_ntp(ntp_from_system(now));
        assert!((a - unix).abs() < 1e-6);
    }

    #[test]
    fn packet_round_trip() {
        let p = Packet {
            leap: 1,
            version: 4,
            mode: MODE_SERVER,
            stratum: 2,
            poll: 6,
            precision: -20,
            root_delay: 0x0001_8000,
            root_dispersion: 42,
            reference_id: [10, 0, 0, 1],
            reference: 1,
            origin: 2,
            receive: 3,
            transmit: 4,
        };
        let bytes = p.encode();
        assert_eq!(bytes[0], 0b01_100_100);
        assert_eq!(Packet::parse(&bytes), Some(p));
        assert_eq!(Packet::parse(&bytes[..47]), None);
    }

    #[test]
    fn server_answers_only_clients() {
        let config = NtpServerConfig::default();
        let mut req = Packet {
            version: 3,
            mode: MODE_CLIENT,
            transmit: 0x1234_5678_9abc_def0,
            ..Packet::default()
        };
        let reply = Packet::parse(&answer(&req.encode(), 7, 5, &config).unwrap()).unwrap();
        assert_eq!(reply.mode, MODE_SERVER);
        assert_eq!(reply.version, 3);
        assert_eq!(reply.origin, req.transmit);
        assert_eq!(reply.receive, 7);
        assert_eq!(reply.reference, 5);
        assert_eq!(reply.stratum, 10);
        assert_eq!(reply.reference_id, [127, 127, 1, 1]);
        assert!(reply.transmit >= 7);

        for mode in [0, 1, 2, MODE_SERVER, MODE_BROADCAST, 6, 7] {
            req.mode = mode;
            assert!(
                answer(&req.encode(), 7, 5, &config).is_none(),
                "mode {mode}"
            );
        }
        req.mode = MODE_CLIENT;
        req.version = 0;
        assert!(answer(&req.encode(), 7, 5, &config).is_none());
        assert!(answer(&[0x23; 20], 7, 5, &config).is_none());
    }

    #[test]
    fn offset_and_delay() {
        let server: SocketAddr = "127.0.0.1:123".parse().unwrap();
        // local clock 100 s, wall clock = local + 1000; server 0.5 s ahead of our wall clock;
        // 10 ms each way, 1 ms in the server
        let to_unix = 1000.0;
        let (t1, t4) = (100.0, 100.021);
        let reply = Packet {
            mode: MODE_SERVER,
            stratum: 1,
            reference_id: *b"GPS\0",
            receive: ntp_from_unix(t1 + to_unix + 0.5 + 0.010),
            transmit: ntp_from_unix(t1 + to_unix + 0.5 + 0.011),
            ..Packet::default()
        };
        let s = sample_from(&reply, t1, t4, to_unix, server).unwrap();
        assert!((s.offset - 0.5).abs() < 1e-6, "{s:?}");
        assert!((s.delay - 0.020).abs() < 1e-6, "{s:?}");
        assert!((s.unix_time(t1) - (t1 + to_unix + 0.5)).abs() < 1e-6);
        assert_eq!(s.reference_id_string(), "GPS");

        let kiss = Packet {
            stratum: 0,
            reference_id: *b"RATE",
            ..reply
        };
        let err = sample_from(&kiss, t1, t4, to_unix, server).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::ConnectionRefused);
        assert!(err.to_string().contains("RATE"));

        let unsynced = Packet {
            leap: LEAP_UNSYNCHRONISED,
            ..reply
        };
        assert!(sample_from(&unsynced, t1, t4, to_unix, server).is_err());
    }

    #[test]
    fn client_and_server_agree() {
        let server = NtpServer::start("127.0.0.1:0".parse().unwrap()).unwrap();
        let addr = server.local_addr().to_string();
        let s = sync(&addr, 4, 5.0).unwrap();
        // the same machine's clock on both ends
        assert!(s.offset.abs() < 0.005, "{s:?}");
        assert!(s.delay < 0.05, "{s:?}");
        assert_eq!(s.stratum, 10);
        assert_eq!(s.reference_id_string(), "127.127.1.1");
        let utc = s.unix_time(local_clock());
        let wall = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        assert!((utc - wall).abs() < 0.01);
        assert!(server.requests_served() >= 1);
        server.stop();
    }

    #[test]
    fn silent_server_times_out() {
        // a bound socket that never answers
        let quiet = UdpSocket::bind("127.0.0.1:0").unwrap();
        let addr = quiet.local_addr().unwrap().to_string();
        let started = Instant::now();
        let err = query(&addr, 0.2).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn rejects_bad_stratum() {
        let config = NtpServerConfig {
            stratum: 16,
            ..NtpServerConfig::default()
        };
        assert!(NtpServer::start_with("127.0.0.1:0".parse().unwrap(), config).is_err());
    }
}
