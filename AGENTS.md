# spargen

A compile-time-correct Rust client generator for OpenAPI 3.1.x and 3.2.x. [`README.md`](README.md)
and [`docs/book/src/generated-api.md`](docs/book/src/generated-api.md) carry the product contract;
[`docs/support-matrix.md`](docs/support-matrix.md) and [`docs/errors.md`](docs/errors.md) are the
operational surface. `references/3.2.0.md` is the ground truth for any 3.2 conformance question.

Rules for one area live in `.claude/rules/`, each scoped by its `paths:` frontmatter; read the one
for the area you change: `support-runtime.md`, `diagnostics.md` (codes, `oas31`, `frontend/`),
`codegen.md` (`codegen`, `emit`, `ir`, `e2e`), `ci-gates.md` (mise, hooks, workflows, toolchain),
`dependencies.md` (manifests, lockfiles, advisories), `releases.md` and `corpus.md`.

## Workspace

- `spargen/` — the published library and `cli`-gated binary, split into subsystems whose allowed
  dependencies each `//! layer-deps:` header declares (`spargen/tests/layering/` holds them to the
  real `crate::` edges).
- `spargen-macro/` — the `generate_api!` proc-macro over spargen's renderer. It depends on
  `spargen`; `spargen` never depends on it.
- `support-runtime/` — the runtime embedded into generated output (`publish = false`).
- `examples/` — each its own workspace, so a consumer's crate layout is what is tested.
- `corpus/` — pinned real-world descriptions (Git-LFS) with expected outcomes.
- `fuzz/` — cargo-fuzz, nightly, outside the workspace: `cargo +nightly fuzz run frontend`.
- `docs/book/` — the mdBook site, which includes the standalone `docs/*.md`.

## Validate

`mise run check`, `fmt`, `lint` and `test` cover most changes; `mise tasks` lists the rest. Run
`mise run hooks` once. A green pre-push is not a green CI, and a green pull request is not a green
`master`: the slow gates (`msrv`, `powerset`, `corpus-smoke`, `github-api`, `example`,
`bench-build`) skip ordinary pull requests, so a change that risks one (a toolchain or dependency
bump, a codegen change) first runs `gh workflow run ci.yml --ref <branch>`, which tests the branch
head, not the merge, and skips the `package` job's macro dry-run.

## Invariants

- Output is **deterministic**: same spargen version + spec + config ⇒ byte-identical output.
- No spargen crate ever appears in a consumer's runtime graph; `spargen-macro` and everything it
  reaches are host/build-time only.
- Generated code never silently degrades a typed schema to `serde_json::Value`, and every spec
  construct is supported, warned, or rejected — no fourth, silent behavior. The one `Value` a
  typed schema yields is opt-in: under `open_unions`, a discriminated union's `Unknown { tag,
  value }` variant, holding only a payload no listed variant claims.
- Generated output stays consumable via `include!`: no crate-level inner attributes.
- Prefer `pub(crate)` over `pub` outside the `build.rs` facade and emitted APIs.
- A doc comment may say something **does not exist** only while a test fails as soon as that stops
  being true; otherwise state what does exist, in the present tense, or leave the sentence out.
  When such a test fails, rewrite the sentence and retire the test; never widen it. There is
  deliberately no `trybuild` harness: write "cannot be named" as the fact behind it ("boxed behind
  a private type").
- Every bug becomes a fixture (usually in `spargen/tests/frontend/` or a runtime test module)
  *before* its fix. Tests live closest to what they pin: extend the suite of the subsystem you
  touch.

## Commits and releases

Commits follow [Conventional Commits](https://www.conventionalcommits.org/), checked by `convco`
at commit time, on pre-push and in CI; merge commits are exempt. release-plz drives releases:
never bump a version or tag by hand, and never merge its release pull request while `master` or
its own run is red. The semver surface is the public API of generated output. A red `deny`,
`deps-latest` or slow gate that a diff did not cause is the maintainers' to fix, the day it is
seen, in a pull request of its own.
