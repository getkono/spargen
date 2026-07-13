# Corpus

The validation corpus is vendored under [`corpus/`](../corpus/README.md) and large JSON/YAML files
are stored through Git LFS. The machine-readable pin list is
[`corpus/manifest.toml`](../corpus/manifest.toml).

Current cases:

- `github-api-3-1-strict`: `github/rest-api-description@v2.1.0`,
  `descriptions-next/api.github.com/api.github.com.json`, SHA-256
  `212c0264968bf20b4415575509172800d041694f4a8c2d0120da502f678c377d`.
- `github-api-3-0`: `github/rest-api-description@v2.1.0`,
  `descriptions/api.github.com/api.github.com.json`, SHA-256
  `b138e9cdcf4ac29a23fea1f6579d2840668a5f3d41fe7f160b263bec590d2e3f`.
- `openai-openapi`: `openai/openai-openapi@5162af98d3147432c14680df789e8e12d4891e6b`,
  `openapi.yaml`, SHA-256
  `74cbcf73838f4cd7e209b2d3f2e9ddc9fa155f21a44360b6fac7646a6d4f5f8b`.
- `ollama`: `ollama/ollama@d47859ce495496196df211e939702364492a2b7f`,
  `docs/openapi.yaml`, SHA-256
  `d54e2ef5c24a396662ca7222af31ab3e26a642efbd2bb060d921b9393e9fef87`.
- `openapi-boilerplate`:
  `dgarcia360/openapi-boilerplate@41630ba37b628c7bd871230f480f62f694607d3f`,
  `src/**`, tree SHA-256
  `6a1a45fb44e25fb931c3c2b7c85d2b10b70cd82bae78b189b30035cca973c3e8` and root
  `src/openapi.yaml` SHA-256
  `580f8d9b131756c29dd82535a74f6948dc77e53377e2b3292b49d2778029d209`.
- `stripe`: `stripe/openapi@d5d11f661d1180a847d6e26774517756e6a493a1`,
  `openapi/spec3.json`, SHA-256
  `e24a26de4188fd64dec4c043d5d3726277fdcb07556a493ea481c305b0a223d8` (OpenAPI 3.0.0 → reject
  `E001`).
- `twilio-api-2010`: `twilio/twilio-oai@bb6288e9f540d2d63540bbaadf6b73fd262c2df3`,
  `spec/json/twilio_api_v2010.json`, SHA-256
  `a6753266b8b05a201e8658734e332ee51d07a0913f2d419335d87bdb287643a2` (OpenAPI 3.0.1 → reject
  `E001`).
- `kubernetes-authentication-v1`:
  `kubernetes/kubernetes@fb3cf74c50ec5d117a7d17f1115c9413fd492c3d`,
  `api/openapi-spec/v3/apis__authentication.k8s.io__v1_openapi.json`, SHA-256
  `443427d822f77db77202c96df06d453845abc5cc67390180129a67e6c74d421e` (a representative single
  API-group document; OpenAPI 3.0.0 → reject `E001`).
- `meilisearch`: `meilisearch/open-api@a2bd2133ac9f9b85fca8fb8b1aa69063c8f1002c`,
  `open-api.json`, SHA-256
  `83cbd10cea1ca75590dc31f1d2e40ef2b636297d47b39c9aefd813e41454cfd1` (OpenAPI 3.1.0; clears the
  version gate and exercises the pipeline, rejecting on non-disjoint `integer | number` unions →
  `E007`).

Excluded by decision: HoneyHive, IOTA gas-station, and Redocly.

Fast corpus smoke checks:

```bash
cargo run -q -p spargen -- check corpus/github-api-3-0/api.github.com.json --format json  # reject:E001
cargo run -q -p spargen -- check corpus/ollama/openapi.yaml --format json  # expected: generates (W001 only)
cargo run -q -p spargen -- check corpus/openapi-boilerplate/src/openapi.yaml --format json
cargo run -q -p spargen -- check corpus/stripe/spec3.json --format json  # reject:E001 (3.0.0)
cargo run -q -p spargen -- check corpus/twilio-api-2010/twilio_api_v2010.json --format json  # reject:E001 (3.0.1)
cargo run -q -p spargen -- check corpus/kubernetes-authentication-v1/apis__authentication.k8s.io__v1_openapi.json --format json  # reject:E001 (3.0.0)
# meilisearch is 3.1.0: it clears the version gate and rejects with E007 deep in the pipeline.
# The default batch_cap (100) fills with W001, so raise it to surface the E007 error:
cargo run -q -p spargen -- check corpus/meilisearch/open-api.json --format json --config <(printf '[features]\nbatch_cap = 100000\n')  # reject:E007 (3.1.0)
```

The full GitHub 3.1 strict/compat split remains the large-corpus release gate: strict mode should
fail loudly today, and a reviewed omit profile will define the generated subset.
