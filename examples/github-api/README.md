# GitHub API compile fixture

This application-owned crate compiles the pinned, vendored GitHub OpenAPI 3.1 document through
spargen's supported `build.rs` path. CI runs strict native Clippy and a wasm check against the
generated module.
