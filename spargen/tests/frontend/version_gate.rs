//! The Schema Object fields only OpenAPI 3.2 defines, `discriminator.defaultMapping` and
//! `xml.nodeType`, rejected in a 3.1 document (`E011`) wherever a schema the document reaches
//! carries them, and generated from the same document declaring 3.2. The 3.2 Path Item fields'
//! rejection, which the 3.1 document schema reports, is pinned in `document.rs`.

use super::*;

/// `defaultMapping` is a Discriminator Object field only OpenAPI 3.2 defines, and a 3.1 document
/// using it got the 3.2 fallback branch with no diagnostic (#1448). It is `E011` at the field,
/// naming the version that introduces it, as a 3.2 Path Item field is in a 3.1 document; the same
/// document declaring 3.2 generates that fallback.
#[test]
fn e011_an_oas31_discriminator_default_mapping_is_rejected_as_needing_oas32() {
    let spec = |version: &str| {
        format!(
            "openapi: {version}\ninfo: {{ title: T, version: 1.0.0 }}\npaths: {{}}\ncomponents:\n  \
             schemas:\n    \
             Cat: {{ type: object, required: [kind], properties: {{ kind: {{ type: string }} }} }}\n    \
             Other: {{ type: object, properties: {{ x: {{ type: string }} }} }}\n    \
             Pet:\n      \
             oneOf: [{{ $ref: '#/components/schemas/Cat' }}, {{ $ref: '#/components/schemas/Other' }}]\n      \
             discriminator: {{ propertyName: kind, defaultMapping: Other }}\n"
        )
    };
    let report = generate(&spec("3.1.0"));
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    let rejected: Vec<_> = report
        .diagnostics()
        .iter()
        .filter(|d| d.code == Code::InvalidInput)
        .map(|d| (d.pointer.as_str(), d.message.as_str()))
        .collect();
    assert_eq!(
        rejected,
        [(
            "/components/schemas/Pet/discriminator/defaultMapping",
            "`defaultMapping` is an OpenAPI 3.2 Discriminator Object field, and this document \
             declares 3.1"
        )],
        "{report:#?}"
    );

    // `Pet` is the document's one union, so no unknown-tag error anywhere means its fallback is
    // generated.
    let (report, code) = generate_with_code(&spec("3.2.0"));
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::InvalidInput), "{report:#?}");
    assert!(!code.contains("unknown discriminator value"), "{code}");
}

/// The version gate reads every schema the document reaches, so a 3.2-only `defaultMapping` in a
/// file a 3.1 root `$ref`s is rejected there, at its own pointer in that file.
#[test]
fn e011_an_oas31_default_mapping_in_a_referenced_file_is_rejected() {
    let (report, _, _) = split(
        "./lib.yaml#/Pet",
        "Cat: { type: object, required: [kind], properties: { kind: { type: string } } }\n\
         Other: { type: object, properties: { x: { type: string } } }\n\
         Pet:\n  \
         oneOf: [{ $ref: '#/Cat' }, { $ref: '#/Other' }]\n  \
         discriminator: { propertyName: kind, defaultMapping: '#/Other' }\n",
    );
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        report
            .diagnostics()
            .iter()
            .any(|d| d.code == Code::InvalidInput
                && d.pointer.as_str() == "/Pet/discriminator/defaultMapping"
                && d.message.starts_with("`defaultMapping` is an OpenAPI 3.2")),
        "{report:#?}"
    );
}

/// `nodeType` is an XML Object field only OpenAPI 3.2 defines, and a 3.1 document using it had it
/// drive the XML node table with no diagnostic (#1448). It is `E011` at the field, naming the
/// version that introduces it, as a 3.2 Path Item field is in a 3.1 document; the same document
/// declaring 3.2 generates the attribute.
#[test]
fn e011_an_oas31_xml_node_type_is_rejected_as_needing_oas32() {
    let spec = |version: &str| {
        format!(
            r##"
openapi: {version}
info: {{ title: T, version: 1.0.0 }}
paths:
  /item:
    get:
      responses:
        '200':
          description: ok
          content:
            application/xml:
              schema:
                type: object
                properties:
                  id: {{ type: string, xml: {{ nodeType: attribute }} }}
"##
        )
    };
    let report = generate(&spec("3.1.0"));
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    let rejected: Vec<_> = report
        .diagnostics()
        .iter()
        .filter(|d| d.code == Code::InvalidInput)
        .map(|d| (d.pointer.as_str(), d.message.as_str()))
        .collect();
    assert_eq!(
        rejected,
        [(
            "/paths/~1item/get/responses/200/content/application~1xml/schema/properties/id/xml/nodeType",
            "`nodeType` is an OpenAPI 3.2 XML Object field, and this document declares 3.1"
        )],
        "{report:#?}"
    );

    let (report, code) = generate_with_code(&spec("3.2.0"));
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::InvalidInput), "{report:#?}");
    assert!(code.contains("@id"), "{code}");
}

/// A `nodeType` under a keyword lowering never reads is still a field a 3.1 document cannot carry,
/// so it is rejected there too: the gate reads every schema the document holds.
#[test]
fn e011_an_oas31_xml_node_type_in_an_unlowered_subschema_is_rejected() {
    let report = generate(&single_component_document(
        "      type: object\n      not: { type: string, xml: { nodeType: text } }\n",
    ));
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        report
            .diagnostics()
            .iter()
            .any(|d| d.code == Code::InvalidInput
                && d.pointer.as_str() == "/components/schemas/U/not/xml/nodeType"
                && d.message.starts_with("`nodeType` is an OpenAPI 3.2")),
        "{report:#?}"
    );
}
