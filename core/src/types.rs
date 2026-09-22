use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileTransferStatus {
    Cancelled,
    Completed,
    HashFailed,
    RejectedAlreadySending,
    RejectedAlreadyReceiving,
    RejectedUnwanted,
    Etc,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProgressInfo {
    pub bytes_transferred: u64,
    pub total_bytes: u64,
    pub speed_bytes_per_second: f64,
    pub percentage: f64,
    pub estimated_remaining_secs: Option<f64>,
    pub is_completed: bool,
    pub average_speed_bytes_per_second: Option<f64>,
    pub total_time_secs: Option<f64>,
}

pub const ALPN_PROTOCOL: &[u8] = b"fileShare";
pub const CONTROL_STREAM_HEADER: u8 = 0x01;
pub const FILE_STREAM_HEADER: u8 = 0x02;
pub const PIPE_STREAM_HEADER: u8 = 0x03;
pub const STREAM_ERROR_CODE: u32 = 0x0A;
pub const CLOSE_ERROR_CODE: u32 = 0x0B;
pub const IDLE_TIMEOUT_SECS: u64 = 30;
pub const KEEPALIVE_INTERVAL_SECS: u64 = 2;
pub const FILE_CHUNK_SIZE: usize = 2 * 1024 * 1024; // 2 MiB
pub const FILE_BUFFER_SIZE: usize = 32 * 1024 * 1024; // 32 MiB
pub const HASH_CHANNEL_CAPACITY: usize = 128;
pub const HOLE_PUNCH_PROBES: usize = 5;
pub const PROGRESS_REPORT_INTERVAL_MS: u64 = 500;
pub const SPEED_ESTIMATION_INTERVAL_MS: u64 = 2000;
pub const LIVENESS_CHECK_INTERVAL_SECS: u64 = 2;
pub const HTTP_TIMEOUT_SECS: u64 = 5;
pub const RESUME_THRESHOLD_BYTES: u64 = 20 * 1024 * 1024; // 20 MiB
pub const RESUME_FINGERPRINT_SIZE: usize = 64 * 1024; // 64 KiB
pub const PARTIAL_FILE_SUFFIX: &str = ".quic-part";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlMessage {
    Metadata {
        file_name: String,
        file_size: u64,
        resume: bool,
        checksum: bool,
    },
    Ready,
    ResumeReady {
        offset: u64,
        prefix_hash: Option<String>,
    },
    ResumeOk,
    RestartFromZero,
    Pause,
    Resume,
    RejectedUnwanted,
    RejectedAlreadyReceiving,
    RejectedAlreadySending,
    FileSent {
        hash: String,
    },
    ReceivedFileOk,
    ReceivedFileFailed,
}

#[derive(Serialize, Deserialize)]
struct MetadataPayload {
    #[serde(rename = "FileName")]
    file_name: String,
    #[serde(rename = "FileSize")]
    file_size: String,
    #[serde(rename = "Resume", default)]
    resume: bool,
    #[serde(rename = "Checksum", default)]
    checksum: bool,
}

#[derive(Serialize, Deserialize)]
struct ResumeReadyPayload {
    #[serde(rename = "Offset")]
    offset: String,
    #[serde(rename = "PrefixHash", skip_serializing_if = "Option::is_none")]
    prefix_hash: Option<String>,
}

impl ControlMessage {
    pub fn serialize(&self) -> String {
        match self {
            ControlMessage::Metadata {
                file_name,
                file_size,
                resume,
                checksum,
            } => {
                let payload = MetadataPayload {
                    file_name: file_name.clone(),
                    file_size: file_size.to_string(),
                    resume: *resume,
                    checksum: *checksum,
                };
                let json = serde_json::to_string(&payload).unwrap();
                format!("METADATA:{}", json)
            }
            ControlMessage::Ready => "READY".to_string(),
            ControlMessage::ResumeReady {
                offset,
                prefix_hash,
            } => {
                let payload = ResumeReadyPayload {
                    offset: offset.to_string(),
                    prefix_hash: prefix_hash.clone(),
                };
                let json = serde_json::to_string(&payload).unwrap();
                format!("RESUME_READY:{}", json)
            }
            ControlMessage::ResumeOk => "RESUME_OK".to_string(),
            ControlMessage::RestartFromZero => "RESTART_FROM_ZERO".to_string(),
            ControlMessage::Pause => "PAUSE".to_string(),
            ControlMessage::Resume => "RESUME".to_string(),
            ControlMessage::RejectedUnwanted => "REJECTED:UNWANTED".to_string(),
            ControlMessage::RejectedAlreadyReceiving => "REJECTED:ALREADY_RECEIVING".to_string(),
            ControlMessage::RejectedAlreadySending => "REJECTED:ALREADY_SENDING".to_string(),
            ControlMessage::FileSent { hash } => format!("FILE_SENT:{}", hash),
            ControlMessage::ReceivedFileOk => "RECEIVED_FILE:OK".to_string(),
            ControlMessage::ReceivedFileFailed => "RECEIVED_FILE:FAILED".to_string(),
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        if s == "READY" {
            Some(ControlMessage::Ready)
        } else if s == "RESUME_OK" {
            Some(ControlMessage::ResumeOk)
        } else if s == "RESTART_FROM_ZERO" {
            Some(ControlMessage::RestartFromZero)
        } else if s == "PAUSE" {
            Some(ControlMessage::Pause)
        } else if s == "RESUME" {
            Some(ControlMessage::Resume)
        } else if s == "REJECTED:UNWANTED" {
            Some(ControlMessage::RejectedUnwanted)
        } else if s == "REJECTED:ALREADY_RECEIVING" {
            Some(ControlMessage::RejectedAlreadyReceiving)
        } else if s == "REJECTED:ALREADY_SENDING" {
            Some(ControlMessage::RejectedAlreadySending)
        } else if s == "RECEIVED_FILE:OK" {
            Some(ControlMessage::ReceivedFileOk)
        } else if s == "RECEIVED_FILE:FAILED" {
            Some(ControlMessage::ReceivedFileFailed)
        } else if let Some(json) = s.strip_prefix("METADATA:") {
            if let Ok(payload) = serde_json::from_str::<MetadataPayload>(json) {
                if let Ok(size) = payload.file_size.parse::<u64>() {
                    Some(ControlMessage::Metadata {
                        file_name: payload.file_name,
                        file_size: size,
                        resume: payload.resume,
                        checksum: payload.checksum,
                    })
                } else {
                    None
                }
            } else {
                None
            }
        } else if let Some(json) = s.strip_prefix("RESUME_READY:") {
            if let Ok(payload) = serde_json::from_str::<ResumeReadyPayload>(json) {
                if let Ok(offset) = payload.offset.parse::<u64>() {
                    Some(ControlMessage::ResumeReady {
                        offset,
                        prefix_hash: payload.prefix_hash,
                    })
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            s.strip_prefix("FILE_SENT:")
                .map(|hash| ControlMessage::FileSent {
                    hash: hash.to_string(),
                })
        }
    }
}

pub fn get_part_path(path: &std::path::Path) -> std::path::PathBuf {
    let mut file_name = path.file_name().unwrap_or_default().to_os_string();
    file_name.push(PARTIAL_FILE_SUFFIX);
    path.with_file_name(file_name)
}

pub async fn compute_prefix_hash(
    path: &std::path::Path,
    max_bytes: usize,
) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};
    use tokio::io::AsyncReadExt;
    let mut file = tokio::fs::File::open(path).await?;
    let mut buf = vec![0u8; max_bytes];
    let n = file.read(&mut buf).await?;
    let mut hasher = Sha256::new();
    hasher.update(&buf[..n]);
    Ok(hex::encode(hasher.finalize()).to_lowercase())
}

pub fn compute_file_sha256_blocking(path: &std::path::Path) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 2 * 1024 * 1024]; // 2 MiB chunks
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(hex::encode(hasher.finalize()).to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_control_message_wire_format() {
        // Metadata
        let msg = ControlMessage::Metadata {
            file_name: "test.zip".to_string(),
            file_size: 1048576,
            resume: true,
            checksum: true,
        };
        let serialized = msg.serialize();
        assert!(serialized.starts_with("METADATA:"));
        let parsed = ControlMessage::parse(&serialized).expect("failed to parse metadata");
        match parsed {
            ControlMessage::Metadata {
                file_name,
                file_size,
                resume,
                checksum,
            } => {
                assert_eq!(file_name, "test.zip");
                assert_eq!(file_size, 1048576);
                assert!(resume);
                assert!(checksum);
            }
            _ => panic!("Expected metadata variant"),
        }

        // Ready
        assert_eq!(ControlMessage::Ready.serialize(), "READY");
        assert!(matches!(
            ControlMessage::parse("READY"),
            Some(ControlMessage::Ready)
        ));

        // ResumeReady
        let resume_msg = ControlMessage::ResumeReady {
            offset: 20971520,
            prefix_hash: Some("abcd1234ef56".to_string()),
        };
        let res_ser = resume_msg.serialize();
        assert!(res_ser.starts_with("RESUME_READY:"));
        match ControlMessage::parse(&res_ser).expect("failed to parse resume ready") {
            ControlMessage::ResumeReady {
                offset,
                prefix_hash,
            } => {
                assert_eq!(offset, 20971520);
                assert_eq!(prefix_hash.as_deref(), Some("abcd1234ef56"));
            }
            _ => panic!("Expected ResumeReady variant"),
        }

        // Pause / Resume / Restart
        assert_eq!(ControlMessage::Pause.serialize(), "PAUSE");
        assert!(matches!(
            ControlMessage::parse("PAUSE"),
            Some(ControlMessage::Pause)
        ));
        assert_eq!(ControlMessage::Resume.serialize(), "RESUME");
        assert!(matches!(
            ControlMessage::parse("RESUME"),
            Some(ControlMessage::Resume)
        ));
        assert_eq!(
            ControlMessage::RestartFromZero.serialize(),
            "RESTART_FROM_ZERO"
        );
        assert!(matches!(
            ControlMessage::parse("RESTART_FROM_ZERO"),
            Some(ControlMessage::RestartFromZero)
        ));

        // Rejections
        assert_eq!(
            ControlMessage::RejectedUnwanted.serialize(),
            "REJECTED:UNWANTED"
        );
        assert_eq!(
            ControlMessage::RejectedAlreadyReceiving.serialize(),
            "REJECTED:ALREADY_RECEIVING"
        );
        assert_eq!(
            ControlMessage::RejectedAlreadySending.serialize(),
            "REJECTED:ALREADY_SENDING"
        );
        assert!(matches!(
            ControlMessage::parse("REJECTED:UNWANTED"),
            Some(ControlMessage::RejectedUnwanted)
        ));

        // File Sent
        let file_sent = ControlMessage::FileSent {
            hash: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".to_string(),
        };
        assert_eq!(
            file_sent.serialize(),
            "FILE_SENT:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        match ControlMessage::parse("FILE_SENT:abc123hash").unwrap() {
            ControlMessage::FileSent { hash } => assert_eq!(hash, "abc123hash"),
            _ => panic!("Expected FileSent"),
        }

        // Received File
        assert_eq!(
            ControlMessage::ReceivedFileOk.serialize(),
            "RECEIVED_FILE:OK"
        );
        assert_eq!(
            ControlMessage::ReceivedFileFailed.serialize(),
            "RECEIVED_FILE:FAILED"
        );
        assert!(matches!(
            ControlMessage::parse("RECEIVED_FILE:OK"),
            Some(ControlMessage::ReceivedFileOk)
        ));
        assert!(matches!(
            ControlMessage::parse("RECEIVED_FILE:FAILED"),
            Some(ControlMessage::ReceivedFileFailed)
        ));
    }
}
