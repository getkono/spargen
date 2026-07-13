//! End-to-end generation-pipeline benchmarks.
//!
//! Measures spargen's wall-clock cost on representative inputs:
//!
//! - `check/*`   — the frontend only (`source` → `oas31` → `ir` → `name`), no codegen or I/O.
//! - `generate/*` — the full pipeline through `codegen` + `emit`, writing to a scratch tempdir.
//!
//! Inputs: a tiny inline spec (fixed cost floor), the 3.1 petstore example, and the real-world
//! Ollama spec (a mid-sized generating corpus case). Run with `cargo bench`; see
//! `docs/benchmarks.md` for methodology and the external-tool comparison harness.

use std::hint::black_box;

use camino::Utf8PathBuf;
use criterion::{criterion_group, criterion_main, Criterion};
use spargen::{Config, Outcome, OutputTarget};

/// A minimal but non-trivial 3.1 spec: one operation, one parameter, one model. Serves as the
/// fixed-cost floor for the pipeline (thread spawn, meta-schema validation, lowering, allocation).
const TINY_SPEC: &str = r##"
openapi: 3.1.0
info: { title: Tiny, version: 1.0.0 }
paths:
  /widgets/{id}:
    get:
      operationId: getWidget
      parameters:
        - { name: id, in: path, required: true, schema: { type: string } }
      responses:
        "200":
          description: ok
          content:
            application/json:
              schema: { $ref: "#/components/schemas/Widget" }
components:
  schemas:
    Widget:
      type: object
      required: [id, name]
      properties:
        id: { type: string }
        name: { type: string }
        tags: { type: array, items: { type: string } }
"##;

/// Absolute path to a repo file, resolved from this crate's manifest dir so benches run from any
/// CWD (`cargo bench` sets it to the package root).
fn repo_path(relative: &str) -> Utf8PathBuf {
    let manifest = Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .expect("crate dir has a workspace parent")
        .join(relative)
}

/// Write `contents` to a fresh tempdir and return (the guard, the spec path). The guard must be
/// held for the file to stay live.
fn spec_in_tempdir(contents: &str) -> (tempfile::TempDir, Utf8PathBuf) {
    let dir = tempfile::tempdir().expect("create tempdir");
    let spec = dir.path().join("openapi.yaml");
    std::fs::write(&spec, contents).expect("write spec");
    let spec = Utf8PathBuf::from_path_buf(spec).expect("utf8 spec path");
    (dir, spec)
}

/// Benchmark the frontend only (`spargen::check`) — parse, validate, lower, name-allocate. No
/// codegen, no filesystem writes; this is the shared cost every `generate` also pays.
fn bench_check(c: &mut Criterion) {
    let (_tiny_dir, tiny_spec) = spec_in_tempdir(TINY_SPEC);
    let cases: [(&str, Utf8PathBuf); 3] = [
        ("tiny", tiny_spec),
        ("petstore", repo_path("examples/petstore/petstore.yaml")),
        ("ollama", repo_path("corpus/ollama/openapi.yaml")),
    ];

    let mut group = c.benchmark_group("check");
    for (name, spec) in &cases {
        // A dummy output target — `check` never writes, so the path is irrelevant.
        let config = Config::new(
            spec.clone(),
            OutputTarget::Module(Utf8PathBuf::from("unused.rs")),
        );
        group.bench_function(*name, |b| {
            b.iter(|| {
                let report = spargen::check(black_box(&config));
                assert_ne!(report.outcome, Outcome::Rejected, "{name} must lower");
                black_box(report);
            });
        });
    }
    group.finish();
}

/// Benchmark the full pipeline (`spargen::generate`) through codegen and emit, writing to a scratch
/// tempdir. Reuses one output dir per case (emit overwrites deterministically), so the measured
/// cost is generation, not directory churn.
fn bench_generate(c: &mut Criterion) {
    let (_tiny_dir, tiny_spec) = spec_in_tempdir(TINY_SPEC);
    let cases: [(&str, Utf8PathBuf); 3] = [
        ("tiny", tiny_spec),
        ("petstore", repo_path("examples/petstore/petstore.yaml")),
        ("ollama", repo_path("corpus/ollama/openapi.yaml")),
    ];

    let mut group = c.benchmark_group("generate");
    for (name, spec) in &cases {
        let out_dir = tempfile::tempdir().expect("create out tempdir");
        let out_path =
            Utf8PathBuf::from_path_buf(out_dir.path().join("api.rs")).expect("utf8 out path");
        let config = Config::new(spec.clone(), OutputTarget::Module(out_path));
        group.bench_function(*name, |b| {
            b.iter(|| {
                let report = spargen::generate(black_box(&config));
                assert_eq!(report.outcome, Outcome::Generated, "{name} must generate");
                black_box(report);
            });
        });
        // `bench_function` measures synchronously, so `out_dir` has served its purpose; hold the
        // binding to the loop-iteration end so the scratch dir outlives the measurement above.
        let _ = &out_dir;
    }
    group.finish();
}

criterion_group!(benches, bench_check, bench_generate);
criterion_main!(benches);
