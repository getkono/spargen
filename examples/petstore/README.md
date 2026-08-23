# Petstore example

The full spargen loop in one crate: [`petstore.yaml`](petstore.yaml) (OpenAPI 3.1) is turned
into a typed client by [`build.rs`](build.rs) at compile time, and [`src/main.rs`](src/main.rs)
drives that client against a tiny mock HTTP server on `127.0.0.1` — so the example needs no
network access, no API key, and cannot spam a real service no matter how often it runs.

```bash
cargo run
```

What it exercises:

- **Generation** — the whole spargen pipeline (`source` → `oas31` → `ir`/`name` → `codegen` →
  `emit`) runs from `build.rs`; `spargen` is a build-dependency only and never appears in the
  runtime dependency tree.
- **Typed surface** — models (`Pet`, `NewPet`, a `Status` enum), one method per operation,
  positional required parameters, an optional-`Params` struct, `ResponseValue<T>` with status
  and headers.
- **Wire behavior** — path/query/header parameters, a `deepObject` filter that travels as
  `filter[name]=…` pairs, a JSON request body, a multipart body whose parts carry the
  `Content-Type` the Encoding Object declares, and a `204` unit response. The mock server asserts
  those bytes, so a serialization regression fails the example rather than passing silently.
- **Typed servers** — the spec's server is templated (`http://{host}:{port}`), so spargen emits a
  builder whose variables default to a resolvable URL.
- **Typed response headers** — the documented `X-Total-Count` is read through a generated
  accessor, as an explicit second step that cannot turn a successful call into a failure.
- **Auth** — a bearer credential registered with `with_credential`; a missing credential fails
  before the request is sent.
- **Error taxonomy** — a documented `404` arrives as the operation's typed error body; an
  undocumented `401` is preserved as `Error::UnexpectedStatus` and classified non-transient.

To review and commit the generated client, change the `build.rs` output to `src/petstore.rs` and
include it as a normal module. Generation remains a compilation-time step.

```rust
let build = spargen::Spec::new("petstore.yaml").build("src/petstore.rs");
```
