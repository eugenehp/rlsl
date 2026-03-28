//! TCP data server for a stream outlet.
//!
//! Implements the liblsl TCP protocol:
//! - LSL:shortinfo - returns shortinfo XML if query matches
//! - LSL:fullinfo  - returns full info XML
//! - LSL:streamfeed/110 - negotiates and streams samples

use crate::config::CONFIG;
use crate::sample::Sample;
use crate::send_buffer::SendBuffer;
use crate::stream_info::StreamInfo;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

pub struct TcpServer {
    info: StreamInfo,
    send_buffer: Arc<SendBuffer>,
    shutdown: Arc<AtomicBool>,
}

/// Ports returned by `TcpServer::start`.
pub struct TcpPorts {
    pub v4_port: u16,
    pub v6_port: u16,
    pub shutdown: Arc<AtomicBool>,
}

impl TcpServer {
    /// Start the TCP server on both IPv4 and IPv6.
    /// Returns ports and a shared shutdown handle.
    pub fn start(info: StreamInfo, send_buffer: Arc<SendBuffer>, chunk_size: i32) -> TcpPorts {
        let shutdown = Arc::new(AtomicBool::new(false));

        let server = Arc::new(TcpServer {
            info: info.clone(),
            send_buffer,
            shutdown: shutdown.clone(),
        });

        // --- IPv4 listener ---
        let v4_port = {
            let listener = crate::RUNTIME
                .block_on(async { TcpListener::bind("0.0.0.0:0").await })
                .expect("Failed to bind TCPv4 server");
            let port = listener.local_addr().unwrap().port();
            let srv = server.clone();
            crate::RUNTIME.spawn(async move {
                Self::accept_loop(listener, srv, chunk_size).await;
            });
            port
        };

        // --- IPv6 listener ---
        let v6_port = if crate::config::CONFIG.allow_ipv6 {
            match crate::RUNTIME.block_on(async { TcpListener::bind("[::]:0").await }) {
                Ok(listener) => {
                    let port = listener.local_addr().unwrap().port();
                    let srv = server.clone();
                    crate::RUNTIME.spawn(async move {
                        Self::accept_loop(listener, srv, chunk_size).await;
                    });
                    port
                }
                Err(_) => 0,
            }
        } else {
            0
        };

        TcpPorts {
            v4_port,
            v6_port,
            shutdown,
        }
    }

    async fn accept_loop(listener: TcpListener, server: Arc<TcpServer>, chunk_size: i32) {
        loop {
            tokio::select! {
                result = listener.accept() => {
                    match result {
                        Ok((stream, _addr)) => {
                            let srv = server.clone();
                            let cs = chunk_size;
                            tokio::spawn(async move {
                                let _ = srv.handle_connection(stream, cs).await;
                            });
                        }
                        Err(_) => {
                            if server.shutdown.load(Ordering::Relaxed) { break; }
                        }
                    }
                }
                _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {
                    if server.shutdown.load(Ordering::Relaxed) { break; }
                }
            }
        }
    }

    async fn handle_connection(&self, stream: TcpStream, chunk_size: i32) -> std::io::Result<()> {
        stream.set_nodelay(true)?;
        let mut reader = BufReader::new(stream);

        // Read the command line
        let mut command = String::new();
        reader.read_line(&mut command).await?;
        let command = command.trim().to_string();

        if command == "LSL:shortinfo" {
            self.handle_shortinfo(&mut reader).await
        } else if command == "LSL:fullinfo" {
            self.handle_fullinfo(&mut reader).await
        } else if command.starts_with("LSL:streamfeed") {
            self.handle_streamfeed(&mut reader, &command, chunk_size)
                .await
        } else {
            Ok(())
        }
    }

    async fn handle_shortinfo(&self, reader: &mut BufReader<TcpStream>) -> std::io::Result<()> {
        let mut query = String::new();
        reader.read_line(&mut query).await?;
        let query = query.trim().to_string();

        if self.info.matches_query(&query) {
            let msg = self.info.to_shortinfo_message();
            reader.get_mut().write_all(msg.as_bytes()).await?;
        }
        Ok(())
    }

    async fn handle_fullinfo(&self, reader: &mut BufReader<TcpStream>) -> std::io::Result<()> {
        let msg = self.info.to_fullinfo_message();
        reader.get_mut().write_all(msg.as_bytes()).await?;
        Ok(())
    }

    async fn handle_streamfeed(
        &self,
        reader: &mut BufReader<TcpStream>,
        command: &str,
        chunk_size: i32,
    ) -> std::io::Result<()> {
        let mut max_buffered = 360;
        let mut max_chunklen = 0;
        let mut _request_uid = String::new();
        let mut data_protocol_version = 110;

        if command.starts_with("LSL:streamfeed/") {
            // Parse version and optional UID from command line
            let parts: Vec<&str> = command.split_whitespace().collect();
            if let Some(ver_str) = parts
                .first()
                .and_then(|s| s.strip_prefix("LSL:streamfeed/"))
            {
                data_protocol_version = ver_str.parse().unwrap_or(110);
            }
            if parts.len() > 1 {
                _request_uid = parts[1].to_string();
            }

            // Read feed parameters (key: value headers until empty line)
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).await?;
                let trimmed = line.trim().to_string();
                if trimmed.is_empty() {
                    break;
                }
                let line = trimmed;
                if let Some(colon) = line.find(':') {
                    let key = line[..colon].trim().to_lowercase();
                    let val = line[colon + 1..].trim().to_string();
                    match key.as_str() {
                        "max-buffer-length" => {
                            max_buffered = val.parse().unwrap_or(360);
                        }
                        "max-chunk-length" => {
                            max_chunklen = val.parse().unwrap_or(0);
                        }
                        _ => {}
                    }
                }
            }

            // Send response
            let response = format!(
                "LSL/{} 200 OK\r\nUID: {}\r\nByte-Order: 1234\r\nSuppress-Subnormals: 0\r\nData-Protocol-Version: {}\r\n\r\n",
                CONFIG.use_protocol_version,
                self.info.uid(),
                data_protocol_version
            );
            reader.get_mut().write_all(response.as_bytes()).await?;
            reader.get_mut().flush().await?;
        } else {
            // Protocol 1.00 fallback - read two integers
            let mut params = String::new();
            reader.read_line(&mut params).await?;
            let parts: Vec<&str> = params.split_whitespace().collect();
            if parts.len() >= 2 {
                max_buffered = parts[0].parse().unwrap_or(360);
                max_chunklen = parts[1].parse().unwrap_or(0);
            }
        }

        // Send test pattern samples using the negotiated protocol version
        let fmt = self.info.channel_format();
        let nch = self.info.channel_count();
        for test_offset in [4, 2] {
            let mut test_sample = Sample::new(fmt, nch, 0.0);
            test_sample.assign_test_pattern(test_offset);
            let mut buf = Vec::new();
            if data_protocol_version >= 110 {
                test_sample.serialize_110(&mut buf);
            } else {
                test_sample.serialize_100(&mut buf);
            }
            reader.get_mut().write_all(&buf).await?;
        }
        reader.get_mut().flush().await?;

        if max_buffered <= 0 {
            return Ok(());
        }

        // Subscribe to the send buffer
        let consumer = self.send_buffer.new_consumer(max_buffered as usize);

        let effective_chunk = if max_chunklen > 0 {
            max_chunklen
        } else if chunk_size > 0 {
            chunk_size
        } else {
            i32::MAX
        };

        // Stream samples
        let mut chunk_count = 0;
        let mut chunk_buf = Vec::with_capacity(4096);

        loop {
            if self.shutdown.load(Ordering::Relaxed) {
                break;
            }

            match consumer.recv_timeout(std::time::Duration::from_millis(100)) {
                Ok(Some(sample)) => {
                    if data_protocol_version >= 110 {
                        sample.serialize_110(&mut chunk_buf);
                    } else {
                        sample.serialize_100(&mut chunk_buf);
                    }
                    chunk_count += 1;

                    if sample.pushthrough || chunk_count >= effective_chunk {
                        if reader.get_mut().write_all(&chunk_buf).await.is_err() {
                            break;
                        }
                        chunk_buf.clear();
                        chunk_count = 0;
                    }
                }
                Ok(None) => break, // sentinel
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                    // Flush any pending data
                    if !chunk_buf.is_empty() {
                        if reader.get_mut().write_all(&chunk_buf).await.is_err() {
                            break;
                        }
                        chunk_buf.clear();
                        chunk_count = 0;
                    }
                }
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
            }
        }

        Ok(())
    }
}
