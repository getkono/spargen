---
paths:
  - "support-runtime/**"
  - "spargen/src/support/**"
---

# support-runtime

- Everything above a source file's `#[cfg(test)]` marker is embedded, trimmed and path-rewritten,
  into generated code and must compile there. Each file keeps that test module last: inline, or
  `mod tests;` reading `<stem>/tests.rs`. Comments above it name no undisclosed test-only item.
- `spargen/src/support/runtime/` is symlinks to `support-runtime/src`, which is how the published
  crate ships the runtime; `mise run package` proves it stays self-contained.
- Its unconditional dependencies are exactly `reqwest`, `http`, `http-body`, `url`, `serde`,
  `serde_json`, `bytes` and `secrecy`. Anything else is optional and enabled only by the
  conditionally embedded module that uses it: `futures-core` and reqwest's `stream` feature
  (`chunked`, `stream`), `quick-xml` (`xml`), `tokio` (`blocking`), and `time`, `jiff` and `chrono`
  (the date-time backends). A new dependency also moves the consumer-manifest
  audit in `spargen/src/runtime_contract/`.
- `lib.rs`'s `pub use` list is what the generated `support` module re-exports
  (`spargen/tests/reexport_lists.rs`).
- Tests run without an async runtime (`poll_ready` / `poll_to_completion` with `Waker::noop`),
  except `blocking.rs`'s tests and the refused-connection test. Cover each failure path: a body
  failing mid-read, a transport failure, a `Pending` poll, an overflowing backoff.
