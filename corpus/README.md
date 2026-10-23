# Spargen Validation Corpus

This corpus is vendored for repeatable OpenAPI 3.1.x/3.2.x implementation and regression checks.
Large JSON/YAML artifacts are stored through Git LFS, and every file's `SHA-256` below is
verified by `spargen/tests/corpus_manifest.rs` against `manifest.toml`.

Included public APIs:

| ID | Upstream | Revision | Path | SHA-256 | Expected |
| --- | --- | --- | --- | --- | --- |
| `github-api-3-1` | `github/rest-api-description` | `03ca9c1cac754ec9b8369dc75de8a8c753c6e087` | `descriptions-next/api.github.com/api.github.com.json` | `d88008d8198becda210d59fbe64a6554bcc4c979be2348e2e356638b369eee47` | Generate |
| `github-api-3-0` | `github/rest-api-description` | `v2.1.0` | `descriptions/api.github.com/api.github.com.json` | `b138e9cdcf4ac29a23fea1f6579d2840668a5f3d41fe7f160b263bec590d2e3f` | Reject `E001` |
| `openai-openapi` | `openai/openai-openapi` | `5162af98d3147432c14680df789e8e12d4891e6b` | `openapi.yaml` | `74cbcf73838f4cd7e209b2d3f2e9ddc9fa155f21a44360b6fac7646a6d4f5f8b` | Reject `E009` (an `application/sdp`-only response body) |
| `ollama` | `ollama/ollama` | `d47859ce495496196df211e939702364492a2b7f` | `docs/openapi.yaml` | `d54e2ef5c24a396662ca7222af31ab3e26a642efbd2bb060d921b9393e9fef87` | Generate |
| `openapi-boilerplate` | `dgarcia360/openapi-boilerplate` | `41630ba37b628c7bd871230f480f62f694607d3f` | `src/**` | tree `6a1a45fb44e25fb931c3c2b7c85d2b10b70cd82bae78b189b30035cca973c3e8`; root `580f8d9b131756c29dd82535a74f6948dc77e53377e2b3292b49d2778029d209` | Generate |
| `stripe` | `stripe/openapi` | `d5d11f661d1180a847d6e26774517756e6a493a1` | `openapi/spec3.json` | `e24a26de4188fd64dec4c043d5d3726277fdcb07556a493ea481c305b0a223d8` | Reject `E001` (OpenAPI 3.0.0) |
| `twilio-api-2010` | `twilio/twilio-oai` | `bb6288e9f540d2d63540bbaadf6b73fd262c2df3` | `spec/json/twilio_api_v2010.json` | `a6753266b8b05a201e8658734e332ee51d07a0913f2d419335d87bdb287643a2` | Reject `E001` (OpenAPI 3.0.1) |
| `kubernetes-authentication-v1` | `kubernetes/kubernetes` | `fb3cf74c50ec5d117a7d17f1115c9413fd492c3d` | `api/openapi-spec/v3/apis__authentication.k8s.io__v1_openapi.json` | `443427d822f77db77202c96df06d453845abc5cc67390180129a67e6c74d421e` | Reject `E001` (OpenAPI 3.0.0) |
| `meilisearch` | `meilisearch/open-api` | `a2bd2133ac9f9b85fca8fb8b1aa69063c8f1002c` | `open-api.json` | `83cbd10cea1ca75590dc31f1d2e40ef2b636297d47b39c9aefd813e41454cfd1` | Reject `E011` (OpenAPI 3.1.0; invalid null `externalDocs.description`) |
| `mastodon-openapi` | `abraham/mastodon-openapi` | `aea01d055ea82b898ff24f5004d1012bec1de25f` | `dist/schema.json` | `87d163d80860be314a86128a02b60baa5f829643a2a9b92d18b7165c7f3f2435` | Generate |

Why each case expects what it does (the `corpus-smoke` gate, `mise run corpus-smoke` and CI's
job of that name, checks each expectation; this is the one place the reasons are written):

- `github-api-3-1` must clear the strict frontend with no carve and no omits. Its `spargen.toml`
  raises the batch cap so that a new terminal diagnostic cannot hide behind the warning volume.
- `github-api-3-0` is OpenAPI 3.0, rejected at the version gate (`E001`).
- `openai-openapi` is 3.1 and past the version gate, but `POST /realtime/calls` documents
  `application/sdp` as its only `201` content entry, and spargen has no codec for an unregistered
  media type (`E009`).
- `ollama` and `openapi-boilerplate` generate: `check` succeeds with no rejection.
- Stripe (3.0.0), Twilio (3.0.1) and a representative Kubernetes API-group document (3.0.0) pin
  the version gate (`E001`) on major APIs.
- `meilisearch` is genuine OpenAPI 3.1.0 and exercises strict official document validation past
  the gate, rejecting its null Tag `externalDocs.description` fields (`E011`).
- `mastodon-openapi` generates; its recursive-nullable `$ref`s and `$ref`-with-`type` siblings
  must keep clearing lowering. It is described below.

`mastodon-openapi` (#215) is genuine OpenAPI 3.1.0 that generates. It is the corpus's only
real-world instance of the recursive-nullable `$ref`, the standard 3.1 spelling of "optionally
another one of these": `oneOf: [{$ref: '#/components/schemas/Account'}, {type: 'null'}]` on
`Account.moved`, and the same through `Status` on `Status.reblog` and `Quote.quoted_status`.
`spargen/tests/snapshot.rs` asserts each lowers to `Option<Box<…>>`. The document is derived from
the GFDL-1.3 `mastodon/documentation` and declares that licence in its `info.license`, so the
licence text is vendored beside it as `mastodon-openapi/COPYING`; the generator repository itself
is MIT.

Deliberately not included: HoneyHive, IOTA gas-station, and Redocly.

## What a green corpus does not prove

A passing `corpus-smoke`, `corpus_manifest`, `snapshot`, or `recipes` run is evidence only for the
constructs these descriptions contain and that reach the code being changed. It is silent about
the rest. Five of the ten cases (and the `poem-openapi` recipe) are rejected before any schema is
lowered: `E001` for the four OpenAPI 3.0.x documents and the recipe, and `E011` document validation
for `meilisearch`. Schema lowering is reached only by `github-api-3-1`, `openai-openapi`, `ollama`,
`openapi-boilerplate`, `mastodon-openapi`, and the `utoipa`, `utoipa-untagged-overlap`, and `aide`
recipes.

**Schema Object `$ref` beside shape-bearing sibling keywords** (the intersection the support matrix
describes, and its `E013` rejections) is reached by `mastodon-openapi` and by one `oneOf` member of
`openai-openapi`. Mastodon carries 35 `$ref`s with a `type` sibling, each a string enum component
referenced beside `type: string` (33) or `type: [string, 'null']` (2);
`/components/schemas/Status/properties/visibility` is one. In the other pinned documents and
recipes, the only `$ref` with a sibling other than an annotation (`description`, `title`,
`deprecated`, `default`, 3.0's `nullable`) is `openai-openapi`'s
`/components/schemas/InputItem/oneOf/1`, which sets `type: object` beside
`$ref: '#/components/schemas/Item'`. A union member's `$ref` siblings were dropped before lowering
until [#279](https://github.com/getkono/spargen/issues/279) was fixed, and since then this member
reaches the intersection too. Every other intersection behaviour — an empty intersection, a
recursive target, and a non-empty one of any other shape — is pinned only by inline fixtures:
those in `spargen/tests/frontend/`, and the two `--compat` carve fixtures in
`spargen/tests/carve.rs` (`carve_removes_a_ref_whose_siblings_cannot_be_intersected` and
`carve_removes_a_recursive_ref_whose_siblings_bear_a_shape`), which carve away a `$ref`-sibling
`E013`.

Measured by running `spargen check` over every case and recipe that reaches lowering, with one
mutation of the lowering pass (then `spargen/src/oas31/lower.rs`, now the
`spargen/src/oas31/lower/` modules) at a time:

- Making every shape-bearing `$ref` sibling that reaches the intersection reject with `E013`
  rejects `mastodon-openapi` and adds an `E013` to the already-rejected
  `openai-openapi`, at `/components/schemas/InputItem/oneOf/1`; every other case and recipe keeps
  its outcome and histogram (measured on `master@38c1154`, where `corpus_manifest` and `snapshot`
  fail and `recipes` passes). On `master@b1961a4`, before #279 was fixed, only `mastodon-openapi`
  changed. Before `mastodon-openapi` was added this mutation left the whole corpus green (measured
  on `master@16eda2e`: `corpus_manifest`, `snapshot` and `recipes` passed with no snapshot
  changed).
- Rejecting the recursive-nullable collapse (a single real union member that closes a reference
  cycle, without sibling keywords) with `E013` rejects `mastodon-openapi`; every
  other case and recipe keeps its outcome (measured on `master@b1961a4`).

Before relying on a green corpus run for a change to a construct, check that some pinned
description contains that construct in a position that reaches the code. If none does, the
evidence has to come from `spargen/tests/frontend/`, `carve.rs`, and `e2e.rs` fixtures, or from a
new corpus case.
