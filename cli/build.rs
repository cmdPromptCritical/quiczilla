use std::{
    env, fs,
    path::{Path, PathBuf},
};

fn system_linux_msquic() -> Option<PathBuf> {
    if !cfg!(target_os = "linux") {
        return None;
    }

    let mut candidates = vec![
        PathBuf::from("/usr/lib/x86_64-linux-gnu/libmsquic.so"),
        PathBuf::from("/usr/lib/x86_64-linux-gnu/libmsquic.so.2"),
        PathBuf::from("/usr/local/lib/libmsquic.so"),
        PathBuf::from("/usr/local/lib/libmsquic.so.2"),
        PathBuf::from("/usr/lib/libmsquic.so"),
        PathBuf::from("/usr/lib/libmsquic.so.2"),
    ];
    if let Some(home) = env::var_os("HOME") {
        let local_lib = PathBuf::from(home).join(".local/lib");
        candidates.push(local_lib.join("libmsquic.so"));
        candidates.push(local_lib.join("libmsquic.so.2"));
    }
    candidates.into_iter().find(|candidate| candidate.is_file())
}

fn emit_runtime(name: &str) {
    let manifest_dir_value =
        env::var("CARGO_MANIFEST_DIR").expect("Cargo must set CARGO_MANIFEST_DIR");
    let manifest_dir = Path::new(&manifest_dir_value);
    let embedded_source = manifest_dir.join("embedded").join(name);
    let source = if embedded_source.is_file() {
        Some(embedded_source)
    } else if name == "msquic-linux-x86_64.so" {
        // A Linux source build can use the supported system package as the
        // embedded remote-worker runtime. Official release CI stages the same
        // library explicitly; this makes local Cargo builds equally capable.
        system_linux_msquic()
    } else {
        None
    };
    if let Some(source) = &source {
        println!("cargo:rerun-if-changed={}", source.display());
    }

    let out_dir_value = env::var("OUT_DIR").expect("Cargo must set OUT_DIR");
    let output = Path::new(&out_dir_value).join(name);
    let generated = output.with_extension("rs");

    if let Some(source) = source {
        fs::copy(&source, &output).expect("copy embedded MsQuic runtime");
        fs::write(
            generated,
            format!("pub const DATA: Option<&[u8]> = Some(include_bytes!(concat!(env!(\"OUT_DIR\"), \"/{name}\")));"),
        )
        .expect("write embedded-runtime declaration");
    } else {
        // Builds without a local native runtime remain possible, but can only
        // use a pre-installed remote worker rather than deploying one.
        fs::write(generated, "pub const DATA: Option<&[u8]> = None;")
            .expect("write absent-runtime declaration");
    }
}

fn main() {
    emit_runtime("msquic-linux-x86_64.so");
    emit_runtime("msquic-windows-x86_64.dll");
}
