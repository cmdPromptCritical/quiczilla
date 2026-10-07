#![no_main]

use libfuzzer_sys::fuzz_target;
use std::io::Cursor;

fuzz_target!(|data: &[u8]| {
    let temp_root = std::env::temp_dir().join(format!("qz_fuzz_dir_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&temp_root);

    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(_) => return,
    };

    rt.block_on(async {
        let mut cursor = Cursor::new(data);
        let _ = quiczilla_core::directory::receive_directory(&mut cursor, &temp_root).await;
    });

    let _ = std::fs::remove_dir_all(&temp_root);
});
