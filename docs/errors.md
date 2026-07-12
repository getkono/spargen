# Diagnostic Index

Stable diagnostics are product surface. `spargen explain E###` returns the same explanation used by
the library API.

| Code | Severity | Title |
| --- | --- | --- |
| `E001` | Error | unsupported OpenAPI version |
| `E002` | Error | unsupported JSON Schema dialect |
| `E003` | Error | absolute `$ref` unsupported |
| `E004` | Error | unresolved `$ref` |
| `E005` | Error | `patternProperties` not representable as a typed map |
| `E006` | Error | dynamic reference unsupported |
| `E007` | Error | union variants are not disjoint |
| `E008` | Error | enum values are not homogeneous scalars |
| `E009` | Error | unsupported media type |
| `E010` | Error | unsupported parameter style |
| `E011` | Error | invalid input document |
| `E012` | Error | unknown security scheme |
| `E013` | Error | `allOf` composition unsupported |
| `E019` | Error | invalid omit rule |
| `E020` | Error | omit profile created an invalid document |
| `W001` | Warning | validation-only keyword ignored |
| `W002` | Warning | server-initiated flow ignored |
| `W003` | Warning | response body degrades to `serde_json::Value` |
| `W004` | Warning | checked-in output drifted |
| `W005` | Warning | schema default not applied |
| `W009` | Warning | construct omitted |
