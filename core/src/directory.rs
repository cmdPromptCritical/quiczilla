//! Bounded, streamed directory-transfer protocol.
//!
//! A directory job is one ordered byte stream containing directory and file
//! frames. `pack` frames are logical checkpoints only: a 16/32/64 MiB pack is
//! never assembled in memory or written to a temporary archive. File contents
//! flow through a single 2 MiB buffer and the receiver writes each file as it
//! arrives.

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::types::FILE_CHUNK_SIZE;

const FRAME_START: u8 = 0x11;
const FRAME_DIRECTORY: u8 = 0x12;
const FRAME_PACK_START: u8 = 0x13;
const FRAME_FILE: u8 = 0x14;
const FRAME_PACK_END: u8 = 0x15;
const FRAME_END: u8 = 0x16;
const ACK_OK: u8 = 0x21;
const ACK_ERROR: u8 = 0x22;

/// Paths and error acknowledgements are deliberately bounded. A sender cannot
/// turn an untrusted path or diagnostic into an unbounded allocation.
pub const MAX_PATH_BYTES: usize = 32 * 1024;
pub const MAX_ACK_ERROR_BYTES: usize = 4 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageProfile {
    Hdd,
    Auto,
    Nvme,
}

impl StorageProfile {
    /// Logical pack target. It bounds retry/accounting granularity, not RAM.
    pub const fn pack_target_bytes(self) -> u64 {
        match self {
            Self::Hdd => 16 * 1024 * 1024,
            Self::Auto => 32 * 1024 * 1024,
            Self::Nvme => 64 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DirectoryTransferStats {
    pub files: u64,
    pub directories: u64,
    pub bytes: u64,
    pub packs: u64,
}

fn path_to_wire(path: &Path) -> Result<String> {
    let value = path
        .to_str()
        .context("directory transfer currently requires UTF-8 paths")?
        .replace('\\', "/");
    validate_relative_path(&value)?;
    Ok(value)
}

/// Validate a relative protocol path before it is used to create a path on the
/// receiver. Backslashes are rejected even on Unix so a Windows receiver cannot
/// reinterpret a seemingly-safe separator as a traversal component.
pub fn validate_relative_path(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > MAX_PATH_BYTES
        || value.contains('\0')
        || value.contains('\\')
    {
        bail!("invalid directory transfer path");
    }
    let path = Path::new(value);
    if path.is_absolute() || path.has_root() || path.components().count() == 0 {
        bail!("directory transfer path must be relative");
    }
    for component in path.components() {
        match component {
            Component::Normal(name)
                if name != OsStr::new(".")
                    && name != OsStr::new("..")
                    && !name.to_string_lossy().chars().any(char::is_control) => {}
            _ => bail!("directory transfer path contains an unsafe component"),
        }
    }
    Ok(())
}

fn safe_child(root: &Path, relative: &str) -> Result<PathBuf> {
    validate_relative_path(relative)?;
    let mut output = root.to_path_buf();
    for component in Path::new(relative).components() {
        if let Component::Normal(name) = component {
            output.push(name);
        }
    }
    Ok(output)
}

async fn create_safe_directory(root: &Path, relative: Option<&str>) -> Result<PathBuf> {
    let mut current = root.to_path_buf();
    if let Some(relative) = relative {
        validate_relative_path(relative)?;
        for component in Path::new(relative).components() {
            if let Component::Normal(name) = component {
                current.push(name);
                match tokio::fs::symlink_metadata(&current).await {
                    Ok(metadata) if metadata.file_type().is_symlink() => {
                        bail!("refusing destination symlink in directory transfer")
                    }
                    Ok(metadata) if !metadata.is_dir() => {
                        bail!("destination directory component is not a directory")
                    }
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        tokio::fs::create_dir(&current).await?;
                    }
                    Err(error) => return Err(error.into()),
                }
            }
        }
    } else {
        tokio::fs::create_dir_all(&current).await?;
        let metadata = tokio::fs::symlink_metadata(&current).await?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            bail!("directory transfer root is not a real directory");
        }
    }
    Ok(current)
}

async fn write_u32<W: AsyncWrite + Unpin>(writer: &mut W, value: u32) -> Result<()> {
    writer.write_all(&value.to_be_bytes()).await?;
    Ok(())
}

async fn read_u32<R: AsyncRead + Unpin>(reader: &mut R) -> Result<u32> {
    let mut bytes = [0; 4];
    reader.read_exact(&mut bytes).await?;
    Ok(u32::from_be_bytes(bytes))
}

async fn write_path<W: AsyncWrite + Unpin>(writer: &mut W, value: &str) -> Result<()> {
    validate_relative_path(value)?;
    write_u32(writer, value.len() as u32).await?;
    writer.write_all(value.as_bytes()).await?;
    Ok(())
}

async fn read_path<R: AsyncRead + Unpin>(reader: &mut R) -> Result<String> {
    let length = read_u32(reader).await? as usize;
    if length == 0 || length > MAX_PATH_BYTES {
        bail!("directory transfer path length is invalid");
    }
    let mut bytes = vec![0; length];
    reader.read_exact(&mut bytes).await?;
    let value = String::from_utf8(bytes).context("directory transfer path is not UTF-8")?;
    validate_relative_path(&value)?;
    Ok(value)
}

async fn write_pack_start<W: AsyncWrite + Unpin>(writer: &mut W, pack: u64) -> Result<()> {
    writer.write_u8(FRAME_PACK_START).await?;
    writer.write_all(&pack.to_be_bytes()).await?;
    Ok(())
}

/// Stream a directory without materializing an archive or complete manifest.
pub async fn send_directory<W: AsyncWrite + Unpin>(
    writer: &mut W,
    source: &Path,
    profile: StorageProfile,
    checksum: bool,
) -> Result<DirectoryTransferStats> {
    let source = source
        .canonicalize()
        .context("could not resolve source directory")?;
    if !source.is_dir() {
        bail!("directory transfer source is not a directory");
    }
    let root_name = source
        .file_name()
        .context("directory transfer source has no directory name")?;
    let root_name = path_to_wire(Path::new(root_name))?;
    writer.write_u8(FRAME_START).await?;
    write_path(writer, &root_name).await?;

    let target = profile.pack_target_bytes();
    let mut stats = DirectoryTransferStats {
        directories: 1,
        packs: 1,
        ..Default::default()
    };
    let mut pack_bytes = 0u64;
    let mut pack_number = 0u64;
    write_pack_start(writer, pack_number).await?;

    // A stack of open directory iterators is bounded by hierarchy depth, not
    // by file count. It starts transferring immediately and avoids a full
    // metadata pre-scan.
    let mut stack = vec![std::fs::read_dir(&source).context("could not read source directory")?];
    while !stack.is_empty() {
        let entry = match stack.last_mut().expect("checked non-empty").next() {
            Some(entry) => entry.context("could not enumerate source directory")?,
            None => {
                stack.pop();
                continue;
            }
        };
        let path = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("could not inspect {}", path.display()))?;
        if file_type.is_symlink() {
            continue;
        }
        let metadata = entry
            .metadata()
            .with_context(|| format!("could not stat {}", path.display()))?;
        let relative = path.strip_prefix(&source).expect("walked below source");
        let relative = path_to_wire(relative)?;

        if file_type.is_dir() {
            writer.write_u8(FRAME_DIRECTORY).await?;
            write_path(writer, &relative).await?;
            stats.directories += 1;
            stack.push(std::fs::read_dir(path).context("could not read nested source directory")?);
            continue;
        }
        if !file_type.is_file() {
            // Symlinks and special files are intentionally excluded until a
            // cross-platform policy for them is designed and tested.
            continue;
        }

        let file_size = metadata.len();
        if pack_bytes > 0 && pack_bytes.saturating_add(file_size) > target {
            writer.write_u8(FRAME_PACK_END).await?;
            pack_number += 1;
            stats.packs += 1;
            pack_bytes = 0;
            write_pack_start(writer, pack_number).await?;
        }
        writer.write_u8(FRAME_FILE).await?;
        write_path(writer, &relative).await?;
        writer.write_all(&file_size.to_be_bytes()).await?;
        writer.write_u8(u8::from(checksum)).await?;

        let mut file = tokio::fs::File::open(&path)
            .await
            .with_context(|| format!("could not open {}", path.display()))?;
        let mut remaining = file_size;
        let mut buffer = vec![0u8; FILE_CHUNK_SIZE];
        let mut hasher = checksum.then(Sha256::new);
        while remaining > 0 {
            let limit = remaining.min(buffer.len() as u64) as usize;
            let read = file.read(&mut buffer[..limit]).await?;
            if read == 0 {
                bail!(
                    "source file changed while being transferred: {}",
                    path.display()
                );
            }
            if let Some(hasher) = &mut hasher {
                hasher.update(&buffer[..read]);
            }
            writer.write_all(&buffer[..read]).await?;
            remaining -= read as u64;
        }
        if let Some(hasher) = hasher {
            writer.write_all(&hasher.finalize()).await?;
        }
        stats.files += 1;
        stats.bytes += file_size;
        pack_bytes += file_size;
    }
    writer.write_u8(FRAME_PACK_END).await?;
    writer.write_u8(FRAME_END).await?;
    writer.write_all(&stats.files.to_be_bytes()).await?;
    writer.write_all(&stats.directories.to_be_bytes()).await?;
    writer.write_all(&stats.bytes.to_be_bytes()).await?;
    writer.flush().await?;
    Ok(stats)
}

/// Receive a streamed directory below `destination_base`, verifying every
/// requested path before it reaches the filesystem.
pub async fn receive_directory<R: AsyncRead + Unpin>(
    reader: &mut R,
    destination_base: &Path,
) -> Result<DirectoryTransferStats> {
    if reader.read_u8().await? != FRAME_START {
        bail!("directory transfer did not start with a directory frame");
    }
    let root_name = read_path(reader).await?;
    // The root itself must be a single safe component.
    if Path::new(&root_name).components().count() != 1 {
        bail!("directory transfer root must be one path component");
    }
    let root = safe_child(destination_base, &root_name)?;
    let root = create_safe_directory(&root, None).await?;
    let mut stats = DirectoryTransferStats {
        directories: 1,
        ..Default::default()
    };

    loop {
        match reader.read_u8().await? {
            FRAME_DIRECTORY => {
                let relative = read_path(reader).await?;
                create_safe_directory(&root, Some(&relative)).await?;
                stats.directories += 1;
            }
            FRAME_PACK_START => {
                let mut ignored = [0; 8];
                reader.read_exact(&mut ignored).await?;
                stats.packs += 1;
            }
            FRAME_PACK_END => {}
            FRAME_FILE => {
                let relative = read_path(reader).await?;
                let mut size_bytes = [0; 8];
                reader.read_exact(&mut size_bytes).await?;
                let size = u64::from_be_bytes(size_bytes);
                let checksum = reader.read_u8().await? != 0;
                let destination = safe_child(&root, &relative)?;
                let parent_relative = Path::new(&relative).parent().and_then(Path::to_str);
                let parent = match parent_relative {
                    Some("") | None => root.clone(),
                    Some(path) => create_safe_directory(&root, Some(path)).await?,
                };
                let name = destination
                    .file_name()
                    .context("safe destination has no name")?;
                if let Ok(metadata) = tokio::fs::symlink_metadata(&destination).await {
                    if metadata.file_type().is_symlink() {
                        bail!("refusing to replace destination symlink");
                    }
                }
                let partial = parent.join(format!(".{}.quic-part", name.to_string_lossy()));
                let mut output = tokio::fs::File::create(&partial).await?;
                let mut remaining = size;
                let mut buffer = vec![0u8; FILE_CHUNK_SIZE];
                let mut hasher = checksum.then(Sha256::new);
                while remaining > 0 {
                    let limit = remaining.min(buffer.len() as u64) as usize;
                    let read = reader.read(&mut buffer[..limit]).await?;
                    if read == 0 {
                        bail!("directory transfer ended inside file {relative}");
                    }
                    if let Some(hasher) = &mut hasher {
                        hasher.update(&buffer[..read]);
                    }
                    output.write_all(&buffer[..read]).await?;
                    remaining -= read as u64;
                }
                output.flush().await?;
                drop(output);
                if checksum {
                    let mut expected = [0u8; 32];
                    reader.read_exact(&mut expected).await?;
                    let actual: [u8; 32] = hasher.expect("checksum enabled").finalize().into();
                    if actual != expected {
                        let _ = tokio::fs::remove_file(&partial).await;
                        bail!("directory transfer checksum mismatch for {relative}");
                    }
                }
                tokio::fs::rename(&partial, &destination).await?;
                stats.files += 1;
                stats.bytes += size;
            }
            FRAME_END => {
                let mut totals = [0u8; 24];
                reader.read_exact(&mut totals).await?;
                let files = u64::from_be_bytes(totals[0..8].try_into().expect("fixed size"));
                let directories = u64::from_be_bytes(totals[8..16].try_into().expect("fixed size"));
                let bytes = u64::from_be_bytes(totals[16..24].try_into().expect("fixed size"));
                if files != stats.files || directories != stats.directories || bytes != stats.bytes
                {
                    bail!("directory transfer totals do not match received data");
                }
                return Ok(stats);
            }
            _ => bail!("directory transfer contains an unknown frame"),
        }
    }
}

pub async fn write_directory_ack<W: AsyncWrite + Unpin>(
    writer: &mut W,
    result: &Result<DirectoryTransferStats>,
) -> Result<()> {
    match result {
        Ok(stats) => {
            writer.write_u8(ACK_OK).await?;
            writer.write_all(&stats.files.to_be_bytes()).await?;
            writer.write_all(&stats.bytes.to_be_bytes()).await?;
        }
        Err(error) => {
            let message = error.to_string();
            let bytes = &message.as_bytes()[..message.len().min(MAX_ACK_ERROR_BYTES)];
            writer.write_u8(ACK_ERROR).await?;
            write_u32(writer, bytes.len() as u32).await?;
            writer.write_all(bytes).await?;
        }
    }
    writer.shutdown().await?;
    Ok(())
}

pub async fn read_directory_ack<R: AsyncRead + Unpin>(
    reader: &mut R,
) -> Result<DirectoryTransferStats> {
    match reader.read_u8().await? {
        ACK_OK => {
            let mut values = [0u8; 16];
            reader.read_exact(&mut values).await?;
            Ok(DirectoryTransferStats {
                files: u64::from_be_bytes(values[0..8].try_into().expect("fixed size")),
                bytes: u64::from_be_bytes(values[8..16].try_into().expect("fixed size")),
                ..Default::default()
            })
        }
        ACK_ERROR => {
            let length = read_u32(reader).await? as usize;
            if length > MAX_ACK_ERROR_BYTES {
                bail!("directory receiver returned an oversized error");
            }
            let mut bytes = vec![0; length];
            reader.read_exact(&mut bytes).await?;
            bail!(
                "directory receiver rejected transfer: {}",
                String::from_utf8_lossy(&bytes)
            );
        }
        _ => bail!("directory receiver returned an invalid acknowledgement"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_traversal_and_platform_separators() {
        for path in ["../secret", "a/../../secret", "/root", "a\\b", "a/\0b"] {
            assert!(validate_relative_path(path).is_err(), "{path}");
        }
        assert!(validate_relative_path("nested/file.txt").is_ok());
    }

    #[tokio::test]
    async fn streams_nested_tree_with_bounded_protocol() -> Result<()> {
        let unique = format!("quiczilla-directory-test-{}", std::process::id());
        let base = std::env::temp_dir().join(unique);
        let source = base.join("source");
        let destination = base.join("destination");
        std::fs::create_dir_all(source.join("nested/empty"))?;
        std::fs::write(source.join("nested/hello.txt"), b"hello directory protocol")?;
        std::fs::write(source.join("root.bin"), vec![7u8; 96 * 1024])?;
        let (mut sender, mut receiver) = tokio::io::duplex(2 * 1024 * 1024);
        let send_source = source.clone();
        let receive_destination = destination.clone();
        let (sent, received) = tokio::join!(
            send_directory(&mut sender, &send_source, StorageProfile::Hdd, true),
            receive_directory(&mut receiver, &receive_destination)
        );
        let sent = sent?;
        let received = received?;
        assert_eq!(sent.files, received.files);
        assert_eq!(sent.bytes, received.bytes);
        assert_eq!(
            std::fs::read(destination.join("source/nested/hello.txt"))?,
            b"hello directory protocol"
        );
        assert!(destination.join("source/nested/empty").is_dir());
        std::fs::remove_dir_all(base)?;
        Ok(())
    }
}
