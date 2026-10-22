//! XML element names a field takes from the schema it references: OpenAPI 3.2's `$ref` node
//! type `none`, and the names an array's items take from their struct's root `xml.name`.

use super::*;

/// OpenAPI 3.2 gives a `$ref` the node type `none`, so a property that is a `$ref` to a component
/// struct, and each item of an array property whose items are, is the referenced component's own
/// element: named by its root `xml.name` (`pet`), or else by its component name (`Cat`), not by the
/// property (#844). OpenAPI 3.1 has no node types and names a property's element after the property,
/// so there both fields keep their names (`Cat` has no root `xml.name` for #826's items rule to
/// follow, and `favourite` is no array). An owner that also travels as JSON keeps its field names
/// under the shared-type `W006`; an owner no XML body reaches derives no rename and warns nothing.
#[test]
fn a_ref_property_names_its_element_after_the_component_under_oas32() {
    for version in ["3.2.0", "3.1.0"] {
        for reach in ["xml", "shared", "json"] {
            let body_media = if reach == "json" {
                "application/json"
            } else {
                "application/xml"
            };
            let json_use = if reach == "shared" {
                "
        '200':
          description: ok
          content:
            application/json: { schema: { $ref: '#/components/schemas/Owner' } }"
            } else {
                ""
            };
            let spec = format!(
                r##"
openapi: {version}
info: {{ title: T, version: 1.0.0 }}
paths:
  /owners:
    post:
      requestBody:
        content:
          {body_media}: {{ schema: {{ $ref: '#/components/schemas/Owner' }} }}
      responses:
        '204': {{ description: ok }}{json_use}
components:
  schemas:
    Owner:
      type: object
      properties:
        favourite: {{ $ref: '#/components/schemas/Pet' }}
        others: {{ type: array, items: {{ $ref: '#/components/schemas/Cat' }} }}
    Pet: {{ type: object, xml: {{ name: pet }}, properties: {{ id: {{ type: integer }} }} }}
    Cat: {{ type: object, properties: {{ id: {{ type: integer }} }} }}
"##
            );
            let case = format!("{version} reach={reach}");
            let (report, code) = generate_with_code(&spec);
            assert_eq!(report.outcome(), Outcome::Generated, "{case}: {report:#?}");
            let renamed =
                [r#"rename = "pet","#, r#"rename = "Cat","#].map(|rename| code.contains(rename));
            // The shared owner's warning names the derived element names, not a hint nobody wrote.
            let shared_warning = messages_for(&report, Code::XmlHintIgnored).iter().any(|m| {
                m.contains("shared between an XML body")
                    && m.contains("derived from the referenced schema")
                    && m.contains("property `favourite` (`pet`)")
                    && m.contains("property `others` (`Cat`)")
                    && !m.contains("`xml.name`/`xml.attribute`")
            });
            match (version, reach) {
                ("3.2.0", "xml") => {
                    assert_eq!(renamed, [true, true], "{case}: {code}");
                    assert!(
                        !has_code(&report, Code::XmlHintIgnored),
                        "{case}: {report:#?}"
                    );
                }
                ("3.2.0", "shared") => {
                    assert_eq!(renamed, [false, false], "{case}: {code}");
                    assert!(shared_warning, "{case}: {report:#?}");
                }
                _ => {
                    assert_eq!(renamed, [false, false], "{case}: {code}");
                    assert!(
                        !has_code(&report, Code::XmlHintIgnored),
                        "{case}: {report:#?}"
                    );
                }
            }
        }
    }
}

/// A derived element name that would give two children of one struct the same name — two `$ref`
/// properties to one component, or a `$ref` whose component name is another property's name — is
/// withdrawn with `W006`, and each such field keeps its property name: serde would otherwise read
/// both fields from one element, which quick-xml refuses as a duplicate field. A derived name that
/// clashes with nothing (`Dog`) still applies. The same holds for the names an array's items take
/// from an explicit root `xml.name`, on OpenAPI 3.1 as on 3.2.
#[test]
fn a_derived_element_name_that_clashes_with_a_sibling_is_withdrawn() {
    let refs = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /owners:
    post:
      requestBody:
        content:
          application/xml: { schema: { $ref: '#/components/schemas/Owner' } }
      responses:
        '204': { description: ok }
components:
  schemas:
    Owner:
      type: object
      properties:
        first: { $ref: '#/components/schemas/Pet' }
        second: { $ref: '#/components/schemas/Pet' }
        pick: { $ref: '#/components/schemas/cat' }
        cat: { type: string }
        dog: { $ref: '#/components/schemas/Dog' }
    Pet: { type: object, properties: { id: { type: integer } } }
    cat: { type: object, properties: { id: { type: integer } } }
    Dog: { type: object, properties: { id: { type: integer } } }
"##;
    let (report, code) = generate_with_code(refs);
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    for kept in ["first", "second", "pick"] {
        assert!(
            code.contains(&format!(r#"rename = "{kept}","#)),
            "{kept}: {code}"
        );
    }
    assert!(!code.contains(r#"rename = "Pet","#), "{code}");
    assert!(code.contains(r#"rename = "Dog","#), "{code}");
    let messages = messages_for(&report, Code::XmlHintIgnored);
    assert!(
        messages
            .iter()
            .any(|m| m.contains("property `first` (`Pet`)")
                && m.contains("property `second` (`Pet`)")
                && m.contains("property `pick` (`cat`)")
                && !m.contains("`dog`")),
        "{report:#?}"
    );

    for version in ["3.1.0", "3.2.0"] {
        let items = format!(
            r##"
openapi: {version}
info: {{ title: T, version: 1.0.0 }}
paths:
  /owners:
    post:
      requestBody:
        content:
          application/xml: {{ schema: {{ $ref: '#/components/schemas/Owner' }} }}
      responses:
        '204': {{ description: ok }}
components:
  schemas:
    Owner:
      type: object
      properties:
        cats: {{ type: array, items: {{ $ref: '#/components/schemas/Pet' }} }}
        dogs: {{ type: array, items: {{ $ref: '#/components/schemas/Pet' }} }}
    Pet: {{ type: object, xml: {{ name: pet }}, properties: {{ id: {{ type: integer }} }} }}
"##
        );
        let (report, code) = generate_with_code(&items);
        assert_eq!(
            report.outcome(),
            Outcome::Generated,
            "{version}: {report:#?}"
        );
        assert!(!code.contains(r#"rename = "pet","#), "{version}: {code}");
        assert!(code.contains(r#"rename = "cats","#), "{version}: {code}");
        assert!(code.contains(r#"rename = "dogs","#), "{version}: {code}");
        assert!(
            messages_for(&report, Code::XmlHintIgnored)
                .iter()
                .any(|m| m.contains("property `cats` (`pet`)")
                    && m.contains("property `dogs` (`pet`)")),
            "{version}: {report:#?}"
        );
    }
}

/// An explicit `xml.nodeType: element` on a `$ref` property, or on the `$ref` items of an array
/// property, makes that node an element of its own, named after the property: it takes no name
/// from the referenced component under OpenAPI 3.2.
#[test]
fn a_ref_declared_an_element_keeps_its_property_name_under_oas32() {
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /owners:
    post:
      requestBody:
        content:
          application/xml: { schema: { $ref: '#/components/schemas/Owner' } }
      responses:
        '204': { description: ok }
components:
  schemas:
    Owner:
      type: object
      properties:
        favourite: { $ref: '#/components/schemas/Pet', xml: { nodeType: element } }
        others:
          type: array
          items: { $ref: '#/components/schemas/Cat', xml: { nodeType: element } }
        spare: { $ref: '#/components/schemas/Dog', xml: { nodeType: none } }
    Pet: { type: object, xml: { name: pet }, properties: { id: { type: integer } } }
    Cat: { type: object, properties: { id: { type: integer } } }
    Dog: { type: object, properties: { id: { type: integer } } }
"##;
    let (report, code) = generate_with_code(spec);
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert!(code.contains(r#"rename = "favourite","#), "{code}");
    assert!(code.contains(r#"rename = "others","#), "{code}");
    // `none`, restated, is the default: the referenced element stands in its place.
    assert!(code.contains(r#"rename = "Dog","#), "{code}");
}
