# Runtime & Ergonomics

The runtime support code is embedded into generated output as a private `support` module — no
spargen crate ever enters a consumer's dependency graph. Its core dependency set is `reqwest`
(default features off) / `http` / `http-body` / `serde` / `serde_json` / `bytes` / `secrecy`;
`http` and `http-body` carry the [transport seam](#the-transport-seam)'s request, response and body
types. A capability that needs more
adds it only when the compiled API uses it, as this table lists; each crate's exact Cargo features
are in the contract linked below:

| Capability | Adds |
| --- | --- |
| `multipart/form-data` request bodies | reqwest's `multipart` feature |
| A raw `bytes` value serde encodes (a `format: binary` value in a model, a JSON `content` parameter, or a serde-encoded body; a [`contentEncoding: base64`](#binary-content) one carries its own codec) | `bytes`' `serde` feature |
| [Streaming](#streaming) (sequential responses) | `futures-core` and reqwest's `stream` feature |
| [XML bodies](#xml-bodies) | `quick-xml` |
| [`format: date-time` / `date`](#uuid-and-time) | `time` |
| [`format: uuid`](#uuid-and-time) | `uuid` |
| [Blocking client](#blocking-feature-blocking) | an optional, native-only `tokio`, behind the consumer's own `blocking` feature |

Beyond that core set, the transport seam, retry, middleware, pagination, problem details and
WebAssembly support add no dependency: no `tower`, no `async-trait`, and no async timer of its own. Std's `Future` / `Pin` /
`Box` carry the policy abstractions.

The exact tested version floors and conditional Cargo features are the
[runtime dependency contract](./getting-started.md#runtime-dependency-contract), and
`spargen deps <spec>` prints them for one spec. Spargen derives that contract from the lowered API
and audits it during compilation.

The capabilities are layered around a single seam so the generated `Client` stays non-generic and
a caller adds retry or middleware by wrapping the backend. Several sections below summarize one
module of the embedded runtime and link to its source; that module's own documentation, carried
as doc comments in the private `support` module of every generated client that embeds the module,
is the full account. The `stream`, `xml`, `datetime` and `base64` modules are embedded only when the
compiled API uses them; the rest are in every client.

## The transport seam

`HttpBackend` is a `dyn`-able trait that abstracts exactly one step: how a prepared
`http::Request<HttpBody>` is executed into an `http::Response<HttpBody>`. Everything else — URL
building, auth attachment, decode, streaming, pagination — operates on the request/response
*around* that step, so swapping the backend swaps only the execute step and leaves the rest
untouched. The generated `Client` holds an `Arc<dyn HttpBackend>` (not a type parameter), and async
methods return a manually boxed future rather than using `async-trait`.

The currency is the `http` crate's types, so a backend need not be built on reqwest. `HttpBody` is
the runtime's own body: bytes in memory, which clone (so a request can be retried or a stream
reconnected), or any `http_body::Body` adopted with `HttpBody::wrap`, which streams and is read
once. A backend reports a failure as a `TransportError` built with the constructor naming its
class — `connection`, `timeout`, `protocol`, `redirect`, `request` or `interrupted` — and the
runtime maps it to the matching `Error` variant, so `Error::is_transient()` and retry policies read
one classification whichever backend ran.

`ReqwestBackend` is the default backend: it adapts each request to a `reqwest::Client` and each
response back, an in-memory body moving across without a copy off wasm, and classifies reqwest's
failures exactly as executing on the client directly always has. On `wasm32` it runs reqwest's
`fetch` backend, which sends no streaming request body. The retry and middleware adapters below
are themselves `HttpBackend`s that wrap an inner backend, so they compose by nesting.
`Client::with_backend(backend, base_url)` takes the outermost one as an `Arc<dyn HttpBackend>`;
requests are still built on a default `reqwest::Client`, and only the execute step goes through the
backend.

## Retry

`RetryBackend` wraps any inner `HttpBackend` and re-executes a request per a caller-supplied
`RetryPolicy`, which is handed each attempt's `RetryOutcome` and returns the wait before the next
one as a `RetryWait`, built on the caller's own timer: the runtime has none. A request whose body
cannot be cloned is sent exactly once. `Error::is_transient()` and `RetryOutcome::is_transient()`
classify retry-worthy failures, and `Error::status()` reports the status of every failure that
produced a response. `RetryOutcome::is_transient()` also requires the request to be safe to
repeat: its method idempotent by [RFC 9110 §9.2.2](https://www.rfc-editor.org/rfc/rfc9110#section-9.2.2)
(`GET`, `HEAD`, `OPTIONS`, `TRACE`, `PUT`, `DELETE`), or an `Idempotency-Key` header on it. A
`POST` or `PATCH` without one may already have been applied when its response was lost, so the
default classifier does not resend it; `RetryOutcome::method()` and `RetryOutcome::request()`
give a policy that knows better what it needs to decide otherwise.

`RetryOutcome::retry_after(now)` reads a response's `Retry-After`, as delay-seconds or any of the
three HTTP-date forms, as the delay from the caller's clock, and `RetryOutcome::backoff(attempt,
base, max, now)` is `exponential_backoff(attempt, base, max)` raised to that delay where it is
longer. The server's delay is a floor that `max` does not cap, so a policy unwilling to wait that
long stops instead.

Size `base` and `max` against the failures being retried. A backoff shorter than the network's
loss bursts, or than the server's outages, lands each retry inside the same bad state and spends
the retry budget there; a `base` above a typical loss burst and a `max` above a typical outage
let later attempts arrive after the condition has cleared. The [`retry` module](https://github.com/getkono/spargen/blob/master/support-runtime/src/retry.rs)
states the timing and replay contracts and carries an example policy; the
[petstore example](https://github.com/getkono/spargen/tree/master/examples/petstore) ships a
complete one driven by a tokio timer.

## Problem details

`Error::problem()` returns the [RFC 9457](https://www.rfc-editor.org/rfc/rfc9457) members of the
failed call's error response body as a `ProblemDetails` (`problem_type`, `title`, `status`,
`detail`, `instance`), whichever operation and status produced it. Its bound is
`ApiErrorProblem`, which every generated error type implements — the multi-status enum, including
one whose statuses carry different body types and so has no `api_body`, the single-body newtype,
and the uninhabited shape — so one function generic over `E` reads every operation:

```rust
fn log_problem<E: api::ApiErrorProblem>(error: &api::Error<E>) {
    if let Some(problem) = error.problem() {
        eprintln!("{}: {:?}", problem.problem_type_or_blank(), problem.detail);
    }
}
```

It answers from whichever class carried a body:

- `Error::Api`: the typed body, read as it serializes to JSON. A documented bodyless status, and
  a body read as raw bytes, answer `None`.
- `Error::Decode` and `Error::UnexpectedStatus` with a `4xx` or `5xx` status: the retained raw
  body, parsed as JSON. A documented error status whose body did not match its schema — a problem
  `type` the description does not list — still yields its `type` and `detail` here. A body the
  error-body cap truncated is no longer JSON and answers `None`, and a success status is never
  read.

A member is `Some` only with the type RFC 9457 gives it (a string, or an integer HTTP status for
`status`); `problem_type_or_blank()` applies the RFC's `about:blank` default for an absent `type`.
The reader asserts nothing about whether the server meant the body as problem details: an object
body answers with whichever of those member names it carries. Every other member is an extension
member, kept as JSON in `extensions` (a `serde_json::Map`), so a generic reader reaches one such as
a validation problem's `errors` the same way across every operation:

```rust
fn validation_errors<E: api::ApiErrorProblem>(error: &api::Error<E>) -> Option<serde_json::Value> {
    error.problem()?.extensions.remove("errors")
}
```

A standard member carried with another type than the RFC gives it is in neither place.

A description that narrows the problem `type` per status (`allOf: [$ref: Problem, {properties:
{type: {const: …}}}]`) makes an unlisted `type` a decode failure by default, so it reaches the raw
path above. The opt-in `open_narrowing` (`Spec::open_narrowing(true)`, `open_narrowing = true` in
`spargen.toml`, `--open-narrowing`, or `open_narrowing` in `generate_api!`) lowers that narrowing,
in a response body's own schema, to an open enum instead: each listed value keeps its variant, one
more (`Other(String)`, renamed when a listed value takes that name) holds any other string, and
the response decodes into `Error::Api`, where `problem()` reads it like any other.

Only that position opens. A narrowing written inside a `$ref` target (a component, a sub-file
schema) or inside a `oneOf`/`anyOf` stays closed, because a target is shared by every use and a
union's variants are told apart by what each refuses. A narrowing whose value set is itself a
`$ref` target opens a copy and leaves the target closed. Request bodies, parameters, and a
non-`string` or formatted (`uuid`, date) base stay closed. `spargen diff` reads the catch-all as a
variant: `variant-added` when a set opens, `variant-removed` when it closes or is renamed.

## Middleware

`MiddlewareBackend` wraps an inner backend with an ordered chain of `Middleware`, each handed the
request and a `Next` continuation: it may modify the request, read the response, short-circuit
without calling `Next::run`, or do async work around the call. The first middleware runs
outermost, and nesting the chain inside or outside a `RetryBackend` decides whether it runs per
attempt or once. The [`middleware` module](https://github.com/getkono/spargen/blob/master/support-runtime/src/middleware.rs)
states the ordering and lifetime contracts.

## Pagination

OpenAPI declares no machine-readable pagination, so the runtime ships a generic helper a caller
drives explicitly: `client.core().paginate_links::<T>(first_url)` returns a `LinkPaginator<T>`
that follows `Link: <url>; rel="next"` headers one `next_page().await` at a time, issuing a plain
`GET` that attaches no per-operation credentials. Cursor and offset schemes are a loop over the
generated operation. The [`paginate` module](https://github.com/getkono/spargen/blob/master/support-runtime/src/paginate.rs)
shows both, and how to authenticate follow-up pages.

## Streaming

A streaming operation returns an `EventStream<T>` for Server-Sent Events (`text/event-stream`),
newline-delimited JSON, or JSON Text Sequences. It implements the standard
`futures_core::Stream<Item = Result<T, StreamError>>` while retaining an inherent
`next().await` convenience method, and pulls the response's `HttpBody` chunks incrementally as the
backend delivers them. Dropping it cancels
the response through ordinary HTTP drop semantics.

For OpenAPI 3.2 SSE, spargen parses the envelope fields first. A string `data` property annotated
with `contentMediaType: application/json` and a typed `contentSchema` yields that JSON payload type
directly. `last_event_id()` and `reconnect_delay()` expose the latest valid `id:` and `retry:`
metadata, including metadata-only events.

Automatic reconnect is explicit: call `with_reconnect(Arc<dyn ReconnectPolicy>)`. The caller's
policy owns attempt limits and supplies the wait future, so the runtime introduces neither a timer
nor a retry default. A replayable prepared request is cloned, its cookies and other headers are
preserved, and the latest event ID is sent as `Last-Event-ID`. Declining a reconnect yields the
original typed stream error unchanged. On `wasm32`, reqwest's fetch implementation may buffer the
body internally; the stream API and framing remain the same.

## Blocking (feature `blocking`)

A synchronous `BlockingClient` for callers without an async runtime: `BlockingRuntime` drives the
async operation futures on a current-thread `tokio` runtime, so it adds only tokio's `rt` feature.
It compiles only under the `blocking` feature on a non-wasm32 target, so a wasm32 build never
carries it. The feature resolves against the consumer crate, so leaving `blocking` undeclared
compiles the facade out; [`spargen deps`](./cli.md#spargen-deps) prints the opt-in. A
`BlockingRuntime` must not be built inside another async runtime;
the [`blocking` module](https://github.com/getkono/spargen/blob/master/support-runtime/src/blocking.rs)
says where to drive one instead.

## WebAssembly

A generated client compiles for native targets and for `wasm32-unknown-unknown` (the browser, via
reqwest's `fetch` backend): the `MaybeSend` / `MaybeSync` bounds are exactly `Send` / `Sync` off
wasm and vacuous on it. The [`wasm` module](https://github.com/getkono/spargen/blob/master/support-runtime/src/wasm.rs)
explains the mechanism, and the [support matrix](./support-matrix.md)'s Targets row lists what wasm
lacks.

## XML bodies

An XML request/response body codec backed by `quick-xml`, mirroring the JSON paths. It is embedded,
and `quick-xml` required of the consumer, only when the spec uses an `application/xml` /
`text/xml` body; no consumer-side Cargo feature turns it on or off. The
[`xml` module](https://github.com/getkono/spargen/blob/master/support-runtime/src/xml.rs) is the
codec.

## Format mappings

Numeric and boolean types map from `type` and `format` alone, with no knob:

| Schema | Rust |
| --- | --- |
| `type: integer, format: int32` | `i32` |
| `type: integer` (any other or absent `format`, including `int64`) | `i64` |
| `type: number` (any `format`, including `float` and `double`) | `f64` |
| `type: boolean` | `bool` |
| `type: string` (any unrecognized `format`) | `String` |

`float` widens to `f64` rather than mapping to `f32`: the specification makes support for any
registered format "strictly OPTIONAL" and lets a tool "default back to the `type` alone", and a
narrower float would round-trip fewer values than the wire carries. An unrecognized `format` is a
JSON Schema annotation, not an assertion, so it is likewise carried into the base type.

A finite `f64` read from a `simple`-style response header is exactly the value the server wrote:
its text is parsed with the standard library's correctly rounding parser. A JSON body (and a
`content`-typed header) is decoded by `serde_json`, which without its `float_roundtrip` feature
may read a float's shortest text as a neighbouring value. Generated code does not require that
feature; a consumer that needs exact floats from JSON enables it on its own `serde_json`
dependency, and Cargo's feature unification applies it to the generated client:

```toml
serde_json = { version = "1", features = ["float_roundtrip"] }
```

### `uuid` and `time`

`format: uuid` maps to the `uuid` crate and `format: date-time` / `date` to `time`, as opt-out
mappings in generated code. Call `Spec::uuid(false)` / `Spec::time(false)` (or use
the macro's `no_uuid` / `no_time`, or `uuid = false` / `time = false` in `spargen.toml`) to fall
back to `String`. The corresponding dependency is
required only when that mapping is enabled and actually occurs in the compiled API.

Dates are emitted as the embedded `DateTime(pub time::OffsetDateTime)` and `Date(pub time::Date)`
newtypes rather than `time`'s own types, because `time`'s serde output is not the RFC 3339 text
these formats are defined as. They carry a hand-written RFC 3339 codec, so only `time`'s
`formatting` and `parsing` features are needed; they `Deref` to and convert with `From` from the
inner type, and parse with `FromStr`, failing with `DateParseError`, which the generated root
re-exports beside them. The [`datetime` module](https://github.com/getkono/spargen/blob/master/support-runtime/src/datetime.rs)
sets out why `time`'s own representation is not RFC 3339.

### Binary content

`format: binary` maps to `bytes::Bytes`. A `contentEncoding: base64` string is a generated newtype
over `bytes::Bytes` (`pub struct Data(pub bytes::Bytes);`), which `Deref`s to the bytes and
converts with `From` in both directions. Its serde form is RFC 4648 base64 text, standard alphabet
and padded, wherever a JSON, XML or form body carries it: a field, an array item, a map value or a
union variant. Decoding also accepts the text without its padding. `bytes::Bytes`' own serde form
is a sequence of numbers, so the newtype carries the embedded
[`base64` module](https://github.com/getkono/spargen/blob/master/support-runtime/src/base64.rs)'s
codec instead, which adds no dependency and is embedded only when such a schema is emitted. A
whole request or response body and a multipart part carry the octets themselves under either
keyword: a whole body is typed `bytes::Bytes`, and a part is read through the newtype's `Deref`.
