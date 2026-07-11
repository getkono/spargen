# OpenAPI Specification References

These OpenAPI Specification texts are vendored from the Apache-2.0 licensed
[`OAI/OpenAPI-Specification`](https://github.com/OAI/OpenAPI-Specification)
repository for implementation reference and auditability. Product scope and
normative precedence remain defined in [`docs/prd.md`](../docs/prd.md); this
file defines which vendored spec texts to consult.

## Patch-Version Policy

The OAS 3.x specification text defines a `major.minor.patch` versioning scheme:
`major.minor` designates the OAS feature set, while patch versions address errors
or clarifications and do not change the feature set. The vendored revision
histories also identify every `3.0.x` and `3.1.x` patch after `.0` as a patch
release.

Upstream release notes are consistent with that policy:

- [OAS 3.0.4](https://github.com/OAI/OpenAPI-Specification/releases/tag/3.0.4)
  makes no changes to requirements from 3.0.3.
- [OAS 3.1.1](https://github.com/OAI/OpenAPI-Specification/releases/tag/3.1.1)
  makes no changes to requirements from 3.1.0.
- [OAS 3.1.2](https://github.com/OAI/OpenAPI-Specification/releases/tag/3.1.2)
  has no material changes.

Therefore, implementation work should use only the latest vendored patch for each
minor line. Older patch files stay in this directory only as historical audit
material when checking how wording changed.

## Active References

Use these as the sole active references for their minor lines:

| Minor line | Active reference | Use |
| --- | --- | --- |
| 3.0 | [`3.0.4.md`](3.0.4.md) | Historical comparison and 3.0 rejection/dialect-difference checks |
| 3.1 | [`3.1.2.md`](3.1.2.md) | OpenAPI 3.1 feature-set reference |
| 3.2 | [`3.2.0.md`](3.2.0.md) | Future-minor comparison |

Do not use `3.0.0.md`, `3.0.1.md`, `3.0.2.md`, `3.0.3.md`, `3.1.0.md`, or
`3.1.1.md` for implementation decisions unless the task is explicitly auditing
historical wording between patch releases.
