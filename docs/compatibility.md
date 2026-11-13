# Compatibility Omit Mode

Spargen normally treats the input schema as authoritative: unsupported constructs reject generation.
Compatibility omit mode is an explicit preprocessor for vendored upstream specs when one operation,
path, component, or pointer is outside Spargen's current support surface.

Rules:

- Omit profiles never modify the source schema on disk.
- Omit rules are exact by default, or **glob** (bulk) when the value carries a metacharacter (see
  [Globbing](#globbing-bulk-omits) below).
- Every rule must match at least one construct, or generation fails with `E019` (an exact rule that
  matches nothing, or a glob rule that matches nothing).
- Every omitted construct emits `W009` — one per removed construct, so a bulk glob rule reports each.
- If the remaining document is structurally invalid, generation fails with `E020`.
- Dangling `$ref`s remain errors; omit dependent consumers too (or use [auto-carve](#auto-carve)).
- Generated provenance includes an omit profile fingerprint.

## Globbing (bulk omits)

A `path`, operation `path`, component `name`, or `pointer` value that contains a glob
metacharacter is matched as a glob and removes **every** matching construct (a bulk omit); a value
with no metacharacter is an exact rule. The matcher is `/`-aware:

| Token  | Matches                                                          |
| ------ | --------------------------------------------------------------- |
| `*`    | zero or more characters within a single segment (never a `/`)   |
| `**`   | zero or more characters across any depth (including `/`)         |
| `?`    | exactly one character other than `/`                             |
| `\*`, `\?` | that character, literally — the value stays an exact rule   |

A backslash escapes a metacharacter, because a URI path may legitimately contain one (RFC 3986
lists `*` as a sub-delimiter). `path = "/files/\*"` removes exactly the path named `/files/*`,
while `path = "/files/*"` removes every path under `/files/`. A backslash escapes whatever
character follows it, in exact and glob rules alike: `\b` names `b`, and a literal backslash in a
path is written `\\`. Rules that [auto-carve](#auto-carve)
derives from a document are escaped for you, so carving one operation never widens into its
siblings.

```rust
let spec = spargen::Spec::new("api/openapi.yaml").omit(spargen::omit! {
    paths { "/admin/**"; }
    operations { get "/internal/*"; }
    components { schemas { "Legacy*"; } }
});
```

## Keep rules (allow-list)

To generate a client for a few operations of a large description, name the operations to keep
rather than omitting everything else:

```rust
use spargen::OmitMethod;

let spec = spargen::Spec::new("api/openapi.yaml")
    .keep_operation(OmitMethod::Get, "/v1/sync")
    .keep_operation(OmitMethod::Get, "/v1/blob/{address}")
    .keep_operation(OmitMethod::Post, "/v1/federation/capabilities/refresh");
```

Once any keep rule is present:

- every operation no keep rule names is omitted: a `paths` entry with no kept operation is removed
  whole, the other methods of an entry are removed one by one, and so are its OpenAPI 3.2
  `additionalOperations`, which no keep rule names;
- the `webhooks` are omitted;
- every component of the root document that nothing left in the document reaches is omitted. Reach
  follows `$ref`s (across files), `discriminator` mapping values, and the scheme names of security
  requirements, through every component it reaches; a component is kept or removed whole;
- each removed construct is reported via `W009`, as an omit rule's are.

A keep rule's path is read as an operation omit rule's is: exact, or a glob when it carries a
metacharacter. Keep rules are resolved against the document as written, before any omit rule
applies: a keep rule that names no declared operation (an exact one that operation, a glob one at
least one) fails generation with `E019`, rather than passing silently. Omit rules still apply on
top, so a kept operation an omit rule also removes is omitted, with no `E019` for the keep rule.

A `paths` entry written as a `$ref` is kept **whole** once one of its target's operations is kept,
because the target can be shared by other entries; a keep rule names such an operation by the
`paths` key that refers to it.

Keep rules are part of the `Omit` profile and its fingerprint: `Spec::omit` appends a profile's keep
rules as it appends its omit rules, and `Spec::clear_omissions` removes both. In `spargen.toml` each
is a `[[keep]]` entry with both keys required:

```toml
[[keep]]
method = "get"
path = "/v1/sync"
```

On the command line it is the repeatable `--keep-operation "get /v1/sync"`, and in `generate_api!`
a `keep { get "/v1/sync"; }` block. Each adds to the keep rules the file already holds.

## Auto-carve

`Spec::with_carve(Carve::Auto)` (or the macro's `carve` argument) turns a spec that would **reject**
into a generate-what-you-can outcome. Instead of failing on rejections, spargen:

1. runs the frontend audit;
2. maps each error diagnostic's JSON pointer to the smallest enclosing **omittable** construct — a
   pointer under `/paths/<path>/<method>/…` carves that operation, one at the path-item level carves
   the path, one under `/components/<kind>/<name>/…` carves that component. A pointer is read in
   the file it was reported in, so a rejection inside a referenced sub-file is carved there:
   - the same constructs in a sub-file are carved as a file-scoped pointer rule
     (`lib.yaml#/components/schemas/Node`, the file named relative to the root document), and the
     `$ref`s left dangling by it are carved on the next round;
   - in a bare path item file (one a `$ref` reaches where a Path Item belongs), a rejection inside
     one of its methods carves that operation alone, as a file-scoped pointer rule
     (`pi.yaml#/get`), so its healthy sibling methods survive;
   - anything else in a sub-file with no `paths` or `components` of its own — a bare schema file,
     or a path item file's own `parameters` — is carved at every construct whose `$ref` reaches
     the rejected part of it, followed back through any intermediate files to the root document;
3. adds those omit rules and re-runs, **iterating to a fixpoint** (omitting one construct can clear
   some rejections and surface others — e.g. a now-dangling `$ref`) until the frontend is clean or a
   round makes no progress. The number of rounds is bounded, so it always terminates.

Every carved construct is reported via `W009`, so you see exactly what was dropped — carving is
never silent. If some rejections cannot be carved (they enclose no omittable construct — the
document root, an unmodelled component kind, …), spargen reports those residual errors honestly and
does **not** emit partial/broken output. The carve set is deterministic: the same spec always carves
the same constructs in the same order.

Auto-carve is a pragmatic escape hatch (bring a large upstream spec online quickly, then narrow the
gaps). For a committed, reviewed subset, prefer explicit omit rules.

Library API:

```rust
let spec = spargen::Spec::new("api/openapi.yaml").omit(spargen::omit! {
    operations {
        post "/repos/{owner}/{repo}/releases/{release_id}/assets";
    }

    paths {
        "/octocat";
    }

    components {
        schemas { "legacy-schema"; }
        request_bodies { "legacy-body"; }
    }

    pointers {
        "/paths/~1legacy/get/responses/200";
    }

    file("schemas/legacy.yaml") {
        pointers {
            "/properties/unsupported";
        }
    }
});
```

Use omit profiles as reviewed compatibility code. Do not generate them automatically in production;
developer tooling may suggest rules, but committed profiles should be explicit and stale-rule
failures should be fixed promptly.

## `spargen.toml` and the analysis CLI

The analysis subcommands that read a spec — `check`, `deps`, `lock`, and `diff` — apply the
settings in `spargen.toml` and the shared flags while they work. None of them generates code; put
settings both the build and these commands must agree on in `spargen.toml`.

`spargen.toml` is auto-discovered beside the spec (`--config <path>` overrides the location). The
CLI reference's
[`spargen.toml` reference](https://github.com/getkono/spargen/blob/master/docs/book/src/cli.md#spargentoml-reference)
lists its other keys; this page covers its `[[omit]]` rules. Their kinds are discriminated by
**field presence** (TOML has no enums), and an entry that also sets a key its kind does not use —
`file` or `name` beside `path`, `path`, `method` or `file` beside `component`, `name` beside
`pointer` — is a config error naming the key and the kind, not a key silently dropped:

```toml
[[omit]]
path = "/pets/{id}"                     # → OmitRule::Path (exact)

[[omit]]
path = "/admin/**"                      # → OmitRule::Path (glob: bulk removal)

[[omit]]
method = "get"                          # method + path → OmitRule::Operation
path = "/pets"

[[omit]]
component = "schemas"                   # component + name → OmitRule::Component
name = "LegacyPet"                      #   schemas / responses / parameters / request_bodies / headers / security_schemes / path_items / media_types

[[omit]]
pointer = "/components/schemas/X"       # pointer → OmitRule::Pointer
file = "extra.yaml"                     #   file optional (file-local pointer)
```

A pointer rule's `file` — in `spargen.toml`, an `--omit-pointer` flag, `OmitRule::pointer`, or the
macro's `file("…")` — is relative to the root document's directory, as the root's own `$ref`s are.
It names a loaded document by its path exactly as loaded, else by its path relative to the root
document's directory, else by the trailing whole path components exactly one loaded document ends
in: `pet.yaml` reaches `schemas/pet.yaml`, never `schemas/legacy-pet.yaml`. That last step is the
only one two documents can meet at — `a/pet.yaml` and `b/pet.yaml`, or a local `schemas/pet.yaml`
and a vendored copy under `.spargen/vendor/<host>/…/pet.yaml` — and then the rule is `E019`
naming each candidate, never whichever loaded first, so name a file relative to the root document
to be sure which one a rule reaches. The rules auto-carve derives are written that way, so the omit
fingerprint they stamp into generated output does not depend on where the checkout lives — except
for a document the description reaches by an absolute-path `$ref`, which lies outside the root
document's directory and is named by that absolute path instead. The description itself fixes that
location, so moving the checkout already breaks such a `$ref`.

Equivalent repeatable CLI flags (unioned with any config-file omit rules):

```
spargen check spec.yaml \
  --omit-path "/pets/{id}" \
  --omit-operation "get /pets" \
  --omit-component "schemas:LegacyPet" \
  --omit-pointer "extra.yaml#/components/schemas/X"   # or "/pointer" for the root document
```

The CLI reference's
[Flags and `spargen.toml`](https://github.com/getkono/spargen/blob/master/docs/book/src/cli.md#flags-and-spargentoml)
states how the flags and the file combine, and its
[exit status](https://github.com/getkono/spargen/blob/master/docs/book/src/cli.md#exit-status)
table lists what a rejected configuration or flag value exits with.

The same file is available to the generation APIs — `Spec::discover_config_file()` from `build.rs`,
and automatically beside the spec for `generate_api!` — because it is parsed by the library, not by
the CLI. Setters called after it still win, so build code remains authoritative:

```rust
let spec = spargen::Spec::new("api/openapi.yaml")
    .discover_config_file()?
    .with_carve(spargen::Carve::Off);   // overrides the file
```
