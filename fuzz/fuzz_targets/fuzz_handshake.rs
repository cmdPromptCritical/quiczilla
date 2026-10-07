#![no_main]

use libfuzzer_sys::fuzz_target;
use quiczilla_core::cert::normalize_thumbprint;

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = normalize_thumbprint(s);

        if let Ok(val) = serde_json::from_str::<serde_json::Value>(s) {
            let status = val.get("status").and_then(|v| v.as_str());
            let port = val.get("udp_port").and_then(|v| v.as_u64());
            let thumbprint = val.get("thumbprint").and_then(|v| v.as_str());
            let version = val.get("protocol_version").and_then(|v| v.as_u64());

            if let Some(tp) = thumbprint {
                let _ = normalize_thumbprint(tp);
            }

            if status == Some("ready") || status == Some("daemon_ready") {
                if let (Some(p), Some(v)) = (port, version) {
                    assert!(p <= 65535, "Port must fit in u16");
                    let _ = v < quiczilla_core::types::MIN_SUPPORTED_PROTOCOL_VERSION as u64;
                }
            }
        }
    }
});
