# Framework round-trip recipe specs

These specs back the framework round-trip recipes in [`docs/recipes.md`](../../docs/recipes.md)
and the compatibility test `spargen/tests/recipes.rs`. Each one mirrors the OpenAPI document a
Rust server framework EMITS, so the test proves spargen actually consumes that framework's output
idioms (the round-trip `Rust server → OpenAPI → spargen client`).

They are **hand-crafted** (small, reviewable, deterministic) rather than captured from a running
server, but each faithfully reproduces the framework's emitted structure and, crucially, the
**OpenAPI version** that framework emits — verified against the framework's own source, pinned
below. The test reads only these local files; it never hits the network.

| Spec | Framework | Emits | Expected | Verified against |
| --- | --- | --- | --- | --- |
| `utoipa.json` | [`juhaku/utoipa`](https://github.com/juhaku/utoipa) 5.x | OpenAPI `3.1.0` | generate (clean) | `OpenApiVersion::Version31` (`#[serde(rename = "3.1.0")]`, `#[default]`) in `utoipa/src/openapi.rs`; exported via `ApiDoc::openapi().to_pretty_json()` |
| `aide.json` | [`tamasfe/aide`](https://github.com/tamasfe/aide) | OpenAPI `3.1.0` | generate (only `W001`) | `serde_version` serializes `"3.1.0"` in `crates/aide/src/openapi/openapi.rs`; component schemas come from `schemars` (JSON Schema 2020-12) |
| `poem-openapi.json` | [`poem-web/poem`](https://github.com/poem-web/poem) (`poem-openapi`) | OpenAPI `3.0.0` | reject `E001` | `const OPENAPI_VERSION: &str = "3.0.0"` in `poem-openapi/src/registry/ser.rs`; exported via `OpenApiService::spec()` |
| `utoipa-untagged-overlap.json` | `juhaku/utoipa` 5.x | OpenAPI `3.1.0` | generate (clean) | as `utoipa.json`; adds one `#[serde(untagged)]` numeric enum → typed trial-matching `oneOf` |

## Idioms exercised

- **`utoipa.json`** — 3.1 nullable primitive (`type: ["string","null"]`), nullable `$ref`
  (`oneOf` with a `{"type":"null"}` member), `allOf`-composed model, a `discriminator`-tagged
  `oneOf`, tag-grouped operations, path parameters, a multi-status response (`200` + `404`
  bodies), and an `http` bearer security scheme.
- **`aide.json`** — schemars-style nullables (`anyOf` with `{"type":"null"}` and
  `type: ["integer","null"]`), an externally-tagged enum (`oneOf` of closed
  `additionalProperties: false` objects with unique required keys, dispatched by content), a
  by-JSON-type disjoint `oneOf` (`string | integer`), an `allOf` `#[serde(flatten)]` composition,
  and a multi-status response. The `minimum`/`format` validation hints schemars emits are
  faithfully ignored (`W001`).
- **`poem-openapi.json`** — a 3.0.0 document (with 3.0-style `nullable: true`); rejected on the
  version alone.
- **`utoipa-untagged-overlap.json`** — one clean operation plus one operation whose response is an
  overlapping untagged union (`integer | number`); both operations generate, and the overlap is
  represented by a typed enum with exact-one trial matching.

## Reproducing / refreshing

Because these are crafted, there is no upstream file hash to pin. To confirm the emitted versions
haven't drifted, re-check the source constants cited above (they are the load-bearing facts). If a
framework changes the OpenAPI version it emits, update the matching spec, the recipe in
`docs/recipes.md`, and the expected outcome in `spargen/tests/recipes.rs` together.
