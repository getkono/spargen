//! The atomic file write the generated module and the build cache's record go through.

use std::io::Write as _;

use camino::Utf8Path;

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

pub(crate) fn atomic_write(path: &Utf8Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Utf8Path::new("."));
    std::fs::create_dir_all(parent)?;
    let name = path.file_name().unwrap_or("spargen-output");
    let mut attempt = 0u32;
    let temporary = loop {
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
                break candidate;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                attempt = attempt.saturating_add(1);
            }
            Err(error) => return Err(error),
        }
    };
    if let Err(error) = std::fs::rename(&temporary, path) {
        #[cfg(windows)]
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            std::fs::remove_file(path)?;
            return std::fs::rename(&temporary, path);
        }
        let _ = std::fs::remove_file(&temporary);
        return Err(error);
    }
    Ok(())
}
