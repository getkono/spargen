# Generated API & Guarantees

What every client spargen emits looks like, and what spargen promises about it. The
[Getting Started](./getting-started.md#the-generated-client-api) example shows the same surface in
use.

## Generated surface

- `Client::new(base_url)` / `Client::with_client(reqwest::Client, base_url)` — the injected
  client is the extension point for TLS choice, proxies, and timeouts. Every constructor, and
  `with_base_url`, refuses a base URL that is not an `http` or `https` URL (`localhost:8080`,
  `mailto:x`) as `Error::RequestConstruction`, as each request refuses such a server override
  before anything is sent. `Client` is `Debug` and
  `Clone`. `Client::with_timeout(Option<Duration>)` gives a derived client value its own total
  timeout (`client.clone().with_timeout(Some(UPLOAD))`), elapsing as
  `Error::Timeout(TimeoutKind::Total)`; an `EventStream` operation is exempt from it.
  `Client::with_options(RequestOptions)` gives a derived client value per-call options: a
  response body limit (`max_body`, refused as `Error::BodyTooLarge`), extensions a transport
  backend reads off each request, and headers added where a request lacks them.
- One `async` method per operation: path parameters positional, in the order the URL template
  names them, every other parameter in a per-operation `…Params` struct, and
  `Result<ResponseValue<T>, Error<E>>` out. Where every parameter in the struct is optional, it
  derives `Default` and is built `…Params::default().limit(20)`, and the method takes it as
  `impl Into<Option<…>>`. Where a query, header, cookie or `querystring` parameter is required,
  `…Params::new(..)` takes each required one, in declaration order, the optional ones are set by
  name as before, and the method takes the struct itself: `client.search(SearchParams::new(from,
  to).limit(20))`. Only path parameters and `new`'s arguments are positional, and `spargen diff`
  reports a reorder of either as major (`positional-args-reordered`), since two same-typed
  arguments trading places still compiles. String parameters and the optional bundle take
  `impl Into<…>`, so `client.get_pet("1")` and `client.list_pets(params)` need no `to_owned()` or
  `Some`. An operation whose method name
  would spell one of `Client`'s or `BlockingClient`'s fixed methods (`new`, `with_timeout`,
  …) takes a disambiguated name instead. Both clients emit `with_default_server` when the
  description declares a server, and only `BlockingClient` emits `inner`; both names are reserved
  on both clients regardless, so declaring a server never renames an operation.
- Beside each operation method, a `{method}_url` accessor on both clients returns, as
  `Result<url::Url, Error<Infallible>>`, the URL that method sends its request to, less any
  `apiKey` query credential, without sending it: the same path-parameter and query encoding,
  resolved against the base URL or the operation's server override. It takes the path parameters
  and, where it holds a query or `querystring` parameter, the `…Params` bundle; it attaches no
  credential. This is for a URL handed to a fetcher of its own, such as a media player
  or a browser (`client.stream_file_url(id)`). Where another operation's method already spells
  `{method}_url`, that method keeps it and the accessor takes a disambiguated name.
- Each operation is also its two halves, on both clients: `{method}_request` takes the method's
  arguments and returns, as `Result<http::Request<HttpBody>, Error<Infallible>>`, exactly the
  request the method sends — URL, headers, credentials, body, and the client's `RequestOptions` —
  without sending it, and `decode_{method}` decodes an `http::Response<HttpBody>` the caller
  obtained into the method's own `Result`, with the same status dispatch, typed error and body
  limits. A caller that signs the exact request, paces or retries its own sends, or uses another
  transport keeps the typed surface. Only the method itself replays a call a refreshable
  credential was rejected on. Where another method already spells either name, that method keeps
  it and the companion takes a disambiguated name.
- An `ops` module holds one `ops::Operation` constant per operation (`ops::GET_PET`, its method
  name upper-cased) with its `id` (the `operationId`), HTTP `method` token and `path_template`, so
  a caller names an operation without copying its path. Every request an operation builds carries
  its `Operation` in its extensions, a replay or stream reconnect included, so a transport backend
  can sign, route or meter by operation.
- A `format: binary` request body is taken as `impl Into<HttpBody>`: `Bytes` or `Vec<u8>` move in
  without a copy, and `HttpBody::wrap` streams an upload as it is read. A `format: binary` success
  body read whole, as the one success body or as a status of a success enum, is bounded (64 MiB
  unless `max_body` or `max_success_body` says otherwise), and the operation gains a
  `{method}_stream` twin on `Client`. Where the binary body is the one success body, the twin
  returns `ResponseValue<ByteStream>`; where the success is an enum (a `200` and a `206` of
  octets, or a `200` download beside a bodyless `204`), it returns
  `ResponseValue<{Method}StreamResponse>`, the method's enum with each binary status's variant
  carrying a `ByteStream` and every other status read as the method reads it. A `ByteStream` is
  read chunk by chunk with `next()` or whole with `collect_bounded(limit)`. The twin is its two
  halves too: `{method}_stream_request` builds its request, without the total timeout, and
  `decode_{method}_stream` hands a response's binary success body over as a `ByteStream`. Where
  another method already spells `{method}_stream` or either of its halves, that method keeps it
  and the twin's name takes a disambiguated one, as the twin's enum does where another type
  already spells `{Method}StreamResponse`.
- Every object struct in `types` derives `PartialEq`, as does every type it can hold, so a test
  compares a decoded value with an expected one by `assert_eq!`. `Type::new(..)` takes the
  required members by value in declaration order and sets each optional member as decoding an
  absent one does (its schema `default`, else `None`); a consuming `with_<member>(value)` per
  optional member, and per typed `additionalProperties` map, sets the rest, so a new optional
  member leaves code built that way compiling. A struct with no required member implements
  `Default` as `new()`.
- A member that is both optional and nullable has three wire states, so it is a
  `Nullable<T>` (re-exported at the root): `Nullable::Absent` leaves it off the wire and is what a
  missing member decodes as, `Nullable::Null` writes and reads an explicit `null`, and
  `Nullable::Value(v)` holds a value. Its `with_<member>` setter takes `impl Into<Nullable<T>>`,
  so `with_nickname("Rex".to_owned())` sets a value, and `with_nickname_null()` (or
  `with_nickname(Nullable::Null)`) sends the `null` that clears it in a PATCH. `Nullable` has no
  `From<Option<T>>`, so an `Option` must be spelled out as the state it means. An optional JSON
  `content` parameter whose schema is nullable is a `Nullable<T>` field of its `…Params` struct
  the same way, set by `<param>(value)` and cleared by `<param>_null()`. Where a member or
  parameter is already spelled `<name>_null`, the setter takes a suffix. A required nullable
  member is `Option<T>`, and an optional non-nullable one is `Option<T>` whose `None` is absence.
- Each declared security scheme that takes a credential gives both clients typed setters, named
  after the scheme's `securitySchemes` key: `with_<scheme>(token)` for `http bearer`, `apiKey`,
  `oauth2` and `openIdConnect`, taking a [`secrecy`](https://docs.rs/secrecy) `SecretString`,
  with `with_<scheme>_provider(TokenProvider)` for an async token provider and
  `with_<scheme>_refreshable(token, is_rejection, on_rejected)` for a refreshable token beside
  it; `with_<scheme>(username, password)` for `http basic`; and `without_<scheme>()` for each.
  A `mutualTLS` scheme has none, since the transport's client certificate satisfies it. Each
  setter's rustdoc carries its scheme's description, flows and scopes. A setter whose name an
  operation method or a fixed method already spells (a scheme named `timeout`) takes a
  disambiguated name instead. Operation `security` requirements pick the first satisfiable
  alternative and attach bearer/basic/apiKey credentials. A missing required credential is a
  request-construction error — typed as `RequestError::MissingCredential`, listing for each
  security alternative the schemes with no registered credential; an `http basic` username
  holding `:` (which RFC 7617 §2 forbids) as `RequestError::CredentialMismatch`; and a failed
  token provider as `RequestError::CredentialProvider` — never a silent 401. A failure inside the
  selected alternative never falls through to a later one; `without_<scheme>()` unregisters a
  scheme, so a client derived with it (`client.clone().without_oauth()`) selects the next fully
  registered alternative. A request carries one `Authorization` header: a
  basic or bearer credential replaces the `Basic` one a base URL's `user:pass@` userinfo sets.
  A refreshable token adds the reactive
  case: when its check reads a failed response as a rejection of the token it carried, the
  runtime asks it for a fresh token and sends the call once more, returning the original response
  when the body cannot be cloned, the refresh fails, or the replay is rejected again.
- A header every operation repeats (an API version, a tenant) can be bound once on the client:
  `Spec::bind_header("X-Api-Version")` drops it from every operation's arguments, and
  `Client::with_bound_header(name, value)?` sets the value each operation declaring it sends,
  refusing a value that is not a valid header value (a control character but a tab, such as CR or
  LF) as `Error::RequestConstruction`. An operation that requires it while no value is bound fails
  before anything is sent, as `RequestError::MissingBoundParameter`.
- Cookie parameters share one `Cookie` header, and each value is checked before it joins it, so
  no value can split into a cookie the caller never declared. A `content` or `style: cookie`
  value, sent without escaping, must be an RFC 6265 `cookie-value` (visible ASCII but `"`, `,`,
  `;` and `\`, bare or between two `"`, plus the `,` an `explode: false` `style: cookie` value
  joins its items with); a `style: form` value is percent-encoded, and only a `;` or a control
  character `allowReserved` left literal is refused. A refused value fails the call before
  anything is sent, as `Error::RequestConstruction` whose `RequestError::Other` source downcasts
  to `ParameterError::InvalidCookie`, which names the parameter and never the value.
- A closed error taxonomy, identical across all spargen output: request-construction
  (`Error::RequestConstruction`, carrying a `RequestError` — one of the three credential causes
  above, the missing bound header above, or `RequestError::Other` for every other cause), transport (`Error::Transport`), timeout
  (`Error::Timeout`), protocol (`Error::Protocol`), redirect (`Error::Redirect`), documented API
  error (`Error::Api`, typed `E`), undocumented status (`Error::UnexpectedStatus`, raw body
  preserved), decode failure (`Error::Decode`: status, headers, serde path + body, capped except
  on the one path the emitted `ClientConfig::max_error_body` doc names), interrupted body
  (`Error::InterruptedBody`: the connection dropped partway through a streamed response
  body, or on native targets any other response body, and transient), pagination cycle (`Error::PaginationCycle`: a `LinkPaginator`
  `next` link back to a page it already fetched), pagination origin change
  (`Error::PaginationOriginChanged`: a `LinkPaginator` `next` link to another scheme, host or port
  than its first page's, refused unsent), oversized body (`Error::BodyTooLarge`: a
  non-streaming response body past the `max_body` or `max_success_body` limit the call was made
  under, or a raw binary one past the 64 MiB `ClientConfig::DEFAULT_MAX_BINARY_BODY` where neither
  is set, or a `ByteStream` past the limit its `collect_bounded` was given, refused unread from its
  `Content-Length` or abandoned where it crossed). Every generated error
  type is `Display` + `std::error::Error`, so `Error<E>` drops straight into `?`, `anyhow`, or
  `thiserror`. `Error::is_transient()` classifies retry-worthy failures. An error enum whose
  bodied statuses carry the same body type (one schema, or schemas that generate the same Rust
  type) gets `body()` and an owning `into_body()`, and it, the single-body newtype,
  and the uninhabited shape implement `ApiErrorBody`, so `Error::api_body()` and the owning
  `Error::into_api_body()` hand that body back whichever status carried it (`Error::status()`
  reports that status, the same value as `ResponseValue::status()` on `Error::Api`, and
  `Error::headers()` the response's headers on every class that keeps them); an enum mixing
  body types is matched by variant instead. A multi-status success enum gets `body()` and
  `into_body()` the same way when its bodied statuses share one type (bare, with no `Option`, when
  every status carries a non-null body), and every status enum, and the single-body newtype,
  whose statuses are all exact codes gets `status()`. Every error shape implements `ApiErrorProblem`, so `Error::problem()` reads the RFC 9457
  members (`type`, `title`, `status`, `detail`, `instance`) of whichever error body a failure
  carried, across every operation and body type: the typed body on `Error::Api`, and the raw body
  of a `4xx`/`5xx` `Error::Decode` or `Error::UnexpectedStatus`. The opt-in `open_narrowing`
  decodes a problem type the description does not list into `Error::Api`; see
  [problem details](./runtime.md#problem-details).
- Beyond the request/response path, the embedded runtime carries a swappable transport seam
  (`HttpBackend`) with composable retry and middleware adapters, `Link:`-header pagination, typed
  SSE/NDJSON/JSON-sequence streams, an opt-in `blocking` client, and `wasm32-unknown-unknown`
  support. Streams and XML are emitted only when the spec uses them; the blocking client is
  opt-in. The [runtime reference](./runtime.md) lists what each capability adds to the
  runtime dependency set. Spargen ships no retry *policy* and no async timer — the caller supplies
  both.
- Each schema becomes a model in the generated `types` module. A model whose name would spell one
  of the ten names that module itself uses (`Date`, `DateTime`, `Serialize`, `Deserialize`,
  `BTreeMap`, `String`, `Option`, `Vec`, `Box`, `Result`) takes a disambiguated name instead.
  All ten are reserved whether or not the module writes them, so starting to use a date type never
  renames a model. Likewise, a server builder or server-variable enum in the generated `servers`
  module whose name would spell one of the three names that module uses (`Default`, `Into`,
  `String`) takes a disambiguated name; all three are reserved whether or not the module writes
  them, so adding a free-form server variable never renames a server. A disambiguated name's
  suffix is a hash of the item's JSON Pointer with the type and subtype of each media key on it
  lowercased, so respelling a media key's type or subtype in another letter case renames nothing;
  its parameters are hashed as written, so a respelled parameter (`Charset` for `charset`) does.
- Where two properties of one model, or two values of one enum, case to the same field or variant
  name, the one whose wire name already is that name keeps it (`foo_bar` beside `fooBar`), and
  otherwise the lexically smaller wire name does; the other takes a disambiguated name seeded from
  its own wire name. The order the spec declares them in decides nothing. A name that comes to
  stand for another wire name (a new `foo_bar` beside an existing `fooBar`, or a new query
  `page-size` beside `pageSize`, or a new header `id` beside an existing cookie `id`, since a
  header ranks before a cookie) still compiles and silently sends or reads the other member, so
  `spargen diff` reports it as major (`identifier-rebound`).
- Spec `title`/`summary`/`description` become rustdoc; `deprecated` becomes `#[deprecated]`.

## Design guarantees

- **Freestanding output.** The runtime support code is embedded into the generated module; no
  spargen crate ever appears in a consumer's runtime dependency tree. The runtime dependency set,
  and what each capability adds to it only when the compiled API uses it, is stated in the
  [runtime reference](./runtime.md), and `spargen deps <spec>` prints it for one spec.
  Generation audits the consumer manifest against the tested floors and feature set before
  Cargo/rustc compile the emitted client; see the
  [runtime dependency contract](./getting-started.md#runtime-dependency-contract).
- **Deterministic.** Same spargen version + spec + config ⇒ byte-identical output, enforced by
  test. Item order follows the document's order; nothing depends on hash-map iteration.
- **Edition-independent output.** The generated module is `include!`d into the consumer's crate and
  compiles under *that* crate's edition, so identifiers are escaped against the union of Rust's
  reserved words across editions 2015-2024: a spec that names a field `type` or `gen` emits
  `r#type` / `r#gen` rather than code only some editions accept. Wire names are unaffected — they
  ride on an explicit `serde(rename)`.
- **`include!`-friendly output.** Generated code carries no crate-level inner attributes, so it
  drops into a module or an `OUT_DIR` file consumed with `include!`.
- **Every construct has a disposition.** Supported, warned, or rejected — never a fourth, silent
  behavior; a typed schema is never silently degraded to `serde_json::Value`. The
  [support matrix](./support-matrix.md) and [diagnostic index](./errors.md) are the
  operational contract; the index gives each code's severity and title, and
  `spargen explain E013` prints its full explanation.
- **No `serde(untagged)`.** First-match-wins deserialization can silently misparse; undiscriminated
  unions are rejected instead.
- **`#[forbid(unsafe_code)]` on the embedded `support` module, the `types` module, `impl Client`,
  and `impl BlockingClient`**, `Debug`-redacted
  secrets, and a 64 KiB (configurable) cap on error-body retention, whose reach the emitted
  `ClientConfig::max_error_body` doc states.

## Versioning

The semver surface is the public API of generated output: changes that alter generated
signatures, type shapes, or variant sets are major; output changes invisible to that API are
minor; generator-internal fixes are patch. [`spargen diff`](./cli.md#spargen-diff) classifies a
spec change the same way.
