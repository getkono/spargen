//! `allOf` composition: member merging, property intersection, and the meets an `allOf` beside a
//! union produces.

use super::*;

/// A union met with an `allOf`'s other members, or with a `$ref`'s target, is lowered on its own
/// first, with its merge held back, and the meet re-emits what it leaves under the schema's own
/// name. The pre-meet union, and the branch types the meet replaced, were left in the output as
/// public types nothing referred to (#561), such as `Umember0`, a trial-matched `oneOf` with a
/// `serde_json::Value` variant, beside the met union `U`. Each spelling of the meet now withholds
/// them: as an `allOf` member, beside an `allOf`, and beside a `$ref` alone or with keywords, where
/// the keywords' own lowered type, `UconstraintConstraint`, is withheld too (#571). What
/// something can still name stays: a branch the met union refers to, a `$ref`'d component first
/// lowered inside the union, and a `$ref`'d subschema the resolver's memo hands a later `$ref`.
#[test]
fn an_all_of_union_meet_emits_no_union_its_result_does_not_use() {
    /// The types the `types` module declares, in source order.
    fn declared(code: &str) -> Vec<String> {
        types_module(code)
            .lines()
            .map(str::trim_start)
            .filter_map(|line| {
                ["pub struct ", "pub enum ", "pub type "]
                    .iter()
                    .find_map(|item| line.strip_prefix(item))
            })
            .filter_map(|rest| rest.split([' ', '<', '{', '(', ';']).next())
            .map(str::to_owned)
            .collect()
    }
    let refiner = "{ properties: { a: { type: string } } }";
    let b = "B: { type: object, properties: { a: { type: string } } }";
    // Each spelling, with the types it no longer emits and the ones it must keep.
    let cases: [(&str, String, &[&str], &[&str]); 7] = [
        (
            "an allOf member",
            format!("U: {{ allOf: [{{ oneOf: [{{ type: string }}, {{}}] }}, {refiner}] }}"),
            &["Umember0", "Umember0Variant1"],
            &["U", "Umember0Variant0"],
        ),
        (
            "beside an allOf",
            format!("U: {{ oneOf: [{{ type: string }}, {{}}], allOf: [{refiner}] }}"),
            &["Uunion", "UunionVariant1"],
            &["U", "UunionVariant0"],
        ),
        (
            "beside a $ref",
            format!(
                "{b}\n    U: {{ $ref: '#/components/schemas/B', \
                 oneOf: [{{ required: [a] }}, {{ type: object }}] }}"
            ),
            &["Uconstraint", "UconstraintVariant0", "UconstraintVariant1"],
            &["B", "U"],
        ),
        (
            "beside a $ref and its keywords",
            format!(
                "{b}\n    U: {{ $ref: '#/components/schemas/B', properties: {{ c: {{ type: integer \
                 }} }}, oneOf: [{{ required: [a] }}, {{ type: object }}] }}"
            ),
            &[
                "Uunion",
                "UunionVariant0",
                "UunionVariant1",
                "UconstraintConstraint",
            ],
            &["B", "U", "UconstraintConstraintc"],
        ),
        (
            "beside a $ref and its typed keywords, met with the target first",
            format!(
                "{b}\n    U: {{ $ref: '#/components/schemas/B', type: object, properties: {{ c: \
                 {{ type: integer }} }}, oneOf: [{{ required: [a] }}, {{ type: object }}] }}"
            ),
            &[
                "Uunion",
                "UunionVariant0",
                "UunionVariant1",
                "Uconstraint",
                "UreferenceComposition",
            ],
            &["B", "U", "Uconstraintc"],
        ),
        (
            "a $ref'd component first lowered inside the union",
            format!(
                "U: {{ allOf: [{{ oneOf: [{{ $ref: '#/components/schemas/S' }}, {{}}] }}, \
                 {refiner}] }}\n    S: {{ type: object, properties: {{ s: {{ type: string }} }} }}"
            ),
            &["Umember0", "Umember0Variant1"],
            &["U", "S", "Ss"],
        ),
        (
            "a $ref'd subschema the resolver hands a later $ref",
            format!(
                "U: {{ allOf: [{{ oneOf: [{{ type: string }}, {{ $ref: \
                 '#/components/schemas/W/properties/w' }}] }}, {refiner}] }}\n    \
                 W: {{ type: object, properties: {{ w: {{ type: object, properties: \
                 {{ b: {{ type: integer }} }} }} }} }}\n    \
                 X: {{ type: object, properties: {{ x: {{ $ref: \
                 '#/components/schemas/W/properties/w' }} }} }}"
            ),
            &["Umember0"],
            &["U", "Umember0Variant0", "X"],
        ),
    ];
    for (spelling, schemas, withheld, kept) in cases {
        let (report, code) = generate_with_code(&format!(
            "openapi: 3.1.0\ninfo: {{ title: T, version: 1.0.0 }}\npaths: {{}}\ncomponents:\n  \
             schemas:\n    {schemas}\n"
        ));
        assert_eq!(
            report.outcome(),
            Outcome::Generated,
            "{spelling}: {report:#?}"
        );
        let declared = declared(&code);
        for name in withheld {
            assert!(
                !declared.iter().any(|declared| declared == name),
                "{spelling}: `{name}` is still emitted: {declared:?}"
            );
        }
        for name in kept {
            assert!(
                declared.iter().any(|declared| declared == name),
                "{spelling}: `{name}` is no longer emitted: {declared:?}"
            );
        }
        // The type the resolver first lowered inside the union is the one `X.x` names.
        if let Some((_, rest)) = code.split_once("pub x: Option<") {
            let shared = rest.split('>').next().unwrap_or_default();
            assert!(
                declared.iter().any(|declared| declared == shared),
                "{spelling}: `X.x`'s `{shared}` is not emitted: {declared:?}"
            );
        }
        let found = oracles::indistinguishable_variants(&code);
        assert!(found.is_empty(), "{spelling}: {found:#?}");
    }
}

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
    for (entry, report) in [("generate", &report), ("check", &check(spec))] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
    }
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
    let checked = check(&root);
    let (split_generated, split_checked, split_code) =
        split("./lib.yaml#/components/schemas/Holder", COMPONENTS);
    for (entry, report) in [
        ("root/generate", &generated),
        ("root/check", &checked),
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
    for (entry, report) in [("generate", &report), ("check", &check(spec))] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
    }
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
    for (entry, report) in [("generate", &report), ("check", &check(&spec))] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
    }
    for (field, nullable) in [("pub untyped_leaf:", true), ("pub typed_leaf:", false)] {
        let ty = field_type(&code, field).unwrap_or_else(|| panic!("no `{field}` field: {code}"));
        assert_eq!(ty.starts_with("Option<"), nullable, "`{field}` is `{ty}`");
    }
}

/// `allOf: [{$ref: T}, {oneOf/anyOf: […]}]` describes exactly the instances `{$ref: T,
/// oneOf/anyOf: […]}` does, and so does the union written beside the `allOf`: all three are one
/// conjunction, and must agree (#463). The `allOf`-member spelling used to read the union as a
/// scalar member, so beside `T`'s object it was `E013` "mixes object and scalar members" where the
/// `$ref` spelling generated. Each case is lowered in all three spellings as the component `Pick`,
/// and each must reach the same outcome, report the same codes at `Pick`, and give `Pick` the same
/// shape: branches the meet leaves as one type collapse with `W001` and keep the keyword's `null`
/// rule, a branch the target excludes drops out, a union none of whose branches meets the target
/// is `E013`, and a union of one real branch beside `null` is met as that branch.
#[test]
fn an_all_of_union_member_is_met_with_the_other_members_as_a_ref_sibling_union_is() {
    let base =
        "    Base:\n      type: object\n      properties:\n        a: { type: string }\n        \
                b: { type: string }\n";
    let nullable_base =
        "    NB:\n      type: [object, 'null']\n      properties:\n        a: { type: \
                         string }\n        b: { type: string }\n";
    let name = "    Name: { type: string }\n";
    let required = "[ { required: [a] }, { required: [b] } ]";
    let enumerated = "[ { type: string, enum: [red, green] }, { type: integer } ]";
    let single =
        "[ { type: object, required: [a], properties: { c: { type: integer } } }, { type: \
                  'null' } ]";
    // (case, target, keyword, branches, the codes reported at `Pick`, `Pick`'s shape). `Fields`
    // carries whether the required `pick` property is `Option`, that is, whether `null` is valid.
    enum Shape {
        Fields(&'static [&'static str], bool),
        Variants(&'static [&'static str]),
        Rejected,
    }
    let cases = [
        (
            "required-only oneOf",
            "Base",
            "oneOf",
            required,
            vec![Code::ValidationKeywordIgnored],
            Shape::Fields(&["a", "b"], false),
        ),
        (
            "required-only anyOf",
            "Base",
            "anyOf",
            required,
            vec![Code::ValidationKeywordIgnored],
            Shape::Fields(&["a", "b"], false),
        ),
        (
            "nullable target, oneOf",
            "NB",
            "oneOf",
            required,
            vec![Code::ValidationKeywordIgnored],
            Shape::Fields(&["a", "b"], false),
        ),
        (
            "nullable target, anyOf",
            "NB",
            "anyOf",
            required,
            vec![Code::ValidationKeywordIgnored],
            Shape::Fields(&["a", "b"], true),
        ),
        (
            "excluded branch, oneOf",
            "Name",
            "oneOf",
            enumerated,
            vec![],
            Shape::Variants(&["Red", "Green"]),
        ),
        (
            "excluded branch, anyOf",
            "Name",
            "anyOf",
            enumerated,
            vec![],
            Shape::Variants(&["Red", "Green"]),
        ),
        (
            "empty meet",
            "Name",
            "oneOf",
            "[ { type: integer } ]",
            vec![Code::AllOfIrreconcilable],
            Shape::Rejected,
        ),
        (
            "single nullable branch",
            "Base",
            "oneOf",
            single,
            vec![],
            Shape::Fields(&["a", "b", "c"], false),
        ),
    ];
    // A further conjunct `required: [a]`, which every instance satisfies as well, written as an
    // `allOf` member that is either those untyped object keywords alone (a member that refines the
    // union's object branches) or the same keywords nested in an `allOf` of their own (a member
    // combined with the target). `null` satisfies the target, the conjunct and both branches, so
    // the `anyOf` admits it and the `oneOf`, which it matches twice, does not. The `$ref` spelling
    // writes the conjunct's keywords beside the `$ref`, `{$ref: NB, required: [a], oneOf|anyOf:
    // […]}` (#538), and its nested-`allOf` form `{$ref: NB, allOf: [{required: [a]}], …}`, whose
    // untyped `allOf` admits the target's `null` without deciding it (#562).
    let mut conjunct_cases = Vec::new();
    //
    // A typed conjunct, a nullable object with a further property `c`, meets the target before the
    // union rather than refining its branches, and admits `null` of its own accord.
    let ab: &[&str] = &["a", "b"];
    let abc: &[&str] = &["a", "b", "c"];
    for (form, conjunct, fields) in [
        (
            "refiner member",
            ("{ required: [a] }", ", required: [a]"),
            ab,
        ),
        (
            "nested allOf member",
            (
                "{ allOf: [ { required: [a] } ] }",
                ", allOf: [ { required: [a] } ]",
            ),
            ab,
        ),
        (
            "typed member",
            (
                "{ type: [object, 'null'], properties: { c: { type: integer } } }",
                ", type: [object, 'null'], properties: { c: { type: integer } }",
            ),
            abc,
        ),
    ] {
        for (keyword, nullable) in [("oneOf", false), ("anyOf", true)] {
            conjunct_cases.push((
                format!("nullable target, {form}, {keyword}"),
                "NB",
                keyword,
                required,
                vec![Code::ValidationKeywordIgnored],
                Shape::Fields(fields, nullable),
                Some(conjunct),
            ));
        }
    }
    let cases = cases
        .into_iter()
        .map(|(case, target, keyword, branches, codes, shape)| {
            (
                case.to_owned(),
                target,
                keyword,
                branches,
                codes,
                shape,
                None,
            )
        })
        .chain(conjunct_cases);
    for (case, target, keyword, branches, codes, shape, conjunct) in cases {
        let target_ref = format!("'#/components/schemas/{target}'");
        let member = conjunct.map_or_else(String::new, |(member, _)| format!(", {member}"));
        // The conjunct's keywords written beside the `$ref`, as further siblings.
        let keywords = conjunct.map_or("", |(_, keywords)| keywords);
        for (spelling, site) in [
            (
                "$ref sibling",
                format!("{{ $ref: {target_ref}{keywords}, {keyword}: {branches} }}"),
            ),
            (
                "allOf member",
                format!(
                    "{{ allOf: [ {{ $ref: {target_ref} }}{member}, {{ {keyword}: {branches} }} \
                     ] }}"
                ),
            ),
            (
                "beside allOf",
                format!("{{ allOf: [ {{ $ref: {target_ref} }}{member} ], {keyword}: {branches} }}"),
            ),
        ] {
            let spec = format!(
                r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
servers: [{{ url: 'https://e.com' }}]
paths:
  /p:
    get:
      operationId: fetch
      responses:
        '200':
          description: ok
          content:
            application/json: {{ schema: {{ $ref: '#/components/schemas/Holder' }} }}
components:
  schemas:
{base}{nullable_base}{name}    Pick: {site}
    Holder:
      type: object
      properties:
        pick: {{ $ref: '#/components/schemas/Pick' }}
      required: [pick]
"##
            );
            let label = format!("{case}, {spelling}");
            let (report, code) = generate_with_code(&spec);
            let at_pick: Vec<Code> = report
                .diagnostics()
                .iter()
                .filter(|d| d.pointer.as_str() == "/components/schemas/Pick")
                .map(|d| d.code)
                .collect();
            assert_eq!(at_pick, codes, "{label}: {report:#?}\n{spec}");
            let types = types_module(&code);
            match shape {
                Shape::Rejected => {
                    assert_eq!(report.outcome(), Outcome::Rejected, "{label}: {report:#?}");
                }
                Shape::Fields(fields, nullable) => {
                    assert_ne!(report.outcome(), Outcome::Rejected, "{label}: {report:#?}");
                    assert_eq!(declared_fields(&types, "Pick"), fields, "{label}: {types}");
                    let pick = field_type(&types, "pub pick")
                        .unwrap_or_else(|| panic!("{label}: no `pick` field: {types}"));
                    assert_eq!(
                        pick.starts_with("Option<"),
                        nullable,
                        "{label}: `pick` is `{pick}`: {types}"
                    );
                }
                Shape::Variants(variants) => {
                    assert_ne!(report.outcome(), Outcome::Rejected, "{label}: {report:#?}");
                    assert_eq!(enum_variants(&types, "Pick"), variants, "{label}: {types}");
                }
            }
        }
    }
}

/// Several `oneOf`/`anyOf` members beside an object member are not met with it: their meet would
/// nest one union in another's branches, so the composition is rejected with the stable `E013` it
/// has always drawn rather than emitting a union of identical variants (#463).
#[test]
fn an_all_of_with_several_union_members_beside_an_object_is_rejected() {
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
            application/json: { schema: { $ref: '#/components/schemas/Pick' } }
components:
  schemas:
    Base:
      type: object
      properties:
        a: { type: string }
        b: { type: string }
    Pick:
      allOf:
        - $ref: '#/components/schemas/Base'
        - oneOf: [ { required: [a] }, { required: [b] } ]
        - anyOf: [ { required: [a] }, { required: [b] } ]
"##;
    let report = generate(spec);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        report
            .diagnostics()
            .iter()
            .any(|d| d.code == Code::AllOfIrreconcilable
                && d.pointer.as_str() == "/components/schemas/Pick"),
        "{report:#?}"
    );
}

/// Issue #419: a schema carrying `allOf` and `oneOf`/`anyOf` together lowered through the `allOf`
/// arm alone, which never read the union, so the union — and a `discriminator` beside it — was
/// dropped and `check` called the document clean. The two are a conjunction: the `allOf`
/// composition is met with the union branch by branch, as a `$ref` is met with a union target, so
/// a branch the composition excludes drops out. Where no branch is left, or a meet has no single
/// type, it is `E013` at the schema.
#[test]
fn a_union_beside_all_of_that_contradicts_it_is_rejected() {
    let cases = [
        (
            "issue reproduction (oneOf)",
            "    Pet:\n      \
             allOf: [ { type: object, properties: { id: { type: integer } } } ]\n      \
             oneOf: [ { type: string } ]\n",
            "/components/schemas/Pet",
        ),
        (
            "anyOf",
            "    Pet:\n      \
             allOf: [ { type: object, properties: { id: { type: integer } } } ]\n      \
             anyOf: [ { type: string } ]\n",
            "/components/schemas/Pet",
        ),
        (
            "scalar allOf against a disjoint union",
            "    Pet:\n      \
             allOf: [ { type: integer } ]\n      \
             oneOf: [ { type: string }, { type: boolean } ]\n",
            "/components/schemas/Pet",
        ),
        (
            "nested allOf member carrying a union",
            "    Pet:\n      allOf:\n        \
             - allOf: [ { type: object, properties: { id: { type: integer } } } ]\n          \
             oneOf: [ { type: string } ]\n",
            "/components/schemas/Pet/allOf/0",
        ),
    ];
    for (what, schemas, pointer) in cases {
        let spec = with_schemas("3.1.0", schemas);
        for (entry, report) in [("generate", generate(&spec)), ("check", check(&spec))] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "{what} via {entry}: {report:#?}\n{spec}"
            );
            assert!(
                report
                    .diagnostics()
                    .iter()
                    .any(|d| d.code == Code::AllOfIrreconcilable && d.pointer.as_str() == pointer),
                "{what} via {entry}: E013 must sit at `{pointer}`: {report:#?}"
            );
        }
    }
}

/// Issue #419, the inline-member spelling: an `allOf` member carrying object keywords beside its
/// own `oneOf` was read as an object by its keywords and its union dropped. The member is lowered
/// as the union it is, its object keywords refining the branches of their own category, so an
/// explicit `type: object` beside a string-only union leaves no branch and is reported, and untyped
/// `properties` beside it constrain nothing the union accepts.
#[test]
fn an_all_of_member_with_its_own_union_beside_object_keywords_keeps_the_union() {
    let typed = with_schemas(
        "3.1.0",
        "    Pet:\n      allOf:\n        \
         - type: object\n          properties: { id: { type: integer } }\n          \
         oneOf: [ { type: string } ]\n",
    );
    for (entry, report) in [("generate", generate(&typed)), ("check", check(&typed))] {
        assert_eq!(
            report.outcome(),
            Outcome::Rejected,
            "via {entry}: an object no string is: {report:#?}"
        );
        assert!(
            has_code(&report, Code::NonDisjointUnion),
            "via {entry}: {report:#?}"
        );
    }

    let untyped = with_schemas(
        "3.1.0",
        "    Pet:\n      allOf:\n        \
         - properties: { id: { type: integer } }\n          \
         oneOf: [ { type: string } ]\n",
    );
    let (report, code) = generate_with_code(&untyped);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    let types = types_module(&code);
    assert!(
        types.contains("pub type Pet = String;"),
        "the member is a string, not a struct with `id`:\n{types}"
    );
    assert!(
        !types.contains("pub struct Pet "),
        "the union must not be dropped for the object keywords:\n{types}"
    );
}

/// Issue #419: where the conjunction has a type, it is the union narrowed by the `allOf`, not the
/// `allOf` alone. An annotation-only `allOf` leaves the discriminated union exactly as the same
/// schema without it generates; a scalar `allOf` narrows the union to the branches it admits.
#[test]
fn a_union_beside_all_of_composes_with_it() {
    let union = "      oneOf: [ { $ref: '#/components/schemas/Cat' }, { type: string } ]\n      \
                 discriminator: { propertyName: kind, mapping: { cat: Cat } }\n";
    let bare = with_schemas("3.1.0", &format!("    Pet:\n{union}"));
    let shadowed = with_schemas(
        "3.1.0",
        &format!("    Pet:\n      allOf: [ {{ description: a pet }} ]\n{union}"),
    );
    let (bare_report, bare_code) = generate_with_code(&bare);
    assert_ne!(bare_report.outcome(), Outcome::Rejected, "{bare_report:#?}");
    assert_eq!(
        enum_variants(&bare_code, "Pet"),
        ["Cat(Box<Cat>)", "PetVariant1(Box<PetVariant1>)"],
        "{bare_code}"
    );
    let (report, code) = generate_with_code(&shadowed);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(report.diagnostics().is_empty(), "{report:#?}");
    assert_eq!(types_module(&code), types_module(&bare_code));
    assert!(
        code.contains("\"cat\""),
        "the discriminator still dispatches:\n{code}"
    );
    let checked = check(&shadowed);
    assert!(checked.diagnostics().is_empty(), "{checked:#?}");

    // The `allOf` polymorphism parent spelled beside its own union: every branch is met with the
    // shared base, so each variant carries the base's `id` as well as its own fields.
    let based = with_schemas(
        "3.1.0",
        "    Base: { type: object, required: [id], properties: { id: { type: integer } } }\n    \
         Dog: { type: object, required: [kind], properties: { kind: { type: string }, bark: { type: boolean } } }\n    \
         Pet:\n      allOf: [ { $ref: '#/components/schemas/Base' } ]\n      \
         oneOf: [ { $ref: '#/components/schemas/Cat' }, { $ref: '#/components/schemas/Dog' } ]\n      \
         discriminator: { propertyName: kind }\n",
    );
    let (report, code) = generate_with_code(&based);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    let types = types_module(&code);
    let variants = enum_variants(&types, "Pet");
    assert_eq!(variants.len(), 2, "{types}");
    for variant in &variants {
        let payload = variant
            .split_once("(Box<")
            .and_then(|(_, rest)| rest.strip_suffix(">)"))
            .unwrap_or_else(|| panic!("{variant}: {types}"));
        let fields = declared_fields(&types, payload);
        assert!(
            fields.iter().any(|field| field == "id") && fields.iter().any(|field| field == "kind"),
            "{payload} must carry the base's `id` beside its own `kind`: {fields:?}\n{types}"
        );
    }

    let narrowed = with_schemas(
        "3.1.0",
        "    Pet:\n      allOf: [ { type: integer } ]\n      \
         oneOf: [ { type: string }, { type: integer, format: int32 } ]\n",
    );
    let (report, code) = generate_with_code(&narrowed);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    let types = types_module(&code);
    assert!(
        types.contains("pub type Pet = i32;"),
        "only the integer branch meets `type: integer`:\n{types}"
    );
}

/// Issue #419: an `allOf` beside the schema's own `oneOf`/`anyOf` excludes only the branches it
/// constrains away. The schema's own keywords beside both are the union's siblings, refining its
/// branches of their own category, and are not also an object member of the composition; an
/// untyped `allOf` member is refined into the union the same way, as a `$ref` to a union refines
/// with untyped siblings. Either, read as an object of the composition, dropped every string and
/// `null` branch, so the generated client refused values the description accepts. A typed member
/// that admits `null` keeps the union's `null` branch.
#[test]
fn a_union_beside_all_of_keeps_the_branches_and_null_the_composition_admits() {
    let mixed = [
        (
            "the schema's own `required` beside an annotation-only allOf",
            "    Pet:\n      required: [kind]\n      allOf: [ { description: a pet } ]\n      \
             oneOf: [ { type: string }, { $ref: '#/components/schemas/Cat' } ]\n",
        ),
        (
            "an untyped `required` allOf member",
            "    Pet:\n      allOf: [ { required: [kind] } ]\n      \
             oneOf: [ { type: string }, { $ref: '#/components/schemas/Cat' } ]\n",
        ),
    ];
    for (what, schemas) in mixed {
        let spec = with_schemas("3.1.0", schemas);
        let (report, code) = generate_with_code(&spec);
        assert_ne!(report.outcome(), Outcome::Rejected, "{what}: {report:#?}");
        let types = types_module(&code);
        assert!(
            !types.contains("pub struct Pet "),
            "{what}: the string branch must survive:\n{types}"
        );
        let variants = enum_variants(&types, "Pet");
        assert_eq!(variants.len(), 2, "{what}: {variants:?}\n{types}");
        assert!(
            variants.iter().any(|variant| {
                variant
                    .split_once("(Box<")
                    .and_then(|(_, rest)| rest.strip_suffix(">)"))
                    .is_some_and(|payload| types.contains(&format!("pub type {payload} = String;")))
            }),
            "{what}: one variant is the string branch: {variants:?}\n{types}"
        );
        let checked = check(&spec);
        assert_ne!(checked.outcome(), Outcome::Rejected, "{what}: {checked:#?}");
    }

    let owner = "    Owner:\n      type: object\n      required: [p]\n      \
                 properties: { p: { $ref: '#/components/schemas/Pet' } }\n";
    let nullable = [
        (
            "the schema's own `properties` beside an annotation-only allOf",
            "    Pet:\n      properties: { name: { type: string } }\n      \
             allOf: [ { description: a pet } ]\n      \
             oneOf: [ { $ref: '#/components/schemas/Cat' }, { type: 'null' } ]\n",
        ),
        (
            "an untyped `properties` allOf member",
            "    Pet:\n      allOf: [ { properties: { name: { type: string } } } ]\n      \
             oneOf: [ { $ref: '#/components/schemas/Cat' }, { type: 'null' } ]\n",
        ),
        (
            "a typed allOf member admitting null",
            "    Pet:\n      \
             allOf: [ { type: [object, 'null'], properties: { name: { type: string } } } ]\n      \
             oneOf: [ { $ref: '#/components/schemas/Cat' }, { type: 'null' } ]\n",
        ),
    ];
    for (what, schemas) in nullable {
        let spec = with_schemas("3.1.0", &format!("{owner}{schemas}"));
        let (report, code) = generate_with_code(&spec);
        assert_ne!(report.outcome(), Outcome::Rejected, "{what}: {report:#?}");
        let types = types_module(&code);
        let p = field_type(&types, "pub p:").unwrap_or_else(|| panic!("{what}: no `p`\n{types}"));
        assert!(
            p.starts_with("Option<"),
            "{what}: `null` satisfies both, so `p` is nullable, not `{p}`:\n{types}"
        );
        let fields = declared_fields(&types, "Pet");
        assert!(
            fields.iter().any(|field| field == "name")
                && fields.iter().any(|field| field == "kind"),
            "{what}: the object branch is `Cat` refined by `name`: {fields:?}\n{types}"
        );
        let checked = check(&spec);
        assert_ne!(checked.outcome(), Outcome::Rejected, "{what}: {checked:#?}");
    }
}

/// Issue #419: a union of two or more non-null branches plus `null` stays a nullable union when
/// an untyped `allOf` member refines it, as it does when the same keywords sit beside a `$ref`
/// to it or beside the union itself. The untyped keywords say nothing about `null`, so the union's
/// `null` branch survives the refinement. The branch-by-branch meet built a new union without it,
/// and a required property reaching the schema was typed `Pet`, refusing the `null` the
/// description accepts.
#[test]
fn an_untyped_all_of_member_keeps_a_multi_branch_unions_null() {
    let owner = "    Owner:\n      type: object\n      required: [p]\n      \
                 properties: { p: { $ref: '#/components/schemas/Pet' } }\n";
    let union =
        "oneOf: [ { type: string }, { $ref: '#/components/schemas/Cat' }, { type: 'null' } ]";
    let cases = [
        (
            "an untyped `required` allOf member",
            format!("    Pet:\n      allOf: [ {{ required: [kind] }} ]\n      {union}\n"),
        ),
        (
            "an untyped `properties` allOf member",
            format!(
                "    Pet:\n      allOf: [ {{ properties: {{ name: {{ type: string }} }} }} ]\n      \
                 {union}\n"
            ),
        ),
        (
            "two untyped allOf members",
            format!(
                "    Pet:\n      allOf: [ {{ required: [kind] }}, \
                 {{ properties: {{ name: {{ type: string }} }} }} ]\n      {union}\n"
            ),
        ),
        (
            "the schema's own `required` beside an annotation-only allOf",
            format!(
                "    Pet:\n      required: [kind]\n      allOf: [ {{ description: a pet }} ]\n      \
                 {union}\n"
            ),
        ),
        (
            "the same keywords beside a `$ref` to the union",
            format!(
                "    Pet:\n      $ref: '#/components/schemas/U'\n      required: [kind]\n    \
                 U:\n      {union}\n"
            ),
        ),
    ];
    for (what, schemas) in cases {
        let spec = with_schemas("3.1.0", &format!("{owner}{schemas}"));
        let (report, code) = generate_with_code(&spec);
        assert_ne!(report.outcome(), Outcome::Rejected, "{what}: {report:#?}");
        let types = types_module(&code);
        let p = field_type(&types, "pub p:").unwrap_or_else(|| panic!("{what}: no `p`\n{types}"));
        assert_eq!(
            p, "Option<Pet>",
            "{what}: `null` satisfies both, so `p` is nullable:\n{types}"
        );
        let variants = enum_variants(&types, "Pet");
        assert_eq!(
            variants.len(),
            2,
            "{what}: both non-null branches survive: {variants:?}\n{types}"
        );
        let checked = check(&spec);
        assert_ne!(checked.outcome(), Outcome::Rejected, "{what}: {checked:#?}");
    }
}

/// Issue #419, the `$ref` spelling of an `allOf` member beside a union (decision 10): the member
/// is never a scoped refiner, so the union meets the target's lowered component type. A target
/// holding only `required` lowers to an untyped schema, which excludes no branch, so the string
/// branch and the `null` both survive, as with the inline spelling. A target holding `properties`
/// lowers to an object, so the meet keeps only the object branch and the string branch drops, with
/// no diagnostic. That target states no `type`, so it admits `null` without deciding it (#541),
/// and the `null` branch survives as well: `p` is `Option<Pet>`. This pins today's behaviour of
/// both, so a change to either is a visible diff.
#[test]
fn a_ref_all_of_member_beside_a_union_meets_its_targets_lowered_type() {
    let owner = "    Owner:\n      type: object\n      required: [p]\n      \
                 properties: { p: { $ref: '#/components/schemas/Pet' } }\n";
    let with_string =
        "oneOf: [ { type: string }, { $ref: '#/components/schemas/Cat' }, { type: 'null' } ]";
    let without_string = "oneOf: [ { $ref: '#/components/schemas/Cat' }, { type: 'null' } ]";
    // (target, union, `p`'s type, `Pet`'s variant count: 0 where it is not an enum).
    let cases = [
        (
            "    HasKind:\n      required: [kind]\n",
            with_string,
            "Option<Pet>",
            2,
        ),
        (
            "    HasKind:\n      required: [kind]\n",
            without_string,
            "Option<Pet>",
            0,
        ),
        (
            "    HasName:\n      properties: { name: { type: string } }\n",
            with_string,
            "Option<Pet>",
            0,
        ),
        (
            "    HasName:\n      properties: { name: { type: string } }\n",
            without_string,
            "Option<Pet>",
            0,
        ),
    ];
    for (target, union, p_type, variant_count) in cases {
        let name = target.trim().split(':').next().unwrap_or_default();
        let what = format!("{name} beside {union}");
        let spec = with_schemas(
            "3.1.0",
            &format!(
                "{owner}{target}    Pet:\n      \
                 allOf: [ {{ $ref: '#/components/schemas/{name}' }} ]\n      {union}\n"
            ),
        );
        let (report, code) = generate_with_code(&spec);
        assert_eq!(report.outcome(), Outcome::Generated, "{what}: {report:#?}");
        assert!(report.diagnostics().is_empty(), "{what}: {report:#?}");
        let types = types_module(&code);
        let p = field_type(&types, "pub p:").unwrap_or_else(|| panic!("{what}: no `p`\n{types}"));
        assert_eq!(p, p_type, "{what}:\n{types}");
        let variants = enum_variants(&types, "Pet");
        assert_eq!(
            variants.len(),
            variant_count,
            "{what}: {variants:?}\n{types}"
        );
        if name == "HasName" {
            assert!(
                types.contains("pub struct Pet {"),
                "{what}: the meet keeps only the object branch:\n{types}"
            );
        }
        let checked = check(&spec);
        assert!(checked.diagnostics().is_empty(), "{what}: {checked:#?}");
    }
}

/// Issue #419: an untyped `allOf` member beside a union is reported as the same keywords are
/// beside a `$ref` to it. Reaching no branch of its category it constrains nothing the union
/// accepts (`W011` at the member, and the union generates); object and array keywords together
/// against a branch that states no category are `E013` at the member; and a meet with the one
/// object branch that leaves no value is `E013` at the schema carrying both keywords.
#[test]
fn an_untyped_all_of_member_beside_a_union_is_reported_as_a_ref_sibling_is() {
    let unreached = with_schemas(
        "3.1.0",
        "    Pet:\n      allOf: [ { required: [kind] } ]\n      \
         oneOf: [ { type: string }, { type: integer } ]\n",
    );
    for (entry, report) in [
        ("generate", generate(&unreached)),
        ("check", check(&unreached)),
    ] {
        assert_ne!(
            report.outcome(),
            Outcome::Rejected,
            "via {entry}: {report:#?}"
        );
        assert!(
            report
                .diagnostics()
                .iter()
                .any(|d| d.code == Code::DeclarationHasNoEffect
                    && d.pointer.as_str() == "/components/schemas/Pet/allOf/0"),
            "via {entry}: W011 must sit at the member: {report:#?}"
        );
    }

    let rejected = [
        (
            "both kinds against a branch that states no category",
            "    Pet:\n      allOf: [ { required: [kind], items: { type: string } } ]\n      \
             oneOf: [ { type: string }, { description: anything } ]\n",
            "/components/schemas/Pet/allOf/0",
        ),
        (
            "a property type the object branch contradicts",
            "    Pet:\n      \
             allOf: [ { required: [kind], properties: { kind: { type: integer } } } ]\n      \
             oneOf: [ { $ref: '#/components/schemas/Cat' } ]\n",
            "/components/schemas/Pet",
        ),
    ];
    for (what, schemas, pointer) in rejected {
        let spec = with_schemas("3.1.0", schemas);
        for (entry, report) in [("generate", generate(&spec)), ("check", check(&spec))] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "{what} via {entry}: {report:#?}"
            );
            assert!(
                report
                    .diagnostics()
                    .iter()
                    .any(|d| d.code == Code::AllOfIrreconcilable && d.pointer.as_str() == pointer),
                "{what} via {entry}: E013 must sit at `{pointer}`: {report:#?}"
            );
        }
    }
}

/// A single-member `allOf` now MERGES into one typed struct instead of being rejected (E013 is
/// repurposed to mean "irreconcilable composition"). Generation succeeds with no E013.
#[test]
fn all_of_single_member_merges_into_struct() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Composed:
      allOf:
        - type: object
          properties:
            a: { type: string }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::AllOfIrreconcilable), "{report:#?}");

    let types = types_module(&code);
    assert_eq!(declared_fields(&types, "Composed"), ["a"], "{types}");
    assert_eq!(
        field_owner(&types, "pub a:").as_deref(),
        Some("Composed"),
        "{types}"
    );
}

/// `allOf: [{$ref: Base}, {properties: {extra}}]` flattens the referenced component's fields plus
/// the inline member's fields into one struct.
#[test]
fn all_of_ref_plus_inline_members_merge() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Base:
      type: object
      required: [id]
      properties:
        id: { type: string }
    Derived:
      allOf:
        - $ref: "#/components/schemas/Base"
        - type: object
          properties:
            extra: { type: integer }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::AllOfIrreconcilable), "{report:#?}");

    // `Derived` carries `Base`'s required `id` (plain) and the inline member's optional `extra`.
    let types = types_module(&code);
    assert_eq!(
        declared_fields(&types, "Derived"),
        ["id", "extra"],
        "{types}"
    );
    let derived = &types[types
        .find("pub struct Derived ")
        .expect("Derived is a struct")..];
    assert_eq!(
        field_type(derived, "pub id:").as_deref(),
        Some("Baseid"),
        "{types}"
    );
    assert_eq!(
        field_type(derived, "pub extra:").as_deref(),
        Some("Option<DerivedMember1extra>"),
        "{types}"
    );
    assert_eq!(
        alias_target(&types, "DerivedMember1extra").as_deref(),
        Some("i64"),
        "{types}"
    );
}

/// A nested `allOf` (an `allOf` member that itself has an `allOf`) flattens recursively into one
/// struct.
#[test]
fn all_of_nested_merges() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Nested:
      allOf:
        - allOf:
            - type: object
              properties:
                a: { type: string }
        - type: object
          properties:
            b: { type: string }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::AllOfIrreconcilable), "{report:#?}");

    // One struct carrying the inner `allOf`'s `a` beside the outer member's `b`.
    let types = types_module(&code);
    assert_eq!(declared_fields(&types, "Nested"), ["a", "b"], "{types}");
}

/// `allOf` beside the enclosing schema's own sibling `properties`: both sets of fields merge.
#[test]
fn all_of_beside_sibling_properties_merges() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Sibling:
      type: object
      properties:
        own: { type: string }
      allOf:
        - type: object
          properties:
            base: { type: string }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::AllOfIrreconcilable), "{report:#?}");

    // Both sets of fields land in the one struct.
    let types = types_module(&code);
    assert_eq!(
        declared_fields(&types, "Sibling"),
        ["base", "own"],
        "{types}"
    );
}

/// Repeated properties in an `allOf` are intersections. Compatible refinements retain the narrower
/// typed shape recursively: integer within number, enum within string, non-null within nullable,
/// exact null, and nested array/object item constraints.
#[test]
fn all_of_recursively_intersects_compatible_property_types() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Refined:
      allOf:
        - type: object
          properties:
            run_id: { type: number }
            status: { type: string }
            marker: { type: [string, "null"] }
            items:
              type: array
              items: { type: [object, "null"] }
        - type: object
          properties:
            run_id: { type: integer }
            status: { type: string, enum: [queued, complete] }
            marker: { type: "null" }
            items:
              type: array
              items:
                type: object
                required: [name]
                properties:
                  name: { type: string }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::AllOfIrreconcilable), "{report:#?}");

    let checked = check(spec);
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
    assert!(
        !has_code(&checked, Code::AllOfIrreconcilable),
        "{checked:#?}"
    );

    // Each property keeps the narrower side of its intersection, read through the merged
    // struct's own field types rather than from a type name appearing somewhere.
    let types = types_module(&code);
    let refined = &types[types
        .find("pub struct Refined ")
        .expect("Refined is a struct")..];
    let target = |field: &str| {
        let ty = field_type(refined, &format!("pub {field}:"))
            .unwrap_or_else(|| panic!("`Refined` has no `{field}` field: {types}"));
        let inner = ty
            .strip_prefix("Option<")
            .and_then(|rest| rest.strip_suffix('>'))
            .unwrap_or_else(|| panic!("optional `{field}` is `{ty}`: {types}"));
        alias_target(&types, inner).unwrap_or_else(|| format!("enum or struct {inner}"))
    };
    // integer within number.
    assert_eq!(target("run_id"), "i64", "{types}");
    // enum within string: a two-variant enum, not a plain `String`.
    let status = field_type(refined, "pub status:").expect("a status field");
    let status = status.trim_start_matches("Option<").trim_end_matches('>');
    assert_eq!(
        enum_variants(&types, status),
        ["Queued", "Complete"],
        "{types}"
    );
    // exact null within nullable string.
    assert_eq!(target("marker"), "()", "{types}");
    // A non-null item struct carrying `name`, within nullable untyped items.
    let items = target("items");
    let item = items
        .strip_prefix("Vec<")
        .and_then(|rest| rest.strip_suffix('>'))
        .unwrap_or_else(|| panic!("`items` is `{items}`: {types}"));
    assert!(
        !item.starts_with("Option<"),
        "the item stayed nullable: {types}"
    );
    assert_eq!(declared_fields(&types, item), ["name"], "{types}");
}

#[test]
fn e013_all_of_conflicting_property_types_rejected() {
    let report = generate(ALL_OF_CONFLICT_SPEC);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::AllOfIrreconcilable));
}

/// Mixing an object member with a scalar member has no single representable type → E013.
#[test]
fn e013_all_of_object_scalar_mix_rejected() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Mixed:
      allOf:
        - type: object
          properties:
            a: { type: string }
        - type: string
"##;
    let report = generate(spec);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::AllOfIrreconcilable));
}

/// Lay out a root document whose `Wrap` component is `allOf: [member]`, beside a `lib.yaml`
/// sub-file, and run both entry points. Returns `(generate, check, generated source)`.
fn all_of_member_layout(member: &str) -> (Report, Report, String) {
    let temp = tempfile::tempdir().unwrap();
    let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
    std::fs::write(
        dir.join("openapi.yaml"),
        format!(
            "openapi: 3.1.0\n\
             info: {{ title: T, version: 1.0.0 }}\n\
             servers: [{{ url: 'https://e.com' }}]\n\
             paths:\n  \
             /w:\n    \
             get:\n      \
             operationId: getW\n      \
             responses:\n        \
             '200':\n          \
             description: ok\n          \
             content:\n            \
             application/json:\n              \
             schema: {{ $ref: '#/components/schemas/Wrap' }}\n\
             components:\n  \
             schemas:\n    \
             Wrap:\n      \
             allOf:\n        \
             - {member}\n    \
             Leaf:\n      \
             type: object\n      \
             properties: {{ a: {{ type: string }} }}\n"
        ),
    )
    .unwrap();
    std::fs::write(
        dir.join("lib.yaml"),
        "Leaf:\n  type: object\n  properties: { a: { type: string } }\n",
    )
    .unwrap();
    let out = dir.join("client.rs");
    let generated = run_generate(&build(dir.join("openapi.yaml"), out.clone()));
    let code = std::fs::read_to_string(&out).unwrap_or_default();
    let checked = run_check(&Spec::new(dir.join("openapi.yaml")));
    (generated, checked, code)
}

/// Issue #185: an `allOf` member that is a `$ref` with shape-bearing siblings used to contribute
/// its target alone. `gather_member` returned as soon as it had pushed the target, so the member's
/// own `properties`/`required`/`type` never reached the merge: a property vanished from the
/// generated client with no diagnostic, and a member contradicting its target generated a type
/// that looked inhabited. `$ref` is an applicator, so the member is the target AND its siblings,
/// and inside an `allOf` that is two more conjuncts of the same merge.
///
/// Both local arms are driven: the root-component spelling and the file-reference spelling, which
/// takes the inlining arm. The remote arm has its own fixture in `mod remote`.
#[test]
fn an_all_of_member_ref_intersects_its_shape_bearing_siblings() {
    for target in ["#/components/schemas/Leaf", "./lib.yaml#/Leaf"] {
        // The dropped-property case: the member adds a required field its target lacks.
        let (generated, checked, code) = all_of_member_layout(&format!(
            "{{ $ref: '{target}', type: object, properties: {{ dropped: {{ type: integer }} }}, \
             required: [dropped] }}"
        ));
        for (entry, report) in [("generate", &generated), ("check", &checked)] {
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "{target} via {entry}: {report:#?}"
            );
        }
        assert_eq!(
            declared_fields(&code, "Wrap"),
            vec!["a", "dropped"],
            "{target}: the member's own property must survive beside its target's: {code}"
        );
        let dropped = field_type(&code, "pub dropped").unwrap_or_default();
        assert!(
            !dropped.starts_with("Option<"),
            "{target}: the member's `required` must survive too: {code}"
        );
        assert!(
            code.contains(&format!("pub type {dropped} = i64;")),
            "{target}: the member's property keeps its own `integer` type: {code}"
        );

        // The contradictory cases, which are empty intersections and must be E013 at `Wrap`: a
        // property both sides type differently that the member requires, and a scalar sibling
        // beside an object target.
        for siblings in [
            "properties: { a: { type: integer } }, required: [a]",
            "type: string",
        ] {
            let (generated, checked, _) =
                all_of_member_layout(&format!("{{ $ref: '{target}', {siblings} }}"));
            for (entry, report) in [("generate", &generated), ("check", &checked)] {
                assert_eq!(
                    report.outcome(),
                    Outcome::Rejected,
                    "{target} + `{siblings}` via {entry} must be rejected: {report:#?}"
                );
                let pointers: Vec<&str> = report
                    .diagnostics()
                    .iter()
                    .filter(|d| d.code == Code::AllOfIrreconcilable)
                    .map(|d| d.pointer.as_str())
                    .collect();
                assert_eq!(
                    pointers,
                    vec!["/components/schemas/Wrap"],
                    "{target} + `{siblings}` via {entry}: {report:#?}"
                );
            }
        }

        // An optional contradiction empties only the property, not the object. The one-member
        // spelling must type it exactly as the two-member spelling of the same conjunction does
        // (`allOf: [{$ref}, {properties}]`), and never as the target's `String`.
        let (generated, _, one_member) = all_of_member_layout(&format!(
            "{{ $ref: '{target}', properties: {{ a: {{ type: integer }} }} }}"
        ));
        assert_ne!(generated.outcome(), Outcome::Rejected, "{generated:#?}");
        let (split_report, _, two_members) = all_of_member_layout(&format!(
            "{{ $ref: '{target}' }}\n        - {{ properties: {{ a: {{ type: integer }} }} }}"
        ));
        assert_ne!(
            split_report.outcome(),
            Outcome::Rejected,
            "{split_report:#?}"
        );
        assert_ne!(
            field_type(&one_member, "pub a").as_deref(),
            Some("Option<String>"),
            "{target}: the contradicted property must not keep the target's type: {one_member}"
        );
        // Field by field rather than whole modules: the two spellings name the dead per-member
        // alias of `a: integer` differently (`WrapMember0Constrainta`, `WrapMember1a`).
        assert_eq!(
            declared_fields(&one_member, "Wrap"),
            declared_fields(&two_members, "Wrap"),
            "{target}: one member and two members are one conjunction"
        );
        assert_eq!(
            field_type(&one_member, "pub a:"),
            field_type(&two_members, "pub a:"),
            "{target}: one member and two members are one conjunction"
        );
    }
}

/// A field an object carries only because its `required` names the key is no declaration of the
/// property, so in a composition it takes its type from the other sides as well: a side's
/// `additionalProperties` value schema constrains every key that side does not declare, and a
/// side that declares the property supplies its type and metadata outright, exactly as it does for
/// the same composition with no `required` in it. Before, the placeholder's own type (unconstrained,
/// or the requiring side's `additionalProperties` value) was intersected like a declaration: a typed
/// `additionalProperties` on the other side never reached it, so a string-valued key became
/// `serde_json::Value`; and a declaration on the other side was intersected with the requiring
/// side's value schema, which rejected a composition the same document without the `required`
/// generates.
#[test]
fn a_required_only_field_takes_its_type_from_every_side_of_a_composition() {
    const HEAD: &str = "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\nservers: [{ url: \
                        'https://e.com' }]\npaths: {}\ncomponents:\n  schemas:\n    Labels: { \
                        type: object, additionalProperties: { type: string } }\n    Base: { type: \
                        object, properties: { id: { type: integer } } }\n    Req: { type: object, \
                        required: [a] }\n";
    let types_for = |schema: &str| {
        let spec = format!("{HEAD}    Thing: {schema}\n");
        let checked = check(&spec);
        assert_ne!(
            checked.outcome(),
            Outcome::Rejected,
            "{schema}: {checked:#?}"
        );
        assert!(
            !has_code(&checked, Code::AllOfIrreconcilable),
            "{schema}: {checked:#?}"
        );
        let (report, code) = generate_with_code(&spec);
        assert_ne!(report.outcome(), Outcome::Rejected, "{schema}: {report:#?}");
        types_module(&code)
    };
    // The field of `Thing` itself: `Req` carries an `a` of its own.
    let thing_field = |types: &str, name: &str| {
        let prefix = format!("pub {name}: ");
        types
            .lines()
            .map(str::trim)
            .skip_while(|line| !line.starts_with("pub struct Thing "))
            .take_while(|line| *line != "}")
            .find_map(|line| line.strip_prefix(&prefix).map(str::to_owned))
    };
    let field = |schema: &str, name: &str| {
        let types = types_for(schema);
        let ty = thing_field(&types, name)
            .unwrap_or_else(|| panic!("`{schema}` generated no `{name}` field:\n{types}"))
            .trim_end_matches(',')
            .to_owned();
        // The field is written through its own named alias; report what that alias names.
        let alias = format!("pub type {ty} = ");
        let resolved = types
            .lines()
            .map(str::trim)
            .find_map(|line| line.strip_prefix(&alias))
            .map_or(ty.clone(), |rest| rest.trim_end_matches(';').to_owned());
        format!("pub {name}: {resolved},")
    };

    // The other side's typed `additionalProperties` reaches the required key, in the `$ref`
    // spelling and in both orders of the `allOf` spelling.
    for schema in [
        "{ $ref: '#/components/schemas/Labels', required: [a] }",
        "{ allOf: [{ $ref: '#/components/schemas/Labels' }, { required: [a] }] }",
        "{ allOf: [{ required: [a] }, { $ref: '#/components/schemas/Labels' }] }",
        "{ allOf: [{ $ref: '#/components/schemas/Labels' }, { $ref: '#/components/schemas/Req' }] }",
    ] {
        assert_eq!(field(schema, "a"), "pub a: String,", "{schema}");
    }

    // The other side's declaration is the property's type (required, so no `Option`), and the
    // requiring side's value schema does not reach it, as it does not without the `required`.
    for schema in [
        "{ allOf: [{ $ref: '#/components/schemas/Base' }, { type: object, additionalProperties: \
         { type: string }, required: [id] }] }",
        "{ allOf: [{ type: object, additionalProperties: { type: string }, required: [id] }, \
         { $ref: '#/components/schemas/Base' }] }",
        "{ $ref: '#/components/schemas/Base', additionalProperties: { type: string }, \
         required: [id] }",
    ] {
        assert_eq!(field(schema, "id"), "pub id: i64,", "{schema}");
    }

    // A component's required-only field keeps giving way when the component is copied into an
    // `allOf`: the later member's declaration supplies the metadata.
    let types = types_for(
        "{ allOf: [{ $ref: '#/components/schemas/Req' }, { type: object, properties: { a: { \
         type: string, deprecated: true } } }] }",
    );
    let lines: Vec<&str> = types
        .lines()
        .map(str::trim)
        .skip_while(|line| !line.starts_with("pub struct Thing "))
        .collect();
    let at = lines
        .iter()
        .position(|line| line.starts_with("pub a: "))
        .unwrap_or_else(|| panic!("no `a` field:\n{types}"));
    assert!(
        lines[..at]
            .iter()
            .rev()
            .take_while(|line| line.starts_with("#[") || line.starts_with("///"))
            .any(|line| line.contains("Deprecated per the spec")),
        "the declaring member's `deprecated` was lost to the component's placeholder:\n{types}"
    );

    // Every side constrains the undeclared key, so value schemas that share no value leave no
    // value for the required key, even where `additionalProperties: false` keeps the two maps
    // themselves from being intersected.
    let spec = format!(
        "{HEAD}    Thing: {{ allOf: [{{ $ref: '#/components/schemas/Labels' }}, {{ type: object, \
         additionalProperties: false }}, {{ type: object, additionalProperties: {{ type: integer \
         }}, required: [a] }}] }}\n"
    );
    for report in [generate(&spec), check(&spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        let messages = messages_for(&report, Code::AllOfIrreconcilable);
        assert_eq!(messages.len(), 1, "{report:#?}");
        assert!(
            messages[0].contains("property `a` is required but no `allOf` member declares it"),
            "{messages:?}"
        );
    }
}

/// `E013`'s explain is what `spargen explain E013` prints, and this change REWROTE it: the code was
/// repurposed from "irreconcilable allOf composition" to a composition-generic one, and the text
/// moved with it. Nothing held the new text to the code. Measured before this fixture existed:
/// reverting the explain to its `allOf`-only wording, and replacing it with text flatly
/// contradicting the code ("a `$ref` replaces the containing schema: its sibling keywords are
/// discarded, never intersected"), BOTH survived the entire suite.
///
/// The assertions are claims, not typography — a string-equality snapshot would pin the wording and
/// go stale on every edit without ever catching a false clause. Each one below is a promise some
/// other fixture in this suite enforces against the code, so the two cannot drift apart silently.
#[test]
fn the_composition_explain_covers_every_cause_that_reports_it() {
    let explain = Code::AllOfIrreconcilable.explain();

    // Both constructs that report it, and that `$ref` siblings are INTERSECTED rather than
    // discarded — the sentence the whole change exists to make true.
    assert!(explain.contains("`allOf`"), "{explain}");
    assert!(
        explain.contains("`$ref` is an applicator"),
        "the explain must say why a `$ref`'s siblings are not discarded: {explain}"
    );
    assert!(
        explain.contains("instead of being discarded"),
        "the explain must say the siblings are intersected rather than discarded: {explain}"
    );

    // The two-tier sibling-keyword rule, pinned against the code by
    // `every_sibling_keyword_the_explain_names_is_actually_intersected`.
    assert!(
        explain.contains("A sibling bears a shape of its own through"),
        "{explain}"
    );
    // The category rule for untyped applicators, and the consequences
    // `a_ref_sibling_applicator_establishes_its_category_and_keeps_the_targets_null`,
    // `an_untyped_union_sibling_refines_only_the_branches_of_its_category` and
    // `a_required_name_no_property_declares_is_still_required` pin against the code.
    assert!(
        explain.contains("establish an object and the array keywords"),
        "{explain}"
    );
    assert!(
        explain.contains("the target's nullability stands"),
        "{explain}"
    );
    assert!(
        explain.contains("refine only the branches of their own category"),
        "{explain}"
    );
    assert!(
        explain.contains("are acknowledged with `W011` rather than rejected"),
        "{explain}"
    );
    assert!(
        explain.contains("A `required` name no `properties` entry declares is still a requirement"),
        "{explain}"
    );
    assert!(
        explain.contains("In a composition such a field is no declaration of the property"),
        "{explain}"
    );

    // The array-arm doctrine. `intersect_structs` now mirrors it for an optional conflicting
    // property, so withdrawing this sentence would leave that behaviour unexplained.
    assert!(
        explain.contains("uninhabited item type"),
        "the explain must keep the doctrine that an empty item intersection stays representable: \
         {explain}"
    );

    // The hedge the round-1 repair put on every message that reports this code: `intersect_types`
    // fails for an empty intersection AND for an inhabited one with no single Rust type,
    // and the text must not claim the first when it may be the second.
    assert!(explain.contains("empty or unrepresentable"), "{explain}");

    // The rejection causes, including the recursive one all three spellings now share — stated as a
    // property of the document rather than of lowering order, which is what makes the verdict
    // reproducible when `components.schemas` is reordered.
    assert!(explain.contains("closes a reference cycle"), "{explain}");
    assert!(
        !explain.contains("not yet known") && !explain.contains("still being lowered"),
        "the explain still describes the recursive cause as a lowering-order fact, which the \
         guard no longer is: {explain}"
    );

    // Presence assertions cannot see a contradiction ADDED after them. Appending a paragraph
    // saying sibling keywords are "discarded and never intersected, so none of the above applies"
    // left every assertion above true and eleven suites green. The remedy is the last thing the
    // explain says, so anything appended moves it — which is a structural rule, not typography.
    assert!(
        explain
            .trim_end()
            .ends_with("omit this API segment with `spargen::omit!`."),
        "`E013`'s explain must end with its remedy; text after it can contradict everything \
         above and no presence assertion would notice: {explain}"
    );
    // And the in-place forms of the same contradiction.
    for denial in [
        "never intersected",
        "none of the above",
        "always exactly its target",
        "siblings are discarded",
    ] {
        assert!(
            !explain.contains(denial),
            "`E013`'s explain contradicts the code it documents (`{denial}`): {explain}"
        );
    }
}

/// An object `allOf` merge that is rejected from inside its member loop reports no `W005` for the
/// `default`s it had read by then: which member's `default` the merged field keeps is decided
/// once, after the loop (#577), and a rejected merge emits no field to keep one. Before #577 the
/// pairwise fold had already reported one of M0's `x` and M1's `y` (both required, so neither
/// applies) by the time M2 was read, so a partial, order-dependent `W005` stood beside the
/// rejection. Each case puts the rejecting member last, after the two conflicting `default`s, and
/// pins the three in-loop rejections: an `additionalProperties` conflict, a reference cycle
/// through an `additionalProperties` value schema, and a repeated property whose types share
/// values no single Rust type represents.
#[test]
fn an_all_of_merge_rejected_inside_its_member_loop_reports_no_default_it_read() {
    let base = "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\npaths: {}\ncomponents:\n  \
                schemas:\n    M0: { type: object, required: [a], properties: { a: { type: \
                string, default: x }, b: B0 }, additionalProperties: { type: string } }\n    \
                M1: { type: object, required: [a], properties: { a: { type: string, default: y \
                } } }\n    M2: M2BODY\n    Holder: { allOf: [{ $ref: '#/components/schemas/M0' \
                }, { $ref: '#/components/schemas/M1' }, { $ref: '#/components/schemas/M2' }] }\n";
    for (label, b0, m2, message) in [
        (
            "additionalProperties conflict",
            "{ type: string }",
            "{ type: object, additionalProperties: { type: integer } }",
            "`allOf` members declare conflicting `additionalProperties`",
        ),
        (
            "additionalProperties reference cycle",
            "{ type: string }",
            "{ type: object, additionalProperties: { $ref: '#/components/schemas/Holder' } }",
            "closes a reference cycle back to the schema being lowered",
        ),
        (
            "unrepresentable property meet",
            "{ type: array, prefixItems: [{ type: number }, { type: number }], items: false }",
            "{ type: object, properties: { b: { type: array, items: { type: string } } } }",
            "share values no single Rust type represents",
        ),
    ] {
        let spec = base.replace("B0", b0).replace("M2BODY", m2);
        for (entry, report) in [("generate", generate(&spec)), ("check", check(&spec))] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "{label}, {entry}: {report:#?}\n{spec}"
            );
            assert!(
                report
                    .diagnostics()
                    .iter()
                    .any(|d| d.code == Code::AllOfIrreconcilable && d.message.contains(message)),
                "{label}, {entry}: {report:#?}"
            );
            let reported: Vec<&str> = report
                .diagnostics()
                .iter()
                .filter(|d| d.code == Code::SchemaDefaultNotApplied)
                .map(|d| d.pointer.as_str())
                .collect();
            assert_eq!(
                reported,
                Vec::<&str>::new(),
                "{label}, {entry}: {report:#?}"
            );
        }
    }
}

/// An object `allOf` whose members repeat an object property meets that property pair by pair, and
/// the struct an earlier pair met it in is superseded by the next meet and not emitted (#428). The
/// post-lowering passes that report `W005` (#404) and `W006` read only emitted types, so a
/// superseded meet reports nothing of its own: three members say exactly what the two members
/// without the superseded one say. Before #428 the three-member `string`/`enum`/`integer` spelling
/// warned `W005` against the dead `string`∩`enum` struct while the two-member spelling, whose
/// result is the same uninhabited field, was clean.
///
/// This pins the two spellings agreeing. In the first two cases the emitted struct itself drops
/// the default — the field the first one empties included (#453) — and both spellings must report
/// it once.
#[test]
fn a_superseded_all_of_meet_reports_no_default_or_xml_diagnostic_of_its_own() {
    fn spec(media: &str, members: &[&str]) -> String {
        let mut spec = format!(
            "openapi: 3.1.0\ninfo: {{ title: T, version: 1.0.0 }}\nservers: [{{ url: 'https://e.com' }}]\n\
             paths:\n  /u:\n    get:\n      operationId: fetch\n      responses:\n        '200':\n          \
             description: ok\n          content:\n            {media}:\n              \
             schema:\n                allOf:\n",
        );
        for k in members {
            spec.push_str(&format!(
                "                  - {{ type: object, properties: {{ inner: {{ type: object, \
                 properties: {{ k: {k} }} }} }} }}\n"
            ));
        }
        spec
    }
    fn reported(report: &Report) -> Vec<(&'static str, String, String)> {
        let mut reported: Vec<_> = report
            .diagnostics()
            .iter()
            .filter(|d| matches!(d.code, Code::SchemaDefaultNotApplied | Code::XmlHintIgnored))
            .map(|d| (d.code.as_str(), d.pointer.to_string(), d.message.clone()))
            .collect();
        reported.sort();
        reported
    }

    // (body media type, three members, the two left once the superseded middle one is dropped,
    // `W005`/`W006` count)
    let json = "application/json";
    let cases: [(&str, [&str; 3], [&str; 2], usize); 4] = [
        (
            json,
            [
                "{ type: string, default: z }",
                "{ enum: [a, b] }",
                "{ type: integer }",
            ],
            ["{ type: string, default: z }", "{ type: integer }"],
            // The emptied field drops `z`, reported once at its `default` (#453).
            1,
        ),
        (
            json,
            [
                "{ type: string, default: z }",
                "{ enum: [a, b, c] }",
                "{ enum: [a, b] }",
            ],
            ["{ type: string, default: z }", "{ enum: [a, b] }"],
            1,
        ),
        (
            json,
            [
                "{ type: string, xml: { name: kay } }",
                "{ type: string, maxLength: 9 }",
                "{ type: string, minLength: 1 }",
            ],
            [
                "{ type: string, xml: { name: kay } }",
                "{ type: string, minLength: 1 }",
            ],
            // The first member's own `inner` struct and the merged one both ignore the rename.
            // The merged struct is located at the first member's `inner` (#454), so the two
            // `W006` are one diagnostic, at the hint the author wrote.
            1,
        ),
        // Under an XML body the merged struct is XML-dedicated and keeps its rename. Both spellings
        // report one `W006`, for the first member's own `inner` struct, which no body reaches. The
        // superseded meet is reached from no body either, so a pass reading it would report a
        // second `W006` for a type the output does not have.
        (
            "application/xml",
            [
                "{ type: string, xml: { name: kay } }",
                "{ type: string, maxLength: 9 }",
                "{ type: string, minLength: 1 }",
            ],
            [
                "{ type: string, xml: { name: kay } }",
                "{ type: string, minLength: 1 }",
            ],
            1,
        ),
    ];
    for (media, three, two, count) in cases {
        let (three, two) = (spec(media, &three), spec(media, &two));
        for (entry, run) in [
            ("generate", generate as fn(&str) -> Report),
            ("check", check),
        ] {
            let (with_superseded, without) = (run(&three), run(&two));
            assert_ne!(
                with_superseded.outcome(),
                Outcome::Rejected,
                "{entry}: {with_superseded:#?}"
            );
            assert_eq!(
                reported(&with_superseded),
                reported(&without),
                "{entry}: the superseded meet must report nothing the two-member spelling does \
                 not:\n{three}"
            );
            assert_eq!(reported(&without).len(), count, "{entry}: {without:#?}");
        }
    }
}

/// Three more sites re-emit a meet's kind under the enclosing schema's own name, as an all-scalar
/// `allOf` and a `$ref`-sibling intersection do (#401), so the meet's own definition is unused
/// unless the re-emitted kind refers to it (#462). Each leaked one: a union whose sole non-null
/// member meets the union's siblings left `…Constrained`, a union whose siblings exclude all but
/// one member left the survivor's `…Variant0Constrained`, and a `$ref` to a union refined by
/// untyped sibling keywords left the met union `…ReferenceIntersection`. Each is now discarded,
/// while a type the re-emitted kind does refer to — the `$ref` case's met branch, or the nested
/// struct a sole member's meet builds for a property both sides declare — is kept.
#[test]
fn a_re_emitted_union_meet_emits_no_meet_its_result_does_not_use() {
    fn spec(field: &str) -> String {
        format!(
            "openapi: 3.1.0\ninfo: {{ title: T, version: 1.0.0 }}\nservers: [{{ url: 'https://e.com' }}]\n\
             paths:\n  /u:\n    get:\n      operationId: fetch\n      responses:\n        '200':\n          \
             description: ok\n          content:\n            application/json:\n              \
             schema: {{ $ref: '#/components/schemas/Holder' }}\n\
             components:\n  schemas:\n    \
             Pet: {{ type: object, properties: {{ name: {{ type: string }} }} }}\n    \
             Box: {{ type: object, properties: {{ sub: {{ type: object, properties: {{ q: {{ type: string }} }} }} }} }}\n    \
             Target: {{ oneOf: [{{ type: object, properties: {{ name: {{ type: string }} }} }}, {{ type: string }}] }}\n    \
             Holder: {{ type: object, properties: {{ field: {field} }} }}\n"
        )
    }
    /// The types lowered for `Holder.field`, in source order.
    fn field_types(code: &str) -> Vec<String> {
        types_module(code)
            .lines()
            .map(str::trim_start)
            .filter_map(|line| {
                ["pub struct ", "pub enum ", "pub type "]
                    .iter()
                    .find_map(|item| line.strip_prefix(item))
            })
            .filter_map(|rest| rest.split([' ', '<', '{', '(', ';']).next())
            .filter(|name| name.starts_with("Holderfield"))
            .map(str::to_owned)
            .collect()
    }

    let cases: [(&str, &str, &[&str], &str); 4] = [
        (
            "sole non-null member",
            "{ oneOf: [{ $ref: '#/components/schemas/Pet' }, { type: 'null' }], required: [name] }",
            &["HolderfieldConstraint", "Holderfield"],
            "HolderfieldConstrained",
        ),
        (
            "sole non-null member whose meet builds a nested struct",
            "{ oneOf: [{ $ref: '#/components/schemas/Box' }, { type: 'null' }], \
             properties: { sub: { type: object, required: [q] } } }",
            &[
                "HolderfieldConstraintsubq",
                "HolderfieldConstraintsub",
                "HolderfieldConstraint",
                "HolderfieldConstrainedsub",
                "Holderfield",
            ],
            "HolderfieldConstrained",
        ),
        (
            "one member left by the siblings",
            "{ oneOf: [{ $ref: '#/components/schemas/Pet' }, { type: integer }], type: object, \
             required: [name] }",
            &[
                "HolderfieldConstraint",
                "HolderfieldVariant1",
                "Holderfield",
            ],
            "HolderfieldVariant0Constrained",
        ),
        (
            "$ref to a union with untyped siblings",
            "{ $ref: '#/components/schemas/Target', required: [name] }",
            &[
                "HolderfieldConstraint",
                "HolderfieldReferenceIntersectionVariant0",
                "Holderfield",
            ],
            "HolderfieldReferenceIntersection",
        ),
    ];
    for (case, field, expected, discarded) in cases {
        let (report, code) = generate_with_code(&spec(field));
        assert_eq!(report.outcome(), Outcome::Generated, "{case}: {report:#?}");
        let mut lowered = field_types(&code);
        // The sibling's own lowered field alias is emitted as every lowered sibling is, and is not
        // what this pins.
        lowered.retain(|name| name != "HolderfieldConstraintname");
        assert_eq!(lowered, expected, "{case}: `{discarded}` is unused");
    }
}

/// Issue #454: two `allOf` members that both declare an object-typed property `p` meet it in a
/// struct of its own (`PetpIntersection`). That struct carried the document root's provenance, so
/// a `W005` against it named the type as `` in `` `` and a `W006` against it had an empty pointer
/// and a span covering the whole document. Both must name the property whose types were met.
fn nested_meet_spec(first: &str, second: &str) -> String {
    format!(
        r##"
openapi: 3.1.0
info: {{ title: t, version: "1" }}
paths:
  /p:
    get:
      operationId: getP
      responses:
        "200":
          description: ok
          content:
            application/json:
              schema: {{ $ref: "#/components/schemas/Pet" }}
components:
  schemas:
    Pet:
      allOf:
        - type: object
          properties:
            p:
              type: object
              properties:
                {first}
        - type: object
          properties:
            p:
              type: object
              properties:
                {second}
"##
    )
}

#[test]
fn a_nested_all_of_meet_locates_its_diagnostics_at_the_met_property() {
    const MET: &str = "/components/schemas/Pet/allOf/0/properties/p";

    let w005 = nested_meet_spec(
        "a: { type: string, default: zzz }",
        "a: { type: string, enum: [a, b] }",
    );
    let w006 = nested_meet_spec(
        "a: { type: string, xml: { namespace: \"http://example.com/ns\" } }",
        "b: { type: string }",
    );
    for (entry, run) in [
        ("generate", generate as fn(&str) -> Report),
        ("check", check),
    ] {
        let report = run(&w005);
        assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        let messages = messages_for(&report, Code::SchemaDefaultNotApplied);
        assert!(
            !messages.is_empty(),
            "{entry}: `zzz` is dropped: {report:#?}"
        );
        for message in &messages {
            assert!(
                message.contains(&format!("in `{MET}`")) && !message.contains("in ``"),
                "{entry}: the `W005` names the met property, not the root: {message}"
            );
        }

        let report = run(&w006);
        assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        let ignored: Vec<_> = report
            .diagnostics()
            .iter()
            .filter(|d| d.code == Code::XmlHintIgnored)
            .collect();
        assert!(
            !ignored.is_empty(),
            "{entry}: the namespace is ignored: {report:#?}"
        );
        for diagnostic in ignored {
            assert_eq!(
                diagnostic.pointer.as_str(),
                MET,
                "{entry}: every `W006` points at the met property: {report:#?}"
            );
            let span = diagnostic.span.expect("a located `W006`");
            assert!(
                span.start.line > 2,
                "{entry}: the `W006` span is the property's, not the document's: {span:?}"
            );
        }
    }
}

/// A meet's recorded location belongs to the id the meet struct was inserted at, and lifting a
/// component root out of that id frees it for the next insert. `A`'s `$ref`-sibling meet is
/// located at `B` and lifted into `A`'s reserved id; the struct `C` then reuses the freed id, and
/// its `W006` must point at `C`, not inherit the lifted meet's `B` location.
#[test]
fn a_freed_meet_id_does_not_lend_its_location_to_the_next_type() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    B:
      type: object
    A:
      $ref: '#/components/schemas/B'
      properties:
        m: { type: integer }
    C:
      type: object
      properties:
        x:
          type: string
          xml: { namespace: "http://example.com/ns" }
"##;
    for (entry, run) in [
        ("generate", generate as fn(&str) -> Report),
        ("check", check),
    ] {
        let report = run(spec);
        assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        let ignored: Vec<_> = report
            .diagnostics()
            .iter()
            .filter(|d| d.code == Code::XmlHintIgnored)
            .collect();
        assert!(
            !ignored.is_empty(),
            "{entry}: the namespace is ignored: {report:#?}"
        );
        for diagnostic in ignored {
            assert!(
                diagnostic
                    .pointer
                    .as_str()
                    .starts_with("/components/schemas/C"),
                "{entry}: the `W006` for `C.x` points at `C`: {report:#?}"
            );
        }
    }
}

/// A property whose two sides cannot be intersected does not make the composition empty unless
/// some instance is obliged to carry it. When the property is optional on BOTH sides, `{}` and
/// `{"zz": 1}` still satisfy the whole schema — an independent Draft 2020-12 validator confirms
/// both — so rejecting the document deletes a body that has valid instances.
///
/// This is `E013`'s own published doctrine, which the array arm of `intersect_non_null` already
/// implements: "an empty array-item intersection becomes an uninhabited item type so the valid
/// empty array remains representable". `intersect_structs` propagated the failure instead. It now
/// mirrors the array arm: the field takes an uninhabited type, so the instances that remain are
/// exactly the ones that omit it.
///
/// Requiring the property on either side is the real empty composition, and both controls must
/// keep rejecting — the validator says nothing satisfies them.
#[test]
fn a_property_conflict_on_an_optional_property_does_not_empty_the_object() {
    const HEAD: &str =
        "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\nservers: [{ url: 'https://e.com' }]\n";
    const PATH: &str = r##"paths:
  /u:
    get:
      operationId: fetch
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema:
                BODY
"##;

    // Both sites that reach `intersect_structs` with a `$ref`-sibling composition: the `$ref` arm
    // of `lower_schema_inner`, and the sole-real-member union collapse.
    let inhabited: &[(&str, &str, &str)] = &[
        (
            "a `$ref` sibling conflicting on a property neither side requires",
            "$ref: '#/components/schemas/Obj'\n                type: object\n                properties: { a: { type: integer } }",
            "components:\n  schemas:\n    Obj: { type: object, properties: { a: { type: string } } }\n",
        ),
        (
            "a sole-member union conflicting on a property neither side requires",
            "type: object\n                properties: { a: { type: integer } }\n                oneOf: [{ type: object, properties: { a: { type: string } } }]",
            "components:\n  schemas:\n    Unused: { type: string }\n",
        ),
    ];
    for (what, body, components) in inhabited {
        let spec = format!("{HEAD}{}{components}", PATH.replace("BODY", body));
        for (entry, report) in [("generate", generate(&spec)), ("check", check(&spec))] {
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "`{what}` still admits `{{}}`, so {entry} must not reject it: {report:#?}"
            );
            assert!(
                !has_code(&report, Code::AllOfIrreconcilable)
                    && !has_code(&report, Code::NonDisjointUnion),
                "`{what}` reported an irreconcilable composition through {entry}: {report:#?}"
            );
        }
        // Not silently widened to something that accepts `{"a": 1}`: the field itself is
        // uninhabited, so only instances omitting the property can be built or decoded.
        let (_, code) = generate_with_code(&spec);
        assert!(
            code.contains("no JSON value can inhabit schema"),
            "`{what}` must give the conflicting property an uninhabited type: {code}"
        );
        assert_no_untyped_value(&code);
    }

    // The controls. Requiring the property on either side obliges every instance to carry a value
    // no type admits, so the composition really is empty and the rejection is right.
    let empty: &[(&str, &str, &str)] = &[
        (
            "the sibling requires the conflicting property",
            "$ref: '#/components/schemas/Obj'\n                type: object\n                required: [a]\n                properties: { a: { type: integer } }",
            "components:\n  schemas:\n    Obj: { type: object, properties: { a: { type: string } } }\n",
        ),
        (
            "the target requires the conflicting property",
            "$ref: '#/components/schemas/Obj'\n                type: object\n                properties: { a: { type: integer } }",
            "components:\n  schemas:\n    Obj: { type: object, required: [a], properties: { a: { type: string } } }\n",
        ),
    ];
    for (what, body, components) in empty {
        let spec = format!("{HEAD}{}{components}", PATH.replace("BODY", body));
        for (entry, report) in [("generate", generate(&spec)), ("check", check(&spec))] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "`{what}` admits no value at all, so {entry} must still reject it: {report:#?}"
            );
            assert!(
                has_code(&report, Code::AllOfIrreconcilable),
                "`{what}` did not report E013 through {entry}: {report:#?}"
            );
        }
    }
}

/// Four spellings of one conjunction — a `$ref` with sibling `properties`, the same pair written
/// as `allOf` members, a sole-member `oneOf` beside `properties`, and two inline `allOf` members —
/// have identical valid sets when a property conflicts and is optional on every side: `{}` and
/// `{"zz": 1}` satisfy all four and no `a`-bearing instance satisfies any. The intersection path
/// typed the property uninhabited; the `allOf` merge, which does not go through
/// `intersect_structs`, rejected the same document with `E013`. So whether a document was an error
/// depended on which equivalent spelling its author chose.
///
/// Every spelling below must now agree, and so must every control: requiring the property anywhere
/// — on the member that declares it, on a later member that only lists it in `required` (read
/// after the conflict is seen, so the decision cannot be made at the conflict), on the enclosing
/// schema, or on the `$ref` target — empties the object, and all of those reject.
#[test]
fn an_all_of_conflict_on_an_optional_property_agrees_with_every_other_spelling() {
    const HEAD: &str =
        "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\nservers: [{ url: 'https://e.com' }]\n";
    const PATH: &str = r##"paths:
  /u:
    get:
      operationId: fetch
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema:
                BODY
"##;
    const OBJ: &str =
        "components:\n  schemas:\n    Obj: { type: object, properties: { a: { type: string } } }\n";
    const OBJ_REQUIRED: &str = "components:\n  schemas:\n    Obj: { type: object, required: [a], properties: { a: { type: string } } }\n";
    const UNUSED: &str = "components:\n  schemas:\n    Unused: { type: string }\n";

    let inhabited: &[(&str, &str, &str)] = &[
        (
            "a `$ref` with a conflicting sibling property",
            "$ref: '#/components/schemas/Obj'\n                type: object\n                properties: { a: { type: integer } }",
            OBJ,
        ),
        (
            "the same pair as `allOf` members",
            "allOf:\n                  - $ref: '#/components/schemas/Obj'\n                  - { type: object, properties: { a: { type: integer } } }",
            OBJ,
        ),
        (
            "a sole-member `oneOf` beside a conflicting property",
            "type: object\n                properties: { a: { type: integer } }\n                oneOf: [{ type: object, properties: { a: { type: string } } }]",
            UNUSED,
        ),
        (
            "two inline `allOf` members",
            "allOf:\n                  - { type: object, properties: { a: { type: string } } }\n                  - { type: object, properties: { a: { type: integer } } }",
            UNUSED,
        ),
        (
            "an `allOf` member conflicting with the enclosing schema's own property",
            "type: object\n                properties: { a: { type: integer } }\n                allOf:\n                  - { type: object, properties: { a: { type: string } } }",
            UNUSED,
        ),
        (
            "a third `allOf` member meeting an already-uninhabited property",
            "allOf:\n                  - { type: object, properties: { a: { type: string } } }\n                  - { type: object, properties: { a: { type: integer } } }\n                  - { type: object, properties: { a: { type: boolean } } }",
            UNUSED,
        ),
    ];
    for (what, body, components) in inhabited {
        let spec = format!("{HEAD}{}{components}", PATH.replace("BODY", body));
        for (entry, report) in [("generate", generate(&spec)), ("check", check(&spec))] {
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "`{what}` still admits `{{}}`, so {entry} must not reject it: {report:#?}"
            );
            assert!(
                !has_code(&report, Code::AllOfIrreconcilable)
                    && !has_code(&report, Code::NonDisjointUnion),
                "`{what}` reported an irreconcilable composition through {entry}: {report:#?}"
            );
        }
        let (_, code) = generate_with_code(&spec);
        assert!(
            code.contains("no JSON value can inhabit schema"),
            "`{what}` must give the conflicting property an uninhabited type: {code}"
        );
        assert!(
            code.contains("pub a: Option<"),
            "`{what}` must keep the conflicting property optional: {code}"
        );
        assert_no_untyped_value(&code);
    }

    let empty: &[(&str, &str, &str)] = &[
        (
            "the member that declares the property requires it",
            "allOf:\n                  - { type: object, properties: { a: { type: string } } }\n                  - { type: object, required: [a], properties: { a: { type: integer } } }",
            UNUSED,
        ),
        (
            "a later member requires the property without declaring it",
            "allOf:\n                  - { type: object, properties: { a: { type: string } } }\n                  - { type: object, properties: { a: { type: integer } } }\n                  - { type: object, required: [a] }",
            UNUSED,
        ),
        (
            "the enclosing schema requires the property",
            "type: object\n                required: [a]\n                allOf:\n                  - { type: object, properties: { a: { type: string } } }\n                  - { type: object, properties: { a: { type: integer } } }",
            UNUSED,
        ),
        (
            "the `$ref` member's target requires the property",
            "allOf:\n                  - $ref: '#/components/schemas/Obj'\n                  - { type: object, properties: { a: { type: integer } } }",
            OBJ_REQUIRED,
        ),
        (
            "the `$ref` sibling spelling of the same requirement",
            "$ref: '#/components/schemas/Obj'\n                type: object\n                properties: { a: { type: integer } }",
            OBJ_REQUIRED,
        ),
    ];
    for (what, body, components) in empty {
        let spec = format!("{HEAD}{}{components}", PATH.replace("BODY", body));
        for (entry, report) in [("generate", generate(&spec)), ("check", check(&spec))] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "`{what}` admits no value at all, so {entry} must still reject it: {report:#?}"
            );
            assert!(
                has_code(&report, Code::AllOfIrreconcilable),
                "`{what}` did not report E013 through {entry}: {report:#?}"
            );
        }
    }
}

/// Two types with no typed intersection are not always an empty intersection. `uuid` and
/// `contentEncoding: base64` are both annotations on a string in 2020-12, so every string satisfies
/// both and `["x"]` is a valid instance of an array whose two item schemas are those; there is
/// simply no single Rust type for the meet. Only a *provably empty* meet — `uuid` against
/// `integer`, disjoint JSON types — may be typed uninhabited, because only then are the instances
/// the uninhabited type still admits (`[]`, an object omitting the property) exactly the valid ones.
///
/// Every site that used to read "no typed intersection" as "empty" is driven here: the array item
/// (under `allOf` and under a `$ref` with siblings), an optional property (under a `$ref` with
/// siblings and under `allOf`), the null-only collapse of two nullable sides, a union branch the
/// enclosing schema's siblings meet, a branch of a referenced union, and a tuple position under
/// array items. Each typed the inhabited
/// case as uninhabited, as `()`, or dropped the branch, and generated. Each now rejects with
/// `E013`, and the provably empty control beside it keeps generating as before.
#[test]
fn an_inhabited_intersection_with_no_rust_type_is_rejected_not_typed_uninhabited() {
    const HEAD: &str =
        "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\nservers: [{ url: 'https://e.com' }]\n";
    const PATH: &str = r##"paths:
  /u:
    get:
      operationId: fetch
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema:
                BODY
"##;
    // `OTHER` is the second side's schema: the unrepresentable `base64` string, or the disjoint
    // `integer` control.
    let cases: &[(&str, &str, &str)] = &[
        (
            "array items under `allOf`",
            "allOf:\n                  - { type: object, properties: { ids: { type: array, items: { $ref: '#/components/schemas/Id' } } } }\n                  - { type: object, properties: { ids: { type: array, items: OTHER } } }",
            "components:\n  schemas:\n    Id: { type: string, format: uuid }\n",
        ),
        (
            "array items under a `$ref` with siblings",
            "{ $ref: '#/components/schemas/Ids', type: array, items: OTHER }",
            "components:\n  schemas:\n    Ids: { type: array, items: { type: string, format: uuid } }\n",
        ),
        (
            "an optional property under a `$ref` with siblings",
            "{ $ref: '#/components/schemas/Obj', type: object, properties: { a: OTHER } }",
            "components:\n  schemas:\n    Obj: { type: object, properties: { a: { type: string, format: uuid } } }\n",
        ),
        (
            "an optional property under `allOf`",
            "allOf:\n                  - { type: object, properties: { a: { type: string, format: uuid } } }\n                  - { type: object, properties: { a: OTHER } }",
            "components:\n  schemas:\n    Unused: { type: string }\n",
        ),
    ];
    for (what, body, components) in cases {
        for (other, inhabited) in [
            ("{ type: string, contentEncoding: base64 }", true),
            ("{ type: integer }", false),
        ] {
            let spec = format!(
                "{HEAD}{}{components}",
                PATH.replace("BODY", &body.replace("OTHER", other))
            );
            for (entry, report) in [("generate", generate(&spec)), ("check", check(&spec))] {
                if inhabited {
                    assert_eq!(
                        report.outcome(),
                        Outcome::Rejected,
                        "{what}: a uuid/base64 meet is inhabited but has no Rust type, so {entry} \
                         must reject it rather than type it uninhabited: {report:#?}"
                    );
                    assert!(
                        has_code(&report, Code::AllOfIrreconcilable),
                        "{what}: {entry} must report E013: {report:#?}"
                    );
                } else {
                    assert_ne!(
                        report.outcome(),
                        Outcome::Rejected,
                        "{what}: a uuid/integer meet is provably empty, so {entry} must keep \
                         generating: {report:#?}"
                    );
                }
            }
            if !inhabited {
                let (_, code) = generate_with_code(&spec);
                assert!(
                    code.contains("no JSON value can inhabit schema"),
                    "{what}: the empty meet must still be typed uninhabited: {code}"
                );
            }
        }
    }

    // The sites that do not fall back to `Never`, but read the same answer in other ways: the
    // null-only collapse of two nullable sides typed the position `()`, and a union branch the
    // siblings could not be typed against was dropped (with a `W011` claiming it cannot satisfy
    // them, or with nothing at all under a `$ref`), leaving a lone `uuid` for a union that also
    // admits every other string. And a tuple position that cannot meet is not an empty tuple:
    // `prefixItems` does not require the array to reach it, so `[]` satisfies both `[string]` and
    // `[integer]` tuples, and an outer array of them is not only `[]`.
    let components = "components:\n  schemas:\n    N: { type: [string, 'null'], format: uuid }\n    \
                      U: { anyOf: [{ type: string, contentEncoding: base64 }, { type: string }] }\n    \
                      Pairs: { type: array, items: { type: array, prefixItems: [{ type: string }], items: false } }\n";
    for (what, body) in [
        (
            "array items that are tuples whose positions do not meet",
            "{ $ref: '#/components/schemas/Pairs', type: array, items: { type: array, prefixItems: [{ type: integer }], items: false } }",
        ),
        (
            "two nullable sides whose non-null shapes have no Rust type",
            "{ $ref: '#/components/schemas/N', type: [string, 'null'], contentEncoding: base64 }",
        ),
        (
            "a union branch under the enclosing schema's siblings",
            "{ type: string, format: uuid, anyOf: [{ type: string, contentEncoding: base64 }, { type: string }] }",
        ),
        (
            "a branch of a referenced union under a `$ref`'s siblings",
            "{ $ref: '#/components/schemas/U', type: string, format: uuid }",
        ),
    ] {
        let spec = format!("{HEAD}{}{components}", PATH.replace("BODY", body));
        for (entry, report) in [("generate", generate(&spec)), ("check", check(&spec))] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "{what}: {entry} must reject rather than narrow: {report:#?}"
            );
            assert!(
                has_code(&report, Code::AllOfIrreconcilable),
                "{what}: {entry} must report E013: {report:#?}"
            );
            assert!(
                !has_code(&report, Code::DeclarationHasNoEffect),
                "{what}: {entry} must not claim a branch cannot satisfy the siblings: {report:#?}"
            );
        }
    }

    // An unrepresentable property does not settle a struct meet on its own: `intersect_structs`
    // defers it, because a LATER property that some side requires and whose types share no value
    // proves every object empty, and then the array of them is exactly `[]` — `Vec<Never>`, not a
    // rejection. The uuid/base64 property `a` comes first so the deferral is what is exercised;
    // with the required property's types compatible instead, nothing empties the object and the
    // deferred unrepresentable answer stands.
    let components = "components:\n  schemas:\n    Objs: { type: array, items: { type: object, \
                      properties: { a: { type: string, format: uuid }, b: { type: string } } } }\n";
    for (required_type, empty) in [("integer", true), ("string", false)] {
        let body = format!(
            "{{ $ref: '#/components/schemas/Objs', type: array, items: {{ type: object, \
             properties: {{ a: {{ type: string, contentEncoding: base64 }}, \
             b: {{ type: {required_type} }} }}, required: [b] }} }}"
        );
        let spec = format!("{HEAD}{}{components}", PATH.replace("BODY", &body));
        for (entry, report) in [("generate", generate(&spec)), ("check", check(&spec))] {
            if empty {
                assert_ne!(
                    report.outcome(),
                    Outcome::Rejected,
                    "a required string/integer property empties every item, so {entry} must keep \
                     generating the empty array despite the earlier uuid/base64 one: {report:#?}"
                );
            } else {
                assert_eq!(
                    report.outcome(),
                    Outcome::Rejected,
                    "with no empty property the uuid/base64 one is unrepresentable, so {entry} \
                     must reject: {report:#?}"
                );
                assert!(
                    has_code(&report, Code::AllOfIrreconcilable),
                    "{entry} must report E013: {report:#?}"
                );
            }
        }
        if empty {
            let (_, code) = generate_with_code(&spec);
            assert!(
                code.contains("no JSON value can inhabit schema"),
                "the emptied item must be typed uninhabited: {code}"
            );
            let never = code
                .lines()
                .find_map(|line| {
                    line.trim()
                        .strip_prefix("pub enum ")
                        .and_then(|rest| rest.strip_suffix(" {}"))
                })
                .unwrap_or_else(|| panic!("no uninhabited enum was emitted: {code}"));
            assert!(
                code.contains(&format!("Vec<{never}>")),
                "the body must be an array of the uninhabited item `{never}`: {code}"
            );
        }
    }
}

/// An irreconcilable meet of nullable objects leaves exactly `null`: no object satisfies both
/// members, whose required property types share no value, and both admit `null`. Every spelling
/// of that conjunction lowers it to the null type `()`, as the `$ref`-sibling spelling already
/// did; the `allOf` spellings, and a `$ref` beside untyped object keywords, rejected it with
/// `E013` (#542). A member that denies `null` leaves nothing at all, which stays `E013` in every
/// spelling.
#[test]
fn an_irreconcilable_meet_of_nullable_objects_is_the_null_type() {
    let document = |second_type: &str, p: &str| {
        format!(
            "openapi: 3.1.0\ninfo: {{ title: T, version: 1.0.0 }}\npaths: {{}}\ncomponents:\n  \
             schemas:\n    M0:\n      type: [object, 'null']\n      required: [a]\n      \
             properties: {{ a: {{ type: string }}, b: {{ type: string }} }}\n    M1:\n      \
             type: {second_type}\n      required: [b]\n      \
             properties: {{ b: {{ type: integer }} }}\n    Holder:\n      type: object\n      \
             required: [p]\n      properties:\n        p: {p}\n"
        )
    };
    let spellings = |second_type: &str| {
        [
            (
                "an `allOf` of `$ref`s",
                "{ allOf: [{ $ref: '#/components/schemas/M0' }, \
                 { $ref: '#/components/schemas/M1' }] }"
                    .to_owned(),
            ),
            (
                "an inline `allOf`",
                format!(
                    "{{ allOf: [{{ type: [object, 'null'], properties: {{ b: {{ type: string }} }} }}, \
                     {{ type: {second_type}, required: [b], \
                     properties: {{ b: {{ type: integer }} }} }}] }}"
                ),
            ),
            (
                "keywords beside an `allOf` member",
                format!(
                    "{{ type: {second_type}, required: [b], properties: {{ b: {{ type: integer }} }}, \
                     allOf: [{{ type: [object, 'null'], properties: {{ b: {{ type: string }} }} }}] }}"
                ),
            ),
            (
                "a `$ref` beside typed keywords",
                format!(
                    "{{ $ref: '#/components/schemas/M0', type: {second_type}, required: [b], \
                     properties: {{ b: {{ type: integer }} }} }}"
                ),
            ),
        ]
    };

    for (second_type, admits_null) in [("[object, 'null']", true), ("object", false)] {
        for (what, p) in spellings(second_type) {
            let spec = document(second_type, &p);
            let (report, code) = generate_with_code(&spec);
            if !admits_null {
                assert_eq!(report.outcome(), Outcome::Rejected, "{what}: {report:#?}");
                assert!(
                    has_code(&report, Code::AllOfIrreconcilable),
                    "{what}: {report:#?}"
                );
                continue;
            }
            assert_ne!(report.outcome(), Outcome::Rejected, "{what}: {report:#?}");
            assert!(
                !has_code(&report, Code::AllOfIrreconcilable),
                "{what}: {report:#?}"
            );
            let ty = field_type(&types_module(&code), "pub p:")
                .unwrap_or_else(|| panic!("{what}: no `Holder.p`: {code}"));
            assert!(
                code.contains(&format!("pub type {ty} = ();")),
                "{what}: only `null` satisfies both members, so `Holder.p` is the null type: {code}"
            );
        }
    }

    // A `$ref` to a nullable object beside untyped object keywords: the keywords admit `null`
    // without deciding it (#425), so this meet is null-only too, not a category contradiction.
    let untyped = document(
        "[object, 'null']",
        "{ $ref: '#/components/schemas/M0', properties: { a: { type: integer } } }",
    );
    let (report, code) = generate_with_code(&untyped);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::AllOfIrreconcilable), "{report:#?}");
    let ty = field_type(&types_module(&code), "pub p:")
        .unwrap_or_else(|| panic!("no `Holder.p`: {code}"));
    assert!(code.contains(&format!("pub type {ty} = ();")), "{code}");

    // The schema's own `type` listing `null` does not make `null` valid where its `allOf` members
    // deny it: no value satisfies the schema, so that stays `E013`, never the null type.
    let denied = document(
        "object",
        "{ type: [object, 'null'], allOf: [{ type: object, properties: { b: { type: string } } }, \
         { $ref: '#/components/schemas/M1' }] }",
    );
    let (report, _) = generate_with_code(&denied);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::AllOfIrreconcilable), "{report:#?}");

    // The schema's own `type`, `enum` or `const` beside the `allOf` of the two nullable members
    // is no object keyword, so it contributes no member, yet it constrains every value: one that
    // excludes `null` leaves nothing, which stays `E013`; one that admits it leaves `null`. So
    // does a `oneOf` member beside them, met with the null type the object members leave.
    let members = "{ $ref: '#/components/schemas/M0' }, { $ref: '#/components/schemas/M1' }";
    for (what, p, null_only) in [
        (
            "own `type: string`",
            format!("{{ type: string, allOf: [{members}] }}"),
            false,
        ),
        (
            "own `enum: [1]`",
            format!("{{ enum: [1], allOf: [{members}] }}"),
            false,
        ),
        (
            "own `const: x`",
            format!("{{ const: x, allOf: [{members}] }}"),
            false,
        ),
        (
            "own `type: [string, 'null']`",
            format!("{{ type: [string, 'null'], allOf: [{members}] }}"),
            true,
        ),
        (
            "own `enum: [null]`",
            format!("{{ enum: [null], allOf: [{members}] }}"),
            true,
        ),
        // A nested `allOf` member is flattened into the outer meet, and its own `type`, `enum` or
        // `const` constrains every value just as the outer schema's does (#569).
        (
            "a nested member's own `type: string`",
            format!("{{ allOf: [{{ type: string, allOf: [{members}] }}] }}"),
            false,
        ),
        (
            "a nested member's own `enum: [1]`",
            format!("{{ allOf: [{{ enum: [1], allOf: [{members}] }}] }}"),
            false,
        ),
        (
            "a nested member's own `const: x`",
            format!("{{ allOf: [{{ const: x, allOf: [{members}] }}] }}"),
            false,
        ),
        (
            "a twice-nested member's own `type: string`",
            format!("{{ allOf: [{{ allOf: [{{ type: string, allOf: [{members}] }}] }}] }}"),
            false,
        ),
        (
            "a `$ref` member's own `type: string` beside a nested `allOf`",
            "{ allOf: [{ $ref: '#/components/schemas/M0', type: string, \
             allOf: [{ $ref: '#/components/schemas/M1' }] }] }"
                .to_owned(),
            false,
        ),
        (
            "a nested member's own `type: [string, 'null']`",
            format!("{{ allOf: [{{ type: [string, 'null'], allOf: [{members}] }}] }}"),
            true,
        ),
        (
            "a nested member's own `enum: [null]`",
            format!("{{ allOf: [{{ enum: [null], allOf: [{members}] }}] }}"),
            true,
        ),
        (
            "a `oneOf` member admitting `null`",
            format!(
                "{{ allOf: [{members}, {{ oneOf: [{{ type: string }}, {{ type: 'null' }}] }}] }}"
            ),
            true,
        ),
        (
            "a `oneOf` member denying `null`",
            format!(
                "{{ allOf: [{members}, {{ oneOf: [{{ type: string }}, {{ type: integer }}] }}] }}"
            ),
            false,
        ),
    ] {
        let spec = document("[object, 'null']", &p);
        let (report, code) = generate_with_code(&spec);
        if !null_only {
            assert_eq!(report.outcome(), Outcome::Rejected, "{what}: {report:#?}");
            assert!(
                has_code(&report, Code::AllOfIrreconcilable),
                "{what}: {report:#?}"
            );
            continue;
        }
        assert_ne!(report.outcome(), Outcome::Rejected, "{what}: {report:#?}");
        assert!(
            !has_code(&report, Code::AllOfIrreconcilable),
            "{what}: {report:#?}"
        );
        let ty = field_type(&types_module(&code), "pub p:")
            .unwrap_or_else(|| panic!("{what}: no `Holder.p`: {code}"));
        assert!(
            code.contains(&format!("pub type {ty} = ();")),
            "{what}: only `null` is left, so `Holder.p` is the null type: {code}"
        );
    }

    // A non-component `$ref` member is expanded in place, so a target that is itself a nested
    // `allOf` is flattened into the outer meet, and its own `type` constrains every value just as
    // an inline member's does. The bare `$ref` and the `allOf` holding it are one conjunction, so
    // both stay `E013`, in `generate` and `check` alike; a target whose `type` admits `null`
    // leaves the null type in both spellings.
    for (target_type, null_only) in [("string", false), ("[string, 'null']", true)] {
        for (what, p) in [
            ("a bare `$ref`", "{ $ref: '#/x-defs/Q' }"),
            (
                "an `allOf` of the `$ref`",
                "{ allOf: [{ $ref: '#/x-defs/Q' }] }",
            ),
            (
                "an `allOf` of an alias of the `$ref`",
                "{ allOf: [{ $ref: '#/x-defs/R' }] }",
            ),
        ] {
            let what = format!("{what} to a nested `allOf` of `type: {target_type}`");
            let spec = format!(
                "{}x-defs:\n  Q:\n    type: {target_type}\n    allOf: [{members}]\n  R:\n    \
                 $ref: '#/x-defs/Q'\n",
                document("[object, 'null']", p)
            );
            let (report, code) = generate_with_code(&spec);
            let checked = check(&spec);
            if !null_only {
                for (entry, report) in [("generate", &report), ("check", &checked)] {
                    assert_eq!(
                        report.outcome(),
                        Outcome::Rejected,
                        "{what} ({entry}): {report:#?}"
                    );
                    assert!(
                        has_code(report, Code::AllOfIrreconcilable),
                        "{what} ({entry}): {report:#?}"
                    );
                }
                continue;
            }
            for (entry, report) in [("generate", &report), ("check", &checked)] {
                assert_ne!(
                    report.outcome(),
                    Outcome::Rejected,
                    "{what} ({entry}): {report:#?}"
                );
                assert!(
                    !has_code(report, Code::AllOfIrreconcilable),
                    "{what} ({entry}): {report:#?}"
                );
            }
            let ty = field_type(&types_module(&code), "pub p:")
                .unwrap_or_else(|| panic!("{what}: no `Holder.p`: {code}"));
            assert!(
                code.contains(&format!("pub type {ty} = ();")),
                "{what}: only `null` is left, so `Holder.p` is the null type: {code}"
            );
        }
    }
}

/// An `allOf` member of untyped array applicators alone (`items` or `prefixItems`, no `type`) was
/// read as neither an object nor a scalar, so it was dropped as an annotation with no diagnostic:
/// `allOf: [{type: array}, {items: {type: string}}]` generated `Vec<serde_json::Value>` (#607).
/// Its applicators constrain only arrays, so they now refine the merge's arrays, in either member
/// order, through a `$ref` member's array target, from a nested `allOf` and from a sub-file
/// target, and the array branch of a multi-type member; they establish the array category where
/// no other member constrains; and they leave every other category as it is, so beside a string
/// or an object member they change nothing. Beside a union, a nested member refines the union's
/// array branch as the same member written directly does, rather than dropping its string branch.
#[test]
fn an_all_of_member_of_untyped_array_applicators_refines_the_merge_s_arrays() {
    /// The right-hand side of `pub type ty = …;`, whitespace removed, which the formatter may have
    /// broken across lines (a long element name does).
    fn alias(code: &str, ty: &str) -> Option<String> {
        let head = format!("pub type {ty} = ");
        let start = code.find(&head)? + head.len();
        let end = start + code[start..].find(';')?;
        Some(code[start..end].split_whitespace().collect::<String>())
    }
    /// The type `ty` names once every bare alias on the way is followed.
    fn resolve(code: &str, ty: &str) -> String {
        let mut ty = ty.trim().to_owned();
        while let Some(target) = alias(code, &ty) {
            if target.contains(['<', '(']) {
                return target;
            }
            ty = target;
        }
        ty
    }
    /// The element type of the `Vec` that `ty` resolves to, resolved in turn.
    fn element(code: &str, ty: &str, what: &str) -> String {
        let vec = resolve(code, ty);
        let inner = vec
            .strip_prefix("Vec<")
            .and_then(|rest| rest.strip_suffix('>'))
            .map(|inner| inner.trim_end_matches(','))
            .unwrap_or_else(|| panic!("{what}: `{ty}` is `{vec}`, not a `Vec`: {code}"));
        resolve(code, inner)
    }
    let narrowing = [
        (
            "the array member first",
            "    Probe:\n      allOf: [ { type: array }, { items: { type: string } } ]\n",
        ),
        (
            "the items member first",
            "    Probe:\n      allOf: [ { items: { type: string } }, { type: array } ]\n",
        ),
        (
            "a `$ref` member to an array component",
            "    Probe:\n      allOf: [ { $ref: '#/components/schemas/Arr' }, \
             { items: { type: string } } ]\n    Arr: { type: array }\n",
        ),
        (
            "a nested `allOf`",
            "    Probe:\n      allOf: [ { type: array }, { allOf: [ { items: { type: string } } \
             ] } ]\n",
        ),
        (
            "no other constraining member",
            "    Probe:\n      allOf: [ { items: { type: string } }, { description: d } ]\n",
        ),
    ];
    for (what, schemas) in narrowing {
        let (report, code) = generate_with_code(&with_schemas("3.1.0", schemas));
        assert_eq!(report.outcome(), Outcome::Generated, "{what}: {report:#?}");
        assert!(codes(&report).is_empty(), "{what}: {report:#?}");
        assert_eq!(element(&code, "Probe", what), "String", "{what}");
    }

    // A sub-file target is gathered as an inline member, so its applicators refine as one does.
    let root = "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\npaths: {}\ncomponents:\n  \
                schemas:\n    Probe:\n      allOf: [ { type: array }, { $ref: './lib.yaml#/It' } \
                ]\n";
    let lib = "It: { items: { type: string } }\n";
    let (generated, checked, code) =
        generate_and_check_files(&[("openapi.yaml", root), ("lib.yaml", lib)]);
    assert_check_agrees(&generated, &checked);
    assert_eq!(generated.outcome(), Outcome::Generated, "{generated:#?}");
    assert_eq!(element(&code, "Probe", "a sub-file target"), "String");

    // `prefixItems` narrows the array to the tuple it describes.
    let (report, code) = generate_with_code(&with_schemas(
        "3.1.0",
        "    Probe:\n      allOf: [ { type: array }, { prefixItems: [ { type: string } ], \
         items: false } ]\n",
    ));
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    let tuple = resolve(&code, "Probe");
    let position = tuple
        .strip_prefix('(')
        .and_then(|rest| rest.strip_suffix(",)"))
        .unwrap_or_else(|| panic!("`Probe` is `{tuple}`, not a one-tuple: {code}"));
    assert_eq!(resolve(&code, position), "String");

    // The array branch of a multi-type member is refined; its string branch is kept.
    let (report, code) = generate_with_code(&with_schemas(
        "3.1.0",
        "    Probe:\n      allOf: [ { type: [array, string] }, { items: { type: string } } ]\n",
    ));
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    let mut branches: Vec<String> = enum_variants(&code, "Probe")
        .iter()
        .map(|variant| {
            let inner = variant
                .split_once("(Box<")
                .and_then(|(_, rest)| rest.strip_suffix(">)"))
                .unwrap_or_else(|| panic!("variant `{variant}`: {code}"));
            match resolve(&code, inner).as_str() {
                "String" => "string".to_owned(),
                _ => format!("array of {}", element(&code, inner, "the array branch")),
            }
        })
        .collect();
    branches.sort();
    assert_eq!(branches, ["array of String", "string"], "{code}");

    // `null` stays where the array member admits it.
    let (report, code) = generate_with_code(&with_schemas(
        "3.1.0",
        "    Probe:\n      allOf: [ { type: [array, 'null'] }, { items: { type: string } } ]\n    \
         Holder: { type: object, required: [p], properties: { p: { $ref: \
         '#/components/schemas/Probe' } } }\n",
    ));
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert_eq!(element(&code, "Probe", "a nullable array"), "String");
    assert_eq!(
        field_type(&types_module(&code), "pub p:").as_deref(),
        Some("Option<Probe>"),
        "{code}"
    );

    // Beside members of another category the applicators constrain nothing.
    let vacuous = [
        (
            "a string member",
            "    Probe:\n      allOf: [ { type: string }, { items: { type: string } } ]\n",
            "String",
        ),
        (
            "an object member",
            "    Probe:\n      allOf: [ { type: object, properties: { a: { type: string } } }, \
             { items: { type: string } } ]\n",
            "struct",
        ),
    ];
    for (what, schemas, expected) in vacuous {
        let (report, code) = generate_with_code(&with_schemas("3.1.0", schemas));
        assert_eq!(report.outcome(), Outcome::Generated, "{what}: {report:#?}");
        assert!(codes(&report).is_empty(), "{what}: {report:#?}");
        if expected == "struct" {
            assert_eq!(declared_fields(&code, "Probe"), ["a"], "{what}: {code}");
        } else {
            assert_eq!(resolve(&code, "Probe"), expected, "{what}: {code}");
        }
    }

    // Beside a union, nested or not, the member refines the union's array branch alone.
    for (what, all_of) in [
        ("written directly", "[ { items: { type: string } } ]"),
        ("nested", "[ { allOf: [ { items: { type: string } } ] } ]"),
    ] {
        let (report, code) = generate_with_code(&with_schemas(
            "3.1.0",
            &format!(
                "    Probe:\n      allOf: {all_of}\n      oneOf: [ {{ type: string }}, {{ type: \
                 array }} ]\n"
            ),
        ));
        assert_eq!(report.outcome(), Outcome::Generated, "{what}: {report:#?}");
        let variants = enum_variants(&code, "Probe");
        assert_eq!(
            variants.len(),
            2,
            "{what}: the string branch must stay: {code}"
        );
        let array = variants
            .iter()
            .filter_map(|variant| {
                variant
                    .split_once("(Box<")
                    .and_then(|(_, rest)| rest.strip_suffix(">)"))
            })
            .find(|inner| resolve(&code, inner) != "String")
            .unwrap_or_else(|| panic!("{what}: no array branch: {code}"));
        assert_eq!(element(&code, array, what), "String", "{what}");
    }

    // Beside a union member with no array branch, the member reaches nothing the union accepts:
    // `W011` at the member, written directly or nested, and the union generates as it is. The
    // nested spelling took the scalar shortcut and was met silently.
    for (what, refiner, pointer) in [
        (
            "written directly",
            "{ items: { type: string } }",
            "/components/schemas/Probe/allOf/1",
        ),
        (
            "nested",
            "{ allOf: [ { items: { type: string } } ] }",
            "/components/schemas/Probe/allOf/1/allOf/0",
        ),
    ] {
        let spec = with_schemas(
            "3.1.0",
            &format!(
                "    Probe:\n      allOf: [ {{ oneOf: [ {{ type: string }}, {{ type: integer }} \
                 ] }}, {refiner} ]\n"
            ),
        );
        let (report, code) = generate_with_code(&spec);
        assert_eq!(report.outcome(), Outcome::Generated, "{what}: {report:#?}");
        assert!(
            report
                .diagnostics()
                .iter()
                .any(|d| d.code == Code::DeclarationHasNoEffect && d.pointer.as_str() == pointer),
            "{what}: W011 must sit at `{pointer}`: {report:#?}"
        );
        assert_eq!(enum_variants(&code, "Probe").len(), 2, "{what}: {code}");
    }
}
