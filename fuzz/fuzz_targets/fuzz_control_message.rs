#![no_main]

use libfuzzer_sys::fuzz_target;
use quiczilla_core::types::ControlMessage;

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        if let Some(msg) = ControlMessage::parse(s) {
            let serialized = msg.serialize();
            let reparsed = ControlMessage::parse(&serialized);
            assert_eq!(Some(msg), reparsed);
        }
    }
});
