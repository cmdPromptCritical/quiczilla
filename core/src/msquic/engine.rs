use anyhow::{Context, Result};
use std::ffi::CString;
use std::net::SocketAddr;
use std::ptr;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

use super::ffi::*;
use super::stream::{MsQuicStream, msquic_stream_callback};

pub struct MsQuicEngine {
    pub registration: HQUIC,
}

unsafe impl Send for MsQuicEngine {}
unsafe impl Sync for MsQuicEngine {}

impl MsQuicEngine {
    pub fn new() -> Result<Self> {
        tracing::debug!("[engine] Getting API");
        let api = super::ffi::get_api()?;
        tracing::debug!("[engine] Got API");

        let mut registration: HQUIC = ptr::null_mut();
        let app_name = std::ffi::CString::new("quiczilla")?;
        let reg_config = QUIC_REGISTRATION_CONFIG {
            AppName: app_name.as_ptr(),
            ExecutionProfile: QUIC_EXECUTION_PROFILE_QUIC_EXECUTION_PROFILE_TYPE_MAX_THROUGHPUT,
        };

        let status = unsafe { (api.RegistrationOpen.unwrap())(&reg_config, &mut registration) };

        if status != 0 {
            anyhow::bail!("RegistrationOpen failed with status 0x{:08x}", status);
        }

        Ok(Self { registration })
    }

    pub fn create_configuration(
        &self,
        is_server: bool,
        pkcs12_der: Option<&[u8]>,
        pkcs12_pwd: Option<&str>,
        cert_file: Option<&str>,
        key_file: Option<&str>,
    ) -> Result<MsQuicConfiguration> {
        self.create_configuration_inner(
            is_server, pkcs12_der, pkcs12_pwd, cert_file, key_file, None,
        )
    }

    #[cfg(windows)]
    pub fn create_configuration_from_hash(
        &self,
        is_server: bool,
        certificate_hash: [u8; 20],
    ) -> Result<MsQuicConfiguration> {
        self.create_configuration_inner(is_server, None, None, None, None, Some(certificate_hash))
    }

    fn create_configuration_inner(
        &self,
        is_server: bool,
        pkcs12_der: Option<&[u8]>,
        pkcs12_pwd: Option<&str>,
        cert_file: Option<&str>,
        key_file: Option<&str>,
        certificate_hash: Option<[u8; 20]>,
    ) -> Result<MsQuicConfiguration> {
        #[cfg(windows)]
        let _ = (pkcs12_der, pkcs12_pwd, cert_file, key_file);
        #[cfg(not(windows))]
        let _ = certificate_hash;

        tracing::debug!("[engine] Getting API");
        let api = super::ffi::get_api()?;
        tracing::debug!("[engine] Got API");

        let alpn_str = b"fileShare";
        let alpn = QUIC_BUFFER {
            Length: alpn_str.len() as u32,
            Buffer: alpn_str.as_ptr() as *mut u8,
        };

        let mut settings: QUIC_SETTINGS = unsafe { std::mem::zeroed() };
        tracing::debug!("[engine] Zeroing settings");

        // High throughput optimizations
        unsafe {
            // StreamReceiveWindow = 32 MB
            settings.StreamRecvWindowDefault = 32 * 1024 * 1024;
            settings
                .__bindgen_anon_1
                .IsSet
                .set_StreamRecvWindowDefault(1);

            // ConnReceiveWindow = 64 MB
            settings.ConnFlowControlWindow = 64 * 1024 * 1024;
            settings.__bindgen_anon_1.IsSet.set_ConnFlowControlWindow(1);

            // SendBufferingEnabled = 1
            settings.set_SendBufferingEnabled(1);
            settings.__bindgen_anon_1.IsSet.set_SendBufferingEnabled(1);

            // CongestionControlAlgorithm = BBR (1)
            settings.CongestionControlAlgorithm =
                QUIC_CONGESTION_CONTROL_ALGORITHM_QUIC_CONGESTION_CONTROL_ALGORITHM_BBR as u16;
            settings
                .__bindgen_anon_1
                .IsSet
                .set_CongestionControlAlgorithm(1);

            // MaximumMtu = 1500
            settings.MaximumMtu = 1500;
            settings.__bindgen_anon_1.IsSet.set_MaximumMtu(1);

            // MaxWorkerQueueDelayUs = 1000
            settings.MaxWorkerQueueDelayUs = 1000;
            settings.__bindgen_anon_1.IsSet.set_MaxWorkerQueueDelayUs(1);

            // PeerBidiStreamCount = 100
            settings.PeerBidiStreamCount = 100;
            settings.__bindgen_anon_1.IsSet.set_PeerBidiStreamCount(1);

            // Idle Timeout = 30 seconds
            settings.IdleTimeoutMs = 30000;
            settings.__bindgen_anon_1.IsSet.set_IdleTimeoutMs(1);
        }

        let mut configuration: HQUIC = ptr::null_mut();
        tracing::debug!("[engine] Calling ConfigurationOpen");
        let status = unsafe {
            (api.ConfigurationOpen.unwrap())(
                self.registration,
                &alpn,
                1,
                &settings,
                std::mem::size_of::<QUIC_SETTINGS>() as u32,
                ptr::null_mut(),
                &mut configuration,
            )
        };
        tracing::debug!("[engine] ConfigurationOpen returned {}", status);

        if status != 0 {
            anyhow::bail!("ConfigurationOpen failed with status 0x{:08x}", status);
        }

        // Configure Credentials:
        let mut cred_config = unsafe { std::mem::zeroed::<QUIC_CREDENTIAL_CONFIG>() };

        #[cfg(windows)]
        let mut certificate_hash_storage = QUIC_CERTIFICATE_HASH {
            ShaHash: certificate_hash.unwrap_or([0; 20]),
        };
        #[cfg(windows)]
        if certificate_hash.is_some() {
            cred_config.Type = QUIC_CREDENTIAL_TYPE_QUIC_CREDENTIAL_TYPE_CERTIFICATE_HASH;
            cred_config.__bindgen_anon_1.CertificateHash = &mut certificate_hash_storage;
        } else {
            cred_config.Type = QUIC_CREDENTIAL_TYPE_QUIC_CREDENTIAL_TYPE_NONE;
        }

        #[cfg(not(windows))]
        let mut pkcs12 = unsafe { std::mem::zeroed::<QUIC_CERTIFICATE_PKCS12>() };
        #[cfg(not(windows))]
        let mut cert_file_struct = unsafe { std::mem::zeroed::<QUIC_CERTIFICATE_FILE>() };
        #[cfg(not(windows))]
        let c_pwd = pkcs12_pwd.map(|password| std::ffi::CString::new(password).unwrap());
        #[cfg(not(windows))]
        let c_cert = cert_file.map(|certificate| std::ffi::CString::new(certificate).unwrap());
        #[cfg(not(windows))]
        let c_key = key_file.map(|key| std::ffi::CString::new(key).unwrap());
        #[cfg(not(windows))]
        if let (Some(der), Some(password)) = (pkcs12_der, &c_pwd) {
            pkcs12.Asn1Blob = der.as_ptr();
            pkcs12.Asn1BlobLength = der.len() as u32;
            pkcs12.PrivateKeyPassword = password.as_ptr();
            cred_config.Type = QUIC_CREDENTIAL_TYPE_QUIC_CREDENTIAL_TYPE_CERTIFICATE_PKCS12;
            cred_config.__bindgen_anon_1.CertificatePkcs12 = &mut pkcs12;
        } else if let (Some(certificate), Some(key)) = (&c_cert, &c_key) {
            cert_file_struct.CertificateFile = certificate.as_ptr();
            cert_file_struct.PrivateKeyFile = key.as_ptr();
            cred_config.Type = QUIC_CREDENTIAL_TYPE_QUIC_CREDENTIAL_TYPE_CERTIFICATE_FILE;
            cred_config.__bindgen_anon_1.CertificateFile = &mut cert_file_struct;
        } else {
            cred_config.Type = QUIC_CREDENTIAL_TYPE_QUIC_CREDENTIAL_TYPE_NONE;
        }

        if is_server {
            cred_config.Flags =
                QUIC_CREDENTIAL_FLAGS_QUIC_CREDENTIAL_FLAG_INDICATE_CERTIFICATE_RECEIVED
                    | QUIC_CREDENTIAL_FLAGS_QUIC_CREDENTIAL_FLAG_REQUIRE_CLIENT_AUTHENTICATION;
            #[cfg(windows)]
            {
                cred_config.Flags |=
                    QUIC_CREDENTIAL_FLAGS_QUIC_CREDENTIAL_FLAG_DEFER_CERTIFICATE_VALIDATION;
            }
            #[cfg(not(windows))]
            {
                cred_config.Flags |=
                    QUIC_CREDENTIAL_FLAGS_QUIC_CREDENTIAL_FLAG_NO_CERTIFICATE_VALIDATION
                        | QUIC_CREDENTIAL_FLAGS_QUIC_CREDENTIAL_FLAG_USE_PORTABLE_CERTIFICATES;
            }
        } else {
            cred_config.Flags = QUIC_CREDENTIAL_FLAGS_QUIC_CREDENTIAL_FLAG_CLIENT
                | QUIC_CREDENTIAL_FLAGS_QUIC_CREDENTIAL_FLAG_INDICATE_CERTIFICATE_RECEIVED;
            #[cfg(windows)]
            {
                cred_config.Flags |=
                    QUIC_CREDENTIAL_FLAGS_QUIC_CREDENTIAL_FLAG_DEFER_CERTIFICATE_VALIDATION
                        | QUIC_CREDENTIAL_FLAGS_QUIC_CREDENTIAL_FLAG_USE_SUPPLIED_CREDENTIALS;
            }
            #[cfg(not(windows))]
            {
                cred_config.Flags |=
                    QUIC_CREDENTIAL_FLAGS_QUIC_CREDENTIAL_FLAG_NO_CERTIFICATE_VALIDATION
                        | QUIC_CREDENTIAL_FLAGS_QUIC_CREDENTIAL_FLAG_USE_PORTABLE_CERTIFICATES;
            }
        }

        tracing::debug!("[engine] Calling ConfigurationLoadCredential");
        let status =
            unsafe { (api.ConfigurationLoadCredential.unwrap())(configuration, &cred_config) };
        tracing::debug!("[engine] ConfigurationLoadCredential returned {}", status);

        if status != 0 {
            if let Some(close_fn) = api.ConfigurationClose {
                unsafe { close_fn(configuration) };
            }
            anyhow::bail!(
                "ConfigurationLoadCredential failed with status 0x{:08x}",
                status
            );
        }

        Ok(MsQuicConfiguration {
            handle: configuration,
        })
    }
}

pub struct MsQuicConfiguration {
    pub handle: HQUIC,
}

unsafe impl Send for MsQuicConfiguration {}
unsafe impl Sync for MsQuicConfiguration {}

impl Drop for MsQuicConfiguration {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            if let Ok(api) = super::ffi::get_api() {
                if let Some(close_fn) = api.ConfigurationClose {
                    unsafe { close_fn(self.handle) };
                }
            }
        }
    }
}

impl Drop for MsQuicEngine {
    fn drop(&mut self) {
        if !self.registration.is_null() {
            if let Ok(api) = super::ffi::get_api() {
                if let Some(close_fn) = api.RegistrationClose {
                    unsafe { close_fn(self.registration) };
                }
            }
        }
    }
}

struct ConnectionContext {
    connect_tx: Option<oneshot::Sender<Result<()>>>,
    stream_tx: mpsc::UnboundedSender<MsQuicStream>,
    allowed_peer_thumbprints: Vec<String>,
}

unsafe extern "C" fn connection_callback(
    _connection: HQUIC,
    context: *mut std::ffi::c_void,
    event: *mut QUIC_CONNECTION_EVENT,
) -> QUIC_STATUS {
    if context.is_null() || event.is_null() {
        return QUIC_STATUS_SUCCESS;
    }

    let ctx = unsafe { &mut *(context as *mut ConnectionContext) };
    let ev = unsafe { &*event };

    tracing::debug!("[connection] event Type = {}", ev.Type);

    match ev.Type {
        QUIC_CONNECTION_EVENT_TYPE_QUIC_CONNECTION_EVENT_CONNECTED => {
            tracing::debug!("[connection] CONNECTED event");
            if let Some(tx) = ctx.connect_tx.take() {
                let _ = tx.send(Ok(()));
            }
        }
        QUIC_CONNECTION_EVENT_TYPE_QUIC_CONNECTION_EVENT_PEER_CERTIFICATE_RECEIVED => {
            tracing::debug!("[engine] PEER_CERTIFICATE_RECEIVED callback fired!");
            if !ctx.allowed_peer_thumbprints.is_empty() {
                tracing::debug!(
                    "[engine] Allowed peer thumbprints: {}",
                    ctx.allowed_peer_thumbprints.len()
                );
                let cert_data = unsafe { &ev.__bindgen_anon_1.PEER_CERTIFICATE_RECEIVED };
                if !cert_data.Certificate.is_null() {
                    #[cfg(not(windows))]
                    let der_slice = unsafe {
                        let buffer = &*(cert_data.Certificate as *const QUIC_BUFFER);
                        std::slice::from_raw_parts(buffer.Buffer, buffer.Length as usize)
                    };
                    #[cfg(windows)]
                    let der_slice = unsafe {
                        let cert_context = cert_data.Certificate as *const std::ffi::c_void;
                        #[repr(C)]
                        #[allow(non_snake_case)]
                        struct CertContext {
                            dwCertEncodingType: u32,
                            pbCertEncoded: *mut u8,
                            cbCertEncoded: u32,
                        }
                        let ctx_ptr = cert_context as *const CertContext;
                        std::slice::from_raw_parts(
                            (*ctx_ptr).pbCertEncoded,
                            (*ctx_ptr).cbCertEncoded as usize,
                        )
                    };

                    let actual_thumbprint = crate::cert::compute_thumbprint(der_slice);
                    if ctx
                        .allowed_peer_thumbprints
                        .iter()
                        .any(|expected| actual_thumbprint.eq_ignore_ascii_case(expected))
                    {
                        return QUIC_STATUS_SUCCESS;
                    } else {
                        tracing::debug!(
                            "Peer certificate thumbprint was not authorized: {}",
                            actual_thumbprint
                        );
                        return super::ffi::QUIC_STATUS_BAD_CERTIFICATE;
                    }
                }
                return super::ffi::QUIC_STATUS_BAD_CERTIFICATE;
            }
        }
        QUIC_CONNECTION_EVENT_TYPE_QUIC_CONNECTION_EVENT_SHUTDOWN_INITIATED_BY_TRANSPORT => {
            let ev_data = unsafe { &ev.__bindgen_anon_1.SHUTDOWN_INITIATED_BY_TRANSPORT };
            tracing::debug!(
                "[connection] SHUTDOWN_INITIATED_BY_TRANSPORT: Status 0x{:08x}, ErrorCode 0x{:x} ({})",
                ev_data.Status,
                ev_data.ErrorCode,
                ev_data.ErrorCode
            );
            if let Some(tx) = ctx.connect_tx.take() {
                let _ = tx.send(Err(anyhow::anyhow!(
                    "Connection failed with transport error 0x{:08x}",
                    ev_data.Status
                )));
            }
        }
        QUIC_CONNECTION_EVENT_TYPE_QUIC_CONNECTION_EVENT_SHUTDOWN_INITIATED_BY_PEER => {
            let ev_data = unsafe { &ev.__bindgen_anon_1.SHUTDOWN_INITIATED_BY_PEER };
            tracing::debug!(
                "[connection] SHUTDOWN_INITIATED_BY_PEER: ErrorCode 0x{:x} ({})",
                ev_data.ErrorCode,
                ev_data.ErrorCode
            );
            if let Some(tx) = ctx.connect_tx.take() {
                let _ = tx.send(Err(anyhow::anyhow!("Peer shutdown")));
            }
        }
        QUIC_CONNECTION_EVENT_TYPE_QUIC_CONNECTION_EVENT_PEER_STREAM_STARTED => {
            let stream_data = unsafe { &ev.__bindgen_anon_1.PEER_STREAM_STARTED };
            let stream_handle = stream_data.Stream;
            tracing::debug!(
                "[connection] PEER_STREAM_STARTED: handle={:?}, flags={:?}",
                stream_handle,
                stream_data.Flags
            );
            let (send, recv) = MsQuicStream::new(stream_handle);
            let msquic_stream = MsQuicStream { send, recv };

            let api = match super::ffi::get_api() {
                Ok(a) => a,
                Err(_) => return QUIC_STATUS_SUCCESS,
            };
            if let Some(set_cb) = api.SetCallbackHandler {
                let raw_ctx =
                    Arc::into_raw(msquic_stream.send.shared.clone()) as *mut std::ffi::c_void;
                let handler_ptr = msquic_stream_callback as *mut std::ffi::c_void;
                unsafe {
                    set_cb(stream_handle, handler_ptr, raw_ctx);
                }
            }

            let _ = ctx.stream_tx.send(msquic_stream);
        }
        _ => {}
    }

    QUIC_STATUS_SUCCESS
}

pub struct MsQuicConnection {
    pub handle: HQUIC,
    _ctx_box: Box<ConnectionContext>,
    pub stream_rx: mpsc::UnboundedReceiver<MsQuicStream>,
}

unsafe impl Send for MsQuicConnection {}
unsafe impl Sync for MsQuicConnection {}

/// Closes a connection that is still being established if its future is
/// cancelled or returns an error before ownership moves to `MsQuicConnection`.
/// This matters for candidate racing, where the losing connection future is
/// intentionally dropped as soon as another authenticated path wins.
struct PendingConnection {
    handle: HQUIC,
}

// MsQuic connection handles are thread-safe native handles; `MsQuicConnection`
// itself carries the same invariant.
unsafe impl Send for PendingConnection {}

impl PendingConnection {
    fn take(&mut self) -> HQUIC {
        std::mem::replace(&mut self.handle, ptr::null_mut())
    }
}

impl Drop for PendingConnection {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            if let Ok(api) = super::ffi::get_api() {
                if let Some(close_fn) = api.ConnectionClose {
                    unsafe { close_fn(self.handle) };
                }
            }
        }
    }
}

impl MsQuicConnection {
    pub async fn connect(
        engine: &MsQuicEngine,
        config: &MsQuicConfiguration,
        remote_addr: SocketAddr,
        local_bind_addr: Option<SocketAddr>,
        expected_thumbprint: Option<String>,
    ) -> Result<Self> {
        tracing::debug!("[engine] Getting API");
        let api = super::ffi::get_api()?;
        tracing::debug!("[engine] Got API");

        let (connect_tx, connect_rx) = oneshot::channel();
        let (stream_tx, stream_rx) = mpsc::unbounded_channel();

        let mut ctx_box = Box::new(ConnectionContext {
            connect_tx: Some(connect_tx),
            stream_tx,
            allowed_peer_thumbprints: expected_thumbprint.into_iter().collect(),
        });

        let mut pending_connection = {
            let mut connection: HQUIC = ptr::null_mut();
            let status = unsafe {
                (api.ConnectionOpen.unwrap())(
                    engine.registration,
                    Some(connection_callback),
                    ctx_box.as_mut() as *mut ConnectionContext as *mut std::ffi::c_void,
                    &mut connection,
                )
            };

            if status != 0 {
                anyhow::bail!("ConnectionOpen failed with status 0x{:08x}", status);
            }
            let pending_connection = PendingConnection { handle: connection };

            if let Some(local_addr) = local_bind_addr {
                let addr_storage = socket2::SockAddr::from(local_addr);
                let set_param = api.SetParam.context("MsQuic SetParam is unavailable")?;

                #[cfg(windows)]
                let addr_len = 28; // sizeof(SOCKADDR_INET)
                #[cfg(not(windows))]
                let addr_len = 128; // sizeof(sockaddr_storage)

                // Copy to a padded buffer of the correct size to satisfy MsQuic length checks
                let mut padded_addr = [0u8; 128];
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        addr_storage.as_ptr() as *const u8,
                        padded_addr.as_mut_ptr(),
                        addr_storage.len() as usize,
                    );
                }

                let status = unsafe {
                    set_param(
                        pending_connection.handle,
                        QUIC_PARAM_CONN_LOCAL_ADDRESS,
                        addr_len as u32,
                        padded_addr.as_ptr() as *const std::ffi::c_void,
                    )
                };
                if status != 0 {
                    anyhow::bail!(
                        "Failed to bind local QUIC address {local_addr}: 0x{:08x}",
                        status
                    );
                }
            }

            let host = remote_addr.ip().to_string();
            let host_c = CString::new(host.as_str())?;
            let port = remote_addr.port();

            let status = unsafe {
                (api.ConnectionStart.unwrap())(
                    pending_connection.handle,
                    config.handle,
                    0, // AF_UNSPEC
                    host_c.as_ptr(),
                    port,
                )
            };

            if !super::ffi::is_quic_success_or_pending(status) {
                anyhow::bail!("ConnectionStart failed with status 0x{:08x}", status);
            }

            pending_connection
        };

        connect_rx.await??;

        Ok(Self {
            handle: pending_connection.take(),
            _ctx_box: ctx_box,
            stream_rx,
        })
    }

    pub async fn open_stream(&self) -> Result<MsQuicStream> {
        tracing::debug!("[engine] Getting API");
        let api = super::ffi::get_api()?;
        tracing::debug!("[engine] Got API");

        let mut stream_handle: HQUIC = ptr::null_mut();
        let (send, recv) = MsQuicStream::new(ptr::null_mut());
        let msquic_stream = MsQuicStream { send, recv };

        let raw_ctx = Arc::into_raw(msquic_stream.send.shared.clone()) as *mut std::ffi::c_void;

        let status = unsafe {
            (api.StreamOpen.unwrap())(
                self.handle,
                QUIC_STREAM_OPEN_FLAGS_QUIC_STREAM_OPEN_FLAG_NONE,
                Some(msquic_stream_callback),
                raw_ctx,
                &mut stream_handle,
            )
        };

        if status != 0 {
            let _ = unsafe {
                Arc::from_raw(
                    raw_ctx as *const std::sync::Mutex<crate::msquic::stream::StreamShared>,
                )
            };
            anyhow::bail!("StreamOpen failed with status 0x{:08x}", status);
        }

        // Update stream handle in shared state
        {
            let mut state = msquic_stream.send.shared.lock().unwrap();
            state.stream_handle = stream_handle;
        }

        let status = unsafe {
            (api.StreamStart.unwrap())(
                stream_handle,
                QUIC_STREAM_START_FLAGS_QUIC_STREAM_START_FLAG_IMMEDIATE,
            )
        };
        tracing::debug!("[engine] StreamStart returned 0x{:08x}", status);

        if !super::ffi::is_quic_success_or_pending(status) {
            anyhow::bail!("StreamStart failed with status 0x{:08x}", status);
        }

        Ok(msquic_stream)
    }

    pub async fn accept_stream(&mut self) -> Option<MsQuicStream> {
        self.stream_rx.recv().await
    }

    /// Request an orderly application shutdown before this connection is
    /// dropped and its native handle is closed.
    pub fn shutdown(&self) {
        if self.handle.is_null() {
            return;
        }
        if let Ok(api) = super::ffi::get_api() {
            if let Some(shutdown_fn) = api.ConnectionShutdown {
                unsafe {
                    shutdown_fn(
                        self.handle,
                        QUIC_CONNECTION_SHUTDOWN_FLAGS_QUIC_CONNECTION_SHUTDOWN_FLAG_NONE,
                        0,
                    );
                }
            }
        }
    }
}

impl Drop for MsQuicConnection {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            if let Ok(api) = super::ffi::get_api() {
                if let Some(close_fn) = api.ConnectionClose {
                    unsafe { close_fn(self.handle) };
                }
            }
        }
    }
}

struct ListenerContext {
    conn_tx: mpsc::UnboundedSender<MsQuicConnection>,
    server_config: HQUIC,
    allowed_peer_thumbprints: Vec<String>,
}

unsafe extern "C" fn listener_callback(
    _listener: HQUIC,
    context: *mut std::ffi::c_void,
    event: *mut QUIC_LISTENER_EVENT,
) -> QUIC_STATUS {
    if context.is_null() || event.is_null() {
        return QUIC_STATUS_SUCCESS;
    }

    let ctx = unsafe { &mut *(context as *mut ListenerContext) };
    let ev = unsafe { &*event };

    if ev.Type == QUIC_LISTENER_EVENT_TYPE_QUIC_LISTENER_EVENT_NEW_CONNECTION {
        tracing::debug!("[listener] NEW_CONNECTION received!");
        let new_conn_data = unsafe { &ev.__bindgen_anon_1.NEW_CONNECTION };
        let conn_handle = new_conn_data.Connection;

        let (stream_tx, stream_rx) = mpsc::unbounded_channel();
        let mut conn_ctx = Box::new(ConnectionContext {
            connect_tx: None,
            stream_tx,
            allowed_peer_thumbprints: ctx.allowed_peer_thumbprints.clone(),
        });

        let api = match super::ffi::get_api() {
            Ok(a) => a,
            Err(_) => return QUIC_STATUS_SUCCESS,
        };

        if let Some(set_cb) = api.SetCallbackHandler {
            let conn_ctx_ptr = conn_ctx.as_mut() as *mut ConnectionContext as *mut std::ffi::c_void;
            let handler_ptr = connection_callback as *mut std::ffi::c_void;
            unsafe {
                set_cb(conn_handle, handler_ptr, conn_ctx_ptr);
            }
        }

        if let Some(set_config) = api.ConnectionSetConfiguration {
            let status = unsafe { set_config(conn_handle, ctx.server_config) };
            tracing::debug!(
                "[listener] ConnectionSetConfiguration returned 0x{:08x}",
                status
            );
            if !super::ffi::is_quic_success_or_pending(status) {
                return status;
            }
        }

        let msquic_conn = MsQuicConnection {
            handle: conn_handle,
            _ctx_box: conn_ctx,
            stream_rx,
        };

        match ctx.conn_tx.send(msquic_conn) {
            Ok(_) => tracing::debug!("[listener] successfully sent conn to conn_tx"),
            Err(e) => tracing::debug!("[listener] FAILED to send conn to conn_tx: {:?}", e),
        }
    }

    QUIC_STATUS_SUCCESS
}

pub struct MsQuicListener {
    pub handle: HQUIC,
    _ctx_box: Box<ListenerContext>,
    pub conn_rx: mpsc::UnboundedReceiver<MsQuicConnection>,
}

unsafe impl Send for MsQuicListener {}
unsafe impl Sync for MsQuicListener {}

impl MsQuicListener {
    pub fn start(
        engine: &MsQuicEngine,
        server_config: &MsQuicConfiguration,
        port: u16,
        allowed_peer_thumbprints: Vec<String>,
    ) -> Result<Self> {
        tracing::debug!("[engine] Getting API");
        let api = super::ffi::get_api()?;
        tracing::debug!("[engine] Got API");

        let (conn_tx, conn_rx) = mpsc::unbounded_channel();
        let mut ctx_box = Box::new(ListenerContext {
            conn_tx,
            server_config: server_config.handle,
            allowed_peer_thumbprints,
        });

        let mut listener: HQUIC = ptr::null_mut();
        let status = unsafe {
            (api.ListenerOpen.unwrap())(
                engine.registration,
                Some(listener_callback),
                ctx_box.as_mut() as *mut ListenerContext as *mut std::ffi::c_void,
                &mut listener,
            )
        };

        if status != 0 {
            anyhow::bail!("ListenerOpen failed with status 0x{:08x}", status);
        }

        let alpn_str = b"fileShare";
        let alpn = QUIC_BUFFER {
            Length: alpn_str.len() as u32,
            Buffer: alpn_str.as_ptr() as *mut u8,
        };

        // Bind address
        let std_sock = std::net::SocketAddrV4::new(std::net::Ipv4Addr::UNSPECIFIED, port);
        let sock_addr: std::net::SocketAddr = std_sock.into();
        let addr_storage = socket2::SockAddr::from(sock_addr);
        let quic_addr_ptr = addr_storage.as_ptr() as *const QUIC_ADDR;

        let status = unsafe { (api.ListenerStart.unwrap())(listener, &alpn, 1, quic_addr_ptr) };

        if status != 0 {
            anyhow::bail!("ListenerStart failed with status 0x{:08x}", status);
        }

        Ok(Self {
            handle: listener,
            _ctx_box: ctx_box,
            conn_rx,
        })
    }

    pub async fn accept(&mut self) -> Option<MsQuicConnection> {
        self.conn_rx.recv().await
    }

    pub fn local_addr(&self) -> Result<SocketAddr> {
        tracing::debug!("[engine] Getting API");
        let api = super::ffi::get_api()?;
        tracing::debug!("[engine] Got API");
        let mut len = 128u32;
        let mut buffer = [0u8; 128];

        let status = unsafe {
            (api.GetParam.unwrap())(
                self.handle,
                QUIC_PARAM_LISTENER_LOCAL_ADDRESS,
                &mut len,
                buffer.as_mut_ptr() as *mut std::ffi::c_void,
            )
        };

        if status != 0 {
            anyhow::bail!(
                "GetParam(QUIC_PARAM_LISTENER_LOCAL_ADDRESS) failed: 0x{:08x}",
                status
            );
        }

        let sock_addr = unsafe {
            let (_, addr) = socket2::SockAddr::try_init(|storage, actual_len| {
                let to_copy = (len as usize).min(buffer.len());
                std::ptr::copy_nonoverlapping(
                    buffer.as_ptr(),
                    storage as *mut _ as *mut u8,
                    to_copy,
                );
                *actual_len = len as _;
                Ok(())
            })?;
            addr.as_socket()
                .ok_or_else(|| anyhow::anyhow!("Invalid socket addr from MsQuic"))?
        };

        Ok(sock_addr)
    }
}

impl Drop for MsQuicListener {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            if let Ok(api) = super::ffi::get_api() {
                if let Some(stop_fn) = api.ListenerStop {
                    unsafe { stop_fn(self.handle) };
                }
                if let Some(close_fn) = api.ListenerClose {
                    unsafe { close_fn(self.handle) };
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    #[ignore = "MsQuic FFI event callbacks require multi-threaded runtime pump"]
    async fn test_msquic_loopback_ping_pong() -> Result<()> {
        let engine = match MsQuicEngine::new() {
            Ok(e) => e,
            Err(e) => {
                tracing::debug!(
                    "MsQuic not available on this platform/path, skipping test: {}",
                    e
                );
                return Ok(());
            }
        };

        let engine = std::sync::Arc::new(engine);
        let server_config = engine.create_configuration(true, None, None, None, None)?;
        let client_config =
            std::sync::Arc::new(engine.create_configuration(false, None, None, None, None)?);

        let mut listener = MsQuicListener::start(&engine, &server_config, 0, Vec::new())?;
        let local_addr = listener.local_addr()?;
        tracing::debug!("MsQuicListener bound to: {}", local_addr);

        let engine_clone = engine.clone();
        let client_config_clone = client_config.clone();
        let client_task = tokio::spawn(async move {
            MsQuicConnection::connect(&engine_clone, &client_config_clone, local_addr, None, None)
                .await
        });

        let mut server_conn = listener.accept().await.expect("Server accept failed");
        let client_conn = client_task.await??;

        let client_stream = client_conn.open_stream().await?;
        let (mut client_recv, mut client_send) = client_stream.split();

        client_send.write_all(b"PING!").await?;

        let server_stream = server_conn
            .accept_stream()
            .await
            .expect("Server stream accept failed");
        let (mut server_recv, mut server_send) = server_stream.split();

        let mut buf = [0u8; 5];
        server_recv.read_exact(&mut buf).await?;
        assert_eq!(&buf, b"PING!");

        server_send.write_all(b"PONG!").await?;

        let mut resp = [0u8; 5];
        client_recv.read_exact(&mut resp).await?;
        assert_eq!(&resp, b"PONG!");

        Ok(())
    }
}
