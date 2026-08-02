# CLI Reference

The `spargen` binary is built with the `cli` feature (`cargo install spargen --features cli`). It
provides analysis and schema-vendoring tools only. Client generation belongs in `build.rs` or
`generate_api!`; the CLI intentionally has no `generate`, stdout, watch, drift, or crate-scaffold
path.

```text
Usage: spargen <COMMAND>

Commands:
  check    Audit a spec's feature support without generating code
  lock     Fetch, vendor, and hash-pin remote $refs into spargen.lock (the only networked step)
  explain  Show extended documentation for a diagnostic code
  diff     Report the semver impact of regenerating the client from a newer spec
```

## `spargen check`

Audit a vendored spec with the same frontend used during generation, without emitting code.

```bash
spargen check <SPEC> [OPTIONS]
```

It accepts `--format <human|json>`, `--config`, `--carve`, and repeatable `--omit-path`,
`--omit-operation`, `--omit-component`, and `--omit-pointer` flags. These are analysis controls;
put generation controls in `Config` or `generate_api!` for the compile-time path.

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
