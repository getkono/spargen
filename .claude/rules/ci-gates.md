---
paths:
  - "mise.toml"
  - "hk.pkl"
  - ".github/workflows/**"
  - "rust-toolchain.toml"
  - "spargen/tests/repo_gates/**"
---

# CI gates

- A mise task and its CI job are **identical**: the same commands and flags, in the same order,
  under the same environment. CI spells the commands out rather than calling `mise`; change both
  sides together. The only differences are the named exceptions in `PAIRINGS`
  (`spargen/tests/repo_gates/pairing.rs`), each stating its reason. `release-plz.yml` is the one
  non-gate workflow.
- Each gate workflow's `on:`, `concurrency:`, `permissions:` and other steps are pinned literally,
  and a task key that changes what runs without appearing in its command is rejected.
- Slow gates carry one pinned `if:` (`SLOW_GATE_IF`): every push to `master`, `workflow_dispatch`,
  and release-plz's release pull request once a maintainer approves its runs.
- `hk.pkl`'s steps are each a `mise run` of an existing task; every other task is too slow for a
  hook. Pre-commit runs `fmt` and `lint` as fixers; pre-push runs only `fmt-check`,
  `commit-range` and the globbed `deny`, and CI is the gate for `lint` and `test`
  (`the_pre_push_hook_runs_only_the_fast_checks` pins that set).
- `rust-toolchain.toml`'s `channel` is a concrete release, and every `dtolnay/rust-toolchain@` step
  installs it; `TOOLCHAIN_EXCEPTIONS` names the others (`msrv` on the workspace `rust-version`, and
  the `runtime-dependencies` job's floating `@nightly`). Bump the pin in a pull request of its own
  that moves the `channel`, every step and the matching `PAIRINGS` literals together, and passes
  clippy and fmt on the new release.
- No gate writes a fixed `/tmp/` path, and no workflow is triggered by a tag push or a release
  event, which release-plz's `GITHUB_TOKEN` never starts.
