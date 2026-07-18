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

```toml
[[omit]]
path = "/admin/**"                     # every path under /admin (bulk)
[[omit]]
component = "schema"                    # every schema named Legacy… (bulk)
name = "Legacy*"
```

```
spargen generate spec.yaml --out src/api.rs \
  --omit-path "/admin/**" \
  --omit-operation "get /internal/*" \
  --omit-component "schema:Legacy*"
```

## Auto-carve

`--carve` (CLI) or `carve = true` under `[features]` in `spargen.toml` (library:
`Config { carve: true, .. }`) turns a spec that would **reject** into a generate-what-you-can
outcome — the "generate every spec" escape hatch. Instead of failing on rejections, spargen:

1. runs the frontend audit;
2. maps each error diagnostic's JSON pointer to the smallest enclosing **omittable** construct — a
   pointer under `/paths/<path>/<method>/…` carves that operation, one at the path-item level carves
   the path, one under `/components/<kind>/<name>/…` carves that component;
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
let mut config = spargen::Config::new(
    "api/openapi.yaml",
    spargen::OutputTarget::Module("src/api.rs".into()),
);

config.omit = spargen::omit! {
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
};
```

Use omit profiles as reviewed compatibility code. Do not generate them automatically in production;
developer tooling may suggest rules, but committed profiles should be explicit and stale-rule
failures should be fixed promptly.

## Config file (`spargen.toml`) and CLI omit surface

The CLI (`spargen generate` / `spargen check`) can build the same `Config` — features, caps, and
omit rules — from a `spargen.toml` file and/or repeatable flags, with no `build.rs`.

`spargen.toml` is auto-discovered beside the spec (`--config <path>` overrides the location). Its
schema mirrors `Config`; omit-rule kinds are discriminated by **field presence** (TOML has no
enums):

```toml
[features]
uuid = true             # default true; false ≡ --no-uuid
time = true             # default true; false ≡ --no-time
error_body_cap = 65536  # optional (default 65536)
batch_cap = 100         # optional (default 100)
as_crate = false        # optional; generate a standalone crate instead of a module
carve = false           # optional; auto-carve unsupported constructs (same as --carve)

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

Equivalent repeatable CLI flags (unioned with any config-file omit rules):

```
spargen generate spec.yaml --out src/api.rs \
  --omit-path "/pets/{id}" \
  --omit-operation "get /pets" \
  --omit-component "schema:LegacyPet" \
  --omit-pointer "extra.yaml#/components/schemas/X"   # or "/pointer" for the root document
```

**Precedence (low → high): built-in defaults < `spargen.toml` < CLI flags.** A missing
auto-discovered config file is fine (defaults apply); a missing `--config` target, a malformed
config file, or bad omit-flag syntax is a clear error with a non-zero (usage) exit — never a panic.
The library `Config` API is unchanged; this is CLI-level plumbing.
