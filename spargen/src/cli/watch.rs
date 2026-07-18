//! `generate --watch`: regenerate whenever the spec (or a file it references), the `spargen.toml`
//! config, or the `spargen.lock` changes — running until interrupted (Ctrl-C).
//!
//! Watching is a CLI concern, so the file watcher (`notify-debouncer-mini`) rides the `cli` feature
//! and never enters a library/`build.rs` graph nor the generated-output runtime.
//!
//! The OS-facing event loop is kept deliberately thin so the core is unit-testable without any
//! filesystem-event timing:
//! - [`regenerate_once`] is the single per-change action — run the library facade [`generate`] and
//!   render its report. It is exactly the one-shot path, so a watched regenerate can never diverge
//!   from a plain `spargen generate`.
//! - [`watched_files`] computes the file set to watch (spec + `$ref` targets + vendored copies +
//!   config + lock) from the on-disk state alone.
//!
//! Both are exercised directly by the tests below; [`watch`] merely wires them to `notify` and
//! debounces bursts of events.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use camino::{Utf8Path, Utf8PathBuf};
use notify_debouncer_mini::new_debouncer;
use notify_debouncer_mini::notify::{RecursiveMode, Watcher};

use crate::{generate, Config, Report};

use super::run::{render_report, status_for_report};
use super::{ExitStatus, Format};

/// Debounce window: coalesce a burst of save events (editors often write a file in several
/// syscalls, or via an atomic rename) into a single regeneration.
const DEBOUNCE: Duration = Duration::from_millis(200);

/// Run the watch loop: generate once, then regenerate on every relevant change until interrupted.
///
/// Ctrl-C (SIGINT) terminates the process — the loop itself blocks on filesystem events and never
/// exits on its own. A failing regenerate (e.g. the user saves a momentarily-malformed spec) is
/// printed and *does not* stop watching: [`generate`] returns a [`Report`] rather than panicking,
/// so the loop simply reports it and waits for the next change.
pub(super) fn watch(config: &Config, config_path: Option<&Utf8Path>, format: Format) -> ExitCode {
    // Initial build.
    print_header("initial build");
    let mut last = status_for_report(&regenerate_once(config, format));

    // The watch set, and the canonicalized form used to filter raw watcher events down to it.
    let mut watched = watched_files(config, config_path);
    let mut watched_norm = normalized_set(&watched);

    let (tx, rx) = std::sync::mpsc::channel();
    let mut debouncer = match new_debouncer(DEBOUNCE, tx) {
        Ok(debouncer) => debouncer,
        Err(error) => {
            eprintln!("error: failed to start file watcher: {error}");
            return ExitStatus::Usage.into();
        }
    };

    // Watch the *parent directories* of the watched files (not the files themselves): this survives
    // editors' atomic rename-on-save, and we filter events back down to `watched_norm` ourselves —
    // which also means writing the output file (never in the watch set) can't retrigger a build.
    let mut watched_dirs: HashSet<PathBuf> = HashSet::new();
    register_dirs(debouncer.watcher(), &watched, &mut watched_dirs);

    println!(
        "watching {} file(s) for changes; press Ctrl-C to stop",
        watched.len()
    );

    loop {
        let events = match rx.recv() {
            Ok(Ok(events)) => events,
            // A watch-backend error: report it and keep watching.
            Ok(Err(error)) => {
                eprintln!("warning: file-watch error: {error}");
                continue;
            }
            // The channel closed (the debouncer was dropped) — nothing more will arrive.
            Err(_) => break,
        };

        let relevant = events
            .iter()
            .any(|event| normalize(&event.path).is_some_and(|path| watched_norm.contains(&path)));
        if !relevant {
            continue;
        }

        print_header("change detected");
        last = status_for_report(&regenerate_once(config, format));

        // A change may have added or removed a `$ref` (or added a config/lock): recompute the set
        // and start watching any newly-referenced directories. We never *stop* watching a directory
        // mid-session, so a file that briefly disappears (a malformed save) is still tracked.
        let next = watched_files(config, config_path);
        if next != watched {
            watched = next;
            watched_norm = normalized_set(&watched);
            register_dirs(debouncer.watcher(), &watched, &mut watched_dirs);
        }
    }

    last.into()
}

/// The single per-change action: run the full pipeline once and render its report. A thin reuse of
/// the library facade [`generate`] — identical to the one-shot `spargen generate` path — so a
/// watched regenerate produces byte-identical output for the same on-disk state. Rejections and
/// errors return via the [`Report`] (generation never panics), letting the loop keep watching.
fn regenerate_once(config: &Config, format: Format) -> Report {
    let report = generate(config);
    render_report(&report, format);
    report
}

/// Compute the files to watch for `config`: the spec's on-disk footprint (spec + relative-file
/// `$ref` targets + vendored remote copies, via [`crate::source_files`]), plus the config file in
/// effect (explicit `--config`, else an auto-discovered `spargen.toml` beside the spec) and the
/// lock — each when present. Sorted and de-duplicated, so the result is deterministic.
fn watched_files(config: &Config, config_path: Option<&Utf8Path>) -> Vec<Utf8PathBuf> {
    let mut files = crate::source_files(config);

    let spec_dir = config.spec.parent().unwrap_or_else(|| Utf8Path::new(""));
    match config_path {
        // An explicit `--config` drives the run even when it lives outside the spec dir, so it is
        // always watched.
        Some(path) => files.push(path.to_path_buf()),
        // Otherwise the auto-discovered `spargen.toml` beside the spec, iff present (mirrors the
        // discovery in `config::resolve`).
        None => {
            let discovered = spec_dir.join("spargen.toml");
            if discovered.as_std_path().is_file() {
                files.push(discovered);
            }
        }
    }

    // The lock next to the spec: a re-`lock` or a vendored-file edit should retrigger a build.
    let lock = spec_dir.join("spargen.lock");
    if lock.as_std_path().is_file() {
        files.push(lock);
    }

    files.sort();
    files.dedup();
    files
}

/// Register the parent directory of every watched file with the debouncer (non-recursively),
/// tracking which directories are already watched so re-registration after a recompute is
/// idempotent. A directory that cannot be watched is reported but does not abort the loop.
fn register_dirs(
    watcher: &mut (impl Watcher + ?Sized),
    watched: &[Utf8PathBuf],
    watched_dirs: &mut HashSet<PathBuf>,
) {
    for file in watched {
        let dir = match file.parent() {
            Some(parent) if !parent.as_str().is_empty() => parent.to_path_buf(),
            // A bare filename lives in the current directory.
            _ => Utf8PathBuf::from("."),
        };
        let dir_std = dir.as_std_path().to_path_buf();
        if watched_dirs.insert(dir_std.clone()) {
            if let Err(error) = watcher.watch(&dir_std, RecursiveMode::NonRecursive) {
                eprintln!("warning: cannot watch `{dir}`: {error}");
                watched_dirs.remove(&dir_std);
            }
        }
    }
}

/// Normalize a path for event↔watch-set comparison: canonicalize its *parent* directory (stable
/// even while the file itself is being atomically replaced) and re-attach the file name. Returns
/// `None` for a path with no parent/file name or an uncanonicalizable parent.
fn normalize(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?;
    let parent = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    let canonical_parent = std::fs::canonicalize(parent).ok()?;
    Some(canonical_parent.join(name))
}

/// The canonicalized form of a watch set, for membership tests against raw watcher event paths.
fn normalized_set(watched: &[Utf8PathBuf]) -> HashSet<PathBuf> {
    watched
        .iter()
        .filter_map(|path| normalize(path.as_std_path()))
        .collect()
}

/// Print a concise, wall-clock-stamped line marking the start of a run. Only reached in the live
/// loop (never from a test or from [`regenerate_once`]), so reading the clock here cannot affect
/// the determinism of any generated output or test.
fn print_header(reason: &str) {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    let (hours, mins, secs) = ((secs / 3600) % 24, (secs / 60) % 60, secs % 60);
    println!("[{hours:02}:{mins:02}:{secs:02}] spargen: {reason} — regenerating");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::OutputTarget;

    /// A root spec that pulls in a sibling file via a relative-file `$ref`.
    const ROOT_WITH_REF: &str = r#"
openapi: 3.1.0
info: { title: Watch Test, version: 1.0.0 }
servers: [{ url: https://example.com }]
paths:
  /pets:
    get:
      operationId: listPets
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema: { $ref: "pet.yaml#/Pet" }
"#;

    const PET: &str = r#"
Pet:
  type: object
  properties:
    id: { type: string }
"#;

    /// Turn a tempdir child into a `Utf8PathBuf`.
    fn child(dir: &tempfile::TempDir, name: &str) -> Utf8PathBuf {
        Utf8PathBuf::from_path_buf(dir.path().join(name)).unwrap()
    }

    #[test]
    fn watched_set_includes_refs_config_and_lock() {
        // A multi-file bundle plus an auto-discovered config and a lock: the watch set must contain
        // the root spec, the relative-`$ref` target, the `spargen.toml`, and the `spargen.lock`.
        let dir = tempfile::tempdir().unwrap();
        let spec = child(&dir, "openapi.yaml");
        let pet = child(&dir, "pet.yaml");
        let config_file = child(&dir, "spargen.toml");
        let lock = child(&dir, "spargen.lock");
        std::fs::write(&spec, ROOT_WITH_REF).unwrap();
        std::fs::write(&pet, PET).unwrap();
        std::fs::write(&config_file, "[features]\nuuid = false\n").unwrap();
        std::fs::write(&lock, "version = 1\n").unwrap();

        let config = Config::new(spec.clone(), OutputTarget::Module(child(&dir, "out.rs")));
        let watched = watched_files(&config, None);

        for expected in [&spec, &pet, &config_file, &lock] {
            assert!(
                watched.contains(expected),
                "watch set {watched:?} is missing {expected}"
            );
        }
        // The output file is emphatically NOT watched (writing it must never retrigger a build).
        assert!(
            !watched.iter().any(|path| path.ends_with("out.rs")),
            "the output file must not be watched: {watched:?}"
        );
    }

    #[test]
    fn explicit_config_outside_spec_dir_is_watched() {
        // An explicit `--config` path is watched even when it lives outside the spec directory, and
        // a `spargen.toml` beside the spec is NOT auto-added when `--config` is given.
        let dir = tempfile::tempdir().unwrap();
        let spec = child(&dir, "openapi.yaml");
        std::fs::write(&spec, ROOT_WITH_REF).unwrap();
        std::fs::write(child(&dir, "pet.yaml"), PET).unwrap();

        let cfg_dir = tempfile::tempdir().unwrap();
        let explicit = child(&cfg_dir, "custom.toml");
        std::fs::write(&explicit, "[features]\nuuid = false\n").unwrap();

        let config = Config::new(spec, OutputTarget::Module(child(&dir, "out.rs")));
        let watched = watched_files(&config, Some(explicit.as_path()));
        assert!(
            watched.contains(&explicit),
            "explicit --config must be watched: {watched:?}"
        );
    }

    #[test]
    fn missing_config_and_lock_are_omitted() {
        // With neither a config nor a lock on disk, the watch set is exactly the source files —
        // proving the presence checks are honest (no phantom paths that would never fire).
        let dir = tempfile::tempdir().unwrap();
        let spec = child(&dir, "openapi.yaml");
        let pet = child(&dir, "pet.yaml");
        std::fs::write(&spec, ROOT_WITH_REF).unwrap();
        std::fs::write(&pet, PET).unwrap();

        let config = Config::new(spec.clone(), OutputTarget::Module(child(&dir, "out.rs")));
        let mut watched = watched_files(&config, None);
        watched.sort();
        let mut expected = vec![spec, pet];
        expected.sort();
        assert_eq!(watched, expected, "only the source files should be watched");
    }

    #[test]
    fn regenerate_once_reflects_spec_edits() {
        // The core per-change action, exercised WITHOUT the OS watcher: editing the spec content and
        // re-running `regenerate_once` produces updated output. Proves watch's per-change step is
        // correct on its own.
        fn spec_text(operation_id: &str) -> String {
            format!(
                r#"
openapi: 3.1.0
info: {{ title: Watch Test, version: 1.0.0 }}
servers: [{{ url: https://example.com }}]
paths:
  /pets:
    get:
      operationId: {operation_id}
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema: {{ type: string }}
"#
            )
        }

        let dir = tempfile::tempdir().unwrap();
        let spec = child(&dir, "openapi.yaml");
        let out = child(&dir, "client.rs");
        let config = Config::new(spec.clone(), OutputTarget::Module(out.clone()));

        // First generation.
        std::fs::write(&spec, spec_text("listPets")).unwrap();
        let report = regenerate_once(&config, Format::Human);
        assert_eq!(report.outcome, crate::Outcome::Generated, "{report:?}");
        let first = std::fs::read_to_string(&out).unwrap();
        assert!(first.contains("list_pets"), "first output:\n{first}");

        // Edit the spec and regenerate — the output must track the change.
        std::fs::write(&spec, spec_text("listCats")).unwrap();
        let report = regenerate_once(&config, Format::Human);
        assert_eq!(report.outcome, crate::Outcome::Generated, "{report:?}");
        let second = std::fs::read_to_string(&out).unwrap();
        assert!(second.contains("list_cats"), "second output:\n{second}");
        assert!(
            !second.contains("list_pets"),
            "stale operation must be gone:\n{second}"
        );
    }

    #[test]
    fn regenerate_once_survives_a_malformed_spec() {
        // A momentarily-malformed spec must come back as a rejected Report (never a panic), so the
        // watch loop can print it and keep going.
        let dir = tempfile::tempdir().unwrap();
        let spec = child(&dir, "openapi.yaml");
        std::fs::write(&spec, "this: is: not: openapi").unwrap();
        let config = Config::new(spec, OutputTarget::Module(child(&dir, "client.rs")));
        let report = regenerate_once(&config, Format::Human);
        assert_eq!(report.outcome, crate::Outcome::Rejected, "{report:?}");
    }
}
