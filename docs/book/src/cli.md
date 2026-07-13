# CLI Reference

The `spargen` binary is built with the `cli` feature (`cargo install spargen --features cli`). It
exposes five subcommands. Every subcommand accepts `--format <human|json>` (default `human`);
`json` is the machine-readable form for CI.

```text
Usage: spargen <COMMAND>

Commands:
  generate  Generate a client from a spec (or, with --check, fail on drift)
  check     Audit a spec's feature support without generating code
  lock      Fetch, vendor, and hash-pin remote $refs into spargen.lock (the only networked step)
  explain   Show extended documentation for a diagnostic code
  diff      Report the semver impact of regenerating the client from a newer spec
```

## `spargen generate`

Generate a client from a spec.

```bash
spargen generate <SPEC> --out <OUT> [OPTIONS]
```

| Flag | Purpose |
| --- | --- |
| `<SPEC>` | Path to the root OpenAPI document (positional, required). |
| `-o`, `--out <OUT>` | Output module path, or crate directory with `--as-crate` (required). |
| `--check` | Fail if the checked-in output has drifted from the spec, instead of writing (`W004`). A CI drift gate. |
| `--as-crate` | Emit a standalone publishable crate rather than a single module. |
| `--watch` | Watch the spec and its referenced files, config, and lock; regenerate on every change. Runs until interrupted (Ctrl-C). |
| `--carve` | Auto-carve: instead of failing on rejections, omit the minimal set of unsupported constructs (each reported via `W009`) and generate the rest. Un-carvable rejections are still reported. |
| `--no-uuid` | Disable the `format: uuid` mapping (fall back to `String`). |
| `--no-time` | Disable the `format: date-time`/`date` mappings (fall back to `String`). |
| `--config <CONFIG>` | Path to a `spargen.toml` config file. Defaults to `spargen.toml` beside the spec, if present. |
| `--omit-path <PATH>` | Omit a path item and every operation under it (repeatable), e.g. `--omit-path /pets/{id}`. |
| `--omit-operation <METHOD /path>` | Omit one operation (repeatable), e.g. `--omit-operation "get /pets"`. |
| `--omit-component <kind:name>` | Omit a named component (repeatable), e.g. `--omit-component "schema:LegacyPet"`. |
| `--omit-pointer <[file#]/pointer>` | Omit an RFC 6901 pointer (repeatable), e.g. `--omit-pointer "[file#]/pointer"`. |

The `--carve` flag and the `--omit-*` family are the [compatibility omit mode](./compatibility.md):
carve unsupported segments out of a vendored spec without editing it.

## `spargen check`

Audit a spec's feature support without generating code. `check` and `generate` stay in
diagnostic parity — a spec that generates cleanly checks cleanly, and vice versa.

```bash
spargen check <SPEC> [OPTIONS]
```

Accepts `--config`, `--carve`, and the `--omit-path` / `--omit-operation` / `--omit-component` /
`--omit-pointer` family, all with the same semantics as `generate`.

## `spargen lock`

Fetch, vendor, and hash-pin remote `$ref`s into `spargen.lock`. This is the **only** networked
step; `generate` and `check` never reach the network.

```bash
spargen lock <SPEC>
```

Remote `$ref`s reachable from the root document are fetched, vendored under `.spargen/vendor/`,
and pinned in `spargen.lock` beside the spec.

## `spargen explain`

Show extended documentation for a diagnostic code — the same text the [diagnostic
index](./errors.md) carries.

```bash
spargen explain E013
```

## `spargen diff`

Report the semver impact of regenerating the client from a newer spec.

```bash
spargen diff <OLD> <NEW> [--exit-code]
```

| Flag | Purpose |
| --- | --- |
| `<OLD>` | Path to the OLD (baseline) OpenAPI document. |
| `<NEW>` | Path to the NEW (candidate) OpenAPI document. |
| `--exit-code` | Exit non-zero (status 1) when the diff is a breaking (`major`) change — a CI gate. Without this flag `diff` always exits 0 (a spec that fails to lower still exits 1 either way). |
