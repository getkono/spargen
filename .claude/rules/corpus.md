---
paths:
  - "corpus/**"
  - "spargen/tests/corpus_manifest.rs"
  - "spargen/tests/snapshot.rs"
---

# Corpus

- `corpus/manifest.toml` is the single source of expectations (`expect = "generate"` or
  `"reject:E###"`). Change an expectation only with a reviewed reason.
- Adding a case means adding it everywhere `corpus_manifest.rs` checks: the manifest with its
  pinned `sha256`, `corpus/README.md`'s case table, the `corpus-smoke` task and CI job, and
  `snapshot.rs`.
- Specs are Git-LFS content; an unfetched pointer fails the suite rather than skipping.
- Snapshots pin the outcome, a diagnostic histogram and, for small cases, the API surface, never
  the emitted source.
