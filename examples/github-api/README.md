# GitHub API compile fixture

This application-owned crate compiles the pinned, vendored GitHub OpenAPI 3.1 document through
spargen's supported `build.rs` path. `mise run github-api`, from the repository root, runs what CI
runs against the generated module: strict native Clippy and a `wasm32-unknown-unknown` check, both
with `--all-features`.
