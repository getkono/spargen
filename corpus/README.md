# Spargen Validation Corpus

This corpus is vendored for repeatable OpenAPI 3.1 implementation and regression checks. Large
JSON/YAML artifacts are stored through Git LFS.

Included public APIs:

| ID | Upstream | Revision | Path | SHA-256 | Expected |
| --- | --- | --- | --- | --- | --- |
| `github-api-3-1` | `github/rest-api-description` | `03ca9c1cac754ec9b8369dc75de8a8c753c6e087` | `descriptions-next/api.github.com/api.github.com.json` | `d88008d8198becda210d59fbe64a6554bcc4c979be2348e2e356638b369eee47` | Generate |
| `github-api-3-0` | `github/rest-api-description` | `v2.1.0` | `descriptions/api.github.com/api.github.com.json` | `b138e9cdcf4ac29a23fea1f6579d2840668a5f3d41fe7f160b263bec590d2e3f` | Reject `E001` |
| `openai-openapi` | `openai/openai-openapi` | `5162af98d3147432c14680df789e8e12d4891e6b` | `openapi.yaml` | `74cbcf73838f4cd7e209b2d3f2e9ddc9fa155f21a44360b6fac7646a6d4f5f8b` | Reject unsupported media/features |
| `ollama` | `ollama/ollama` | `d47859ce495496196df211e939702364492a2b7f` | `docs/openapi.yaml` | `d54e2ef5c24a396662ca7222af31ab3e26a642efbd2bb060d921b9393e9fef87` | Generate |
| `openapi-boilerplate` | `dgarcia360/openapi-boilerplate` | `41630ba37b628c7bd871230f480f62f694607d3f` | `src/**` | tree `6a1a45fb44e25fb931c3c2b7c85d2b10b70cd82bae78b189b30035cca973c3e8`; root `580f8d9b131756c29dd82535a74f6948dc77e53377e2b3292b49d2778029d209` | Generate |
| `stripe` | `stripe/openapi` | `d5d11f661d1180a847d6e26774517756e6a493a1` | `openapi/spec3.json` | `e24a26de4188fd64dec4c043d5d3726277fdcb07556a493ea481c305b0a223d8` | Reject `E001` (OpenAPI 3.0.0) |
| `twilio-api-2010` | `twilio/twilio-oai` | `bb6288e9f540d2d63540bbaadf6b73fd262c2df3` | `spec/json/twilio_api_v2010.json` | `a6753266b8b05a201e8658734e332ee51d07a0913f2d419335d87bdb287643a2` | Reject `E001` (OpenAPI 3.0.1) |
| `kubernetes-authentication-v1` | `kubernetes/kubernetes` | `fb3cf74c50ec5d117a7d17f1115c9413fd492c3d` | `api/openapi-spec/v3/apis__authentication.k8s.io__v1_openapi.json` | `443427d822f77db77202c96df06d453845abc5cc67390180129a67e6c74d421e` | Reject `E001` (OpenAPI 3.0.0) |
| `meilisearch` | `meilisearch/open-api` | `a2bd2133ac9f9b85fca8fb8b1aa69063c8f1002c` | `open-api.json` | `83cbd10cea1ca75590dc31f1d2e40ef2b636297d47b39c9aefd813e41454cfd1` | Reject `E013` (OpenAPI 3.1.0; clears the gate, real pipeline) |

The last four are pinned real-world APIs added to broaden coverage: Stripe, Twilio and a
representative Kubernetes API-group document are still OpenAPI 3.0.x/3.0.1, so they pin the version
gate (`E001`) on major APIs; `meilisearch` is genuine OpenAPI 3.1.0 and exercises the full pipeline
past the gate, rejecting on incompatible typed intersections (`E013`; unsupported media also remains).

Explicitly not included per planning constraints: HoneyHive, IOTA gas-station, and Redocly.
