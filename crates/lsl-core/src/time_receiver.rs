//! Time correction: estimates clock offset between inlet and outlet machines.
//!
//! Implements NTP-like round-trip probing against the outlet's UDP time service.
//! Sends `LSL:timedata` queries and computes the offset as:
//!   offset = ((t1 - t0) + (t2 - t3)) / 2
//! where t0,t3 are local times and t1,t2 are remote times.

use crate::clock::local_clock;
use crate::config::CONFIG;
use std::time::Duration;
use tokio::net::UdpSocket;

/// Estimate time correction offset between local and remote clocks.
///
/// Sends multiple NTP-like probes to the outlet's UDP service port and
/// returns the median offset from the probes with the smallest round-trip times.
pub fn time_correction(host: &str, udp_port: u16, timeout: f64) -> f64 {
    if udp_port == 0 {
        return 0.0;
    }

    // Spawn the async probing on the RUNTIME and wait via a channel
    // (cannot use block_on — the RUNTIME is already active for TCP/UDP servers)
    let host = host.to_string();
    let (tx, rx) = std::sync::mpsc::channel();
    crate::RUNTIME.spawn(async move {
        let result = time_correction_async(&host, udp_port, timeout).await;
        let _ = tx.send(result.unwrap_or(0.0));
    });

    // Wait with a timeout
    let deadline = std::time::Duration::from_secs_f64(timeout + 1.0);
    rx.recv_timeout(deadline).unwrap_or(0.0)
}

async fn time_correction_async(host: &str, port: u16, timeout: f64) -> Result<f64, String> {
    let addr = if host.contains(':') {
        format!("[{}]:{}", host, port) // IPv6
    } else {
        format!("{}:{}", host, port)
    };

    let socket = UdpSocket::bind("0.0.0.0:0")
        .await
        .map_err(|e| e.to_string())?;

    let n_probes = CONFIG.time_probe_count as usize;
    let probe_interval = Duration::from_secs_f64(CONFIG.time_probe_interval);
    let max_rtt = CONFIG.time_probe_max_rtt;
    let deadline = tokio::time::Instant::now() + Duration::from_secs_f64(timeout);

    let mut results: Vec<(f64, f64)> = Vec::new(); // (rtt, offset)

    for wave_id in 0..n_probes {
        if tokio::time::Instant::now() >= deadline {
            break;
        }

        let t0 = local_clock();
        let msg = format!("LSL:timedata\r\n{} {}\r\n", wave_id, t0);

        if socket.send_to(msg.as_bytes(), &addr).await.is_err() {
            continue;
        }

        // Wait for reply
        let mut buf = [0u8; 256];
        let recv_timeout = probe_interval.min(Duration::from_secs_f64(
            (deadline - tokio::time::Instant::now())
                .as_secs_f64()
                .max(0.01),
        ));

        if let Ok(Ok((len, _))) =
            tokio::time::timeout(recv_timeout, socket.recv_from(&mut buf)).await
        {
            let t3 = local_clock();
            let reply = std::str::from_utf8(&buf[..len]).unwrap_or("").trim();
            let parts: Vec<&str> = reply.split_whitespace().collect();

            // Reply format: " <wave_id> <t0_echo> <t1> <t2>"
            if parts.len() >= 4 {
                if let (Ok(t1), Ok(t2)) = (parts[2].parse::<f64>(), parts[3].parse::<f64>()) {
                    let rtt = (t3 - t0) - (t2 - t1);
                    let offset = ((t1 - t0) + (t2 - t3)) / 2.0;

                    if rtt >= 0.0 && rtt <= max_rtt {
                        results.push((rtt, offset));
                    }
                }
            }
        }

        if wave_id + 1 < n_probes {
            tokio::time::sleep(probe_interval).await;
        }
    }

    if results.is_empty() {
        return Ok(0.0);
    }

    // Sort by RTT and take the best probes
    results.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let n_use = results
        .len()
        .min(CONFIG.time_update_minprobes as usize)
        .max(1);
    let best = &results[..n_use];

    // Return median offset of the best probes
    let mut offsets: Vec<f64> = best.iter().map(|(_, o)| *o).collect();
    offsets.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median = offsets[offsets.len() / 2];

    Ok(median)
}
