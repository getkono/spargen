//! Coverage-guided (libFuzzer) fuzz target for the `oas31` frontend.
//!
//! It feeds arbitrary bytes to `spargen::check` — the full frontend: `source` parse → `oas31`
//! parse/validate/audit → `ir` lower → `name` allocate — and asserts that no input can panic,
//! overflow the stack, or abort the generator. It also holds every report to the oracles the
//! `spargen/tests/` suites share (`oracles::run_violations`): no `E011` reporting an IR invariant or
//! codegen failure (`oracles::internal_failures`), and every diagnostic naming a real location
//! (`oracles::location_violations`). A run that breaks one panics, so libFuzzer records it as a
//! crash. `check` is hermetic (no network, no output written), so this is safe to run in a loop.
//! The companion `spargen/tests/fuzz_frontend.rs` proptest harness is the always-on CI guard; this
//! target is the deep, opt-in search.
//!
//! Run it (needs a nightly toolchain and `cargo install cargo-fuzz`):
//!
//! ```text
//! cargo +nightly fuzz run frontend -- -dict=fuzz/dict/openapi.dict
//! ```
//!
//! `fuzz/README.md` says how to seed it from the pinned descriptions under `corpus/`.
//!
//! The first byte selects one of three parser routes via the spec file extension — `.json` (JSON),
//! `.yaml` (YAML), or `.txt` (the extension-sniff fallback: YAML, then JSON); the rest is the
//! document body. Bytes need not be valid UTF-8 — the frontend must reject that gracefully.

#![no_main]

use std::io::Write;
use std::sync::OnceLock;

use libfuzzer_sys::fuzz_target;
use tempfile::TempDir;

// The oracles the `spargen/tests/` suites hold every run to, compiled from their one copy.
#[path = "../../spargen/tests/oracles/mod.rs"]
mod oracles;

/// One reusable temp directory for the whole fuzzing session — created once, so the hot loop does no
/// directory setup, only a rewrite of the three spec files.
fn temp_dir() -> &'static TempDir {
    static DIR: OnceLock<TempDir> = OnceLock::new();
    DIR.get_or_init(|| TempDir::new().expect("create fuzz temp dir"))
}

fn run(ext: &str, body: &[u8]) {
    let path = temp_dir().path().join(format!("spec.{ext}"));
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&path)
        .expect("open spec file");
    file.write_all(body).expect("write spec body");
    drop(file);

    let spec = camino::Utf8PathBuf::from_path_buf(path).expect("utf8 temp path");
    // The property: this returns a `Report` for every input, never panics/aborts, and the report
    // breaks none of the shared oracles. `check` writes no module, so there is none to hold.
    let report = spargen::check(&spargen::Spec::new(spec));
    let violations = oracles::run_violations(&report, body, None);
    assert!(
        violations.is_empty(),
        "the report broke an oracle every run is held to: {violations:#?}"
    );
}

fuzz_target!(|data: &[u8]| {
    // First byte routes to one of three parser routes; the remainder is the body.
    let (selector, body) = data.split_first().unwrap_or((&0, &[]));
    let ext = match selector % 3 {
        0 => "json",
        1 => "yaml",
        _ => "txt", // extension-sniff fallback (YAML then JSON)
    };
    run(ext, body);
});
