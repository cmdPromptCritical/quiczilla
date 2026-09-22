use anyhow::Result;
use quiczilla_core::cert::build_quic_config;
use quiczilla_core::msquic::engine::{MsQuicConnection, MsQuicEngine, MsQuicListener};
use quiczilla_core::pipe::run_pipe;
use quiczilla_core::types::PIPE_STREAM_HEADER;
use std::net::SocketAddr;
use std::time::Instant;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt, ReadBuf};
use tokio_util::sync::CancellationToken;

// High performance memory pattern reader for benchmarking
struct BenchReader {
    remaining: u64,
    chunk: Vec<u8>,
}

impl BenchReader {
    fn new(total_bytes: u64) -> Self {
        let chunk = vec![0x5A; 1024 * 1024]; // 1 MiB chunk
        Self {
            remaining: total_bytes,
            chunk,
        }
    }
}

impl AsyncRead for BenchReader {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        if self.remaining == 0 {
            return std::task::Poll::Ready(Ok(()));
        }

        let to_write = (self.remaining as usize)
            .min(buf.remaining())
            .min(self.chunk.len());
        buf.put_slice(&self.chunk[..to_write]);
        self.remaining -= to_write as u64;
        std::task::Poll::Ready(Ok(()))
    }
}

// Zero-allocation discard writer that counts received bytes
struct BenchWriter {
    received: u64,
}

impl BenchWriter {
    fn new() -> Self {
        Self { received: 0 }
    }
}

impl tokio::io::AsyncWrite for BenchWriter {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        self.received += buf.len() as u64;
        std::task::Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let size_mb: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(1024);

    let total_bytes = size_mb * 1024 * 1024;

    println!("============================================================");
    println!("   Quiczilla Local Loopback (127.0.0.1) Benchmark");
    println!("   Payload Size: {} MB ({} bytes)", size_mb, total_bytes);
    println!("   Protocol: MsQuic RFC 9000 + mTLS Encryption + Flow Control");
    println!("============================================================\n");

    // 1. Generate Certificates & mTLS Config
    let engine = MsQuicEngine::new()?;
    let (server_tp, server_config) = build_quic_config(&engine, true)?;
    let (client_tp, client_config) = build_quic_config(&engine, false)?;

    println!("[1/4] Generated server and client mTLS credentials");

    // 2. Start Local QUIC Server on 127.0.0.1
    let mut server_endpoint = MsQuicListener::start(&engine, &server_config, 0, vec![client_tp])?;
    let bound_port = server_endpoint.local_addr()?.port();
    println!("[2/4] QUIC Receiver listening on 127.0.0.1:{}", bound_port);

    let cancel = CancellationToken::new();
    let cancel_srv = cancel.clone();

    // Spawn receiver task
    let server_handle = tokio::spawn(async move {
        if let Some(mut connection) = server_endpoint.conn_rx.recv().await {
            let stream = connection.accept_stream().await.expect("Accept stream");
            let mut recv = stream.recv;
            let send = stream.send;
            let mut header = [0u8; 1];
            recv.read_exact(&mut header).await?;
            assert_eq!(header[0], PIPE_STREAM_HEADER);

            let writer = BenchWriter::new();
            let res = run_pipe(
                send,
                recv,
                None::<BenchReader>,
                Some(writer),
                false, // verify flag
                cancel_srv,
            )
            .await?;
            return Ok::<_, anyhow::Error>(res);
        }
        anyhow::bail!("No incoming connection");
    });

    // 3. Connect Local Client
    let remote_addr: SocketAddr = format!("127.0.0.1:{}", bound_port).parse()?;
    let conn_start = Instant::now();
    let connection =
        MsQuicConnection::connect(&engine, &client_config, remote_addr, None, Some(server_tp))
            .await?;
    let conn_duration = conn_start.elapsed();
    println!(
        "[3/4] QUIC mTLS Connected in {:.2}ms",
        conn_duration.as_secs_f64() * 1000.0
    );

    let stream = connection.open_stream().await?;
    let recv_stream = stream.recv;
    let mut send_stream = stream.send;
    send_stream.write_all(&[PIPE_STREAM_HEADER]).await?;

    // 4. Stream Payload
    println!("[4/4] Blasting {} MB over local loopback...", size_mb);
    let stream_start = Instant::now();
    let reader = BenchReader::new(total_bytes);

    let client_res = run_pipe(
        send_stream,
        recv_stream,
        Some(reader),
        None::<BenchWriter>,
        false,
        cancel,
    )
    .await?;

    let stream_duration = stream_start.elapsed();
    let server_res = server_handle.await??;

    let duration_secs = stream_duration.as_secs_f64().max(0.001);
    let speed_mb_s = (client_res.bytes_sent as f64 / 1_048_576.0) / duration_secs;
    let speed_gbps = (speed_mb_s * 8.0) / 1024.0;

    println!("\n============================================================");
    println!("   LOCAL BENCHMARK RESULTS (100% Zero-WAN)");
    println!("============================================================");
    println!(
        "Bytes Transferred : {} MB ({} bytes)",
        size_mb, server_res.bytes_received
    );
    println!(
        "Handshake Latency : {:.2} ms",
        conn_duration.as_secs_f64() * 1000.0
    );
    println!("Transfer Duration : {:.3} s", duration_secs);
    println!("Throughput (MB/s) : {:.2} MB/s", speed_mb_s);
    println!("Throughput (Gbps) : {:.2} Gbps", speed_gbps);
    println!("============================================================\n");

    Ok(())
}
