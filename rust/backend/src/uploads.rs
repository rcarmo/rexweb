//! Confined storage for uploaded workspace files.

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const MAX_UPLOAD_BYTES: usize = 16 << 20;
static NEXT_ID: AtomicU64 = AtomicU64::new(0);

/// A workspace upload directory pinned by an open directory descriptor.
pub struct UploadRoot {
    directory: File,
    path: PathBuf,
}

struct Temporary {
    path: PathBuf,
    present: bool,
}

impl Temporary {
    fn committed(&mut self) {
        self.present = false;
    }
}

impl Drop for Temporary {
    fn drop(&mut self) {
        if self.present {
            let _ = fs::remove_file(&self.path);
        }
    }
}

impl UploadRoot {
    /// Open `path` without following a final symlink and retain the directory
    /// descriptor for all subsequent storage operations.
    pub fn open(path: &Path) -> Result<Self> {
        let directory = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)
            .with_context(|| format!("open upload root {}", path.display()))?;
        Ok(Self {
            directory,
            path: path.to_path_buf(),
        })
    }

    /// Store one stream in a private, newly-created workspace file.
    ///
    /// `name` is accepted for API compatibility, but it is never used to build
    /// a filesystem path. This preserves the Go service's generated-name
    /// behaviour and confines untrusted client filenames to display metadata.
    pub fn store(&self, _name: &str, data: &mut impl Read) -> Result<Value> {
        let dir_path = PathBuf::from(format!("/proc/self/fd/{}", self.directory.as_raw_fd()));
        let id = random_id()?;
        let temporary_name = format!(".upload-{id}.part");
        let temporary_path = dir_path.join(&temporary_name);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .mode(0o600)
            .open(&temporary_path)
            .context("create exclusive upload temporary")?;
        let mut temporary = Temporary {
            path: temporary_path.clone(),
            present: true,
        };

        let mut prefix = Vec::with_capacity(512);
        let mut buffer = [0_u8; 16 * 1024];
        while prefix.len() < 512 {
            let amount = match data.read(&mut buffer[..512 - prefix.len()]) {
                Ok(0) => break,
                Ok(amount) => amount,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error).context("read upload"),
            };
            prefix.extend_from_slice(&buffer[..amount]);
        }
        file.write_all(&prefix).context("write upload prefix")?;
        let mut size = prefix.len();

        loop {
            let amount = match data.read(&mut buffer) {
                Ok(0) => break,
                Ok(amount) => amount,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error).context("read upload"),
            };
            if size.saturating_add(amount) > MAX_UPLOAD_BYTES {
                bail!("upload exceeds 16 MiB");
            }
            file.write_all(&buffer[..amount])
                .context("write upload data")?;
            size += amount;
        }

        let mime = detect_mime(&prefix);
        let final_name = format!("upload-{id}{}", extension(mime));
        let final_path = dir_path.join(&final_name);
        file.sync_all().context("sync upload temporary")?;
        drop(file);

        // A hard-link publication is atomic and never replaces an existing
        // destination, as a locked down substitute for renameat2(NOREPLACE)
        // available through std. Both names refer to the same pinned inode.
        commit_no_replace(&temporary_path, &final_path)?;
        fs::remove_file(&temporary_path).context("remove upload temporary")?;
        temporary.committed();

        Ok(json!({
            "name": final_name,
            "path": self.path.join(final_name),
            "mime": mime,
            "size": size,
        }))
    }
}

fn random_id() -> Result<String> {
    let mut random = [0_u8; 16];
    File::open("/dev/urandom")?
        .read_exact(&mut random)
        .context("read upload identifier")?;
    let sequence = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let mut id = String::with_capacity(48);
    use std::fmt::Write as _;
    for byte in random {
        write!(&mut id, "{byte:02x}").expect("writing to String cannot fail");
    }
    write!(&mut id, "{sequence:x}").expect("writing to String cannot fail");
    Ok(id)
}

fn commit_no_replace(temporary: &Path, destination: &Path) -> Result<()> {
    fs::hard_link(temporary, destination).context("commit upload without replacement")
}

fn detect_mime(prefix: &[u8]) -> &'static str {
    if prefix.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if prefix.starts_with(b"\xff\xd8\xff") {
        "image/jpeg"
    } else if prefix.starts_with(b"GIF87a") || prefix.starts_with(b"GIF89a") {
        "image/gif"
    } else if prefix.len() >= 12 && &prefix[..4] == b"RIFF" && &prefix[8..12] == b"WEBP" {
        "image/webp"
    } else if prefix.starts_with(b"%PDF-") {
        "application/pdf"
    } else if prefix.starts_with(b"<?xml") {
        "text/xml; charset=utf-8"
    } else if prefix.starts_with(b"%!PS-Adobe-") {
        "application/postscript"
    } else if prefix.starts_with(b"\x00\x00\x01\x00") {
        "image/x-icon"
    } else if prefix.starts_with(b"BM") {
        "image/bmp"
    } else if prefix.len() >= 12 && &prefix[..4] == b"FORM" && &prefix[8..12] == b"AIFF" {
        "audio/aiff"
    } else if prefix.starts_with(b".snd") {
        "audio/basic"
    } else if prefix.starts_with(b"MThd") {
        "audio/midi"
    } else if prefix.starts_with(b"OggS\x00") {
        "application/ogg"
    } else if prefix.len() >= 12
        && &prefix[..4] == b"RIFF"
        && (&prefix[8..12] == b"WAVE" || &prefix[8..12] == b"AVI ")
    {
        if &prefix[8..12] == b"WAVE" {
            "audio/wave"
        } else {
            "video/avi"
        }
    } else if html_prefix(prefix).is_some() {
        "text/html; charset=utf-8"
    } else if is_plain_text(prefix) {
        "text/plain; charset=utf-8"
    } else {
        "application/octet-stream"
    }
}

fn html_prefix(prefix: &[u8]) -> Option<()> {
    let text = prefix
        .iter()
        .position(|byte| !matches!(*byte, b'\t' | b'\n' | b'\x0c' | b'\r' | b' '))
        .map_or(&[][..], |index| &prefix[index..]);
    let mut upper = [0_u8; 16];
    for (slot, byte) in upper.iter_mut().zip(text) {
        *slot = byte.to_ascii_uppercase();
    }
    let upper = &upper[..text.len().min(upper.len())];
    const TAGS: [&[u8]; 17] = [
        b"<!DOCTYPE HTML",
        b"<HTML",
        b"<HEAD",
        b"<SCRIPT",
        b"<IFRAME",
        b"<H1",
        b"<DIV",
        b"<FONT",
        b"<TABLE",
        b"<A",
        b"<STYLE",
        b"<TITLE",
        b"<B",
        b"<BODY",
        b"<BR",
        b"<P",
        b"<!--",
    ];
    TAGS.iter().any(|tag| upper.starts_with(tag)).then_some(())
}

fn is_plain_text(prefix: &[u8]) -> bool {
    !prefix.iter().any(|byte| {
        matches!(
            *byte,
            0x00..=0x08 | 0x0b | 0x0e..=0x1a | 0x1c..=0x1f
        )
    })
}

fn extension(mime: &str) -> &'static str {
    match mime {
        "image/png" => ".png",
        "image/jpeg" => ".jpg",
        "image/gif" => ".gif",
        "image/webp" => ".webp",
        "application/pdf" => ".pdf",
        mime if mime.starts_with("text/plain") => ".txt",
        _ => ".bin",
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_UPLOAD_BYTES, UploadRoot, commit_no_replace};
    use std::fs;
    use std::io::Cursor;
    use std::os::unix::fs::symlink;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST: AtomicU64 = AtomicU64::new(0);

    fn test_dir(label: &str) -> std::path::PathBuf {
        let sequence = NEXT_TEST.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "rexweb-upload-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        path
    }

    #[test]
    fn stores_detected_file_and_uses_private_permissions() {
        let path = test_dir("success");
        let root = UploadRoot::open(&path).unwrap();
        let mut input = Cursor::new(b"\x89PNG\r\n\x1a\nrest".to_vec());
        let result = root.store("../../hostile.png", &mut input).unwrap();

        assert_eq!(result["mime"], "image/png");
        assert_eq!(result["size"], 12);
        assert!(result["name"].as_str().unwrap().ends_with(".png"));
        let stored: std::path::PathBuf = result["path"].as_str().unwrap().into();
        assert_eq!(fs::read(&stored).unwrap(), b"\x89PNG\r\n\x1a\nrest");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&stored).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn rejects_oversize_without_leaving_temporary_files() {
        let path = test_dir("oversize");
        let root = UploadRoot::open(&path).unwrap();
        let mut input = Cursor::new(vec![0; MAX_UPLOAD_BYTES + 1]);
        assert!(root.store("large.bin", &mut input).is_err());
        assert_eq!(fs::read_dir(&path).unwrap().count(), 0);
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn commit_does_not_replace_an_existing_name() {
        let path = test_dir("replace");
        let temporary = path.join("temporary");
        let destination = path.join("destination");
        fs::write(&temporary, b"new").unwrap();
        fs::write(&destination, b"old").unwrap();
        assert!(commit_no_replace(&temporary, &destination).is_err());
        assert_eq!(fs::read(&destination).unwrap(), b"old");
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn open_root_remains_pinned_after_its_path_is_replaced() {
        let original = test_dir("pinned");
        let root = UploadRoot::open(&original).unwrap();
        let moved = original.with_file_name(format!(
            "{}-moved",
            original.file_name().unwrap().to_str().unwrap()
        ));
        fs::rename(&original, &moved).unwrap();
        let outside = test_dir("outside");
        symlink(&outside, &original).unwrap();

        let mut input = Cursor::new(b"hello upload".to_vec());
        let result = root.store("file.txt", &mut input).unwrap();
        let name = result["name"].as_str().unwrap();
        assert!(moved.join(name).exists());
        assert!(!outside.join(name).exists());
        assert!(UploadRoot::open(&original).is_err());

        fs::remove_file(original).unwrap();
        fs::remove_dir_all(moved).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }
}
