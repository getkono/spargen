# spargen frontend fuzzing

Deep, opt-in [cargo-fuzz](https://rust-fuzz.github.io/book/cargo-fuzz.html) (libFuzzer)
harness for the `oas31` frontend. It feeds arbitrary bytes to `spargen::check` — the full
`source` → `oas31` → `ir` → `name` pipeline — and asserts, by never crashing, that **no
input can panic, overflow the stack, or abort the generator**. `check` is hermetic (no
network, no output written), so it is safe to run in a tight loop.

The always-on CI guard is the bounded, deterministic proptest harness in
[`../spargen/tests/fuzz_frontend.rs`](../spargen/tests/fuzz_frontend.rs) (part of
`mise run test`). This crate is the manual, coverage-guided complement.

## This crate is excluded from the workspace

`fuzz/` is listed under `[workspace] exclude` in the root `Cargo.toml`. It depends on
`libfuzzer-sys` and requires the nightly libFuzzer sysroot, so it must never enter the
normal build/test/lint/deny paths. A contributor without cargo-fuzz installed is completely
unaffected: `cargo build --workspace`, `mise run check`, `mise run lint`, `mise run test`,
and `mise run deny` never touch it.

## Running

Requires a nightly toolchain and the cargo-fuzz subcommand:

```bash
rustup toolchain install nightly
cargo install cargo-fuzz

# from the repo root:
cargo +nightly fuzz run frontend                 # fuzz until a crash (or Ctrl-C)
cargo +nightly fuzz run frontend -- -max_total_time=60   # time-boxed run
cargo +nightly fuzz list                         # show available targets
```

Any crash is written to `fuzz/artifacts/frontend/` and reproduced with:

```bash
cargo +nightly fuzz run frontend fuzz/artifacts/frontend/crash-<hash>
```

## If the fuzzer finds a crash

A crash is a real bug: the frontend must reject or handle every input **gracefully**, with a
diagnostic. Fix it in the frontend (source/oas31/ir), then add a regression fixture to
`spargen/tests/frontend.rs` (or the relevant in-module test) so it cannot reappear silently,
per the repo's bug-fix discipline.
