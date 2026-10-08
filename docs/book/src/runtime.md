# Runtime & Ergonomics

The runtime support code is embedded verbatim into generated output — no spargen crate ever
enters a consumer's dependency graph. Its core dependency set is fixed at
`reqwest` / `serde` / `serde_json` / `bytes` / `secrecy`; sequential APIs additionally require
`futures-core` and reqwest's `stream` feature. Every other capability below preserves that set:
no `tower`, no `async-trait`, and no async timer of its own. Std's `Future` / `Pin` / `Box` carry
the policy abstractions.

The exact tested version floors and conditional Cargo features are the
[runtime dependency contract](./getting-started.md#runtime-dependency-contract). Spargen derives
that contract from the lowered API and audits it during compilation; optional codec and mapping
dependencies are required only when the emitted API references them.

The capabilities are layered around a single seam so the generated `Client` stays non-generic and
each capability is opt-in. Several sections below summarize one module of the embedded runtime and
link to its source; that module's own documentation, which every generated client carries and
`cargo doc` renders, is the full account.

## The transport seam

`HttpBackend` is a `dyn`-able trait that abstracts exactly one step: how a prepared
`reqwest::Request` is executed into a `reqwest::Response`. Everything else — URL building, auth
attachment, decode, streaming, pagination — operates on the request/response *around* that step,
so swapping the backend swaps only the execute step and leaves the rest untouched. The generated
`Client` holds an `Arc<dyn HttpBackend>` (not a type parameter), and async methods return a
manually boxed future rather than using `async-trait`.

`ReqwestBackend` is the default backend (execute directly on a `reqwest::Client`). The retry and
middleware adapters below are themselves `HttpBackend`s that wrap an inner backend, so they
compose by nesting.

## Retry

`RetryBackend` wraps any inner `HttpBackend` and re-executes a request per a caller-supplied
`RetryPolicy`, which is handed each attempt's `RetryOutcome` and returns the wait before the next
one as a `RetryWait`, built on the caller's own timer: the runtime has none. A request whose body
cannot be cloned is sent exactly once. `Error::is_transient()` and `RetryOutcome::is_transient()`
classify retry-worthy failures, and `Error::status()` reports the status of every failure that
produced a response. The [`retry` module](https://github.com/getkono/spargen/blob/master/support-runtime/src/retry.rs)
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
body answers with whichever of those member names it carries. Extension members are not read.

A description that narrows the problem `type` per status (`allOf: [$ref: Problem, {properties:
{type: {const: …}}}]`) makes an unlisted `type` a decode failure by default, so it reaches the raw
path above. The opt-in `open_narrowing` (`Spec::open_narrowing(true)`, `open_narrowing = true` in
`spargen.toml`, `--open-narrowing`, or `open_narrowing` in `generate_api!`) lowers that narrowing,
in a response body's own schema, to an open enum instead: each listed value keeps its variant, one
more (`Other(String)`) holds any other string, and the response decodes into `Error::Api`, where
`problem()` reads it like any other. The [support matrix](./support-matrix.md)'s Responses row
states exactly which positions it opens.

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
`next().await` convenience method, and pulls reqwest body chunks incrementally. Dropping it cancels
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
The feature resolves against the consumer crate: leaving `blocking` undeclared compiles the facade
out, and opting in means declaring `blocking = ["dep:tokio"]` and a native-only optional `tokio`,
as `spargen deps` prints them. A `BlockingRuntime` must not be built inside another async runtime;
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
