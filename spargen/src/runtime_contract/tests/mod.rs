//! The audit's fixtures, one file per concern: `versions` (floors, ceilings and conditional
//! requirements), `targets` (target-table evaluation), `workspace` (inheritance and the
//! workspace-root walk), `explain` (the `E023` explain text) and `deps_block` (the block
//! `spargen deps` prints). This file holds what they share: the sandbox every audit runs in, the
//! helpers, the list of test sources the self-reading tests scan, and the tests over those.

mod deps_block;
mod explain;
mod targets;
mod versions;
mod workspace;

use cfg_expr::targets::{get_builtin_target_by_triple, ALL_BUILTINS};
use cfg_expr::Expression;
use semver::Version;

use super::check::*;
use super::requirements::*;
use super::target::*;
use super::*;

/// Every file of this test module with its source, for the tests that read the fixtures' text:
/// the `E023` explain test, which checks each fixture it cites is a `#[test]` here, and
/// `every_fixture_bounds_its_walk_by_its_own_sandbox`.
///
/// Each source is an `include_str!`, so a file listed here that goes missing fails to compile
/// rather than leaving those tests reading less; a file added beside these and not listed fails
/// `the_test_sources_are_every_file_of_the_test_module`.
const TEST_SOURCES: &[(&str, &str)] = &[
    ("deps_block.rs", include_str!("deps_block.rs")),
    ("explain.rs", include_str!("explain.rs")),
    ("mod.rs", include_str!("mod.rs")),
    ("targets.rs", include_str!("targets.rs")),
    ("versions.rs", include_str!("versions.rs")),
    ("workspace.rs", include_str!("workspace.rs")),
];

/// Whether `name` is a `#[test]` in any file of this test module.
fn is_test_fn(name: &str) -> bool {
    TEST_SOURCES
        .iter()
        .any(|(_, source)| crate::diag::is_test_fn(source, name))
}

#[test]
fn the_test_sources_are_every_file_of_the_test_module() {
    let directory = concat!(env!("CARGO_MANIFEST_DIR"), "/src/runtime_contract/tests");
    let on_disk: std::collections::BTreeSet<String> = std::fs::read_dir(directory)
        .expect("the test module's directory is readable")
        .map(|entry| {
            entry
                .expect("readable directory entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    let listed: std::collections::BTreeSet<String> = TEST_SOURCES
        .iter()
        .map(|(name, _)| (*name).to_owned())
        .collect();
    assert_eq!(
        on_disk, listed,
        "list every file of src/runtime_contract/tests/ in `TEST_SOURCES`, or the tests that read \
         the fixtures' source read less than the module holds"
    );
}

const CORE_MANIFEST: &str = r#"
[package]
name = "consumer"
version = "0.0.0"

[dependencies]
bytes = "1.12.1"
reqwest = { version = "0.12.28", default-features = false }
secrecy = "0.10.3"
serde = { version = "1.0.229", features = ["derive"] }
serde_json = "1.0.151"
"#;

/// The five core dependencies as a `[workspace.dependencies]` body, reusing `CORE_MANIFEST` so
/// the floors in these fixtures cannot drift from the ones every other test audits against.
fn core_workspace_dependencies() -> &'static str {
    CORE_MANIFEST.split_once("[dependencies]\n").unwrap().1
}

/// `CORE_MANIFEST`'s whole line for the dependency `key`, and the version it pins.
///
/// Every fixture that rewrites a core entry locates it here, so the floors in `CORE_MANIFEST`
/// are stated once in this module: restating one at a call site made a bump there red the
/// fixture on its needle instead of on anything the fixture is about. The key is matched as
/// `key = ` at the start of a line, and a missing entry or unquoted version fails loudly.
fn core_entry(key: &str) -> (&'static str, &'static str) {
    let prefix = format!("{key} = ");
    let line = core_workspace_dependencies()
        .lines()
        .find(|line| line.starts_with(&prefix))
        .unwrap_or_else(|| panic!("CORE_MANIFEST declares `{key}` under that key"));
    let version = line
        .split('"')
        .nth(1)
        .unwrap_or_else(|| panic!("CORE_MANIFEST's `{key}` entry pins a quoted version"));
    (line, version)
}

const CORE_INHERITED: &str = "\
[dependencies]
bytes.workspace = true
reqwest.workspace = true
secrecy.workspace = true
serde.workspace = true
serde_json.workspace = true
";

/// A build-script environment as Cargo sets it: `TARGET` plus `CARGO_CFG_*`, each `(cfg,
/// value)` pair written without the prefix.
fn build_target(triple: &str, cfgs: &[(&str, &str)]) -> TargetContext {
    TargetContext::Build(BuildTarget::from_vars(
        std::iter::once(("TARGET".to_owned(), triple.to_owned()))
            .chain(
                cfgs.iter()
                    .map(|(cfg, value)| (format!("CARGO_CFG_{cfg}"), (*value).to_owned())),
            )
            // Unrelated variables are ignored, as a real build script's environment has many.
            .chain(std::iter::once(("PATH".to_owned(), "/bin".to_owned()))),
    ))
}

const LINUX_CFGS: &[(&str, &str)] = &[
    ("TARGET_ARCH", "x86_64"),
    ("TARGET_OS", "linux"),
    ("TARGET_FAMILY", "unix"),
    ("UNIX", ""),
    ("TARGET_ENV", "gnu"),
    ("TARGET_VENDOR", "unknown"),
    ("TARGET_POINTER_WIDTH", "64"),
    ("TARGET_ENDIAN", "little"),
    ("TARGET_HAS_ATOMIC", "8,16,32,64,ptr"),
    ("PANIC", "unwind"),
];

fn linux() -> TargetContext {
    build_target("x86_64-unknown-linux-gnu", LINUX_CFGS)
}

/// A temporary directory that is also the ceiling of every workspace-root walk audited through
/// it, and the only way a fixture here makes one.
///
/// The walk otherwise climbs out of the directory to the filesystem root, so a `Cargo.toml`
/// that some unrelated process left in `/tmp` or `/` decided the outcome: one declaring
/// `[workspace]` turned every no-root fixture into a root-found one, and one that does not parse
/// became the candidate their messages name (#214). Auditing through [`Sandbox::audit`] bounds
/// the walk to this directory, so a fixture reads only the files it wrote.
///
/// The path is canonicalized once, here. The ceiling is compared lexically against the
/// absolutized manifest path, and a relative manifest path is absolutized against the working
/// directory, which the platform reports canonically: where the temporary directory is reached
/// through a symlink, an uncanonicalized ceiling would never be met and the walk would climb
/// past it.
///
/// `every_fixture_bounds_its_walk_by_its_own_sandbox` holds every fixture in this module to it.
struct Sandbox {
    root: Utf8PathBuf,
    _directory: tempfile::TempDir,
}

impl Sandbox {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(temporary.path().canonicalize().unwrap()).unwrap();
        Self {
            root,
            _directory: temporary,
        }
    }

    fn path(&self) -> &std::path::Path {
        self.root.as_std_path()
    }

    /// [`audit`], bounded by this sandbox.
    fn audit(&self, manifest_path: &Utf8Path, requirements: &RuntimeRequirements) -> Audit {
        self.audit_in(manifest_path, requirements, &TargetContext::from_env())
    }

    /// [`audit_in`], bounded by this sandbox, with each message's audited-manifest suffix
    /// checked and removed.
    ///
    /// Every fixture that audits through here therefore also asserts that each diagnostic it
    /// sees names the absolutized `manifest_path` exactly as [`diagnostic`] renders it, and the
    /// wording each fixture pins stays the message proper. A relative `manifest_path` is
    /// absolutized against the working directory the audit itself ran under.
    fn audit_in(
        &self,
        manifest_path: &Utf8Path,
        requirements: &RuntimeRequirements,
        target: &TargetContext,
    ) -> Audit {
        let mut result = self.audit_unstripped(manifest_path, requirements, target);
        let suffix = format!("{AUDITED_MANIFEST}`{}`", absolutized(manifest_path));
        for diagnostic in &mut result.diagnostics {
            let Some(message) = diagnostic.message.strip_suffix(&suffix) else {
                panic!(
                    "an E023 message does not end by naming the audited manifest \
                     `{manifest_path}` as `{suffix}`: {}",
                    diagnostic.message
                );
            };
            assert!(
                !message.contains(AUDITED_MANIFEST),
                "an E023 message names the audited manifest more than once: {}",
                diagnostic.message
            );
            diagnostic.message = message.to_owned();
        }
        result
    }

    /// [`audit_in`], bounded by this sandbox, messages exactly as they are rendered.
    fn audit_unstripped(
        &self,
        manifest_path: &Utf8Path,
        requirements: &RuntimeRequirements,
        target: &TargetContext,
    ) -> Audit {
        audit_in(manifest_path, requirements, target, Some(&self.root))
    }
}

fn audit_manifest(contents: &str, requirements: RuntimeRequirements) -> Vec<Diagnostic> {
    let directory = Sandbox::new();
    let manifest = Utf8PathBuf::from_path_buf(directory.path().join("Cargo.toml")).unwrap();
    std::fs::write(&manifest, contents).unwrap();
    directory
        .audit_in(&manifest, &requirements, &TargetContext::Unknown)
        .diagnostics
}

/// `haystack` with its one occurrence of `needle` replaced by `with`.
///
/// A plain `str::replace` whose needle has gone stale — a floor bumped in `CORE_MANIFEST`, say
/// — silently returns the manifest unchanged, and the fixture then fails on an assertion about
/// the contract instead of on the needle. Requiring exactly one match makes it fail as what it
/// is.
fn replace_once(haystack: &str, needle: &str, with: &str) -> String {
    assert_eq!(
        haystack.matches(needle).count(),
        1,
        "fixture needle {needle:?} must occur exactly once in {haystack}"
    );
    haystack.replacen(needle, with, 1)
}

fn audit_manifest_for(contents: &str, target: &TargetContext) -> Vec<Diagnostic> {
    let directory = Sandbox::new();
    let manifest = Utf8PathBuf::from_path_buf(directory.path().join("Cargo.toml")).unwrap();
    std::fs::write(&manifest, contents).unwrap();
    directory
        .audit_in(&manifest, &RuntimeRequirements::default(), target)
        .diagnostics
}

fn messages(diagnostics: &[Diagnostic]) -> String {
    diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn every_fixture_bounds_its_walk_by_its_own_sandbox() {
    // #214 was thirteen fixtures, each making its own temporary directory and auditing through
    // the unbounded entry point, so each climbed out through `/tmp` to `/`. A fourteenth written
    // the same way would reintroduce it with every test green on a clean host, so the shape is
    // held here: `Sandbox::new` is the one place this module makes a temporary directory, and
    // `Sandbox::audit_in` the one place it calls the audit directly. Every other audit goes
    // through a sandbox, and so is bounded by it. The needles are assembled so this test does
    // not count itself. Every file of the test module is read, through `TEST_SOURCES`.
    let tempdir = concat!("tempfile::", "tempdir(");
    assert_eq!(
        TEST_SOURCES
            .iter()
            .map(|(_, tests)| tests.matches(tempdir).count())
            .sum::<usize>(),
        1,
        "make a fixture's directory with `Sandbox::new()`, not `{tempdir})`"
    );
    // The arguments of each call not reached through a receiver — `name(` preceded by neither
    // `.` nor an identifier character, and not a definition — up to the first `)`.
    let bare_calls = |name: &str| -> Vec<&str> {
        let call = format!("{name}(");
        TEST_SOURCES
            .iter()
            .flat_map(|(_, tests)| {
                tests
                    .match_indices(&call)
                    .filter(|(at, _)| {
                        let before = &tests[..*at];
                        !before.ends_with('.')
                            && !before.ends_with("fn ")
                            && !before
                                .chars()
                                .next_back()
                                .is_some_and(|c| c.is_alphanumeric() || c == '_')
                    })
                    .map(|(at, _)| {
                        let arguments = &tests[at + call.len()..];
                        arguments
                            .split_once(')')
                            .map_or(arguments, |(head, _)| head)
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    };
    assert_eq!(
        bare_calls(concat!("aud", "it")),
        Vec::<&str>::new(),
        "audit through `Sandbox::audit`, which bounds the walk"
    );
    assert_eq!(
        bare_calls(concat!("aud", "it_in")).len(),
        1,
        "audit through `Sandbox::audit_in`, which bounds the walk"
    );
    // `workspace_root` is called directly only by the fixtures that pin the ceiling itself, and
    // only `the_production_walk_has_no_ceiling` runs it unbounded, from a sandbox of its own.
    let unbounded = bare_calls(concat!("workspace", "_root"))
        .into_iter()
        .filter(|arguments| arguments.trim_end().ends_with("None"))
        .count();
    assert_eq!(
        unbounded, 1,
        "bound a direct walk by its sandbox, as `the_walk_reads_nothing_above_its_ceiling` does"
    );
}

#[test]
fn a_manifest_path_with_a_line_break_is_never_written_as_a_directive() {
    // Deleting the guard left the whole suite green (#202). Written out, the text after the
    // break would reach Cargo as a directive of its own, so a crafted directory name could
    // inject one. Every other path is still named, in the order given.
    let manifests = [
        Utf8PathBuf::from("/work/client/Cargo.toml"),
        Utf8PathBuf::from("/work/x\ncargo:rustc-cfg=injected/Cargo.toml"),
        Utf8PathBuf::from("/work/y\rcargo:rustc-cfg=injected/Cargo.toml"),
        Utf8PathBuf::from("/work/Cargo.toml"),
    ];
    assert_eq!(
        rerun_if_changed_lines(&manifests),
        [
            "cargo:rerun-if-changed=/work/client/Cargo.toml",
            "cargo:rerun-if-changed=/work/Cargo.toml",
        ]
    );
}
