# CLI Reference

The `spargen` binary is built with the `cli` feature (`cargo install spargen --features cli`). It
provides analysis and schema-vendoring tools only. Client generation belongs in `build.rs` or
`generate_api!`; the CLI intentionally has no `generate`, stdout, watch, drift, or crate-scaffold
path.

## Cargo features

Two features gate the generator crate itself. Neither affects generated output — that stays
freestanding either way.

| Feature | Default | What it adds |
| --- | --- | --- |
| `cli` | off | The `spargen` binary and its argument parser. Implies `remote-fetch`, so an installed binary can run `spargen lock`. |
| `remote-fetch` | off | The `reqwest`-backed fetcher behind [`spargen lock`](#spargen-lock) — the only code path in the crate that opens a network connection. |

`remote-fetch` is separate from `cli` so a library-only consumer — a `build.rs`, or a tool calling
`spargen::vendor` directly — can vendor remote `$ref`s without pulling in the whole CLI stack:

```toml
[build-dependencies]
spargen = { version = "0.4", features = ["remote-fetch"] }
```

With neither feature on (the default), the crate links no HTTP client at all, and `generate` and
`check` cannot reach the network even in principle.

```text
Usage: spargen <COMMAND>

Commands:
  check    Audit a spec's feature support without generating code
  deps     Print the [dependencies] block generated output from this spec requires
  lock     Fetch, vendor, and hash-pin remote $refs into spargen.lock (the only networked step)
  explain  Show extended documentation for a diagnostic code
  diff     Report the semver impact of regenerating the client from a newer spec
```

`check`, `deps`, `lock`, and `diff` share one spec-options group — `--config`, `--carve`,
`--no-uuid`, `--no-time`, `--error-body-cap`, `--batch-cap`, and the repeatable `--omit-path`,
`--omit-operation`, `--omit-component`, `--omit-pointer` — which is exactly the setter list on
`spargen::Spec`, so the CLI, `build.rs`, `generate_api!`, and `spargen.toml` cannot drift.

## `spargen check`

Audit a vendored spec with the same frontend used during generation, without emitting code.

```bash
spargen check <SPEC> [OPTIONS]
```

It accepts `--format <human|json>` plus the shared spec options above. Run from a build script it
also audits the consuming package's `Cargo.toml` (`E023`); elsewhere there is no consuming package,
and `spargen deps` covers that ground instead.

## `spargen deps`

Print the exact `[dependencies]` block generated output from this spec needs. Generated output is
freestanding, so the consuming package declares its own runtime dependencies — and which ones
depends on the API: multipart bodies pull in a `reqwest` feature, `format: uuid` pulls in `uuid`,
sequential responses pull in `futures-core`. This answers that up front rather than one `E023` at a
time.

```bash
spargen deps <SPEC> [OPTIONS]
```

The block it prints is the block the audit accepts — both read one table, and a test pins that they
agree. Opt-in dependencies (the blocking client's `tokio`) are printed commented out under the
Cargo feature that would require them. `--format json` emits the same set structurally.

## `spargen lock`

Fetch, vendor, and hash-pin remote `$ref`s into `spargen.lock`. This is the **only** networked
step; compilation-time generation and `check` never reach the network.

```bash
spargen lock <SPEC> [--format <human|json>]
```

Commit `.spargen/vendor/`, `spargen.lock`, the root document, and all relative source files.

## `spargen explain`

Show the extended documentation behind a stable diagnostic code.

```bash
spargen explain E013 [--format <human|json>]
```

## `spargen diff`

Report the generated public-API impact of moving from one vendored spec to another.

```bash
spargen diff <OLD> <NEW> [--exit-code] [--format <human|json>]
```

`--exit-code` exits non-zero for a breaking (`major`) change. A spec that cannot lower is always a
hard error.

`--format json` prints one object: `changes`, an array of `{ kind, impact, location, detail }`
sorted most-severe first, plus the top-level `bump`. `kind` is the stable kebab-case change code
(`operation-added`, `method-renamed`, …) and `impact`/`bump` are the lowercase semver labels
`major` / `minor` / `patch` — the same spellings the human rendering prints, so a script and a
reader never disagree. The set of `kind` codes grows with new change classes, so match on the
codes a caller knows and treat an unrecognized one as its stated `impact`.
