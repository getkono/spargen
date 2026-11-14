---
paths:
  - "spargen/src/codegen/**"
  - "spargen/src/emit/**"
  - "spargen/src/ir/**"
  - "spargen/tests/e2e/**"
  - "spargen/tests/e2e.rs"
  - "spargen/tests/snapshots/**"
---

# Code generation

- `spargen/tests/e2e.rs` generates into an application-owned fixture crate under `cargo check` and
  `cargo clippy -D warnings`; extend `spargen/tests/e2e/specs/` when emitting a new construct.
  `BASIC_SPEC` or `OAS32_SPEC` emits every `support::AuthKind` and `support::Framing`.
- `e2e__basic_spec_module.snap` and `e2e__basic_spec_module_no_features.snap` pin the emitted
  source, embedded runtime included. Regenerate with
  `INSTA_UPDATE=always cargo test -p spargen --test e2e basic_spec_module` and review the diff.
- Attributes ride on emitted items, never at crate level, so output stays `include!`-able.
- No wildcard `match` over `TypeKind` that does not name `Reserved` first.
- A codegen change risks the slow gates (`github-api`, `example`): run
  `gh workflow run ci.yml --ref <branch>` before asking for review.
