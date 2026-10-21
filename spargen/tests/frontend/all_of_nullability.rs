//! `allOf` nullability: an object `allOf` admits `null` exactly when every member does, a `$ref`
//! member to an untyped object or untyped `allOf` component among them, and a branching graph of
//! such `$ref` members, under `#/components` or a bundle, is read once per target.

use super::*;

/// Issue #425: an object `allOf` admits `null` exactly when every member does, as its `$ref`-sibling
/// spelling and the all-scalar `allOf` already do. A nullable `$ref` member decides `null` for
/// itself; an untyped inline member's object keywords bind objects only, so it admits `null`
/// without deciding; a member typed `object` alone denies it. Untyped members alone decide nothing,
/// so they keep the non-null struct an untyped object schema lowers to by itself. The enclosing
/// schema's own untyped object keywords beside the `allOf` are neutral in the same way, and so is
/// an `allOf` of untyped members written beside a `$ref` (#562): it used to lower on its own to
/// that non-null struct and deny the nullable target's `null`.
#[test]
fn an_object_all_of_admits_null_when_every_member_does() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers: [{ url: 'https://e.com' }]
paths:
  /p:
    get:
      operationId: fetch
      responses:
        '200':
          description: ok
          content:
            application/json: { schema: { $ref: '#/components/schemas/Holder' } }
components:
  schemas:
    Base:
      type: [object, 'null']
      properties:
        id: { type: string }
    Untyped:
      properties:
        extra: { type: string }
    Holder:
      type: object
      required:
        - viaAllOf
        - viaSibling
        - both
        - single
        - denied
        - untypedOnly
        - enclosing
        - siblingAllOf
        - siblingAllOfRef
        - siblingAllOfDenied
      properties:
        viaAllOf:
          allOf:
            - $ref: '#/components/schemas/Base'
            - properties: { extra: { type: string } }
        viaSibling:
          $ref: '#/components/schemas/Base'
          properties: { extra: { type: string } }
        both:
          allOf:
            - $ref: '#/components/schemas/Base'
            - type: [object, 'null']
              properties: { extra: { type: string } }
        single:
          allOf:
            - $ref: '#/components/schemas/Base'
        denied:
          allOf:
            - $ref: '#/components/schemas/Base'
            - type: object
              properties: { extra: { type: string } }
        untypedOnly:
          allOf:
            - properties: { id: { type: string } }
            - properties: { extra: { type: string } }
        enclosing:
          allOf:
            - $ref: '#/components/schemas/Base'
          properties: { z: { type: string } }
        siblingAllOf:
          $ref: '#/components/schemas/Base'
          allOf:
            - properties: { extra: { type: string } }
        siblingAllOfRef:
          $ref: '#/components/schemas/Base'
          allOf:
            - $ref: '#/components/schemas/Untyped'
        siblingAllOfDenied:
          $ref: '#/components/schemas/Base'
          allOf:
            - type: object
              properties: { extra: { type: string } }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    let types = types_module(&code);
    for (field, nullable) in [
        ("pub via_all_of", true),
        ("pub via_sibling", true),
        ("pub both", true),
        ("pub single", true),
        ("pub denied", false),
        ("pub untyped_only", false),
        ("pub enclosing", true),
        ("pub sibling_all_of", true),
        ("pub sibling_all_of_ref", true),
        ("pub sibling_all_of_denied", false),
    ] {
        let ty = field_type(&types, field).unwrap_or_else(|| panic!("no `{field}` field: {types}"));
        assert_eq!(
            ty.starts_with("Option<"),
            nullable,
            "`{field}` is `{ty}`, but `null` is {} here: {types}",
            if nullable { "valid" } else { "invalid" }
        );
    }
}

/// Issue #541: a `$ref` member whose target is an untyped object component admits `null` without
/// deciding it, exactly as the same member written inline does (#425). The `$ref` to `U` used to
/// record `U`'s lowered non-null struct as a decision, so `viaRefs`, `untypedRef` and `besideOneOf`
/// denied `null` that `inline` and `sibling` admit. A bare alias `A` of `U` is `U` (`viaAlias`,
/// `aliasRef`). `U` alone still lowers to a non-null struct, and a meet of untyped members alone
/// still decides nothing. Held in the root document, in a sub-file (whose `#/components/schemas/`
/// names its own components), and for a vendored remote `U`.
#[test]
fn a_ref_to_an_untyped_object_component_admits_null_in_an_object_meet() {
    const COMPONENTS: &str = r##"
components:
  schemas:
    N: { type: [object, 'null'], properties: { n: { type: string } } }
    U: { properties: { u: { type: string } } }
    V: { properties: { v: { type: string } } }
    A: { $ref: '#/components/schemas/U' }
    Holder:
      type: object
      required: [viaRefs, inline, sibling, untypedRef, besideOneOf, viaAlias, aliasRef, untypedOnly, alone]
      properties:
        viaAlias: { allOf: [{ $ref: '#/components/schemas/N' }, { $ref: '#/components/schemas/A' }] }
        aliasRef: { $ref: '#/components/schemas/A', type: [object, 'null'], properties: { n: { type: string } } }
        viaRefs: { allOf: [{ $ref: '#/components/schemas/N' }, { $ref: '#/components/schemas/U' }] }
        inline: { allOf: [{ type: [object, 'null'], properties: { n: { type: string } } }, { properties: { u: { type: string } } }] }
        sibling: { $ref: '#/components/schemas/N', properties: { u: { type: string } } }
        untypedRef: { $ref: '#/components/schemas/U', type: [object, 'null'], properties: { n: { type: string } } }
        besideOneOf: { allOf: [{ $ref: '#/components/schemas/N' }, { $ref: '#/components/schemas/U' }], oneOf: [{}] }
        untypedOnly: { allOf: [{ $ref: '#/components/schemas/U' }, { $ref: '#/components/schemas/V' }] }
        alone: { $ref: '#/components/schemas/U' }
"##;
    const EXPECTED: [(&str, bool); 9] = [
        ("pub via_refs:", true),
        ("pub inline:", true),
        ("pub sibling:", true),
        ("pub untyped_ref:", true),
        ("pub beside_one_of:", true),
        ("pub via_alias:", true),
        ("pub alias_ref:", true),
        ("pub untyped_only:", false),
        ("pub alone:", false),
    ];
    let root = format!(
        r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
servers: [{{ url: 'https://e.com' }}]
paths:
  /u:
    get:
      operationId: getU
      responses:
        '200':
          description: ok
          content:
            application/json: {{ schema: {{ $ref: '#/components/schemas/Holder' }} }}
{COMPONENTS}"##
    );
    let (generated, root_code) = generate_with_code(&root);
    let (split_generated, split_checked, split_code) =
        split("./lib.yaml#/components/schemas/Holder", COMPONENTS);
    for (entry, report) in [
        ("root/generate", &generated),
        ("split/generate", &split_generated),
        ("split/check", &split_checked),
    ] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
    }
    for (spelling, code) in [("root", &root_code), ("split", &split_code)] {
        for (field, nullable) in EXPECTED {
            let ty = field_type(code, field)
                .unwrap_or_else(|| panic!("{spelling}: no `{field}` field: {code}"));
            assert_eq!(
                ty.starts_with("Option<"),
                nullable,
                "{spelling}: `{field}` is `{ty}`, but `null` is {} here: {code}",
                if nullable { "valid" } else { "invalid" }
            );
        }
    }

    // The vendored-remote spelling: `U` is a whole remote document, met with the nullable `N`.
    use sha2::{Digest, Sha256};
    const URL: &str = "https://api.example.com/schemas/untyped.yaml";
    const VENDORED: &str = "api.example.com/schemas/untyped.yaml";
    let untyped = "properties:\n  u: { type: string }\n";
    let temp = tempfile::tempdir().unwrap();
    let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
    std::fs::write(
        dir.join("openapi.yaml"),
        format!(
            r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
servers: [{{ url: 'https://e.com' }}]
paths:
  /u:
    get:
      operationId: getU
      responses:
        '200':
          description: ok
          content:
            application/json: {{ schema: {{ $ref: '#/components/schemas/Holder' }} }}
components:
  schemas:
    N: {{ type: [object, 'null'], properties: {{ n: {{ type: string }} }} }}
    Holder:
      type: object
      required: [remote, remoteSibling, remoteOnly]
      properties:
        remote: {{ allOf: [{{ $ref: '#/components/schemas/N' }}, {{ $ref: '{URL}' }}] }}
        remoteSibling: {{ $ref: '{URL}', type: [object, 'null'], properties: {{ n: {{ type: string }} }} }}
        remoteOnly: {{ allOf: [{{ $ref: '{URL}' }}] }}
"##
        ),
    )
    .unwrap();
    std::fs::write(
        dir.join("spargen.lock"),
        format!(
            "version = 1\n\n[[remote]]\nurl = \"{URL}\"\nsha256 = \"{:x}\"\npath = \"{VENDORED}\"\n",
            Sha256::digest(untyped.as_bytes())
        ),
    )
    .unwrap();
    let vendored = dir.join(".spargen/vendor").join(VENDORED);
    std::fs::create_dir_all(vendored.parent().unwrap()).unwrap();
    std::fs::write(&vendored, untyped).unwrap();
    let out = dir.join("client.rs");
    let report = run_generate(&build(dir.join("openapi.yaml"), out.clone()));
    let code = std::fs::read_to_string(&out).unwrap_or_default();
    assert_ne!(report.outcome(), Outcome::Rejected, "remote: {report:#?}");
    for (field, nullable) in [
        ("pub remote:", true),
        ("pub remote_sibling:", true),
        ("pub remote_only:", false),
    ] {
        let ty = field_type(&code, field)
            .unwrap_or_else(|| panic!("remote: no `{field}` field: {code}"));
        assert_eq!(
            ty.starts_with("Option<"),
            nullable,
            "remote: `{field}` is `{ty}`, but `null` is {} here: {code}",
            if nullable { "valid" } else { "invalid" }
        );
    }
}

/// Issue #565: a `$ref` to a component that is itself an `allOf` of untyped object members admits
/// `null` without deciding it, as the same members written inline do (#425, #541). `C`'s merge
/// decides nothing, yet its lowered non-null struct used to be recorded as a decision, so
/// `viaComposed`, `viaComposedRefs` and `composedSibling` denied the `null` that `inline` admits.
/// A composition some member decides keeps its answer: `D` denies `null` (`viaDecided`) and `E`
/// admits it (`viaDecidedNull`). `C` alone, and met only with an untyped member, still lowers to a
/// non-null struct.
///
/// The same reading reaches three more spellings, each of which also generated the plain struct
/// before: a `$ref` whose `allOf` sibling names `C` (`siblingNamesComposed`, through
/// `lower_ref_sibling`), a nested inline `allOf` naming `C` (`nestedNamesComposed`), and a member
/// `$ref` to `RS`, a `$ref` to an untyped component with an untyped `allOf` sibling
/// (`viaRefWithAllOfSibling`).
#[test]
fn a_ref_to_an_untyped_all_of_component_admits_null_in_an_object_meet() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers: [{ url: 'https://e.com' }]
paths:
  /u:
    get:
      operationId: getU
      responses:
        '200':
          description: ok
          content:
            application/json: { schema: { $ref: '#/components/schemas/Holder' } }
components:
  schemas:
    N: { type: [object, 'null'], properties: { n: { type: string } } }
    U: { properties: { u: { type: string } } }
    V: { properties: { v: { type: string } } }
    C: { allOf: [{ properties: { u: { type: string } } }, { properties: { w: { type: string } } }] }
    R: { allOf: [{ $ref: '#/components/schemas/U' }, { $ref: '#/components/schemas/V' }] }
    D: { allOf: [{ type: object, properties: { d: { type: string } } }] }
    E: { allOf: [{ $ref: '#/components/schemas/N' }, { properties: { e: { type: string } } }] }
    RS: { $ref: '#/components/schemas/U', allOf: [{ properties: { w: { type: string } } }] }
    Holder:
      type: object
      required: [viaComposed, viaComposedRefs, composedSibling, inline, viaDecided, viaDecidedNull, composedOnly, alone, siblingNamesComposed, nestedNamesComposed, viaRefWithAllOfSibling]
      properties:
        siblingNamesComposed: { $ref: '#/components/schemas/N', allOf: [{ $ref: '#/components/schemas/C' }] }
        nestedNamesComposed: { allOf: [{ $ref: '#/components/schemas/N' }, { allOf: [{ $ref: '#/components/schemas/C' }] }] }
        viaRefWithAllOfSibling: { allOf: [{ $ref: '#/components/schemas/N' }, { $ref: '#/components/schemas/RS' }] }
        viaComposed: { allOf: [{ $ref: '#/components/schemas/N' }, { $ref: '#/components/schemas/C' }] }
        viaComposedRefs: { allOf: [{ $ref: '#/components/schemas/N' }, { $ref: '#/components/schemas/R' }] }
        composedSibling: { $ref: '#/components/schemas/C', type: [object, 'null'], properties: { n: { type: string } } }
        inline: { allOf: [{ $ref: '#/components/schemas/N' }, { allOf: [{ properties: { u: { type: string } } }, { properties: { w: { type: string } } }] }] }
        viaDecided: { allOf: [{ $ref: '#/components/schemas/N' }, { $ref: '#/components/schemas/D' }] }
        viaDecidedNull: { allOf: [{ $ref: '#/components/schemas/E' }, { $ref: '#/components/schemas/U' }] }
        composedOnly: { allOf: [{ $ref: '#/components/schemas/C' }, { $ref: '#/components/schemas/U' }] }
        alone: { $ref: '#/components/schemas/C' }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    for (field, nullable) in [
        ("pub via_composed:", true),
        ("pub via_composed_refs:", true),
        ("pub composed_sibling:", true),
        ("pub inline:", true),
        ("pub via_decided:", false),
        ("pub via_decided_null:", true),
        ("pub composed_only:", false),
        ("pub alone:", false),
        ("pub sibling_names_composed:", true),
        ("pub nested_names_composed:", true),
        ("pub via_ref_with_all_of_sibling:", true),
    ] {
        let ty = field_type(&code, field).unwrap_or_else(|| panic!("no `{field}` field: {code}"));
        assert_eq!(
            ty.starts_with("Option<"),
            nullable,
            "`{field}` is `{ty}`, but `null` is {} here: {code}",
            if nullable { "valid" } else { "invalid" }
        );
    }
}

/// Issue #565's reading of a `$ref` target's `allOf` body is done once per target, not once per
/// path. `C<i>` is `allOf: [$ref C<i+1>, $ref C<i+1>]`, so the paths to the leaf number 2^30;
/// re-reading the body at each `$ref` (as the walk briefly did) would not finish at this depth,
/// where lowering the same components shares each one's type and stays linear. The answer is
/// still the leaf's, replayed: an untyped leaf (`C`) decides nothing, so `N`'s `null` survives; a
/// leaf typed `object` (`T`) denies it.
#[test]
fn a_ref_to_a_branching_untyped_all_of_graph_reads_each_target_once() {
    let mut defs = String::new();
    for (chain, depth, leaf) in [
        ("C", 30, "properties: { c: { type: string } }"),
        ("T", 30, "type: object"),
    ] {
        for level in 0..depth {
            let next = level + 1;
            defs.push_str(&format!(
                "    {chain}{level}: {{ allOf: [{{ $ref: '#/components/schemas/{chain}{next}' }}, \
                 {{ $ref: '#/components/schemas/{chain}{next}' }}] }}\n"
            ));
        }
        defs.push_str(&format!("    {chain}{depth}: {{ {leaf} }}\n"));
    }
    let spec = format!(
        r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
servers: [{{ url: 'https://e.com' }}]
paths:
  /u:
    get:
      operationId: getU
      responses:
        '200':
          description: ok
          content:
            application/json: {{ schema: {{ $ref: '#/components/schemas/Holder' }} }}
components:
  schemas:
    N: {{ type: [object, 'null'], properties: {{ n: {{ type: string }} }} }}
    Holder:
      type: object
      required: [untypedLeaf, typedLeaf]
      properties:
        untypedLeaf: {{ allOf: [{{ $ref: '#/components/schemas/N' }}, {{ $ref: '#/components/schemas/C0' }}] }}
        typedLeaf: {{ allOf: [{{ $ref: '#/components/schemas/N' }}, {{ $ref: '#/components/schemas/T0' }}] }}
{defs}"##
    );
    let (report, code) = generate_with_code(&spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    for (field, nullable) in [("pub untyped_leaf:", true), ("pub typed_leaf:", false)] {
        let ty = field_type(&code, field).unwrap_or_else(|| panic!("no `{field}` field: {code}"));
        assert_eq!(ty.starts_with("Option<"), nullable, "`{field}` is `{ty}`");
    }
}

/// A bundle-`$ref` `allOf` member is expanded in place, once per target, and each target counts
/// once per composition however many paths reach it (#616). `D<i>` is `allOf: [$ref D<i+1>, $ref
/// D<i+1>]` under `#/x-defs`, outside `#/components`, so its contribution is flattened rather than
/// shared as a type; replaying every recorded copy doubled the contribution at each level, and
/// generation exhausted memory near depth 22. `E<i>` reaches `E<i+1>` through two *distinct*
/// targets, `A<i+1>` and `B<i+1>`, so it doubles even when no target is named twice in one
/// `allOf`, unless what a recorded expansion holds of each nested target is replayed once. The
/// typed leaves still decide the merge: their fields, `required`, and types survive.
#[test]
fn a_branching_bundle_all_of_graph_contributes_each_target_once() {
    const DEPTH: usize = 40;
    let mut defs = String::new();
    for level in 0..DEPTH {
        let next = level + 1;
        defs.push_str(&format!(
            "  D{level}: {{ allOf: [{{ $ref: '#/x-defs/D{next}' }}, {{ $ref: '#/x-defs/D{next}' }}] }}\n\
             \x20 E{level}: {{ allOf: [{{ $ref: '#/x-defs/A{next}' }}, {{ $ref: '#/x-defs/B{next}' }}] }}\n\
             \x20 A{next}: {{ allOf: [{{ $ref: '#/x-defs/E{next}' }}] }}\n\
             \x20 B{next}: {{ allOf: [{{ $ref: '#/x-defs/E{next}' }}, {{ type: object }}] }}\n"
        ));
    }
    defs.push_str(&format!(
        "  D{DEPTH}: {{ type: object, required: [a], properties: {{ a: {{ type: string }}, b: {{ type: integer }} }} }}\n\
         \x20 E{DEPTH}: {{ type: object, required: [e], properties: {{ e: {{ type: string }} }} }}\n"
    ));
    let spec = format!(
        r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
servers: [{{ url: 'https://e.com' }}]
paths:
  /u:
    get:
      operationId: getU
      responses:
        '200':
          description: ok
          content:
            application/json: {{ schema: {{ $ref: '#/components/schemas/Holder' }} }}
components:
  schemas:
    Holder:
      type: object
      required: [doubled, diamond]
      properties:
        doubled: {{ allOf: [{{ $ref: '#/x-defs/D0' }}, {{ $ref: '#/x-defs/D0' }}] }}
        diamond: {{ allOf: [{{ $ref: '#/x-defs/E0' }}, {{ $ref: '#/x-defs/A1' }}] }}
x-defs:
{defs}"##
    );
    let (report, code) = generate_with_code(&spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    let types = types_module(&code);
    for (field, required, alias) in [
        ("pub a:", true, "String"),
        ("pub b:", false, "i64"),
        ("pub e:", true, "String"),
    ] {
        let ty = field_type(&types, field).unwrap_or_else(|| panic!("no `{field}` field: {code}"));
        let named = match ty.strip_prefix("Option<") {
            Some(inner) => inner.trim_end_matches('>'),
            None => ty.as_str(),
        };
        assert_eq!(
            ty.starts_with("Option<"),
            !required,
            "`{field}` is `{ty}`: {code}"
        );
        assert!(
            types.contains(&format!("pub type {named} = {alias};")),
            "`{field}` is `{ty}`, not `{alias}`: {code}"
        );
    }
}
