# OpenAPI 3.2 scope

OpenAPI 3.2 is a focused extension of 3.1, not a new schema generation model. It keeps JSON
Schema 2020-12 and the `https://spec.openapis.org/oas/3.1/dialect/base` dialect URI. Spargen
therefore validates 3.1 and 3.2 with their version-specific official document schemas, then lowers
both through the same typed frontend and version-neutral IR.

The client-relevant differences between the repository references
[`3.1.2`](../references/3.1.2.md) and [`3.2.0`](../references/3.2.0.md) are deliberately small:

| 3.2 area | Spargen disposition |
| --- | --- |
| Root `$self` | Supported as the document's canonical identity and base for references back to the root. Relative file references continue to resolve from the file that contains them. Remote retrieval remains hermetic through `spargen.lock`. |
| `QUERY` and Path Item `additionalOperations` | Supported. Each valid custom method produces a generated client method and uses the exact HTTP method token on the wire. |
| Whole-query-string parameters and cookie style | `in: querystring` supports JSON and `application/x-www-form-urlencoded`; it is mutually exclusive with ordinary query parameters. `in: cookie, style: cookie` uses semicolon-separated pairs without percent encoding. |
| Reusable media types and sequential media | `components.mediaTypes` references are supported. `itemSchema` produces typed streams for SSE, JSON Lines/NDJSON, and RFC 7464 JSON Text Sequences. SSE is parsed into the standard `data`/`event`/`id`/`retry` event object before item deserialization. |
| Metadata additions | Info `summary`, Server `name`, response `summary` and optional `description`, operation tags, and tag `summary`/`parent`/`kind` are retained as generated rustdoc. Missing/cyclic tag parents are rejected. |
| XML `nodeType` | `attribute` and `element` map to the existing XML representation. `text`, `cdata`, and `none` are reported as unsupported XML hints (`W006`). |
| OAuth device/metadata descriptions | Accepted as scheme documentation. As with 3.1 OAuth flows, generated clients use a caller-supplied bearer token; spargen does not implement token acquisition. |
| Examples | The new `dataValue`/`serializedValue` forms are accepted as documentation-only fields and do not affect the generated wire implementation. |
| Complete sequential schemas | A sequential-media `schema` describes the complete sequence, while `itemSchema` describes a stream item. Spargen currently exposes the streaming shape only, so a 3.2 sequence with only `schema` is rejected (`E009`) rather than miscompiled. |
| Explicit media encodings | Media Type Object `encoding`, `prefixEncoding`, and `itemEncoding` require additional by-name/by-position wire codecs and are rejected (`E009`) rather than ignored. |
| Discriminator fallback | `defaultMapping` requires a generated fallback branch for absent or unknown discriminator values and is rejected (`E007`) until that branch is representable. |
| External Security Scheme URI | Security requirements that use a URI instead of a local component name are rejected (`E012`); only declared local schemes can be connected to caller credentials. |

Everything else continues under the 3.1 contract in the [support matrix](support-matrix.md). This
includes the important distinction between validation-only JSON Schema keywords (`W001`) and
shape-changing keywords: nested shape-changing constructs are still traversed, so an unsupported
dynamic reference cannot hide beneath a conditional or other validation keyword.

## Structural validation provenance

The generator embeds date-pinned official OpenAPI schemas and never fetches them while checking or
generating:

- OpenAPI 3.1: `https://spec.openapis.org/oas/3.1/schema/2025-09-15`
- OpenAPI 3.2: `https://spec.openapis.org/oas/3.2/schema/2025-09-17`

Their source URLs and SHA-256 digests are recorded beside the vendored files under
`spargen/src/oas31/spec/`. Version-specific rules therefore remain distinct—for example, a 3.1
Response Object still requires `description`, while 3.2 permits it to be absent.
