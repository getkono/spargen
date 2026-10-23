//! An Encoding Object's RFC 6570 `style` and `explode` on `multipart` and
//! `application/x-www-form-urlencoded` properties: the combinations the
//! specification defines, and the ones it leaves undefined.

use super::*;

#[test]
fn e009_multipart_deep_object_encoding_rejected() {
    // `deepObject` builds `name[key]=value` query fragments. A multipart part carries its name in
    // `Content-Disposition` and its value alone, so the style has no representation there — it used
    // to generate parts holding only the values, with the keys dropped. The property is a string,
    // not an object, so the RFC 6570 object-property gate beside this one does not also refuse it.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        content:
          multipart/form-data:
            schema:
              type: object
              properties:
                filter: { type: string }
            encoding:
              filter:
                style: deepObject
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert_eq!(
            messages_for(&report, Code::UnsupportedMediaType),
            [
                "`encoding.filter.style: deepObject` is defined only for `in: query`; it has no \
                 `multipart/form-data` part representation"
            ],
            "{report:#?}"
        );
    }
}

#[test]
fn e009_multipart_rfc6570_object_property_rejected() {
    // The specification applies the Encoding Object to the entire value for a non-array property,
    // but defines no part representation for an object. Reusing the query builders silently emitted
    // one part per member carrying only the value.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        content:
          multipart/form-data:
            schema:
              type: object
              properties:
                meta:
                  type: object
                  properties:
                    a: { type: string }
            encoding:
              meta:
                style: form
                explode: true
      responses:
        "204": { description: No Content }
"##;
    let report = generate(spec);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::UnsupportedMediaType), "{report:#?}");
}

#[test]
fn e009_encoding_delimited_style_with_explode_rejected() {
    // The specification's serialization table marks `spaceDelimited`/`pipeDelimited` with
    // `explode: true` as *n/a*. The identical parameter-side construct is already `E010`; an
    // Encoding Object used to accept it and then ignore the `explode`.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        content:
          application/x-www-form-urlencoded:
            schema:
              type: object
              properties:
                tags:
                  type: array
                  items: { type: string }
            encoding:
              tags:
                style: pipeDelimited
                explode: true
      responses:
        "204": { description: No Content }
"##;
    let report = generate(spec);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::UnsupportedMediaType), "{report:#?}");
}

/// The Encoding Object form of #696: `spaceDelimited`/`pipeDelimited` are defined only for arrays
/// and objects and `deepObject` only for objects, so a property whose schema admits a scalar
/// under one of them has no defined serialization. On form-urlencoded the runtime refused such a
/// value at request time (`ExpectedComposite`/`ExpectedObject`), failing every call that carried
/// it; on multipart a delimited scalar was sent as if `style: form` had been declared. Both are
/// `E009`, as every other undefined RFC 6570 combination in an Encoding Object is. A nullable
/// array property stays supported: a `null` property is omitted from a form body, not serialized.
#[test]
fn e009_an_encoding_style_over_a_property_admitting_a_value_it_cannot_serialize() {
    const TEMPLATE: &str = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        content:
          MEDIA:
            schema:
              type: object
              properties:
                p: SCHEMA
            encoding:
              p:
                style: STYLE
      responses:
        "204": { description: No Content }
components:
  schemas:
    Arr: { type: array, items: { type: string } }
    Nil: { type: 'null' }
"##;
    let spec = |media: &str, style: &str, schema: &str| {
        TEMPLATE
            .replace("MEDIA", media)
            .replace("STYLE", style)
            .replace("SCHEMA", schema)
    };
    let form = "application/x-www-form-urlencoded";
    let multipart = "multipart/form-data";
    // A `null` member reached through a `$ref` is not folded into the property's root, so it
    // stays a union member whose `null` is the whole value.
    let member_null = "{ oneOf: [{ $ref: '#/components/schemas/Arr' }, \
                       { $ref: '#/components/schemas/Nil' }] }";
    let rejected = [
        (form, "spaceDelimited", "{ type: string }"),
        (
            form,
            "pipeDelimited",
            "{ oneOf: [{ type: array, items: { type: string } }, { type: integer }] }",
        ),
        (form, "deepObject", "{ type: integer }"),
        (
            form,
            "deepObject",
            "{ type: array, items: { type: string } }",
        ),
        (multipart, "spaceDelimited", "{ type: string }"),
        (multipart, "pipeDelimited", "{ type: boolean }"),
    ];
    for (media, style, schema) in rejected {
        let spec = spec(media, style, schema);
        let shape = if style == "deepObject" {
            "only objects"
        } else {
            "only arrays and objects"
        };
        for report in [generate(&spec), check(&spec)] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "{media} {style} {schema}: {report:#?}"
            );
            let messages = messages_for(&report, Code::UnsupportedMediaType);
            assert_eq!(messages.len(), 1, "{media} {style} {schema}: {report:#?}");
            assert!(
                messages[0].contains(&format!("`encoding.p.style: {style}` serializes {shape}")),
                "{media} {style} {schema}: {messages:?}"
            );
        }
    }
    let accepted = [
        (
            form,
            "spaceDelimited",
            "{ type: array, items: { type: string } }",
        ),
        (
            form,
            "pipeDelimited",
            "{ type: object, properties: { a: { type: string } } }",
        ),
        (
            form,
            "spaceDelimited",
            "{ type: [array, 'null'], items: { type: string } }",
        ),
        (
            form,
            "deepObject",
            "{ type: object, properties: { a: { type: string } } }",
        ),
        (
            multipart,
            "pipeDelimited",
            "{ type: array, items: { type: string } }",
        ),
        (form, "spaceDelimited", member_null),
    ];
    for (media, style, schema) in accepted {
        let spec = spec(media, style, schema);
        let report = generate(&spec);
        assert_ne!(
            report.outcome(),
            Outcome::Rejected,
            "{media} {style} {schema}: {report:#?}"
        );
    }
}

#[test]
fn multipart_rfc6570_array_encoding_generates() {
    // The shapes that *are* defined stay supported: an array property under a delimited or form
    // style, exploded or not.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        content:
          multipart/form-data:
            schema:
              type: object
              properties:
                tags:
                  type: array
                  items: { type: string }
                names:
                  type: array
                  items: { type: string }
            encoding:
              tags:
                style: spaceDelimited
                explode: false
              names:
                style: form
                explode: true
      responses:
        "204": { description: No Content }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
}
