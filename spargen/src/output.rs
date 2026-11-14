//! The atomic file writes the generated module, the build cache's record, and `spargen lock`'s
//! vendored copies and lock go through.

use std::io::Write as _;

use camino::{Utf8Path, Utf8PathBuf};

/// Write the rendered module to `path`, leaving the file untouched when it already holds exactly
/// `contents`, so an [`Outcome::Generated`](crate::Outcome::Generated) run rewrites the output
/// only when its bytes change.
pub(crate) fn write_output(path: &Utf8Path, contents: &str) -> Result<(), String> {
    if std::fs::read(path).ok().as_deref() == Some(contents.as_bytes()) {
        return Ok(());
    }
    atomic_write(path, contents.as_bytes())
        .map_err(|error| format!("failed to write generated module `{path}`: {error}"))
}

/// Write `bytes` to `path` through a temporary file beside it, renamed into place, so a reader sees
/// the old contents or the new ones and never a partial write.
pub(crate) fn atomic_write(path: &Utf8Path, bytes: &[u8]) -> std::io::Result<()> {
    Staged::new(path, bytes)?.commit()
}

/// Write every `(path, bytes)` pair in order, each as [`atomic_write`] writes one, staging all of
/// them before any is renamed into place. A failure to stage any file (a directory that cannot be
/// created, a full disk) replaces none: every staged temporary is removed and every `path` keeps
/// what it held. Only a rename failing part-way, after every file was staged, leaves the earlier
/// files replaced, so a file whose meaning depends on the others goes last. The error names the
/// path it was writing.
#[cfg(feature = "remote-fetch")]
pub(crate) fn atomic_write_all(
    files: &[(Utf8PathBuf, Vec<u8>)],
) -> Result<(), (Utf8PathBuf, std::io::Error)> {
    let mut staged = Vec::with_capacity(files.len());
    for (path, bytes) in files {
        staged.push(Staged::new(path, bytes).map_err(|error| (path.clone(), error))?);
    }
    for file in staged {
        let path = file.path.clone();
        file.commit().map_err(|error| (path, error))?;
    }
    Ok(())
}

/// A file's new contents, written and synced under a temporary name beside it and not yet renamed
/// into place. Dropped uncommitted, it removes the temporary.
struct Staged {
    temporary: Utf8PathBuf,
    path: Utf8PathBuf,
    committed: bool,
}

impl Staged {
    fn new(path: &Utf8Path, bytes: &[u8]) -> std::io::Result<Self> {
        let parent = path.parent().unwrap_or_else(|| Utf8Path::new("."));
        std::fs::create_dir_all(parent)?;
        let name = path.file_name().unwrap_or("spargen-output");
        let mut attempt = 0u32;
        loop {
            let candidate = parent.join(format!(
                ".{name}.spargen-{}-{attempt}.tmp",
                std::process::id()
            ));
            match std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&candidate)
            {
                Ok(mut file) => {
                    if let Err(error) = file.write_all(bytes).and_then(|()| file.sync_all()) {
                        let _ = std::fs::remove_file(&candidate);
                        return Err(error);
                    }
                    return Ok(Self {
                        temporary: candidate,
                        path: path.to_path_buf(),
                        committed: false,
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    attempt = attempt.saturating_add(1);
                }
                Err(error) => return Err(error),
            }
        }
    }

    /// Rename the temporary into place. On failure the temporary is removed (on drop) and the file
    /// keeps what it held.
    fn commit(mut self) -> std::io::Result<()> {
        let renamed = match std::fs::rename(&self.temporary, &self.path) {
            #[cfg(windows)]
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                std::fs::remove_file(&self.path)
                    .and_then(|()| std::fs::rename(&self.temporary, &self.path))
            }
            renamed => renamed,
        };
        self.committed = renamed.is_ok();
        renamed
    }
}

impl Drop for Staged {
    fn drop(&mut self) {
        if !self.committed {
            let _ = std::fs::remove_file(&self.temporary);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> (tempfile::TempDir, Utf8PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
        (temp, dir)
    }

    /// Every file named in the directory, temporaries included, sorted.
    fn listing(dir: &Utf8Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    #[cfg(feature = "remote-fetch")]
    #[test]
    fn writes_every_file_in_order_and_leaves_no_temporary() {
        let (_temp, dir) = dir();
        std::fs::write(dir.join("b"), "old b").unwrap();
        let files = vec![
            (dir.join("a"), b"new a".to_vec()),
            (dir.join("b"), b"new b".to_vec()),
            (dir.join("sub/c"), b"new c".to_vec()),
        ];
        atomic_write_all(&files).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("a")).unwrap(), "new a");
        assert_eq!(std::fs::read_to_string(dir.join("b")).unwrap(), "new b");
        assert_eq!(std::fs::read_to_string(dir.join("sub/c")).unwrap(), "new c");
        assert_eq!(listing(&dir), ["a", "b", "sub"]);
    }

    /// The third file cannot be staged (its parent is a regular file), so neither earlier file is
    /// replaced, no temporary is left behind, and the error names the file that failed.
    #[cfg(feature = "remote-fetch")]
    #[test]
    fn a_file_that_cannot_be_staged_replaces_none() {
        let (_temp, dir) = dir();
        std::fs::write(dir.join("a"), "old a").unwrap();
        std::fs::write(dir.join("blocked"), "a file, not a directory").unwrap();
        let files = vec![
            (dir.join("a"), b"new a".to_vec()),
            (dir.join("b"), b"new b".to_vec()),
            (dir.join("blocked/c"), b"new c".to_vec()),
        ];
        let (path, _) = atomic_write_all(&files).unwrap_err();
        assert_eq!(path, dir.join("blocked/c"));
        assert_eq!(std::fs::read_to_string(dir.join("a")).unwrap(), "old a");
        assert_eq!(listing(&dir), ["a", "blocked"]);
    }

    #[test]
    fn atomic_write_replaces_a_file_and_leaves_no_temporary() {
        let (_temp, dir) = dir();
        std::fs::write(dir.join("a"), "old").unwrap();
        atomic_write(&dir.join("a"), b"new").unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("a")).unwrap(), "new");
        assert_eq!(listing(&dir), ["a"]);
    }

    /// `src/api.rs -> ../generated/api.rs`: the write lands in the link's target, the link
    /// survives, and no temporary is left beside either.
    #[cfg(unix)]
    #[test]
    fn atomic_write_writes_through_a_symlinked_path() {
        let (_temp, dir) = dir();
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::create_dir_all(dir.join("generated")).unwrap();
        std::fs::write(dir.join("generated/api.rs"), "old").unwrap();
        std::os::unix::fs::symlink("../generated/api.rs", dir.join("src/api.rs")).unwrap();
        atomic_write(&dir.join("src/api.rs"), b"new").unwrap();
        assert!(std::fs::symlink_metadata(dir.join("src/api.rs"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            std::fs::read_to_string(dir.join("generated/api.rs")).unwrap(),
            "new"
        );
        assert_eq!(listing(&dir.join("src")), ["api.rs"]);
        assert_eq!(listing(&dir.join("generated")), ["api.rs"]);
    }

    /// A chain of links, the last one dangling: the write follows every hop and creates the file
    /// the chain ends at.
    #[cfg(unix)]
    #[test]
    fn atomic_write_follows_a_link_chain_to_a_missing_target() {
        let (_temp, dir) = dir();
        std::os::unix::fs::symlink("b", dir.join("a")).unwrap();
        std::os::unix::fs::symlink(dir.join("c"), dir.join("b")).unwrap();
        atomic_write(&dir.join("a"), b"new").unwrap();
        assert!(std::fs::symlink_metadata(dir.join("a"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(std::fs::read_to_string(dir.join("c")).unwrap(), "new");
        assert_eq!(listing(&dir), ["a", "b", "c"]);
    }

    /// A link cycle is refused rather than followed forever, and nothing is written.
    #[cfg(unix)]
    #[test]
    fn atomic_write_refuses_a_link_cycle() {
        let (_temp, dir) = dir();
        std::os::unix::fs::symlink("b", dir.join("a")).unwrap();
        std::os::unix::fs::symlink("a", dir.join("b")).unwrap();
        atomic_write(&dir.join("a"), b"new").unwrap_err();
        assert_eq!(listing(&dir), ["a", "b"]);
    }

    /// `atomic_write_all` writes through a link too, and a staging failure still names the path
    /// the caller passed, not the link's target.
    #[cfg(all(unix, feature = "remote-fetch"))]
    #[test]
    fn atomic_write_all_writes_through_a_link_and_names_the_given_path() {
        let (_temp, dir) = dir();
        std::fs::write(dir.join("target"), "old").unwrap();
        std::os::unix::fs::symlink("target", dir.join("link")).unwrap();
        atomic_write_all(&[(dir.join("link"), b"new".to_vec())]).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("target")).unwrap(), "new");
        assert_eq!(listing(&dir), ["link", "target"]);

        std::fs::write(dir.join("blocked"), "a file, not a directory").unwrap();
        std::os::unix::fs::symlink("blocked/c", dir.join("into-blocked")).unwrap();
        let (path, _) =
            atomic_write_all(&[(dir.join("into-blocked"), b"new".to_vec())]).unwrap_err();
        assert_eq!(path, dir.join("into-blocked"));
    }
}
