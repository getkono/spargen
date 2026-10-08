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
with no metacharacter is an exact rule and behaves exactly as before. The matcher is `/`-aware:

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

## Auto-carve

`Spec::carve(true)` (or the macro's `carve` argument) turns a spec that would **reject**
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

`spargen check` can apply batch, carve, and omit settings from `spargen.toml` and
repeatable flags while auditing a schema. This never generates code; keep the generation profile
in `build.rs` or the macro invocation.

`spargen.toml` is auto-discovered beside the spec (`--config <path>` overrides the location). Its
omit-rule kinds are discriminated by **field presence** (TOML has no enums):

```toml
uuid = true             # optional (default true); map `format: uuid` to `uuid::Uuid`
time = true             # optional (default true); map `format: date-time`/`date` to `time`
carve = false           # optional; auto-carve unsupported constructs
batch_cap = 100         # optional (default 100)
error_body_cap = 65536  # optional (default 64 KiB)
open_narrowing = false  # optional (default false); open a response body's string `enum`/`const` narrowings

[[omit]]
path = "/pets/{id}"                     # → OmitRule::Path (exact)

[[omit]]
path = "/admin/**"                      # → OmitRule::Path (glob: bulk removal)

[[omit]]
method = "get"                          # method + path → OmitRule::Operation
path = "/pets"

[[omit]]
component = "schema"                    # component + name → OmitRule::Component
name = "LegacyPet"                      #   schema / response / parameter / requestBody / header / securityScheme

[[omit]]
pointer = "/components/schemas/X"       # pointer → OmitRule::Pointer
file = "extra.yaml"                     #   file optional (file-local pointer)
```

A pointer rule's `file` names a loaded document by its path exactly as loaded, else by its path
relative to the root document's directory, else by suffix. The suffix step is the ambiguous one —
`lib.yaml` is a suffix of `xlib.yaml` — so name a file relative to the root document to be sure
which one a rule reaches. The rules auto-carve derives are written that way, so the omit
fingerprint they stamp into generated output does not depend on where the checkout lives — except
for a document the description reaches by an absolute-path `$ref`, which lies outside the root
document's directory and is named by that absolute path instead. The description itself fixes that
location, so moving the checkout already breaks such a `$ref`.

`error_body_cap` sets the generated client's `ClientConfig::max_error_body`, whose emitted doc
([source](https://github.com/getkono/spargen/blob/master/support-runtime/src/client.rs)) is the
one statement of what it bounds on each target and which paths it does not cap. On native targets
an over-cap error body is abandoned partway, which forgoes reuse of that connection, so a cap set
far below the error bodies an API actually returns trades connection reuse for a smaller retained
prefix.

Equivalent repeatable CLI flags (unioned with any config-file omit rules):

```
spargen check spec.yaml \
  --omit-path "/pets/{id}" \
  --omit-operation "get /pets" \
  --omit-component "schema:LegacyPet" \
  --omit-pointer "extra.yaml#/components/schemas/X"   # or "/pointer" for the root document
```

**Precedence (low → high): built-in defaults < `spargen.toml` < CLI flags.** A missing
auto-discovered config file is fine (defaults apply); a missing `--config` target, a malformed
config file, or bad omit-flag syntax is a clear error with a non-zero (usage) exit — never a panic.

The same file is available to the generation APIs — `Spec::discover_config_file()` from `build.rs`,
and automatically beside the spec for `generate_api!` — because it is parsed by the library, not by
the CLI. Setters called after it still win, so build code remains authoritative:

```rust
let spec = spargen::Spec::new("api/openapi.yaml")
    .discover_config_file()?
    .carve(false);   // overrides the file
```
