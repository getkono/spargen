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

Excluded by decision: HoneyHive, IOTA gas-station, and Redocly.

Fast corpus smoke checks:

```bash
cargo run -q -p spargen -- check corpus/github-api-3-0/api.github.com.json --format json
cargo run -q -p spargen -- check corpus/ollama/openapi.yaml --format json  # expected: E007
cargo run -q -p spargen -- check corpus/openapi-boilerplate/src/openapi.yaml --format json
```

The full GitHub 3.1 strict/compat split remains the large-corpus release gate: strict mode should
fail loudly today, and a reviewed omit profile will define the generated subset.
