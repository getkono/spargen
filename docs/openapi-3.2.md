# OpenAPI 3.2 scope

OpenAPI 3.2 is a focused extension of 3.1, not a new schema generation model. It keeps JSON
Schema 2020-12 and the `https://spec.openapis.org/oas/3.1/dialect/base` dialect URI. Spargen
therefore validates 3.1 and 3.2 with their version-specific official document schemas, then lowers
both through the same typed frontend and version-neutral IR.

The client-relevant differences between the repository references
[`3.1.2`](../references/3.1.2.md) and [`3.2.0`](../references/3.2.0.md) are deliberately small:

| 3.2 area | Spargen disposition |
| --- | --- |
| Root `$self` | Supported as the document's canonical identity. It establishes the document's base URI, so relative references resolve against `$self` rather than against the path the file was read from; a relative `$self` is itself resolved against that retrieval path. Remote retrieval remains hermetic through `spargen.lock`. |
| `QUERY` and Path Item `additionalOperations` | Supported. Each valid custom method produces a generated client method and uses the exact HTTP method token on the wire. |
| Whole-query-string parameters and cookie style | `in: querystring` supports JSON and `application/x-www-form-urlencoded`; it is mutually exclusive with ordinary query parameters. `in: cookie, style: cookie` uses semicolon-separated pairs without percent encoding. |
| Reusable media types and sequential media | `components.mediaTypes` references are supported. `itemSchema` produces typed streams for SSE, JSON Lines/NDJSON, and RFC 7464 JSON Text Sequences. SSE is parsed into the standard `data`/`event`/`id`/`retry` event object before item deserialization. When the envelope's `data` is a string annotated with `contentMediaType: application/json` and `contentSchema`, the stream item is the declared JSON payload type directly; those content annotations remain `W001` outside this recognized wire position. |
| Metadata additions | Info `summary`, Server `name`, response `summary` and optional `description`, operation tags, and tag `summary`/`parent`/`kind` are retained as generated rustdoc. Missing/cyclic tag parents are rejected. |
| XML `nodeType` | `attribute` and `element` map to the existing XML representation, with the specification's own defaulting from `attribute`/`wrapped`. `text`, `cdata`, and `none` change the XML on the wire, so on a type actually serialized as XML they are rejected (`E009`); on a type never serialized as XML they genuinely have no effect and are ignored (`W006`). Declaring `attribute` or `wrapped` beside `nodeType` is contradictory and rejected (`E011`). |
| OAuth device/metadata descriptions | Accepted as scheme documentation. As with 3.1 OAuth flows, generated clients use a caller-supplied bearer token; spargen does not implement token acquisition. |
| Examples | The new `dataValue`/`serializedValue` forms are accepted as documentation-only fields and do not affect the generated wire implementation. |
| Complete sequential schemas | A sequential-media `schema` describes the complete sequence, while `itemSchema` describes a stream item. Spargen currently exposes the streaming shape only, so a 3.2 sequence with only `schema` is rejected (`E009`) rather than miscompiled. |
| Explicit media encodings | `encoding` supported; `prefixEncoding`/`itemEncoding` not. The Media Type Object's `encoding` drives per-property serialization under the specification's mode switch: any explicit `style`/`explode`/`allowReserved` selects RFC 6570 query-style serialization and makes `contentType` inert; otherwise the property is rendered by its `contentType`, explicit or defaulted per the version's table. 3.1 and 3.2 differ only for arrays — 3.1 follows the item type, 3.2 simplified this to JSON — and spargen applies the 3.2 rule to both versions rather than branching: the two agree that an array of objects is JSON, and JSON is the only self-consistent reading for a nested array. Multipart parts therefore carry a resolved `Content-Type`, and a form-urlencoded body is built byte for byte. `encoding.headers` honors a Header Object's `const` then `default`; a Header Object pins no value of its own, so one that declares neither leaves a client nothing to send and is reported as having no effect (`W011`). 3.2's `prefixEncoding`/`itemEncoding` describe the positional parts of an *array-shaped* multipart body; spargen generates `multipart/form-data` from an object schema, which has no positions, so on multipart both are rejected (`E009`) and on `application/x-www-form-urlencoded` — where the specification scopes them to multipart anyway — both are reported as having no effect (`W011`). `encoding` on any media that is neither multipart nor form-urlencoded is likewise acknowledged as inert (`W011`) wherever it appears: request body, response, parameter content, header content, or a `components.mediaTypes` entry. The specification requires implementations to support one level of *nested* `encoding`, which describes a `multipart/mixed` part inside a part; spargen generates one flat level and rejects nesting (`E009`) rather than emitting a body it cannot build. |
| Discriminator fallback | Supported. `defaultMapping` names the branch that absent or unknown discriminator values decode into, and spargen generates exactly that fallback. A discriminated union whose `defaultMapping` names no declared branch is still `E007`. |
| External Security Scheme URI | Supported. A Security Requirement key that is a URI resolves with the specification's component-name precedence — a matching `components.securitySchemes` name wins, otherwise the URI is retrieved through the same hermetic, hash-pinned vendoring as any other reference. A URI that resolves to nothing is `E012`. |
| `mutualTLS` | Accepted and always satisfiable: the requirement is met by the transport's client certificate, which the caller configures on the injected `reqwest::Client`, so spargen attaches nothing for it (`W011`). |
| Parameter `allowReserved` beyond query | 3.2 permits `allowReserved` on header and cookie parameters, where those are never percent-encoded — so it is accepted and reported as having no effect (`W011`) rather than silently dropped. In 3.1 the official schema scopes it to query, so the same document is `E011` there. |

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
