# Diagnostic Index

Stable diagnostics are product surface. `spargen explain E###` returns the same explanation used by
the library API.

| Code | Severity | Title |
| --- | --- | --- |
| `E001` | Error | unsupported OpenAPI version (3.1.x and 3.2.x are supported) |
| `E002` | Error | unsupported JSON Schema dialect |
| `E003` | Error | remote `$ref` not pinned |
| `E004` | Error | unresolved `$ref` |
| `E005` | Error | `patternProperties` not representable as a typed map |
| `E006` | Error | dynamic reference unsupported |
| `E007` | Error | union variants are not disjoint |
| `E008` | Error | enum values are not homogeneous scalars |
| `E009` | Error | unsupported media type |
| `E010` | Error | unsupported parameter style |
| `E011` | Error | invalid input document |
| `E012` | Error | unknown security scheme |
| `E013` | Error | irreconcilable `allOf` composition |
| `E014` | Error | schema nesting is too deep to lower |
| `E019` | Error | invalid omit rule |
| `E020` | Error | omit profile created an invalid document |
| `E021` | Error | vendored remote `$ref` drifted from lock |
| `E022` | Error | duplicate object key |
| `W001` | Warning | validation-only keyword ignored |
| `W002` | Warning | server-initiated flow ignored |
| `W004` | Warning | checked-in output drifted |
| `W005` | Warning | schema default not applied |
| `W006` | Warning | unsupported XML hint ignored |
| `W009` | Warning | construct omitted |
| `W010` | Warning | OpenAPI 3.2 construct ignored |
