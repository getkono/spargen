# Compatibility Omit Mode

Spargen normally treats the input schema as authoritative: unsupported constructs reject generation.
Compatibility omit mode is an explicit preprocessor for vendored upstream specs when one operation,
path, component, or pointer is outside Spargen's current support surface.

Rules:

- Omit profiles never modify the source schema on disk.
- Omit rules are exact. There is no globbing and no implicit cascading.
- Every rule must match at least one construct, or generation fails with `E019`.
- Every omitted construct emits `W009`.
- If the remaining document is structurally invalid, generation fails with `E020`.
- Dangling `$ref`s remain errors; omit dependent consumers too.
- Generated provenance includes an omit profile fingerprint.

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

[[omit]]
path = "/pets/{id}"                     # → OmitRule::Path

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
