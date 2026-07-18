# Benchmarks

Spargen's performance surface, and a documented, reproducible methodology for comparing it against
the two Rust-relevant OpenAPI client generators: [`progenitor`][progenitor] (Rust) and
[`openapi-generator`][openapi-generator] (Java, polyglot).

Two things are measured:

1. **Generation wall-clock** — how long it takes to turn a spec into a client. Measured for
   spargen with a [criterion][criterion] micro-benchmark, and across tools with a shell harness.
2. **Output ergonomics / compile-correctness** — spargen's actual differentiators, which a
   stopwatch does not capture. Discussed qualitatively under [Fair comparison](#fair-comparison).

> Speed is not the headline. Spargen's contract is *compile-time correctness* for OpenAPI **3.1**:
> typed unions with no `serde(untagged)`, a freestanding runtime, `include!`-safe output, and no
> silent degradation to `serde_json::Value`. The numbers below exist to show spargen is fast
> *enough* to sit in a `build.rs`, and to contextualize it honestly against tools with different
> scopes — not to win a race that isn't the point.

## Spargen's own benchmark

`spargen/benches/generate.rs` is a criterion benchmark over the real pipeline. It is a
**dev-only** artifact: `criterion` is a dev-dependency (never in the library, `build.rs`, or
generated-output graph), and `cargo bench` is separate from the `cargo test` gate.

Run it:

```bash
cargo bench                       # full run (criterion picks sample sizes for stable statistics)
mise run bench                    # the same, via the task runner
cargo bench --no-run              # compile-only (CI runs this on every push, so benches can't rot)
cargo bench --bench generate -- --warm-up-time 0.5 --measurement-time 1.5   # quick run
```

### Downloadable results (no local setup)

You do not need to run anything to see the numbers. The [`Benchmarks`](../.github/workflows/benchmarks.yml)
workflow runs on every release tag (and on demand via *Run workflow*) and uploads a
`benchmarks-<ref>` artifact containing `bench-results.txt` (the captured summary) and criterion's
full report tree. Grab it from the workflow run's *Artifacts* section. CI-runner numbers are noisy
in absolute terms — read them for ratios and cross-release trend, not stopwatch precision.

Two benchmark groups, each over three inputs (a tiny inline spec, the 3.1 `petstore` example, and
the real-world `ollama` corpus spec):

- **`check/*`** — the frontend only (`source` → `oas31` → `ir` → `name`): parse, meta-schema
  validate, lower, name-allocate. No codegen, no filesystem writes. This is the shared cost every
  `generate` also pays, and what `spargen check` (the CI contract gate) runs.
- **`generate/*`** — the full pipeline through `codegen` + `emit`, writing a crate to a scratch
  tempdir. The delta over `check` is codegen + `prettyplease` formatting + output I/O, which
  dominates.

### Observed numbers

Illustrative snapshot on one developer machine (`rustc 1.97.0`, release/`bench` profile, Linux
x86-64). Absolute numbers vary by hardware; the **ratios** are the durable signal.

| Benchmark          | Input      | Wall-clock (median) |
| ------------------ | ---------- | ------------------- |
| `check/tiny`       | inline     | ~122 µs             |
| `check/petstore`   | 3.1, ~30 LOC spec | ~232 µs      |
| `check/ollama`     | 3.1, real  | ~1.88 ms            |
| `generate/tiny`    | inline     | ~5.5 ms             |
| `generate/petstore`| 3.1        | ~6.2 ms             |
| `generate/ollama`  | 3.1, real  | ~10.6 ms            |

Reading it: the frontend is sub-2 ms even on a real spec; full generation is single-digit-to-low-
double-digit milliseconds. `prettyplease` formatting and writing the crate dominate `generate`
(hence the ~5 ms floor even for the tiny spec). This is comfortably inside a `build.rs` budget.

## Comparing against progenitor and openapi-generator

`scripts/compare-generators.sh` runs each tool on the **same spec**, timing whole-process
generation and measuring output size. It **skips any tool that is not installed** and reports —
without failing — any tool that runs but rejects the spec.

```bash
scripts/compare-generators.sh [SPEC] [OUT_DIR]
# SPEC     default: examples/petstore/petstore.yaml
# OUT_DIR  default: a fresh mktemp -d
# env: RUNS=<n> (timed repetitions, best wall-clock reported; default 3)
#      SPARGEN_BIN=<path> (skip the release build, time an existing binary)
```

The committed artifact is the script plus this doc — it downloads no binaries. Timing includes
each tool's full process startup (fair: JVM boot, cargo dispatch, and spargen's frontend worker
thread are all real per-invocation costs).

### Installing the other tools

- **progenitor** — a Rust cargo subcommand:
  ```bash
  cargo install cargo-progenitor
  # invoked as: cargo progenitor --input SPEC --output DIR --name NAME --version 0.0.0
  ```
- **openapi-generator** — a Java CLI (needs a JRE on `PATH`). Easiest via npm's launcher, or the
  jar / Docker image:
  ```bash
  npx --yes @openapitools/openapi-generator-cli generate -i SPEC -g rust -o DIR
  # or: docker run --rm -v "$PWD:/local" openapitools/openapi-generator-cli generate \
  #       -i /local/SPEC -g rust -o /local/DIR
  ```
  The harness auto-detects `openapi-generator-cli`, `openapi-generator`, or `npx`, and uses the
  `rust` generator. (`rust-server` is the server-stub generator — not comparable here.)

### Observed snapshot

Really run in this repo's environment. Tool versions: **spargen 0.1.0**, **cargo-progenitor
0.14.0** (bundles progenitor 0.14 / typify 0.6), **openapi-generator-cli 7.23.0** on **OpenJDK
26**; `rustc 1.97.0`; `RUNS=3`, best wall-clock. Numbers are illustrative — a single machine, tiny
specs — not a published leaderboard.

Spec A — `examples/petstore/petstore.yaml` (**OpenAPI 3.1**):

| Tool               | Result   | Wall-clock | Output |
| ------------------ | -------- | ---------- | ------ |
| spargen            | ok       | ~10 ms     | 120 KiB |
| progenitor         | **rejected** — `invalid version: 3.1.0` | — | — |
| openapi-generator  | ok       | ~1254 ms   | 96 KiB |

Spec B — a minimal hand-written **OpenAPI 3.0.3** document (one operation, one model):

| Tool               | Result   | Wall-clock | Output |
| ------------------ | -------- | ---------- | ------ |
| spargen            | **rejected** — `E001` (3.0 is out of scope, by design) | — | — |
| progenitor         | ok       | ~71 ms     | 12 KiB |
| openapi-generator  | ok       | ~1151 ms   | 64 KiB |

What this actually shows:

- **There is no spec all three accept.** spargen is 3.1-only and *rejects* 3.0 loudly (`E001`);
  progenitor is 3.0-only and *rejects* 3.1 (`invalid version: 3.1.0`). That disjointness is the
  single most important comparison result — see below. openapi-generator accepts both.
- On a spec spargen supports (3.1), it generates in **~10 ms** vs openapi-generator's **~1.25 s** —
  roughly two orders of magnitude, essentially all JVM/generator startup. Fast enough to run every
  build; openapi-generator is a one-shot scaffold step you would not want in a hot `build.rs` loop.
- progenitor, when it accepts a spec, is also fast (~70 ms) — it too is a native Rust tool. But it
  accepts a *narrow* slice of specs: beyond the version gate it rejected a real 3.0 Kubernetes spec
  on `unexpected content type: */*`. It targets clean, purpose-written 3.0 documents.
- Output size is not quality: spargen's larger output embeds the **freestanding runtime** verbatim
  (its whole point — no external client crate), where progenitor factors out a `progenitor-client`
  dependency and openapi-generator emits a thinner crate over `reqwest`. Bytes are not comparable
  across these contracts.

## Fair comparison

The three tools do genuinely different jobs; a wall-clock table without this context would
mislead.

- **OpenAPI version scope is disjoint.** Spargen is **3.1-only** and treats 3.0 as a hard,
  diagnosed rejection (`E001`) rather than silently mis-generating it. progenitor is built on the
  `openapiv3` crate and is **3.0-only** — it rejects 3.1 outright. openapi-generator spans 2.0/3.0/
  3.1. So the *same* spec cannot be a like-for-like input for spargen and progenitor; each must run
  on a spec in its own dialect. This is the honest headline, not the milliseconds.
- **Correctness contract differs.** Spargen's value is what it *won't* do: no `serde(untagged)`
  first-match-wins unions (it emits content-inspecting or discriminated deserializers instead), no
  silent `serde_json::Value` fallback, and every unsupported construct is warned or rejected with a
  stable code — never silently dropped. A generation-time benchmark cannot see any of this;
  measuring only wall-clock rewards a tool for cutting exactly these corners.
- **Output shape differs.** Spargen emits a **freestanding, `include!`-safe** module/crate with the
  runtime embedded and no `spargen` dependency in the consumer graph. progenitor depends on
  `progenitor-client`; openapi-generator emits a conventional crate over `reqwest`. "Output size"
  therefore measures contract, not efficiency.
- **Startup is part of the cost — and it's honest to include it.** openapi-generator's ~1.2 s is
  dominated by JVM boot and generator initialization; that cost is real every invocation and is why
  a native tool is preferable inside a build. The harness includes process startup for all tools so
  no one is unfairly advantaged.

Bottom line: use these numbers to confirm spargen is fast enough to live in a `build.rs`, and to
understand that the *interesting* comparison against progenitor and openapi-generator is about
**scope and correctness guarantees**, not speed.

## Reproducing

```bash
# Spargen's own pipeline benchmark:
cargo bench --bench generate -- --warm-up-time 0.5 --measurement-time 1.5

# Cross-tool comparison on a 3.1 spec (spargen + openapi-generator run; progenitor rejects 3.1):
scripts/compare-generators.sh examples/petstore/petstore.yaml

# Cross-tool comparison on a 3.0 spec (progenitor + openapi-generator run; spargen rejects 3.0):
scripts/compare-generators.sh path/to/a-3.0-spec.json
```

Install the external tools as shown [above](#installing-the-other-tools); the harness skips any
that are absent, so it is safe to run with none, one, or all of them present.

[criterion]: https://github.com/bheisler/criterion.rs
[progenitor]: https://github.com/oxidecomputer/progenitor
[openapi-generator]: https://openapi-generator.tech/
