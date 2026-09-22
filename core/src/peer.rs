use crate::msquic::engine::MsQuicConnection;
use crate::msquic::stream::{MsQuicRecvStream, MsQuicSendStream};
use anyhow::Result;
use sha2::{Digest, Sha256};
use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::types::{
    ControlMessage, FILE_CHUNK_SIZE, FileTransferStatus, PROGRESS_REPORT_INTERVAL_MS, ProgressInfo,
    RESUME_FINGERPRINT_SIZE, RESUME_THRESHOLD_BYTES, SPEED_ESTIMATION_INTERVAL_MS,
    compute_file_sha256_blocking, compute_prefix_hash, get_part_path,
};

#[derive(Debug, Clone)]
pub enum PeerEvent {
    Connected,
    Disconnected {
        reason: String,
    },
    FileOffered {
        file_name: String,
        file_size: u64,
        resume: bool,
        checksum: bool,
    },
    TransferProgress(ProgressInfo),
    TransferPaused,
    TransferResumed,
    TransferComplete {
        status: FileTransferStatus,
    },
    TransferInitiationChanged {
        initiated: bool,
    },
}

#[derive(Debug, Clone)]
pub enum PeerCommand {
    SendFile {
        path: String,
        checksum: bool,
        resume: bool,
    },
    AcceptFile {
        save_dir: String,
        checksum: bool,
        resume: bool,
    },
    RejectFile,
    Pause,
    Resume,
    Disconnect,
}

#[allow(clippy::too_many_arguments)]
pub async fn run_peer(
    connection: MsQuicConnection,
    mut control_send: MsQuicSendStream,
    mut control_recv: MsQuicRecvStream,
    mut file_send: MsQuicSendStream,
    mut file_recv: MsQuicRecvStream,
    event_tx: mpsc::UnboundedSender<PeerEvent>,
    mut command_rx: mpsc::UnboundedReceiver<PeerCommand>,
    cancel: CancellationToken,
) -> Result<()> {
    let _ = event_tx.send(PeerEvent::Connected);

    // Channel for sending control messages
    let (control_tx, mut control_rx) = mpsc::unbounded_channel::<String>();

    // Channel to trigger file sending: (path, checksum, resume_offset)
    let (send_file_trigger_tx, mut send_file_trigger_rx) =
        mpsc::unbounded_channel::<(String, bool, u64)>();

    // Channel to trigger file receiving: (dest_path, file_size, checksum, resume_offset, use_part)
    let (recv_file_trigger_tx, mut recv_file_trigger_rx) =
        mpsc::unbounded_channel::<(PathBuf, u64, bool, u64, bool)>();

    // Channel to notify the file receiver of the expected hash (from FILE_SENT:<hash>)
    let (hash_tx, mut hash_rx) = mpsc::unbounded_channel::<String>();

    // Pause state watch channels
    let (pause_tx, pause_rx) = tokio::sync::watch::channel(false);
    let pause_rx_send = pause_rx.clone();
    let pause_rx_recv = pause_rx.clone();

    // 1. Control Send Task
    let cancel_send = cancel.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                biased;
                _ = cancel_send.cancelled() => break,
                res = control_rx.recv() => {
                    match res {
                        Some(m) => {
                            if send_control_message(&mut control_send, &m).await.is_err() {
                                break;
                            }
                        }
                        None => break,
                    }
                }
            }
        }
    });

    // 2. Liveness Loop (pings every 2s)
    let cancel_live = cancel.clone();
    let control_tx_live = control_tx.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(2));
        interval.tick().await; // Consume initial immediate tick
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if control_tx_live.send(String::new()).is_err() {
                        break;
                    }
                }
                _ = cancel_live.cancelled() => break,
            }
        }
    });

    // 3. File Send Task
    let cancel_file_send = cancel.clone();
    let event_tx_send = event_tx.clone();
    let control_tx_file = control_tx.clone();
    tokio::spawn(async move {
        while let Some((path_str, checksum, resume_offset)) = send_file_trigger_rx.recv().await {
            let path = Path::new(&path_str);
            let mut file = match File::open(path).await {
                Ok(f) => f,
                Err(e) => {
                    let _ = event_tx_send.send(PeerEvent::Disconnected {
                        reason: format!("Failed to open file to send: {e}"),
                    });
                    continue;
                }
            };

            let total_bytes = match file.metadata().await {
                Ok(m) => m.len(),
                Err(_) => 0,
            };

            if resume_offset > 0 {
                if let Err(e) = file.seek(SeekFrom::Start(resume_offset)).await {
                    let _ = event_tx_send.send(PeerEvent::Disconnected {
                        reason: format!("Failed to seek file for resumption: {e}"),
                    });
                    continue;
                }
            }

            let (chunk_tx, mut chunk_rx) = mpsc::channel::<Vec<u8>>(16);
            let cancel_reader = cancel_file_send.clone();
            let path_buf = path.to_path_buf();

            // When resuming with checksum enabled, compute whole-file hash in background thread
            let full_hash_handle = if checksum && resume_offset > 0 {
                Some(tokio::task::spawn_blocking(move || {
                    compute_file_sha256_blocking(&path_buf).unwrap_or_else(|_| "NONE".to_string())
                }))
            } else {
                None
            };

            // Task 3a: Disk reader & optional hasher producer
            let reader_handle = tokio::spawn(async move {
                let mut hasher = if checksum && resume_offset == 0 {
                    Some(Sha256::new())
                } else {
                    None
                };
                let mut buffer = vec![0u8; FILE_CHUNK_SIZE];
                let mut total_read = 0u64;

                loop {
                    if cancel_reader.is_cancelled() {
                        return Err(anyhow::anyhow!("File read cancelled"));
                    }

                    let n = match file.read(&mut buffer).await {
                        Ok(0) => break,
                        Ok(n) => n,
                        Err(e) => return Err(anyhow::anyhow!("File read error: {e}")),
                    };

                    if let Some(ref mut h) = hasher {
                        h.update(&buffer[..n]);
                    }
                    total_read += n as u64;

                    if chunk_tx.send(buffer[..n].to_vec()).await.is_err() {
                        return Err(anyhow::anyhow!("Stream writer disconnected"));
                    }
                }

                let final_hash = hasher
                    .map(|h| hex::encode(h.finalize()).to_lowercase())
                    .unwrap_or_else(|| "NONE".to_string());
                Ok((total_read, final_hash))
            });

            // Task 3b: QUIC stream network consumer
            let mut bytes_transferred = resume_offset;
            let start_time = Instant::now();
            let mut last_report_time = Instant::now();
            let mut last_speed_time = Instant::now();
            let mut last_speed_bytes = resume_offset;
            let mut current_speed = 0.0;
            let mut send_success = true;
            let mut is_paused = pause_rx_send.clone();

            while let Some(chunk) = chunk_rx.recv().await {
                while *is_paused.borrow() {
                    if cancel_file_send.is_cancelled() {
                        send_success = false;
                        break;
                    }
                    if is_paused.changed().await.is_err() {
                        break;
                    }
                }

                let chunk_len = chunk.len();
                if let Err(e) = file_send.write_all(&chunk).await {
                    tracing::error!("Error writing file chunk: {e}");
                    send_success = false;
                    break;
                }

                bytes_transferred += chunk_len as u64;

                let now = Instant::now();
                if now.duration_since(last_speed_time)
                    >= Duration::from_millis(SPEED_ESTIMATION_INTERVAL_MS)
                {
                    let elapsed = now.duration_since(last_speed_time).as_secs_f64();
                    current_speed = (bytes_transferred - last_speed_bytes) as f64 / elapsed;
                    last_speed_bytes = bytes_transferred;
                    last_speed_time = now;
                }

                if now.duration_since(last_report_time)
                    >= Duration::from_millis(PROGRESS_REPORT_INTERVAL_MS)
                {
                    let remaining_secs = if current_speed > 0.0 {
                        Some((total_bytes - bytes_transferred) as f64 / current_speed)
                    } else {
                        None
                    };

                    let _ = event_tx_send.send(PeerEvent::TransferProgress(ProgressInfo {
                        bytes_transferred,
                        total_bytes,
                        speed_bytes_per_second: current_speed,
                        percentage: if total_bytes > 0 {
                            (bytes_transferred as f64 / total_bytes as f64) * 100.0
                        } else {
                            0.0
                        },
                        estimated_remaining_secs: remaining_secs,
                        is_completed: false,
                        average_speed_bytes_per_second: None,
                        total_time_secs: None,
                    }));
                    last_report_time = now;
                }

                if cancel_file_send.is_cancelled() {
                    send_success = false;
                    break;
                }
            }

            if send_success {
                match reader_handle.await {
                    Ok(Ok((_, inline_hash))) => {
                        let _ = file_send.flush().await;
                        let final_hash = if let Some(h) = full_hash_handle {
                            h.await.unwrap_or_else(|_| "NONE".to_string())
                        } else {
                            inline_hash
                        };

                        let total_elapsed = start_time.elapsed().as_secs_f64();
                        let avg_speed = if total_elapsed > 0.0 {
                            (bytes_transferred - resume_offset) as f64 / total_elapsed
                        } else {
                            0.0
                        };

                        let _ = event_tx_send.send(PeerEvent::TransferProgress(ProgressInfo {
                            bytes_transferred,
                            total_bytes,
                            speed_bytes_per_second: current_speed,
                            percentage: 100.0,
                            estimated_remaining_secs: Some(0.0),
                            is_completed: true,
                            average_speed_bytes_per_second: Some(avg_speed),
                            total_time_secs: Some(total_elapsed),
                        }));

                        let _ = control_tx_file
                            .send(ControlMessage::FileSent { hash: final_hash }.serialize());
                    }
                    Ok(Err(e)) => {
                        tracing::error!("Disk reader error: {e}");
                    }
                    Err(e) => {
                        tracing::error!("Reader task join error: {e}");
                    }
                }
            }
        }
    });

    // 4. File Receive Task
    let cancel_file_recv = cancel.clone();
    let event_tx_recv = event_tx.clone();
    let control_tx_recv = control_tx.clone();
    tokio::spawn(async move {
        while let Some((dest_path, file_size, checksum, resume_offset, use_part)) =
            recv_file_trigger_rx.recv().await
        {
            if let Some(parent) = dest_path.parent() {
                let _ = tokio::fs::create_dir_all(parent).await;
            }

            let target_write_path = if use_part {
                get_part_path(&dest_path)
            } else {
                dest_path.clone()
            };

            let mut file = if resume_offset > 0 {
                match tokio::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create(true)
                    .truncate(false)
                    .open(&target_write_path)
                    .await
                {
                    Ok(mut f) => {
                        if let Err(e) = f.seek(SeekFrom::Start(resume_offset)).await {
                            let _ = event_tx_recv.send(PeerEvent::Disconnected {
                                reason: format!("Failed to seek destination file: {e}"),
                            });
                            continue;
                        }
                        f
                    }
                    Err(e) => {
                        let _ = event_tx_recv.send(PeerEvent::Disconnected {
                            reason: format!("Failed to open destination file for resume: {e}"),
                        });
                        continue;
                    }
                }
            } else {
                match File::create(&target_write_path).await {
                    Ok(f) => f,
                    Err(e) => {
                        let _ = event_tx_recv.send(PeerEvent::Disconnected {
                            reason: format!("Failed to create destination file: {e}"),
                        });
                        continue;
                    }
                }
            };

            let (chunk_tx, mut chunk_rx) = mpsc::channel::<Vec<u8>>(16);
            let cancel_writer = cancel_file_recv.clone();
            let compute_inline_hash = checksum && resume_offset == 0;

            // Task 4a: Disk writer & optional hasher consumer
            let writer_handle = tokio::spawn(async move {
                let mut hasher = if compute_inline_hash {
                    Some(Sha256::new())
                } else {
                    None
                };
                let mut total_written = 0u64;

                while let Some(chunk) = chunk_rx.recv().await {
                    if cancel_writer.is_cancelled() {
                        return Err(anyhow::anyhow!("File write cancelled"));
                    }

                    if let Some(ref mut h) = hasher {
                        h.update(&chunk);
                    }
                    if let Err(e) = file.write_all(&chunk).await {
                        return Err(anyhow::anyhow!("File write error: {e}"));
                    }
                    total_written += chunk.len() as u64;
                }

                if let Err(e) = file.flush().await {
                    return Err(anyhow::anyhow!("File flush error: {e}"));
                }

                let actual_hash = hasher
                    .map(|h| hex::encode(h.finalize()).to_lowercase())
                    .unwrap_or_else(|| "NONE".to_string());
                Ok((total_written, actual_hash))
            });

            // Task 4b: QUIC stream network reader producer
            let mut buffer = vec![0u8; FILE_CHUNK_SIZE];
            let mut total_received = resume_offset;
            let start_time = Instant::now();
            let mut last_report_time = Instant::now();
            let mut last_speed_time = Instant::now();
            let mut last_speed_bytes = resume_offset;
            let mut current_speed = 0.0;
            let mut recv_success = true;
            let mut is_paused = pause_rx_recv.clone();

            while total_received < file_size {
                while *is_paused.borrow() {
                    if cancel_file_recv.is_cancelled() {
                        recv_success = false;
                        break;
                    }
                    if is_paused.changed().await.is_err() {
                        break;
                    }
                }

                let remaining = (file_size - total_received) as usize;
                let to_read = remaining.min(FILE_CHUNK_SIZE);

                let read_bytes = match file_recv.read(&mut buffer[..to_read]).await {
                    Ok(0) => {
                        recv_success = false;
                        break;
                    }
                    Ok(n) => n,
                    Err(e) => {
                        tracing::error!("Error reading from file stream: {e}");
                        recv_success = false;
                        break;
                    }
                };

                if chunk_tx.send(buffer[..read_bytes].to_vec()).await.is_err() {
                    tracing::error!("Disk writer disconnected");
                    recv_success = false;
                    break;
                }

                total_received += read_bytes as u64;

                let now = Instant::now();
                if now.duration_since(last_speed_time)
                    >= Duration::from_millis(SPEED_ESTIMATION_INTERVAL_MS)
                {
                    let elapsed = now.duration_since(last_speed_time).as_secs_f64();
                    current_speed = (total_received - last_speed_bytes) as f64 / elapsed;
                    last_speed_bytes = total_received;
                    last_speed_time = now;
                }

                if now.duration_since(last_report_time)
                    >= Duration::from_millis(PROGRESS_REPORT_INTERVAL_MS)
                {
                    let remaining_secs = if current_speed > 0.0 {
                        Some((file_size - total_received) as f64 / current_speed)
                    } else {
                        None
                    };

                    let _ = event_tx_recv.send(PeerEvent::TransferProgress(ProgressInfo {
                        bytes_transferred: total_received,
                        total_bytes: file_size,
                        speed_bytes_per_second: current_speed,
                        percentage: if file_size > 0 {
                            (total_received as f64 / file_size as f64) * 100.0
                        } else {
                            0.0
                        },
                        estimated_remaining_secs: remaining_secs,
                        is_completed: false,
                        average_speed_bytes_per_second: None,
                        total_time_secs: None,
                    }));
                    last_report_time = now;
                }

                if cancel_file_recv.is_cancelled() {
                    recv_success = false;
                    break;
                }
            }

            // Drop chunk_tx so writer_handle finishes its while let Some(chunk) loop
            drop(chunk_tx);

            if recv_success {
                let actual_hash = match writer_handle.await {
                    Ok(Ok((_, inline_hash))) => {
                        if checksum && resume_offset > 0 {
                            let p = target_write_path.clone();
                            match tokio::task::spawn_blocking(move || {
                                compute_file_sha256_blocking(&p)
                            })
                            .await
                            {
                                Ok(Ok(h)) => h,
                                _ => "NONE".to_string(),
                            }
                        } else {
                            inline_hash
                        }
                    }
                    Ok(Err(e)) => {
                        tracing::error!("Disk writer error: {e}");
                        continue;
                    }
                    Err(e) => {
                        tracing::error!("Writer task join error: {e}");
                        continue;
                    }
                };

                // Wait for the expected hash sent by sender
                if let Some(expected_hash) = hash_rx.recv().await {
                    let total_elapsed = start_time.elapsed().as_secs_f64();
                    let avg_speed = if total_elapsed > 0.0 {
                        (total_received - resume_offset) as f64 / total_elapsed
                    } else {
                        0.0
                    };

                    let _ = event_tx_recv.send(PeerEvent::TransferProgress(ProgressInfo {
                        bytes_transferred: total_received,
                        total_bytes: file_size,
                        speed_bytes_per_second: current_speed,
                        percentage: 100.0,
                        estimated_remaining_secs: Some(0.0),
                        is_completed: true,
                        average_speed_bytes_per_second: Some(avg_speed),
                        total_time_secs: Some(total_elapsed),
                    }));

                    let hash_ok = expected_hash == "NONE"
                        || expected_hash == "SKIP"
                        || actual_hash == "NONE"
                        || actual_hash.eq_ignore_ascii_case(&expected_hash);

                    if hash_ok {
                        if use_part {
                            if let Err(e) = tokio::fs::rename(&target_write_path, &dest_path).await
                            {
                                tracing::error!("Failed to rename part file to dest: {e}");
                                let _ = control_tx_recv
                                    .send(ControlMessage::ReceivedFileFailed.serialize());
                                let _ = event_tx_recv.send(PeerEvent::TransferComplete {
                                    status: FileTransferStatus::HashFailed,
                                });
                                continue;
                            }
                        }

                        let _ = control_tx_recv.send(ControlMessage::ReceivedFileOk.serialize());
                        let _ = event_tx_recv.send(PeerEvent::TransferComplete {
                            status: FileTransferStatus::Completed,
                        });
                    } else {
                        let _ =
                            control_tx_recv.send(ControlMessage::ReceivedFileFailed.serialize());
                        let _ = event_tx_recv.send(PeerEvent::TransferComplete {
                            status: FileTransferStatus::HashFailed,
                        });
                    }
                }
            }
        }
    });

    // 5. Main Coordination Loop
    let mut pending_send_path: Option<(String, bool, bool)> = None; // (path, checksum, resume)
    let mut offered_file_name: Option<String> = None;
    let mut offered_file_size: Option<u64> = None;
    let mut offered_resume = false;
    let mut pending_accept: Option<(String, bool, bool)> = None;
    let mut pending_resume_recv: Option<(PathBuf, u64, bool, u64, bool)> = None;
    let mut current_part_path: Option<PathBuf> = None;

    loop {
        tokio::select! {
            msg_res = read_control_message(&mut control_recv) => {
                match msg_res {
                    Ok(Some(msg)) => {
                        if msg.is_empty() {
                            continue; // Liveness ping
                        }
                        if let Some(ctrl_msg) = ControlMessage::parse(&msg) {
                            match ctrl_msg {
                                ControlMessage::Metadata { file_name, file_size, resume, checksum } => {
                                    offered_file_name = Some(file_name.clone());
                                    offered_file_size = Some(file_size);
                                    offered_resume = resume;
                                    let _ = event_tx.send(PeerEvent::TransferInitiationChanged { initiated: true });
                                    let _ = event_tx.send(PeerEvent::FileOffered { file_name, file_size, resume, checksum });

                                    if let Some((save_dir, checksum, accept_resume)) = pending_accept.take() {
                                        handle_accept(
                                            save_dir,
                                            checksum,
                                            accept_resume,
                                            offered_file_name.take().unwrap(),
                                            offered_file_size.take().unwrap(),
                                            offered_resume,
                                            &mut current_part_path,
                                            &mut pending_resume_recv,
                                            &recv_file_trigger_tx,
                                            &control_tx,
                                        ).await;
                                    }
                                }
                                ControlMessage::Ready => {
                                    if let Some((path, checksum, _)) = pending_send_path.take() {
                                        let _ = send_file_trigger_tx.send((path, checksum, 0));
                                    }
                                }
                                ControlMessage::ResumeReady { offset, prefix_hash } => {
                                    if let Some((path, checksum, _)) = pending_send_path.take() {
                                        let file_len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                                        if offset == file_len {
                                            // 100% already transferred
                                            let _ = event_tx.send(PeerEvent::TransferProgress(ProgressInfo {
                                                bytes_transferred: offset,
                                                total_bytes: file_len,
                                                speed_bytes_per_second: 0.0,
                                                percentage: 100.0,
                                                estimated_remaining_secs: Some(0.0),
                                                is_completed: true,
                                                average_speed_bytes_per_second: Some(0.0),
                                                total_time_secs: Some(0.0),
                                            }));
                                            let _ = event_tx.send(PeerEvent::TransferComplete {
                                                status: FileTransferStatus::Completed,
                                            });
                                        } else if offset > 0 {
                                            let local_hash = compute_prefix_hash(Path::new(&path), RESUME_FINGERPRINT_SIZE).await.ok();
                                            if local_hash == prefix_hash {
                                                // Verified prefix fingerprint match! Send ResumeOk and start at offset
                                                let _ = control_tx.send(ControlMessage::ResumeOk.serialize());
                                                let _ = send_file_trigger_tx.send((path, checksum, offset));
                                            } else {
                                                // Mismatch! Fall back to starting from zero
                                                pending_send_path = Some((path, checksum, false));
                                                let _ = control_tx.send(ControlMessage::RestartFromZero.serialize());
                                            }
                                        } else {
                                            let _ = send_file_trigger_tx.send((path, checksum, 0));
                                        }
                                    }
                                }
                                ControlMessage::ResumeOk => {
                                    if let Some((dest_path, size, checksum, offset, use_part)) = pending_resume_recv.take() {
                                        let _ = recv_file_trigger_tx.send((dest_path, size, checksum, offset, use_part));
                                    }
                                }
                                ControlMessage::RestartFromZero => {
                                    if let Some((dest_path, size, checksum, _, use_part)) = pending_resume_recv.take() {
                                        if let Some(ref part_path) = current_part_path {
                                            let _ = tokio::fs::File::create(part_path).await;
                                        }
                                        let _ = recv_file_trigger_tx.send((dest_path, size, checksum, 0, use_part));
                                        let _ = control_tx.send(ControlMessage::Ready.serialize());
                                    }
                                }
                                ControlMessage::Pause => {
                                    let _ = pause_tx.send(true);
                                    let _ = event_tx.send(PeerEvent::TransferPaused);
                                }
                                ControlMessage::Resume => {
                                    let _ = pause_tx.send(false);
                                    let _ = event_tx.send(PeerEvent::TransferResumed);
                                }
                                ControlMessage::FileSent { hash } => {
                                    let _ = hash_tx.send(hash);
                                }
                                ControlMessage::ReceivedFileOk => {
                                    let _ = event_tx.send(PeerEvent::TransferComplete {
                                        status: FileTransferStatus::Completed,
                                    });
                                }
                                ControlMessage::ReceivedFileFailed => {
                                    let _ = event_tx.send(PeerEvent::TransferComplete {
                                        status: FileTransferStatus::HashFailed,
                                    });
                                }
                                ControlMessage::RejectedUnwanted => {
                                    let _ = event_tx.send(PeerEvent::TransferComplete {
                                        status: FileTransferStatus::RejectedUnwanted,
                                    });
                                }
                                ControlMessage::RejectedAlreadyReceiving => {
                                    let _ = event_tx.send(PeerEvent::TransferComplete {
                                        status: FileTransferStatus::RejectedAlreadyReceiving,
                                    });
                                }
                                ControlMessage::RejectedAlreadySending => {
                                    let _ = event_tx.send(PeerEvent::TransferComplete {
                                        status: FileTransferStatus::RejectedAlreadySending,
                                    });
                                }
                            }
                        }
                    }
                    Ok(None) => {
                        let _ = event_tx.send(PeerEvent::Disconnected {
                            reason: "Peer closed connection".to_string(),
                        });
                        break;
                    }
                    Err(e) => {
                        let _ = event_tx.send(PeerEvent::Disconnected {
                            reason: e.to_string(),
                        });
                        break;
                    }
                }
            }
            cmd = command_rx.recv() => {
                match cmd {
                    Some(PeerCommand::SendFile { path, checksum, resume }) => {
                        let path_obj = Path::new(&path);
                        if let Ok(meta) = std::fs::metadata(path_obj) {
                            let file_name = path_obj
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .to_string();
                            let file_size = meta.len();

                            pending_send_path = Some((path, checksum, resume));
                            let _ = event_tx.send(PeerEvent::TransferInitiationChanged { initiated: true });
                            let _ = control_tx.send(ControlMessage::Metadata {
                                file_name,
                                file_size,
                                resume,
                                checksum,
                            }.serialize());
                        } else {
                            tracing::error!("std::fs::metadata failed for path: {}", path);
                        }
                    }
                    Some(PeerCommand::AcceptFile { save_dir, checksum, resume }) => {
                        if let (Some(name), Some(size)) = (offered_file_name.take(), offered_file_size.take()) {
                            handle_accept(
                                save_dir,
                                checksum,
                                resume,
                                name,
                                size,
                                offered_resume,
                                &mut current_part_path,
                                &mut pending_resume_recv,
                                &recv_file_trigger_tx,
                                &control_tx,
                            ).await;
                        } else {
                            pending_accept = Some((save_dir, checksum, resume));
                        }
                    }
                    Some(PeerCommand::RejectFile) => {
                        offered_file_name = None;
                        offered_file_size = None;
                        let _ = control_tx.send(ControlMessage::RejectedUnwanted.serialize());
                    }
                    Some(PeerCommand::Pause) => {
                        let _ = pause_tx.send(true);
                        let _ = control_tx.send(ControlMessage::Pause.serialize());
                        let _ = event_tx.send(PeerEvent::TransferPaused);
                    }
                    Some(PeerCommand::Resume) => {
                        let _ = pause_tx.send(false);
                        let _ = control_tx.send(ControlMessage::Resume.serialize());
                        let _ = event_tx.send(PeerEvent::TransferResumed);
                    }
                    Some(PeerCommand::Disconnect) => {
                        break;
                    }
                    None => break,
                }
            }
            _ = cancel.cancelled() => break,
        }
    }

    // Notify the peer promptly instead of relying on the 30-second idle
    // timeout. Dropping `connection` immediately afterward releases its
    // native MsQuic handle and all associated streams.
    connection.shutdown();
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn handle_accept(
    save_dir: String,
    checksum: bool,
    resume: bool,
    offered_name: String,
    size: u64,
    offered_resume: bool,
    current_part_path: &mut Option<PathBuf>,
    pending_resume_recv: &mut Option<(PathBuf, u64, bool, u64, bool)>,
    recv_file_trigger_tx: &mpsc::UnboundedSender<(PathBuf, u64, bool, u64, bool)>,
    control_tx: &mpsc::UnboundedSender<String>,
) {
    let clean_name = offered_name.trim_matches(|c: char| c.is_control() || c == '\0');
    let safe_name = Path::new(clean_name)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let clean_save_dir = save_dir.trim_matches(|c: char| c.is_control() || c == '\0');
    let dest_path = Path::new(clean_save_dir).join(&safe_name);

    let should_resume = offered_resume && resume;
    if should_resume && size >= RESUME_THRESHOLD_BYTES {
        let part_path = get_part_path(&dest_path);
        *current_part_path = Some(part_path.clone());

        if part_path.exists() {
            let existing_len = std::fs::metadata(&part_path).map(|m| m.len()).unwrap_or(0);
            let chunk_align = FILE_CHUNK_SIZE as u64;
            let mut aligned_offset = (existing_len / chunk_align) * chunk_align;
            if aligned_offset > size {
                aligned_offset = 0;
            }

            if aligned_offset > 0 {
                // Truncate to aligned chunk boundary
                if let Ok(f) = std::fs::OpenOptions::new().write(true).open(&part_path) {
                    let _ = f.set_len(aligned_offset);
                }
                let fingerprint = compute_prefix_hash(&part_path, RESUME_FINGERPRINT_SIZE)
                    .await
                    .ok();
                *pending_resume_recv = Some((dest_path, size, checksum, aligned_offset, true));
                let _ = control_tx.send(
                    ControlMessage::ResumeReady {
                        offset: aligned_offset,
                        prefix_hash: fingerprint,
                    }
                    .serialize(),
                );
            } else {
                let _ = recv_file_trigger_tx.send((dest_path, size, checksum, 0, true));
                let _ = control_tx.send(ControlMessage::Ready.serialize());
            }
        } else if dest_path.exists()
            && std::fs::metadata(&dest_path).map(|m| m.len()).unwrap_or(0) == size
        {
            let _ = control_tx.send(
                ControlMessage::ResumeReady {
                    offset: size,
                    prefix_hash: None,
                }
                .serialize(),
            );
        } else {
            let _ = recv_file_trigger_tx.send((dest_path, size, checksum, 0, true));
            let _ = control_tx.send(ControlMessage::Ready.serialize());
        }
    } else {
        *current_part_path = None;
        let _ = recv_file_trigger_tx.send((dest_path, size, checksum, 0, false));
        let _ = control_tx.send(ControlMessage::Ready.serialize());
    }
}

async fn send_control_message(stream: &mut MsQuicSendStream, msg: &str) -> Result<()> {
    let bytes = msg.as_bytes();
    let len = (bytes.len() as u32).to_be_bytes();
    stream.write_all(&len).await?;
    if !bytes.is_empty() {
        stream.write_all(bytes).await?;
    }
    stream.flush().await?;
    Ok(())
}

async fn read_control_message(stream: &mut MsQuicRecvStream) -> Result<Option<String>> {
    let mut len_buf = [0u8; 4];
    match stream.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(ref e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    let len = u32::from_be_bytes(len_buf) as usize;
    if len == 0 {
        return Ok(Some(String::new()));
    }
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf).await?;
    let s = String::from_utf8(buf)?;
    Ok(Some(s))
}
