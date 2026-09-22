use anyhow::{Context, Result};
use quiczilla_core::cert::{
    build_persistent_quic_config, build_quic_config, default_identity_dir, normalize_thumbprint,
};
use quiczilla_core::msquic::engine::MsQuicConnection;
use quiczilla_core::msquic::engine::MsQuicEngine;
use quiczilla_core::msquic::engine::MsQuicListener;
use quiczilla_core::peer::{PeerCommand, PeerEvent, run_peer};
use quiczilla_core::pipe::run_pipe;
use quiczilla_core::types::{CONTROL_STREAM_HEADER, FILE_STREAM_HEADER, PIPE_STREAM_HEADER};
use std::env;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ConflictPolicy {
    Overwrite,
    Refuse,
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let args: Vec<String> = env::args().collect();
    let mut expected_thumbprint = String::new();
    let mut save_dir = std::env::current_dir()?.to_string_lossy().to_string();
    let mut is_pipe_mode = false;
    let mut exec_command: Option<String> = None;
    let mut verify = false;
    let mut is_daemon = false;
    let mut allow_thumbprints_file: Option<String> = None;
    let mut port: u16 = 0;
    let mut punch_port: Option<u16> = None;
    let mut punch_ip: Option<String> = None;
    let mut stun_server: Option<std::net::SocketAddr> = None;
    let mut resume = false;
    let mut identity_dir = None;
    let mut conflict_policy = ConflictPolicy::Overwrite;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--peer-thumbprint" => {
                if i + 1 < args.len() {
                    expected_thumbprint = args[i + 1].clone();
                    i += 1;
                }
            }
            "--save-dir" => {
                if i + 1 < args.len() {
                    save_dir = args[i + 1]
                        .trim_matches(|c: char| c.is_control() || c == '\0')
                        .to_string();
                    i += 1;
                }
            }
            "--pipe" => {
                is_pipe_mode = true;
            }
            "--exec" => {
                if i + 1 < args.len() {
                    exec_command = Some(args[i + 1].clone());
                    i += 1;
                }
            }
            "--exec-hex" => {
                if i + 1 < args.len() {
                    if let Ok(bytes) = hex::decode(&args[i + 1]) {
                        if let Ok(cmd) = String::from_utf8(bytes) {
                            exec_command = Some(cmd);
                        }
                    }
                    i += 1;
                }
            }
            "--verify" | "--checksum" => {
                verify = true;
            }
            "--daemon" => {
                is_daemon = true;
            }
            "--allow-thumbprints" => {
                if i + 1 < args.len() {
                    allow_thumbprints_file = Some(args[i + 1].clone());
                    i += 1;
                }
            }
            "--port" => {
                if i + 1 < args.len() {
                    port = args[i + 1].parse().unwrap_or(0);
                    i += 1;
                }
            }
            "--punch-port" => {
                if i + 1 < args.len() {
                    punch_port = args[i + 1].parse().ok();
                    i += 1;
                }
            }
            "--punch-ip" => {
                if i + 1 < args.len() {
                    punch_ip = Some(args[i + 1].clone());
                    i += 1;
                }
            }
            "--stun-server" => {
                if i + 1 < args.len() {
                    stun_server = Some(args[i + 1].parse().with_context(|| {
                        format!("invalid STUN server address '{}'", args[i + 1])
                    })?);
                    i += 1;
                }
            }
            "-c" | "--resume" => {
                resume = true;
            }
            "--identity-dir" if i + 1 < args.len() => {
                identity_dir = Some(std::path::PathBuf::from(&args[i + 1]));
                i += 1;
            }
            "--on-conflict" if i + 1 < args.len() => {
                conflict_policy = match args[i + 1].as_str() {
                    "overwrite" => ConflictPolicy::Overwrite,
                    "refuse" => ConflictPolicy::Refuse,
                    value => anyhow::bail!(
                        "invalid --on-conflict value '{value}'; expected overwrite or refuse"
                    ),
                };
                i += 1;
            }
            "--on-conflict" => {
                anyhow::bail!("--on-conflict requires overwrite or refuse");
            }
            _ => {}
        }
        i += 1;
    }

    // Determine allowed thumbprints
    let mut allowed_thumbprints = Vec::new();
    if let Some(ref file_path) = allow_thumbprints_file {
        let content = std::fs::read_to_string(file_path)?;
        for (line_number, line) in content.lines().enumerate() {
            let trimmed = line.trim();
            if !trimmed.is_empty() && !trimmed.starts_with('#') {
                let thumbprint = normalize_thumbprint(trimmed).with_context(|| {
                    format!("invalid thumbprint at {file_path}:{}", line_number + 1)
                })?;
                if !allowed_thumbprints.contains(&thumbprint) {
                    allowed_thumbprints.push(thumbprint);
                }
            }
        }
    }
    if !expected_thumbprint.is_empty() {
        allowed_thumbprints.push(normalize_thumbprint(&expected_thumbprint)?);
    }

    if allowed_thumbprints.is_empty() {
        anyhow::bail!(
            "client authorization is required; use --peer-thumbprint or --allow-thumbprints"
        );
    }

    // Compute punch target if available
    let punch_target = if let Some(p_port) = punch_port {
        let ip_str = if let Some(ref ip) = punch_ip {
            ip.clone()
        } else if let Ok(ssh_conn) = std::env::var("SSH_CONNECTION") {
            ssh_conn.split_whitespace().next().unwrap_or("").to_string()
        } else if let Ok(ssh_client) = std::env::var("SSH_CLIENT") {
            ssh_client
                .split_whitespace()
                .next()
                .unwrap_or("")
                .to_string()
        } else {
            String::new()
        };

        if !ip_str.is_empty() {
            format!("{}:{}", ip_str, p_port)
                .parse::<std::net::SocketAddr>()
                .ok()
        } else {
            None
        }
    } else {
        None
    };

    // Generate server certificate
    let engine = MsQuicEngine::new()?;
    let (thumbprint, config) = if is_daemon {
        let identity_dir = match identity_dir {
            Some(path) => path,
            None => default_identity_dir("daemon")?,
        };
        build_persistent_quic_config(&engine, true, &identity_dir)?
    } else {
        build_quic_config(&engine, true)?
    };

    let bind_port = if is_daemon && port == 0 { 55441 } else { port };

    // Discover the worker's server-reflexive UDP candidate before opening the
    // MsQuic listener. Reusing the discovered local port keeps the NAT mapping
    // associated with the port advertised to the client over SSH bootstrap.
    let (bind_port, public_udp_addr) = if let Some(server) = stun_server {
        let candidate = quiczilla_core::stun::discover(server, bind_port)
            .with_context(|| format!("STUN discovery failed for {server}"))?;
        (candidate.local_port, Some(candidate.public_addr))
    } else {
        (bind_port, None)
    };

    if let Some(target) = punch_target {
        if let Ok(std_socket) = std::net::UdpSocket::bind(format!("0.0.0.0:{}", bind_port)) {
            for _ in 0..5 {
                let _ = std_socket.send_to(b"QUIC_NAT_PUNCH", target);
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }

    let mut endpoint = MsQuicListener::start(&engine, &config, bind_port, allowed_thumbprints)?;
    let local_port = endpoint.local_addr()?.port();

    // Output JSON metadata to stdout so CLI/caller can parse it
    let status_str = if is_daemon { "daemon_ready" } else { "ready" };
    let json_output = serde_json::json!({
        "status": status_str,
        "udp_port": local_port,
        "thumbprint": thumbprint,
        "public_udp_addr": public_udp_addr.map(|address| address.to_string())
    });
    println!("{}", json_output);

    let cancel = CancellationToken::new();

    if is_daemon {
        eprintln!(
            "[Worker Daemon] Listening on UDP port {}. Thumbprint: {}",
            local_port, thumbprint
        );
        let save_dir_arc = Arc::new(save_dir);

        while let Some(incoming) = endpoint.conn_rx.recv().await {
            let save_dir = Arc::clone(&save_dir_arc);
            tokio::spawn(async move {
                let connection = incoming;
                {
                    eprintln!("[Worker Daemon] Peer connected from [remote]");
                    let _ = handle_incoming_connection(
                        connection,
                        (*save_dir).clone(),
                        false,
                        None,
                        verify,
                        resume,
                        conflict_policy,
                        CancellationToken::new(),
                    )
                    .await;
                }
            });
        }
        return Ok(());
    }

    // Ephemeral single-session mode
    eprintln!("[Worker] Waiting for connection on conn_rx...");
    if let Some(incoming) = endpoint.conn_rx.recv().await {
        let connection = incoming;
        eprintln!("[Worker] Connection established from [remote]");
        handle_incoming_connection(
            connection,
            save_dir,
            is_pipe_mode,
            exec_command,
            verify,
            resume,
            conflict_policy,
            cancel.clone(),
        )
        .await?;
    } else {
        eprintln!("[Worker] endpoint.conn_rx returned None!");
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn handle_incoming_connection(
    mut connection: MsQuicConnection,
    save_dir: String,
    is_pipe_mode: bool,
    exec_command: Option<String>,
    verify: bool,
    resume: bool,
    conflict_policy: ConflictPolicy,
    cancel: CancellationToken,
) -> Result<()> {
    // Accept the first bidirectional stream to determine mode
    eprintln!("[Worker] handle_incoming_connection: accepting stream 1...");
    let stream = match connection.accept_stream().await {
        Some(s) => s,
        None => {
            eprintln!("[Worker] accept_stream() for stream 1 returned None!");
            return Ok(());
        }
    };
    eprintln!(
        "[Worker] Accepted stream 1 (handle {:?})",
        stream.send.shared.lock().unwrap().stream_handle
    );
    let mut recv = stream.recv;
    let send = stream.send;
    let mut header = [0u8; 1];
    match recv.read_exact(&mut header).await {
        Ok(_) => eprintln!("[Worker] Stream 1 header: 0x{:02x}", header[0]),
        Err(e) => {
            eprintln!("[Worker] Failed to read stream 1 header: {:?}", e);
            return Ok(());
        }
    }

    if header[0] == PIPE_STREAM_HEADER || is_pipe_mode {
        eprintln!("[Worker] Entering raw pipe mode");

        if let Some(cmd) = exec_command {
            eprintln!("[Worker Pipe] Executing child command: {}", cmd);
            #[cfg(not(target_os = "windows"))]
            let mut child = tokio::process::Command::new("sh")
                .arg("-c")
                .arg(&cmd)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()?;

            #[cfg(target_os = "windows")]
            let mut child = tokio::process::Command::new("cmd")
                .arg("/c")
                .arg(&cmd)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()?;

            let child_in = child.stdin.take();
            let child_out = child.stdout.take();

            let cancel_child = cancel.clone();
            let child_wait = tokio::spawn(async move {
                let s = child.wait().await;
                tokio::time::sleep(Duration::from_millis(100)).await;
                cancel_child.cancel();
                s
            });

            let res = run_pipe(send, recv, child_out, child_in, verify, cancel.clone()).await?;
            let status = child_wait.await??;
            eprintln!(
                "[Worker Pipe] Command exited: {:?}. Bytes In: {}, Bytes Out: {}",
                status.code(),
                res.bytes_received,
                res.bytes_sent
            );
            if verify {
                eprintln!(
                    "[Worker Pipe SHA-256] In: {:?} | Out: {:?}",
                    res.received_hash, res.sent_hash
                );
            }
        } else {
            let res = run_pipe(
                send,
                recv,
                Some(tokio::io::stdin()),
                Some(tokio::io::stdout()),
                verify,
                cancel.clone(),
            )
            .await?;
            eprintln!(
                "[Worker Pipe] Completed. Bytes In: {}, Bytes Out: {}",
                res.bytes_received, res.bytes_sent
            );
            if verify {
                eprintln!(
                    "[Worker Pipe SHA-256] In: {:?} | Out: {:?}",
                    res.received_hash, res.sent_hash
                );
            }
        }
        return Ok(());
    } else if header[0] == CONTROL_STREAM_HEADER || header[0] == FILE_STREAM_HEADER {
        // File transfer mode: accept second stream
        let mut ctrl_stream = None;
        let mut file_stream = None;

        if header[0] == CONTROL_STREAM_HEADER {
            ctrl_stream = Some((send, recv));
        } else {
            file_stream = Some((send, recv));
        }

        eprintln!("[Worker] handle_incoming_connection: accepting stream 2...");
        let stream2 = match connection.accept_stream().await {
            Some(s) => s,
            None => {
                eprintln!("[Worker] accept_stream() for stream 2 returned None!");
                return Ok(());
            }
        };
        eprintln!(
            "[Worker] Accepted stream 2 (handle {:?})",
            stream2.send.shared.lock().unwrap().stream_handle
        );
        let mut r2 = stream2.recv;
        let s2 = stream2.send;
        let mut h2 = [0u8; 1];
        match r2.read_exact(&mut h2).await {
            Ok(_) => eprintln!("[Worker] Stream 2 header: 0x{:02x}", h2[0]),
            Err(e) => {
                eprintln!("[Worker] Failed to read stream 2 header: {:?}", e);
                return Ok(());
            }
        }

        if h2[0] == CONTROL_STREAM_HEADER {
            ctrl_stream = Some((s2, r2));
        } else if h2[0] == FILE_STREAM_HEADER {
            file_stream = Some((s2, r2));
        } else {
            eprintln!(
                "[Worker] ERROR: Unrecognized stream 2 header: 0x{:02x} ('{}')!",
                h2[0], h2[0] as char
            );
        }

        if let (Some((c_send, c_recv)), Some((f_send, f_recv))) =
            (ctrl_stream.take(), file_stream.take())
        {
            eprintln!("[Worker] Both streams matched! Spawning run_peer...");
            let (event_tx, mut event_rx) = mpsc::unbounded_channel();
            let (command_tx, command_rx) = mpsc::unbounded_channel();

            let cancel_peer = cancel.clone();
            let peer_handle = tokio::spawn(async move {
                run_peer(
                    connection,
                    c_send,
                    c_recv,
                    f_send,
                    f_recv,
                    event_tx,
                    command_rx,
                    cancel_peer,
                )
                .await
            });

            while let Some(event) = event_rx.recv().await {
                match event {
                    PeerEvent::Connected => {
                        eprintln!("Worker: Connected to peer.");
                    }
                    PeerEvent::FileOffered {
                        file_name,
                        file_size,
                        resume: offered_resume,
                        checksum: offered_checksum,
                    } => {
                        eprintln!(
                            "Worker: Receiving file: {} ({} bytes, resume: {}, checksum: {})",
                            file_name, file_size, offered_resume, offered_checksum
                        );
                        let safe_name = Path::new(&file_name)
                            .file_name()
                            .context("offered file has no usable file name")?;
                        let destination = Path::new(&save_dir).join(safe_name);
                        let resume_requested = resume || offered_resume;
                        if conflict_policy == ConflictPolicy::Refuse
                            && destination.exists()
                            && !resume_requested
                        {
                            eprintln!(
                                "Worker: Refusing existing destination {} (--on-conflict refuse)",
                                destination.display()
                            );
                            command_tx.send(PeerCommand::RejectFile)?;
                            continue;
                        }
                        command_tx.send(PeerCommand::AcceptFile {
                            save_dir: save_dir.clone(),
                            checksum: verify || offered_checksum,
                            resume: resume_requested,
                        })?;
                    }
                    PeerEvent::TransferProgress(prog) => {
                        if prog.percentage >= 100.0 {
                            eprintln!("Worker: File transfer 100% completed.");
                        }
                    }
                    PeerEvent::TransferPaused => {
                        eprintln!("Worker: Transfer paused.");
                    }
                    PeerEvent::TransferResumed => {
                        eprintln!("Worker: Transfer resumed.");
                    }
                    PeerEvent::TransferComplete { status } => {
                        eprintln!("Worker: TransferComplete: {:?}", status);
                        tokio::time::sleep(Duration::from_millis(200)).await;
                        cancel.cancel();
                        break;
                    }
                    PeerEvent::Disconnected { reason } => {
                        eprintln!("Worker: Disconnected: {}", reason);
                        cancel.cancel();
                        break;
                    }
                    _ => {}
                }
            }

            let _ = peer_handle.await;
        } else {
            eprintln!("[Worker] ERROR: Stream pair mismatch: need both control and file stream!");
        }
    } else {
        eprintln!(
            "[Worker] ERROR: Unrecognized stream 1 header: 0x{:02x} ('{}')!",
            header[0], header[0] as char
        );
    }

    Ok(())
}
