# Runtime & Ergonomics

The runtime support code is embedded into generated output as a private `support` module — no
spargen crate ever enters a consumer's dependency graph. Its core dependency set is `reqwest`
(default features off) / `http` / `http-body` / `url` / `serde` / `serde_json` / `bytes` /
`secrecy`; `http` and `http-body` carry the [transport seam](#the-transport-seam)'s request,
response and body types, and `url` the URL a request is built on. A capability that needs more
adds it only when the compiled API uses it, as this table lists; each crate's exact Cargo features
are in the contract linked below. A `multipart/form-data` request body is encoded by the runtime
itself, so it adds nothing:

| Capability | Adds |
| --- | --- |
| A raw `bytes` value serde encodes (a `format: binary` value in a model, a JSON `content` parameter, or a serde-encoded body; a [`contentEncoding: base64`](#binary-content) one carries its own codec) | `bytes`' `serde` feature |
| A body read as it arrives: [streaming](#streaming) (sequential responses), or a [binary `_stream` twin](#binary-content) | `futures-core` and reqwest's `stream` feature |
| [XML bodies](#xml-bodies) | `quick-xml` |
| [`format: date-time` / `date`](#uuid-and-dates) | `time`, or the `jiff` or `chrono` crate the `datetime` backend selects |
| [`format: uuid`](#uuid-and-dates) | `uuid` |
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

The currency is the `http` crate's types, so a backend need not be built on reqwest. The runtime
builds every request itself, as an `http::Request<HttpBody>`, a `multipart/form-data` body
included, so no HTTP client is built to build requests, whichever backend executes them. `HttpBody` is
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
`Client::with_backend(backend, base_url)` takes the outermost one as an `Arc<dyn HttpBackend>`
and builds nothing else, so clients that share one backend share everything they send through.
`Client::with_client(client, base_url)` is the same as
`Client::with_backend(Arc::new(ReqwestBackend::new(client)), base_url)`.
`client.clone().with_base_url(url)?` derives a client value repointed at another base URL, keeping
its backend, configuration and credentials.

Every operation method is its two halves around that step: `client.{method}_request(..)` builds,
without sending it, exactly the `http::Request<HttpBody>` the method hands the backend, and
`client.decode_{method}(response)` decodes an `http::Response<HttpBody>` into the method's own
`Result`, under the client's body limits. A caller that owns the send — signing the exact request,
pacing it, re-preparing it on each attempt, or sending it over another transport — calls the two
itself; only the method's own send replays a call a refreshable credential was rejected on. A
binary operation's `{method}_stream` twin is split the same way, into
`client.{method}_stream_request(..)` and `client.decode_{method}_stream(response)` (see
[Binary content](#binary-content)). Each request an operation builds carries the operation's `ops::Operation` (its `id`, `method` and
`path_template`) among its extensions, a replay and a stream reconnect included, so a backend reads
`request.extensions().get::<ops::Operation>()` to sign, route or meter by operation rather than by
parsing paths.

### Testing with a mock backend

`Spec::testing(true)` (`testing = true` in `spargen.toml`, or the macro's `testing` switch) emits a
`testing` module beside the client. Its `MockBackend` is a backend to build the client on, with one
typed registration per operation, so a test fakes the API by operation rather than by path:

```rust,ignore
use std::sync::Arc;
use api::{ops, testing};

let mock = testing::MockBackend::new()
    .on_get_pet(|call| testing::get_pet::Reply::status_200(&pet(call.path())))
    .on_delete_pet(|_| testing::delete_pet::Reply::status_4xx(409, &problem("pet.busy")));
let client = api::Client::with_backend(Arc::new(mock.clone()), "http://mock")?;
// ... exercise the code under test ...
assert_eq!(mock.calls(ops::GET_PET).len(), 1);
```

- **Routing.** Each request is routed by the `ops::Operation` among its extensions to the handler
  `on_{method}` registered; the latest registration for an operation wins. A path changed in the
  description needs no change to the fake, and a renamed operation renames its registration, so a
  stale fake fails to compile rather than answering a route the client no longer sends.
- **Replies.** A handler returns its operation's own `testing::{method}::Reply`, with one
  constructor per documented response: `status_200(&body)` for an exact status,
  `status_4xx(status, &body)` for a range, `default(status, &body)` for `default`, and no body
  argument for a bodyless response. Each encodes its body with the codec the client decodes that
  status with (JSON, `text/plain`, XML, or raw octets), so the client reads it as the typed success
  or the typed `Error::Api`, never as `Error::Decode`. A streamed body, or a response the
  description does not document, is built with `Reply::from_reply(testing::Reply::new(status)…)`.
- **Recording.** Every request is recorded as a `testing::Call` (its operation, method, URI,
  headers and body, with `json::<T>()` to decode the body) before it is routed; `calls(op)` and
  `all_calls()` read them back. Clones of a `MockBackend` share their handlers and calls.
- **Unrouted requests.** A request no handler is registered for fails with
  `Error::RequestConstruction`, whose source chain holds a `testing::Unrouted` naming the
  operation, its method and path template, and the URI it was sent to.

## Timeouts

`Client::new` bounds how long a request may wait, through three `ClientConfig` fields whose
defaults are deliberately generous, for an application to tighten or loosen:

| Field | Default | Bounds |
| --- | --- | --- |
| `connect_timeout` | 30 s | name resolution, the TCP handshake and TLS |
| `read_timeout` | 5 min | each wait for the next bytes of a response, a stream's included; every read restarts it |
| `timeout` | none | one non-streaming request, from connecting until its whole body has been read |

Each one that elapses fails the request with `Error::Timeout`, which is transient. Set them with
`Client::with_config(base_url, |config| { … })`, which hands the configuration over before the
`reqwest::Client` the default backend runs on is built from the connect and read timeouts. The
total `timeout` is applied to each request as it is sent, as a `TotalTimeout` extension the
backend enforces (`ReqwestBackend` does, and a backend of your own reads it from the request), and
an operation returning an `EventStream` never applies it, since a stream may rightly stay open
indefinitely. An event stream whose server sends nothing for longer than
`read_timeout` still fails with `Error::Timeout`, which a reconnect policy may answer.

`Client::with_timeout(Option<Duration>)` (and `BlockingClient::with_timeout`) sets the total
`timeout` of the client value it returns, so one call can have a deadline of its own without an
async timer: `client.clone().with_timeout(Some(UPLOAD)).add_torrent(&request).await`. An elapsed
one is `Error::Timeout(TimeoutKind::Total)`. `None` sets no total bound of the client's own;
the connect and read timeouts still apply, and so does a total timeout configured on a
`reqwest::Client` injected with `with_client`, which reqwest lets a request shorten or lengthen
but not lift. An operation returning an `EventStream` is exempt, as it is from `timeout`.

`Client::with_client` and `Client::with_backend` run on the default configuration, whose total
`timeout` is none, and leave connecting and reading to what they were given: the caller's
`reqwest::Client` keeps its own timeouts, and a backend bounds requests however it bounds them.
On `wasm32`, reqwest's `fetch` backend has no connect or read timeout, so only the total `timeout`
applies there.

## Per-call options

`Client::with_options(RequestOptions)` (and `BlockingClient::with_options`) sets the options every
call made through the client value it returns carries, replacing the ones it had, so a call whose
limits differ derives a client for itself rather than arming shared state in a backend:

```rust,ignore
let page = client
    .clone()
    .with_options(RequestOptions {
        max_body: Some(PAGE_LIMIT),
        ..RequestOptions::default()
    })
    .sync_feed(&params)
    .await?;
```

- `max_body` bounds the response body of a non-streaming operation. A `Content-Length` past it is
  refused before the body is read, and a body that grows past it is abandoned at the chunk that
  crosses it; either is `Error::BodyTooLarge { limit, status }`, which is not transient. It
  replaces `ClientConfig::max_success_body` (default none: a structured success body is read whole,
  and a raw binary one within `ClientConfig::DEFAULT_MAX_BINARY_BODY`, 64 MiB), which
  `with_config` sets for every call. An error body is read no further than `max_error_body` in any
  case; where `max_body` is the smaller of the two, an error body past it is `BodyTooLarge` instead
  of truncated. An operation returning an `EventStream` is exempt, its error body included:
  `max_stream_frame` bounds its items. So is each raw binary body a `_stream` twin returns as a
  [`ByteStream`](#binary-content); any other success body a twin of a success enum reads is
  bounded as the method's is.
- `extensions` are copied onto each request, where a caller-supplied `HttpBackend` reads them from
  the `http::Request` it executes. An extension type the request already carries keeps its value.
- `headers` are added to each request, streaming operations' included. A header the request already
  carries (an operation's own header parameter, its content type, an attached credential) keeps
  its own values. `ClientCore`'s `Debug` prints them; mark a secret value with
  `HeaderValue::set_sensitive`.

## Retry

`RetryBackend` wraps any inner `HttpBackend` and re-executes a request per a caller-supplied
`RetryPolicy`, which is handed each attempt's `RetryOutcome` and returns the wait before the next
one as a `RetryWait`, built on the caller's own timer: the runtime has none. A request whose body
cannot be cloned is sent exactly once. A `multipart/form-data` body is encoded in memory, every
part included, so it clones: a multipart call is retried, and replayed after a credential refresh,
like any other; a body adopted with `HttpBody::wrap` streams, and is sent once.
`Error::is_transient()` and `RetryOutcome::is_transient()` classify retry-worthy failures, and `Error::status()` reports the status of every failure that
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
original typed stream error unchanged. On `wasm32`, `ReqwestBackend`'s `fetch` response is read
chunk by chunk as the browser delivers it, through reqwest's `stream` feature; the stream API and
framing are the same as natively.

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
| `type: integer, format: uint8` | `u8` |
| `type: integer, format: uint16` | `u16` |
| `type: integer, format: uint32` | `u32` |
| `type: integer, format: uint64` | `u64` |
| `type: integer` (any other or absent `format`, including `int64`) | `i64` |
| `type: number` (any `format`, including `float` and `double`) | `f64` |
| `type: boolean` | `bool` |
| `type: string` (any unrecognized `format`) | `String` |

`float` widens to `f64` rather than mapping to `f32`: the specification makes support for any
registered format "strictly OPTIONAL" and lets a tool "default back to the `type` alone", and a
narrower float would round-trip fewer values than the wire carries. An unrecognized `format` is a
JSON Schema annotation, not an assertion, so it is likewise carried into the base type.

The unsigned formats map to the unsigned type of their width, so a `uint64` above `i64::MAX`
decodes and a negative value is a decode error. Through 0.7 they lowered to `i64`, so a description
that uses them generates a different field type than it did then, which `spargen diff` classifies
as major. Parameters and headers of an unsigned type are written and read as decimal, like the
signed ones. An unsigned field's `default` is applied only within its width; a negative one, or one
past the width, is documented and reported as `W005`. An `allOf` that meets an unsigned width with a
signed integer or a `number` keeps the unsigned type, and two unsigned widths meet at the narrower.

A finite `f64` read from a `simple`-style response header is exactly the value the server wrote:
its text is parsed with the standard library's correctly rounding parser. A JSON body (and a
`content`-typed header) is decoded by `serde_json`, which without its `float_roundtrip` feature
may read a float's shortest text as a neighbouring value. Generated code does not require that
feature; a consumer that needs exact floats from JSON enables it on its own `serde_json`
dependency, and Cargo's feature unification applies it to the generated client:

```toml
serde_json = { version = "1", features = ["float_roundtrip"] }
```

### `uuid` and dates

`format: uuid` maps to the `uuid` crate, as an opt-out mapping: call `Spec::uuid(false)` (or use the
macro's `no_uuid`, or `uuid = false` in `spargen.toml`) to fall back to `String`.

`format: date-time` / `date` map to the crate the date-time backend names: `Spec::datetime`
(`spargen::DateTimeBackend`), `datetime = "…"` in `spargen.toml`, `--datetime …` on the CLI, or
the macro's `datetime = "…"`.

| Backend | `DateTime` wraps | `Date` wraps | Requires |
| --- | --- | --- | --- |
| `time` (default) | `time::OffsetDateTime` | `time::Date` | `time` with `formatting` and `parsing` |
| `jiff` | `jiff::civil::DateTime` and `jiff::tz::Offset` | `jiff::civil::Date` | `jiff` |
| `chrono` | `chrono::DateTime<chrono::FixedOffset>` | `chrono::NaiveDate` | `chrono` |
| `string` | — (`String`) | — (`String`) | nothing |

Each dependency is required only when its mapping is selected and actually occurs in the compiled
API.

Dates are emitted as the embedded `DateTime` and `Date` newtypes rather than the crate's own types,
because `time`'s serde output is not the RFC 3339 text these formats are defined as, and so that
every backend reads and writes one wire form. They carry a hand-written RFC 3339 codec, so no
backend's `serde` feature is needed (only `time`'s `formatting` and `parsing`). `Date`, and the
`time` and `chrono` backends' `DateTime`, `Deref` to and convert with `From` from the inner type
(the `jiff` `DateTime`'s conversions are set out below). Every newtype parses with `FromStr`,
failing with `DateParseError`, which the generated root re-exports beside them. The `jiff` and `chrono` newtypes share the
[`rfc3339` codec](https://github.com/getkono/spargen/blob/master/support-runtime/src/rfc3339.rs),
which states the grammar they accept and write.

That grammar is the specification's, not any crate's: OpenAPI's `format: date-time` is JSON
Schema's, which is RFC 3339 §5.6's `date-time`. Its year is four digits, so every year from `0000`
to `9999` decodes, at any offset; the separator between date and time is `T` or `t` (so the `time`
backend refuses the space `time` itself would read); and a leap second (`:60`) is read only in the
minute that is 23:59 UTC on a month's last day, as the last nanosecond of that minute. Every backend
keeps the offset a value was written with and writes it back at that offset. A `chrono` value holding
a leap second anywhere else is written as the last nanosecond of its second, since no backend reads
`:60` there.

`jiff::Timestamp` ends at `9999-12-30T22:00:00.999999999Z`, short of the last instants RFC 3339
writes, so the `jiff` backend's `DateTime` holds the civil date-time and its offset instead of
dereferencing to one `jiff` value: `datetime()` and `offset()` read the parts, `From` converts a
`jiff::Timestamp` (at UTC), and `to_timestamp()` (or `TryFrom`) gives the instant, failing past
that range. Equality, ordering and hashing compare instants under every backend. The
[`datetime` module](https://github.com/getkono/spargen/blob/master/support-runtime/src/datetime.rs)
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

A whole `format: binary` request body is taken by value as `impl Into<HttpBody>` (an optional one
as `Option<HttpBody>`): `Bytes` and `Vec<u8>` move in without a copy, and `HttpBody::wrap` sends any
`http_body::Body` as it is read, for an upload produced incrementally or larger than it should be
held in memory. A streaming body does not clone, so neither a retry nor a credential refresh
resends it; reqwest's `fetch` backend on `wasm32` refuses one as `Error::RequestConstruction`.

```rust,ignore
client.put_blob(Bytes::from(sealed)).await?;              // in memory, no copy
client.put_blob(HttpBody::wrap(paced_upload)).await?;     // streamed as it is read
```

An operation whose success holds a whole `format: binary` body, as its one success body or as a
status of its success enum, reads it within `ClientConfig::DEFAULT_MAX_BINARY_BODY` (64 MiB) unless
`max_body` or `max_success_body` sets another limit, so a large or hostile download fails as
`Error::BodyTooLarge` instead of growing without bound. Each such operation also has a
`{method}_stream` twin. Where the binary body is the one success body, the twin returns
`ResponseValue<ByteStream>`; where the success is an enum (a `200` and a `206` of octets, or a
`200` download beside a bodyless `204`), it returns `ResponseValue<{Method}StreamResponse>`, the
method's enum with each binary status's variant carrying a `ByteStream`, and every other status
read as the method reads it, within the client's body limits. That enum has the method's enum's
`status()`, and an `into_body()` where every bodied status is binary. A `ByteStream` is the body
handed over unread: `next()` yields it chunk by chunk,
`collect_bounded(limit)` reads the rest whole within a limit of the caller's choosing (a
`Content-Length` past it refused unread), and `into_body()` hands over the `HttpBody` for a reader
of the caller's own. `collect_bounded` is its only collector. Like an `EventStream`, the
twin is exempt from the total timeout and the body limits; the connect and read timeouts still
apply. Its errors are `Error<Infallible>`, which `Error::widen` turns into the operation's.
On `wasm32` too, the twin reads `ReqwestBackend`'s `fetch` response chunk by chunk as the browser
delivers it, so `collect_bounded` stops at the chunk that crosses its limit rather than after the
whole body is in memory; an API with a twin therefore requires reqwest's `stream` feature and
`futures-core`, as one with a sequential response does. The buffered method reads a `fetch`
response whole on `wasm32`, and checks its limit on what it read, or up front on a declared
`Content-Length`.

```rust,ignore
let blob = client.get_blob_stream(id).await?.into_inner();
let bytes = blob.collect_bounded(MAX_BLOB).await.map_err(Error::widen)?;

match client.get_range_stream(id).await?.into_inner() {
    GetRangeStreamResponse::Status200(whole) => store(whole).await?,
    GetRangeStreamResponse::Status206(part) => append(part).await?,
}
```

The twin has its own two halves, as every operation method does: `{method}_stream_request(..)`
builds the twin's request without sending it, which carries no total timeout where the method's
`{method}_request` carries the client's, and `decode_{method}_stream(response)` hands a binary
success body over unread as a `ByteStream` and dispatches an error status into the operation's
typed error, as the twin does. Neither reads a binary body under the client's body limits. Where
another method already spells either name, that method keeps it and the twin's companion takes a
disambiguated name, as the twin's enum does where another type already spells
`{Method}StreamResponse`.

```rust,ignore
let request = client.get_blob_stream_request(id).await?;
let response = my_transport.send(request).await?;
let blob = client.decode_get_blob_stream(response).await?.into_inner();
```
