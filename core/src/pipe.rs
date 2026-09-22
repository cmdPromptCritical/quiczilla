use crate::msquic::stream::{MsQuicRecvStream, MsQuicSendStream};
use anyhow::Result;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

pub const PIPE_BUFFER_SIZE: usize = 128 * 1024; // 128 KiB chunk buffer

#[derive(Debug, Clone, Default)]
pub struct PipeResult {
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub sent_hash: Option<String>,
    pub received_hash: Option<String>,
}

pub async fn run_pipe<R, W>(
    mut send_stream: MsQuicSendStream,
    mut recv_stream: MsQuicRecvStream,
    input: Option<R>,
    output: Option<W>,
    compute_hash: bool,
    cancel: CancellationToken,
) -> Result<PipeResult>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let cancel_send = cancel.clone();
    let cancel_recv = cancel.clone();

    // 1. Task: Input -> SendStream
    let send_task = tokio::spawn(async move {
        let mut bytes_sent = 0u64;
        let mut hasher = if compute_hash {
            Some(Sha256::new())
        } else {
            None
        };

        if let Some(mut reader) = input {
            let mut buf = vec![0u8; PIPE_BUFFER_SIZE];
            loop {
                tokio::select! {
                    biased;
                    _ = cancel_send.cancelled() => {
                        break;
                    }
                    read_res = reader.read(&mut buf) => {
                        match read_res {
                            Ok(0) => {
                                break;
                            }
                            Ok(n) => {
                                if let Some(ref mut h) = hasher {
                                    h.update(&buf[..n]);
                                }
                                if let Err(e) = send_stream.write_all(&buf[..n]).await {
                                    tracing::error!("Error writing to QUIC stream: {e}");
                                    break;
                                }
                                bytes_sent += n as u64;
                            }
                            Err(e) => {
                                tracing::error!("Error reading local input stream: {e}");
                                break;
                            }
                        }
                    }
                }
            }
        }

        let sent_hash = hasher.map(|h| hex::encode(h.finalize()).to_lowercase());
        (bytes_sent, sent_hash)
    });

    // 2. Task: RecvStream -> Output
    let recv_task = tokio::spawn(async move {
        let mut bytes_received = 0u64;
        let mut hasher = if compute_hash {
            Some(Sha256::new())
        } else {
            None
        };

        if let Some(mut writer) = output {
            let mut buf = vec![0u8; PIPE_BUFFER_SIZE];
            loop {
                tokio::select! {
                    biased;
                    _ = cancel_recv.cancelled() => {
                        break;
                    }
                    recv_res = recv_stream.read(&mut buf) => {
                        match recv_res {
                            Ok(0) => {
                                // Remote signalled FIN (clean EOF)
                                let _ = writer.flush().await;
                                cancel_recv.cancel();
                                break;
                            }
                            Ok(n) => {
                                if let Some(ref mut h) = hasher {
                                    h.update(&buf[..n]);
                                }
                                if let Err(e) = writer.write_all(&buf[..n]).await {
                                    tracing::error!("Error writing to local output stream: {e}");
                                    break;
                                }
                                bytes_received += n as u64;
                            }
                            Err(e) => {
                                tracing::error!("Error reading from QUIC stream: {e}");
                                break;
                            }
                        }
                    }
                }
            }
            let _ = writer.flush().await;
            drop(writer);
        }

        let received_hash = hasher.map(|h| hex::encode(h.finalize()).to_lowercase());
        (bytes_received, received_hash)
    });

    let (send_res, recv_res) = tokio::try_join!(send_task, recv_task)?;

    Ok(PipeResult {
        bytes_sent: send_res.0,
        bytes_received: recv_res.0,
        sent_hash: send_res.1,
        received_hash: recv_res.1,
    })
}
