# Spargen Validation Corpus

This corpus is vendored for repeatable OpenAPI 3.1 implementation and regression checks. Large
JSON/YAML artifacts are stored through Git LFS.

Included public APIs:

| ID | Upstream | Revision | Path | SHA-256 | Expected |
| --- | --- | --- | --- | --- | --- |
| `github-api-3-1-strict` | `github/rest-api-description` | `v2.1.0` | `descriptions-next/api.github.com/api.github.com.json` | `212c0264968bf20b4415575509172800d041694f4a8c2d0120da502f678c377d` | Strict reject today; compatibility profile target |
| `github-api-3-0` | `github/rest-api-description` | `v2.1.0` | `descriptions/api.github.com/api.github.com.json` | `b138e9cdcf4ac29a23fea1f6579d2840668a5f3d41fe7f160b263bec590d2e3f` | Reject `E001` |
| `openai-openapi` | `openai/openai-openapi` | `5162af98d3147432c14680df789e8e12d4891e6b` | `openapi.yaml` | `74cbcf73838f4cd7e209b2d3f2e9ddc9fa155f21a44360b6fac7646a6d4f5f8b` | Reject unsupported media/features |
| `ollama` | `ollama/ollama` | `d47859ce495496196df211e939702364492a2b7f` | `docs/openapi.yaml` | `d54e2ef5c24a396662ca7222af31ab3e26a642efbd2bb060d921b9393e9fef87` | Generate |
| `openapi-boilerplate` | `dgarcia360/openapi-boilerplate` | `41630ba37b628c7bd871230f480f62f694607d3f` | `src/**` | tree `6a1a45fb44e25fb931c3c2b7c85d2b10b70cd82bae78b189b30035cca973c3e8`; root `580f8d9b131756c29dd82535a74f6948dc77e53377e2b3292b49d2778029d209` | Generate |

Explicitly not included per planning constraints: HoneyHive, IOTA gas-station, and Redocly.
