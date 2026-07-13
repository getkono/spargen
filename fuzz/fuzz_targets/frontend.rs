//! Coverage-guided (libFuzzer) fuzz target for the `oas31` frontend.
//!
//! It feeds arbitrary bytes to `spargen::check` — the full frontend: `source` parse → `oas31`
//! parse/validate/audit → `ir` lower → `name` allocate — and asserts, by simply not crashing, that
//! no input can panic, overflow the stack, or abort the generator. `check` is hermetic (no network,
//! no output written), so this is safe to run in a loop. The companion `spargen/tests/fuzz_frontend.rs`
//! proptest harness is the always-on CI guard; this target is the deep, opt-in search.
//!
//! Run it (needs a nightly toolchain and `cargo install cargo-fuzz`):
//!
//! ```text
//! cargo +nightly fuzz run frontend
//! ```
//!
//! The first byte selects the parser exercised (JSON vs YAML) via the spec file extension; the rest
//! is the document body. Bytes need not be valid UTF-8 — the frontend must reject that gracefully.

#![no_main]

use std::io::{Seek, SeekFrom, Write};
use std::sync::OnceLock;

use libfuzzer_sys::fuzz_target;
use tempfile::TempDir;

/// One reusable temp directory for the whole fuzzing session — created once, so the hot loop does no
/// directory setup, only a rewrite of the two spec files.
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
    file.seek(SeekFrom::Start(0)).ok();
    drop(file);

    let spec = camino::Utf8PathBuf::from_path_buf(path).expect("utf8 temp path");
    let out = spargen::OutputTarget::Module(camino::Utf8PathBuf::from("unused.rs"));
    // The property: this returns a `Report` for every input, never panics/aborts.
    let _report = spargen::check(&spargen::Config::new(spec, out));
}

fuzz_target!(|data: &[u8]| {
    // First byte routes to a parser so the corpus reaches both frontends; the remainder is the body.
    let (selector, body) = data.split_first().unwrap_or((&0, &[]));
    let ext = match selector % 3 {
        0 => "json",
        1 => "yaml",
        _ => "txt", // extension-sniff fallback (YAML then JSON)
    };
    run(ext, body);
});
