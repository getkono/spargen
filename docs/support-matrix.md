# Support Matrix

This is the operational support matrix for the current 3.1 implementation slice. Unsupported
constructs fail loudly — a diagnostic and no output — rather than degrading silently to
`serde_json::Value`. Where a construct does map to an untyped value it is either faithful (the
schema was itself untyped) or reported with a warning; degradation is never silent.

| Area | Supported | Warned | Rejected |
| --- | --- | --- | --- |
| Version | OpenAPI `3.1.x` | - | OpenAPI `3.0.x`/`3.2.x`/other versions (`E001`) |
| Dialect | OAS 3.1 base dialect or omitted | - | Other `jsonSchemaDialect` values (`E002`) |
| References | Local/internal component refs used by the frontend | - | Absolute URL refs (`E003`), unresolved refs (`E004`) |
| Schema shape | objects, arrays, tuples, maps, scalar primitives, homogeneous scalar enums, untyped schemas, recursive `$ref` cycles (self- and mutually-recursive; cycle-closing references are boxed); `readOnly`/`writeOnly`/`deprecated`/`title`/`description` annotations surface as rustdoc / `#[deprecated]` | validation-only keywords (`W001`); `default` values ignored (no deserialization defaults) | `patternProperties` (`E005`), dynamic refs (`E006`), all `oneOf`/`anyOf` unions — with or without a discriminator (`E007`), non-scalar enums (`E008`), `allOf` composition (`E013`) |
| Media | `application/json`, `application/x-www-form-urlencoded`, `application/octet-stream`, `text/plain` | - | Other request/response media (`E009`) |
| Parameters | path/query/header/cookie with simple/form styles; JSON content params | examples ignored | unsupported styles (`E010`) |
| Responses | single success body typed; `204`/bodyless → `()`; no documented error body → an uninhabited error type, with undocumented statuses surfaced as `UnexpectedStatus` | multiple documented success or error bodies degrade to `serde_json::Value` (`W003`) | - |
| Security | `http` bearer/basic and `apiKey` (header/query/cookie) attach registered credentials per operation `security` (first satisfiable alternative; a missing credential is a request-construction error); `oauth2`/`openIdConnect` accept a caller-supplied token attached as a bearer | - | a requirement naming an undeclared or unsupported scheme (`E012`) |
| Document | servers and path/operation metadata lowered | `webhooks`, operation `callbacks`, and response `links` acknowledged (`W002`), no code emitted | - |
| Drift | - | `generate --check` reports checked-in output that drifted or is missing (`W004`) | - |
| Compatibility | exact omit rules for paths, operations, components, pointers, file-local pointers | matched omissions (`W009`) | unmatched/invalid omit rules (`E019`), invalid post-omit document (`E020`) |

Generated output is freestanding Rust with embedded support code and no `spargen` runtime
dependency.
