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
spargen = { version = "0.7", features = ["remote-fetch"] }
```

`reqwest` is an optional dependency of the crate, and `remote-fetch` is the only feature that
enables it (`cli` reaches it through `remote-fetch`).

## Commands and shared options

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

`check`, `deps`, `lock`, and `diff` share one spec-options group — `--config`, the switch pairs
`--carve`/`--no-carve`, `--uuid`/`--no-uuid`, `--open-narrowing`/`--no-open-narrowing`,
`--open-unions`/`--no-open-unions`, `--verbatim-docs`/`--no-verbatim-docs` and
`--testing`/`--no-testing`, `--datetime`,
`--error-body-cap`, `--batch-cap`, and the repeatable
`--omit-path`, `--omit-operation`, `--omit-component`, `--omit-pointer`, `--keep-operation` and
`--bind-header`. The
group mirrors the setter list on `spargen::Spec` and the keys of [`spargen.toml`](#spargentoml-reference). `lock`
fetches every remote `$ref` the spec reaches whatever the omit and keep rules and switches say, so of the
group it reads only `--config` and `--batch-cap` (and the file's `batch_cap`); the rest are
accepted there and have no effect.

### Flags and `spargen.toml`

Each spec is resolved as built-in defaults, then its `spargen.toml` (the `--config` path, or the
file beside the spec when there is one), then the flags. `--datetime`, `--error-body-cap` and
`--batch-cap` replace the file's value. The `--omit-*` flags add rules to the file's `[[omit]]` rules,
`--keep-operation` to its `[[keep]]` rules, and `--bind-header` to its `bind_headers`, rather
than replacing them. Each switch pair sets its setting in either direction, so `--no-carve`
overrides `carve = true` in the file and `--uuid` overrides `uuid = false`; with neither flag of a
pair given, the file's value stands, and giving both is rejected by argument parsing (exit status
`2`). `diff` resolves each of its two specs this way on its own: absent `--config`, each side
discovers the `spargen.toml` beside it, while `--config` applies its one file to both sides.

### `spargen.toml` reference

The same file is read by `build.rs` (`Spec::config_file`, `Spec::discover_config_file`), by
`generate_api!` (always, from beside the spec), and by these subcommands. Every key is optional:

```toml
uuid = true             # default true; map `format: uuid` to `uuid::Uuid`
datetime = "time"       # default "time"; the crate `format: date-time`/`date` map to: "time",
                        # "jiff", "chrono", or "string" for none
carve = false           # default false; auto-carve unsupported constructs
batch_cap = 100         # default 100; diagnostics collected before the report is truncated
error_body_cap = 65536  # default 64 KiB
open_narrowing = false  # default false; open a response body's `enum`/`const` narrowings
open_unions = false     # default false; give a discriminated union an `Unknown { tag, value }` variant
verbatim_docs = false   # default false; carry the spec's prose into rustdoc as written
testing = false         # default false; emit the `testing` module and its typed `MockBackend`
bind_headers = []       # default none; header parameters bound once on the client
```

`error_body_cap` sets the generated client's `ClientConfig::max_error_body`, whose emitted doc
([source](https://github.com/getkono/spargen/blob/master/support-runtime/src/client.rs)) is the
one statement of what it bounds on each target and which paths it does not cap. `carve` is
described under [auto-carve](./compatibility.md#auto-carve), `open_narrowing` under
[problem details](./runtime.md#problem-details), `open_unions` on `Spec::with_open_unions` (a
discriminator value no variant lists decodes into the `Unknown` variant instead of failing; a
union with a 3.2 `defaultMapping` keeps its fallback), `verbatim_docs` on `Spec::with_verbatim_docs`
(by default the spec's `summary`, `description` and `title` text is rewritten so rustdoc has
nothing to report at the `include!` site: an intra-doc link such as ``[`crate::serve`]`` becomes
its text, a bare URL is wrapped in `<…>`, and an HTML tag nothing closes has its `<` escaped),
`testing` under [testing with a mock backend](./runtime.md#testing-with-a-mock-backend),
`bind_headers` on `Spec::bind_header` (each named header parameter, matched in any letter case,
is no argument of any operation; the generated `with_bound_header(name, value)` sets the value
every operation declaring it sends, refusing one that is not a valid header value, and a name no operation declares is `E026`), and the `[[omit]]` and `[[keep]]` rules the file may
also carry under [keep rules](./compatibility.md#keep-rules-allow-list) and [`spargen.toml` and the analysis CLI](./compatibility.md#spargentoml-and-the-analysis-cli).
An unknown key is a configuration error (exit status `3`).

### Exit status

| Status | Meaning |
| --- | --- |
| `0` | Success, including a run that reported only warnings, and `--help` / `--version`. |
| `1` | An error-severity diagnostic was reported (`check`, `deps`, `lock`, or a `diff` spec that cannot lower), `diff --exit-code` found a breaking (`major`) change, or output could not be written for a reason other than a closed stream. |
| `2` | The command line did not parse: an unknown subcommand or flag, a missing argument, or two conflicting flags. |
| `3` | A configuration or flag value was rejected (a missing `--config` target, a malformed `spargen.toml`, bad `--omit-*` or `--keep-operation` syntax), or `explain` was given an unknown code. |

A closed stdout or stderr (`spargen diff old.yaml new.yaml --format json | head -1`) is no failure:
what would have gone to it is dropped, and the command exits with the status its outcome maps to.

## `spargen check`

Audit a vendored spec with the same frontend used during generation, without emitting code.

```bash
spargen check <SPEC> [OPTIONS]
```

It accepts `--format <human|json>` plus the shared spec options above. A diagnostic whose location
is known renders as `--> <file>:<line>:<col>` in human output, naming the file it lies in, a
`$ref`'d file included, by the path the spec's references resolved to; under `--format json` the
same path is the diagnostic's `file`, beside its `span`. When spawned from a build script, whose
environment it inherits, it also audits the consuming package's `Cargo.toml` (`E023`); elsewhere
there is no consuming package, and `spargen deps` covers that ground instead.

From Rust, `spargen::check(&spec)` runs the same check in-process, with the same audit from a build
script. `spargen::check_with(&spec, CargoIntegration::Off)` skips that audit and the
`cargo:rerun-if-changed` lines, for a build script whose package is not the client's consumer (a
CI gate crate checking a spec another crate generates from); `CargoIntegration::Required` turns a
run outside a build script into `E024`, as it does for `generate`.

## `spargen deps`

Print the exact `[dependencies]` block generated output from this spec needs. Generated output is
freestanding, so the consuming package declares its own runtime dependencies — and which ones
depends on the API: `format: uuid` pulls in `uuid`, sequential responses and binary `_stream`
twins pull in `futures-core` and a `reqwest` feature. This answers that up front rather than one `E023` at a
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
same set structurally. A spec that lowers with warnings still prints the block on stdout, and
prints its report (outcome `clean`, with those warnings) on stderr, in the same format.

The block is written for the package's own tables. A workspace member may inherit its entries from
`[workspace.dependencies]` instead; the
[runtime dependency contract](getting-started.md#workspace-inheritance-and-the-full-contract)
states how the audit reads that.

## `spargen lock`

Fetch, vendor, and hash-pin remote `$ref`s into `spargen.lock`. This is the **only** networked
step; compilation-time generation and `check` never reach the network.

```bash
spargen lock <SPEC> [OPTIONS]
```

It accepts `--format <human|json>` plus the [shared spec options](#commands-and-shared-options).
The configuration is resolved in full, so a malformed `spargen.toml` or a bad flag value exits `3`
here as well.

Commit `.spargen/vendor/`, `spargen.lock`, the root document, and all relative source files.

A fetch follows up to ten redirects, refuses one that leaves `https` for plain `http`, and refuses
a document over 64 MiB. Nothing is written until every fetch has ended: then each vendored copy
is written to a temporary file beside it before any replaces the file it updates, and
`spargen.lock` is renamed into place last, so a file that cannot be written (a blocked directory,
a full disk) leaves the earlier lock and copies as they were.

A fetch that fails (an error status, a refused connection, a refused redirect, a body over the
limit) is reported as `E025` and fails the
command, but what it leaves on disk is still a usable lock. Every document it did fetch is
re-vendored and re-pinned at what the server served. A document it could not fetch, and any
document reachable only through it, keeps the pin the earlier `spargen.lock` held, beside the
vendored copy the earlier run wrote, so `check` and builds keep resolving it offline. A pin this
run did not refresh stays until a `spargen lock` in which every fetch succeeds rewrites the lock
from scratch.

### TLS

`spargen lock` fetches over reqwest's rustls stack with rustls's defaults and the bundled Mozilla
roots, and spargen holds no TLS position of its own: it interoperates with exactly the peers that
rustls release accepts, and refuses the rest as `E025` with rustls's reason. Protocol conformance
is rustls's, so a rustls upgrade may refuse a non-conformant server that an older one accepted.
The one behaviour pinned here is the strictness an advisory named:
[`spargen/tests/tls_strictness.rs`](https://github.com/getkono/spargen/blob/master/spargen/tests/tls_strictness.rs)
drives a deliberately non-conformant peer (RUSTSEC-2026-0285) and fails on a rustls that accepts
it again.

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
`0` whatever its bump. A spec that cannot lower is always a hard error and exits `1`. When either
spec lowers with warnings, the comparison is printed on stdout as usual and those warnings go to
stderr in the same format: each warning side's report under an `old spec:` / `new spec:` heading,
or, under `--format json`, one object keyed `old` and `new`.

Each side is compared as generated under its own settings, so a `uuid`, `datetime` or `testing`
setting that only one side's config file changes is classified like a spec change: every member it
retypes is a type change (`major`), and the `testing` module appearing is `minor`, disappearing
`major`.

`--format json` prints one object: `changes`, an array of `{ kind, impact, location, detail }`
sorted most-severe first, plus the top-level `bump`. `kind` is the stable kebab-case change code
(`operation-added`, `method-renamed`, …) and `impact`/`bump` are the lowercase semver labels
`major` / `minor` / `patch` — the same spellings the human rendering prints, so a script and a
reader never disagree. The set of `kind` codes grows with new change classes, so match on the
codes a caller knows and treat an unrecognized one as its stated `impact`.
