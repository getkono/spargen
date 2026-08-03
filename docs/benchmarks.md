# Benchmarks

Spargen's benchmark answers the operational question that matters for a compilation-time
generator: is it fast enough to run from `build.rs` on every invalidated build?

> Speed is not the headline. Spargen's contract is compile-time correctness for OpenAPI 3.1/3.2:
> typed unions with no `serde(untagged)`, a freestanding runtime, `include!`-safe output, and no
> silent degradation to `serde_json::Value`.

## Spargen's own benchmark

`spargen/benches/generate.rs` is a criterion benchmark over the real Rust API. It is dev-only:
`criterion` never enters the library, build-script, or generated-code dependency graph.

```bash
cargo bench
mise run bench
cargo bench --no-run
cargo bench --bench generate -- --warm-up-time 0.5 --measurement-time 1.5
```

### Downloadable results

The [`Benchmarks`](../.github/workflows/benchmarks.yml) workflow runs on every release tag and on
demand. Its `benchmarks-<ref>` artifact contains the captured summary and criterion report tree.
CI-runner numbers are noisy in absolute terms; use them for ratios and cross-release trends.

The benchmark covers a tiny inline spec, the petstore example, and the real-world Ollama corpus
spec in two groups:

- `check/*` runs the frontend only: parse, validate, lower, and allocate names.
- `generate/*` runs the full pipeline and writes a module into a temporary application-owned path.

### Observed numbers

Illustrative snapshot on one developer machine (`rustc 1.97.0`, release/bench profile, Linux
x86-64):

| Benchmark | Input | Wall-clock (median) |
| --- | --- | --- |
| `check/tiny` | inline | ~122 µs |
| `check/petstore` | 3.1, ~30 LOC spec | ~232 µs |
| `check/ollama` | 3.1, real | ~1.88 ms |
| `generate/tiny` | inline | ~5.5 ms |
| `generate/petstore` | 3.1 | ~6.2 ms |
| `generate/ollama` | 3.1, real | ~10.6 ms |

The frontend is sub-2 ms even on the real spec; full generation is single-digit-to-low-double-digit
milliseconds. Formatting and output I/O dominate the fixed floor. Build-script caching skips that
work entirely when the generator, full transitive input set, lock, and configuration are unchanged.

[criterion]: https://github.com/bheisler/criterion.rs
