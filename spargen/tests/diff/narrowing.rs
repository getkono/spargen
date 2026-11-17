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

/// A string enum's catch-all is surface: an open enum carries it as a variant, and a listed value
/// that takes its name moves it, which is a rename a consumer's `match` sees.
#[test]
fn an_open_enums_catch_all_is_a_variant_of_its_surface() {
    let closed = |spec: Spec| spec.with_closed_enums(spargen::StringEnums::Closed);

    // A new listed value is an added variant, major as on a closed enum: the open enum is
    // exhaustive too, so a consumer's exhaustive `match` breaks.
    let added = diff_configured(
        &narrowed_problem("a, b"),
        &narrowed_problem("a, b, c"),
        |spec| spec,
        |spec| spec,
    );
    assert_eq!(kinds(&added), vec![ChangeKind::VariantAdded], "{added:?}");
    assert_eq!(added.bump, Impact::Major);

    // A new listed value spelled `unknown` takes the catch-all's name, so the catch-all is renamed.
    let renamed = diff_configured(
        &narrowed_problem("a, b"),
        &narrowed_problem("a, b, unknown"),
        |spec| spec,
        |spec| spec,
    );
    let details: Vec<&str> = renamed
        .changes
        .iter()
        .map(|change| change.detail.as_str())
        .collect();
    assert!(
        details.contains(
            &"variant removed: `Unknown(UnknownValue<Self>)`, which held any unlisted value"
        ),
        "{details:?}"
    );
    assert!(details.contains(&"variant added: `unknown`"), "{details:?}");
    assert_eq!(renamed.bump, Impact::Major);

    // Turning `closed_enums` on closes the same type in place: it loses the catch-all, a removed
    // variant, and turning it off adds it back.
    let closing = diff_configured(
        &narrowed_problem("a, b"),
        &narrowed_problem("a, b"),
        |spec| spec,
        closed,
    );
    assert_eq!(
        kinds(&closing),
        vec![ChangeKind::VariantRemoved],
        "{closing:?}"
    );
    assert_eq!(closing.bump, Impact::Major);
    let opening = diff_configured(
        &narrowed_problem("a, b"),
        &narrowed_problem("a, b"),
        closed,
        |spec| spec,
    );
    assert_eq!(
        kinds(&opening),
        vec![ChangeKind::VariantAdded],
        "{opening:?}"
    );
    assert_eq!(
        opening.changes[0].detail,
        "variant added: `Unknown(UnknownValue<Self>)`, holding any unlisted value"
    );
    assert_eq!(opening.bump, Impact::Major);

    // `open_narrowing` governs integer and boolean sets only: it leaves a string enum as it is.
    let narrowing = diff_configured(
        &narrowed_problem("a, b"),
        &narrowed_problem("a, b"),
        |spec| spec,
        |spec| spec.with_open_narrowing(spargen::Narrowing::Open),
    );
    assert!(narrowing.changes.is_empty(), "{narrowing:?}");
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

/// The enum `kind` lowers to, with `open_narrowing` on: its variant lines, in order, the catch-all
/// spelled `Unknown`.
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
        .map(|line| {
            if line.starts_with("Unknown(") {
                "Unknown".to_owned()
            } else {
                line.to_owned()
            }
        })
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

/// An `allOf`'s string members meet in the same set whatever order they are written in: the
/// values every member lists, open like every string enum, whether a member is a plain `string`, a
/// `$ref`'d set or another narrowing, and whether `open_narrowing` is on or not. Reordering the
/// members is then a patch: an inline member's type is named for its position, but the meet
/// consumes it and the output carries none of the member types (#924), so moving a member renames
/// no type of the output.
#[test]
fn a_string_meet_is_the_same_open_set_in_every_member_order() {
    let open = |spec: Spec| spec.with_open_narrowing(spargen::Narrowing::Open);
    let string = "{ type: string }";
    let x = "{ const: x }";
    let listed = "{ $ref: '#/components/schemas/Listed' }";
    let expected = ["X,", "Unknown"];

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

    // Two nested narrowings meeting, in both orders.
    let nested_x = "{ allOf: [{ type: string }, { const: x }] }";
    let nested_xy = "{ allOf: [{ type: string }, { enum: [x, y] }] }";
    for order in [[nested_x, nested_xy], [nested_xy, nested_x]] {
        assert_eq!(
            open_kind_variants(&kind_meeting(&order)),
            expected,
            "order {order:?}"
        );
    }
}

/// `open_narrowing` changes nothing about a string set, wherever it is met: every string enum is
/// open (#1444), and the copy an intersection once made of a `$ref`'d set to open it is not made
/// (#401), so turning the option on adds, removes and opens no type of a string `allOf`.
#[test]
fn open_narrowing_leaves_every_string_set_as_it_is() {
    let open = |spec: Spec| spec.with_open_narrowing(spargen::Narrowing::Open);
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
        vec![string, "{ type: string, format: uuid }", x],
        vec!["{ $ref: '#/components/schemas/UuidNarrowed' }", string],
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
    for (case, spec) in &specs {
        let report = diff_configured(spec, spec, |spec| spec, open);
        assert!(
            report.changes.is_empty(),
            "{case}: the option changed a string set: {report:?}"
        );
    }
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
                !source.contains("pub enum ResponseBodyKindIntersection"),
                "open: {open}, {order:?} emits a superseded meet:\n{source}"
            );
        }
        let arrays = kind_meeting(&[
            "{ type: array, items: { $ref: '#/components/schemas/Listed' } }",
            "{ type: array, items: { enum: [x, y] } }",
        ]);
        let source = generated_source(&arrays, open);
        let item = "ResponseBodyKindIntersection1Item";
        assert!(
            source.contains(&format!("pub enum {item} {{"))
                && source.contains(&format!("pub type ResponseBodyKind = Vec<{item}>;")),
            "open: {open}, the array's meet keeps its item set:\n{source}"
        );
        assert!(
            !source.contains("pub type ResponseBodyKindIntersection1 "),
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
/// member's meet supersedes an earlier one's: `const: x` narrows the field past `string` meeting
/// `Listed` (#428). The struct the merge emits refers only to the last meet, so the superseded
/// ones are not emitted: not, when the repeated property is itself an object, the struct an
/// earlier pair of members met in. Unlike the re-emitted results of #401, the merged struct does refer to some of the meets'
/// inserts, interleaved with the ones it does not, so this holds for each insert on its own.
/// Turning `open_narrowing` on therefore adds no type, in any order of the members.
#[test]
fn an_object_all_of_emits_no_meet_a_later_member_superseded() {
    let object = |property: &str, ty: &str| {
        format!("{{ type: object, required: [{property}], properties: {{ {property}: {ty} }} }}")
    };
    let open = |spec: Spec| spec.with_open_narrowing(spargen::Narrowing::Open);
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
                // first pair's meet is superseded. A meet's `…Required` struct is not a meet.
                assert!(
                    source
                        .lines()
                        .filter(|line| {
                            line.contains("pub struct ResponseBodyInnerIntersection")
                                && !line.contains("Required")
                        })
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
