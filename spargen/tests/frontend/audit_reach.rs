//! What the per-keyword audit reaches: every schema position lowering can read gets the same
//! `W001` for a validation-only keyword and the same `E004` for a reference under a keyword
//! lowering never reads, wherever the position sits — under a Header Object, an Encoding Object,
//! or a Path Item reached through `$ref` (#715).

use super::*;

/// The pointer of every `code` diagnostic in `report`, in emission order. A fixture with a second
/// file gives its positions pointers no root position shares, so the pointer alone tells them apart.
fn sites(report: &Report, code: Code) -> Vec<String> {
    report
        .diagnostics()
        .iter()
        .filter(|d| d.code == code)
        .map(|d| d.pointer.as_str().to_owned())
        .collect()
}

/// Run both entry points over `files` and assert each is rejected exactly when `rejected` says,
/// and carries `code` at exactly the `expected` pointers, each once.
fn assert_sites(files: &[(&str, &str)], code: Code, rejected: bool, expected: &[&str]) {
    let (generated, checked, _) = generate_and_check_files(files);
    for (entry, report) in [("generate", generated), ("check", checked)] {
        assert_eq!(
            report.outcome() == Outcome::Rejected,
            rejected,
            "{entry}: {report:#?}"
        );
        let mut found = sites(&report, code);
        found.sort();
        let mut expected = expected.to_vec();
        expected.sort_unstable();
        assert_eq!(found, expected, "{entry}: {report:#?}");
    }
}

const HEAD: &str = "openapi: 3.1.0\n\
                    info: { title: T, version: 1.0.0 }\n\
                    servers: [{ url: 'https://e.com' }]\n";

/// The issue's reproducer: a response header, inline and through `$ref` into
/// `components.headers`, and a header described by `content` rather than `schema`. A component
/// header two responses reference is audited once.
#[test]
fn a_response_header_schema_gets_the_audit_a_parameter_schema_does() {
    let spec = format!(
        "{HEAD}paths:\n  \
         /a:\n    \
         get:\n      \
         operationId: getA\n      \
         responses:\n        \
         '200':\n          \
         description: ok\n          \
         headers:\n            \
         X-Count: {{ $ref: '#/components/headers/Count' }}\n            \
         X-Inline: {{ schema: {{ type: string, maxLength: 8 }} }}\n            \
         X-Content:\n              \
         content:\n                \
         text/plain: {{ schema: {{ type: string, pattern: '^a' }} }}\n        \
         '404':\n          \
         description: missing\n          \
         headers:\n            \
         X-Count: {{ $ref: '#/components/headers/Count' }}\n\
         components:\n  \
         headers:\n    \
         Count:\n      \
         schema: {{ type: integer, minimum: 1 }}\n"
    );
    assert_sites(
        &[("openapi.yaml", &spec)],
        Code::ValidationKeywordIgnored,
        false,
        &[
            "/components/headers/Count/schema",
            "/paths/~1a/get/responses/200/headers/X-Inline/schema",
            "/paths/~1a/get/responses/200/headers/X-Content/content/text~1plain/schema",
        ],
    );
}

/// A dangling reference under a header schema's `not`, which lowering never reads, is `E004` as it
/// is under a parameter's.
#[test]
fn a_dangling_reference_under_a_header_schema_is_unresolved() {
    let spec = format!(
        "{HEAD}paths:\n  \
         /a:\n    \
         get:\n      \
         operationId: getA\n      \
         responses:\n        \
         '200':\n          \
         description: ok\n          \
         headers:\n            \
         X-Count:\n              \
         schema: {{ type: integer, not: {{ $ref: '#/components/schemas/Missing' }} }}\n"
    );
    assert_sites(
        &[("openapi.yaml", &spec)],
        Code::UnresolvedRef,
        true,
        &["/paths/~1a/get/responses/200/headers/X-Count/schema/not"],
    );
}

/// An Encoding Object's headers: an inline one and one through `$ref` into `components.headers`.
#[test]
fn an_encoding_header_schema_gets_the_audit_a_parameter_schema_does() {
    let spec = format!(
        "{HEAD}paths:\n  \
         /upload:\n    \
         post:\n      \
         operationId: upload\n      \
         requestBody:\n        \
         content:\n          \
         multipart/form-data:\n            \
         schema:\n              \
         type: object\n              \
         properties:\n                \
         file: {{ type: string, format: binary }}\n            \
         encoding:\n              \
         file:\n                \
         headers:\n                  \
         X-Rate: {{ schema: {{ type: string, const: fast, maxLength: 9 }} }}\n                  \
         X-Tag: {{ $ref: '#/components/headers/Tag' }}\n      \
         responses:\n        \
         '204': {{ description: done }}\n\
         components:\n  \
         headers:\n    \
         Tag:\n      \
         schema: {{ type: string, const: t, pattern: '^t$' }}\n"
    );
    assert_sites(
        &[("openapi.yaml", &spec)],
        Code::ValidationKeywordIgnored,
        false,
        &[
            "/components/headers/Tag/schema",
            "/paths/~1upload/post/requestBody/content/multipart~1form-data/encoding/file/headers/X-Rate/schema",
        ],
    );
}

/// A Path Item reached through `$ref` — into `components.pathItems`, and into another file — has
/// its operations audited as an inline one's are. Two paths referencing the same item audit it
/// once.
#[test]
fn a_referenced_path_item_gets_the_audit_an_inline_one_does() {
    let root = format!(
        "{HEAD}paths:\n  \
         /a: {{ $ref: '#/components/pathItems/Shared' }}\n  \
         /b: {{ $ref: '#/components/pathItems/Shared' }}\n  \
         /c: {{ $ref: './item.yaml' }}\n\
         components:\n  \
         pathItems:\n    \
         Shared:\n      \
         get:\n        \
         parameters:\n          \
         - {{ name: q, in: query, schema: {{ type: string, maxLength: 3 }} }}\n        \
         responses:\n          \
         '200':\n            \
         description: ok\n            \
         content:\n              \
         application/json: {{ schema: {{ type: object, maxProperties: 2 }} }}\n"
    );
    let item = "post:\n  \
                operationId: postC\n  \
                requestBody:\n    \
                content:\n      \
                application/json: { schema: { type: object, minProperties: 1 } }\n  \
                responses:\n    \
                '204': { description: done }\n";
    assert_sites(
        &[("openapi.yaml", &root), ("item.yaml", item)],
        Code::ValidationKeywordIgnored,
        false,
        &[
            "/components/pathItems/Shared/get/parameters/0/schema",
            "/components/pathItems/Shared/get/responses/200/content/application~1json/schema",
            "/post/requestBody/content/application~1json/schema",
        ],
    );
}

/// A component Path Item or Header that nothing references is audited as an unreferenced component
/// Parameter is: its validation-only keyword still warns.
#[test]
fn an_unreferenced_component_path_item_or_header_is_audited() {
    let spec = format!(
        "{HEAD}paths: {{}}\n\
         components:\n  \
         headers:\n    \
         Unused: {{ schema: {{ type: integer, maximum: 9 }} }}\n  \
         pathItems:\n    \
         Unused:\n      \
         get:\n        \
         operationId: unused\n        \
         responses:\n          \
         '200':\n            \
         description: ok\n            \
         headers:\n              \
         X-Id: {{ schema: {{ type: string, minLength: 1 }} }}\n"
    );
    assert_sites(
        &[("openapi.yaml", &spec)],
        Code::ValidationKeywordIgnored,
        false,
        &[
            "/components/headers/Unused/schema",
            "/components/pathItems/Unused/get/responses/200/headers/X-Id/schema",
        ],
    );
}

/// OpenAPI 3.2's positional `prefixEncoding` and `itemEncoding` hold headers too. Multipart
/// rejects both (`E009`), and their header schemas are audited all the same, as every schema
/// position of a rejected document is.
#[test]
fn a_positional_encoding_header_schema_is_audited() {
    let spec = "openapi: 3.2.0\n\
                info: { title: T, version: 1.0.0 }\n\
                servers: [{ url: 'https://e.com' }]\n\
                paths:\n  \
                /upload:\n    \
                post:\n      \
                operationId: upload\n      \
                requestBody:\n        \
                content:\n          \
                multipart/mixed:\n            \
                schema: { type: array, items: { type: string } }\n            \
                prefixEncoding:\n              \
                - headers: { X-First: { schema: { type: string, const: a, maxLength: 1 } } }\n            \
                itemEncoding:\n              \
                headers: { X-Rest: { schema: { type: string, const: b, minLength: 1 } } }\n      \
                responses:\n        \
                '204': { description: done }\n";
    let base = "/paths/~1upload/post/requestBody/content/multipart~1mixed";
    assert_sites(
        &[("openapi.yaml", spec)],
        Code::ValidationKeywordIgnored,
        true,
        &[
            &format!("{base}/prefixEncoding/0/headers/X-First/schema"),
            &format!("{base}/itemEncoding/headers/X-Rest/schema"),
        ],
    );
}

/// A Path Item `$ref` chain that returns to itself is lowering's `E004`, which declines every
/// chained Path Item reference; the audit's walk over it terminates and reaches no schema twice.
#[test]
fn a_path_item_reference_cycle_terminates_the_audit() {
    let spec = format!(
        "{HEAD}paths:\n  \
         /a: {{ $ref: '#/components/pathItems/One' }}\n\
         components:\n  \
         pathItems:\n    \
         One: {{ $ref: '#/components/pathItems/Two' }}\n    \
         Two: {{ $ref: '#/components/pathItems/One' }}\n"
    );
    assert_sites(
        &[("openapi.yaml", &spec)],
        Code::UnresolvedRef,
        true,
        &["/paths/~1a"],
    );
}
