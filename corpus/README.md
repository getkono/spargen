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

The five after `openapi-boilerplate` are pinned real-world APIs added to broaden coverage: Stripe,
Twilio and a representative Kubernetes API-group document are still OpenAPI 3.0.x/3.0.1, so they
pin the version gate (`E001`) on major APIs; `meilisearch` is genuine OpenAPI 3.1.0 and exercises
strict official document validation past the gate, rejecting its null Tag
`externalDocs.description` fields (`E011`); `mastodon-openapi`, the fifth, is described below.

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

### Which diagnostic emission sites the corpus notices

The snapshot histograms show which codes the corpus *emits*, so they catch a site that stops
firing. Whether the corpus would notice a site that starts *over*-firing depends on a different
fact: whether some pinned description reaches the site with its condition false. This table records
that for every emission site in `spargen/src/oas31` and `spargen/src/source` (the frontend; codes
emitted only by `codegen`, `compat`, `name` or the facade are out of scope).

Measured on `master@38c1154`, and not re-measured since: no test holds this table, so a site added,
moved, or regated after that commit may be reached differently or missing from it. Only the
*Fired by the corpus* column is held: its manifest cases by the snapshot histograms, and its
recipe cells (`poem-openapi`'s `E001`, `aide`'s `W001`) by `recipes.rs`. An emission site is one `Code::`
construction outside a `#[cfg(test)]` module. The mutation for a site makes it
fire whenever the statement that selects it is evaluated: the innermost `if` or `let … else`
condition, or the `match` whose arm it is, with any early exit ahead of it in the same block
skipped. For a helper that only builds a diagnostic (`reject_all_of_cycle`, `reject_unpinned`,
`duplicate_key_error`, …) that statement is at its call sites, and the helper counts as noticed if
any of them is. A site is **noticed** when some description evaluates that statement more often than
the site already fires, on a run that observes the result:

- all ten manifest cases, through `snapshot.rs`'s uncapped histograms (errors and warnings);
- the recipes, through `recipes.rs`, which observe errors, plus warnings other than `W001` in
  `aide`'s case; the other recipes' warnings are not asserted.

Which statements each description evaluates was read from source-based coverage
(`cargo +stable llvm-cov`) of `spargen check --batch-cap 1000000` over each case and recipe on its
own, and of the `corpus_manifest`, `snapshot` and `recipes` suites; the two agree on every site.
Real mutations run through the three suites checked the method. Forcing seven sites marked not
noticed, all at once, left all three green (the frontend suite, then the single file `frontend.rs`
and now `spargen/tests/frontend/`, failed under the same mutations, so they were live). Forcing a
noticed site failed exactly the tests of the descriptions listed as reaching it, for each of four: `W002` for `callbacks` (five snapshots and the `aide` recipe),
`E004` for a Path Item `$ref` hop (`openapi-boilerplate` in `corpus_manifest` and `snapshot`),
`W011` for a second per-operation `servers` entry (the `github-api-3-1` snapshot), and the
`$ref`-sibling `E013` above (`mastodon-openapi` and `openai-openapi`). The `E004` and `W011`
mutations shared one run; no description reaches both.

Per code, where `—` in the last column means every site of the code was reached at that commit:

| Code | Fired by the corpus | Sites no pinned description reaches (at `master@38c1154`) |
| --- | --- | --- |
| `E001` | the four 3.0 cases, `poem-openapi` recipe | — |
| `E002` | — | root `jsonSchemaDialect` not the OAS dialect; a schema `$schema` naming another dialect |
| `E003` | — | `spargen lock` meeting an unfetchable remote scheme (`vendor.rs`) |
| `E004` | — | remote alias cycle (`ensure_remote`); bundle alias cycle (`ensure_resolved`); both arms of `reject_unfollowable_reference`; a Security Scheme `$ref` that does not resolve |
| `E005` | — | `patternProperties` beside `additionalProperties: false`; heterogeneous `patternProperties` value types |
| `E006` | — | — |
| `E007` | — | — |
| `E008` | — | — |
| `E009` | `openai-openapi` | a `content` parameter under a media other than JSON or text; a non-object form-urlencoded request body; `prefixEncoding`/`itemEncoding` on multipart; an Encoding Object style outside the four, delimited with `explode: true`, `deepObject` in multipart, or an object property in multipart; an XML hint on a type serialized as XML |
| `E010` | — | the `in: querystring` sites (no content, unsupported media, no schema, non-object form body) |
| `E011` | `meilisearch` | an `additionalOperations` method colliding with a fixed field; a tag `parent` cycle and an unknown `parent`; `xml.nodeType` beside `attribute`/`wrapped`; a duplicate path-item parameter; a vendored remote that is not UTF-8; a malformed `spargen.lock`; `spargen lock` I/O failures |
| `E012` | — | — |
| `E013` | — | an all-scalar `allOf` with no common value (`reject_all_of_scalars`) |
| `E014` | — | — |
| `E015` | — | `prefixItems` beside a typed `items` rest |
| `E016` | — | — |
| `E021` | — | a vendored remote missing or drifted from its pin |
| `E022` | — | — |
| `E025` | — | a failed `spargen lock` fetch |
| `W001` | `github-api-3-1`, `openai-openapi`, `ollama`, `mastodon-openapi`, `aide` recipe | — |
| `W002` | `github-api-3-1`, `openai-openapi`, `mastodon-openapi` | — |
| `W005` | `github-api-3-1`, `openai-openapi` | a `default` beside an annotation-only component `$ref` (`ensure_component`) |
| `W006` | — | the `XmlHintIgnored` sites |
| `W010` | — | `itemSchema` on a response header's `content` |
| `W011` | `github-api-3-1`, `openai-openapi` | cases `positional-encoding-form`, `allow-reserved-multipart`, `encoding-headers-non-multipart`, `encoding-header-no-value`, and the `response-header-untyped` sites under a header's `content` |
| `W014` | `github-api-3-1`, `openai-openapi`, `ollama` | — |

A noticed site can rest on one description. At the same commit these were noticed by exactly one,
so removing or re-pinning it would leave them unguarded:

- `openapi-boilerplate`: the `E004` sites in `resolve.rs`, `chain_component_alias`'s cycle, and the
  Path Item `$ref` hop, and `E016`'s Path Item `$ref` siblings.
- `openai-openapi`: the `E009` sites for a nested `encoding`, a `contentType` range, a malformed
  `contentType`, and an unsendable `contentType` parameter, and `W011`'s
  `encoding-unknown-property`.
- `mastodon-openapi`: `E009` for a sequential response's `schema` under OpenAPI 3.2, the
  `E011` server-variable checks, and `W011`'s `unused-server-variable`.
- `github-api-3-1`: `W011`'s `extra-servers`.

The sites listed as not reached get no evidence at all from a green corpus run, in either direction;
only `spargen/tests/frontend/` and the other inline fixtures pin them. Re-measure this table when a
case is added or re-pinned, and when a change moves a site or its gate.

Before relying on a green corpus run for a change to a construct, check that some pinned
description contains that construct in a position that reaches the code. If none does, the
evidence has to come from `spargen/tests/frontend/`, `carve.rs`, and `e2e.rs` fixtures, or from a
new corpus case.
