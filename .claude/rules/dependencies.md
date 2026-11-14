---
paths:
  - "**/Cargo.toml"
  - "**/Cargo.lock"
  - "deny.toml"
  - "spargen/src/runtime_contract/**"
---

# Dependencies, lockfiles and advisories

- Four lockfiles are committed: the workspace's and one per example workspace. A lockfile changes
  only with the manifest change that needs it, and the workspace one also in release-plz's release
  pull request. The rewrite a local `mise run example` or `mise run deny` makes is never committed.
- The supply-chain audit is not hermetic: an advisory or yank can turn every open pull request red
  with nothing committed. That red is the maintainers' to fix, the day it is seen, in a
  `fix(deps):` pull request of its own that merges first; open pull requests then merge `master`.
  The comments on `[tasks.deny]` and `[tasks.deny-published]` in `mise.toml` explain each command
  and its scope. `deny.yml` reruns `deny` daily; `deny-published` audits, on that schedule only,
  the `Cargo.lock` the latest release ships: fix it as a red `deny`, then release, or yank the
  affected version where no fix exists. GitHub disables a schedule after 60 days without
  repository activity; re-enable it (`deny.yml`, `deps-latest.yml`) from the Actions tab.
- Where a patched release exists and the crate is in **spargen's published graph** (a normal
  dependency of `spargen/Cargo.toml` under any feature), a lockfile bump is no fix, since Cargo
  ignores a dependency's lockfile. Add a direct, default-features-off requirement at the patched
  release to `spargen/Cargo.toml` (optional, enabled by the feature that reaches the crate, never
  named in code) *and* a `[bans] deny` entry in `deny.toml` (`name` + `version = "<patched"`, its
  `reason` naming the advisory), then bump each lockfile carrying the crate;
  `advisory_floors_are_manifest_requirements` holds the halves together. A later advisory moves
  both. Where the patched release is outside the parent's range (a new minor on a `0.x` line),
  bump the parent and floor what it still leaves open. A floor goes when the parent's own
  requirement reaches it or the parent moves off that line; removing the rustls floor also removes
  that test's assertion that it exists.
- **Anywhere else** (a dev-dependency, a crate only `support-runtime` or an example carries, a
  lockfile entry no feature activates): `cargo update -p <crate> --precise <patched>` in each
  lockfile that carries it.
- Where no patched release exists, add an `[advisories] ignore` entry to `deny.toml` saying why it
  does not reach this graph and what lifts it.
- A release never ships a runtime requirement behind its crate's latest release. `deps-latest`
  (weekly, and as a job every release waits on) fails on a requirement that rejects the newest
  stable release; nothing publishes until a pull request of its own moves the requirement, its
  floor and ceiling in `runtime_contract/requirements.rs`, and the lockfiles that carry the crate.
  That is the maintainers' work, the day it is seen. A requirement that admits the newest release
  from an older floor only prints a notice: raise the floor and cut a patch release when worth it.
