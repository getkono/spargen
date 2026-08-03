//! Content-addressed build-script caching. This module is facade plumbing, not a public subsystem.

use std::io::Write as _;

use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};

use crate::diag::{Diagnostic, Diagnostics, InterpId, JsonPointer, Loc, Span};
use crate::runtime_contract::RuntimeRequirements;
use crate::source::{sha256_hex, InputBundle};
use crate::{Code, Config, OmitRule};

const CACHE_FORMAT: u32 = 2;
const INPUT_PREFIX: &str = "// input-sha256: ";
const CONTENT_PREFIX: &str = "// content-sha256: ";

pub(crate) struct InputSnapshot {
    pub digest: String,
    pub paths: Vec<Utf8PathBuf>,
}

impl InputSnapshot {
    pub fn load(config: &Config) -> Result<Self, Vec<Diagnostic>> {
        let mut diagnostics = Diagnostics::new(config.batch_cap);
        let bundle = match InputBundle::load(&config.spec, &mut diagnostics) {
            Ok(bundle) => bundle,
            Err(_) => return Err(diagnostics.items().to_vec()),
        };

        let mut inputs = bundle
            .source_inputs()
            .map(|(path, bytes)| (path.to_path_buf(), bytes.to_vec()))
            .collect::<Vec<_>>();
        let spec_dir = config.spec.parent().unwrap_or_else(|| Utf8Path::new(""));
        let lock = spec_dir.join("spargen.lock");
        if lock.is_file() {
            match std::fs::read(&lock) {
                Ok(bytes) => inputs.push((lock, bytes)),
                Err(error) => {
                    return Err(vec![cache_diagnostic(format!(
                        "failed to read build input `{lock}`: {error}"
                    ))]);
                }
            }
        }
        inputs.sort_by(|left, right| left.0.cmp(&right.0));
        inputs.dedup_by(|left, right| left.0 == right.0);

        let mut fingerprint = b"spargen-build-input-v1\0".to_vec();
        append(&mut fingerprint, env!("CARGO_PKG_VERSION").as_bytes());
        append(
            &mut fingerprint,
            env!("SPARGEN_BUILD_FINGERPRINT").as_bytes(),
        );
        append(&mut fingerprint, config.spec.as_str().as_bytes());
        append(&mut fingerprint, &[u8::from(config.features.uuid)]);
        append(&mut fingerprint, &[u8::from(config.features.time)]);
        append(
            &mut fingerprint,
            &(config.error_body_cap as u64).to_be_bytes(),
        );
        append(&mut fingerprint, &(config.batch_cap as u64).to_be_bytes());
        append(&mut fingerprint, &[u8::from(config.carve)]);
        for rule in &config.omit.rules {
            match rule {
                OmitRule::Path { path } => {
                    append(&mut fingerprint, b"path");
                    append(&mut fingerprint, path.as_bytes());
                }
                OmitRule::Operation { method, path } => {
                    append(&mut fingerprint, b"operation");
                    append(&mut fingerprint, format!("{method:?}").as_bytes());
                    append(&mut fingerprint, path.as_bytes());
                }
                OmitRule::Component { kind, name } => {
                    append(&mut fingerprint, b"component");
                    append(&mut fingerprint, format!("{kind:?}").as_bytes());
                    append(&mut fingerprint, name.as_bytes());
                }
                OmitRule::Pointer { file, pointer } => {
                    append(&mut fingerprint, b"pointer");
                    append(&mut fingerprint, file.unwrap_or("").as_bytes());
                    append(&mut fingerprint, pointer.as_bytes());
                }
            }
        }
        for (path, bytes) in &inputs {
            append(&mut fingerprint, path.as_str().as_bytes());
            append(&mut fingerprint, bytes);
        }

        Ok(Self {
            digest: sha256_hex(&fingerprint),
            paths: inputs.into_iter().map(|(path, _)| path).collect(),
        })
    }
}

pub(crate) fn cargo_directives(config: &Config, snapshot: Option<&InputSnapshot>) {
    let mut paths = snapshot
        .map(|snapshot| snapshot.paths.clone())
        .unwrap_or_else(|| vec![config.spec.clone()]);
    paths.push(config.output.clone());
    paths.sort();
    paths.dedup();
    for path in paths {
        if !path.as_str().contains(['\n', '\r']) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
}

pub(crate) fn cache_dir() -> Option<Utf8PathBuf> {
    std::env::var("OUT_DIR")
        .ok()
        .map(Utf8PathBuf::from)
        .map(|path| path.join("spargen"))
}

pub(crate) fn cache_path(cache_dir: &Utf8Path, output: &Utf8Path) -> Utf8PathBuf {
    cache_dir.join(format!("{}.json", sha256_hex(output.as_str().as_bytes())))
}

pub(crate) fn finalized(rendered: &str, input_digest: &str) -> (String, String) {
    let content_digest = sha256_hex(rendered.as_bytes());
    (
        format!("{INPUT_PREFIX}{input_digest}\n{CONTENT_PREFIX}{content_digest}\n{rendered}"),
        content_digest,
    )
}

pub(crate) fn verified_output(path: &Utf8Path, input_digest: &str) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let first = bytes.iter().position(|byte| *byte == b'\n')?;
    let second_rel = bytes[first + 1..].iter().position(|byte| *byte == b'\n')?;
    let second = first + 1 + second_rel;
    let input = std::str::from_utf8(&bytes[..first]).ok()?;
    let content = std::str::from_utf8(&bytes[first + 1..second]).ok()?;
    if input.strip_prefix(INPUT_PREFIX)? != input_digest {
        return None;
    }
    let expected = content.strip_prefix(CONTENT_PREFIX)?;
    let actual = sha256_hex(&bytes[second + 1..]);
    (actual == expected).then_some(actual)
}

pub(crate) struct CachedRun {
    pub diagnostics: Vec<Diagnostic>,
    pub requirements: RuntimeRequirements,
}

pub(crate) fn read_cache(
    path: &Utf8Path,
    input_digest: &str,
    content_digest: &str,
) -> Option<CachedRun> {
    let Ok(bytes) = std::fs::read(path) else {
        return None;
    };
    let Ok(record) = serde_json::from_slice::<CacheRecord>(&bytes) else {
        return None;
    };
    if record.format != CACHE_FORMAT
        || record.input_sha256 != input_digest
        || record.content_sha256 != content_digest
    {
        return None;
    }
    let diagnostics = record
        .diagnostics
        .into_iter()
        .filter_map(CachedDiagnostic::into_diagnostic)
        .collect();
    Some(CachedRun {
        diagnostics,
        requirements: record.requirements,
    })
}

pub(crate) fn write_cache(
    path: &Utf8Path,
    input_digest: &str,
    content_digest: &str,
    diagnostics: &[Diagnostic],
    requirements: &RuntimeRequirements,
) -> Result<(), String> {
    let record = CacheRecord {
        format: CACHE_FORMAT,
        input_sha256: input_digest.to_owned(),
        content_sha256: content_digest.to_owned(),
        diagnostics: diagnostics.iter().map(CachedDiagnostic::from).collect(),
        requirements: requirements.clone(),
    };
    let bytes = serde_json::to_vec(&record).map_err(|error| error.to_string())?;
    atomic_write(path, &bytes).map_err(|error| format!("failed to write cache `{path}`: {error}"))
}

pub(crate) fn write_output(path: &Utf8Path, contents: &str) -> Result<(), String> {
    if std::fs::read(path).ok().as_deref() == Some(contents.as_bytes()) {
        return Ok(());
    }
    atomic_write(path, contents.as_bytes())
        .map_err(|error| format!("failed to write generated module `{path}`: {error}"))
}

fn atomic_write(path: &Utf8Path, bytes: &[u8]) -> std::io::Result<()> {
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

fn append(output: &mut Vec<u8>, bytes: &[u8]) {
    output.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
    output.extend_from_slice(bytes);
}

fn cache_diagnostic(message: String) -> Diagnostic {
    Diagnostic {
        code: Code::InvalidInput,
        severity: Code::InvalidInput.severity(),
        pointer: JsonPointer::root(),
        span: None,
        message,
        remedy: None,
        interpretation: None,
    }
}

#[derive(Serialize, Deserialize)]
struct CacheRecord {
    format: u32,
    input_sha256: String,
    content_sha256: String,
    diagnostics: Vec<CachedDiagnostic>,
    requirements: RuntimeRequirements,
}

#[derive(Serialize, Deserialize)]
struct CachedDiagnostic {
    code: String,
    pointer: String,
    span: Option<CachedSpan>,
    message: String,
    remedy: Option<String>,
    interpretation: Option<u16>,
}

impl From<&Diagnostic> for CachedDiagnostic {
    fn from(value: &Diagnostic) -> Self {
        Self {
            code: value.code.as_str().to_owned(),
            pointer: value.pointer.as_str().to_owned(),
            span: value.span.map(CachedSpan::from),
            message: value.message.clone(),
            remedy: value.remedy.clone(),
            interpretation: value.interpretation.map(|id| id.0),
        }
    }
}

impl CachedDiagnostic {
    fn into_diagnostic(self) -> Option<Diagnostic> {
        let code = self.code.parse::<Code>().ok()?;
        Some(Diagnostic {
            code,
            severity: code.severity(),
            pointer: JsonPointer::from(self.pointer),
            span: self.span.map(CachedSpan::into_span),
            message: self.message,
            remedy: self.remedy,
            interpretation: self.interpretation.map(InterpId),
        })
    }
}

#[derive(Clone, Copy, Serialize, Deserialize)]
struct CachedSpan {
    file: u32,
    start: CachedLoc,
    end: CachedLoc,
}

impl From<Span> for CachedSpan {
    fn from(value: Span) -> Self {
        Self {
            file: value.file.0,
            start: CachedLoc::from(value.start),
            end: CachedLoc::from(value.end),
        }
    }
}

impl CachedSpan {
    fn into_span(self) -> Span {
        Span {
            file: crate::FileId(self.file),
            start: self.start.into_loc(),
            end: self.end.into_loc(),
        }
    }
}

#[derive(Clone, Copy, Serialize, Deserialize)]
struct CachedLoc {
    line: u32,
    col: u32,
    offset: usize,
}

impl From<Loc> for CachedLoc {
    fn from(value: Loc) -> Self {
        Self {
            line: value.line,
            col: value.col,
            offset: value.offset,
        }
    }
}

impl CachedLoc {
    fn into_loc(self) -> Loc {
        Loc {
            line: self.line,
            col: self.col,
            offset: self.offset,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{cache_path, finalized, verified_output, InputSnapshot};
    use crate::{Config, Outcome};
    use camino::Utf8PathBuf;

    fn fixture() -> (tempfile::TempDir, Config) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("openapi.yaml");
        let schema = temp.path().join("schema.yaml");
        std::fs::write(
            &root,
            r#"openapi: 3.1.0
info: { title: Cache, version: 1.0.0 }
paths: {}
components:
  schemas:
    Item: { $ref: "schema.yaml" }
"#,
        )
        .unwrap();
        std::fs::write(&schema, "type: string\n").unwrap();
        let config = Config::new(
            Utf8PathBuf::from_path_buf(root).unwrap(),
            Utf8PathBuf::from_path_buf(temp.path().join("api.rs")).unwrap(),
        );
        (temp, config)
    }

    #[test]
    fn fingerprint_covers_transitive_files_lock_and_config() {
        let (temp, config) = fixture();
        let initial = InputSnapshot::load(&config).unwrap();
        assert_eq!(initial.paths.len(), 2);

        std::fs::write(temp.path().join("schema.yaml"), "type: integer\n").unwrap();
        let transitive_changed = InputSnapshot::load(&config).unwrap();
        assert_ne!(initial.digest, transitive_changed.digest);

        std::fs::write(temp.path().join("spargen.lock"), "version = 1\n").unwrap();
        let lock_changed = InputSnapshot::load(&config).unwrap();
        assert_ne!(transitive_changed.digest, lock_changed.digest);
        assert_eq!(lock_changed.paths.len(), 3);

        let mut configured = config;
        configured.error_body_cap += 1;
        let config_changed = InputSnapshot::load(&configured).unwrap();
        assert_ne!(lock_changed.digest, config_changed.digest);
    }

    #[test]
    fn output_fingerprints_detect_missing_stale_and_edited_modules() {
        let (temp, config) = fixture();
        assert_eq!(verified_output(&config.output, "input-a"), None);

        let (contents, content_digest) = finalized("pub struct Api;\n", "input-a");
        std::fs::write(&config.output, &contents).unwrap();
        assert_eq!(
            verified_output(&config.output, "input-a"),
            Some(content_digest)
        );
        assert_eq!(verified_output(&config.output, "input-b"), None);

        std::fs::write(
            &config.output,
            contents.replace("pub struct Api;", "pub struct Edited;"),
        )
        .unwrap();
        assert_eq!(verified_output(&config.output, "input-a"), None);

        drop(temp);
    }

    #[test]
    fn generation_seeds_cache_and_repairs_missing_or_edited_output() {
        let (temp, config) = fixture();
        let cache_dir = Utf8PathBuf::from_path_buf(temp.path().join("cache")).unwrap();

        let first = crate::generate_with_cache_dir(&config, Some(&cache_dir), false);
        assert_eq!(first.outcome, Outcome::Generated, "{first:#?}");
        let original = std::fs::read_to_string(&config.output).unwrap();
        let record = cache_path(&cache_dir, &config.output);
        assert!(record.is_file(), "generation must seed the target cache");

        std::fs::remove_file(&record).unwrap();
        let seeded = crate::generate_with_cache_dir(&config, Some(&cache_dir), false);
        assert_eq!(seeded.outcome, Outcome::Generated, "{seeded:#?}");
        assert!(
            record.is_file(),
            "verified committed output must reseed cache"
        );
        assert_eq!(std::fs::read_to_string(&config.output).unwrap(), original);

        std::fs::write(&config.output, format!("{original}// manual edit\n")).unwrap();
        let repaired = crate::generate_with_cache_dir(&config, Some(&cache_dir), false);
        assert_eq!(repaired.outcome, Outcome::Generated, "{repaired:#?}");
        assert_eq!(std::fs::read_to_string(&config.output).unwrap(), original);

        std::fs::remove_file(&config.output).unwrap();
        let restored = crate::generate_with_cache_dir(&config, Some(&cache_dir), false);
        assert_eq!(restored.outcome, Outcome::Generated, "{restored:#?}");
        assert_eq!(std::fs::read_to_string(&config.output).unwrap(), original);
    }
}
