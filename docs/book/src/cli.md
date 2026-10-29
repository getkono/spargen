# CLI Reference

The `spargen` binary is built with the `cli` feature (`cargo install spargen --features cli`). It
provides analysis and schema-vendoring tools only: `check`, `deps`, `lock`, `explain`, and `diff`.
Client generation belongs in `build.rs` or `generate_api!`; the CLI has no `generate` subcommand.

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
spargen = { version = "0.5", features = ["remote-fetch"] }
```

`reqwest` is an optional dependency of the crate, and `remote-fetch` is the only feature that
enables it (`cli` reaches it through `remote-fetch`).

`spargen --help` prints the subcommands (a test holds this copy to the binary's output):

```text
A compile-time-correct Rust client generator for OpenAPI 3.1.x and 3.2.x.

Usage: spargen <COMMAND>

Commands:
  check    Audit a spec's feature support without generating code
  deps     Print the `[dependencies]` block generated output from this spec requires
  lock     Fetch, vendor, and hash-pin remote `$ref`s into `spargen.lock` (the only networked step)
  explain  Show extended documentation for a diagnostic code
  diff     Report the semver impact of regenerating the client from a newer spec
  help     Print this message or the help of the given subcommand(s)

Options:
  -h, --help     Print help
  -V, --version  Print version
```

`check`, `deps`, `lock`, and `diff` share one spec-options group — `--config`, `--carve`,
`--no-uuid`, `--no-time`, `--error-body-cap`, `--batch-cap`, `--open-narrowing`, and the
repeatable `--omit-path`,
`--omit-operation`, `--omit-component`, `--omit-pointer`. The group mirrors the setter list on
`spargen::Spec`, and the `spargen.toml` it reads is parsed by the library, the same file
`build.rs` and `generate_api!` read.

### Flags and `spargen.toml`

Each spec is resolved as built-in defaults, then its `spargen.toml` (the `--config` path, or the
file beside the spec when there is one), then the flags. `--error-body-cap` and `--batch-cap`
replace the file's value. The `--omit-*` flags add rules to the file's `[[omit]]` rules rather
than replacing them. The four switches only turn their setting one way, so a file's setting in the
other direction stands whatever the command line says:

- `carve = true` in the file cannot be turned off by a flag; `--carve` only turns carving on.
- `uuid = false` in the file cannot be turned back on; `--no-uuid` only turns it off.
- `time = false` in the file cannot be turned back on; `--no-time` only turns it off.
- `open_narrowing = true` in the file cannot be turned off; `--open-narrowing` only turns it on.

To undo one of those, edit the file or point `--config` at another. `diff` resolves each of its two
specs this way on its own: absent `--config`, each side discovers the `spargen.toml` beside it,
while `--config` applies its one file to both sides.

### Exit status

| Status | Meaning |
| --- | --- |
| `0` | Success, including a run that reported only warnings, and `--help` / `--version`. |
| `1` | An error-severity diagnostic was reported (`check`, `deps`, `lock`, or a `diff` spec that cannot lower), or `diff --exit-code` found a breaking (`major`) change. |
| `2` | The command line did not parse: an unknown subcommand or flag, or a missing argument. |
| `3` | A configuration or flag value was rejected (a missing `--config` target, a malformed `spargen.toml`, bad `--omit-*` syntax), or `explain` was given an unknown code. |

## `spargen check`

Audit a vendored spec with the same frontend used during generation, without emitting code.

```bash
spargen check <SPEC> [OPTIONS]
```

It accepts `--format <human|json>` plus the shared spec options above. A diagnostic whose location
is known renders as `--> <file>:<line>:<col>` in human output, naming the file it lies in, a
`$ref`'d file included, by the path the spec's references resolved to; under `--format json` the
same path is the diagnostic's `file`, beside its `span`. Run from a build script it
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
Cargo feature that would require them, together with that feature's `[features]` entry
(`blocking = ["dep:tokio"]`), so uncommenting them is the whole opt-in — except that an entry whose
`[features]` table, `blocking` key, or dependency table the manifest already declares is merged
into it, since TOML rejects a table or key defined twice. `--format json` emits the
same set structurally.

The block is written for the package's own tables, but a workspace member may inherit it instead:
move each entry, `default-features = false` included, into the root's `[workspace.dependencies]`,
and declare it `{ workspace = true }` in the member under the table the block names, keeping
`optional = true` on the member's line (Cargo refuses it in `[workspace.dependencies]`). The audit
follows that inheritance, and a test drives both layouts through it. A member's
`default-features = false` does not turn off defaults the root leaves on; the
[runtime dependency contract](getting-started.md#workspace-inheritance-and-the-full-contract)
carries the full rules.

## `spargen lock`

Fetch, vendor, and hash-pin remote `$ref`s into `spargen.lock`. This is the **only** networked
step; compilation-time generation and `check` never reach the network.

```bash
spargen lock <SPEC> [OPTIONS]
```

It accepts `--format <human|json>` plus the shared spec options, so one option set serves every
subcommand, but of the settings they resolve it reads only `batch_cap`. The configuration is still
resolved, so a malformed `spargen.toml` or a bad flag value exits `3` here as well.

Commit `.spargen/vendor/`, `spargen.lock`, the root document, and all relative source files.

A fetch that fails (an error status, a refused connection) is reported as `E025` and fails the
command, but what it leaves on disk is still a usable lock. Every document it did fetch is
re-vendored and re-pinned at what the server served. A document it could not fetch, and any
document reachable only through it, keeps the pin the earlier `spargen.lock` held, beside the
vendored copy the earlier run wrote, so `check` and builds keep resolving it offline. A pin this
run did not refresh stays until a `spargen lock` in which every fetch succeeds rewrites the lock
from scratch.

## `spargen explain`

Show the extended documentation behind a stable diagnostic code.

```bash
spargen explain E013 [--format <human|json>]
```

The human format prints the code's explanation text to stdout: what the code means and, for a
code with several causes, each case it covers. `--format json` prints one object,
`{ "code": …, "explain": … }`, holding the code as given and the same text. The
[diagnostic index](./errors.md) lists each code's severity and title; the explanation itself is
what `explain` prints, the same text the library's `spargen::explain` returns. An unknown code
prints an error to stderr and exits `3`.

## `spargen diff`

Report the generated public-API impact of moving from one vendored spec to another.

```bash
spargen diff <OLD> <NEW> [--exit-code] [OPTIONS]
```

It accepts `--format <human|json>` plus the shared spec options, applied to both specs.
`--exit-code` exits `1` for a breaking (`major`) change; without it a successful comparison exits
`0` whatever its bump. A spec that cannot lower is always a hard error and exits `1`.

`--format json` prints one object: `changes`, an array of `{ kind, impact, location, detail }`
sorted most-severe first, plus the top-level `bump`. `kind` is the stable kebab-case change code
(`operation-added`, `method-renamed`, …) and `impact`/`bump` are the lowercase semver labels
`major` / `minor` / `patch` — the same spellings the human rendering prints, so a script and a
reader never disagree. The set of `kind` codes grows with new change classes, so match on the
codes a caller knows and treat an unrecognized one as its stated `impact`.
