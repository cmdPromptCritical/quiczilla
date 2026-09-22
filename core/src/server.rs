use anyhow::Result;
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::msquic::engine::{MsQuicConfiguration, MsQuicEngine, MsQuicListener};
use crate::peer::{PeerCommand, PeerEvent, run_peer};
use crate::types::{CONTROL_STREAM_HEADER, FILE_STREAM_HEADER};

#[allow(clippy::too_many_arguments)]
pub async fn start_server(
    local_port: Option<u16>,
    _is_ipv6: bool,
    allowed_client_thumbprints: Vec<String>,
    engine: &MsQuicEngine,
    config: &MsQuicConfiguration,
    event_tx: mpsc::UnboundedSender<PeerEvent>,
    command_rx: mpsc::UnboundedReceiver<PeerCommand>,
    cancel: CancellationToken,
) -> Result<()> {
    let mut listener = MsQuicListener::start(
        engine,
        config,
        local_port.unwrap_or(0),
        allowed_client_thumbprints,
    )?;

    let _local_addr = listener.local_addr()?;

    if let Some(mut connection) = listener.conn_rx.recv().await {
        let _ = event_tx.send(PeerEvent::Connected);

        let mut control_send = None;
        let mut control_recv = None;
        let mut file_send = None;
        let mut file_recv = None;

        for _ in 0..2 {
            if let Some(mut stream) = connection.accept_stream().await {
                let mut header = [0u8; 1];
                stream.recv.read_exact(&mut header).await?;

                if header[0] == CONTROL_STREAM_HEADER {
                    control_send = Some(stream.send);
                    control_recv = Some(stream.recv);
                } else if header[0] == FILE_STREAM_HEADER {
                    file_send = Some(stream.send);
                    file_recv = Some(stream.recv);
                }
            }
        }

        if let (Some(cs), Some(cr), Some(fs), Some(fr)) =
            (control_send, control_recv, file_send, file_recv)
        {
            run_peer(connection, cs, cr, fs, fr, event_tx, command_rx, cancel).await?;
        }
    }

    Ok(())
}
