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
each capability is opt-in.

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
`RetryPolicy`, returning the last outcome once the policy stops or the request can no longer be
replayed.

- **Bring-your-own timing.** The runtime has no async timer and never pulls in `tokio`. The
  *wait* between attempts is a boxed `Future` the caller builds with their own runtime's timer
  (e.g. `tokio::time::sleep`); `RetryBackend` just `.await`s it. The pure `exponential_backoff`
  helper computes the delay `Duration`.
- **Safe replay.** A retry re-sends the same request, so it must be cloned first. A one-shot
  stream body that cannot be rewound (`reqwest::Request::try_clone` returns `None`) is executed
  **exactly once** and its outcome returned unretried — replaying half a consumed stream would
  send a corrupt body.

`Error::is_transient()` on the generated error type classifies retry-worthy failures, so a policy
that retries only transient outcomes is a few lines. The
[petstore example](https://github.com/getkono/spargen/tree/master/examples/petstore) ships a
complete `RetryPolicy` driven by a tokio timer.

## Middleware

`MiddlewareBackend` wraps an inner backend with an ordered chain of `Middleware`. Each middleware
receives the request plus a `Next` continuation: it may inspect/modify the request before calling
`Next::run`, inspect the response after, short-circuit by returning a response without calling
`run`, or do async work around the call. This is the classic tower-like "onion" shape, expressed
with std's `Future`/`Pin`/`Box` — no `tower`, no `futures`, no `async-trait`. `Next` holds only
borrows, so advancing the chain never clones or reallocates.

## Pagination

OpenAPI has no standard machine-readable pagination declaration, so per-operation auto-paginators
cannot be synthesized from a spec. The runtime instead ships *generic* helpers a caller drives
explicitly. `LinkPaginator<T>` follows the common `Link: <url>; rel="next"` scheme
(RFC 5988 / RFC 8288, the GitHub convention), detectable purely at runtime:

```rust,ignore
let first = reqwest::Url::parse("https://api.example.com/items?page=1")?;
let mut pages = client.core().paginate_links::<Vec<Item>>(first);
while let Some(page) = pages.next_page().await {
    let items = page?.into_inner(); // a decoded Vec<Item> for this page
}
```

The generic paginator issues a plain `GET` per page and does not attach per-operation security —
it has no operation context. To authenticate follow-up pages, inject a preconfigured
`reqwest::Client` (with the appropriate default headers) via `Client::with_client`.

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

A synchronous facade for callers without an async runtime. reqwest's async client needs a running
reactor, so `BlockingRuntime` owns a real current-thread `tokio` runtime and drives the generated
async operation futures to completion on it — the blocking client reuses every line of the async
dispatch. Enabled by the `blocking` cargo feature, which pulls in `tokio` with just the `rt`
feature; a client built without it carries no blocking client and no direct tokio dependency.
For `include!`/build.rs and macro output, the feature resolves against the consumer crate: leaving
it undeclared cleanly compiles the facade out, while opting in requires the consumer to declare
`blocking = ["dep:tokio"]` and its native-only optional Tokio dependency.

> A `BlockingRuntime` must not be constructed from inside another async runtime (tokio's
> `block_on` panics within a runtime context). Drive one on a plain thread, or via
> `spawn_blocking` when already inside an async context.

## WebAssembly

A generated client compiles on both native targets and `wasm32-unknown-unknown` (the browser, via
reqwest's `fetch` backend). On native, reqwest's futures are `Send` and the client is shared
across threads; on wasm the browser is single-threaded and those futures are `!Send`. The
`MaybeSend` / `MaybeSync` marker traits bridge the two: on every non-wasm target they are exactly
`Send` / `Sync` (so native bounds and trait-object auto-traits are unchanged), and on wasm they
are vacuous. One set of source compiles on both.

## XML bodies

An XML request/response body codec backed by `quick-xml`, mirroring the JSON paths. It is embedded
only when the spec actually uses an `application/xml` / `text/xml` body, and only then does the
dependency contract require `quick-xml` of the consumer — so an API without XML carries neither the
module nor the dependency. There is no consumer-side Cargo feature to turn it on or off: whether a
generated client speaks XML is a property of its spec, decided at generation time.

## Format mappings (`uuid` / `time`)

`format: uuid` maps to the `uuid` crate and `format: date-time` / `date` to `time`, as opt-out
mappings in generated code. Call `Spec::uuid(false)` / `Spec::time(false)` (or use
the macro's `no_uuid` / `no_time`, or `uuid = false` / `time = false` in `spargen.toml`) to fall
back to `String`. The corresponding dependency is
required only when that mapping is enabled and actually occurs in the compiled API.

Dates are emitted as the embedded `DateTime` and `Date` newtypes rather than `time`'s own types,
because JSON Schema 2020-12 — and so OpenAPI 3.1/3.2 — defines these formats as RFC 3339, which is
not what `time`'s `Serialize` or `Display` produce: without its `serde-human-readable` feature an
`OffsetDateTime` serializes as a nine-element integer sequence, and with it as
`2023-11-14 22:13:20.0 +00:00:00` — a space separator where RFC 3339 requires `T`. The newtypes
carry a hand-written RFC 3339 codec, so only `time`'s `formatting` and `parsing` features are
needed, never `serde`.

They are transparent wrappers: `DateTime(pub time::OffsetDateTime)` and `Date(pub time::Date)`,
both `Deref`ing to the inner type and converting with `From` in both directions, so the whole `time`
API stays one deref away.

```rust
let at = DateTime(time::OffsetDateTime::now_utc());
let year = at.year();                  // through `Deref`
let inner: time::OffsetDateTime = at.into();
```
