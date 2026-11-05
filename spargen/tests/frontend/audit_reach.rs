//! What the per-keyword audit reaches: every schema position lowering can read gets the same
//! `W001` for a validation-only keyword and the same `E004` for a reference under a keyword
//! lowering never reads, wherever the position sits — under a Header Object, an Encoding Object,
//! or a Path Item reached through `$ref` (#715). A reference under a validation-only keyword is
//! resolved, into other files too, and a discriminator there has its mapping resolved; a back edge
//! only through `contentSchema`, which lowering never descends into, closes no cycle.

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
// W001 case: validation-keyword
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

/// An OpenAPI 3.2 SSE envelope whose `data` `contentMediaType`/`contentSchema` lowering consumes
/// is no `W001` under a referenced or component Path Item, now that the audit reaches it there,
/// just as it is none under an inline one; the envelope's own validation keyword still warns.
#[test]
fn a_consumed_sse_envelope_under_a_referenced_path_item_does_not_warn() {
    let spec = "openapi: 3.2.0\n\
                info: { title: T, version: 1.0.0 }\n\
                servers: [{ url: 'https://e.com' }]\n\
                paths:\n  \
                /events: { $ref: '#/components/pathItems/Events' }\n  \
                /more: { $ref: './item.yaml' }\n\
                components:\n  \
                pathItems:\n    \
                Events:\n      \
                get:\n        \
                operationId: events\n        \
                responses:\n          \
                '200':\n            \
                description: ok\n            \
                content:\n              \
                text/event-stream:\n                \
                itemSchema:\n                  \
                type: object\n                  \
                required: [data]\n                  \
                properties:\n                    \
                data:\n                      \
                type: string\n                      \
                maxLength: 99\n                      \
                contentMediaType: application/json\n                      \
                contentSchema: { type: object, properties: { id: { type: string } } }\n";
    let item = "get:\n  \
                operationId: more\n  \
                responses:\n    \
                '200':\n      \
                description: ok\n      \
                content:\n        \
                text/event-stream:\n          \
                itemSchema:\n            \
                type: object\n            \
                required: [data]\n            \
                properties:\n              \
                data:\n                \
                type: string\n                \
                contentMediaType: application/json\n                \
                contentSchema: { type: object, properties: { id: { type: string } } }\n";
    assert_sites(
        &[("openapi.yaml", spec), ("item.yaml", item)],
        Code::ValidationKeywordIgnored,
        false,
        &["/components/pathItems/Events/get/responses/200/content/text~1event-stream/itemSchema/properties/data"],
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

/// `generate_and_check_files` over `root` and `lib.yaml`, as `(entry, report)` pairs.
fn root_and_lib(root: &str, lib: &str) -> [(&'static str, Report); 2] {
    let (generated, checked, _) =
        generate_and_check_files(&[("openapi.yaml", root), ("lib.yaml", lib)]);
    [("generate", generated), ("check", checked)]
}

/// A reachable `A` whose only content under test sits at `site` (indented as a schema keyword of
/// `A`), with `Present` and `Wrapper` declared so a control can name something that exists.
fn validation_ref_spec(site: &str) -> String {
    format!(
        "openapi: 3.1.0\n\
         info: {{ title: T, version: 1.0.0 }}\n\
         servers: [{{ url: 'https://e.com' }}]\n\
         paths:\n  \
         /a:\n    \
         get:\n      \
         operationId: getA\n      \
         responses:\n        \
         '200':\n          \
         description: ok\n          \
         content:\n            \
         application/json:\n              \
         schema: {{ $ref: '#/components/schemas/A' }}\n\
         components:\n  \
         schemas:\n    \
         Present: {{ type: string }}\n    \
         Wrapper:\n      \
         type: object\n      \
         properties:\n        \
         w: {{ type: integer }}\n    \
         A:\n      \
         type: object\n\
         {site}"
    )
}

/// The keyword positions lowering never reads (#424): `not`, `if`/`then`/`else`, `contains`,
/// `propertyNames`, `unevaluated*`, `dependentSchemas`, an unconsumed `contentSchema`, and an
/// unreferenced `$defs` entry, each at depth one and nested inside another such subtree. `{target}`
/// is the reference each one carries.
fn unlowered_ref_sites(target: &str) -> Vec<(&'static str, String, String)> {
    let at = "/components/schemas/A";
    vec![
        (
            "not",
            format!("      not: {{ $ref: '{target}' }}\n"),
            format!("{at}/not"),
        ),
        (
            "if",
            format!("      if: {{ $ref: '{target}' }}\n"),
            format!("{at}/if"),
        ),
        (
            "then",
            format!("      then: {{ $ref: '{target}' }}\n"),
            format!("{at}/then"),
        ),
        (
            "else",
            format!("      else: {{ $ref: '{target}' }}\n"),
            format!("{at}/else"),
        ),
        (
            "contains",
            format!(
                "      properties:\n        l:\n          type: array\n          items: {{ type: string }}\n          contains: {{ $ref: '{target}' }}\n"
            ),
            format!("{at}/properties/l/contains"),
        ),
        (
            "propertyNames",
            format!("      propertyNames: {{ $ref: '{target}' }}\n"),
            format!("{at}/propertyNames"),
        ),
        (
            "unevaluatedProperties",
            format!("      unevaluatedProperties: {{ $ref: '{target}' }}\n"),
            format!("{at}/unevaluatedProperties"),
        ),
        (
            "unevaluatedItems",
            format!(
                "      properties:\n        l:\n          type: array\n          unevaluatedItems: {{ $ref: '{target}' }}\n"
            ),
            format!("{at}/properties/l/unevaluatedItems"),
        ),
        (
            "dependentSchemas",
            format!("      dependentSchemas:\n        x: {{ $ref: '{target}' }}\n"),
            format!("{at}/dependentSchemas/x"),
        ),
        (
            "contentSchema",
            format!(
                "      properties:\n        s:\n          type: string\n          contentMediaType: application/json\n          contentSchema: {{ $ref: '{target}' }}\n"
            ),
            format!("{at}/properties/s/contentSchema"),
        ),
        (
            "an unreferenced $defs entry",
            format!("      $defs:\n        D: {{ $ref: '{target}' }}\n"),
            format!("{at}/$defs/D"),
        ),
        (
            "a property inside not",
            format!(
                "      not:\n        properties:\n          y: {{ $ref: '{target}' }}\n"
            ),
            format!("{at}/not/properties/y"),
        ),
        (
            "an allOf member inside if",
            format!("      if:\n        allOf:\n          - {{ $ref: '{target}' }}\n"),
            format!("{at}/if/allOf/0"),
        ),
        (
            "a not inside contains",
            format!(
                "      properties:\n        l:\n          type: array\n          contains:\n            not: {{ $ref: '{target}' }}\n"
            ),
            format!("{at}/properties/l/contains/not"),
        ),
    ]
}

/// #424: a `$ref` under a keyword lowering never reads was never resolved, so a dangling one
/// audited clean beside only the parent's `W001`, where the same reference under `properties` is
/// `E004`. The subschema is still not lowered, but its references are resolved, and one naming
/// nothing is `E004` at the reference through both entry points — whether it is a component name
/// the document does not declare or a pointer into a file that holds nothing there.
#[test]
fn a_dangling_ref_under_a_validation_only_keyword_is_e004() {
    for target in [
        "#/components/schemas/Missing",
        "#/components/schemas/A/nothing",
    ] {
        for (what, site, pointer) in unlowered_ref_sites(target) {
            let spec = validation_ref_spec(&site);
            for (entry, report) in [("generate", generate(&spec)), ("check", check(&spec))] {
                assert_eq!(
                    report.outcome(),
                    Outcome::Rejected,
                    "{target} under {what} through {entry}: {report:#?}"
                );
                assert!(
                    report
                        .diagnostics()
                        .iter()
                        .any(|d| d.code == Code::UnresolvedRef && d.pointer.as_str() == pointer),
                    "{target} under {what} through {entry}: E004 must sit at {pointer}: \
                     {report:#?}"
                );
            }
        }
    }
}

/// The control for #424: the same positions naming a schema that exists — a root component, a
/// pointer into the document, a sub-file — still resolve and generate with no `E004`, and the
/// subschema is still not lowered, so the emitted types are those of the document without it.
#[test]
fn a_resolvable_ref_under_a_validation_only_keyword_stays_clean() {
    let (_, base_code) = generate_with_code(&validation_ref_spec(""));
    let base_types = types_module(&base_code);
    for target in [
        "#/components/schemas/Present",
        "#/components/schemas/Wrapper/properties/w",
        "#/components/schemas/A",
    ] {
        for (what, site, _) in unlowered_ref_sites(target) {
            let spec = validation_ref_spec(&site);
            let (report, code) = generate_with_code(&spec);
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "{target} under {what}: {report:#?}"
            );
            assert!(
                !has_code(&report, Code::UnresolvedRef),
                "{target} under {what}: {report:#?}"
            );
            // `contains`/`unevaluatedItems` add an array property and `contentSchema` a string
            // one, so only the positions that add no shape are compared with the baseline.
            if !site.contains("properties:") {
                assert_eq!(
                    types_module(&code),
                    base_types,
                    "{target} under {what} changed the emitted types, so it was lowered"
                );
            }
        }
    }
}

/// #424 across files: a target reached only through `not` is never lowered, so its own references
/// are resolved by following it — a dangling one inside it is `E004` at that reference, in the
/// file it is written in. A cycle of such targets terminates. A bare `#/components/schemas/<name>`
/// written in a sub-file names the root's component when the root declares one, as lowering reads
/// it, so it is not reported against the sub-file.
#[test]
fn a_ref_under_a_validation_only_keyword_is_followed_into_other_files() {
    let root = validation_ref_spec("      not: { $ref: './lib.yaml#/Outer' }\n");

    let dangling = "Outer:\n  type: object\n  properties:\n    y: { $ref: '#/Gone' }\n";
    for (entry, report) in root_and_lib(&root, dangling) {
        assert_eq!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        assert!(
            report
                .diagnostics()
                .iter()
                .any(|d| d.code == Code::UnresolvedRef
                    && d.pointer.as_str() == "/Outer/properties/y"),
            "{entry}: E004 must sit at lib.yaml's /Outer/properties/y: {report:#?}"
        );
    }

    let resolvable = "Outer:\n  \
                      type: object\n  \
                      properties:\n    \
                      y: { $ref: '#/Inner' }\n    \
                      z: { $ref: '#/components/schemas/Present' }\n\
                      Inner:\n  \
                      not: { $ref: '#/Outer' }\n";
    for (entry, report) in root_and_lib(&root, resolvable) {
        assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        assert!(
            !has_code(&report, Code::UnresolvedRef),
            "{entry}: {report:#?}"
        );
    }

    // A bare `#/components/schemas/<name>` written in a sub-file that the sub-file declares and
    // the root does not addresses the sub-file, as lowering reads it: only a reference written in
    // the root is a missing root component when the root does not declare its name.
    let local_component = "Outer:\n  \
                           type: object\n  \
                           properties:\n    \
                           y: { $ref: '#/components/schemas/Local' }\n\
                           components:\n  \
                           schemas:\n    \
                           Local: { type: string }\n";
    for (entry, report) in root_and_lib(&root, local_component) {
        assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        assert!(
            !has_code(&report, Code::UnresolvedRef),
            "{entry}: a sub-file's own component is not a missing root component: {report:#?}"
        );
    }

    let missing_file = validation_ref_spec("      not: { $ref: './absent.yaml#/Outer' }\n");
    let report = generate(&missing_file);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
}

/// #424: a `discriminator` inside a subschema lowering never reads was never checked, so a
/// `mapping` or `defaultMapping` value naming no schema went unreported. Each is resolved as a
/// lowered discriminator's would be, and one naming nothing is `E004` at the entry; a value naming
/// a schema that exists is not.
#[test]
fn a_discriminator_under_a_validation_only_keyword_resolves_its_mapping() {
    let site = |target: &str| {
        format!(
            "      else:\n        \
             discriminator:\n          \
             propertyName: kind\n          \
             mapping:\n            \
             p: {target}\n          \
             defaultMapping: Present\n        \
             oneOf:\n          \
             - {{ $ref: '#/components/schemas/Present' }}\n"
        )
    };
    let dangling = validation_ref_spec(&site("Missing"));
    for (entry, report) in [
        ("generate", generate(&dangling)),
        ("check", check(&dangling)),
    ] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        let e004: Vec<_> = report
            .diagnostics()
            .iter()
            .filter(|d| d.code == Code::UnresolvedRef)
            .map(|d| d.pointer.as_str())
            .collect();
        assert_eq!(
            e004,
            ["/components/schemas/A/else/discriminator/mapping/p"],
            "{entry}: only the dangling mapping entry is E004: {report:#?}"
        );
    }
    for target in ["Present", "'#/components/schemas/Present'"] {
        let clean = validation_ref_spec(&site(target));
        let report = generate(&clean);
        assert_ne!(report.outcome(), Outcome::Rejected, "{target}: {report:#?}");
        assert!(
            !has_code(&report, Code::UnresolvedRef),
            "{target}: {report:#?}"
        );
    }
}

/// #698: the cycle walk followed `contentSchema` as an edge, but lowering a schema clears
/// `contentSchema` rather than descending into it, so a target that reached back only through a
/// `contentSchema` made a `$ref` with shape-bearing siblings look cycle-closing, and the document
/// was rejected as `E013` although no generated type recurses through that edge. Each document
/// below reaches back only that way and generates through both entry points: the issue's
/// reproducer, the same back edge under a property, and an OpenAPI 3.2 `text/event-stream`
/// envelope, where the `data.contentSchema` the SSE codec consumes is lowered as the stream's item
/// type on its own, not inside the envelope it is written in.
#[test]
fn a_back_edge_only_through_content_schema_closes_no_cycle() {
    let head = "info: { title: T, version: 1.0.0 }\nservers: [{ url: 'https://e.com' }]\n";
    let cases = [
        (
            "the issue's reproducer",
            format!(
                "openapi: 3.1.0\n{head}paths: {{}}\n\
                 components:\n  \
                 schemas:\n    \
                 A: {{ $ref: '#/components/schemas/B', type: string }}\n    \
                 B:\n      \
                 type: string\n      \
                 contentSchema: {{ $ref: '#/components/schemas/A' }}\n"
            ),
        ),
        (
            "a contentSchema under a property",
            format!(
                "openapi: 3.1.0\n{head}paths: {{}}\n\
                 components:\n  \
                 schemas:\n    \
                 Holder:\n      \
                 type: object\n      \
                 properties:\n        \
                 l:\n          \
                 $ref: '#/components/schemas/Leaf'\n          \
                 type: object\n          \
                 properties: {{ x: {{ type: string }} }}\n    \
                 Leaf:\n      \
                 type: object\n      \
                 properties:\n        \
                 s:\n          \
                 type: string\n          \
                 contentMediaType: application/json\n          \
                 contentSchema: {{ $ref: '#/components/schemas/Holder' }}\n"
            ),
        ),
        (
            "an SSE envelope's consumed data.contentSchema",
            format!(
                "openapi: 3.2.0\n{head}\
                 paths:\n  \
                 /events:\n    \
                 get:\n      \
                 operationId: events\n      \
                 responses:\n        \
                 '200':\n          \
                 description: ok\n          \
                 content:\n            \
                 text/event-stream:\n              \
                 itemSchema: {{ $ref: '#/components/schemas/Envelope' }}\n\
                 components:\n  \
                 schemas:\n    \
                 Envelope:\n      \
                 type: object\n      \
                 required: [data]\n      \
                 properties:\n        \
                 data:\n          \
                 type: string\n          \
                 contentMediaType: application/json\n          \
                 contentSchema:\n            \
                 $ref: '#/components/schemas/Payload'\n            \
                 type: object\n            \
                 properties: {{ x: {{ type: string }} }}\n    \
                 Payload:\n      \
                 type: object\n      \
                 properties:\n        \
                 env: {{ $ref: '#/components/schemas/Envelope' }}\n"
            ),
        ),
    ];
    for (what, spec) in &cases {
        let report = generate(spec);
        assert_ne!(
            report.outcome(),
            Outcome::Rejected,
            "{what}: lowering never descends into `contentSchema`, so it \
             closes no cycle: {report:#?}"
        );
        assert!(
            !has_code(&report, Code::AllOfIrreconcilable),
            "{what}: {report:#?}"
        );
    }
    // The envelope's `data.contentSchema` is still the one consumed as the stream's item type.
    let (_, code) = generate_with_code(&cases[2].1);
    let flat = code.split_whitespace().collect::<String>();
    assert!(
        flat.contains("EventStream<types::"),
        "the SSE envelope's contentSchema must stay the stream's item type: {code}"
    );
}
