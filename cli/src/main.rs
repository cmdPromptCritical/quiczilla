mod progress;

use anyhow::{Context, Result};
use clap::Parser;
use progress::ProgressRenderer;
use quiczilla_core::cert::{
    build_persistent_quic_config, build_quic_config, default_identity_dir, normalize_thumbprint,
};
use quiczilla_core::client::{connect_client, start_client, start_connected_client};
use quiczilla_core::msquic::engine::{MsQuicConfiguration, MsQuicConnection, MsQuicEngine};
use quiczilla_core::peer::{PeerCommand, PeerEvent};
use quiczilla_core::pipe::run_pipe;
use quiczilla_core::types::{FileTransferStatus, PIPE_STREAM_HEADER};
use sha2::{Digest, Sha256};
use std::env;
use std::fmt;
use std::fs;
use std::io::{BufRead, BufReader, IsTerminal, Write};
use std::net::{SocketAddr, ToSocketAddrs};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

struct RawModeGuard {
    active: bool,
}

impl RawModeGuard {
    fn new() -> Self {
        let active = crossterm::terminal::enable_raw_mode().is_ok();
        Self { active }
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        if self.active {
            let _ = crossterm::terminal::disable_raw_mode();
        }
    }
}

const WORKER_LINUX_X86_64: &[u8] = include_bytes!("../embedded/worker-linux-x86_64");
const WORKER_WINDOWS_X86_64: &[u8] = include_bytes!("../embedded/worker-windows-x86_64.exe");
mod embedded_msquic_linux {
    include!(concat!(env!("OUT_DIR"), "/msquic-linux-x86_64.rs"));
}
mod embedded_msquic_windows {
    include!(concat!(env!("OUT_DIR"), "/msquic-windows-x86_64.rs"));
}
const MSQUIC_LINUX_X86_64: Option<&[u8]> = embedded_msquic_linux::DATA;
const MSQUIC_WINDOWS_X86_64: Option<&[u8]> = embedded_msquic_windows::DATA;
// Kept in lockstep with the release tag by the workspace package version.
const WORKER_VERSION: &str = concat!("v", env!("CARGO_PKG_VERSION"));
const DIRECT_CANDIDATE_HEAD_START: Duration = Duration::from_millis(150);
const COMPILED_DEFAULT_STUN_SERVER: Option<&str> = option_env!("QUICZILLA_DEFAULT_STUN_SERVER");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportPreference {
    Auto,
    Direct,
    Stun,
    Manual,
    Ssh,
}

struct OutputOptions {
    quiet: bool,
    json: bool,
    verbose: bool,
    no_progress: bool,
    resume: bool,
    transport: TransportPreference,
    stun_server: Option<std::net::SocketAddr>,
    quic_host: Option<String>,
    quic_port: Option<u16>,
    ssh_port: Option<u16>,
    ssh_identity: Option<String>,
}

#[derive(Debug)]
struct CliFailure {
    code: i32,
    message: String,
}

impl CliFailure {
    fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for CliFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.message.fmt(formatter)
    }
}

impl std::error::Error for CliFailure {}

impl From<anyhow::Error> for CliFailure {
    fn from(error: anyhow::Error) -> Self {
        Self::new(1, error.to_string())
    }
}

impl From<std::io::Error> for CliFailure {
    fn from(error: std::io::Error) -> Self {
        Self::new(1, error.to_string())
    }
}

type CliResult<T> = std::result::Result<T, CliFailure>;

#[derive(Debug, Parser)]
#[command(name = "quic", disable_help_flag = true, disable_version_flag = true)]
#[allow(dead_code)]
struct TransferCli {
    local_file: String,
    remote_target: String,
    #[arg(short = 'c', long = "resume")]
    resume: bool,
    #[arg(long = "checksum", alias = "verify")]
    checksum: bool,
    #[arg(short = 'i', long = "identity")]
    ssh_identity: Option<String>,
    #[arg(short = 'p', long = "port", value_name = "SSH_PORT")]
    ssh_port: Option<u16>,
    #[arg(short = 'q', long = "quiet")]
    quiet: bool,
    #[arg(long = "no-progress")]
    no_progress: bool,
    #[arg(long = "json")]
    json: bool,
    #[arg(long = "verbose")]
    verbose: bool,
    #[arg(long, value_parser = ["auto", "direct", "stun", "manual", "ssh"])]
    transport: Option<String>,
    #[arg(long = "stun-server")]
    stun_server: Option<String>,
    #[arg(long = "quic-host", value_name = "HOST_OR_IP")]
    quic_host: Option<String>,
    #[arg(long = "quic-port", value_name = "UDP_PORT")]
    quic_port: Option<u16>,
}

#[derive(Debug, Parser)]
#[command(
    name = "quic pipe",
    disable_help_flag = true,
    disable_version_flag = true
)]
#[allow(dead_code)]
struct PipeCli {
    target: String,
    remote_command: Option<String>,
    #[arg(long = "checksum", alias = "verify")]
    checksum: bool,
    #[arg(short = 'i', long = "identity")]
    ssh_identity: Option<String>,
    #[arg(short = 'p', long = "port", value_name = "SSH_PORT")]
    ssh_port: Option<u16>,
    #[arg(short = 'q', long = "quiet")]
    quiet: bool,
    #[arg(long = "no-progress")]
    no_progress: bool,
    #[arg(long = "json")]
    json: bool,
    #[arg(long = "verbose")]
    verbose: bool,
}

#[derive(Debug, Parser)]
#[command(
    name = "quic daemon",
    disable_help_flag = true,
    disable_version_flag = true
)]
#[allow(dead_code)]
struct DaemonCli {
    #[arg(long = "allow-thumbprints", required = true)]
    allow_thumbprints: String,
    #[arg(long, value_name = "UDP_PORT")]
    port: Option<u16>,
    #[arg(long = "save-dir")]
    save_dir: Option<String>,
    #[arg(long = "identity-dir")]
    identity_dir: Option<String>,
    #[arg(long = "checksum", alias = "verify")]
    checksum: bool,
    #[arg(short = 'c', long = "resume")]
    resume: bool,
    #[arg(long = "on-conflict", value_parser = ["overwrite", "refuse"])]
    on_conflict: Option<String>,
}

#[derive(Debug, Parser)]
#[command(
    name = "quic direct",
    disable_help_flag = true,
    disable_version_flag = true
)]
#[allow(dead_code)]
struct DirectCli {
    host: String,
    local_file: String,
    #[arg(long = "thumbprint", required = true)]
    thumbprint: String,
    #[arg(long = "checksum", alias = "verify")]
    checksum: bool,
    #[arg(short = 'c', long = "resume")]
    resume: bool,
    #[arg(long = "identity-dir")]
    identity_dir: Option<String>,
    #[arg(short = 'q', long = "quiet")]
    quiet: bool,
    #[arg(long = "no-progress")]
    no_progress: bool,
    #[arg(long = "verbose")]
    verbose: bool,
}

#[derive(Debug, Parser)]
#[command(
    name = "quic identity",
    disable_help_flag = true,
    disable_version_flag = true
)]
#[allow(dead_code)]
struct IdentityCli {
    #[arg(long = "identity-dir")]
    identity_dir: Option<String>,
}

fn strict_validate_args(args: &[String]) -> Result<()> {
    if args.len() < 2 {
        return Ok(());
    }
    let command = args[1].as_str();
    let parse_result = match command {
        "pipe" => {
            let mut argv = vec!["quic pipe".to_string()];
            argv.extend_from_slice(&args[2..]);
            PipeCli::try_parse_from(argv).map(|_| ())
        }
        "daemon" => {
            let mut argv = vec!["quic daemon".to_string()];
            argv.extend_from_slice(&args[2..]);
            DaemonCli::try_parse_from(argv).map(|_| ())
        }
        "identity" => {
            let mut argv = vec!["quic identity".to_string()];
            argv.extend_from_slice(&args[2..]);
            IdentityCli::try_parse_from(argv).map(|_| ())
        }
        "direct" | "--direct" => {
            let mut argv = vec!["quic direct".to_string()];
            argv.extend_from_slice(&args[2..]);
            DirectCli::try_parse_from(argv).map(|_| ())
        }
        _ => TransferCli::try_parse_from(args).map(|_| ()),
    };
    parse_result
        .map(|_| ())
        .map_err(|error| anyhow::anyhow!(error.to_string()))
}

impl OutputOptions {
    fn from_args(args: &[String]) -> Result<Self> {
        let mut transport = TransportPreference::Auto;
        let mut stun_server = None;
        let mut quic_host = None;
        let mut quic_port = None;
        let mut ssh_port = None;
        let mut ssh_identity = None;

        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
                "--transport" => {
                    if i + 1 < args.len() {
                        transport = match args[i + 1].as_str() {
                            "auto" => TransportPreference::Auto,
                            "direct" => TransportPreference::Direct,
                            "stun" => TransportPreference::Stun,
                            "manual" => TransportPreference::Manual,
                            "ssh" => TransportPreference::Ssh,
                            _ => anyhow::bail!("Invalid transport option"),
                        };
                        i += 1;
                    }
                }
                "--stun-server" => {
                    if i + 1 < args.len() {
                        stun_server =
                            Some(args[i + 1].parse().context("Invalid STUN server address")?);
                        i += 1;
                    }
                }
                "--quic-host" if i + 1 < args.len() => {
                    quic_host = Some(args[i + 1].clone());
                    i += 1;
                }
                "--quic-port" if i + 1 < args.len() => {
                    quic_port = Some(args[i + 1].parse().context("Invalid QUIC port")?);
                    i += 1;
                }
                "-p" | "--port" if i + 1 < args.len() => {
                    ssh_port = Some(args[i + 1].parse().context("Invalid SSH port")?);
                    i += 1;
                }
                "-i" | "--identity" if i + 1 < args.len() => {
                    ssh_identity = Some(args[i + 1].clone());
                    i += 1;
                }
                _ => {}
            }
            i += 1;
        }

        let options = Self {
            quiet: args.iter().any(|arg| arg == "--quiet" || arg == "-q"),
            json: args.iter().any(|arg| arg == "--json"),
            verbose: args.iter().any(|arg| arg == "--verbose"),
            no_progress: args.iter().any(|arg| arg == "--no-progress"),
            resume: args.iter().any(|arg| arg == "-c" || arg == "--resume"),
            transport,
            stun_server,
            quic_host,
            quic_port,
            ssh_port,
            ssh_identity,
        };

        if options.quiet && options.verbose {
            anyhow::bail!("Cannot specify both --quiet and --verbose");
        }

        Ok(options)
    }
}

fn configured_stun_server(output: &OutputOptions) -> Result<Option<SocketAddr>> {
    if matches!(
        output.transport,
        TransportPreference::Direct | TransportPreference::Manual | TransportPreference::Ssh
    ) {
        return Ok(None);
    }

    if let Some(server) = output.stun_server {
        return Ok(Some(server));
    }

    let configured = env::var("QUICZILLA_STUN_SERVER")
        .ok()
        .or_else(|| COMPILED_DEFAULT_STUN_SERVER.map(str::to_owned));
    configured
        .filter(|server| !server.trim().is_empty())
        .map(|server| {
            server
                .parse()
                .context("Invalid configured STUN server address")
        })
        .transpose()
}

fn resolve_quic_address(host: &str, port: u16) -> Result<SocketAddr> {
    let endpoint = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    };
    endpoint
        .to_socket_addrs()
        .with_context(|| format!("Failed to resolve QUIC host '{host}'"))?
        .next()
        .with_context(|| format!("No IP address resolved for QUIC host '{host}'"))
}

async fn connect_quic_candidate(
    output: &OutputOptions,
    engine: &MsQuicEngine,
    config: &MsQuicConfiguration,
    remote_thumbprint: &str,
    direct_addr: SocketAddr,
    stun_addr: Option<SocketAddr>,
    stun_bind_addr: Option<SocketAddr>,
) -> Result<(MsQuicConnection, &'static str)> {
    match output.transport {
        TransportPreference::Direct => Ok((
            connect_client(
                engine,
                config,
                direct_addr,
                None,
                remote_thumbprint.to_string(),
            )
            .await?,
            "direct-quic",
        )),
        TransportPreference::Manual => Ok((
            connect_client(
                engine,
                config,
                direct_addr,
                None,
                remote_thumbprint.to_string(),
            )
            .await?,
            "manual-quic",
        )),
        TransportPreference::Stun => {
            let address =
                stun_addr.context("Remote worker did not report a STUN public candidate")?;
            Ok((
                connect_client(
                    engine,
                    config,
                    address,
                    stun_bind_addr,
                    remote_thumbprint.to_string(),
                )
                .await?,
                "stun-quic",
            ))
        }
        TransportPreference::Ssh => unreachable!("SSH transfers bypass QUIC candidate selection"),
        TransportPreference::Auto => {
            let Some(stun_addr) = stun_addr else {
                return Ok((
                    connect_client(
                        engine,
                        config,
                        direct_addr,
                        None,
                        remote_thumbprint.to_string(),
                    )
                    .await?,
                    "direct-quic",
                ));
            };

            // Give a LAN, VPN, mesh, or directly routed hostname a brief lead.
            // The STUN-reflexive candidate then races independently using its
            // mapped local UDP port, so one candidate cannot monopolize the path.
            let direct = connect_client(
                engine,
                config,
                direct_addr,
                None,
                remote_thumbprint.to_string(),
            );
            let stun = connect_client(
                engine,
                config,
                stun_addr,
                stun_bind_addr,
                remote_thumbprint.to_string(),
            );
            tokio::pin!(direct);
            tokio::pin!(stun);
            let head_start = tokio::time::sleep(DIRECT_CANDIDATE_HEAD_START);
            tokio::pin!(head_start);
            let mut direct_pending = true;
            let mut stun_pending = false;
            let mut direct_error: Option<anyhow::Error> = None;
            let mut stun_error: Option<anyhow::Error> = None;

            loop {
                tokio::select! {
                    result = &mut direct, if direct_pending => {
                        direct_pending = false;
                        match result {
                            Ok(connection) => return Ok((connection, "direct-quic")),
                            Err(error) => {
                                direct_error = Some(error);
                                stun_pending = true;
                            }
                        }
                    }
                    _ = &mut head_start, if !stun_pending => {
                        stun_pending = true;
                    }
                    result = &mut stun, if stun_pending => {
                        stun_pending = false;
                        match result {
                            Ok(connection) => return Ok((connection, "stun-quic")),
                            Err(error) => stun_error = Some(error),
                        }
                    }
                }

                if !direct_pending && !stun_pending {
                    let direct = direct_error
                        .as_ref()
                        .map(ToString::to_string)
                        .unwrap_or_else(|| "not attempted".to_string());
                    let stun = stun_error
                        .as_ref()
                        .map(ToString::to_string)
                        .unwrap_or_else(|| "not attempted".to_string());
                    anyhow::bail!(
                        "direct QUIC candidate failed: {direct}; STUN candidate failed: {stun}"
                    );
                }
            }
        }
    }
}

#[derive(Clone, Copy)]
enum RemotePlatform {
    LinuxX86_64,
    WindowsX86_64,
}

struct WorkerBootstrap {
    child: std::process::Child,
    udp_port: u16,
    remote_thumbprint: String,
    host: String,
    public_udp_addr: Option<std::net::SocketAddr>,
}

#[tokio::main]
async fn main() {
    let json_requested = env::args().any(|argument| argument == "--json");
    let code = match run().await {
        Ok(()) => 0,
        Err(error) => {
            if json_requested {
                let receipt = serde_json::json!({
                    "status": "failed",
                    "exit_code": error.code,
                    "error": error.message,
                });
                println!("{receipt}");
            } else {
                eprintln!("Error: {error}");
            }
            error.code
        }
    };
    std::process::exit(code);
}

async fn run() -> CliResult<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let args: Vec<String> = env::args().collect();

    if args.len() > 2 && args[2..].iter().any(|a| a == "--help" || a == "-h") {
        match args[1].as_str() {
            "daemon" => print_daemon_usage(),
            "direct" | "--direct" => print_direct_usage(),
            "identity" => print_identity_usage(),
            "pipe" => print_pipe_usage(),
            _ => print_usage(),
        }
        return Ok(());
    }

    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_usage();
        return Ok(());
    }

    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("quiczilla {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    if args.len() < 2 {
        print_usage();
        return Err(CliFailure::new(
            2,
            "a command or transfer arguments are required",
        ));
    }
    if let Err(error) = strict_validate_args(&args) {
        return Err(CliFailure::new(2, error.to_string()));
    }

    let output = OutputOptions::from_args(&args)?;
    tracing_subscriber::fmt()
        .with_max_level(if output.verbose {
            tracing::Level::DEBUG
        } else {
            tracing::Level::WARN
        })
        .with_target(false)
        .without_time()
        .init();
    // Subcommand: pipe
    if args[1] == "pipe" {
        return run_pipe_cli(&args[2..], output).await.map_err(Into::into);
    }

    // Subcommand: daemon
    if args[1] == "daemon" {
        return run_daemon_cli(&args[2..]).await.map_err(Into::into);
    }

    // Subcommand: persistent client identity
    if args[1] == "identity" {
        return run_identity_cli(&args[2..]).await.map_err(Into::into);
    }

    // Subcommand: direct transfer
    if args[1] == "direct" || args[1] == "--direct" {
        return run_direct_cli(&args[2..], output).await;
    }

    // Default: file transfer (quiczilla <local_file> <user@host:path>)
    run_file_transfer_cli(&args[1..], output).await
}

fn print_usage() {
    println!("Quiczilla - High-Throughput Encrypted P2P Transfer (QUIC + mTLS)\n");
    println!("Usage:");
    println!(
        "  quic <local_file> <user@host:path> [-c|--resume] [--checksum] [-i|--identity <key>]"
    );
    println!("       [-q|--quiet|--no-progress|--verbose] [-p|--port <ssh-port>]");
    println!("       [--transport auto|direct|stun|manual|ssh] [--stun-server <host:port>]");
    println!("       [--quic-host <host-or-ip>] [--quic-port <port>]\n");
    println!("    Transfer a file with SSH bootstrap, remote userspace caching, and QUIC speed.\n");
    println!("  quic pipe <user@host> [\"<remote_command>\"] [--checksum] [-i|--identity <key>]\n");
    println!("    Universal raw stream pipe (e.g. ZFS send/receive, tar, pg_dump, ffmpeg).\n");
    println!("  quic daemon [--port <port>] [--allow-thumbprints <path>] [--save-dir <dir>]\n");
    println!(
        "    Start a persistent always-on receiver (for WireGuard/Tailscale mesh or static IP).\n"
    );
    println!("  quic identity [--identity-dir <dir>]\n");
    println!("    Create or display this client's persistent mTLS thumbprint.\n");
    println!(
        "  quic direct <host:port> <local_file> --thumbprint <server-thumbprint> [--checksum] [-c]\n"
    );
    println!("    Transfer directly to an authorized persistent daemon without SSH.\n");
    println!("Options:");
    println!("  -c, --resume            Resume an interrupted transfer (>= 20 MB)");
    println!("      --checksum          Calculate and verify SHA-256 integrity hash");
    println!("  -i, --identity <path>   Identity/private key file for SSH authentication");
    println!("  -p, --port <port>       SSH port on the remote target");
    println!(
        "  -q, --quiet             Quiet mode (suppress progress bar and informational messages)"
    );
    println!("      --no-progress       Disable live interactive progress bar");
    println!("      --json              Emit one machine-readable success receipt");
    println!("      --verbose           Show detailed bootstrap, network, and debug logs");
    println!("  -h, --help              Show this help message and exit");
    println!("  -V, --version           Print version information and exit\n");
    println!("Examples:");
    println!("  quic dataset.tar.gz ops@receiver.example.net:/srv/incoming/");
    println!("  quic -c -i ~/.ssh/id_ed25519 disk.iso user@remote:/data/");
    println!(
        "  zfs send tank/data@snap | quic pipe ops@receiver.example.net \"zfs receive backup/data\""
    );
    println!("  tar czf - /var/data | quic pipe user@remote \"tar xzf - -C /backup\"");
}

fn print_daemon_usage() {
    println!(
        "Usage: quic daemon --allow-thumbprints <file> [--port <udp-port>] [--save-dir <dir>] [--identity-dir <dir>] [--on-conflict overwrite|refuse]"
    );
    println!();
    println!("Start a persistent mTLS receiver in the foreground.");
    println!("The allow-list contains one authorized client SHA-1 thumbprint per line.");
    println!(
        "Default UDP port: 55441. Existing files are overwritten unless --on-conflict refuse is set."
    );
}

fn print_direct_usage() {
    println!(
        "Usage: quic direct <host:port> <local_file> --thumbprint <server-thumbprint> [--checksum] [--resume] [--identity-dir <dir>] [--no-progress] [--json]"
    );
    println!();
    println!("Send one file to a persistent daemon without SSH.");
    println!("Both the client identity and the server thumbprint must be enrolled first.");
}

fn print_identity_usage() {
    println!("Usage: quic identity [--identity-dir <dir>]");
    println!();
    println!("Create or display the persistent client mTLS thumbprint.");
    println!("The thumbprint is printed on stdout for use in the daemon allow-list.");
}

fn print_pipe_usage() {
    println!(
        "Usage: quic pipe <user@host> [\"<remote_command>\"] [--checksum] [-q|--quiet] [--no-progress] [-i|--identity <ssh-key>]"
    );
    println!();
    println!("Stream stdin/stdout through an SSH-bootstrapped encrypted QUIC connection.");
}

async fn run_file_transfer_cli(args: &[String], output: OutputOptions) -> CliResult<()> {
    if args.len() < 2 {
        print_usage();
        return Err(CliFailure::new(2, "two transfer arguments are required"));
    }

    let local_file_str = &args[0];
    let remote_target = &args[1];
    let checksum_enabled = args.iter().any(|a| a == "--checksum" || a == "--verify");

    let local_path = Path::new(local_file_str);
    if !local_path.exists() {
        return Err(anyhow::anyhow!("Local file does not exist: {}", local_file_str).into());
    }
    let file_metadata = std::fs::metadata(local_path)
        .with_context(|| format!("Failed to read metadata for {}", local_file_str))?;
    let file_size_bytes = file_metadata.len();
    let file_name = local_path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let parts: Vec<&str> = remote_target.splitn(2, ':').collect();
    if parts.len() != 2 {
        return Err(anyhow::anyhow!(
            "Invalid remote target '{}'. Expected user@host:path",
            remote_target
        )
        .into());
    }

    let ssh_target = parts[0];
    let remote_path = parts[1];
    let mut renderer = ProgressRenderer::new(output.quiet || output.json, output.no_progress);
    renderer.status(&format!("Connecting to {ssh_target}…"));

    // 1. Generate local ephemeral mTLS certificate
    let engine = MsQuicEngine::new()?;
    let (local_thumbprint, config) = build_quic_config(&engine, false)?;

    if output.transport == TransportPreference::Ssh {
        return Ok(run_ssh_fallback(
            local_path,
            ssh_target,
            remote_path,
            &file_name,
            file_size_bytes,
            checksum_enabled,
            output,
            &mut renderer,
        )?);
    }

    // Gather a server-reflexive candidate when STUN is configured. In auto
    // mode a failed STUN lookup does not prevent the independently routable
    // hostname/LAN/mesh candidate from being attempted.
    let stun_server = configured_stun_server(&output)?;
    let (local_udp_port, local_public_candidate, local_stun_bind_addr) = match stun_server {
        Some(server) => match quiczilla_core::stun::discover(server, 0) {
            Ok(candidate) => {
                if output.verbose {
                    renderer.status(&format!("STUN public candidate: {}", candidate.public_addr));
                }
                let bind_addr = candidate
                    .local_addr
                    .map(|ip| std::net::SocketAddr::new(ip, candidate.local_port));
                (candidate.local_port, Some(candidate.public_addr), bind_addr)
            }
            Err(_) if output.transport == TransportPreference::Auto => {
                renderer.status("STUN discovery unavailable; continuing with direct QUIC…");
                let socket = std::net::UdpSocket::bind("0.0.0.0:0")?;
                let port = socket.local_addr()?.port();
                drop(socket);
                (port, None, None)
            }
            Err(error) => {
                return Err(error
                    .context("STUN transport requested but discovery failed")
                    .into());
            }
        },
        None if output.transport == TransportPreference::Stun => {
            return Err(
                anyhow::anyhow!(
                    "--transport stun requires --stun-server, QUICZILLA_STUN_SERVER, or a compiled default"
                )
                .into(),
            );
        }
        None => {
            let local_bind_socket = std::net::UdpSocket::bind("0.0.0.0:0")?;
            let port = local_bind_socket.local_addr()?.port();
            drop(local_bind_socket);
            (port, None, None)
        }
    };

    // 2. Bootstrap remote worker
    let mut worker_args = vec![
        "--peer-thumbprint".to_string(),
        local_thumbprint.clone(),
        "--save-dir".to_string(),
        remote_path.to_string(),
    ];
    if checksum_enabled {
        worker_args.push("--checksum".to_string());
    }
    if output.resume {
        worker_args.push("--resume".to_string());
    }
    if let Some(candidate) = local_public_candidate {
        worker_args.push("--punch-ip".to_string());
        worker_args.push(candidate.ip().to_string());
        worker_args.push("--punch-port".to_string());
        worker_args.push(candidate.port().to_string());
    } else {
        worker_args.push("--punch-port".to_string());
        worker_args.push(local_udp_port.to_string());
    }
    if let Some(server) = stun_server {
        worker_args.push("--stun-server".to_string());
        worker_args.push(server.to_string());
    }
    if let Some(port) = output.quic_port {
        worker_args.push("--port".to_string());
        worker_args.push(port.to_string());
    } else if output.transport == TransportPreference::Manual {
        return Err(anyhow::anyhow!("--transport manual requires --quic-port <port>").into());
    }

    let mut bootstrap = match bootstrap_remote_worker(
        ssh_target,
        &worker_args,
        &local_thumbprint,
        output.verbose,
        output.ssh_port,
        output.ssh_identity.as_deref(),
    ) {
        Ok(bootstrap) => bootstrap,
        Err(error) => {
            renderer.failed(&error.to_string());
            return Err(error.into());
        }
    };

    // Resolve both independently routable and STUN-reflexive candidates.
    // The explicit quic-host option lets users prefer a LAN, VPN, mesh, DNS,
    // or known public endpoint without changing SSH bootstrap.
    let direct_host = output.quic_host.as_deref().unwrap_or(&bootstrap.host);
    let direct_addr = resolve_quic_address(direct_host, bootstrap.udp_port)?;

    renderer.status("Establishing encrypted transfer…");

    let (connection, selected_transport) = match connect_quic_candidate(
        &output,
        &engine,
        &config,
        &bootstrap.remote_thumbprint,
        direct_addr,
        bootstrap.public_udp_addr,
        local_stun_bind_addr,
    )
    .await
    {
        Ok(selected) => selected,
        Err(_) if output.transport == TransportPreference::Auto => {
            renderer.status("UDP candidates failed; falling back to SSH transfer…");
            let _ = bootstrap.child.kill();
            return Ok(run_ssh_fallback(
                local_path,
                ssh_target,
                remote_path,
                &file_name,
                file_size_bytes,
                checksum_enabled,
                output,
                &mut renderer,
            )?);
        }
        Err(error) => return Err(error.into()),
    };

    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let (command_tx, command_rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();

    let cancel_clone = cancel.clone();
    let connect_start = Instant::now();
    let mut transfer_start = Instant::now();
    let mut transfer_completed = false;
    let local_file_clone = local_file_str.to_string();

    let event_tx_err = event_tx.clone();
    let _direct_task = tokio::spawn(async move {
        if let Err(e) = start_connected_client(connection, event_tx, command_rx, cancel_clone).await
        {
            let _ = event_tx_err.send(PeerEvent::Disconnected {
                reason: format!("{:?}", e),
            });
        }
    });

    let (key_tx, mut key_rx) = mpsc::unbounded_channel::<crossterm::event::KeyEvent>();
    let cancel_keys = cancel.clone();
    let interactive_keyboard =
        !output.quiet && !output.no_progress && std::io::stdin().is_terminal();
    let mut raw_guard = if interactive_keyboard {
        Some(RawModeGuard::new())
    } else {
        None
    };

    if raw_guard.is_some() {
        tokio::task::spawn_blocking(move || {
            while !cancel_keys.is_cancelled() {
                if let Ok(true) = crossterm::event::poll(Duration::from_millis(100)) {
                    if let Ok(crossterm::event::Event::Key(key)) = crossterm::event::read() {
                        if key.kind == crossterm::event::KeyEventKind::Press
                            && key_tx.send(key).is_err()
                        {
                            break;
                        }
                    }
                }
            }
        });
    }

    let mut is_paused = false;
    let mut last_prog: Option<quiczilla_core::types::ProgressInfo> = None;
    let mut failure: Option<CliFailure> = None;
    let mut fallback_allowed = false;

    loop {
        tokio::select! {
            Some(key) = key_rx.recv() => {
                match key.code {
                    crossterm::event::KeyCode::Char(' ') | crossterm::event::KeyCode::Char('p') | crossterm::event::KeyCode::Char('P') => {
                        if is_paused {
                            let _ = command_tx.send(PeerCommand::Resume);
                        } else {
                            let _ = command_tx.send(PeerCommand::Pause);
                        }
                    }
                    crossterm::event::KeyCode::Char('q') | crossterm::event::KeyCode::Char('Q') => {
                        renderer.failed("Transfer cancelled by user.");
                        failure = Some(CliFailure::new(130, "transfer cancelled by user"));
                        cancel.cancel();
                        break;
                    }
                    crossterm::event::KeyCode::Char('c') if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) => {
                        renderer.failed("Transfer cancelled by user.");
                        failure = Some(CliFailure::new(130, "transfer cancelled by user"));
                        cancel.cancel();
                        break;
                    }
                    _ => {}
                }
            }
            _ = tokio::signal::ctrl_c() => {
                renderer.failed("Transfer cancelled by user.");
                failure = Some(CliFailure::new(130, "transfer cancelled by user"));
                cancel.cancel();
                break;
            }
            event = event_rx.recv() => {
                let Some(event) = event else { break };
                match event {
                    PeerEvent::Connected => {
                        let _handshake_duration = connect_start.elapsed();
                        transfer_start = Instant::now();
                        renderer.status(&format!("Transferring {file_name}…"));
                        match command_tx.send(PeerCommand::SendFile {
                            path: local_file_clone.clone(),
                            checksum: checksum_enabled,
                            resume: output.resume,
                        }) {
                            Ok(_) => {}
                            Err(error) => {
                                renderer.failed(&format!("could not start transfer: {error}"));
                                failure = Some(CliFailure::new(
                                    1,
                                    format!("could not start transfer: {error}"),
                                ));
                                cancel.cancel();
                                break;
                            }
                        }
                    }
                    PeerEvent::TransferProgress(prog) => {
                        last_prog = Some(prog.clone());
                        if !is_paused {
                            renderer.progress(&file_name, &prog);
                        }
                    }
                    PeerEvent::TransferPaused => {
                        is_paused = true;
                        renderer.paused(&file_name, last_prog.as_ref());
                    }
                    PeerEvent::TransferResumed => {
                        is_paused = false;
                        renderer.resume();
                        if let Some(ref prog) = last_prog {
                            renderer.progress(&file_name, prog);
                        }
                    }
                    PeerEvent::TransferComplete { status } => {
                        let transfer_duration = transfer_start.elapsed();

                        if status == FileTransferStatus::Completed {
                            transfer_completed = true;
                            renderer.completed(&file_name, file_size_bytes, transfer_duration, checksum_enabled);
                            emit_json_receipt(
                                output.json,
                                &file_name,
                                file_size_bytes,
                                transfer_duration,
                                selected_transport,
                                checksum_enabled,
                            );
                        } else {
                            transfer_completed = false;
                            renderer.failed(&format!("{:?}", status));
                            failure = Some(CliFailure::new(1, format!("transfer failed: {status:?}")));
                        }

                        cancel.cancel();
                        break;
                    }
                    PeerEvent::Disconnected { reason } => {
                        if !transfer_completed {
                            renderer.failed(&reason);
                            fallback_allowed = output.transport == TransportPreference::Auto;
                            failure = Some(CliFailure::new(1, reason));
                            cancel.cancel();
                        }
                        break;
                    }
                    _ => {}
                }
            }
        }
    }
    drop(raw_guard.take());
    let _ = bootstrap.child.kill();

    if transfer_completed {
        // The transfer result has already been received and rendered. MsQuic
        // owns native callback threads whose teardown can block indefinitely
        // after a one-shot worker has completed, leaving an otherwise
        // successful CLI invocation at 100%. Flush receipts before taking the
        // process-terminal path used by the direct daemon client as well.
        let _ = std::io::stdout().flush();
        let _ = std::io::stderr().flush();
        std::process::exit(0);
    } else if fallback_allowed {
        renderer.status("UDP transport failed or timed out; falling back to SSH transfer…");
        run_ssh_fallback(
            local_path,
            ssh_target,
            remote_path,
            &file_name,
            file_size_bytes,
            checksum_enabled,
            output,
            &mut renderer,
        )?;
        Ok(())
    } else if let Some(error) = failure {
        Err(error)
    } else {
        Err(CliFailure::new(1, "transfer ended before completion"))
    }
}

async fn run_pipe_cli(args: &[String], output: OutputOptions) -> Result<()> {
    if args.is_empty() {
        eprintln!("Usage: quiczilla pipe <user@host> [\"<remote_command>\"] [--verify]");
        std::process::exit(1);
    }

    let ssh_target = &args[0];
    let mut exec_cmd: Option<String> = None;
    let mut verify = false;

    for arg in &args[1..] {
        if arg == "--verify" || arg == "--checksum" {
            verify = true;
        } else if matches!(
            arg.as_str(),
            "-q" | "--quiet" | "--verbose" | "--no-progress"
        ) {
            continue;
        } else if exec_cmd.is_none() {
            exec_cmd = Some(arg.clone());
        }
    }

    let start_total = Instant::now();

    // 1. Ephemeral mTLS cert
    let engine = MsQuicEngine::new()?;
    let (local_thumbprint, config) = build_quic_config(&engine, false)?;
    eprintln!(
        "[quiczilla pipe] Generated ephemeral mTLS cert (Thumbprint: {})",
        local_thumbprint
    );

    // Pick an available local UDP port for NAT hole-punching
    let local_bind_socket = std::net::UdpSocket::bind("0.0.0.0:0")?;
    let local_udp_port = local_bind_socket.local_addr()?.port();
    drop(local_bind_socket);

    // 2. Build worker arguments for pipe mode
    let mut worker_args = vec![
        "--pipe".to_string(),
        "--peer-thumbprint".to_string(),
        local_thumbprint.clone(),
        "--punch-port".to_string(),
        local_udp_port.to_string(),
    ];
    if let Some(ref cmd) = exec_cmd {
        worker_args.push("--exec-hex".to_string());
        worker_args.push(hex::encode(cmd.as_bytes()));
    }
    if verify {
        worker_args.push("--verify".to_string());
    }

    let mut bootstrap = bootstrap_remote_worker(
        ssh_target,
        &worker_args,
        &local_thumbprint,
        output.verbose,
        output.ssh_port,
        output.ssh_identity.as_deref(),
    )?;

    // 3. Connect via QUIC
    let host_with_port = format!("{}:{}", bootstrap.host, bootstrap.udp_port);
    let remote_addr: SocketAddr = host_with_port
        .to_socket_addrs()
        .with_context(|| format!("Failed to resolve hostname '{}'", bootstrap.host))?
        .next()
        .with_context(|| format!("No IP address resolved for '{}'", bootstrap.host))?;

    eprintln!("[quiczilla pipe] Dialing QUIC tunnel to {}...", remote_addr);
    let connect_start = Instant::now();

    use quiczilla_core::msquic::engine::MsQuicConnection;
    let connection = MsQuicConnection::connect(
        &engine,
        &config,
        remote_addr,
        None,
        Some(bootstrap.remote_thumbprint.clone()),
    )
    .await?;
    let connect_ms = connect_start.elapsed().as_secs_f64() * 1000.0;
    eprintln!(
        "[quiczilla pipe] QUIC mTLS Connected in {:.2}ms! Streaming data...",
        connect_ms
    );

    let stream = connection.open_stream().await?;
    let recv_stream = stream.recv;
    let mut send_stream = stream.send;
    send_stream.write_all(&[PIPE_STREAM_HEADER]).await?;

    let cancel = CancellationToken::new();
    let pipe_start = Instant::now();

    let result = run_pipe(
        send_stream,
        recv_stream,
        Some(tokio::io::stdin()),
        Some(tokio::io::stdout()),
        verify,
        cancel,
    )
    .await?;

    let stream_duration = pipe_start.elapsed().as_secs_f64().max(0.001);
    let mb_sent = result.bytes_sent as f64 / 1_048_576.0;
    let mb_recv = result.bytes_received as f64 / 1_048_576.0;
    let total_mb = mb_sent + mb_recv;
    let speed_mb_s = total_mb / stream_duration;

    eprintln!("\n============================================================");
    eprintln!("[quiczilla pipe] STREAM COMPLETED SUCCESSFULLY!");
    eprintln!("Bytes Sent     : {} ({:.2} MB)", result.bytes_sent, mb_sent);
    eprintln!(
        "Bytes Received : {} ({:.2} MB)",
        result.bytes_received, mb_recv
    );
    eprintln!("Stream Duration: {:.2}s", stream_duration);
    eprintln!("Average Speed  : {:.2} MB/s", speed_mb_s);
    eprintln!(
        "Total Elapsed  : {:.2}s",
        start_total.elapsed().as_secs_f64()
    );
    if verify {
        if let Some(ref h) = result.sent_hash {
            eprintln!("SHA-256 (Sent)    : {}", h);
        }
        if let Some(ref h) = result.received_hash {
            eprintln!("SHA-256 (Received): {}", h);
        }
    }
    eprintln!("============================================================");

    let _ = bootstrap.child.kill();
    let _ = bootstrap.child.wait();

    std::process::exit(0);
}

async fn run_daemon_cli(args: &[String]) -> Result<()> {
    let mut worker_cmd = vec!["--daemon".to_string()];
    worker_cmd.extend_from_slice(args);
    eprintln!("[quiczilla daemon] Starting persistent receiver");

    let worker_name = if cfg!(windows) {
        "quiczilla-worker.exe"
    } else {
        "quiczilla-worker"
    };
    if let Ok(current_exe) = std::env::current_exe() {
        if let Some(directory) = current_exe.parent() {
            let sibling = directory.join(worker_name);
            if sibling.is_file() {
                let status = Command::new(sibling).args(&worker_cmd).status()?;
                std::process::exit(status.code().unwrap_or(1));
            }
        }
    }

    if let Ok(status) = Command::new(worker_name).args(&worker_cmd).status() {
        std::process::exit(status.code().unwrap_or(1));
    }

    // Source-built CLIs may not have the packaged sibling worker. Extract a
    // complete worker/runtime pair so native loading does not depend on .NET.
    let bundle_dir = std::env::temp_dir().join(format!("quiczilla-daemon-{WORKER_VERSION}"));
    std::fs::create_dir_all(&bundle_dir)?;
    let worker_path = bundle_dir.join(worker_name);

    #[cfg(windows)]
    {
        std::fs::write(&worker_path, WORKER_WINDOWS_X86_64)?;
        let runtime = MSQUIC_WINDOWS_X86_64
            .context("this CLI was built without an embedded Windows MsQuic runtime")?;
        std::fs::write(bundle_dir.join("msquic.dll"), runtime)?;
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(&worker_path, WORKER_LINUX_X86_64)?;
        let runtime = MSQUIC_LINUX_X86_64
            .context("this CLI was built without an embedded Linux MsQuic runtime")?;
        std::fs::write(bundle_dir.join("libmsquic.so"), runtime)?;
        std::fs::set_permissions(&worker_path, std::fs::Permissions::from_mode(0o755))?;
    }

    let status = Command::new(worker_path).args(&worker_cmd).status()?;
    std::process::exit(status.code().unwrap_or(1));
}

fn option_value(args: &[String], name: &str) -> Option<String> {
    args.windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].clone())
}

async fn run_identity_cli(args: &[String]) -> Result<()> {
    let identity_dir = option_value(args, "--identity-dir")
        .map(std::path::PathBuf::from)
        .map(Ok)
        .unwrap_or_else(|| default_identity_dir("client"))?;
    let engine = MsQuicEngine::new()?;
    let (thumbprint, _config) = build_persistent_quic_config(&engine, false, &identity_dir)?;
    eprintln!("Persistent client identity: {}", identity_dir.display());
    println!("{thumbprint}");
    Ok(())
}

async fn run_direct_cli(args: &[String], output: OutputOptions) -> CliResult<()> {
    if args.len() < 2 {
        return Err(anyhow::anyhow!(
            "usage: quic direct <host:port> <local_file> --thumbprint <server-thumbprint> [--checksum] [-c]"
        )
        .into());
    }
    let remote_addr = args[0]
        .to_socket_addrs()
        .with_context(|| format!("failed to resolve daemon address {}", args[0]))?
        .next()
        .with_context(|| format!("no address resolved for {}", args[0]))?;
    let local_path = Path::new(&args[1]);
    let metadata = std::fs::metadata(local_path)
        .with_context(|| format!("failed to read local file {}", local_path.display()))?;
    if !metadata.is_file() {
        return Err(anyhow::anyhow!("local path is not a file: {}", local_path.display()).into());
    }
    let file_name = local_path
        .file_name()
        .context("local file has no file name")?
        .to_string_lossy()
        .into_owned();
    let expected_server_thumbprint = normalize_thumbprint(
        &option_value(args, "--thumbprint")
            .context("--thumbprint <server-thumbprint> is required for direct mode")?,
    )?;
    let checksum = args
        .iter()
        .any(|arg| arg == "--checksum" || arg == "--verify");
    let identity_dir = option_value(args, "--identity-dir")
        .map(std::path::PathBuf::from)
        .map(Ok)
        .unwrap_or_else(|| default_identity_dir("client"))?;

    let mut renderer = ProgressRenderer::new(output.quiet || output.json, output.no_progress);
    renderer.status(&format!("Connecting directly to {remote_addr}…"));

    let engine = MsQuicEngine::new()?;
    let (_client_thumbprint, config) = build_persistent_quic_config(&engine, false, &identity_dir)?;
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let (command_tx, command_rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    let cancel_client = cancel.clone();
    let error_tx = event_tx.clone();
    tokio::spawn(async move {
        if let Err(error) = start_client(
            remote_addr,
            None,
            remote_addr.is_ipv6(),
            expected_server_thumbprint,
            &engine,
            &config,
            event_tx,
            command_rx,
            cancel_client,
        )
        .await
        {
            let _ = error_tx.send(PeerEvent::Disconnected {
                reason: error.to_string(),
            });
        }
    });

    let local_file = local_path.to_string_lossy().into_owned();
    let mut transfer_start = Instant::now();
    let mut completed = false;
    let mut failure: Option<CliFailure> = None;
    let idle_timeout = tokio::time::sleep(Duration::from_secs(15));
    tokio::pin!(idle_timeout);
    loop {
        tokio::select! {
            _ = &mut idle_timeout => {
                renderer.failed("direct daemon did not respond for 15 seconds");
                failure = Some(CliFailure::new(
                    1,
                    "direct daemon did not respond for 15 seconds",
                ));
                cancel.cancel();
                break;
            }
            _ = tokio::signal::ctrl_c() => {
                failure = Some(CliFailure::new(130, "transfer cancelled by user"));
                cancel.cancel();
                break;
            }
            event = event_rx.recv() => {
                let Some(event) = event else { break };
                idle_timeout.as_mut().reset(
                    tokio::time::Instant::now() + Duration::from_secs(15)
                );
                match event {
                    PeerEvent::Connected => {
                        transfer_start = Instant::now();
                        renderer.status(&format!("Transferring {file_name}…"));
                        command_tx.send(PeerCommand::SendFile {
                            path: local_file.clone(),
                            checksum,
                            resume: output.resume,
                        })
                        .map_err(|error| {
                            CliFailure::new(1, format!("could not start transfer: {error}"))
                        })?;
                    }
                    PeerEvent::TransferProgress(progress) => {
                        renderer.progress(&file_name, &progress);
                    }
                    PeerEvent::TransferComplete { status: FileTransferStatus::Completed } => {
                        completed = true;
                        renderer.completed(
                            &file_name,
                            metadata.len(),
                            transfer_start.elapsed(),
                            checksum,
                        );
                        emit_json_receipt(
                            output.json,
                            &file_name,
                            metadata.len(),
                            transfer_start.elapsed(),
                            "direct-daemon-quic",
                            checksum,
                        );
                        cancel.cancel();
                        break;
                    }
                    PeerEvent::TransferComplete { status } => {
                        renderer.failed(&format!("direct transfer failed: {status:?}"));
                        failure = Some(CliFailure::new(
                            1,
                            format!("direct transfer failed: {status:?}"),
                        ));
                        cancel.cancel();
                        break;
                    }
                    PeerEvent::Disconnected { reason } if !completed => {
                        renderer.failed(&format!("direct connection failed: {reason}"));
                        failure = Some(CliFailure::new(
                            1,
                            format!("direct connection failed: {reason}"),
                        ));
                        cancel.cancel();
                        break;
                    }
                    _ => {}
                }
            }
        }
    }

    if !completed {
        return Err(failure
            .unwrap_or_else(|| CliFailure::new(1, "direct connection closed before completion")));
    }
    // MsQuic owns background callbacks beyond Tokio task completion. Match the
    // SSH-bootstrap transfer path and let the OS close those native handles.
    Ok(())
}

fn bootstrap_remote_worker(
    ssh_target: &str,
    worker_args: &[String],
    _local_thumbprint: &str,
    verbose: bool,
    ssh_port: Option<u16>,
    ssh_identity: Option<&str>,
) -> Result<WorkerBootstrap> {
    let start_bootstrap = Instant::now();

    let host = if let Some((_, h)) = ssh_target.split_once('@') {
        h.to_string()
    } else {
        ssh_target.to_string()
    };

    let probe_output = ssh_command(ssh_port, ssh_identity)
        .arg("-o")
        .arg("BatchMode=yes")
        .arg("-o")
        .arg("StrictHostKeyChecking=no")
        .arg(ssh_target)
        .arg("uname -sm 2>/dev/null || echo Windows %PROCESSOR_ARCHITECTURE%")
        .stdin(Stdio::null())
        .output()
        .context("Failed to execute SSH probe")?;

    if !probe_output.status.success() {
        let err_msg = String::from_utf8_lossy(&probe_output.stderr);
        anyhow::bail!("SSH probe failed: {}", err_msg.trim());
    }

    let probe_str = String::from_utf8_lossy(&probe_output.stdout)
        .trim()
        .to_string();
    if verbose {
        eprintln!("Remote platform: {probe_str}");
    }

    // A source-built CLI may use a system MsQuic runtime locally but not carry
    // an embedded runtime for remote worker deployment. Keep that runtime
    // optional until we know a bundle upload is necessary: a pre-installed
    // remote worker is already self-contained and needs neither upload nor
    // embedded bytes from this CLI.
    let (platform, embedded_worker_bytes, runtime_bytes, binary_name, runtime_name) =
        if probe_str.contains("Linux") && probe_str.contains("x86_64") {
            (
                RemotePlatform::LinuxX86_64,
                WORKER_LINUX_X86_64,
                MSQUIC_LINUX_X86_64,
                format!("worker-{}", WORKER_VERSION),
                "libmsquic.so",
            )
        } else if probe_str.contains("Windows") {
            (
                RemotePlatform::WindowsX86_64,
                WORKER_WINDOWS_X86_64,
                MSQUIC_WINDOWS_X86_64,
                format!("worker-{}.exe", WORKER_VERSION),
                "msquic.dll",
            )
        } else {
            anyhow::bail!("Unsupported remote environment: '{}'", probe_str);
        };

    // `cargo build --release` places a newly built host worker beside the CLI.
    // Prefer it for same-platform source development so a checkout never
    // deploys an older tracked bootstrap artifact. Official archives contain
    // only their matching host worker beside the CLI; cross-platform targets
    // therefore continue to use the freshly embedded CI worker.
    let local_worker_bytes =
        sibling_worker_bytes(platform).unwrap_or_else(|| embedded_worker_bytes.to_vec());
    let worker_bytes = local_worker_bytes.as_slice();

    let worker_arg_str = worker_args
        .iter()
        .map(|a| {
            if a.contains(' ') {
                format!("\"{}\"", a)
            } else {
                a.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ");

    let find_installed = ssh_command(ssh_port, ssh_identity)
        .arg("-o").arg("BatchMode=yes")
        .arg("-o").arg("StrictHostKeyChecking=no")
        .arg(ssh_target)
        .arg("which quiczilla-worker 2>/dev/null || which quiczilla 2>/dev/null || which quic 2>/dev/null || [ -x ~/.local/bin/quiczilla-worker ] && echo ~/.local/bin/quiczilla-worker || [ -x ~/.local/bin/quiczilla ] && echo ~/.local/bin/quiczilla || [ -x ~/.local/bin/quic ] && echo ~/.local/bin/quic || true")
        .stdin(Stdio::null())
        .output()
        .ok();

    let pre_installed_path = find_installed.and_then(|out| {
        let s = String::from_utf8_lossy(&out.stdout);
        s.lines()
            .map(|l| l.trim().to_string())
            .find(|l| !l.is_empty() && (l.starts_with('/') || l.contains(":\\")))
    });

    let local_worker_hash = hex::encode(Sha256::digest(worker_bytes));
    let installed_worker_hash = pre_installed_path.as_deref().and_then(|path| {
        remote_worker_checksum(platform, ssh_target, path, ssh_port, ssh_identity)
    });
    let installed_worker_matches = installed_worker_hash
        .as_deref()
        .is_some_and(|hash| hash.eq_ignore_ascii_case(&local_worker_hash));

    let mut used_managed_worker = !installed_worker_matches;
    let mut child = if installed_worker_matches {
        let path = pre_installed_path
            .as_deref()
            .expect("a matching worker checksum requires a worker path");
        if verbose {
            eprintln!("Using checksum-matched installed worker: {path}");
        }
        run_installed_worker(
            ssh_target,
            path,
            &worker_arg_str,
            verbose,
            ssh_port,
            ssh_identity,
        )?
    } else {
        if let Some(path) = pre_installed_path.as_deref() {
            let detail = installed_worker_hash
                .as_deref()
                .map(|hash| format!("checksum {hash}"))
                .unwrap_or_else(|| "checksum unavailable".to_string());
            eprintln!(
                "Installed remote worker at {path} differs from this client ({detail}); refreshing managed worker bundle."
            );
        }
        let runtime_bytes = runtime_bytes.context(
            "This source-built CLI has no embedded runtime for remote deployment. Install quiczilla-worker on the remote host, or use an official release artifact.",
        )?;
        let mut hasher = Sha256::new();
        hasher.update(worker_bytes);
        hasher.update(runtime_bytes);
        let bundle_hash = hex::encode(hasher.finalize()).to_lowercase();
        // A content-addressed directory avoids replacing an executable that a
        // previous transfer is still running (Linux rejects that with ETXTBSY).
        // It also makes a failed or interrupted refresh harmless: no existing
        // complete bundle is modified.
        let bundle_dir = format!("bundle-{bundle_hash}");
        let marker_name = "bundle.sha256";

        let cache_hit = match platform {
            RemotePlatform::LinuxX86_64 => {
                let check_cmd =
                    format!("cat ~/.cache/quiczilla/{bundle_dir}/{marker_name} 2>/dev/null");
                let check_output = ssh_command(ssh_port, ssh_identity)
                    .arg("-o")
                    .arg("BatchMode=yes")
                    .arg("-o")
                    .arg("StrictHostKeyChecking=no")
                    .arg(ssh_target)
                    .arg(&check_cmd)
                    .stdin(Stdio::null())
                    .output()
                    .ok();

                if let Some(out) = check_output {
                    let remote_hash = String::from_utf8_lossy(&out.stdout).trim().to_lowercase();
                    remote_hash == bundle_hash
                } else {
                    false
                }
            }
            RemotePlatform::WindowsX86_64 => {
                let check_cmd = format!(
                    "powershell -NoProfile -Command \"Get-Content -Path $env:LOCALAPPDATA\\quiczilla\\{bundle_dir}\\{marker_name} -ErrorAction SilentlyContinue\"",
                );
                let check_output = ssh_command(ssh_port, ssh_identity)
                    .arg("-o")
                    .arg("BatchMode=yes")
                    .arg("-o")
                    .arg("StrictHostKeyChecking=no")
                    .arg(ssh_target)
                    .arg(&check_cmd)
                    .stdin(Stdio::null())
                    .output()
                    .ok();

                if let Some(out) = check_output {
                    let remote_hash = String::from_utf8_lossy(&out.stdout).trim().to_lowercase();
                    remote_hash == bundle_hash
                } else {
                    false
                }
            }
        };

        if !cache_hit {
            if verbose {
                eprintln!(
                    "Worker bundle cache miss; uploading {:.2} MiB",
                    (worker_bytes.len() + runtime_bytes.len()) as f64 / 1_048_576.0
                );
            }
            upload_remote_cache_file(
                platform,
                ssh_target,
                &bundle_dir,
                &binary_name,
                worker_bytes,
                ssh_port,
                ssh_identity,
            )?;
            upload_remote_cache_file(
                platform,
                ssh_target,
                &bundle_dir,
                runtime_name,
                runtime_bytes,
                ssh_port,
                ssh_identity,
            )?;
            upload_remote_cache_file(
                platform,
                ssh_target,
                &bundle_dir,
                marker_name,
                bundle_hash.as_bytes(),
                ssh_port,
                ssh_identity,
            )?;
        } else if verbose {
            eprintln!("Worker bundle cache hit");
        }
        run_cached_worker(
            platform,
            ssh_target,
            &bundle_dir,
            &binary_name,
            &worker_arg_str,
            verbose,
            ssh_port,
            ssh_identity,
        )?
    };

    // A remote cache may be mounted `noexec`. If the managed worker exits
    // before reporting readiness, fall back to an installed worker for these
    // early alpha releases rather than bypassing the host's execution policy.
    let (remote_port, remote_thumbprint, public_udp_addr, mut reader) = loop {
        let stdout = child
            .stdout
            .take()
            .context("Failed to capture worker stdout")?;
        let mut reader = BufReader::new(stdout);
        let mut remote_port = 0u16;
        let mut remote_thumbprint = String::new();
        let mut public_udp_addr = None;
        let mut line = String::new();

        while reader.read_line(&mut line)? > 0 {
            let trimmed = line.trim();
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(trimmed) {
                if let Some(status) = json.get("status").and_then(|s| s.as_str()) {
                    if status == "ready" || status == "daemon_ready" {
                        if let Some(p) = json.get("udp_port").and_then(|p| p.as_u64()) {
                            remote_port = p as u16;
                        }
                        if let Some(t) = json.get("thumbprint").and_then(|t| t.as_str()) {
                            remote_thumbprint = t.to_string();
                        }
                        public_udp_addr = json
                            .get("public_udp_addr")
                            .and_then(|address| address.as_str())
                            .and_then(|address| address.parse::<std::net::SocketAddr>().ok());
                        break;
                    }
                }
            } else if verbose && !trimmed.is_empty() {
                eprintln!("[Worker log] {}", trimmed);
            }
            line.clear();
        }

        if remote_port != 0 && !remote_thumbprint.is_empty() {
            break (remote_port, remote_thumbprint, public_udp_addr, reader);
        }

        let status = child.wait().ok();
        if used_managed_worker {
            if let Some(path) = pre_installed_path.as_deref() {
                eprintln!(
                    "Managed remote worker could not start{}; using installed worker at {path}. Update the installed worker to remove this warning.",
                    status
                        .as_ref()
                        .and_then(|value| value.code())
                        .map(|code| format!(" (exit {code})"))
                        .unwrap_or_default(),
                );
                child = run_installed_worker(
                    ssh_target,
                    path,
                    &worker_arg_str,
                    verbose,
                    ssh_port,
                    ssh_identity,
                )?;
                used_managed_worker = false;
                continue;
            }
        }
        anyhow::bail!("Failed to obtain ready state from remote worker");
    };

    // Keep reader alive in background to prevent BrokenPipe on worker and forward remaining logs
    std::thread::spawn(move || {
        let mut line = String::new();
        while let Ok(n) = reader.read_line(&mut line) {
            if n == 0 {
                break;
            }
            let trimmed = line.trim();
            if verbose && !trimmed.is_empty() {
                eprintln!("[Worker log] {}", trimmed);
            }
            line.clear();
        }
    });

    let bootstrap_duration = start_bootstrap.elapsed();
    if verbose {
        eprintln!(
            "Remote worker ready on UDP port {} after {:.2}s",
            remote_port,
            bootstrap_duration.as_secs_f64(),
        );
    }

    Ok(WorkerBootstrap {
        child,
        udp_port: remote_port,
        remote_thumbprint,
        host,
        public_udp_addr,
    })
}

fn remote_worker_checksum(
    platform: RemotePlatform,
    ssh_target: &str,
    worker_path: &str,
    ssh_port: Option<u16>,
    ssh_identity: Option<&str>,
) -> Option<String> {
    let command = match platform {
        RemotePlatform::LinuxX86_64 => format!("sha256sum -- {}", shell_quote(worker_path)),
        RemotePlatform::WindowsX86_64 => format!(
            "powershell -NoProfile -Command \"(Get-FileHash -Algorithm SHA256 -LiteralPath '{}').Hash\"",
            worker_path.replace('\'', "''")
        ),
    };
    let output = ssh_command(ssh_port, ssh_identity)
        .arg("-o")
        .arg("BatchMode=yes")
        .arg("-o")
        .arg("StrictHostKeyChecking=no")
        .arg(ssh_target)
        .arg(command)
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()?
        .split_whitespace()
        .next()
        .filter(|hash| hash.len() == 64 && hash.as_bytes().iter().all(u8::is_ascii_hexdigit))
        .map(str::to_ascii_lowercase)
}

fn sibling_worker_bytes(platform: RemotePlatform) -> Option<Vec<u8>> {
    let file_name = match platform {
        RemotePlatform::LinuxX86_64 => "quiczilla-worker",
        RemotePlatform::WindowsX86_64 => "quiczilla-worker.exe",
    };
    let path = env::current_exe().ok()?.parent()?.join(file_name);
    let metadata = fs::metadata(&path).ok()?;
    if !metadata.is_file() || metadata.len() == 0 {
        return None;
    }
    fs::read(path).ok()
}

fn run_installed_worker(
    ssh_target: &str,
    worker_path: &str,
    worker_arg_str: &str,
    verbose: bool,
    ssh_port: Option<u16>,
    ssh_identity: Option<&str>,
) -> Result<std::process::Child> {
    let run_cmd = format!("{} {}", shell_quote(worker_path), worker_arg_str);
    ssh_command(ssh_port, ssh_identity)
        .arg("-o")
        .arg("BatchMode=yes")
        .arg("-o")
        .arg("StrictHostKeyChecking=no")
        .arg(ssh_target)
        .arg(run_cmd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(worker_stderr(verbose))
        .spawn()
        .context("Failed to start installed remote worker")
}

fn worker_stderr(verbose: bool) -> Stdio {
    if verbose {
        Stdio::inherit()
    } else {
        Stdio::null()
    }
}

/// Upload one member of the versioned worker bundle. Keeping the native runtime
/// adjacent to the worker lets `find_msquic_library` load it without requiring
/// a system-wide .NET or MsQuic installation on the SSH target.
fn upload_remote_cache_file(
    platform: RemotePlatform,
    ssh_target: &str,
    bundle_dir: &str,
    file_name: &str,
    data: &[u8],
    ssh_port: Option<u16>,
    ssh_identity: Option<&str>,
) -> Result<()> {
    let deploy_cmd = match platform {
        RemotePlatform::LinuxX86_64 => {
            let suffix = if file_name.starts_with("worker-") {
                format!(" && chmod +x ~/.cache/quiczilla/{bundle_dir}/{file_name}")
            } else {
                String::new()
            };
            format!(
                "mkdir -p ~/.cache/quiczilla/{bundle_dir} && cat > ~/.cache/quiczilla/{bundle_dir}/{file_name}{suffix}"
            )
        }
        RemotePlatform::WindowsX86_64 => format!(
            "powershell -NoProfile -Command \"New-Item -ItemType Directory -Force -Path \\\"$env:LOCALAPPDATA\\quiczilla\\{bundle_dir}\\\" | Out-Null; [Console]::OpenStandardInput().CopyTo([IO.File]::Create(\\\"$env:LOCALAPPDATA\\quiczilla\\{bundle_dir}\\{file_name}\\\"))\"",
        ),
    };

    let mut child = ssh_command(ssh_port, ssh_identity)
        .arg("-o")
        .arg("BatchMode=yes")
        .arg("-o")
        .arg("StrictHostKeyChecking=no")
        .arg(ssh_target)
        .arg(deploy_cmd)
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Failed to start SSH worker-bundle upload")?;
    let mut stdin = child
        .stdin
        .take()
        .context("SSH worker-bundle upload has no stdin")?;
    stdin
        .write_all(data)
        .context("Failed to upload worker-bundle member")?;
    drop(stdin);
    let output = child
        .wait_with_output()
        .context("Failed to finish SSH worker-bundle upload")?;
    if !output.status.success() {
        anyhow::bail!(
            "Remote worker-bundle upload failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_cached_worker(
    platform: RemotePlatform,
    ssh_target: &str,
    bundle_dir: &str,
    binary_name: &str,
    worker_arg_str: &str,
    verbose: bool,
    ssh_port: Option<u16>,
    ssh_identity: Option<&str>,
) -> Result<std::process::Child> {
    let run_cmd = match platform {
        RemotePlatform::LinuxX86_64 => {
            format!("~/.cache/quiczilla/{bundle_dir}/{binary_name} {worker_arg_str}")
        }
        RemotePlatform::WindowsX86_64 => format!(
            "powershell -NoProfile -Command \"& '$env:LOCALAPPDATA\\quiczilla\\{bundle_dir}\\{binary_name}' {worker_arg_str}\""
        ),
    };
    ssh_command(ssh_port, ssh_identity)
        .arg("-o")
        .arg("BatchMode=yes")
        .arg("-o")
        .arg("StrictHostKeyChecking=no")
        .arg(ssh_target)
        .arg(run_cmd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(worker_stderr(verbose))
        .spawn()
        .context("Failed to start cached remote worker")
}
use quiczilla_core::types::ProgressInfo;
use std::io::Read;

#[allow(clippy::too_many_arguments)]
fn run_ssh_fallback(
    local_path: &Path,
    ssh_target: &str,
    remote_dir: &str,
    file_name: &str,
    file_size: u64,
    checksum_requested: bool,
    output: OutputOptions,
    renderer: &mut ProgressRenderer,
) -> Result<()> {
    let remote_path = format!("{}/{}", remote_dir.trim_end_matches(['/', '\\']), file_name);
    let remote_command = format!(
        "mkdir -p -- {} && cat > {}",
        shell_quote(remote_dir),
        shell_quote(&remote_path),
    );
    renderer.status(&format!("Transferring {} over SSH…", file_name));

    let mut child = ssh_command(output.ssh_port, output.ssh_identity.as_deref())
        .arg("-o")
        .arg("BatchMode=yes")
        .arg("-o")
        .arg("StrictHostKeyChecking=no")
        .arg(ssh_target)
        .arg(remote_command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(if output.verbose {
            Stdio::inherit()
        } else {
            Stdio::piped()
        })
        .spawn()
        .context("Failed to start SSH fallback transfer")?;
    let mut destination = child
        .stdin
        .take()
        .context("SSH fallback did not provide stdin")?;
    let mut source = std::fs::File::open(local_path)?;
    let start = Instant::now();
    let mut sent = 0u64;
    let mut hasher = checksum_requested.then(Sha256::new);
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let read = source.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        destination.write_all(&buffer[..read])?;
        if let Some(hash) = hasher.as_mut() {
            hash.update(&buffer[..read]);
        }
        sent += read as u64;
        let elapsed = start.elapsed().as_secs_f64().max(0.001);
        let rate = sent as f64 / elapsed;
        renderer.progress(
            file_name,
            &ProgressInfo {
                bytes_transferred: sent,
                total_bytes: file_size,
                speed_bytes_per_second: rate,
                percentage: sent as f64 * 100.0 / file_size.max(1) as f64,
                estimated_remaining_secs: Some(
                    (file_size.saturating_sub(sent)) as f64 / rate.max(1.0),
                ),
                is_completed: sent == file_size,
                average_speed_bytes_per_second: Some(rate),
                total_time_secs: Some(elapsed),
            },
        );
    }
    drop(destination);
    let result = child
        .wait()
        .context("Failed waiting for SSH fallback transfer")?;
    if !result.success() {
        anyhow::bail!("SSH fallback exited with {}", result);
    }

    let verified = if let Some(local_hash) = hasher.map(|hash| hex::encode(hash.finalize())) {
        let check = ssh_command(output.ssh_port, output.ssh_identity.as_deref())
            .arg("-o")
            .arg("BatchMode=yes")
            .arg(ssh_target)
            .arg(format!("sha256sum -- {}", shell_quote(&remote_path)))
            .output();
        check
            .ok()
            .filter(|value| value.status.success())
            .and_then(|value| {
                String::from_utf8(value.stdout)
                    .ok()
                    .and_then(|text| text.split_whitespace().next().map(str::to_owned))
            })
            .is_some_and(|remote_hash| remote_hash.eq_ignore_ascii_case(&local_hash))
    } else {
        false
    };
    renderer.completed(file_name, sent, start.elapsed(), verified);
    emit_json_receipt(
        output.json,
        file_name,
        sent,
        start.elapsed(),
        "ssh-fallback",
        checksum_requested && verified,
    );
    Ok(())
}

fn emit_json_receipt(
    enabled: bool,
    file_name: &str,
    bytes: u64,
    duration: Duration,
    transport: &str,
    checksum_verified: bool,
) {
    if !enabled {
        return;
    }
    let seconds = duration.as_secs_f64().max(0.001);
    let receipt = serde_json::json!({
        "status": "completed",
        "file": file_name,
        "bytes": bytes,
        "duration_seconds": seconds,
        "average_bytes_per_second": bytes as f64 / seconds,
        "duration_scope": "payload",
        "transport": transport,
        "integrity": if checksum_verified { "sha256" } else { "none" },
        "verified": checksum_verified,
    });
    println!("{receipt}");
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r#"'"'"'"#))
}

pub fn ssh_command(port: Option<u16>, identity: Option<&str>) -> Command {
    let mut command = Command::new("ssh");
    if let Some(port) = port {
        command.arg("-p").arg(port.to_string());
    }
    if let Some(identity) = identity {
        command.arg("-i").arg(identity);
    }
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shell_quote() {
        assert_eq!(shell_quote("hello world"), "'hello world'");
        assert_eq!(shell_quote("it's"), r#"'it'"'"'s'"#);
    }

    #[test]
    fn test_ssh_command_flags() {
        let cmd = ssh_command(Some(2222), Some("/path/to/id_ed25519"));
        let debug_str = format!("{:?}", cmd);
        assert!(debug_str.contains("-p"));
        assert!(debug_str.contains("2222"));
        assert!(debug_str.contains("-i"));
        assert!(debug_str.contains("/path/to/id_ed25519"));
    }

    #[test]
    fn short_quiet_flag_matches_long_form() {
        let short = OutputOptions::from_args(&["-q".to_string()]).unwrap();
        let long = OutputOptions::from_args(&["--quiet".to_string()]).unwrap();
        assert!(short.quiet);
        assert_eq!(short.quiet, long.quiet);
    }

    #[test]
    fn strict_parser_rejects_unknown_options() {
        let args = vec![
            "quic".to_string(),
            "daemon".to_string(),
            "--allow-thumbprints".to_string(),
            "authorized.txt".to_string(),
            "--not-a-real-option".to_string(),
        ];
        assert!(strict_validate_args(&args).is_err());
    }

    #[test]
    fn strict_parser_accepts_direct_transfer() {
        let args = vec![
            "quic".to_string(),
            "direct".to_string(),
            "receiver:55441".to_string(),
            "archive.tar".to_string(),
            "--thumbprint".to_string(),
            "0123456789012345678901234567890123456789".to_string(),
            "--checksum".to_string(),
        ];
        assert!(strict_validate_args(&args).is_ok());
    }

    #[test]
    fn json_receipt_flag_is_accepted() {
        let args = vec![
            "quic".to_string(),
            "source.bin".to_string(),
            "user@host:/tmp".to_string(),
            "--json".to_string(),
        ];
        assert!(strict_validate_args(&args).is_ok());
        assert!(OutputOptions::from_args(&args[1..]).unwrap().json);
    }

    #[test]
    fn direct_transport_accepts_a_preferred_quic_host() {
        let args = vec![
            "quic".to_string(),
            "source.bin".to_string(),
            "user@ssh-host:/tmp".to_string(),
            "--transport".to_string(),
            "direct".to_string(),
            "--quic-host".to_string(),
            "192.0.2.44".to_string(),
        ];
        assert!(strict_validate_args(&args).is_ok());
        let options = OutputOptions::from_args(&args[1..]).unwrap();
        assert_eq!(options.transport, TransportPreference::Direct);
        assert_eq!(options.quic_host.as_deref(), Some("192.0.2.44"));
    }

    #[test]
    fn resolves_a_numeric_preferred_quic_host() {
        assert_eq!(
            resolve_quic_address("127.0.0.1", 55441).unwrap(),
            "127.0.0.1:55441".parse().unwrap()
        );
    }
}
