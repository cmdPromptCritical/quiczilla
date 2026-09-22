use anyhow::Result;
use std::net::SocketAddr;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::msquic::engine::{MsQuicConfiguration, MsQuicConnection, MsQuicEngine};
use crate::peer::{PeerCommand, PeerEvent, run_peer};
use crate::types::{CONTROL_STREAM_HEADER, FILE_STREAM_HEADER};

#[allow(clippy::too_many_arguments)]
pub async fn start_client(
    remote_addr: SocketAddr,
    local_bind_addr: Option<std::net::SocketAddr>,
    _is_ipv6: bool,
    expected_server_thumbprint: String,
    engine: &MsQuicEngine,
    config: &MsQuicConfiguration,
    event_tx: mpsc::UnboundedSender<PeerEvent>,
    command_rx: mpsc::UnboundedReceiver<PeerCommand>,
    cancel: CancellationToken,
) -> Result<()> {
    let connection = connect_client(
        engine,
        config,
        remote_addr,
        local_bind_addr,
        expected_server_thumbprint,
    )
    .await?;
    start_connected_client(connection, event_tx, command_rx, cancel).await
}

/// Establish an mTLS-pinned QUIC connection without opening application streams.
///
/// The CLI uses this to race independently routable candidates, then starts the
/// file-transfer protocol only on the first authenticated connection.
pub async fn connect_client(
    engine: &MsQuicEngine,
    config: &MsQuicConfiguration,
    remote_addr: SocketAddr,
    local_bind_addr: Option<std::net::SocketAddr>,
    expected_server_thumbprint: String,
) -> Result<MsQuicConnection> {
    MsQuicConnection::connect(
        engine,
        config,
        remote_addr,
        local_bind_addr,
        Some(expected_server_thumbprint),
    )
    .await
}

/// Start Quiczilla's control and file streams on an authenticated connection.
pub async fn start_connected_client(
    connection: MsQuicConnection,
    event_tx: mpsc::UnboundedSender<PeerEvent>,
    command_rx: mpsc::UnboundedReceiver<PeerCommand>,
    cancel: CancellationToken,
) -> Result<()> {
    tracing::debug!("[client] MsQuic connection established!");

    let mut control_stream = connection.open_stream().await?;
    tracing::debug!("[client] Control stream opened!");
    use tokio::io::AsyncWriteExt;
    control_stream
        .send
        .write_all(&[CONTROL_STREAM_HEADER])
        .await?;
    control_stream.send.flush().await?;
    tracing::debug!("[client] Control stream header written and flushed!");

    let mut file_stream = connection.open_stream().await?;
    tracing::debug!("[client] File stream opened!");
    file_stream.send.write_all(&[FILE_STREAM_HEADER]).await?;
    file_stream.send.flush().await?;
    tracing::debug!("[client] File stream header written and flushed! Starting run_peer...");

    run_peer(
        connection,
        control_stream.send,
        control_stream.recv,
        file_stream.send,
        file_stream.recv,
        event_tx,
        command_rx,
        cancel,
    )
    .await
}
