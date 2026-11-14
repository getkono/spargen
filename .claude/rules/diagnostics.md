---
paths:
  - "spargen/src/diag/**"
  - "spargen/src/oas31/**"
  - "spargen/tests/frontend/**"
  - "docs/errors.md"
  - "docs/support-matrix.md"
---

# Diagnostics

- A new warning or rejection code gets, in the same commit: a stable code in `diag`, an entry in
  `docs/errors.md`, a cell in `docs/support-matrix.md` under its severity, and a fixture in
  `spargen/tests/frontend/`, a minimal inline spec (a rejection asserts `Outcome::Rejected` and the
  code; a warning that it fires and generation succeeds). Tests keyed on the code enforce the last
  three; a code `frontend/` cannot reach is owned by the suite `OWNED_ELSEWHERE` names.
- A case added under an existing code goes into that code's `explain` body (the one case list; the
  matrix defers to `spargen explain`) with a fixture. For a code in `ENUMERATED_CASES`
  (`diag/code/tests.rs`), each emission site carries a marker naming its explain case, and each
  case a fixture carrying the same `// E### case: <tag>` marker and asserting its message.
- Every frontend run is held to the oracles in `spargen/tests/oracles/`, and `generate` to
  `check`'s diagnostics.
