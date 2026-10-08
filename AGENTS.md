# spargen

A compile-time-correct Rust client generator for OpenAPI 3.1.x and 3.2.x. The [`README.md`](README.md)
carries the product contract; [`docs/support-matrix.md`](docs/support-matrix.md) and
[`docs/errors.md`](docs/errors.md) are the operational surface — read them before non-trivial
changes.

## Workspace

- `spargen/` — the primary published crate (library + `cli`-gated binary). Internally partitioned
  into subsystems with a declared dependency DAG: `diag`, `source`, `ir`, `oas31`, `name`,
  `support`, `codegen`, `emit`, `compat`, `surface`, `cli`, and the `lib.rs` facade
  (`cache`, `config`, and `runtime_contract` are facade plumbing, not subsystems). Every subsystem
  declares its allowed dependencies in a `//! layer-deps:` header — keep those honest. The header
  is on its `mod.rs`, except for `cli`, which is no library module and carries it on `cli/run.rs`.
- `spargen-macro/` — the second published crate: a thin `proc-macro` crate exposing
  `generate_api!` over spargen's private in-memory renderer. It depends on `spargen` (host-only); `spargen`
  must **never** depend back on it (that would cycle). A proc-macro crate and everything it reaches
  are host/build-time only, so neither crate enters a consumer's runtime graph — the invariant
  below is unchanged. `examples/petstore-macro` is its end-to-end guard.
- `support-runtime/` — the freestanding runtime embedded verbatim into generated output.
  `publish = false`; its unconditional dependencies are exactly `reqwest` / `serde` / `serde_json` /
  `bytes` / `secrecy` / `futures-core` (the stream module and its dependency are emitted only for
  APIs with sequential responses), plus three optional ones behind features for the conditionally
  embedded modules: `quick-xml` (`xml`), `tokio` (`blocking`), and `time` (`time`, the RFC 3339
  date newtypes). No spargen crate may ever appear in a consumer's runtime graph. Each source file
  keeps its `#[cfg(test)]` module last — everything above that marker is embedded into generated
  code and must compile there.
- `examples/` — each its own workspace, so a consumer's crate layout is what is actually tested.
  `petstore/` (build.rs path) and `petstore-macro/` (macro path) are driven over real HTTP by
  `mise run example`; `github-api/` generates the pinned 12.9 MB GitHub 3.1 description and is
  compile-checked natively and for wasm by `mise run github-api`. All three must stay green.
- `corpus/` — pinned real-world descriptions with expected outcomes in `corpus/manifest.toml`
  (`expect = "generate"` / `"reject:E###"`), mirrored in `corpus/README.md`. Files are Git-LFS.
  `corpus/recipes/` holds the hand-written framework-output specs `tests/recipes.rs` drives.
- `fuzz/` — a cargo-fuzz crate (libFuzzer, nightly, **excluded from the workspace**), manual-only:
  `cargo +nightly fuzz run frontend`. Its always-on counterpart is `tests/fuzz_frontend.rs`, a
  fixed-seed proptest no-panic harness.
- `spargen/benches/` — criterion benchmarks over the generation pipeline (`mise run bench`). CI
  records them on tags as an artifact; they are **not** a gate.
- `docs/book/` — the mdBook site (`mise run docs`), which includes the standalone `docs/*.md`
  rather than duplicating them. `references/` carries the vendored OpenAPI specification texts —
  `3.2.0.md` is the ground truth for any 3.2 conformance question.
- `deny.toml` — the supply-chain policy (`mise run deny`).

## Quality

Validate changes:

```bash
mise run check      # cargo check --workspace --all-features
mise run fmt        # cargo fmt --all
mise run fmt-check  # cargo fmt --all --check
mise run lint       # cargo clippy --workspace --all-targets --all-features -- -D warnings
mise run lint-fix   # cargo clippy --workspace --all-targets --all-features --fix --allow-dirty --allow-staged
mise run test       # cargo test --workspace --all-features
mise run bench      # criterion benchmarks over the generation pipeline (not a gate)
mise run bench-build  # cargo bench --no-run --workspace
mise run msrv       # the declared rust-version floor (+1.88.0): workspace + petstore example
mise run package    # cargo publish --dry-run: the published crates stay self-contained
mise run runtime-dependencies  # the ignored minimal-versions proof in e2e.rs
mise run powerset   # cargo hack: every feature combination, not just --all-features
mise run corpus-smoke  # pinned real-world specs
mise run example    # both petstore examples over a local mock server
mise run github-api # the full GitHub client: native strict clippy + wasm32
mise run deny       # supply-chain audit
mise run deny-published  # advisory audit of the Cargo.lock the latest release ships
mise run release-preview  # release-plz update in a scratch clone: the next release's CHANGELOG
mise run docs       # build the mdBook site (fails on broken links/includes)
mise run doc-links  # rustdoc over the workspace, warnings denied, private items included
mise run commit-msg    # Conventional Commits check of one message, read from stdin
mise run commit-range  # Conventional Commits check of origin/master..HEAD
mise run hooks      # install the git hooks hk.pkl declares
```

`hk.pkl` wires these into git hooks, each step delegating to a `mise` task rather than keeping a
second copy of a gate: `fmt` and `lint` fix the working tree on pre-commit; `fmt-check`, `lint`,
`test`, and `commit-range` gate pre-push, and so does `deny` when the outgoing range changes a
`Cargo.lock` or `Cargo.toml` anywhere, `deny.toml`, or `mise.toml` (`hk.pkl` says why that step
alone is globbed); `commit-msg` validates each message as it is written. Run `mise run hooks` once
to install them. The rest — `check`, `bench-build`, `msrv`, `package`, `runtime-dependencies`,
`powerset`, `corpus-smoke`, `example`, `github-api`, `deny-published`, `release-preview`, `docs`,
and `doc-links` (a step inside the `docs` CI job, not a job of its own) — never run in a hook;
they are too slow, so a green pre-push is not a green CI. `msrv` needs
`rustup toolchain install 1.88.0`, and `package` a clean tree.

CI spells the commands out itself rather than calling `mise`, and a `mise` task and its CI job are
**identical**: the same commands with the same flags, in the same order, under the same
environment, and neither stricter nor narrower than the other. Change both sides together.
`spargen/tests/corpus_manifest.rs` enforces this and the pins below; the "repository gates" row of
the testing-strategy table names each of its tests. Every job of every workflow under
`.github/workflows/` is paired with a task, except in `release-plz.yml`, the one listed non-gate
workflow (it publishes); an unclassified workflow fails. Each gate workflow's `on:`,
`concurrency:` and `permissions:`, and every other step of each job (checkout, toolchain, cache,
tool install), are pinned literally, and task keys such as `dir` or a mise config file beside
`mise.toml`, which would change what a task runs without appearing in its command, are rejected.
CI's `test` job runs the same two commands as `mise run test` and `mise run bench-build`; `bench`
pairs with `benchmarks.yml`, and `deny` and `deny-published` with `deny.yml`'s jobs of those
names. The only differences are named exceptions in that test's `PAIRINGS` table, each pinned
literally on both sides:

- `commits` checks the pull request's `base.sha..head.sha` where `commit-range` checks
  `origin/master..HEAD` (only the range is rewritten; the rest must match).
- The `package` job's release-PR-gated `cargo publish --dry-run -p spargen-macro` step is CI-only.
- `benchmarks.yml` adds `set -o pipefail` and `| tee bench-results.txt` to capture the artifact.
- The `deny-published` job's pinned `if:` runs it only on the schedule and on `workflow_dispatch`.
- CI installs `cargo-deny`, `cargo-audit`, `cargo-hack`, `mdbook`, `convco` and `release-plz`
  itself, at the versions mise's `[tools]` pins; `release-plz.yml`'s action is held to the
  `release-plz` pin through its `version:` input. Every pin but `hk`, which CI never runs, is
  installed by each job that runs it.

The Rust toolchain is pinned the same way: `rust-toolchain.toml`'s `channel` is a concrete release
(not `stable`), which selects the toolchain for every local `cargo` call and so for every
`mise run` gate, and every `dtolnay/rust-toolchain@` step in every workflow installs that release.
Two named exceptions sit in `TOOLCHAIN_EXCEPTIONS`: `msrv`, which runs on the workspace
`rust-version` on both sides (`cargo +1.88.0 …` and `dtolnay/rust-toolchain@1.88.0`), and the
`runtime-dependencies` job's `@nightly`, which stays **floating** (its `-Z
direct-minimal-versions` lockfile resolution needs a nightly cargo; nothing else runs on it). Bump
the pin by hand, in a pull request of its own that changes `rust-toolchain.toml`'s `channel`, every
`dtolnay/rust-toolchain@<version>` step, and the matching literals in `corpus_manifest.rs`'s
`PAIRINGS` together, and that passes clippy and fmt on the new release.

CI additionally gates what no local task can: `commits` checks exactly the pull request's range.

## Lockfiles and advisories

Four lockfiles are committed: the workspace `Cargo.lock` and one per example workspace. The
supply-chain audit is **not hermetic** — cargo-deny fetches the RustSec database on every run and
`yanked = "deny"` reads the registry as it is now — so an advisory can turn every open pull request
red with nothing committed. The comments on `[tasks.deny]` and `[tasks.deny-published]` in
`mise.toml` explain each command and the incidents behind it; the rules they implement:

- `deny` audits the root workspace `--locked`, and each example workspace under the same
  `deny.toml` but unlocked, like every gate that compiles them.
- It also runs `cargo audit` over **every entry** of each lockfile, with `--ignore` exactly
  `deny.toml`'s `[advisories] ignore`, because cargo-deny skips entries no feature activates. A
  yank or advisory on such an entry is fixed as "anywhere else" below: nothing compiles it.
- Each run logs the advisory-database revision it judged against, so a past green is read against
  that revision, and an audit that exited 0 but printed an error or warning fails (`AUDIT_GUARD` in
  `corpus_manifest.rs`).
- `deny.yml` runs it daily on `master` and on `workflow_dispatch`. GitHub sends a failed scheduled
  run to whoever last changed the workflow's `cron`, and disables a schedule after 60 days without
  repository activity; re-enable it from the Actions tab.
- The `example` gate's first step asserts that no example lockfile holds a TLS crate, the premise
  `deny.toml` reasons about TLS advisories on: generated output carries its own
  default-features-off `reqwest`.
- `deny-published` audits the `Cargo.lock` the latest stable `spargen` release ships, which
  `cargo install spargen --features cli --locked` installs, on the schedule only. A red
  `deny-published` is fixed the same way as a red `deny` below, and then by a release: merge
  release-plz's pull request once the fix is on `master`, or yank the affected version where no fix
  exists.

A red `deny` for an advisory or yank the diff did not introduce is the **maintainers'** to fix, not
the author's of whichever pull request showed it first. It is fixed the day it is seen, in a
`fix(deps):` pull request of its own. Where a patched release exists, the fix depends on who
resolves the affected crate:

- **In `spargen`'s published graph** — reached through a normal (not dev-) dependency of
  `spargen/Cargo.toml` under any feature, including `remote-fetch` and `cli`: a lockfile bump
  alone is not a fix. It is undone by one `cargo update -p <crate> --precise <old>` with every gate
  green once the database stops naming the advisory, and it never reaches a consumer, because
  Cargo ignores a dependency's lockfile. The pull request adds a direct, default-features-off
  requirement at the patched release to `spargen/Cargo.toml` (optional and enabled by the feature
  that reaches the crate, never named in code), *and* a `[bans] deny` entry in `deny.toml` in the
  `name` + `version = "<patched"` form whose `reason` names the advisory, and bumps each lockfile
  that carries the crate. A later advisory on a crate that already has a floor moves both to the
  new release. `advisory_floors_are_manifest_requirements` in `spargen/tests/corpus_manifest.rs`
  holds every ban to a manifest requirement at exactly its release, so neither half lands alone.
  Where the patched release is outside the range the parent dependency accepts (a new minor on a
  `0.x` line), a floor cannot select it: bump the parent instead, and floor what it still leaves
  open. A floor goes when the parent's own requirement reaches it, or when the parent moves off
  that line (the stranded requirement would otherwise add a second, unused copy); removing the
  rustls floor also removes that test's assertion that it exists.
- **Anywhere else** — a dev-dependency, or a crate only `support-runtime` or an example lockfile
  carries: bump it with `cargo update -p <crate> --precise <patched>` in each lockfile that
  carries it. That guards only this repository's resolves; a consumer's runtime graph is resolved
  from its own manifest, and this rule does not reach it.

Where no patched release exists, the pull request adds an `[advisories] ignore` entry to
`deny.toml` stating why the advisory does not reach this graph and what lifts it, whichever graph
the crate is in. That pull request merges first, and open
pull requests then merge `master` in; none of them carries the fix. Otherwise a lockfile changes
only with the manifest change that needs it, and the workspace one also in release-plz's release
pull request. That pull request does not touch the example lockfiles, so their `spargen` version
stamp goes stale on each release and a local `mise run example` or `mise run deny` rewrites it; that rewrite is not
part of any change and is not committed with one.

Standing invariants:

- Output is **deterministic**: same spargen version + spec + config ⇒ byte-identical output
  (pinned by `spargen/tests/determinism.rs`).
- Generated code never silently degrades a typed schema to `serde_json::Value`, and every
  spec construct is supported, warned, or rejected — no fourth, silent behavior. New warnings
  and rejections get a stable code in `diag`, an entry in `docs/errors.md`, a cell in
  `docs/support-matrix.md`, and a fixture in `spargen/tests/frontend.rs`, in the same commit.
  The last three are enforced by tests keyed on the **code**, so a rejection or warning added
  under an existing code moves none of them: its matrix cell and its `frontend.rs` fixture are on
  the author and the reviewer. (A new emission site of a code in `code.rs`'s `ENUMERATED_CASES`
  must still carry a case marker naming its explain case.)
- Generated output must stay consumable via `include!` — no crate-level inner attributes;
  attributes ride on emitted items.
- Prefer `pub(crate)` over `pub` for anything not part of the `build.rs` facade or an emitted
  API; module privacy plus the layering DAG is how coupling stays controlled. The DAG is enforced
  by `spargen/tests/layering.rs`, which diffs each `//! layer-deps:` header against the module's
  real `crate::` edges.
- A doc comment may say that something **does not exist** — no such operation, no nameable
  type, no such variant, not among these entries — only while a test fails as soon as that
  stops being true. Otherwise say what does exist in the present tense ("`set_credential` is the
  only writer"), or leave the sentence out: the change that makes an absence claim false is a
  feature landing correctly, which rewrites no prose. Behavioural claims such as "never sent" do
  not count; their tests are the ordinary ones. When such a test fails, rewrite the sentence and
  retire the test with it; do not widen the test. Review enforces this rule, not a gate. There is
  deliberately no compile-fail (`trybuild`) harness: it could prove that a consumer's crate cannot
  name a type, not that an operation is missing at every layer, which a test reading the source
  can check. Write a claim that a type cannot be named as the present-tense fact behind it, for
  example "boxed behind a private type".

## Testing strategy (by subsystem)

Tests live closest to what they pin; when you touch a subsystem, extend its suite:

| Subsystem | Suite | What to cover |
| --- | --- | --- |
| `oas31` (+ `source`) | `spargen/tests/frontend.rs` | One minimal inline-spec fixture per diagnostic code (rejections assert `Outcome::Rejected` + code; warnings assert the code fires and generation still succeeds). `check`/`generate` must stay in parity. |
| `ir` (response shapes) | `spargen/src/ir/media.rs` in-module + `spargen/tests/e2e.rs` + `spargen/tests/frontend.rs` + `spargen/tests/diff.rs` | `Responses::success`/`error`: the body count that picks unit, single, or enum, and a single non-streaming success body beside a documented bodyless success status taking the enum; precedence order (exact, then range, then `default` last on the error side); `default` as the success body only when `by_status` declares no 2xx status (empty, or only non-2xx such as `404`); `default` beside a declared success status never entering the success side. On the error side, `StatusSpec::is_success` admitting exactly 2xx, a single error body beside a bodyless error entry taking the error enum with that entry as a unit variant, and `ErrorShape::api_error_body` giving one shared `Body` type exactly when every error body names the same Rust type. In `e2e.rs`, the generated dispatch for those shapes driven over a mock, one operation per shape, including undocumented, range-only and bodyless statuses on each side and a parse failure as `Decode`; in `frontend.rs` and `diff.rs`, a bodyless error status beside one error body generating cleanly and classifying as a major change. The hand-written `dispatch_success`/`dispatch_error` in `support-runtime/src/dispatch.rs` pin only the runtime primitives, not this emitted dispatch. Doc prose in `ir` carries no general pinning obligation, except `Responses::success`'s two `default` claims, which its doctest pins through `spargen::generate`; a change that narrows or restates a claim about the generated shape pins it the same way. |
| `ir` (invariants) | `spargen/src/ir/invariant.rs` in-module + `spargen/src/ir/types.rs` in-module | `check_invariants`, run after every lowering, reporting a violation as `E011` (a frontend bug, not a spec problem): dangling response-header types, a `TypeKind::Reserved` that survived lowering, and an octet-stream request body over anything but a non-nullable `TypeKind::Bytes`. In `types.rs`, a scan of `spargen/src` holding every `match` over `TypeKind` with a catch-all arm to naming `TypeKind::Reserved` above it, so no read site absorbs a reservation through a wildcard. |
| `codegen` / `emit` | `spargen/tests/e2e.rs` | Generate a module into an application-owned fixture crate and require `cargo check` + `cargo clippy -D warnings` on it; extend the inline spec when emitting new constructs so they are compile-verified. |
| `codegen` (determinism) | `spargen/tests/determinism.rs` | Byte-identical double generation. |
| build cache | `spargen/src/cache.rs` | Complete input fingerprints plus missing, stale, and manually edited output invalidation. |
| `diag` | `spargen/src/diag/code.rs` tests | Code string round-trips; every code has title + explain text. |
| `name` | in-module proptests | Determinism, injectivity in scope, valid identifiers, keyword escaping. |
| `compat` | in-module + `carve.rs` + `e2e.rs` | Omit rules match/apply, fingerprint stability (same profile repeats, different profiles differ, order-sensitive), `W009`/`E019` in-module, `E020` in `carve.rs`. |
| `support-runtime` | in-file `#[cfg(test)]` mods | URL building, auth attachment (all schemes + alternatives + failure modes), status classification, error taxonomy semantics. No async runtime: poll-once with `Waker::noop`. |
| whole tool | `examples/petstore` + `examples/petstore-macro` (`mise run example`) | The generated client driven over real HTTP against a local mock server (params, bodies, auth, typed errors, undocumented statuses), via both the `build.rs` and macro paths; the macro run also asserts spargen stays out of the runtime graph (`cargo tree -e no-proc-macro`). |
| corpus | `spargen/tests/corpus_manifest.rs` / `mise run corpus-smoke` | `corpus/manifest.toml` is the single source of expectations (`expect = "generate"` / `"reject:E###"`); update them only with a reviewed reason. The suite checks the `expect` grammar (`every_declared_expectation_is_a_shape_the_suite_understands`), drives every case (`every_case_meets_its_declared_expectation`), verifies each file is fetched content matching its pinned `sha256` (`every_pinned_spec_is_present_and_is_not_an_unfetched_lfs_pointer`, `every_pinned_spec_matches_the_hash_the_manifest_declares`), and holds the smoke task and the CI job (`the_corpus_smoke_gate_covers_every_manifest_case`), `snapshot.rs` (`the_snapshot_suite_covers_every_manifest_case`), and `corpus/README.md` (`the_corpus_readme_mirrors_the_manifest`) to the manifest — adding a case means adding it everywhere. |
| repository gates | `spargen/tests/corpus_manifest.rs` | Assertions over this repository's own gate configuration (`mise.toml`, `.github/workflows/`, `rust-toolchain.toml`, `deny.toml`, and this file); a new one goes here. Task/CI pairing (`every_mise_task_runs_exactly_what_its_ci_job_runs`); tool and toolchain pins (`ci_installs_exactly_the_tool_versions_mise_pins`, `ci_installs_the_rust_toolchain_this_file_pins`, `the_msrv_gate_runs_on_the_declared_rust_version`); the supply-chain audit's scope, every flag held to an allow-list (`the_deny_gate_states_the_feature_scope_it_audits`, `the_lockfile_audit_reads_every_committed_lockfile`, `the_published_lockfile_audit_covers_every_shipped_binary`, `advisory_floors_are_manifest_requirements`, `the_audit_flag_allow_lists_admit_only_what_they_name`); no fixed `/tmp/` path (`the_corpus_smoke_gate_writes_only_inside_the_checkout`); no LFS smudge in the release preview (`the_release_preview_never_smudges_lfs_content`); and this file's Quality list and table (`the_quality_list_quotes_its_tasks_verbatim`, `the_testing_strategy_table_names_every_test_in_its_suite`, `a_cited_test_counts_whatever_its_name_is_shaped_like`). This row and the corpus row together name every test in the file, and only tests it defines. |
| `compat` (carve) | `spargen/tests/carve.rs` | Omit-profile globbing and auto-carve: rules match what they say, carve reaches a fixpoint, and it stays deterministic. |
| `surface` | `spargen/tests/diff.rs` + in-module | `spargen diff` semver classification per change kind, and stability of the same pair twice. Every `ChangeKind` needs a fixture in `diff.rs`; the in-module tests enforce that, and pin the impact policy and the kebab-case codes (`ChangeKind` is `#[non_exhaustive]`, so only an in-crate test can notice a new variant). |
| `config` / CLI | `spargen/tests/config.rs`, `spargen/tests/cli.rs` | Config discovery and precedence, `spargen deps` output, the Cargo-integration policy, and the subcommand set (`generate` is deliberately absent). |
| lowering invariants | `spargen/tests/lowering_props.rs` | Proptests over union/`allOf` lowering: category disjointness, closed-object disjointness, exact `allOf` merge. |
| robustness | `spargen/tests/fuzz_frontend.rs` | Fixed-seed proptest no-panic harness over `check` (arbitrary bytes, UTF-8, keyword-biased and valid-skeleton documents, deep `$ref` chains), through both parsers. `fuzz/` is the nightly libFuzzer counterpart, run by hand. |
| snapshots | `spargen/tests/snapshot.rs` | One per corpus case (enforced by `corpus_manifest.rs`): the outcome plus a sorted diagnostic histogram, and an API surface for the small generating cases. Deliberately **not** the full emitted source of a corpus case. That is pinned in `spargen/tests/e2e.rs` instead, for `BASIC_SPEC` only: `e2e__basic_spec_module.snap` (default options) and `e2e__basic_spec_module_no_features.snap` (`uuid` and `time` off) hold the whole module, embedded runtime included, with only the cache digests, spargen version, and spec path normalized — so a change to signatures, derives, serde attributes, or the runtime is a diff there. Regenerate with `INSTA_UPDATE=always cargo test -p spargen --test e2e basic_spec_module` and review the diff. |
| generated runtime surface | `spargen/tests/reexport_lists.rs` | Reads the emitted module: the embedded `support` module's `pub use` list equals `support-runtime/src/lib.rs`'s, module by module; every root re-export names something `support` re-exports; no operation error type shadows a root re-export, for operation IDs derived to collide with each; and the root `pub use` surface is the golden file `snapshots/reexport_lists__root_surface.snap`, so a change to the generated public runtime surface is a reviewable diff. |
| framework round-trip | `spargen/tests/recipes.rs` | The OpenAPI documents utoipa / aide / poem-openapi actually emit. |
| `emit` | in-module | The provenance header's format and version stamp, that it precedes the module verbatim and is comments-only (generated output is `include!`d, so an inner attribute here breaks every consumer), the one-file rule, and `EmitError`'s display/source chain. |
| `support` + layering | `spargen/tests/layering.rs` | Each subsystem's `//! layer-deps:` header matches the `crate::` edges it actually takes and the DAG table in `lib.rs`; each runtime source carries `#[cfg(test)]` at most once with nothing after it (the embed splits on that literal); generated output carries no test module; no comment above the marker names a test-only item or "the test module" unless its comment block discloses that the test module is stripped when the file is embedded; `runtime_files()` and the `src/support/runtime/` symlinks equal the `support-runtime/src` file set; every file an `include_str!`/`include_bytes!` under `spargen/src` names ships in the package, test fixtures included; every test that spawns the `spargen` binary compiles only under `cli`; and `REQUEST_VARIANTS` / `ERROR_VARIANTS` in `support-runtime/src/error.rs` equal the variant counts of `RequestError` and `Error<E>`. |
| docs ↔ code | `spargen/src/diag/code.rs` tests | `docs/errors.md` lists exactly `Code::all()` with matching titles; every code a support document cites is real, and every declared code appears in `docs/support-matrix.md` in the column matching its severity (an absent document fails inside the repository; only a packaged `.crate` skips). The matrix's prose is human-reviewed, with one exception: every clause of `E023`'s row that mentions workspace inheritance must be a verbatim excerpt of `E023`'s explain body, which is pinned byte for byte, and the row defers to `spargen explain E023` for the rest. `Code::all()` is checked against the enum, and every code must be asserted by `frontend.rs` or by the suite named in that test's `OWNED_ELSEWHERE` table. |
| docs ↔ repository | `spargen/tests/docs.rs` | Every `spargen`/`spargen-macro` install line in a Markdown file (changelogs excepted) requires the workspace `Cargo.toml`'s current minor, so a minor bump fails until they follow it. The ignored `every_link_in_the_built_book_resolves` is the link check `mise run docs` and the `docs` CI job run after `mdbook build`, which checks no link and exits 0 on an unreadable include: it fails on an `{{#include}}` left unresolved, and on a relative link or anchor that does not resolve inside the built site. A standalone `docs/*.md` page is included into the book, so it links outside `docs/` by repository URL, not by a relative path. |
| `source` (remote fetch) | `spargen/tests/vendor_remote.rs` | `spargen lock`'s real reqwest fetcher, through `spargen::vendor` and (under `cli`) the binary, against a local plain-HTTP server: requests, an error status or refused connection as `E025`, redirects and the URL a redirected document's relative `$ref`s and `$self` resolve against, and offline generation from what was pinned. Gated on `remote-fetch`. |
| TLS strictness | `spargen/tests/tls_strictness.rs` | The same fetcher against a deliberately non-conformant TLS 1.3 peer, which it must refuse as `E025` (the RUSTSEC-2026-0285 record the `rustls` floor fixed), plus a control scenario proving that peer's message well-formed. The one rustls strictness change spargen pins; TLS conformance is rustls's. Gated on `remote-fetch`. |

Bug-fix discipline: every bug becomes a fixture (usually in `frontend.rs` or the runtime test
mods) *before* its fix, so regressions cannot reappear silently.

## Commits

Commits MUST follow [Conventional Commits](https://www.conventionalcommits.org/) (`feat:`,
`fix:`, `chore:`, …) — enforced by `convco` at commit time, on pre-push, and in CI. Merge
commits are exempt.

## Releases

Releases are driven by release-plz: it maintains a version-bump pull request, and merging that
PR tags the release and publishes to crates.io. Never bump the version or tag manually. The
semver surface is the public API of generated output.

Publishing runs strictly in CI via crates.io Trusted Publishing (OIDC) — no
`CARGO_REGISTRY_TOKEN` secret; `release-plz.yml` mints a short-lived token with
`rust-lang/crates-io-auth-action`. Bootstrap was one-time: `0.1.0` was published manually to
create the crate, then a Trusted Publisher (`getkono/spargen`, workflow `release-plz.yml`) was
configured in the crate settings. The published crate must stay self-contained — the runtime
sources are reached through `spargen/src/support/runtime/` symlinks so they ship inside the
`.crate`; the CI `package` job (`cargo publish --dry-run`) enforces this.

`spargen-macro` is a second published crate (it depends on `spargen`, so release-plz publishes
`spargen` first). Its one-time bootstrap is complete: `0.2.0` was published manually, and its
crate settings trust `getkono/spargen`'s `release-plz.yml` workflow. CI fully verifies the macro
artifact on ordinary changes; on release PRs, release-plz performs that verification after the new
`spargen` dependency reaches the registry. Subsequent releases publish via OIDC.
