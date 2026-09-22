#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code,
    unsafe_op_in_unsafe_fn,
    clippy::all
)]

use anyhow::Context;
use libloading::{Library, Symbol};
use std::path::PathBuf;
use std::sync::OnceLock;

pub type QUIC_ADDR = std::ffi::c_void;

#[cfg(target_os = "windows")]
pub type HANDLE = std::os::windows::raw::HANDLE;

#[cfg(target_os = "windows")]
#[repr(C)]
pub struct OVERLAPPED {
    pub Internal: ::std::os::raw::c_ulonglong,
    pub InternalHigh: ::std::os::raw::c_ulonglong,
    pub __bindgen_anon_1: OVERLAPPED__bindgen_ty_1,
    pub hEvent: std::os::windows::raw::HANDLE,
}

#[cfg(target_os = "windows")]
#[repr(C)]
#[derive(Copy, Clone)]
pub union OVERLAPPED__bindgen_ty_1 {
    pub __bindgen_anon_1: OVERLAPPED__bindgen_ty_1__bindgen_ty_1,
    pub Pointer: *mut ::std::os::raw::c_void,
}

#[cfg(target_os = "windows")]
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct OVERLAPPED__bindgen_ty_1__bindgen_ty_1 {
    pub Offset: ::std::os::raw::c_ulong,
    pub OffsetHigh: ::std::os::raw::c_ulong,
}

#[cfg(target_os = "windows")]
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct OVERLAPPED_ENTRY {
    pub lpCompletionKey: ::std::os::raw::c_ulonglong,
    pub lpOverlapped: *mut OVERLAPPED,
    pub Internal: ::std::os::raw::c_ulonglong,
    pub dwNumberOfBytesTransferred: ::std::os::raw::c_ulong,
}

#[cfg(target_os = "linux")]
pub type epoll_event = libc::epoll_event;

#[cfg(not(target_os = "windows"))]
pub type sa_family_t = u16;

#[cfg(not(target_os = "windows"))]
include!("linux_bindings.rs");

#[cfg(target_os = "windows")]
pub type ADDRESS_FAMILY = u16;

#[cfg(target_os = "windows")]
include!("win_bindings.rs");

#[cfg(not(target_os = "windows"))]
pub type QuicFlag = u32;

#[cfg(target_os = "windows")]
pub type QuicFlag = i32;

unsafe impl Send for QUIC_HANDLE {}
unsafe impl Sync for QUIC_HANDLE {}

type MsQuicOpenVersionFn = unsafe extern "C" fn(u32, *mut *const QUIC_API_TABLE) -> u32;
type MsQuicCloseFn = unsafe extern "C" fn(*const QUIC_API_TABLE);

static API_TABLE: OnceLock<usize> = OnceLock::new();
static LIB_HOLDER: OnceLock<Library> = OnceLock::new();

pub fn find_msquic_library() -> Result<PathBuf, anyhow::Error> {
    #[cfg(target_os = "windows")]
    {
        let mut candidates = Vec::new();
        // Release archives put the native runtime beside the executable. Prefer it
        // so local clients and SSH-bootstrapped workers do not require .NET.
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                candidates.push(dir.join("msquic.dll").to_string_lossy().to_string());
            }
        }
        candidates.extend([
            "msquic.dll".to_string(),
            r"C:\Program Files\dotnet\shared\Microsoft.NETCore.App\9.0.20\msquic.dll".to_string(),
            r"C:\Program Files\dotnet\shared\Microsoft.NETCore.App\9.0.19\msquic.dll".to_string(),
            r"C:\Program Files\dotnet\shared\Microsoft.NETCore.App\8.0.31\msquic.dll".to_string(),
            r"C:\Program Files\dotnet\shared\Microsoft.NETCore.App\8.0.11\msquic.dll".to_string(),
        ]);
        // Also check any .NET Core App directory dynamically, sorted in descending order
        // so newer runtimes (e.g. 9.x, 8.x with MsQuic v2) are preferred over older (e.g. 6.x)
        if let Ok(entries) =
            std::fs::read_dir(r"C:\Program Files\dotnet\shared\Microsoft.NETCore.App")
        {
            let mut dirs: Vec<_> = entries.flatten().map(|e| e.path()).collect();
            dirs.sort_by(|a, b| b.cmp(a));
            for dir in dirs {
                let candidate = dir.join("msquic.dll");
                if candidate.exists() {
                    candidates.push(candidate.to_string_lossy().to_string());
                }
            }
        }
        for c in candidates {
            let p = PathBuf::from(&c);
            if p.exists() {
                return Ok(p);
            }
        }
        Ok(PathBuf::from("msquic.dll"))
    }

    #[cfg(not(target_os = "windows"))]
    {
        let mut candidates = Vec::new();
        // See the Windows branch: this supports the self-contained release
        // archive and the cached worker/runtime pair on remote Linux hosts.
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                candidates.push(dir.join("libmsquic.so"));
                candidates.push(dir.join("libmsquic.so.2"));
            }
        }
        if let Ok(home) = std::env::var("HOME") {
            let local_lib = PathBuf::from(home).join(".local/lib");
            candidates.push(local_lib.join("libmsquic.so"));
            candidates.push(local_lib.join("libmsquic.so.2"));
        }
        candidates.push(PathBuf::from("/usr/local/lib/libmsquic.so"));
        candidates.push(PathBuf::from("/usr/local/lib/libmsquic.so.2"));
        candidates.push(PathBuf::from("/usr/lib/x86_64-linux-gnu/libmsquic.so"));
        candidates.push(PathBuf::from("/usr/lib/x86_64-linux-gnu/libmsquic.so.2"));
        candidates.push(PathBuf::from("/usr/lib/libmsquic.so"));
        candidates.push(PathBuf::from("/usr/lib/libmsquic.so.2"));

        // Check system .NET Core App directory (sorted descending for newest MsQuic)
        if let Ok(entries) = std::fs::read_dir("/usr/share/dotnet/shared/Microsoft.NETCore.App") {
            let mut dirs: Vec<_> = entries.flatten().map(|e| e.path()).collect();
            dirs.sort_by(|a, b| b.cmp(a));
            for dir in dirs {
                candidates.push(dir.join("libmsquic.so"));
            }
        }

        // Check user-local ~/.dotnet directory
        if let Ok(home) = std::env::var("HOME") {
            let user_dotnet = PathBuf::from(home).join(".dotnet/shared/Microsoft.NETCore.App");
            if let Ok(entries) = std::fs::read_dir(user_dotnet) {
                let mut dirs: Vec<_> = entries.flatten().map(|e| e.path()).collect();
                dirs.sort_by(|a, b| b.cmp(a));
                for dir in dirs {
                    candidates.push(dir.join("libmsquic.so"));
                }
            }
        }

        for p in candidates {
            if p.exists() {
                return Ok(p);
            }
        }
        Ok(PathBuf::from("libmsquic.so"))
    }
}

pub fn get_api() -> Result<&'static QUIC_API_TABLE, anyhow::Error> {
    if let Some(&ptr) = API_TABLE.get() {
        return Ok(unsafe { &*(ptr as *const QUIC_API_TABLE) });
    }

    let lib_path = find_msquic_library()?;
    let lib = unsafe { Library::new(&lib_path) }.with_context(|| {
        format!(
            "Unable to load MsQuic from '{}'. Install libmsquic (Ubuntu/Debian: `sudo apt install libmsquic`) or place libmsquic.so beside the quiczilla executable",
            lib_path.display()
        )
    })?;
    let open_fn: Symbol<MsQuicOpenVersionFn> = unsafe { lib.get(b"MsQuicOpenVersion\0")? };

    let mut table: *const QUIC_API_TABLE = std::ptr::null();
    let status = unsafe { open_fn(2, &mut table) };
    if status != 0 {
        anyhow::bail!("MsQuicOpenVersion failed with status 0x{:08x}", status);
    }
    if table.is_null() {
        anyhow::bail!("MsQuicOpenVersion returned null table");
    }

    let _ = LIB_HOLDER.set(lib);
    let _ = API_TABLE.set(table as usize);

    Ok(unsafe { &*table })
}

#[inline]
pub fn is_quic_success_or_pending(status: impl Into<i64>) -> bool {
    let s = status.into();
    s == 0 || s as u32 == 0x000703e5 || s as u32 == 0x703e5 || s as u32 == 0xfffffffe
}
