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
