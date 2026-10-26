//! Open narrowing and `allOf` meets, as the surface sees the types they generate.

use super::*;

/// `get /pets` whose documented `404` narrows the `Problem` component's `type` to `values`.
fn narrowed_problem(values: &str) -> String {
    full(
        &format!(
            "  /pets:
    get:
      operationId: listPets
      responses:
        '200':
          description: ok
        '404':
          description: missing
          content:
            application/problem+json:
              schema:
                allOf:
                  - $ref: '#/components/schemas/Problem'
                  - properties: {{ type: {{ enum: [{values}] }} }}
"
        ),
        "    Problem:
      type: object
      required: [type]
      properties:
        type: { type: string }
",
    )
}

/// The open set's catch-all is surface: an open enum carries it as a variant, and a listed value
/// that takes its name moves it, which is a rename a consumer's `match` sees.
#[test]
fn an_open_enums_catch_all_is_a_variant_of_its_surface() {
    let open = |spec: Spec| spec.open_narrowing(true);

    // A new listed value is the additive rule's variant, as on a closed enum.
    let added = diff_configured(
        &narrowed_problem("a, b"),
        &narrowed_problem("a, b, c"),
        open,
        open,
    );
    assert_eq!(kinds(&added), vec![ChangeKind::VariantAdded], "{added:?}");
    assert_eq!(added.bump, Impact::Minor);

    // A new listed value spelled `other` takes the catch-all's name, so the catch-all is renamed.
    let renamed = diff_configured(
        &narrowed_problem("a, b"),
        &narrowed_problem("a, b, other"),
        open,
        open,
    );
    let details: Vec<&str> = renamed
        .changes
        .iter()
        .map(|change| change.detail.as_str())
        .collect();
    assert!(
        details.contains(&"variant removed: `Other(String)`, which held any unlisted value"),
        "{details:?}"
    );
    assert!(details.contains(&"variant added: `other`"), "{details:?}");
    assert_eq!(renamed.bump, Impact::Major);

    // Turning the option on opens the same type in place: it gains the catch-all, which is the
    // additive rule again, and turning it off removes it.
    let opened = diff_configured(
        &narrowed_problem("a, b"),
        &narrowed_problem("a, b"),
        |spec| spec,
        open,
    );
    assert_eq!(kinds(&opened), vec![ChangeKind::VariantAdded], "{opened:?}");
    assert_eq!(
        opened.changes[0].detail,
        "variant added: `Other(String)`, holding any unlisted value"
    );
    assert_eq!(opened.bump, Impact::Minor);
    let closed = diff_configured(
        &narrowed_problem("a, b"),
        &narrowed_problem("a, b"),
        open,
        |spec| spec,
    );
    assert_eq!(
        kinds(&closed),
        vec![ChangeKind::VariantRemoved],
        "{closed:?}"
    );
    assert_eq!(closed.bump, Impact::Major);
}

/// A `404` body whose required `kind` property is the `allOf` of `members`, one member per entry,
/// beside a `Listed` component that is the closed set `[x, listed-only]`, and three components that
/// are that set narrowed against a `uuid` string outside any response body: `UuidListed` declares
/// it as a `uuid` string, `UntypedUuidListed` names the format without `type: string`, and
/// `UuidNarrowed` is the `allOf` of `Listed` and a `uuid` string.
fn kind_meeting(members: &[&str]) -> String {
    let members: String = members
        .iter()
        .map(|member| format!("                      - {member}\n"))
        .collect();
    full(
        &format!(
            "  /pets:
    get:
      operationId: listPets
      responses:
        '200':
          description: ok
        '404':
          description: missing
          content:
            application/json:
              schema:
                type: object
                required: [kind]
                properties:
                  kind:
                    allOf:
{members}"
        ),
        "    Listed:
      type: string
      enum: [x, listed-only]
    UuidListed:
      type: string
      format: uuid
      enum: [x, listed-only]
    UntypedUuidListed:
      format: uuid
      enum: [x, listed-only]
    UuidNarrowed:
      allOf:
        - $ref: '#/components/schemas/Listed'
        - type: string
          format: uuid
",
    )
}

/// The enum `kind` lowers to under `open_narrowing`: its variant lines, in order.
fn open_kind_variants(spec_text: &str) -> Vec<String> {
    let source = generated_source(spec_text, true);
    // The embedded runtime's `AuthScheme` has a `kind` field of its own.
    let ty = source
        .split("pub kind: ")
        .skip(1)
        .map(|field| field[..field.find(',').unwrap()].trim())
        .find(|ty| *ty != "AuthKind")
        .unwrap_or_else(|| panic!("no `kind` field:\n{source}"));
    let body = source
        .split(&format!("pub enum {ty} {{"))
        .nth(1)
        .unwrap_or_else(|| panic!("`kind` is `{ty}`, which is not an enum:\n{source}"));
    // No variant carries braces, so the first one closes the enum.
    body[..body.find('}').unwrap()]
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#') && !line.starts_with("//"))
        .map(str::to_owned)
        .collect()
}

/// The `(kind, location)` of every major change in a report.
fn major_changes(report: &DiffReport) -> Vec<(ChangeKind, String)> {
    report
        .changes
        .iter()
        .filter(|change| change.impact == Impact::Major)
        .map(|change| (change.kind, change.location.clone()))
        .collect()
}

/// Under `open_narrowing`, an `allOf`'s members meet in the same set whatever order they are
/// written in: the values every member lists, open when a member meeting a plain `string` opened
/// it. An open set meeting a closed one keeps only their shared values (the description forbids
/// the rest), and two open sets meet in the values both list. Reordering the members is then no
/// more breaking with the option on than with it off, and a patch either way: an inline member's
/// type is named for its position, but the meet consumes it and the output carries none of the
/// member types (#924), so moving a member renames no type of the output.
#[test]
fn open_narrowing_meets_the_same_set_in_every_member_order() {
    let open = |spec: Spec| spec.open_narrowing(true);
    let string = "{ type: string }";
    let x = "{ const: x }";
    let listed = "{ $ref: '#/components/schemas/Listed' }";
    let expected = ["X,", "Other(String),"];

    // An open set meeting a closed one, in all six orders: `string ∩ {x}` opens `{x}` before it
    // meets the closed `Listed`, `string ∩ Listed` opens a copy of `Listed` before it meets the
    // closed `{x}`, or the two closed sets meet before either meets `string`.
    let orders = [
        [string, x, listed],
        [string, listed, x],
        [x, string, listed],
        [x, listed, string],
        [listed, string, x],
        [listed, x, string],
    ];
    let first = kind_meeting(&orders[0]);
    for order in &orders {
        let spec = kind_meeting(order);
        assert_eq!(open_kind_variants(&spec), expected, "order {order:?}");
        let off = diff_configured(&first, &spec, |spec| spec, |spec| spec);
        let on = diff_configured(&first, &spec, open, open);
        assert_eq!(
            major_changes(&on),
            major_changes(&off),
            "reordering to {order:?} breaks more with the option on: {:?}",
            on.changes
        );
    }

    // Two open sets meeting, in both orders: each nested `allOf` narrows a plain `string`.
    let open_x = "{ allOf: [{ type: string }, { const: x }] }";
    let open_xy = "{ allOf: [{ type: string }, { enum: [x, y] }] }";
    for order in [[open_x, open_xy], [open_xy, open_x]] {
        assert_eq!(
            open_kind_variants(&kind_meeting(&order)),
            expected,
            "order {order:?}"
        );
    }
    let (forward, backward) = (
        kind_meeting(&[open_x, open_xy]),
        kind_meeting(&[open_xy, open_x]),
    );
    assert_eq!(
        major_changes(&diff_configured(&forward, &backward, open, open)),
        major_changes(&diff_configured(
            &forward,
            &backward,
            |spec| spec,
            |spec| spec
        )),
    );
}

/// Under `open_narrowing`, a set narrowed against a `uuid` or date string stays closed whatever
/// order the members are written in: only a plain `string` admits the unlisted string an open set
/// holds, and an `allOf` with a formatted member admits only what that format does. Each meet
/// keeps its operands' order-independence (#400): the format is remembered by the set it narrowed,
/// so a later or earlier plain `string` cannot open it, and an already-open set it meets closes.
#[test]
fn open_narrowing_never_opens_a_set_narrowed_against_a_formatted_string() {
    let string = "{ type: string }";
    let x = "{ const: x }";
    let open_x = "{ allOf: [{ type: string }, { const: x }] }";
    let mut opened = Vec::new();
    for format in ["uuid", "date", "date-time"] {
        let formatted = format!("{{ type: string, format: {format} }}");
        let formatted = formatted.as_str();
        let formatted_x = format!("{{ type: string, format: {format}, const: x }}");
        let formatted_x = formatted_x.as_str();
        // The same own-schema set without `type: string`: `format` alone names the format.
        let untyped_x = format!("{{ format: {format}, const: x }}");
        let untyped_x = untyped_x.as_str();
        let mut orders: Vec<Vec<&str>> = vec![
            vec![string, x, formatted],
            vec![string, formatted, x],
            vec![x, string, formatted],
            vec![x, formatted, string],
            vec![formatted, string, x],
            vec![formatted, x, string],
        ];
        // An open set (a nested narrowing of a plain `string`) meeting the formatted string, and a
        // set narrowed against the format in one schema (with or without `type: string`) meeting a
        // plain `string` or an open set, in both orders.
        for [left, right] in [
            [open_x, formatted],
            [formatted_x, string],
            [formatted_x, open_x],
            [untyped_x, string],
            [untyped_x, open_x],
        ] {
            orders.push(vec![left, right]);
            orders.push(vec![right, left]);
        }
        let mut cases: Vec<(Vec<&str>, &[&str])> = orders
            .into_iter()
            .map(|order| (order, &["X,"][..]))
            .collect();
        // A `$ref` target, which is lowered closed and then meets the format and a plain `string`
        // in all six orders (a locked copy of the target where the format meets it first), and a
        // component locked outside the response body meeting a plain `string`, in both orders:
        // one locked by its own schema's format (spelled with and without `type: string`), and
        // one whose `allOf` narrows `Listed` against a `uuid` string, which is lowered where
        // narrowing does not open a set.
        let listed_values: &[&str] = &["X,", "ListedOnly,"];
        let listed = "{ $ref: '#/components/schemas/Listed' }";
        for order in [
            [string, listed, formatted],
            [string, formatted, listed],
            [listed, string, formatted],
            [listed, formatted, string],
            [formatted, string, listed],
            [formatted, listed, string],
        ] {
            cases.push((order.to_vec(), listed_values));
        }
        if format == "uuid" {
            for target in [
                "{ $ref: '#/components/schemas/UuidListed' }",
                "{ $ref: '#/components/schemas/UntypedUuidListed' }",
                "{ $ref: '#/components/schemas/UuidNarrowed' }",
            ] {
                cases.push((vec![target, string], listed_values));
                cases.push((vec![string, target], listed_values));
            }
        }
        for (order, expected) in cases {
            let variants = open_kind_variants(&kind_meeting(&order));
            if variants != expected {
                opened.push((order.join(" & "), variants));
            }
        }
    }
    assert!(opened.is_empty(), "opened against a format: {opened:#?}");
}

/// Under `open_narrowing`, the copy an intersection makes of a `$ref`'d set to open or lock it is
/// emitted only where the lowered type uses it (#401). An all-scalar `allOf`, and a `$ref` with
/// sibling keywords, re-emit their result under their own name, so a copy no later meet keeps is
/// unused: `ListedOpen` (`string` meeting `Listed`) and the locked copy `UuidNarrowed`'s `allOf`
/// makes of `Listed` were both public types nothing referred to. Turning the option on therefore
/// adds no type: it only opens the types the option-off output already has.
#[test]
fn open_narrowing_adds_no_type_the_output_does_not_use() {
    let open = |spec: Spec| spec.open_narrowing(true);
    let string = "{ type: string }";
    let x = "{ const: x }";
    let listed = "{ $ref: '#/components/schemas/Listed' }";
    let mut specs: Vec<(String, String)> = [
        vec![string, x, listed],
        vec![string, listed, x],
        vec![x, string, listed],
        vec![x, listed, string],
        vec![listed, string, x],
        vec![listed, x, string],
        vec![string, listed],
        vec![listed, string],
        vec![string, listed, listed],
    ]
    .into_iter()
    .map(|order| (order.join(" & "), kind_meeting(&order)))
    .collect();
    // The `$ref`-sibling spelling of `string ∩ Listed`, whose result is re-emitted the same way.
    specs.push((
        "$ref Listed beside type: string".to_owned(),
        kind_meeting(&[string]).replace(
            "                    allOf:\n                      - { type: string }\n",
            "                    $ref: '#/components/schemas/Listed'\n                    type: string\n",
        ),
    ));
    let mut added = Vec::new();
    for (case, spec) in &specs {
        let report = diff_configured(spec, spec, |spec| spec, open);
        // Opening is the option's only effect: one `VariantAdded` per opened set (`kind`, and each
        // member a nested `anyOf` names), and nothing else.
        let kinds = kinds(&report);
        assert!(
            !kinds.is_empty() && kinds.iter().all(|kind| *kind == ChangeKind::VariantAdded),
            "{case}: the option only opens `kind`: {report:?}"
        );
        added.extend(
            report
                .changes
                .iter()
                .filter(|change| change.kind == ChangeKind::TypeAdded)
                .map(|change| format!("{case}: {}", change.location)),
        );
    }
    assert!(added.is_empty(), "types only the option adds: {added:#?}");
}

/// The meets an all-scalar `allOf` folds its members through emit only what its re-emitted result
/// uses, with the option on or off (#401). Two sets that each list a value the other does not meet
/// in a new set, `…Intersection1`, which the `allOf` re-emits as `kind`'s own type: the new set is
/// then unused, and is not emitted. A result that refers to a meet's insert keeps it: two arrays of
/// those sets meet in an array of the new set, which stays the array's item type, while the meet's
/// own array alias, which the re-emitted array replaces, is not emitted (#428).
#[test]
fn an_all_of_emits_no_meet_its_result_does_not_use() {
    let listed = "{ $ref: '#/components/schemas/Listed' }";
    let xy = "{ enum: [x, y] }";
    let string = "{ type: string }";
    for open in [false, true] {
        for order in [
            vec![listed, xy],
            vec![xy, listed],
            vec![listed, xy, string],
            vec![string, xy, listed],
        ] {
            let source = generated_source(&kind_meeting(&order), open);
            assert!(
                !source.contains("pub enum ResponseBodykindIntersection"),
                "open: {open}, {order:?} emits a superseded meet:\n{source}"
            );
        }
        let arrays = kind_meeting(&[
            "{ type: array, items: { $ref: '#/components/schemas/Listed' } }",
            "{ type: array, items: { enum: [x, y] } }",
        ]);
        let source = generated_source(&arrays, open);
        let item = "ResponseBodykindIntersection1Item";
        assert!(
            source.contains(&format!("pub enum {item} {{"))
                && source.contains(&format!("pub type ResponseBodykind = Vec<{item}>;")),
            "open: {open}, the array's meet keeps its item set:\n{source}"
        );
        assert!(
            !source.contains("pub type ResponseBodykindIntersection1 "),
            "open: {open}, the array's superseded meet is emitted:\n{source}"
        );
    }
}

/// A `404` body that is the `allOf` of `members`, one per entry, beside the closed set `Listed`.
fn object_all_of(members: &[&str]) -> String {
    let members: String = members
        .iter()
        .map(|member| format!("                  - {member}\n"))
        .collect();
    full(
        &format!(
            "  /pets:
    get:
      operationId: listPets
      responses:
        '200':
          description: ok
        '404':
          description: missing
          content:
            application/json:
              schema:
                allOf:
{members}"
        ),
        "    Listed: { type: string, enum: [x, listed-only] }
",
    )
}

/// An object `allOf` whose members repeat a property meets it member by member, and a later
/// member's meet supersedes an earlier one's: `string` meeting `Listed` makes the open copy
/// `ListedOpen`, and `const: x` then narrows the field past it (#428). The struct the merge emits
/// refers only to the last meet, so the superseded ones are not emitted — not the open copy, and
/// not, when the repeated property is itself an object, the struct an earlier pair of members met
/// in. Unlike the re-emitted results of #401, the merged struct does refer to some of the meets'
/// inserts, interleaved with the ones it does not, so this holds for each insert on its own.
/// Turning `open_narrowing` on therefore adds no type, in any order of the members.
#[test]
fn an_object_all_of_emits_no_meet_a_later_member_superseded() {
    let object = |property: &str, ty: &str| {
        format!("{{ type: object, required: [{property}], properties: {{ {property}: {ty} }} }}")
    };
    let open = |spec: Spec| spec.open_narrowing(true);
    let types = [
        "{ type: string }",
        "{ $ref: '#/components/schemas/Listed' }",
        "{ const: x }",
    ];
    let orders = [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ];
    let mut added = Vec::new();
    for nested in [false, true] {
        for order in orders {
            let members: Vec<String> = order
                .iter()
                .map(|&index| {
                    let kind = object("kind", types[index]);
                    if nested {
                        object("inner", &kind)
                    } else {
                        kind
                    }
                })
                .collect();
            let members: Vec<&str> = members.iter().map(String::as_str).collect();
            let spec = object_all_of(&members);
            let case = format!("nested: {nested}, {order:?}");
            for open in [false, true] {
                let source = generated_source(&spec, open);
                // No order meets `kind` last against `Listed` alone, so no field keeps its copy.
                assert!(
                    !source.contains("ListedOpen"),
                    "{case}, open: {open} emits a superseded open copy:\n{source}"
                );
                // The second pair of `inner` structs meets in the one the body refers to; the
                // first pair's meet is superseded.
                assert!(
                    source
                        .matches("pub struct ResponseBodyinnerIntersection")
                        .count()
                        == usize::from(nested),
                    "{case}, open: {open} emits a superseded struct meet:\n{source}"
                );
            }
            let report = diff_configured(&spec, &spec, |spec| spec, open);
            added.extend(
                report
                    .changes
                    .iter()
                    .filter(|change| change.kind == ChangeKind::TypeAdded)
                    .map(|change| format!("{case}: {}", change.location)),
            );
        }
    }
    assert!(added.is_empty(), "types only the option adds: {added:#?}");
}
