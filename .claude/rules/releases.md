---
paths:
  - "release-plz.toml"
  - "**/CHANGELOG.md"
  - ".github/workflows/release-plz.yml"
  - "spargen/tests/docs.rs"
  - "docs/book/src/cli.md"
  - "spargen-macro/README.md"
---

# Releases

- release-plz opens the version-bump pull request; merging it tags and publishes. Never bump a
  version or tag by hand; the version and the changelog stay release-plz's.
- release-plz edits no Markdown besides the changelogs, so `release-plz.yml`'s `release-plz-pr`
  job runs `mise run install-lines` on the release branch after every release-plz run and commits
  what it rewrites as a signed `docs:` commit; `every_install_line_names_the_released_minor` holds
  the result. Push nothing to the release branch by hand: release-plz force-pushes it on every
  `master` push.
- That job pushes with a GitHub App token (secrets `RELEASE_APP_ID` and `RELEASE_APP_PRIVATE_KEY`;
  Contents and Pull requests write), so the release PR's CI runs every job, slow gates included,
  without approval. Merge only once that run is green.
- Publishing runs only in CI through crates.io Trusted Publishing (`release-plz.yml`), `spargen`
  before `spargen-macro`. The runtime ships through the `spargen/src/support/runtime/` symlinks.
- Release benchmarks are dispatched from `release-plz.yml` on the `spargen` tag.
