# spargen

A compile-time-correct Rust client generator for OpenAPI 3.1.x. The authoritative product
spec is [`docs/prd.md`](docs/prd.md) — read it before making non-trivial changes.

## Workspace

- `spargen/` — the one published crate (library + `cli`-gated binary). Internally partitioned
  into subsystems with a declared dependency DAG (PRD §2.3): `diag`, `source`, `ir`, `oas31`,
  `name`, `support`, `codegen`, `emit`, `cli`, and the `lib.rs` facade. Every subsystem
  `mod.rs` declares its allowed dependencies in a `//! layer-deps:` header — keep those honest;
  the future `xtask lint-layers` job enforces them.
- `support-runtime/` — the freestanding runtime embedded into generated output. `publish =
  false`; its only dependencies are the near-universal `reqwest` / `serde` / `serde_json` /
  `bytes` set (PRD §2.1). No spargen crate may ever appear in a consumer's runtime graph.

## Quality

Validate changes:

```bash
mise run check   # cargo check --workspace --all-features
mise run fmt     # cargo fmt --all
mise run lint    # cargo clippy --workspace --all-targets --all-features -- -D warnings
```

- All generated output (and the embedded `support` runtime) carries `#![forbid(unsafe_code)]`.
- Output is **deterministic**: same spargen version + spec + config ⇒ byte-identical output.
- Generated code never silently degrades a typed schema to `serde_json::Value` (PRD FR2).
- Prefer `pub(crate)` over `pub` for anything not part of the `build.rs` facade or an emitted
  API; module privacy plus the layering DAG is how coupling stays controlled.

## Commits

Commits MUST follow [Conventional Commits](https://www.conventionalcommits.org/) (`feat:`,
`fix:`, `chore:`, …) — enforced by `convco` on pre-push and in CI. Merge commits are exempt.

## Releases

Releases are driven by release-plz: it maintains a version-bump pull request, and merging that
PR tags the release and publishes to crates.io. Never bump the version or tag manually. The
semver surface is the public API of generated output (PRD D12).
