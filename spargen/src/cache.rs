//! Content-addressed build-script caching: the input fingerprint, the cache record, and the
//! `cargo:rerun-if-changed` directives over those inputs.

use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};

use crate::diag::{Diagnostic, Diagnostics, InterpId, JsonPointer, Loc, OutcomeClaim, Span};
use crate::ir::DateTimeBackend;
use crate::output::atomic_write;
use crate::runtime_contract::RuntimeRequirements;
use crate::source::{sha256_hex, InputBundle, Sha256, LOCK_FILE_NAME};
use crate::{Build, Code, Report, Spec};

const CACHE_FORMAT: u32 = 5;
const INPUT_PREFIX: &str = "// input-sha256: ";
const CONTENT_PREFIX: &str = "// content-sha256: ";

pub(crate) struct InputSnapshot {
    pub(crate) digest: String,
    pub(crate) paths: Vec<Utf8PathBuf>,
}

impl InputSnapshot {
    /// The error carries the whole diagnostic batch, not just its items, so a caller can tell a
    /// complete failure list from one the `batch_cap` truncated.
    pub(crate) fn load(spec: &Spec) -> Result<Self, Diagnostics> {
        let inputs = read_inputs(spec)?;
        let inputs = inputs.sorted();
        Ok(Self {
            digest: fingerprint(
                spec,
                env!("CARGO_PKG_VERSION"),
                env!("SPARGEN_BUILD_FINGERPRINT"),
                &inputs,
            ),
            paths: inputs.iter().map(|(path, _)| path.to_path_buf()).collect(),
        })
    }
}

/// The input digest of a build of `spec` by spargen `version` (`build` being its build
/// fingerprint) over `inputs`: every field framed by [`append`] and hashed as it is framed, so the
/// input bytes are read in place rather than gathered into one buffer first.
fn fingerprint(spec: &Spec, version: &str, build: &str, inputs: &[(&Utf8Path, &[u8])]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"spargen-build-input-v1\0");
    append(&mut hasher, version.as_bytes());
    append(&mut hasher, build.as_bytes());
    append(&mut hasher, spec.path.as_str().as_bytes());
    append(&mut hasher, &[u8::from(spec.uuid)]);
    append(&mut hasher, &[datetime_byte(spec.datetime)]);
    append(&mut hasher, &(spec.error_body_cap as u64).to_be_bytes());
    append(&mut hasher, &(spec.batch_cap as u64).to_be_bytes());
    append(&mut hasher, &[u8::from(spec.carve)]);
    append(&mut hasher, &[u8::from(spec.open_narrowing)]);
    append(&mut hasher, &[u8::from(spec.open_unions)]);
    append(&mut hasher, &[u8::from(spec.verbatim_docs)]);
    // The omit profile's one canonical encoding, the same bytes its provenance-header
    // fingerprint hashes, so the two cannot drift apart.
    append(&mut hasher, &spec.omit.canonical_encoding());
    // The bound header names: their count, then each name, in the order bound.
    append(&mut hasher, &(spec.bind_headers.len() as u64).to_be_bytes());
    for name in &spec.bind_headers {
        append(&mut hasher, name.as_bytes());
    }
    for (path, bytes) in inputs {
        append(&mut hasher, path.as_str().as_bytes());
        append(&mut hasher, bytes);
    }
    hasher.finalize()
}

/// The date-time backend's one fingerprint byte. `Time` and `String` keep the bytes the boolean
/// `time` switch they replace hashed (`1` and `0`), so a build from before the backend choice
/// stays a cache hit.
fn datetime_byte(backend: DateTimeBackend) -> u8 {
    match backend {
        DateTimeBackend::String => 0,
        DateTimeBackend::Time => 1,
        DateTimeBackend::Jiff => 2,
        DateTimeBackend::Chrono => 3,
    }
}

/// The paths [`InputSnapshot::load`] fingerprints, read without hashing them.
pub(crate) fn input_paths(spec: &Spec) -> Result<Vec<Utf8PathBuf>, Diagnostics> {
    Ok(read_inputs(spec)?
        .sorted()
        .into_iter()
        .map(|(path, _)| path.to_path_buf())
        .collect())
}

/// The build inputs [`read_inputs`] read: the loaded bundle, whose files' bytes are borrowed
/// rather than copied, and the other files it read.
struct Inputs {
    bundle: InputBundle,
    files: Vec<(Utf8PathBuf, Vec<u8>)>,
}

impl Inputs {
    /// Every input with its bytes, sorted and deduplicated by path. A path both the bundle and
    /// another read name keeps the bundle's bytes.
    fn sorted(&self) -> Vec<(&Utf8Path, &[u8])> {
        let mut inputs = self
            .bundle
            .source_inputs()
            .chain(
                self.files
                    .iter()
                    .map(|(path, bytes)| (path.as_path(), bytes.as_slice())),
            )
            .collect::<Vec<_>>();
        inputs.sort_by(|left, right| left.0.cmp(right.0));
        inputs.dedup_by(|left, right| left.0 == right.0);
        inputs
    }
}

/// Every build input `spec` reads: the bundle's files, `spargen.lock` beside the spec when there
/// is one, and the applied config files.
fn read_inputs(spec: &Spec) -> Result<Inputs, Diagnostics> {
    let mut diagnostics = Diagnostics::new(spec.batch_cap);
    let bundle = match InputBundle::load(&spec.path, &mut diagnostics) {
        Ok(bundle) => bundle,
        Err(_) => return Err(diagnostics),
    };

    let mut files = Vec::new();
    let spec_dir = spec.path.parent().unwrap_or_else(|| Utf8Path::new(""));
    let lock = spec_dir.join(LOCK_FILE_NAME);
    // The applied config files are read like `spargen.lock`: a file that existed when it was
    // applied and cannot be read now fails the build rather than dropping out of the inputs.
    let lock = lock.is_file().then_some(lock);
    for path in lock.iter().chain(&spec.config_files) {
        match std::fs::read(path) {
            Ok(bytes) => files.push((path.clone(), bytes)),
            Err(error) => {
                Diagnostic::at_root(
                    Code::InvalidInput,
                    format!("failed to read build input `{path}`: {error}"),
                )
                .emit(&mut diagnostics);
                return Err(diagnostics);
            }
        }
    }
    Ok(Inputs { bundle, files })
}

/// The paths a build names to Cargo as `cargo:rerun-if-changed`: each input the snapshot read
/// (only the root spec when there is no snapshot), each applied config file, and the output
/// module, sorted and deduplicated.
pub(crate) fn rerun_paths(build: &Build, snapshot: Option<&InputSnapshot>) -> Vec<Utf8PathBuf> {
    let mut paths = snapshot
        .map(|snapshot| snapshot.paths.clone())
        .unwrap_or_else(|| vec![build.spec.path.clone()]);
    paths.extend(build.spec.config_files.iter().cloned());
    paths.push(build.output.clone());
    paths.sort();
    paths.dedup();
    paths
}

// Mutation testing: replacing this reader survives the unit tests and is declared in
// `.cargo/mutants.toml` rather than killed. Its only input is the process-wide `OUT_DIR`, which a
// unit test could set only under every test running beside it; the cache logic it feeds takes the
// directory as an argument (`pipeline::generate_with_cache_dir`), which the tests below drive.
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

/// The content digest of the generated file at `path` when it is still the output of
/// `input_digest`. `None`, which makes the caller regenerate, when the file is unreadable, lacks
/// either header line, records a different input digest, or its body does not hash to the
/// recorded content digest (a manual edit).
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
    pub(crate) diagnostics: Vec<Diagnostic>,
    /// Whether the seeding run's diagnostic list was cut at `batch_cap`.
    pub(crate) truncated: bool,
    pub(crate) requirements: RuntimeRequirements,
}

/// The recorded run for this input and output. `None`, which makes the caller rerun the pipeline,
/// when the cache file is unreadable or does not parse, or its format, input digest or content
/// digest differs from the current one.
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
        truncated: record.truncated,
        requirements: record.requirements,
    })
}

/// Record `report`, the run that generated the output, so a later cache hit replays its
/// diagnostics and truncation.
pub(crate) fn write_cache(
    path: &Utf8Path,
    input_digest: &str,
    content_digest: &str,
    report: &Report,
    requirements: &RuntimeRequirements,
) -> Result<(), String> {
    let record = CacheRecord {
        format: CACHE_FORMAT,
        input_sha256: input_digest.to_owned(),
        content_sha256: content_digest.to_owned(),
        diagnostics: report
            .diagnostics
            .iter()
            .map(CachedDiagnostic::from)
            .collect(),
        truncated: report.truncated,
        requirements: requirements.clone(),
    };
    let bytes = serde_json::to_vec(&record).map_err(|error| error.to_string())?;
    atomic_write(path, &bytes).map_err(|error| format!("failed to write cache `{path}`: {error}"))
}

fn append(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update(&(bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
}

#[derive(Serialize, Deserialize)]
struct CacheRecord {
    format: u32,
    input_sha256: String,
    content_sha256: String,
    diagnostics: Vec<CachedDiagnostic>,
    /// Stored so a cache hit reports the truncation of the run it replays.
    truncated: bool,
    requirements: RuntimeRequirements,
}

#[derive(Serialize, Deserialize)]
struct CachedDiagnostic {
    code: String,
    pointer: String,
    span: Option<CachedSpan>,
    file: Option<String>,
    message: String,
    remedy: Option<String>,
    interpretation: Option<u16>,
    /// Persisted rather than re-derived from the code's severity, so a replayed diagnostic makes
    /// the claim the fresh run's diagnostic made.
    claim: CachedClaim,
}

/// The cache's copy of an [`OutcomeClaim`], in the kebab-case spelling the public type
/// serializes as.
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum CachedClaim {
    Independent,
    Rejected,
    Generated,
}

impl From<OutcomeClaim> for CachedClaim {
    fn from(value: OutcomeClaim) -> Self {
        match value {
            OutcomeClaim::Independent => Self::Independent,
            OutcomeClaim::Rejected => Self::Rejected,
            OutcomeClaim::Generated => Self::Generated,
        }
    }
}

impl From<CachedClaim> for OutcomeClaim {
    fn from(value: CachedClaim) -> Self {
        match value {
            CachedClaim::Independent => Self::Independent,
            CachedClaim::Rejected => Self::Rejected,
            CachedClaim::Generated => Self::Generated,
        }
    }
}

impl From<&Diagnostic> for CachedDiagnostic {
    fn from(value: &Diagnostic) -> Self {
        Self {
            code: value.code.as_str().to_owned(),
            pointer: value.pointer.as_str().to_owned(),
            span: value.span.map(CachedSpan::from),
            file: value.file.clone(),
            message: value.message.clone(),
            remedy: value.remedy.clone(),
            interpretation: value.interpretation.map(|id| id.0),
            claim: CachedClaim::from(value.claim),
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
            file: self.file,
            message: self.message,
            remedy: self.remedy,
            interpretation: self.interpretation.map(InterpId),
            claim: OutcomeClaim::from(self.claim),
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
    use super::{
        append, cache_path, finalized, fingerprint, rerun_paths, sha256_hex, verified_output,
        CachedDiagnostic, Diagnostic, InputSnapshot, JsonPointer, OutcomeClaim, Sha256,
    };
    use crate::runtime_contract::rerun_if_changed_lines;
    use crate::{Build, CargoIntegration, DateTimeBackend, OmitRule, Outcome, Spec};
    use camino::{Utf8Path, Utf8PathBuf};

    /// The `cargo:rerun-if-changed` lines a build prints for its inputs and output.
    fn rerun_directives(build: &Build, snapshot: Option<&InputSnapshot>) -> Vec<String> {
        rerun_if_changed_lines(&rerun_paths(build, snapshot))
    }

    fn fixture() -> (tempfile::TempDir, Build) {
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
        let build = Spec::new(Utf8PathBuf::from_path_buf(root).unwrap())
            .build(Utf8PathBuf::from_path_buf(temp.path().join("api.rs")).unwrap())
            // Not a build script: the cargo integration has nothing to integrate with, and
            // saying so keeps these reports free of `W013`.
            .cargo(CargoIntegration::Off);
        (temp, build)
    }

    #[test]
    fn an_input_path_with_a_line_break_is_never_written_as_a_directive() {
        // Written out, the text after the break would reach Cargo as a directive of its own, so a
        // crafted file name could inject one (#408; the twin in `runtime_contract` is #202). Every
        // other path, the output module included, is still named, sorted and deduplicated.
        let build = Spec::new(Utf8PathBuf::from("/work/openapi.yaml"))
            .build(Utf8PathBuf::from("/work/out/api.rs"));
        let snapshot = InputSnapshot {
            digest: String::new(),
            paths: vec![
                Utf8PathBuf::from("/work/openapi.yaml"),
                Utf8PathBuf::from("/work/x\ncargo:rustc-cfg=injected.yaml"),
                Utf8PathBuf::from("/work/y\rcargo:rustc-cfg=injected.yaml"),
                Utf8PathBuf::from("/work/openapi.yaml"),
                Utf8PathBuf::from("/work/a.yaml"),
            ],
        };
        assert_eq!(
            rerun_directives(&build, Some(&snapshot)),
            [
                "cargo:rerun-if-changed=/work/a.yaml",
                "cargo:rerun-if-changed=/work/openapi.yaml",
                "cargo:rerun-if-changed=/work/out/api.rs",
            ]
        );

        // Without a snapshot only the root spec and the output are named, under the same guard.
        let crafted = Spec::new(Utf8PathBuf::from("/work/z\ncargo:rustc-cfg=injected.yaml"))
            .build(Utf8PathBuf::from("/work/out/api.rs"));
        assert_eq!(
            rerun_directives(&crafted, None),
            ["cargo:rerun-if-changed=/work/out/api.rs"]
        );
    }

    #[test]
    fn an_applied_config_file_is_a_rerun_trigger_and_a_fingerprinted_input() {
        // Editing only `spargen.toml` changes what is generated, so Cargo must rerun the build
        // script for it (#686). Both the discovered file and an explicitly named one count, with
        // and without a snapshot.
        let (temp, build) = fixture();
        let discovered = temp.path().join("spargen.toml");
        std::fs::write(&discovered, "batch_cap = 7\n").unwrap();
        let discovered = Utf8PathBuf::from_path_buf(discovered).unwrap();
        let explicit = Utf8PathBuf::from_path_buf(temp.path().join("other.toml")).unwrap();
        std::fs::write(&explicit, "carve = true\n").unwrap();
        let spec = build
            .spec
            .clone()
            .discover_config_file()
            .unwrap()
            .config_file(&explicit)
            .unwrap();
        let configured = Build {
            spec,
            ..build.clone()
        };

        let snapshot = InputSnapshot::load(&configured.spec).unwrap();
        for path in [&discovered, &explicit] {
            assert!(snapshot.paths.contains(path), "{:?}", snapshot.paths);
            let directive = format!("cargo:rerun-if-changed={path}");
            for directives in [
                rerun_directives(&configured, Some(&snapshot)),
                rerun_directives(&configured, None),
            ] {
                assert!(directives.contains(&directive), "{directives:#?}");
            }
        }

        // A comment-only edit applies the same knobs, yet the file's bytes are a build input
        // like `spargen.lock`'s, so the digest moves with them.
        std::fs::write(&discovered, "batch_cap = 7 # edited\n").unwrap();
        assert_ne!(
            InputSnapshot::load(&configured.spec).unwrap().digest,
            snapshot.digest
        );

        // No file beside the spec: nothing is recorded, so nothing absent is named to Cargo.
        std::fs::remove_file(&discovered).unwrap();
        let undiscovered = build.spec.clone().discover_config_file().unwrap();
        assert!(undiscovered.config_files.is_empty());
    }

    #[test]
    fn a_replayed_diagnostic_makes_the_claim_the_fresh_one_made() {
        // A cache hit replays the fresh run's warnings under `Outcome::Cached`. The claim is not
        // re-derived from the code, so one that differs from its severity's survives the replay.
        // The record stores each claim in the spelling the public type serializes as.
        for (claim, spelling) in [
            (OutcomeClaim::Independent, "independent"),
            (OutcomeClaim::Rejected, "rejected"),
            (OutcomeClaim::Generated, "generated"),
        ] {
            let fresh = Diagnostic {
                claim,
                ..Diagnostic::warning(
                    crate::Code::AlternativeMediaIgnored,
                    crate::diag::Provenance::new(JsonPointer::root(), None),
                )
                .build()
            };
            let recorded = serde_json::to_value(CachedDiagnostic::from(&fresh)).unwrap();
            assert_eq!(recorded["claim"], spelling);
            assert_eq!(recorded["claim"], serde_json::to_value(claim).unwrap());
            let replayed = serde_json::from_value::<CachedDiagnostic>(recorded)
                .unwrap()
                .into_diagnostic()
                .unwrap();
            assert_eq!(replayed.claim, claim);
        }
    }

    #[test]
    fn fingerprint_covers_transitive_files_lock_and_config() {
        let (temp, config) = fixture();
        let initial = InputSnapshot::load(&config.spec).unwrap();
        assert_eq!(initial.paths.len(), 2);

        std::fs::write(temp.path().join("schema.yaml"), "type: integer\n").unwrap();
        let transitive_changed = InputSnapshot::load(&config.spec).unwrap();
        assert_ne!(initial.digest, transitive_changed.digest);

        std::fs::write(temp.path().join("spargen.lock"), "version = 1\n").unwrap();
        let lock_changed = InputSnapshot::load(&config.spec).unwrap();
        assert_ne!(transitive_changed.digest, lock_changed.digest);
        assert_eq!(lock_changed.paths.len(), 3);

        let mut configured = config;
        configured.spec = configured.spec.clone().error_body_cap(1);
        let config_changed = InputSnapshot::load(&configured.spec).unwrap();
        assert_ne!(lock_changed.digest, config_changed.digest);
    }

    #[test]
    fn the_input_paths_are_the_ones_the_snapshot_fingerprints() {
        // `input_paths` is what `generate_api!` tracks: the paths `InputSnapshot::load` hashes,
        // read without hashing them — the root, each `$ref` target and `spargen.lock`, sorted.
        let (temp, build) = fixture();
        std::fs::write(temp.path().join("spargen.lock"), "version = 1\n").unwrap();
        let root = Utf8Path::from_path(temp.path()).unwrap();
        let paths = super::input_paths(&build.spec).unwrap();
        assert_eq!(
            paths,
            [
                root.join("openapi.yaml"),
                root.join("schema.yaml"),
                root.join("spargen.lock"),
            ]
        );
        assert_eq!(paths, InputSnapshot::load(&build.spec).unwrap().paths);
    }

    #[test]
    fn output_fingerprints_detect_missing_stale_and_edited_modules() {
        let (_temp, config) = fixture();
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
    }

    #[test]
    fn generation_seeds_cache_and_repairs_missing_or_edited_output() {
        let (temp, config) = fixture();
        let cache_dir = Utf8PathBuf::from_path_buf(temp.path().join("cache")).unwrap();

        let first = crate::pipeline::generate_with_cache_dir(&config, Some(&cache_dir));
        assert_eq!(first.outcome(), Outcome::Generated, "{first:#?}");
        let original = std::fs::read_to_string(&config.output).unwrap();
        let record = cache_path(&cache_dir, &config.output);
        assert!(record.is_file(), "generation must seed the target cache");
        let written_at = std::fs::metadata(&config.output)
            .unwrap()
            .modified()
            .unwrap();

        std::fs::remove_file(&record).unwrap();
        let seeded = crate::pipeline::generate_with_cache_dir(&config, Some(&cache_dir));
        assert_eq!(seeded.outcome(), Outcome::Generated, "{seeded:#?}");
        assert!(
            record.is_file(),
            "verified committed output must reseed cache"
        );
        assert_eq!(std::fs::read_to_string(&config.output).unwrap(), original);
        // The rerun rendered the bytes already on disk, and `write_output` leaves such a file
        // alone, so the reseed does not touch the module.
        assert_eq!(
            std::fs::metadata(&config.output)
                .unwrap()
                .modified()
                .unwrap(),
            written_at,
            "a rerun that renders identical bytes must not rewrite the output"
        );

        std::fs::write(&config.output, format!("{original}// manual edit\n")).unwrap();
        let repaired = crate::pipeline::generate_with_cache_dir(&config, Some(&cache_dir));
        assert_eq!(repaired.outcome(), Outcome::Generated, "{repaired:#?}");
        assert_eq!(std::fs::read_to_string(&config.output).unwrap(), original);

        std::fs::remove_file(&config.output).unwrap();
        let restored = crate::pipeline::generate_with_cache_dir(&config, Some(&cache_dir));
        assert_eq!(restored.outcome(), Outcome::Generated, "{restored:#?}");
        assert_eq!(std::fs::read_to_string(&config.output).unwrap(), original);
    }

    /// An unchanged rebuild replays the cache: the path through `read_cache` that returns
    /// `Outcome::Cached`, which every test above, destroying the record or the output first,
    /// leaves untaken.
    #[test]
    fn an_unchanged_rebuild_is_a_cache_hit_that_rewrites_nothing() {
        let (temp, config) = fixture();
        let cache_dir = Utf8PathBuf::from_path_buf(temp.path().join("cache")).unwrap();

        let first = crate::pipeline::generate_with_cache_dir(&config, Some(&cache_dir));
        assert_eq!(first.outcome(), Outcome::Generated, "{first:#?}");
        let generated = std::fs::read_to_string(&config.output).unwrap();
        let written_at = std::fs::metadata(&config.output)
            .unwrap()
            .modified()
            .unwrap();

        let second = crate::pipeline::generate_with_cache_dir(&config, Some(&cache_dir));
        assert_eq!(
            second.outcome(),
            Outcome::Cached,
            "an unchanged rebuild must hit the cache: {second:#?}"
        );
        assert_eq!(std::fs::read_to_string(&config.output).unwrap(), generated);
        assert_eq!(
            std::fs::metadata(&config.output)
                .unwrap()
                .modified()
                .unwrap(),
            written_at,
            "a cache hit must not rewrite the output"
        );
    }

    /// `read_cache` gives up, and the build reruns the pipeline, on each record it cannot trust: an
    /// absent one, one that does not parse, and one whose format, input digest or content digest
    /// is not the current one. Before each corruption the record is shown to be a hit, so the
    /// rerun is the corruption's doing; after it the rerun has written a record that hits again.
    #[test]
    fn a_corrupt_or_stale_cache_record_reruns_the_pipeline() {
        let (temp, config) = fixture();
        let cache_dir = Utf8PathBuf::from_path_buf(temp.path().join("cache")).unwrap();
        let record = cache_path(&cache_dir, &config.output);
        let first = crate::pipeline::generate_with_cache_dir(&config, Some(&cache_dir));
        assert_eq!(first.outcome(), Outcome::Generated, "{first:#?}");

        // A corruption that moves one field of the record: a number up by one, a digest by an
        // appended digit.
        let edit_field = |field: &str| {
            let mut value: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&record).unwrap()).unwrap();
            let slot = value
                .get_mut(field)
                .unwrap_or_else(|| panic!("the record has no `{field}` field"));
            *slot = match slot.take() {
                serde_json::Value::Number(format) => {
                    (format.as_u64().expect("the format is a u64") + 1).into()
                }
                serde_json::Value::String(digest) => format!("{digest}0").into(),
                other => panic!("`{field}` is neither a number nor a string: {other}"),
            };
            std::fs::write(&record, serde_json::to_vec(&value).unwrap()).unwrap();
        };
        let corruptions = [
            ("absent", None),
            ("not JSON", None),
            ("another format", Some("format")),
            ("another input digest", Some("input_sha256")),
            ("another content digest", Some("content_sha256")),
        ];

        for (corruption, field) in corruptions {
            let hit = crate::pipeline::generate_with_cache_dir(&config, Some(&cache_dir));
            assert_eq!(
                hit.outcome(),
                Outcome::Cached,
                "the record must hit before it is made {corruption}: {hit:#?}"
            );
            match (corruption, field) {
                (_, Some(field)) => edit_field(field),
                ("absent", None) => std::fs::remove_file(&record).unwrap(),
                (_, None) => std::fs::write(&record, b"not a cache record").unwrap(),
            }
            let rerun = crate::pipeline::generate_with_cache_dir(&config, Some(&cache_dir));
            assert_eq!(
                rerun.outcome(),
                Outcome::Generated,
                "a record made {corruption} must rerun the pipeline: {rerun:#?}"
            );
        }
        let last = crate::pipeline::generate_with_cache_dir(&config, Some(&cache_dir));
        assert_eq!(last.outcome(), Outcome::Cached, "{last:#?}");
    }

    /// Every fingerprint field is written with its length in front, so two fields whose
    /// concatenations agree, such as a path and its file's bytes split at another point, never
    /// frame to the same bytes and so never share a digest.
    #[test]
    fn the_fingerprint_frames_each_input_so_adjacent_fields_cannot_merge() {
        let framed = |fields: &[&[u8]]| {
            let mut hasher = Sha256::new();
            for field in fields {
                append(&mut hasher, field);
            }
            hasher.finalize()
        };
        assert_ne!(
            framed(&[b"/spec/a.yaml", b"type: string\n"]),
            framed(&[b"/spec/a.yamltype", b": string\n"])
        );
        assert_ne!(framed(&[b"ab", b""]), framed(&[b"", b"ab"]));
        assert_ne!(framed(&[b"ab"]), framed(&[b"a", b"b"]));
        // The frame is the field's length as eight big-endian bytes, then the field.
        assert_eq!(
            framed(&[b"ab"]),
            sha256_hex(&[0, 0, 0, 0, 0, 0, 0, 2, b'a', b'b'])
        );
    }

    /// The input digest is hashed field by field, and stays the digest of the one framed buffer
    /// the cache format defines: the literal pins it, so an existing cache record and generated
    /// module stay valid, and the buffer spelled out below states the format it is the hash of.
    #[test]
    fn the_input_fingerprint_is_the_digest_of_the_framed_input_buffer() {
        let spec = Spec {
            path: Utf8PathBuf::from("/work/openapi.yaml"),
            uuid: true,
            datetime: DateTimeBackend::String,
            omit: crate::Omit::default(),
            error_body_cap: 4096,
            batch_cap: 64,
            carve: true,
            open_narrowing: false,
            open_unions: true,
            verbatim_docs: false,
            bind_headers: vec!["X-Api-Version".to_owned()],
            config_files: Vec::new(),
        };
        // A file over a block long, so its bytes cross a block boundary of the hash.
        let schema = "type: string\n".repeat(20);
        let inputs: [(&Utf8Path, &[u8]); 2] = [
            (
                Utf8Path::new("/work/openapi.yaml"),
                b"openapi: 3.1.0\npaths: {}\n",
            ),
            (Utf8Path::new("/work/schema.yaml"), schema.as_bytes()),
        ];

        let mut buffer = b"spargen-build-input-v1\0".to_vec();
        let mut frame = |field: &[u8]| {
            buffer.extend_from_slice(&(field.len() as u64).to_be_bytes());
            buffer.extend_from_slice(field);
        };
        frame(b"1.2.3");
        frame(b"build");
        frame(b"/work/openapi.yaml");
        frame(&[1]);
        frame(&[0]);
        frame(&4096u64.to_be_bytes());
        frame(&64u64.to_be_bytes());
        frame(&[1]);
        frame(&[0]);
        frame(&[1]);
        frame(&[0]);
        frame(&spec.omit.canonical_encoding());
        frame(&1u64.to_be_bytes());
        frame(b"X-Api-Version");
        for (path, bytes) in inputs {
            frame(path.as_str().as_bytes());
            frame(bytes);
        }

        let digest = fingerprint(&spec, "1.2.3", "build", &inputs);
        assert_eq!(digest, sha256_hex(&buffer));
        assert_eq!(
            digest,
            "f09a08df3695356f4f5d12723bf761ce18b9eb1c3bf898493b4364cb062dacf6"
        );
    }

    #[test]
    fn a_cache_hit_replays_the_diagnostics_of_the_run_that_seeded_it() {
        // A cached run reports what the original run reported. Without this a warning would be
        // announced once and then vanish on every later build, which is worse than never
        // reporting it — the spec still contains the construct. The warning sits in a `$ref`'d
        // file, so the replay must also keep the file its span lies in (the record's `file`).
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("openapi.yaml");
        std::fs::write(
            &root,
            r#"openapi: 3.1.0
info: { title: Cache, version: 1.0.0 }
paths: {}
components:
  schemas:
    Capped: { $ref: './schemas.yaml#/Capped' }
"#,
        )
        .unwrap();
        std::fs::write(
            temp.path().join("schemas.yaml"),
            "Capped: { type: string, minLength: 3 }\n",
        )
        .unwrap();
        let config = Spec::new(Utf8PathBuf::from_path_buf(root).unwrap())
            .build(Utf8PathBuf::from_path_buf(temp.path().join("api.rs")).unwrap())
            .cargo(CargoIntegration::Off);
        let cache_dir = Utf8PathBuf::from_path_buf(temp.path().join("cache")).unwrap();

        let first = crate::pipeline::generate_with_cache_dir(&config, Some(&cache_dir));
        assert_eq!(first.outcome(), Outcome::Generated, "{first:#?}");
        let mut seeded: Vec<(&str, Option<&str>)> = first
            .diagnostics()
            .iter()
            .map(|diagnostic| (diagnostic.code.as_str(), diagnostic.file.as_deref()))
            .collect();
        seeded.sort_unstable();
        let warned_in = seeded
            .iter()
            .find(|(code, _)| *code == "W001")
            .unwrap_or_else(|| {
                panic!("the fixture must warn so there is something to replay: {seeded:?}")
            })
            .1;
        assert!(
            warned_in.is_some_and(|file| file.ends_with("schemas.yaml")),
            "the seeding run must name the referenced file the warning lies in: {seeded:?}"
        );

        let second = crate::pipeline::generate_with_cache_dir(&config, Some(&cache_dir));
        assert_eq!(second.outcome(), Outcome::Cached, "{second:#?}");
        let mut replayed: Vec<(&str, Option<&str>)> = second
            .diagnostics()
            .iter()
            .map(|diagnostic| (diagnostic.code.as_str(), diagnostic.file.as_deref()))
            .collect();
        replayed.sort_unstable();
        assert_eq!(
            replayed, seeded,
            "a cache hit lost the original diagnostics or the files they lie in"
        );
    }

    #[test]
    fn a_cache_hit_replays_the_truncation_of_the_run_that_seeded_it() {
        // The record stores whether the diagnostics were cut at `batch_cap`, so a second build
        // never presents a partial list as the whole one (#724).
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("openapi.yaml");
        std::fs::write(
            &root,
            r#"openapi: 3.1.0
info: { title: Cache, version: 1.0.0 }
paths: {}
components:
  schemas:
    First: { type: string, minLength: 3 }
    Second: { type: string, maxLength: 5 }
"#,
        )
        .unwrap();
        let config = Spec::new(Utf8PathBuf::from_path_buf(root).unwrap())
            .batch_cap(1)
            .build(Utf8PathBuf::from_path_buf(temp.path().join("api.rs")).unwrap())
            .cargo(CargoIntegration::Off);
        let cache_dir = Utf8PathBuf::from_path_buf(temp.path().join("cache")).unwrap();

        let first = crate::pipeline::generate_with_cache_dir(&config, Some(&cache_dir));
        assert_eq!(first.outcome(), Outcome::Generated, "{first:#?}");
        assert!(
            first.truncated(),
            "the fixture must warn past batch_cap: {first:#?}"
        );

        let second = crate::pipeline::generate_with_cache_dir(&config, Some(&cache_dir));
        assert_eq!(second.outcome(), Outcome::Cached, "{second:#?}");
        assert!(
            second.truncated(),
            "a cache hit presented a truncated list as complete: {second:#?}"
        );
        // Past the outcome line, which says `cached` rather than `generated`, the hit prints what
        // the seeding run printed: the same diagnostics and the same truncation notice.
        let body = |report: &crate::Report| {
            let text = report.to_string();
            text.split_once('\n').map(|(_, rest)| rest.to_owned())
        };
        assert_eq!(body(&second), body(&first));
        assert!(
            body(&second).is_some_and(|text| text.contains("truncated at batch_cap")),
            "{second}"
        );
    }

    /// CLAUDE.md calls the fingerprint "complete". Every `Spec` field changes what is generated,
    /// so every one must move the digest — otherwise a config change leaves a stale module in place
    /// with nothing to tell the consumer. `Spec` is destructured with no `..` rest pattern, so a new
    /// field fails to compile here until it is given a variant below.
    #[test]
    fn every_spec_field_moves_the_input_fingerprint() {
        let (temp, config) = fixture();
        let base = config.spec.clone();
        let baseline = InputSnapshot::load(&base).unwrap().digest;
        let Spec {
            path,
            uuid,
            datetime,
            omit,
            error_body_cap,
            batch_cap,
            carve,
            open_narrowing,
            open_unions,
            verbatim_docs,
            bind_headers,
            config_files,
        } = &base;
        assert!(config_files.is_empty());
        let config = Utf8PathBuf::from_path_buf(temp.path().join("spargen.toml")).unwrap();
        std::fs::write(&config, "# applies nothing\n").unwrap();

        // Byte-identical content at a different path is still a different build input, because
        // the provenance header records the path.
        let moved = temp.path().join("elsewhere.yaml");
        std::fs::copy(path, &moved).unwrap();
        let mut more_omitted = omit.clone();
        more_omitted.rules.push(OmitRule::Path {
            path: "/things".into(),
        });

        let mut variants: Vec<(&str, Spec)> = vec![
            (
                "path",
                Spec {
                    path: Utf8PathBuf::from_path_buf(moved).unwrap(),
                    ..base.clone()
                },
            ),
            (
                "uuid",
                Spec {
                    uuid: !uuid,
                    ..base.clone()
                },
            ),
            (
                "omit",
                Spec {
                    omit: more_omitted,
                    ..base.clone()
                },
            ),
            (
                "error_body_cap",
                Spec {
                    error_body_cap: error_body_cap + 1,
                    ..base.clone()
                },
            ),
            (
                "batch_cap",
                Spec {
                    batch_cap: batch_cap + 1,
                    ..base.clone()
                },
            ),
            (
                "carve",
                Spec {
                    carve: !carve,
                    ..base.clone()
                },
            ),
            (
                "open_narrowing",
                Spec {
                    open_narrowing: !open_narrowing,
                    ..base.clone()
                },
            ),
            (
                "open_unions",
                Spec {
                    open_unions: !open_unions,
                    ..base.clone()
                },
            ),
            (
                "verbatim_docs",
                Spec {
                    verbatim_docs: !verbatim_docs,
                    ..base.clone()
                },
            ),
            (
                "bind_headers",
                Spec {
                    bind_headers: bind_headers
                        .iter()
                        .cloned()
                        .chain(["X-Api-Version".to_owned()])
                        .collect(),
                    ..base.clone()
                },
            ),
            (
                // A file that sets no knob still moves the digest: its bytes are an input.
                "config_files",
                Spec {
                    config_files: vec![config],
                    ..base.clone()
                },
            ),
        ];
        // Every backend other than the base's is its own build input.
        variants.extend(
            DateTimeBackend::ALL
                .into_iter()
                .filter(|backend| backend != datetime)
                .map(|backend| {
                    (
                        "datetime",
                        Spec {
                            datetime: backend,
                            ..base.clone()
                        },
                    )
                }),
        );

        for (field, spec) in variants {
            assert_ne!(
                InputSnapshot::load(&spec).unwrap().digest,
                baseline,
                "changing `{field}` left the input fingerprint unchanged, so a rebuild would \
                 serve the module built for the previous value"
            );
        }
    }

    #[test]
    fn two_omit_rule_sets_that_differ_are_not_one_fingerprint() {
        let (_temp, config) = fixture();
        let base = config.spec.clone();

        let by_path = base.clone().omit_rule(OmitRule::Path {
            path: "/things".into(),
        });
        let by_other_path = base.clone().omit_rule(OmitRule::Path {
            path: "/others".into(),
        });
        let by_component = base.clone().omit_rule(OmitRule::Component {
            kind: crate::ComponentKind::Schemas,
            name: "things".into(),
        });

        let digest = |spec: &Spec| InputSnapshot::load(spec).unwrap().digest;
        assert_ne!(digest(&by_path), digest(&by_other_path));
        // The rule *kind* is part of the fingerprint, not just its payload.
        assert_ne!(digest(&by_path), digest(&by_component));

        // A pointer rule with no file and one naming an empty file are different rules, and the
        // shared canonical encoding keeps them apart.
        let no_file = base.clone().omit_rule(OmitRule::Pointer {
            file: None,
            pointer: "/paths/~1things".into(),
        });
        let empty_file = base.clone().omit_rule(OmitRule::Pointer {
            file: Some("".into()),
            pointer: "/paths/~1things".into(),
        });
        assert_ne!(digest(&no_file), digest(&empty_file));
    }
}
