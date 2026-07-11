# Validation Plan

Validation is tiered so pull requests stay fast while release jobs can run expensive coverage.
The per-subsystem strategy (which suite pins what, and what to extend when touching a subsystem)
lives in [`AGENTS.md`](../AGENTS.md#testing-strategy-by-subsystem).

PR-required gates (all run in CI):

- `mise run fmt` / `mise run check` / `mise run lint` (clippy, warnings denied).
- `cargo test --workspace --all-features`:
  - `spargen/tests/frontend.rs` — one fixture per diagnostic code; rejections and warnings each
    demonstrably fire, and `check` stays in parity with `generate`.
  - `spargen/tests/determinism.rs` — double generation is byte-identical.
  - `spargen/tests/drift.rs` — the `--check` clean/drifted/missing contract (`W004`).
  - `spargen/tests/e2e.rs` — a generated standalone crate passes `cargo check` and
    `cargo clippy -D warnings`, covering secured operations and the compatibility omit overlay.
  - `support-runtime` unit tests — URL building, auth attachment, status classification, and the
    error taxonomy, polled without an async runtime.
- Fast corpus smoke: GitHub 3.0 rejection (`E001`), Ollama rejection (`E007`, undiscriminated
  unions), and boilerplate check-clean, against the pinned specs in `corpus/manifest.toml`.
- End-to-end example: `examples/petstore` generates from `build.rs` and drives the client over
  real HTTP against a local mock server.

Release/scheduled gates (not yet automated; run before publishing):

- Full corpus check against every pinned case in `corpus/manifest.toml`.
- Generated crate compile/clippy for every corpus case expected to generate.
- Dependency-graph audit proving generated crates depend only on `reqwest`, `serde`,
  `serde_json`, `bytes`, `secrecy`, and optional `uuid`/`time` — no `spargen` runtime dependency.
- Public API diff of generated output fixtures (the semver surface).

Future hardening, in priority order: fuzzing the JSON/YAML source parsers, mutation testing
(`cargo mutants`) over diagnostics/omit/classification/naming logic, and binary-size tracking for
a reference client. Property-based tests are used where invariants matter more than examples;
the current suite covers naming injectivity.
