# spargen

A compile-time-correct Rust client generator for OpenAPI 3.1.x and 3.2.x. [`README.md`](README.md)
carries the product contract; [`docs/support-matrix.md`](docs/support-matrix.md) and
[`docs/errors.md`](docs/errors.md) are the operational surface — read them before non-trivial
changes.

## Workspace

- `spargen/` — the primary published crate (library + `cli`-gated binary), partitioned into
  subsystems with a declared dependency DAG: `diag`, `source`, `ir`, `oas31`, `name`, `support`,
  `codegen`, `emit`, `compat`, `surface`, `cli`, and the `lib.rs` facade. `report`, `pipeline`,
  `diff_api`, `output`, `cache`, `config` and `runtime_contract` are facade plumbing; the
  `#[cfg(test)]` `test_support` holds the helpers in-crate tests share. Each subsystem declares its
  allowed dependencies in a `//! layer-deps:` header on its `mod.rs` (`cli`, no library module,
  carries it on `cli/run.rs`) — keep those honest.
- `spargen-macro/` — the second published crate: a thin `proc-macro` crate exposing
  `generate_api!` over spargen's private in-memory renderer. It depends on `spargen` (host-only);
  `spargen` must **never** depend back on it (that would cycle). `examples/petstore-macro` is its
  end-to-end guard.
- `support-runtime/` — the freestanding runtime embedded, trimmed and path-rewritten, into
  generated output. `publish = false`; its unconditional dependencies are exactly `reqwest` /
  `http` / `http-body` / `serde` / `serde_json` / `bytes` / `secrecy` (`http` and `http-body`
  carry the transport seam's request, response and body), plus optional ones for the
  conditionally embedded modules: `futures-core` and reqwest's `stream` feature (`stream`, emitted only for APIs with
  sequential responses), `quick-xml` (`xml`), `tokio` (`blocking`), and `time` (`time`, the RFC
  3339 date newtypes). Each source file keeps its `#[cfg(test)]` module last — inline, or `mod
  tests;` reading `<stem>/tests.rs` — and everything above that marker is embedded into generated
  code and must compile there.
- `examples/` — each its own workspace, so a consumer's crate layout is what is tested:
  `petstore/` (build.rs path) and `petstore-macro/` (macro path) over real HTTP (`mise run
  example`), and `github-api/`, the pinned 12.9 MB GitHub 3.1 description compile-checked natively
  and for wasm (`mise run github-api`). All three must stay green.
- `corpus/` — pinned real-world descriptions (Git-LFS) with expected outcomes in
  `corpus/manifest.toml` (`expect = "generate"` / `"reject:E###"`), mirrored in
  `corpus/README.md`. `corpus/recipes/` holds the framework-output specs `tests/recipes.rs` drives.
- `fuzz/` — a cargo-fuzz crate (nightly, **excluded from the workspace**), run by hand:
  `cargo +nightly fuzz run frontend`.
- `spargen/benches/` — criterion benchmarks (`mise run bench`), recorded on demand and once per
  `spargen` release; **not** a gate.
- `docs/book/` — the mdBook site (`mise run docs`), which includes the standalone `docs/*.md`.
  `references/` vendors the OpenAPI specification texts; `3.2.0.md` is the ground truth for any
  3.2 conformance question.
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
mise run bench-build  # cargo test --benches --workspace
mise run msrv       # the declared rust-version floor (+1.88.0): workspace + petstore example
mise run package    # cargo publish --dry-run: the published crates stay self-contained
mise run runtime-dependencies  # the ignored minimal-versions proof in e2e.rs
mise run powerset   # cargo hack: every feature combination, not just --all-features
mise run corpus-smoke  # pinned real-world specs
mise run example    # both petstore examples over a local mock server
mise run github-api # the full GitHub client: native strict clippy + wasm32
mise run mutants    # cargo-mutants over support-runtime, ir/media.rs, ir/responses.rs, name/, compat/, diag/, source/{lock,sha256}.rs, oas31/lower/{shape,defaults}.rs, emit/, codegen/docs.rs and surface/mod.rs (weekly in CI)
mise run deny       # supply-chain audit
mise run deny-published  # advisory audit of the Cargo.lock the latest release ships
mise run release-preview  # release-plz update in a scratch clone: the next release's CHANGELOG
mise run docs       # build the mdBook site (fails on broken links/includes)
mise run doc-links  # rustdoc over the workspace, warnings denied, private items included
mise run commit-msg    # Conventional Commits check of one message, read from stdin
mise run commit-range  # Conventional Commits check of origin/master..HEAD
mise run hooks      # install the git hooks hk.pkl declares
```

`hk.pkl` wires these into git hooks, each step a `mise run` of a task: `fmt` and `lint` fix the
working tree on pre-commit; `fmt-check`, `lint`, `test` and `commit-range` gate pre-push, as does
`deny` when the outgoing range changes a `Cargo.lock` or `Cargo.toml`, `deny.toml`, or `mise.toml`
(`hk.pkl` says why that step alone is globbed); `commit-msg` checks each message as it is written.
Run `mise run hooks` once. Every other task is too slow for a hook, so a green pre-push is not a
green CI; `mutants` runs weekly and on demand in `mutants.yml`, never on a pull request, and
`doc-links` is a step of the `docs` CI job. `msrv` needs `rustup toolchain install 1.88.0`, and
`package` a clean tree.

Nor is a green pull request a green `master`. The slow gates — `msrv`, `powerset`, `corpus-smoke`,
`github-api`, `example` and `bench-build` — carry one pinned `if:` (`SLOW_GATE_IF` in
`repo_gates/pairing.rs`): they run on every push to `master` (each in a concurrency group of its
own commit), on `workflow_dispatch`, and on release-plz's release pull request once a maintainer
approves its runs; every other pull request skips them. No status check is required, so do not
merge the release pull request while `master` or its own run is red. A change that risks a slow
gate (a toolchain or dependency bump, a codegen change) first runs the full suite with `gh
workflow run ci.yml --ref <branch>`, which tests the branch head, not the merge, and skips the
`package` job's macro dry-run. A slow gate red on `master` is the maintainers' to fix, the day it
is seen, in a pull request of its own.

CI spells the commands out rather than calling `mise`, and a task and its CI job are
**identical**: the same commands and flags, in the same order, under the same environment. Change
both sides together. `spargen/tests/repo_gates/` enforces this and every pin below: each job of
every workflow is paired with a task, except in `release-plz.yml`, the one non-gate workflow; each
gate workflow's `on:`, `concurrency:`, `permissions:` and other steps are pinned literally; a task
key that changes what runs without appearing in its command is rejected. The only differences are
the named exceptions in its `PAIRINGS` table, which states each one's reason: `commits` checking
the pull request's range, the release-PR-gated macro dry-run, the benchmark artifact capture,
`deny-published`'s schedule, `SLOW_GATE_IF`, and CI installing each tool at the version mise's
`[tools]` pins.

`rust-toolchain.toml`'s `channel` is a concrete release, which selects the toolchain for every local
`cargo` call, and every `dtolnay/rust-toolchain@` step installs that release. `TOOLCHAIN_EXCEPTIONS`
names the two others: `msrv` on the workspace `rust-version` (`+1.88.0` on both sides), and the
`runtime-dependencies` job's **floating** `@nightly`, which `-Z direct-minimal-versions` needs.
Bump the pin in a pull request of its own that changes the `channel`, every
`dtolnay/rust-toolchain@<version>` step and the matching `PAIRINGS` literals together, and passes
clippy and fmt on the new release.

## Lockfiles and advisories

Four lockfiles are committed: the workspace `Cargo.lock` and one per example workspace. The
supply-chain audit is **not hermetic** — cargo-deny fetches the RustSec database and reads the
registry's yank state as they are now — so an advisory can turn every open pull request red with
nothing committed. The comments on `[tasks.deny]` and `[tasks.deny-published]` in `mise.toml`
explain each command, its scope and the incidents behind it, and `repo_gates/supply_chain.rs`
pins them. `deny` runs on every pull request and every push to `master` (its license, bans and
sources checks fail on what a diff adds), and `deny.yml` reruns it daily on `master` and on
`workflow_dispatch`; GitHub disables that schedule after 60 days without repository activity, so
re-enable it from the Actions tab. `deny-published` audits, on the schedule only, the `Cargo.lock`
the latest `spargen` release ships: fix it as a red `deny`, then release (merge release-plz's pull
request once the fix is on `master`) or yank the affected version where no fix exists.

A red `deny` for an advisory or yank the diff did not introduce is the **maintainers'** to fix, not
the author's of whichever pull request showed it first: the day it is seen, in a `fix(deps):` pull
request of its own, which merges first; open pull requests then merge `master` in, and none of them
carries the fix. Where a patched release exists, the fix depends on who resolves the crate:

- **In `spargen`'s published graph** — reached through a normal (not dev-) dependency of
  `spargen/Cargo.toml` under any feature, `remote-fetch` and `cli` included: a lockfile bump alone
  is no fix, because Cargo ignores a dependency's lockfile. Add a direct, default-features-off
  requirement at the patched release to `spargen/Cargo.toml` (optional and enabled by the feature
  that reaches the crate, never named in code) *and* a `[bans] deny` entry in `deny.toml` in the
  `name` + `version = "<patched"` form whose `reason` names the advisory, and bump each lockfile
  that carries the crate; `advisory_floors_are_manifest_requirements` holds the two halves to each
  other. A later advisory moves both. Where the patched release is outside the range the parent
  accepts (a new minor on a `0.x` line), bump the parent and floor what it still leaves open. A
  floor goes when the parent's own requirement reaches it or the parent moves off that line;
  removing the rustls floor also removes that test's assertion that it exists.
- **Anywhere else** — a dev-dependency, a crate only `support-runtime` or an example lockfile
  carries, or a lockfile entry no feature activates: `cargo update -p <crate> --precise <patched>`
  in each lockfile that carries it. That guards only this repository's resolves, never a
  consumer's.

Where no patched release exists, add an `[advisories] ignore` entry to `deny.toml` stating why the
advisory does not reach this graph and what lifts it. Otherwise a lockfile changes only with the
manifest change that needs it, and the workspace one also in release-plz's release pull request,
which leaves the example lockfiles' `spargen` stamp stale: the rewrite a local `mise run example` or
`mise run deny` makes is never committed with a change.

## Invariants

- Output is **deterministic**: same spargen version + spec + config ⇒ byte-identical output
  (`spargen/tests/determinism.rs`).
- No spargen crate may ever appear in a consumer's runtime graph; `spargen-macro` and everything
  it reaches are host/build-time only.
- Generated code never silently degrades a typed schema to `serde_json::Value`, and every spec
  construct is supported, warned, or rejected — no fourth, silent behavior. A new warning or
  rejection code gets a stable code in `diag`, an entry in `docs/errors.md`, a cell in
  `docs/support-matrix.md`, and a fixture in `spargen/tests/frontend/`, in the same commit; tests
  keyed on the code enforce the last three. A case added under an existing code moves none of
  them: its case in that code's `explain` body (the one case list; the matrix defers to `spargen
  explain`) and its fixture are on the author and the reviewer — except for a code in
  `ENUMERATED_CASES` (`diag/code/tests.rs`), held per case: each emission site carries a marker
  naming its explain case, and each case a fixture carrying the same `// E### case: <tag>` marker.
- Generated output stays consumable via `include!`: no crate-level inner attributes; attributes
  ride on emitted items.
- Prefer `pub(crate)` over `pub` outside the `build.rs` facade and emitted APIs; module privacy and
  the layering DAG (`spargen/tests/layering/` diffs each `//! layer-deps:` header against its real
  `crate::` edges) keep coupling controlled.
- A doc comment may say something **does not exist** (no such operation, nameable type, variant,
  or entry) only while a test fails as soon as that stops being true; otherwise state what does
  exist in the present tense ("`set_credential` is the only writer") or leave the sentence out.
  Behavioural claims ("never sent") are tested the ordinary way. When such a test fails, rewrite
  the sentence and retire the test; never widen it. Review enforces this. There is deliberately no
  `trybuild` harness, since only a test reading the source proves an operation missing at every
  layer; write "cannot be named" as the fact behind it ("boxed behind a private type").
- Every bug becomes a fixture (usually in `frontend/` or the runtime test modules) *before* its
  fix.

## Testing strategy (by subsystem)

Tests live closest to what they pin; when you touch a subsystem, extend its suite:

| Subsystem | Suite | What to cover |
| --- | --- | --- |
| `oas31` + `source` (frontend) | `spargen/tests/frontend/` + `spargen/src/oas31/lower/{shape,defaults}.rs` and `spargen/src/oas31/resolve.rs` in-module | One module per construct family beside the shared harness in `main.rs` (parser output in `source_parse`). One minimal inline-spec fixture per diagnostic code (a rejection asserts `Outcome::Rejected` + code; a warning that it fires and generation succeeds), and one per enumerated case, marked `// E### case: <tag>` and asserting its message (`every_enumerated_case_has_a_frontend_fixture`). Every run is held to the oracles in `spargen/tests/oracles/`, and `generate` to `check`'s diagnostics. In-module: the shape-keyword, `$ref`-sibling, scalar, object-like and union gates, enum defaults, property and parameter defaults lowered end to end, escaped component identities. |
| `ir` | `spargen/src/ir/responses.rs` (tests in `responses/tests.rs`) and `ir/{mod,media,invariant,types,graph}.rs` in-module + `spargen/tests/e2e.rs`, `frontend/` and `diff/` | Response shapes: the body count picking unit, single or enum (a bodyless documented status beside one body taking the enum, except a single streaming success body, which stays plain); precedence (exact, range, then `default`); `default` as the success body only when no 2xx is declared; `ErrorShape::api_error_body` sharing one `Body` type exactly when every error body names the same type. `e2e` drives each shape's dispatch over a mock, `frontend/` every pattern of one to three entries, and `diff/` a new bodyless error status as major. `Responses::success`'s doctest pins its two `default` claims; pin a narrowed claim the same way. The `uses_*` predicates and `MediaType::stream_framing` as tables. `check_invariants` reporting a dangling reference at every position (`every_reachable_type_reference_is_checked_for_dangling`), a surviving `TypeKind::Reserved` and a bad octet-stream body as `E011`; no wildcard `match` over `TypeKind` that does not name `Reserved` first; `graph.rs`'s elision and type equivalence. |
| `codegen` / `emit` | `spargen/tests/e2e.rs` with `spargen/tests/e2e/` + `spargen/src/codegen/{mod,client,dispatch,docs,models,responses,ty}.rs` and `emit` in-module + `spargen/tests/determinism.rs` | Generate into an application-owned fixture crate under `cargo check` + `cargo clippy -D warnings`; extend `spargen/tests/e2e/specs/` when emitting new constructs. `BASIC_SPEC` or `OAS32_SPEC` emits every `support::AuthKind` and `support::Framing` (`the_compiled_specs_emit_every_auth_kind_and_stream_framing`). In-module: single arms and tables (`normalize_rustdoc`, `CLIENT_METHODS`, `TYPES_MODULE_NAMES`, `reqwest_method`), and the provenance header's exact, comments-only text. Determinism: byte-identical double generation. |
| build cache | `spargen/src/cache.rs` + `spargen/tests/e2e/manifest_audit.rs` | Complete input fingerprints, each field length-framed; missing, stale and hand-edited output invalidated; every cache record `read_cache` gives up on rerunning the pipeline, and a rerun that renders the bytes on disk leaving the file untouched. From a real build script, the manifest audit a cache hit reruns and the one `check` runs. |
| `diag` + docs ↔ code | `spargen/src/diag/code/tests.rs` + `spargen/src/diag/{mod,collect,pointer,span,fnv,interp}.rs` in-module | Code strings round-trip, and every code has a title and explain text; `docs/errors.md` lists exactly `Code::all()` with its severities and titles; every code a support document cites is real, and every code sits in `docs/support-matrix.md`'s column for its severity; `E023`'s workspace-inheritance clauses there are verbatim excerpts of its explain body; every code is asserted by `frontend/` or the suite `OWNED_ELSEWHERE` names. In-module: each severity's `OutcomeClaim`, duplicate retention, the JSON Pointer escape laws, a `FileId` reading back its index (`span.rs`), the FNV-1a vectors, `InterpId` rendering. |
| facade | `spargen/src/{lib,report,pipeline,diff_api}.rs` in-module | `vendor` building no warning diagnostics (`vendor_emits_no_warning_severity_diagnostics`); `Outcome`'s predicates and one spelling; `Report`'s severity partition, `expect_success`, `into_result` and Cargo directive severities; the public value types hashing; a carve that stalls or hits its round cap reporting the residual rejection; `Side`'s one spelling. |
| `test_support` | `spargen/src/test_support.rs` in-module | `read_repo_file` panicking on a missing file and `repo_root` found in any checkout, so self-reading tests fail rather than skip; `is_test_fn` accepting exactly a function its `#[test]` immediately precedes. |
| `runtime_contract` | `spargen/src/runtime_contract/tests/` | The consumer-manifest audit, one module per concern: `versions`, `targets`, `inheritance`, `walk`, `manifest_naming`, `explain` (`E023`'s text against the rules) and `deps_block` (`spargen deps`'s block, byte for byte, passing its own audit). `mod.rs`: the shared sandbox, the build-script manifest path, and the Cargo-integration policy for each environment and its messages, which `spargen/tests/config.rs` also drives through `Build`. |
| `source` | `spargen/src/source/{parse,bundle,lock,remote,vendor,sha256}.rs` in-module (tests in `parse/tests.rs`, `vendor/tests.rs`) + `spargen/src/source/selfid_diff_tests.rs` + `spargen/tests/vendor_remote.rs` and `spargen/tests/tls_strictness.rs` | Both parsers' spans, number classification, duplicate keys, and nesting and alias budgets at the bound and one past (as `E011`'s explain text states them); bundle spellings and path normalisation; `spargen.lock`'s round trip and rejections; `$ref` collection, and the `http`/`https` and `$self` classifiers the build and `spargen lock` share; what `spargen lock` pins per `$self` form, and `E025` on an untrusted TLS server; NIST and corpus hashes. Under `remote-fetch`, the real fetcher over plain HTTP (failures as `E025`, redirects, offline generation from the pin), and its refusal of a non-conformant TLS 1.3 peer (RUSTSEC-2026-0285, the `rustls` floor). |
| `name` | in-module proptests (`allocate`'s in `spargen/src/name/tests.rs`) | Determinism, injectivity in scope, valid identifiers, keyword escaping, type and operation names independent of order, and a method keeping a spelling a URL accessor wants. |
| `compat` | `spargen/src/compat/{mod,glob,rule,carve}.rs` in-module + `spargen/tests/carve.rs` (fixtures in `carve/`) + `e2e` | Omit rules match and apply; fingerprints stable and order-sensitive; carve reaching a deterministic fixpoint. `W009`/`E019` per rule in-module and through the public API in `carve.rs`, and `E020` in `carve.rs`, the suite `OWNED_ELSEWHERE` names for all three. |
| `support-runtime` | each file's `#[cfg(test)]` module (out of line in `<stem>/tests.rs` where that exists); the shared fakes in `lib.rs`'s | URL building, auth (every scheme, alternatives, failures), status classification, the error taxonomy, and each failure path: a body failing mid-read, a transport failure, a `Pending` poll, an overflowing backoff. No async runtime (`poll_ready`/`poll_to_completion` with `Waker::noop`), except `blocking.rs`'s tests and the refused-connection test. |
| whole tool | `examples/petstore` + `examples/petstore-macro` (`mise run example`) | The generated client over real HTTP against a mock server, via the `build.rs` and macro paths; the macro run asserts spargen stays out of the runtime graph (`cargo tree -e no-proc-macro`). |
| corpus | `spargen/tests/corpus_manifest.rs` / `mise run corpus-smoke` | `corpus/manifest.toml` is the single source of expectations (`expect = "generate"` / `"reject:E###"`); update them only with a reviewed reason. The suite checks the `expect` grammar (`every_declared_expectation_is_a_shape_the_suite_understands`), drives every case (`every_case_meets_its_declared_expectation`), verifies each file is fetched content matching its pinned `sha256` (`every_pinned_spec_is_present_and_is_not_an_unfetched_lfs_pointer`, `every_pinned_spec_matches_the_hash_the_manifest_declares`), and holds the smoke task, in both directions, and through the pairing test the CI job (`the_corpus_smoke_gate_covers_every_manifest_case`), `snapshot.rs` (`the_snapshot_suite_covers_every_manifest_case`), and every column of `corpus/README.md`'s case table (`the_corpus_readme_mirrors_the_manifest`) to the manifest — adding a case means adding it everywhere. |
| repository gates | `spargen/tests/repo_gates/` | Assertions over `mise.toml`, `.github/workflows/`, `hk.pkl`, `rust-toolchain.toml`, `deny.toml` and this file, each in the module of its kind (`supply_chain.rs`, `pairing.rs`, `toolchain.rs`, `claude_md.rs`), with shared readers in `main.rs` and `shared/repo.rs`. Task/CI pairing (`every_mise_task_runs_exactly_what_its_ci_job_runs`), and every hook step a `mise run` of a task that exists (`every_hook_step_runs_a_mise_task_that_exists`); tool and toolchain pins, with the shell-line readers they judge CI by (`ci_installs_exactly_the_tool_versions_mise_pins`, `the_tool_reader_reads_installs_and_runs_from_shell_lines`, `ci_installs_the_rust_toolchain_this_file_pins`, `the_msrv_gate_runs_on_the_declared_rust_version`); the supply-chain audit's scope, every flag held to an allow-list (`the_deny_gate_states_the_feature_scope_it_audits`, `the_lockfile_audit_reads_every_committed_lockfile`, `the_published_lockfile_audit_covers_every_shipped_binary`, `advisory_floors_are_manifest_requirements`, `the_audit_flag_allow_lists_admit_only_what_they_name`), the TLS stack that scope exists for and the example gate's TLS-crate rule (`the_all_features_workspace_graph_carries_a_tls_stack`, `the_tls_crate_rule_classifies_exactly_what_the_example_gate_greps`); no fixed `/tmp/` path (`no_gate_writes_a_fixed_tmp_path`), and every corpus-smoke redirect into `target/corpus-smoke/` (`the_corpus_smoke_gate_writes_only_inside_the_checkout`); no LFS smudge in the release preview (`the_release_preview_never_smudges_lfs_content`); the release benchmarks dispatched from `release-plz.yml` on the `spargen` tag, and no workflow triggered by a tag push or a release event, which release-plz's `GITHUB_TOKEN` never starts (`benchmarks_are_recorded_once_for_each_spargen_release`); and this file's Quality list and table (`the_quality_list_quotes_its_tasks_verbatim`, `the_testing_strategy_table_names_every_test_in_its_suite`, `a_cited_test_counts_whatever_its_name_is_shaped_like`). This row names every test in the suite's files, and only tests they define; the corpus row does the same for `corpus_manifest.rs`. |
| `surface` | `spargen/tests/diff.rs` with `spargen/tests/diff/` + in-module | `spargen diff` semver classification per change kind, the same pair classified alike twice, and `DiffSides` and `DiffRejection` reporting each side's lowering. Every `ChangeKind` needs a fixture there; the in-module tests enforce that (only an in-crate test sees a new `#[non_exhaustive]` variant) and pin the impact policy, the kebab-case codes, and the human report's spellings. |
| `config` / CLI | `spargen/tests/config.rs`, `spargen/tests/cli.rs` + `spargen/src/config.rs` and `spargen/src/cli/config.rs` in-module | Config discovery and precedence; `spargen deps` output; the `Spec` and `Build` getters reading back the defaults and what the setters wrote; the subcommand set (`generate` deliberately absent); exit status 3 on an unreadable config and 2 on a command line clap rejects; the stream each human report goes to, and a successful `deps` or `diff` printing its warnings on stderr. In-module: pinned defaults, each knob's round trip, setters winning over the file, `omit` mapping, each malformed entry's error, each `--omit` flag kind, switch pairs overriding the file either way, and both flags of a pair as a clap argument conflict. |
| `spargen-macro` | `spargen-macro/src/tests.rs` (out of line from `lib.rs`) + `examples/petstore-macro` | `generate_api!`'s argument parser (every build-configuration control, a bare spec path setting none, each switch overriding the config file either way, and an error naming each malformed argument); the config file beside the spec applied under the arguments; the consuming crate located from `CARGO_MANIFEST_PATH` or `CARGO_MANIFEST_DIR`, and an error when neither names it; a rejection rendered with each error's code, span and remedy; and the tracked inputs (spec, its refs, manifest, lock, config). |
| lowering invariants | `spargen/tests/lowering_props.rs` | Proptests over union/`allOf` lowering: category disjointness (object and `null` branches included), closed-object disjointness, exact `allOf` merge, recursively. |
| robustness | `spargen/tests/fuzz_frontend.rs` | A fixed-seed proptest no-panic harness over `check` (arbitrary bytes, keyword-biased and skeleton documents, deep `$ref` chains) through both parsers, and over `generate` for operation documents; `vendor` and the macro preview held to the frontend stack. `fuzz/` is its nightly libFuzzer counterpart. |
| snapshots | `spargen/tests/snapshot.rs` | One per corpus case: the outcome, a sorted diagnostic histogram, and an API surface for small generating cases — deliberately **not** emitted source, which `e2e__basic_spec_module.snap` and `e2e__basic_spec_module_no_features.snap` pin for `BASIC_SPEC` alone, embedded runtime included. Regenerate with `INSTA_UPDATE=always cargo test -p spargen --test e2e basic_spec_module` and review the diff. |
| generated runtime surface | `spargen/tests/reexport_lists.rs` | The embedded `support` module's `pub use` list equals `support-runtime/src/lib.rs`'s; every root re-export names something `support` re-exports, and no operation error type shadows one; every runtime type a root re-export names is itself nameable at the root (`every_runtime_type_a_root_reexport_names_is_nameable_at_the_root`); the root surface is the golden `snapshots/reexport_lists__root_surface.snap`. |
| framework round-trip | `spargen/tests/recipes.rs` | The OpenAPI documents utoipa / aide / poem-openapi actually emit. |
| layering | `spargen/tests/layering/` | `dag.rs`: each `//! layer-deps:` header equals its production `crate::` edges, without cycles, `lib.rs`'s DAG table matches the headers, and every module is in `SUBSYSTEMS`, `FACADE_PLUMBING` or `TEST_SUPPORT`. `runtime_embed.rs`: one trailing `#[cfg(test)]` per runtime source and none in generated output, comments above it naming no undisclosed test-only item, the `src/support/runtime/` symlinks equal to `support-runtime/src`, and the error variant counts. `packaging.rs`: every included file ships, and binary-spawning tests need `cli`. `file_length.rs`: `MAX_FILE_LINES` (1666) and an `OVER_LENGTH_BASELINE` that only shrinks. `docs_taxonomy.rs`: Markdown cites only real error variants, and `README.md` names each. |
| docs ↔ repository | `spargen/tests/docs.rs` | Every `spargen`/`spargen-macro` install line in a Markdown file (changelogs excepted) requires the current minor. The ignored `every_link_in_the_built_book_resolves` is the link check `mise run docs` runs after `mdbook build`: an unresolved `{{#include}}` or a relative link or anchor that does not resolve fails, so a standalone `docs/*.md` page links outside `docs/` by repository URL. |

## Commits

Commits MUST follow [Conventional Commits](https://www.conventionalcommits.org/) (`feat:`,
`fix:`, `chore:`, …) — enforced by `convco` at commit time, on pre-push, and in CI. Merge
commits are exempt.

## Releases

Releases are driven by release-plz: merging its version-bump pull request tags the release and
publishes to crates.io. Never bump the version or tag manually. The semver surface is the public
API of generated output.

release-plz edits no Markdown besides the changelogs, so a release pull request that moves the
minor fails its `test` job on `every_install_line_names_the_released_minor`
(`spargen/tests/docs.rs`), which holds every `spargen`/`spargen-macro` install line to the
workspace's minor. Before merging that pull request, push one `docs:` commit to its release-plz
branch that bumps every line the test's failure lists to the new minor (at present
`docs/book/src/cli.md` and `spargen-macro/README.md`), and merge only once its `test` job is green. That commit touches nothing else; the version and the changelog
stay release-plz's.

Publishing runs only in CI, via crates.io Trusted Publishing (OIDC, no `CARGO_REGISTRY_TOKEN`):
`release-plz.yml` mints a short-lived token with `rust-lang/crates-io-auth-action` and publishes
`spargen` before `spargen-macro`, which depends on it. Both crates' one-time manual bootstrap
(`0.1.0` and `0.2.0`, then a Trusted Publisher for `getkono/spargen`'s `release-plz.yml`) is
complete. The published crate stays self-contained: the runtime ships through the
`spargen/src/support/runtime/` symlinks, which the `package` job's `cargo publish --dry-run`
enforces. On release pull requests, release-plz verifies the macro artifact once the new `spargen`
reaches the registry.
