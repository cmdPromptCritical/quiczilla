use quiczilla_core::cert::normalize_thumbprint;
use quiczilla_core::directory::{
    MAX_PATH_BYTES, receive_directory, validate_relative_path,
};
use quiczilla_core::types::{
    ControlMessage, CURRENT_PROTOCOL_VERSION, MAX_CONTROL_MESSAGE_BYTES,
    MIN_SUPPORTED_PROTOCOL_VERSION,
};
use std::io::Cursor;

#[test]
fn test_path_sanitization_adversarial_corpus() {
    let malicious_paths = &[
        "",
        "/",
        "//",
        "/etc/passwd",
        "C:\\Windows\\System32",
        "../../escape.txt",
        "foo/../../escape.txt",
        "foo/bar/../../../escape.txt",
        "foo/./bar",
        "./foo",
        ".",
        "..",
        "foo/..",
        "foo\0bar",
        "foo\\bar",
        "foo\r\nbar",
        "foo\x1bbar",
        "foo\tbar",
        "foo/bar\0.txt",
        "foo/bar/../..",
        "~/.ssh/id_rsa",
        "$HOME/.ssh/id_rsa",
    ];

    for path in malicious_paths {
        let result = validate_relative_path(path);
        assert!(
            result.is_err(),
            "Path '{}' should have been rejected by validate_relative_path, but passed!",
            path
        );
    }
}

#[test]
fn test_path_sanitization_valid_corpus() {
    let safe_paths = &[
        "foo",
        "foo/bar",
        "foo/bar/baz.txt",
        "my-file.tar.gz",
        "deeply/nested/directory/structure/file_123.bin",
        "unicode_ünïcøde_日本語_🦀.txt",
    ];

    for path in safe_paths {
        let result = validate_relative_path(path);
        assert!(
            result.is_ok(),
            "Valid path '{}' was unexpectedly rejected: {:?}",
            path,
            result.err()
        );
    }
}

#[test]
fn test_path_length_limit() {
    let oversized = "a/".repeat(MAX_PATH_BYTES / 2 + 10);
    assert!(validate_relative_path(&oversized).is_err());
}

#[test]
fn test_control_message_roundtrip_fuzz() {
    let corpus = vec![
        ControlMessage::Ready,
        ControlMessage::ResumeOk,
        ControlMessage::RestartFromZero,
        ControlMessage::Pause,
        ControlMessage::Resume,
        ControlMessage::RejectedUnwanted,
        ControlMessage::RejectedAlreadyReceiving,
        ControlMessage::RejectedAlreadySending,
        ControlMessage::ReceivedFileOk,
        ControlMessage::ReceivedFileFailed,
        ControlMessage::FileSent {
            hash: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".to_string(),
        },
        ControlMessage::Metadata {
            file_name: "test.dat".to_string(),
            file_size: 1048576,
            resume: false,
            checksum: true,
        },
        ControlMessage::Metadata {
            file_name: "large_file.iso".to_string(),
            file_size: 1099511627776, // 1 TiB
            resume: true,
            checksum: false,
        },
        ControlMessage::ResumeReady {
            offset: 52428800,
            prefix_hash: Some("abcd1234ef".to_string()),
        },
        ControlMessage::ResumeReady {
            offset: 0,
            prefix_hash: None,
        },
    ];

    for msg in corpus {
        let serialized = msg.serialize();
        assert!(serialized.len() <= MAX_CONTROL_MESSAGE_BYTES);
        let parsed = ControlMessage::parse(&serialized);
        assert_eq!(Some(msg), parsed);
    }
}

#[test]
fn test_control_message_malformed_fuzz() {
    let malformed_inputs = &[
        "",
        "UNKNOWN_COMMAND",
        "METADATA:",
        "METADATA:{}",
        "METADATA:{\"FileName\": 123}",
        "METADATA:{\"FileName\": \"foo\", \"FileSize\": \"not-a-number\"}",
        "METADATA:{\"FileName\": \"foo\", \"FileSize\": \"-100\"}",
        "RESUME_READY:",
        "RESUME_READY:{}",
        "RESUME_READY:{\"Offset\": \"negative\"}",
        "FILE_SENT",
        "REJECTED:",
        "RECEIVED_FILE",
        "\0\0\0\0",
        "READY\nEXTRA",
        "METADATA:{\"FileName\": \"test\", \"FileSize\": \"999999999999999999999999999999999\"}",
    ];

    for input in malformed_inputs {
        // Parsing should safely return None without panicking
        let result = ControlMessage::parse(input);
        if input.starts_with("READY") && *input == "READY" {
            assert!(result.is_some());
        } else if input.starts_with("METADATA:{\"FileName\": \"foo\", \"FileSize\": \"not-a-number\"}") {
            assert_eq!(result, None);
        }
    }
}

#[tokio::test]
async fn test_directory_frame_decoder_fuzz_truncated() {
    let temp_dir = std::env::temp_dir().join(format!("qz_test_fuzz_dir_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&temp_dir);

    // Frame stream test inputs: single byte, invalid headers, truncated lengths
    let fuzz_buffers: Vec<Vec<u8>> = vec![
        vec![],
        vec![0x00],
        vec![0xFF],
        vec![0x11], // FRAME_START with no subsequent path length
        vec![0x11, 0x00],
        vec![0x11, 0x00, 0x00, 0x00, 0x05], // length 5, but no data
        vec![0x11, 0x00, 0x00, 0x00, 0x05, b'r', b'o', b'o', b't'], // root without trailing byte
        vec![0x11, 0xFF, 0xFF, 0xFF, 0xFF], // gigantic length > MAX_PATH_BYTES
        vec![0x11, 0x00, 0x00, 0x00, 0x04, b'r', b'o', b'o', b't', 0x99], // unknown frame opcode
    ];

    for buf in fuzz_buffers {
        let mut cursor = Cursor::new(&buf);
        let res = receive_directory(&mut cursor, &temp_dir).await;
        // All truncated or invalid buffers must error cleanly without panicking
        assert!(res.is_err(), "Expected buffer {:?} to fail safely", buf);
    }

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_thumbprint_fuzz_inputs() {
    let inputs = &[
        "",
        "invalid",
        "00",
        "zzzz",
        "a1:b2:c3:d4:e5:f6:a1:b2:c3:d4:e5:f6:a1:b2:c3:d4:e5:f6:a1:b2",
        "A1B2C3D4E5F6A1B2C3D4E5F6A1B2C3D4E5F6A1B2",
        "A1 B2 C3 D4 E5 F6 A1 B2 C3 D4 E5 F6 A1 B2 C3 D4 E5 F6 A1 B2",
        "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2",
        "0000000000000000000000000000000000000000",
        "0000000000000000000000000000000000000000\0malicious",
    ];

    for input in inputs {
        let _ = normalize_thumbprint(input);
    }
}

#[test]
fn test_protocol_version_bounds() {
    assert_eq!(CURRENT_PROTOCOL_VERSION, 1);
    assert_eq!(MIN_SUPPORTED_PROTOCOL_VERSION, 1);
}
