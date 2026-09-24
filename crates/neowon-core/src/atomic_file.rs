//! Files that appear whole. Every file the app writes goes through here, so
//! a reader — an operator's script, a test polling for the file, the next
//! launch — sees the target either absent (or its previous content) or
//! complete, never partly written.
//!
//! The bytes go to a hidden sibling temp file in the target's own directory
//! (a rename across filesystems is not atomic), are flushed and synced, and
//! the temp is renamed over the target. A stream that is dropped before
//! [`AtomicFile::commit`] removes its temp and leaves the target untouched.
//!
//! Only regular files are replaced this way. A target that already exists
//! as something else — `/dev/null`, a FIFO — is written in place, because
//! replacing a device node with a file is never what the operator meant.

use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Write `bytes` to `path` so it appears whole.
pub fn write(path: impl AsRef<Path>, bytes: impl AsRef<[u8]>) -> io::Result<()> {
    let mut f = AtomicFile::create(path)?;
    f.write_all(bytes.as_ref())?;
    f.commit()
}

/// Whether `name` is a temp file this module creates — for a store that
/// sweeps its own directory of the leftovers a killed writer leaves.
pub fn is_temp(name: &str) -> bool {
    name.starts_with('.') && name.ends_with(".tmp")
}

/// A file being written; it appears at its path only on [`commit`].
///
/// [`commit`]: AtomicFile::commit
pub struct AtomicFile {
    out: Option<BufWriter<File>>,
    target: PathBuf,
    /// `None` when writing in place (the target is not a regular file).
    tmp: Option<PathBuf>,
}

impl AtomicFile {
    /// Start writing `path`, with the platform's default permissions.
    pub fn create(path: impl AsRef<Path>) -> io::Result<Self> {
        Self::open(path.as_ref(), false)
    }

    /// Start writing `path` readable by the owner only (mode 0600 on unix),
    /// from its first byte.
    pub fn create_private(path: impl AsRef<Path>) -> io::Result<Self> {
        Self::open(path.as_ref(), true)
    }

    fn open(path: &Path, private: bool) -> io::Result<Self> {
        match std::fs::metadata(path) {
            Ok(m) if !m.is_file() => {
                return Ok(Self {
                    out: Some(BufWriter::new(File::create(path)?)),
                    target: path.to_path_buf(),
                    tmp: None,
                });
            }
            _ => {}
        }
        // Replace what a symlink points at, not the link itself.
        let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let name = target.file_name().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{} names no file", path.display()),
            )
        })?;
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let tmp_name = format!(
            ".{}.{}-{}.tmp",
            name.to_string_lossy(),
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        );
        let tmp = match target.parent() {
            Some(dir) if !dir.as_os_str().is_empty() => dir.join(tmp_name),
            _ => PathBuf::from(tmp_name),
        };
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        if private {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        #[cfg(not(unix))]
        let _ = private;
        let file = opts.open(&tmp)?;
        Ok(Self {
            out: Some(BufWriter::new(file)),
            target,
            tmp: Some(tmp),
        })
    }

    /// Flush, sync and put the file in place. Until this returns `Ok`, the
    /// target holds whatever it held before.
    pub fn commit(mut self) -> io::Result<()> {
        let out = self.out.take().expect("an uncommitted file has a writer");
        let file = out.into_inner().map_err(|e| e.into_error())?;
        let Some(tmp) = self.tmp.take() else {
            return Ok(()); // written in place
        };
        let done = file.sync_all().and_then(|()| {
            drop(file);
            std::fs::rename(&tmp, &self.target)
        });
        if done.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        done
    }
}

impl Write for AtomicFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.out.as_mut().expect("writer").write(buf)
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        self.out.as_mut().expect("writer").write_all(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.out.as_mut().expect("writer").flush()
    }
}

impl Drop for AtomicFile {
    /// Abandoned before `commit`: the target never sees these bytes.
    fn drop(&mut self) {
        drop(self.out.take());
        if let Some(tmp) = self.tmp.take() {
            let _ = std::fs::remove_file(tmp);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listing(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    /// The invariant, stepped through deterministically: at every point a
    /// reader can look, the target is absent or complete.
    #[test]
    fn a_reader_never_sees_a_partial_file() {
        let dir = crate::test_scratch("atomic-partial");
        let path = dir.join("out.json");
        let whole = b"{\"scale\":1.5,\"window\":[1400,800]}";

        let mut f = AtomicFile::create(&path).unwrap();
        assert!(!path.exists(), "the target appeared on create");
        f.write_all(&whole[..10]).unwrap();
        f.flush().unwrap();
        assert!(!path.exists(), "the target appeared mid-write");
        f.write_all(&whole[10..]).unwrap();
        f.flush().unwrap();
        assert!(!path.exists(), "the target appeared before commit");
        f.commit().unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), whole);
        assert_eq!(listing(&dir), ["out.json"], "a temp outlived its commit");

        // A replacement: the reader sees the old bytes until the new are whole.
        let mut f = AtomicFile::create(&path).unwrap();
        f.write_all(b"{\"sca").unwrap();
        f.flush().unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), whole);
        f.write_all(b"le\":2}").unwrap();
        f.commit().unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"{\"scale\":2}");
    }

    #[test]
    fn an_abandoned_write_leaves_the_target_and_no_temp() {
        let dir = crate::test_scratch("atomic-abandon");
        let path = dir.join("keep.bin");
        write(&path, b"old").unwrap();
        let mut f = AtomicFile::create(&path).unwrap();
        f.write_all(b"new but never finished").unwrap();
        drop(f);
        assert_eq!(std::fs::read(&path).unwrap(), b"old");
        assert_eq!(listing(&dir), ["keep.bin"]);
    }

    #[test]
    fn temps_are_unique_hidden_siblings() {
        let dir = crate::test_scratch("atomic-siblings");
        let path = dir.join("a.png");
        let a = AtomicFile::create(&path).unwrap();
        let b = AtomicFile::create(&path).unwrap();
        let names = listing(&dir);
        assert_eq!(names.len(), 2, "two writers share a temp: {names:?}");
        assert!(names.iter().all(|n| is_temp(n) && n.starts_with(".a.png.")));
        drop((a, b));
        assert!(listing(&dir).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn a_non_regular_target_is_written_in_place() {
        write("/dev/null", b"discarded").unwrap();
        assert!(!std::fs::metadata("/dev/null").unwrap().is_file());
    }

    #[cfg(unix)]
    #[test]
    fn a_private_file_is_owner_only_from_the_start() {
        use std::os::unix::fs::PermissionsExt;
        let dir = crate::test_scratch("atomic-private");
        let path = dir.join("7777.token");
        let mut f = AtomicFile::create_private(&path).unwrap();
        let tmp = dir.join(&listing(&dir)[0]);
        assert_eq!(tmp.metadata().unwrap().permissions().mode() & 0o777, 0o600);
        f.write_all(b"secret\n").unwrap();
        f.commit().unwrap();
        assert_eq!(path.metadata().unwrap().permissions().mode() & 0o777, 0o600);
    }
}
