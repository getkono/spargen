# Runtime & Ergonomics

The runtime support code is embedded verbatim into generated output — no spargen crate ever
enters a consumer's dependency graph. Its default dependency set is fixed at
`reqwest` / `serde` / `serde_json` / `bytes` / `secrecy`. Every capability below is built to
preserve that set: no `tower`, no `futures`, no `async-trait`, and no async timer of its own.
Std's `Future` / `Pin` / `Box` carry the abstractions.

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

A streaming operation returns an `EventStream<T>` — a *manual* async iterator for response bodies
delivered as Server-Sent Events (`text/event-stream`) or newline-delimited JSON
(`application/x-ndjson`). It holds the live response and pulls bytes incrementally with
`reqwest::Response::chunk` (no reqwest `stream` feature, no `futures` crate), framing complete
items out of the buffer. Drive it with `while let Some(item) = stream.next().await`. On `wasm32`
the body is read once and framed from memory; the API and yielded items are identical.

## Blocking (feature `blocking`)

A synchronous facade for callers without an async runtime. reqwest's async client needs a running
reactor, so `BlockingRuntime` owns a real current-thread `tokio` runtime and drives the generated
async operation futures to completion on it — the blocking client reuses every line of the async
dispatch. Enabled by the `blocking` cargo feature, which pulls in `tokio` with just the `rt`
feature; a client built without it carries no blocking client and no direct tokio dependency.
Standalone-crate output declares this feature automatically. For `include!`/build.rs and macro
output, the feature resolves against the consumer crate: leaving it undeclared cleanly compiles the
facade out, while opting in requires the consumer to declare `blocking` and its optional Tokio
dependency.

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

## XML bodies (feature `xml`)

An XML request/response body codec backed by `quick-xml`, mirroring the JSON paths. It is **off by
default**, so the default dependency set stays exactly reqwest/serde/serde_json/bytes/secrecy; a
generated client turns the `xml` feature on only when its spec uses an `application/xml` /
`text/xml` body.

## Format mappings (`uuid` / `time`)

`format: uuid` maps to the `uuid` crate and `format: date-time` / `date` to `time`, as opt-out
mappings on the emitted crate. `spargen generate --no-uuid` / `--no-time` fall back to `String`.
These live in the synthesized crate manifest, not the base runtime dependency set.
