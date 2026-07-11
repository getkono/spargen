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
- **Wire behavior** — path/query/header parameters, a JSON request body, a `204` unit response.
- **Auth** — a bearer credential registered with `with_credential`; a missing credential fails
  before the request is sent.
- **Error taxonomy** — a documented `404` arrives as the operation's typed error body; an
  undocumented `401` is preserved as `Error::UnexpectedStatus` and classified non-transient.

The same client can be generated as checked-in code instead:

```bash
spargen generate petstore.yaml --out src/petstore.rs
```
