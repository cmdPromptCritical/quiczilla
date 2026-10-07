#![no_main]

use libfuzzer_sys::fuzz_target;
use quiczilla_core::directory::validate_relative_path;
use std::path::{Component, Path};

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        if validate_relative_path(s).is_ok() {
            let p = Path::new(s);
            assert!(p.is_relative(), "Accepted path must be relative: {s}");
            assert!(!p.has_root(), "Accepted path must not have root: {s}");
            assert!(!s.contains('\0'), "Accepted path must not contain null: {s}");
            assert!(!s.contains('\\'), "Accepted path must not contain backslash: {s}");

            for comp in p.components() {
                match comp {
                    Component::Normal(name) => {
                        assert_ne!(name, ".", "Component must not be dot: {s}");
                        assert_ne!(name, "..", "Component must not be dotdot: {s}");
                    }
                    _ => panic!("Accepted invalid non-normal path component: {comp:?}"),
                }
            }
        }
    }
});
