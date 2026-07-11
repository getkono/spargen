# Validation Plan

Validation is tiered so pull requests stay fast while release/scheduled jobs can run expensive
coverage.

PR-required gates:

- `mise run fmt`: `cargo fmt --all`.
- `mise run check`: `cargo check --workspace --all-features`.
- `mise run lint`: `cargo clippy --workspace --all-targets --all-features -- -D warnings`.
- `cargo test --workspace --all-features`.
- Generated-code E2E: a 3.1 spec generates a standalone crate that passes both `cargo check` and
  `cargo clippy -- -D warnings`.
- Compatibility E2E: `spargen::omit!` removes an unsupported operation, emits `W009`, and still
  generates the supported subset.
- Version gate E2E: OpenAPI 3.0.x rejects with `E001`.
- Fast corpus smoke: GitHub 3.0 rejection (E001), Ollama rejection (E007, undiscriminated unions), and boilerplate check.

Release/scheduled gates:

- Full corpus check against every pinned case in `corpus/manifest.toml`.
- GitHub 3.1 strict rejection plus a reviewed compatibility profile generation run.
- Generated crate compile/clippy for every corpus case expected to generate.
- Dependency graph audit proving generated crates depend on `reqwest`, `serde`, `serde_json`,
  `bytes`, and optional `uuid`/`time`, with no `spargen` runtime dependency.
- Public API diff for generated output fixtures.
- Fuzz parsers for JSON/YAML source loading and `$ref` resolution.
- Mutation testing (`cargo mutants`) focused on diagnostics, omit matching, response
  classification, and naming collision logic.

Property-based tests are used where invariants are more important than examples. The current suite
checks naming injectivity; future additions should target JSON Pointer round-trips, omit
fingerprint stability, and response success/error classification.
