//! `lsl-bridge` — native WebSocket server that bridges LSL to browsers.
//!
//! Usage: `cargo run -p rlsl-wasm --bin lsl-bridge -- [--port 8765]`
//!
//! 1. Discovers LSL streams on the local network.
//! 2. Accepts WebSocket connections from browsers.
//! 3. Clients send `{"type":"list"}` to get streams,
//!    `{"type":"subscribe","stream_id":"<uid>"}` to start receiving data.
//! 4. Pushes data chunks as JSON to subscribed clients.

#[cfg(feature = "bridge")]
mod inner {
    use futures::sink::SinkExt;
    use futures::stream::StreamExt;
    use rlsl::inlet::StreamInlet;
    use rlsl::resolver;
    use rlsl::stream_info::StreamInfo;
    use rlsl_wasm::protocol::*;
    use std::collections::{HashMap, HashSet};
    use std::net::SocketAddr;
    use std::sync::Arc;
    use tokio::net::{TcpListener, TcpStream};
    use tokio::sync::{broadcast, RwLock};
    use tokio_tungstenite::tungstenite::Message;

    /// Per-stream broadcast channel carrying JSON-encoded data chunks.
    struct StreamFeeder {
        info: StreamInfo,
        tx: broadcast::Sender<String>,
        _shutdown: Arc<tokio::sync::Notify>,
    }

    type FeedMap = Arc<RwLock<HashMap<String, Arc<StreamFeeder>>>>;

    pub async fn run(port: u16) -> anyhow::Result<()> {
        let addr: SocketAddr = ([0, 0, 0, 0], port).into();
        let listener = TcpListener::bind(&addr).await?;
        eprintln!("🌐 lsl-bridge listening on ws://0.0.0.0:{}", port);

        let feeds: FeedMap = Arc::new(RwLock::new(HashMap::new()));

        // Background task: periodically resolve streams and start feeders
        let feeds_bg = feeds.clone();
        tokio::spawn(async move {
            loop {
                refresh_streams(&feeds_bg).await;
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        });

        // Accept WebSocket connections
        while let Ok((stream, peer)) = listener.accept().await {
            let feeds = feeds.clone();
            tokio::spawn(async move {
                if let Err(e) = handle_connection(stream, peer, feeds).await {
                    eprintln!("Connection {} error: {}", peer, e);
                }
            });
        }

        Ok(())
    }

    async fn refresh_streams(feeds: &FeedMap) {
        let resolved = tokio::task::spawn_blocking(|| resolver::resolve_all(2.0))
            .await
            .unwrap_or_default();

        let mut map = feeds.write().await;
        for info in &resolved {
            let uid = info.uid();
            if !map.contains_key(&uid) {
                eprintln!("  + discovered: {} ({})", info.name(), uid);
                let (tx, _) = broadcast::channel(256);
                let shutdown = Arc::new(tokio::sync::Notify::new());
                let feeder = Arc::new(StreamFeeder {
                    info: info.clone(),
                    tx: tx.clone(),
                    _shutdown: shutdown.clone(),
                });
                map.insert(uid.clone(), feeder.clone());

                // Spawn inlet reader for this stream
                let info_clone = info.clone();
                let uid_clone = uid.clone();
                tokio::task::spawn_blocking(move || {
                    if let Err(e) = run_inlet(&info_clone, &uid_clone, &tx, &shutdown) {
                        eprintln!("  Inlet {} error: {}", uid_clone, e);
                    }
                });
            }
        }
    }

    fn run_inlet(
        info: &StreamInfo,
        uid: &str,
        tx: &broadcast::Sender<String>,
        _shutdown: &Arc<tokio::sync::Notify>,
    ) -> anyhow::Result<()> {
        let inlet = StreamInlet::new(info, 360, 0, true);
        inlet.open_stream(10.0).map_err(|e| anyhow::anyhow!(e))?;
        eprintln!("  ▶ inlet open: {} ({})", info.name(), uid);

        let nch = info.channel_count() as usize;
        let mut buf = vec![0.0f64; nch];

        loop {
            // Check if we should stop (non-blocking)
            if tx.receiver_count() == 0 {
                // No subscribers — still pull to keep connection alive, but slower
                std::thread::sleep(std::time::Duration::from_millis(500));
                let _ = inlet.pull_sample_d(&mut buf, 0.0);
                continue;
            }

            // Pull available samples
            let mut timestamps = Vec::new();
            let mut rows = Vec::new();
            loop {
                match inlet.pull_sample_d(&mut buf, 0.0) {
                    Ok(ts) if ts > 0.0 => {
                        timestamps.push(ts);
                        rows.push(buf.clone());
                    }
                    _ => break,
                }
                if rows.len() >= 256 {
                    break;
                }
            }

            if !rows.is_empty() {
                let msg = ServerMsg::Data {
                    stream_id: uid.to_string(),
                    timestamps,
                    data: rows,
                };
                if let Ok(json) = serde_json::to_string(&msg) {
                    let _ = tx.send(json);
                }
            }

            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    async fn handle_connection(
        stream: TcpStream,
        peer: SocketAddr,
        feeds: FeedMap,
    ) -> anyhow::Result<()> {
        let ws = tokio_tungstenite::accept_async(stream).await?;
        eprintln!("  🔗 client connected: {}", peer);

        let (mut ws_tx, mut ws_rx) = ws.split();
        let mut subscriptions: HashSet<String> = HashSet::new();
        let mut receivers: HashMap<String, broadcast::Receiver<String>> = HashMap::new();

        loop {
            // Multiplex: client messages + data from subscribed feeds
            tokio::select! {
                // Handle incoming client commands
                msg = ws_rx.next() => {
                    match msg {
                        Some(Ok(Message::Text(text))) => {
                            match serde_json::from_str::<ClientMsg>(&text) {
                                Ok(ClientMsg::List) => {
                                    let map = feeds.read().await;
                                    let streams: Vec<StreamDesc> = map.values().map(|f| {
                                        let i = &f.info;
                                        StreamDesc {
                                            uid: i.uid(),
                                            name: i.name(),
                                            type_: i.type_(),
                                            channel_count: i.channel_count(),
                                            nominal_srate: i.nominal_srate(),
                                            channel_format: i.channel_format().as_str().to_string(),
                                            hostname: i.hostname(),
                                            source_id: i.source_id(),
                                        }
                                    }).collect();
                                    let resp = serde_json::to_string(&ServerMsg::Streams { streams })?;
                                    ws_tx.send(Message::Text(resp.into())).await?;
                                }
                                Ok(ClientMsg::Subscribe { stream_id }) => {
                                    let map = feeds.read().await;
                                    if let Some(feeder) = map.get(&stream_id) {
                                        let rx = feeder.tx.subscribe();
                                        receivers.insert(stream_id.clone(), rx);
                                        subscriptions.insert(stream_id.clone());
                                        eprintln!("  📡 {} subscribed to {}", peer, stream_id);
                                    } else {
                                        let err = serde_json::to_string(&ServerMsg::Error {
                                            message: format!("Unknown stream: {}", stream_id),
                                        })?;
                                        ws_tx.send(Message::Text(err.into())).await?;
                                    }
                                }
                                Ok(ClientMsg::Unsubscribe { stream_id }) => {
                                    subscriptions.remove(&stream_id);
                                    receivers.remove(&stream_id);
                                    eprintln!("  🔇 {} unsubscribed from {}", peer, stream_id);
                                }
                                Err(e) => {
                                    let err = serde_json::to_string(&ServerMsg::Error {
                                        message: format!("Parse error: {}", e),
                                    })?;
                                    ws_tx.send(Message::Text(err.into())).await?;
                                }
                            }
                        }
                        Some(Ok(Message::Close(_))) | None => {
                            eprintln!("  ❌ client disconnected: {}", peer);
                            break;
                        }
                        _ => {}
                    }
                }
                // Forward data from subscribed streams (poll every 20ms)
                _ = tokio::time::sleep(std::time::Duration::from_millis(20)) => {
                    for rx in receivers.values_mut() {
                        while let Ok(json) = rx.try_recv() {
                            if ws_tx.send(Message::Text(json.into())).await.is_err() {
                                break;
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }
}

#[cfg(feature = "bridge")]
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let port: u16 = std::env::args()
        .skip_while(|a| a != "--port")
        .nth(1)
        .and_then(|p| p.parse().ok())
        .unwrap_or(8765);
    inner::run(port).await
}

#[cfg(not(feature = "bridge"))]
fn main() {
    eprintln!("lsl-bridge requires the 'bridge' feature. Build with:");
    eprintln!("  cargo run -p rlsl-wasm --features bridge --bin lsl-bridge");
}
