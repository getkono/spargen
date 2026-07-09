use camino::{Utf8Path, Utf8PathBuf};

use super::{EmitError, EmitPlan};

/// The result of a `--check` drift comparison between a freshly-planned output and the checked-in
/// code (PRD §7.5). A non-`Clean` result fails `spargen generate --check` as a CI contract gate.
#[derive(Debug, Clone)]
pub enum DriftReport {
    /// Checked-in output matches the plan byte-for-byte.
    Clean,
    /// Some files differ; each carries a rendered diff.
    Drifted(Vec<FileDiff>),
    /// Some planned files are absent on disk.
    Missing(Vec<Utf8PathBuf>),
}

/// One drifted file: its path and a unified-diff rendering (via `similar` in the implementation).
#[derive(Debug, Clone)]
pub struct FileDiff {
    /// The file that drifted.
    pub path: Utf8PathBuf,
    /// A human-readable unified diff.
    pub diff: String,
}

/// Compare a plan against checked-in output rooted at `existing_root`, reporting drift (PRD §7.5:
/// clean / drifted / missing). Powers `spargen generate --check`.
pub fn check_drift(plan: &EmitPlan, existing_root: &Utf8Path) -> Result<DriftReport, EmitError> {
    let mut missing = Vec::new();
    let mut drifted = Vec::new();
    for file in &plan.files {
        let path = if file.path.is_absolute() {
            file.path.clone()
        } else {
            existing_root.join(&file.path)
        };
        match std::fs::read_to_string(&path) {
            Ok(existing) if existing == file.contents => {}
            Ok(existing) => {
                let diff = similar::TextDiff::from_lines(&existing, &file.contents)
                    .unified_diff()
                    .header("checked-in", "generated")
                    .to_string();
                drifted.push(FileDiff { path, diff });
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => missing.push(path),
            Err(error) => return Err(EmitError::Io(error)),
        }
    }
    if !missing.is_empty() {
        Ok(DriftReport::Missing(missing))
    } else if !drifted.is_empty() {
        Ok(DriftReport::Drifted(drifted))
    } else {
        Ok(DriftReport::Clean)
    }
}
