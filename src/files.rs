#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
#[cfg(windows)]
use std::os::windows::fs::OpenOptionsExt;
use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};

static TEMP_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug)]
pub struct Entry {
    pub name: OsString,
    pub label: String,
    pub key: String,
    pub directory: bool,
    pub extension: String,
    pub symlink: bool,
}

pub fn scan(path: &Path) -> io::Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for item in fs::read_dir(path)? {
        let item = item?;
        let kind = item.file_type()?;
        let name = item.file_name();
        let label = safe_text(&name.to_string_lossy());
        entries.push(Entry {
            key: label.to_lowercase(),
            extension: Path::new(&name)
                .extension()
                .unwrap_or_default()
                .to_string_lossy()
                .to_lowercase(),
            label,
            name,
            directory: kind.is_dir(),
            symlink: kind.is_symlink(),
        });
    }
    entries.sort_unstable_by(|a, b| {
        b.directory
            .cmp(&a.directory)
            .then(a.key.cmp(&b.key))
            .then(a.name.cmp(&b.name))
    });
    Ok(entries)
}

#[derive(Clone, Debug)]
pub enum Operation {
    Copy {
        source: PathBuf,
        destination: PathBuf,
    },
    Move {
        source: PathBuf,
        destination: PathBuf,
    },
    Delete(PathBuf),
    Mkdir(PathBuf),
}

#[cfg(test)]
pub fn perform(operation: &Operation) -> io::Result<String> {
    perform_cancellable(operation, &AtomicBool::new(false))
}

pub fn perform_cancellable(operation: &Operation, cancelled: &AtomicBool) -> io::Result<String> {
    if cancelled.load(Ordering::Relaxed) {
        return Err(io::Error::new(io::ErrorKind::Interrupted, "Cancelled"));
    }
    match operation {
        Operation::Copy {
            source,
            destination,
        } => {
            let mut input = open_regular(source)?;
            match fs::symlink_metadata(destination) {
                Ok(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        "Destination already exists",
                    ));
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            let parent = destination
                .parent()
                .ok_or_else(|| invalid("Destination has no parent"))?;
            let temporary = parent.join(format!(
                ".mdir-copy-{}-{}",
                std::process::id(),
                TEMP_ID.fetch_add(1, Ordering::Relaxed)
            ));
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            options.mode(0o600);
            let mut output = options.open(&temporary)?;
            let result = (|| {
                let mut buffer = [0_u8; 65_536];
                loop {
                    if cancelled.load(Ordering::Relaxed) {
                        return Err(io::Error::new(
                            io::ErrorKind::Interrupted,
                            "Copy cancelled; destination unchanged",
                        ));
                    }
                    let count = input.read(&mut buffer)?;
                    if count == 0 {
                        break;
                    }
                    output.write_all(&buffer[..count])?;
                }
                output.flush()?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    output.set_permissions(fs::Permissions::from_mode(
                        input.metadata()?.permissions().mode() & 0o777,
                    ))?;
                }
                output.sync_all()?;
                // Close file handles before exclusive publication, including on Windows.
                if cancelled.load(Ordering::Relaxed) {
                    return Err(io::Error::new(
                        io::ErrorKind::Interrupted,
                        "Copy cancelled; destination unchanged",
                    ));
                }
                drop(output);
                drop(input);
                move_without_replace(&temporary, destination)?;
                Ok("Copy complete".to_owned())
            })();
            let cleanup = match fs::remove_file(&temporary) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                result => result,
            };
            match (result, cleanup) {
                (Err(error), Err(cleanup)) => Err(io::Error::other(format!(
                    "{error}; temporary cleanup failed at {}: {cleanup}",
                    temporary.display()
                ))),
                (Err(error), Ok(())) => Err(error),
                (Ok(_), Err(error)) => Err(io::Error::other(format!(
                    "Copy published, but temporary cleanup failed: {error}"
                ))),
                (Ok(message), Ok(())) => Ok(message),
            }
        }
        Operation::Move {
            source,
            destination,
        } => {
            let kind = fs::symlink_metadata(source)?.file_type();
            if kind.is_symlink() || !(kind.is_file() || kind.is_dir()) {
                return Err(invalid("Only regular files and directories can move"));
            }
            move_without_replace(source, destination)?;
            Ok("Move complete".to_owned())
        }
        Operation::Delete(path) => {
            let metadata = fs::symlink_metadata(path)?;
            if metadata.file_type().is_symlink() {
                return Err(invalid("Symlink operations are disabled"));
            }
            if metadata.is_dir() {
                fs::remove_dir(path)?;
            } else if metadata.is_file() {
                fs::remove_file(path)?;
            } else {
                return Err(invalid("Special files cannot be deleted"));
            }
            Ok("Deleted (directories must be empty)".to_owned())
        }
        Operation::Mkdir(path) => {
            fs::create_dir(path)?;
            Ok("Directory created".to_owned())
        }
    }
}

pub fn preview(path: &Path) -> io::Result<String> {
    let file = open_regular(path)?;
    let mut bytes = Vec::new();
    file.take(65_537).read_to_end(&mut bytes)?;
    let truncated = bytes.len() > 65_536;
    bytes.truncate(65_536);
    if bytes.contains(&0) {
        return Ok("Binary file: text preview unavailable".to_owned());
    }
    let text = String::from_utf8_lossy(&bytes);
    let mut result = safe_text(&text);
    if truncated {
        result.push_str("\n[Preview truncated at 64 KiB]");
    }
    Ok(result)
}

pub fn safe_text(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c == '\n'
                || c == '\t'
                || (!c.is_control()
                    && !matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'))
            {
                c
            } else {
                '�'
            }
        })
        .collect()
}

fn open_regular(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    #[cfg(windows)]
    options.custom_flags(0x0020_0000); // FILE_FLAG_OPEN_REPARSE_POINT: inspect, never follow a link.
    let file = options.open(path)?;
    if !file.metadata()?.is_file() || file.metadata()?.file_type().is_symlink() {
        return Err(invalid(
            "Only regular files are supported; symlinks and special files are refused",
        ));
    }
    Ok(file)
}

fn move_without_replace(source: &Path, destination: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::{ffi::CString, os::unix::ffi::OsStrExt};
        let source =
            CString::new(source.as_os_str().as_bytes()).map_err(|_| invalid("NUL in source"))?;
        let destination = CString::new(destination.as_os_str().as_bytes())
            .map_err(|_| invalid("NUL in destination"))?;
        #[cfg(target_os = "macos")]
        let result =
            unsafe { libc::renamex_np(source.as_ptr(), destination.as_ptr(), libc::RENAME_EXCL) };
        #[cfg(target_os = "linux")]
        let result = unsafe {
            libc::syscall(
                libc::SYS_renameat2,
                libc::AT_FDCWD,
                source.as_ptr(),
                libc::AT_FDCWD,
                destination.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Exclusive rename is unavailable on this Unix platform",
        ));
        if result == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn MoveFileExW(existing: *const u16, new: *const u16, flags: u32) -> i32;
        }
        let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
        let destination: Vec<u16> = destination
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();
        if source[..source.len() - 1].contains(&0)
            || destination[..destination.len() - 1].contains(&0)
        {
            return Err(invalid("NUL in path"));
        }
        // No REPLACE_EXISTING or COPY_ALLOWED: refuse collisions and cross-volume moves.
        let result = unsafe { MoveFileExW(source.as_ptr(), destination.as_ptr(), 0) };
        if result != 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::symlink;
    struct Sandbox(PathBuf);
    impl Sandbox {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "mdir-a-test-{}-{}",
                std::process::id(),
                TEMP_ID.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Sandbox {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    #[cfg(unix)]
    fn copies_preserve_private_and_executable_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = Sandbox::new();
        for mode in [0o600, 0o700, 0o750] {
            let source = dir.0.join(format!("source-{mode}"));
            let target = dir.0.join(format!("target-{mode}"));
            fs::write(&source, "private").unwrap();
            fs::set_permissions(&source, fs::Permissions::from_mode(mode)).unwrap();
            perform(&Operation::Copy {
                source,
                destination: target.clone(),
            })
            .unwrap();
            assert_eq!(
                fs::metadata(target).unwrap().permissions().mode() & 0o777,
                mode
            );
        }
    }

    #[test]
    fn directory_rename_and_cancellation_preserve_data() {
        let dir = Sandbox::new();
        let source = dir.0.join("before");
        let target = dir.0.join("after");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("data"), "keep").unwrap();
        perform(&Operation::Move {
            source: source.clone(),
            destination: target.clone(),
        })
        .unwrap();
        assert!(!source.exists());
        assert_eq!(fs::read_to_string(target.join("data")).unwrap(), "keep");
        let destination = dir.0.join("cancelled");
        assert!(
            perform_cancellable(
                &Operation::Copy {
                    source: target.join("data"),
                    destination: destination.clone()
                },
                &AtomicBool::new(true)
            )
            .is_err()
        );
        assert!(!destination.exists());
        assert_eq!(fs::read_to_string(target.join("data")).unwrap(), "keep");
    }

    #[test]
    fn copy_and_move_refuse_overwrite_and_preserve_source() {
        let dir = Sandbox::new();
        let source = dir.0.join("한글.txt");
        let target = dir.0.join("target");
        fs::write(&source, "hello").unwrap();
        fs::write(&target, "existing").unwrap();
        assert!(
            perform(&Operation::Copy {
                source: source.clone(),
                destination: target.clone()
            })
            .is_err()
        );
        assert!(
            perform(&Operation::Move {
                source: source.clone(),
                destination: target.clone()
            })
            .is_err()
        );
        assert_eq!(fs::read_to_string(&target).unwrap(), "existing");
        assert_eq!(fs::read_to_string(&source).unwrap(), "hello");
        fs::remove_file(&target).unwrap();
        perform(&Operation::Copy {
            source: source.clone(),
            destination: target.clone(),
        })
        .unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "hello");
        let moved = dir.0.join("moved");
        perform(&Operation::Move {
            source: source.clone(),
            destination: moved.clone(),
        })
        .unwrap();
        assert!(!source.exists());
        assert_eq!(fs::read_to_string(moved).unwrap(), "hello");
    }
    #[cfg(unix)]
    #[test]
    fn refuses_symlink_copy_and_nonempty_delete() {
        let dir = Sandbox::new();
        let sub = dir.0.join("sub");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("keep"), "x").unwrap();
        assert!(perform(&Operation::Delete(sub.clone())).is_err());
        assert!(sub.join("keep").exists());
        let link = dir.0.join("link");
        symlink(sub.join("keep"), &link).unwrap();
        assert!(
            perform(&Operation::Copy {
                source: link.clone(),
                destination: dir.0.join("copy")
            })
            .is_err()
        );
        assert!(preview(&link).is_err());
        assert!(perform(&Operation::Delete(link)).is_err());
    }
    #[test]
    fn preview_is_bounded_and_control_safe() {
        let dir = Sandbox::new();
        let path = dir.0.join("large");
        fs::write(&path, "\x1b".repeat(100_000)).unwrap();
        let text = preview(&path).unwrap();
        assert!(text.contains("truncated"));
        assert!(!text.contains('\x1b'));
    }
}
