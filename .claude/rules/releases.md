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
- release-plz edits no Markdown besides the changelogs, so a release that moves the minor fails
  `every_install_line_names_the_released_minor`. Before merging, push one `docs:` commit to the
  release branch that bumps every install line the failure lists (at present
  `docs/book/src/cli.md` and `spargen-macro/README.md`) and touches nothing else, and merge only
  once its `test` job is green.
- Publishing runs only in CI through crates.io Trusted Publishing (`release-plz.yml`), `spargen`
  before `spargen-macro`. The runtime ships through the `spargen/src/support/runtime/` symlinks.
- Release benchmarks are dispatched from `release-plz.yml` on the `spargen` tag.
