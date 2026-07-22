#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
work_dir=$(mktemp -d "${TMPDIR:-/tmp}/spargen-github-api.XXXXXX")
trap 'rm -rf "$work_dir"' EXIT

client_dir="$work_dir/github-api-client"
target_dir="$repo_root/target/github-api-client"
spec="$repo_root/corpus/github-api-3-1/api.github.com.json"
config="$repo_root/corpus/github-api-3-1/spargen.toml"

cargo run --quiet --manifest-path "$repo_root/Cargo.toml" -p spargen --features cli -- \
  generate "$spec" --out "$client_dir" --as-crate --config "$config" --format json \
  > "$work_dir/diagnostics.json"

# Clippy type-checks every native emitted item under the strict lint contract used by generated
# clients. The separate wasm check exercises cfg-specific transport/runtime code over the same API.
CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_BUILD_JOBS=1 \
  cargo clippy --manifest-path "$client_dir/Cargo.toml" --target-dir "$target_dir" \
  --lib --all-features -- -D warnings -W clippy::expect-used
CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_BUILD_JOBS=1 \
  cargo check --manifest-path "$client_dir/Cargo.toml" --target-dir "$target_dir" \
  --lib --target wasm32-unknown-unknown --all-features
