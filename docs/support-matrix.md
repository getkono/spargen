# Support Matrix

This is the operational support matrix for the current 3.1 implementation slice. The PRD remains
the target contract; unsupported rows fail loudly rather than degrading to `serde_json::Value`
unless the schema itself is untyped.

| Area | Supported | Warned | Rejected |
| --- | --- | --- | --- |
| Version | OpenAPI `3.1.x` | - | OpenAPI `3.0.x`/`3.2.x`/other versions (`E001`) |
| Dialect | OAS 3.1 base dialect or omitted | - | Other `jsonSchemaDialect` values (`E002`) |
| References | Local/internal component refs used by the frontend | - | Absolute URL refs (`E003`), unresolved refs (`E004`) |
| Schema shape | objects, arrays, tuples, maps, scalar primitives, homogeneous scalar enums, untyped schemas | validation-only keywords (`W001`) | `patternProperties` (`E005`), dynamic refs (`E006`), unsupported unions (`E007`), non-scalar enums (`E008`) |
| Media | `application/json`, `application/x-www-form-urlencoded`, `application/octet-stream`, `text/plain` | - | Other request/response media (`E009`) |
| Parameters | path/query/header/cookie with simple/form styles; JSON content params | examples ignored | unsupported styles (`E010`) |
| Compatibility | exact omit rules for paths, operations, components, pointers, file-local pointers | matched omissions (`W009`) | unmatched/invalid omit rules (`E019`), invalid post-omit document (`E020`) |

Generated output is freestanding Rust with embedded support code and no `spargen` runtime
dependency.
