use anyhow::{Context, Result};
use rcgen::{CertificateParams, ExtendedKeyUsagePurpose, KeyPair};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{ClientConfig, ServerConfig, SignatureScheme};
use sha1::{Digest, Sha1};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub fn generate_self_signed_cert() -> Result<(CertificateDer<'static>, PrivateKeyDer<'static>)> {
    let mut params = CertificateParams::new(vec![
        "quicshare".to_string(),
        "192.0.2.44".to_string(),
        "localhost".to_string(),
        "127.0.0.1".to_string(),
    ])?;
    params.not_before = rcgen::date_time_ymd(2020, 1, 1);
    params.not_after = rcgen::date_time_ymd(2120, 1, 1);
    params.extended_key_usages = vec![
        ExtendedKeyUsagePurpose::ServerAuth,
        ExtendedKeyUsagePurpose::ClientAuth,
    ];

    let key_pair = KeyPair::generate()?;
    let cert = params.self_signed(&key_pair)?;
    let cert_der = cert.der().clone();

    let private_key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_pair.serialize_der()));

    Ok((cert_der, private_key))
}

pub fn compute_thumbprint(cert_der: &[u8]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(cert_der);
    let result = hasher.finalize();
    hex::encode(result).to_uppercase()
}

/// Normalize a SHA-1 certificate thumbprint for stable comparisons and files.
pub fn normalize_thumbprint(value: &str) -> Result<String> {
    let normalized: String = value
        .chars()
        .filter(|character| !character.is_ascii_whitespace() && *character != ':')
        .collect::<String>()
        .to_uppercase();
    if normalized.len() != 40
        || !normalized
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        anyhow::bail!("certificate thumbprint must contain exactly 40 hexadecimal characters");
    }
    Ok(normalized)
}

pub fn default_identity_dir(role: &str) -> Result<PathBuf> {
    #[cfg(windows)]
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .context("LOCALAPPDATA is not set")?;

    #[cfg(not(windows))]
    let base = if let Some(config_home) = std::env::var_os("XDG_CONFIG_HOME") {
        PathBuf::from(config_home)
    } else {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .context("neither XDG_CONFIG_HOME nor HOME is set")?
            .join(".config")
    };

    Ok(base.join("quiczilla").join(role))
}

#[derive(Debug)]
pub struct ThumbprintServerCertVerifier {
    expected_thumbprint: String,
}

impl ThumbprintServerCertVerifier {
    pub fn new(expected_thumbprint: String) -> Self {
        Self {
            expected_thumbprint,
        }
    }
}

impl ServerCertVerifier for ThumbprintServerCertVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let actual = compute_thumbprint(end_entity.as_ref());
        if actual.eq_ignore_ascii_case(&self.expected_thumbprint) {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General(
                "Certificate thumbprint mismatch".into(),
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![
            SignatureScheme::ECDSA_NISTP256_SHA256,
            SignatureScheme::ECDSA_NISTP384_SHA384,
            SignatureScheme::ED25519,
            SignatureScheme::RSA_PKCS1_SHA256,
            SignatureScheme::RSA_PSS_SHA256,
        ]
    }
}

#[derive(Debug)]
pub struct ThumbprintClientCertVerifier {
    allowed_thumbprints: Vec<String>,
}

impl ThumbprintClientCertVerifier {
    pub fn new(expected_thumbprint: String) -> Self {
        Self {
            allowed_thumbprints: vec![expected_thumbprint],
        }
    }

    pub fn new_multi(allowed_thumbprints: Vec<String>) -> Self {
        Self {
            allowed_thumbprints,
        }
    }
}

impl ClientCertVerifier for ThumbprintClientCertVerifier {
    fn offer_client_auth(&self) -> bool {
        true
    }

    fn client_auth_mandatory(&self) -> bool {
        true
    }

    fn root_hint_subjects(&self) -> &[rustls::DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        let actual = compute_thumbprint(end_entity.as_ref());
        if self
            .allowed_thumbprints
            .iter()
            .any(|t| actual.eq_ignore_ascii_case(t))
        {
            Ok(ClientCertVerified::assertion())
        } else {
            Err(rustls::Error::General(
                "Certificate thumbprint not authorized".into(),
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![
            SignatureScheme::ECDSA_NISTP256_SHA256,
            SignatureScheme::ECDSA_NISTP384_SHA384,
            SignatureScheme::ED25519,
            SignatureScheme::RSA_PKCS1_SHA256,
            SignatureScheme::RSA_PSS_SHA256,
        ]
    }
}

pub fn build_client_config(
    own_cert: CertificateDer<'static>,
    own_key: PrivateKeyDer<'static>,
    expected_server_thumbprint: String,
) -> Result<ClientConfig> {
    let verifier = Arc::new(ThumbprintServerCertVerifier::new(
        expected_server_thumbprint,
    ));

    let mut config = ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_client_auth_cert(vec![own_cert], own_key)
        .map_err(|e| anyhow::anyhow!("Failed to build client config: {}", e))?;

    config.alpn_protocols = vec![crate::types::ALPN_PROTOCOL.to_vec()];

    Ok(config)
}

pub fn build_server_config(
    own_cert: CertificateDer<'static>,
    own_key: PrivateKeyDer<'static>,
    expected_client_thumbprint: String,
) -> Result<ServerConfig> {
    build_server_config_multi(own_cert, own_key, vec![expected_client_thumbprint])
}

pub fn build_server_config_multi(
    own_cert: CertificateDer<'static>,
    own_key: PrivateKeyDer<'static>,
    allowed_client_thumbprints: Vec<String>,
) -> Result<ServerConfig> {
    let verifier = Arc::new(ThumbprintClientCertVerifier::new_multi(
        allowed_client_thumbprints,
    ));

    let mut config = ServerConfig::builder()
        .with_client_cert_verifier(verifier)
        .with_single_cert(vec![own_cert], own_key)
        .map_err(|e| anyhow::anyhow!("Failed to build server config: {}", e))?;

    config.alpn_protocols = vec![crate::types::ALPN_PROTOCOL.to_vec()];

    Ok(config)
}

pub fn generate_msquic_pkcs12(password: &str) -> Result<(String, Vec<u8>)> {
    let mut params = CertificateParams::new(vec![
        "quicshare".to_string(),
        "192.0.2.44".to_string(),
        "localhost".to_string(),
        "127.0.0.1".to_string(),
    ])?;
    params.not_before = rcgen::date_time_ymd(2020, 1, 1);
    params.not_after = rcgen::date_time_ymd(2120, 1, 1);
    params.extended_key_usages = vec![
        ExtendedKeyUsagePurpose::ServerAuth,
        ExtendedKeyUsagePurpose::ClientAuth,
    ];

    let key_pair = KeyPair::generate()?;
    let cert = params.self_signed(&key_pair)?;
    let cert_der = cert.der().clone();
    let key_der = key_pair.serialize_der();

    let thumbprint = compute_thumbprint(&cert_der);

    let pfx = p12::PFX::new(&cert_der, &key_der, None, password, "quicshare")
        .ok_or_else(|| anyhow::anyhow!("Failed to generate PKCS12"))?;

    Ok((thumbprint, pfx.to_der()))
}

pub fn generate_msquic_pem() -> Result<(String, String, String)> {
    let mut params = CertificateParams::new(vec![
        "quicshare".to_string(),
        "192.0.2.44".to_string(),
        "localhost".to_string(),
        "127.0.0.1".to_string(),
    ])?;
    params.not_before = rcgen::date_time_ymd(2020, 1, 1);
    params.not_after = rcgen::date_time_ymd(2120, 1, 1);
    params.extended_key_usages = vec![
        ExtendedKeyUsagePurpose::ServerAuth,
        ExtendedKeyUsagePurpose::ClientAuth,
    ];

    let key_pair = KeyPair::generate()?;
    let cert = params.self_signed(&key_pair)?;
    let thumbprint = compute_thumbprint(cert.der());

    Ok((thumbprint, cert.pem(), key_pair.serialize_pem()))
}

use crate::msquic::engine::{MsQuicConfiguration, MsQuicEngine};

#[cfg(windows)]
pub fn build_quic_config(
    engine: &MsQuicEngine,
    is_server: bool,
) -> Result<(String, MsQuicConfiguration)> {
    let role = if is_server { "server" } else { "client" };
    build_persistent_quic_config(engine, is_server, &default_identity_dir(role)?)
}

#[cfg(not(windows))]
pub fn build_quic_config(
    engine: &MsQuicEngine,
    is_server: bool,
) -> Result<(String, MsQuicConfiguration)> {
    let static_cert = std::path::Path::new("/tmp/quiczilla_static_cert.pem");
    let static_key = std::path::Path::new("/tmp/quiczilla_static_key.pem");
    if static_cert.exists() && static_key.exists() {
        let thumbprint = std::fs::read_to_string("/tmp/quiczilla_static_thumbprint.txt")
            .unwrap_or_else(|_| "94510B2219EB97320BDE08C6FE9A06F4917F3E2A".to_string())
            .trim()
            .to_string();
        let cert_path_str = static_cert.to_string_lossy().into_owned();
        let key_path_str = static_key.to_string_lossy().into_owned();
        let config = engine.create_configuration(
            is_server,
            None,
            None,
            Some(&cert_path_str),
            Some(&key_path_str),
        )?;
        return Ok((thumbprint, config));
    }
    let (local_thumbprint, cert_pem, key_pem) = generate_msquic_pem()?;
    let cert_path = std::env::temp_dir().join("quiczilla_cert.pem");
    let key_path = std::env::temp_dir().join("quiczilla_key.pem");
    std::fs::write(&cert_path, cert_pem)?;
    std::fs::write(&key_path, key_pem)?;
    let cert_path_str = cert_path.to_string_lossy().into_owned();
    let key_path_str = key_path.to_string_lossy().into_owned();
    let config = engine.create_configuration(
        is_server,
        None,
        None,
        Some(&cert_path_str),
        Some(&key_path_str),
    )?;
    Ok((local_thumbprint, config))
}

/// Load or create a stable identity for persistent daemon authentication.
/// The directory should be private to the current user.
#[cfg(windows)]
pub fn build_persistent_quic_config(
    engine: &MsQuicEngine,
    is_server: bool,
    identity_dir: &Path,
) -> Result<(String, MsQuicConfiguration)> {
    std::fs::create_dir_all(identity_dir).with_context(|| {
        format!(
            "failed to create identity directory {}",
            identity_dir.display()
        )
    })?;
    let thumbprint_path = identity_dir.join("thumbprint");

    if thumbprint_path.is_file() {
        let thumbprint = normalize_thumbprint(
            &std::fs::read_to_string(&thumbprint_path)
                .with_context(|| format!("failed to read {}", thumbprint_path.display()))?,
        )?;
        if let Ok(config) =
            engine.create_configuration_from_hash(is_server, thumbprint_bytes(&thumbprint)?)
        {
            return Ok((thumbprint, config));
        }
    }

    let thumbprint = create_windows_store_identity()?;
    std::fs::write(&thumbprint_path, format!("{thumbprint}\n"))
        .with_context(|| format!("failed to write {}", thumbprint_path.display()))?;
    let config =
        engine.create_configuration_from_hash(is_server, thumbprint_bytes(&thumbprint)?)?;
    Ok((thumbprint, config))
}

#[cfg(windows)]
fn thumbprint_bytes(thumbprint: &str) -> Result<[u8; 20]> {
    let mut bytes = [0u8; 20];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&thumbprint[index * 2..index * 2 + 2], 16)?;
    }
    Ok(bytes)
}

#[cfg(windows)]
fn create_windows_store_identity() -> Result<String> {
    use std::os::windows::process::CommandExt as _;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let script = concat!(
        "$ErrorActionPreference='Stop'; Import-Module PKI; ",
        "$cert=New-SelfSignedCertificate -Type Custom ",
        "-Subject 'CN=Quiczilla Identity' ",
        "-FriendlyName 'Quiczilla mTLS Identity' ",
        "-TextExtension @('2.5.29.37={text}1.3.6.1.5.5.7.3.1,1.3.6.1.5.5.7.3.2') ",
        "-KeyUsage DigitalSignature -KeyAlgorithm RSA -KeyLength 2048 ",
        "-KeyExportPolicy NonExportable -NotAfter (Get-Date).AddYears(20) ",
        "-CertStoreLocation 'Cert:\\CurrentUser\\My'; ",
        "$cert.Thumbprint"
    );
    let output = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .env_remove("PSModulePath")
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .context("failed to start PowerShell for Windows certificate creation")?;
    if !output.status.success() {
        anyhow::bail!(
            "failed to create Windows certificate-store identity: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    normalize_thumbprint(String::from_utf8_lossy(&output.stdout).trim())
}

/// Load or create a stable identity for persistent daemon authentication.
/// The private key is written mode 0600 on Unix.
#[cfg(not(windows))]
pub fn build_persistent_quic_config(
    engine: &MsQuicEngine,
    is_server: bool,
    identity_dir: &Path,
) -> Result<(String, MsQuicConfiguration)> {
    use std::io::Write as _;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    std::fs::create_dir_all(identity_dir).with_context(|| {
        format!(
            "failed to create identity directory {}",
            identity_dir.display()
        )
    })?;
    std::fs::set_permissions(identity_dir, std::fs::Permissions::from_mode(0o700))?;

    let cert_path = identity_dir.join("identity.pem");
    let key_path = identity_dir.join("identity-key.pem");
    let thumbprint_path = identity_dir.join("thumbprint");

    if !cert_path.is_file() || !key_path.is_file() || !thumbprint_path.is_file() {
        let (thumbprint, cert_pem, key_pem) = generate_msquic_pem()?;
        std::fs::write(&cert_path, cert_pem)
            .with_context(|| format!("failed to write {}", cert_path.display()))?;
        let mut key_file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&key_path)
            .with_context(|| format!("failed to write {}", key_path.display()))?;
        key_file.write_all(key_pem.as_bytes())?;
        std::fs::write(&thumbprint_path, format!("{thumbprint}\n"))
            .with_context(|| format!("failed to write {}", thumbprint_path.display()))?;
    }

    let thumbprint = normalize_thumbprint(
        &std::fs::read_to_string(&thumbprint_path)
            .with_context(|| format!("failed to read {}", thumbprint_path.display()))?,
    )?;
    let cert_path = cert_path.to_string_lossy().into_owned();
    let key_path = key_path.to_string_lossy().into_owned();
    let config =
        engine.create_configuration(is_server, None, None, Some(&cert_path), Some(&key_path))?;
    Ok((thumbprint, config))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cert_generation_and_thumbprint() {
        let (cert, _key) = generate_self_signed_cert().expect("failed to generate cert");
        let thumbprint = compute_thumbprint(cert.as_ref());

        // .NET X509Certificate2.Thumbprint is 40 uppercase hex characters (SHA-1)
        assert_eq!(thumbprint.len(), 40);
        assert!(
            thumbprint
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_lowercase())
        );
    }

    #[test]
    fn test_client_and_server_config_builders() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let (cert1, key1) = generate_self_signed_cert().unwrap();
        let (cert2, key2) = generate_self_signed_cert().unwrap();
        let thumb1 = compute_thumbprint(cert1.as_ref());
        let thumb2 = compute_thumbprint(cert2.as_ref());

        let client_cfg = build_client_config(cert1, key1, thumb2).unwrap();
        assert_eq!(client_cfg.alpn_protocols, vec![b"fileShare".to_vec()]);

        let server_cfg = build_server_config(cert2, key2, thumb1).unwrap();
        assert_eq!(server_cfg.alpn_protocols, vec![b"fileShare".to_vec()]);
    }

    #[test]
    fn test_multi_thumbprint_verifier() {
        let (cert1, _) = generate_self_signed_cert().unwrap();
        let (cert2, _) = generate_self_signed_cert().unwrap();
        let (cert3_rogue, _) = generate_self_signed_cert().unwrap();

        let thumb1 = compute_thumbprint(cert1.as_ref());
        let thumb2 = compute_thumbprint(cert2.as_ref());

        let verifier = ThumbprintClientCertVerifier::new_multi(vec![thumb1, thumb2]);
        let now = UnixTime::now();

        // Cert 1 should pass
        assert!(verifier.verify_client_cert(&cert1, &[], now).is_ok());

        // Cert 2 should pass
        assert!(verifier.verify_client_cert(&cert2, &[], now).is_ok());

        // Rogue Cert 3 should fail
        assert!(verifier.verify_client_cert(&cert3_rogue, &[], now).is_err());
    }

    #[test]
    fn test_thumbprint_normalization_and_validation() {
        let raw = "72:0a:7336:6d92:1f1e:7feb:13e3:bb75:8afa:3e34:753e";
        assert_eq!(
            normalize_thumbprint(raw).unwrap(),
            "720A73366D921F1E7FEB13E3BB758AFA3E34753E"
        );
        assert!(normalize_thumbprint("not-a-thumbprint").is_err());
    }
}
