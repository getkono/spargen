# spargen frontend fuzzing

Deep, opt-in [cargo-fuzz](https://rust-fuzz.github.io/book/cargo-fuzz.html) (libFuzzer)
harness for the `oas31` frontend. It feeds arbitrary bytes to `spargen::check` — the full
`source` → `oas31` → `ir` → `name` pipeline — and asserts that **no input can panic,
overflow the stack, or abort the generator**. It also holds every report to the oracles the
`spargen/tests/` suites share, compiled from their one copy in
[`../spargen/tests/oracles/mod.rs`](../spargen/tests/oracles/mod.rs): no `E011` that reports
an IR invariant or codegen failure (a spargen bug, not a rejection), and every diagnostic
naming a real location. A report that breaks one panics, so libFuzzer records it as a crash.
`check` is hermetic (no network, no output written), so it is safe to run in a tight loop.

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

The first byte of each input picks the parser route from its value modulo 3: `0` writes the
rest as `spec.json` (JSON), `1` as `spec.yaml` (YAML), and `2` as `spec.txt` (the
extension-sniff fallback, YAML then JSON).

### Dictionary

[`dict/openapi.dict`](dict/openapi.dict) holds the tokens an OpenAPI description is built
from — root and operation keys, parameter styles, media types, Schema Object keywords, type
names and formats, and `$ref` targets — so mutation reaches document assembly, resolution and
lowering rather than stopping at the parsers. Pass it to libFuzzer:

```bash
cargo +nightly fuzz run frontend -- -dict=fuzz/dict/openapi.dict
```

### Seeding from `corpus/`

The pinned real-world descriptions under [`../corpus/`](../corpus/) make a good seed corpus.
They are Git-LFS files, so fetch their content first (a seed made from an unfetched pointer
file is valid input but teaches the fuzzer nothing). Each seed needs the route byte above in
front of the document. `cargo fuzz` reads `fuzz/corpus/frontend/` by default, and
`fuzz/.gitignore` keeps it out of commits:

```bash
# from the repo root:
git lfs pull --include 'corpus/**'
mkdir -p fuzz/corpus/frontend
find corpus -size -64k \( -name '*.json' -o -name '*.yaml' \) | while read -r f; do
  case "$f" in *.json) route='\000' ;; *) route='\001' ;; esac
  { printf "$route"; cat "$f"; } > "fuzz/corpus/frontend/$(echo "$f" | tr / _)"
done
cargo +nightly fuzz run frontend -- -dict=fuzz/dict/openapi.dict -max_len=65536
```

The `-size -64k` filter keeps the small descriptions (the `recipes/`, `ollama`,
`kubernetes-authentication-v1`, and the files of `openapi-boilerplate`) and leaves out the
multi-megabyte ones: libFuzzer runs every seed on each start and mutates within `-max_len`,
so a 12 MB description slows every execution without widening what mutation explores. The
files of `openapi-boilerplate` other than its `openapi.yaml` are fragments its root `$ref`s;
alone they reach the frontend's rejection paths, which is coverage too. The same recipe
seeds from `spargen/tests/e2e/specs/` by adding that directory to the `find`.

Any crash is written to `fuzz/artifacts/frontend/` and reproduced with:

```bash
cargo +nightly fuzz run frontend fuzz/artifacts/frontend/crash-<hash>
```

## If the fuzzer finds a crash

A crash is a real bug: the frontend must reject or handle every input **gracefully**, with a
diagnostic, and every report must pass the shared oracles (a crash whose panic message
begins `the report broke an oracle` lists each violation). Fix it in the frontend
(source/oas31/ir), then add a regression fixture to `spargen/tests/frontend/` (or the
relevant in-module test) so it cannot reappear silently, per the repo's bug-fix discipline.
