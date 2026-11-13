use super::*;

/// The specification's own worked example: `color` as a string, an array, and an object.
///
/// `serde_json::Map` is a `BTreeMap` here (the `preserve_order` feature is deliberately not
/// enabled anywhere in the runtime dependency set), so object members serialize in sorted key
/// order — `B`, `G`, `R` — where the specification's table happens to print them in `R`, `G`,
/// `B` order. OpenAPI does not fix object member order for parameter serialization, and sorted
/// order is what keeps generated output deterministic, so the expectations below are the
/// specification's values in sorted order.
fn object() -> Value {
    serde_json::json!({ "R": 100, "G": 200, "B": 150 })
}

fn array() -> Value {
    serde_json::json!(["blue", "black", "brown"])
}

fn string() -> Value {
    serde_json::json!("blue")
}

fn undefined() -> Value {
    Value::Null
}

const U: PercentEncoding = PercentEncoding::Unreserved;
const F: PercentEncoding = PercentEncoding::Form;
const P: PercentEncoding = PercentEncoding::Passthrough;

#[test]
fn matrix_style_matches_the_specifications_serialization_table() {
    assert_eq!(
        serialize_matrix("color", &undefined(), false, U).unwrap(),
        ";color"
    );
    assert_eq!(
        serialize_matrix("color", &string(), false, U).unwrap(),
        ";color=blue"
    );
    assert_eq!(
        serialize_matrix("color", &array(), false, U).unwrap(),
        ";color=blue,black,brown"
    );
    assert_eq!(
        serialize_matrix("color", &object(), false, U).unwrap(),
        ";color=B,150,G,200,R,100"
    );
    assert_eq!(
        serialize_matrix("color", &undefined(), true, U).unwrap(),
        ";color"
    );
    assert_eq!(
        serialize_matrix("color", &string(), true, U).unwrap(),
        ";color=blue"
    );
    assert_eq!(
        serialize_matrix("color", &array(), true, U).unwrap(),
        ";color=blue;color=black;color=brown"
    );
    assert_eq!(
        serialize_matrix("color", &object(), true, U).unwrap(),
        ";B=150;G=200;R=100"
    );
}

#[test]
fn label_style_matches_the_specifications_serialization_table() {
    // The undefined row is `.`, exactly as the specification's table gives it; the URL builder,
    // not this serializer, refuses it where it forms a whole segment.
    assert_eq!(serialize_label(&undefined(), false, U).unwrap(), ".");
    assert_eq!(serialize_label(&string(), false, U).unwrap(), ".blue");
    assert_eq!(
        serialize_label(&array(), false, U).unwrap(),
        ".blue,black,brown"
    );
    assert_eq!(
        serialize_label(&object(), false, U).unwrap(),
        ".B,150,G,200,R,100"
    );
    assert_eq!(serialize_label(&undefined(), true, U).unwrap(), ".");
    assert_eq!(serialize_label(&string(), true, U).unwrap(), ".blue");
    assert_eq!(
        serialize_label(&array(), true, U).unwrap(),
        ".blue.black.brown"
    );
    assert_eq!(
        serialize_label(&object(), true, U).unwrap(),
        ".B=150.G=200.R=100"
    );
}

#[test]
fn simple_style_matches_the_specifications_serialization_table() {
    assert_eq!(serialize_simple(&undefined(), false, U).unwrap(), "");
    assert_eq!(serialize_simple(&string(), false, U).unwrap(), "blue");
    assert_eq!(
        serialize_simple(&array(), false, U).unwrap(),
        "blue,black,brown"
    );
    assert_eq!(
        serialize_simple(&object(), false, U).unwrap(),
        "B,150,G,200,R,100"
    );
    assert_eq!(serialize_simple(&undefined(), true, U).unwrap(), "");
    assert_eq!(serialize_simple(&string(), true, U).unwrap(), "blue");
    assert_eq!(
        serialize_simple(&array(), true, U).unwrap(),
        "blue,black,brown"
    );
    assert_eq!(
        serialize_simple(&object(), true, U).unwrap(),
        "B=150,G=200,R=100"
    );
}

#[test]
fn form_style_matches_the_specifications_serialization_table() {
    assert_eq!(
        serialize_form("color", &undefined(), false, F).unwrap(),
        ["color="]
    );
    assert_eq!(
        serialize_form("color", &string(), false, F).unwrap(),
        ["color=blue"]
    );
    assert_eq!(
        serialize_form("color", &array(), false, F).unwrap(),
        ["color=blue,black,brown"]
    );
    assert_eq!(
        serialize_form("color", &object(), false, F).unwrap(),
        ["color=B,150,G,200,R,100"]
    );
    assert_eq!(
        serialize_form("color", &undefined(), true, F).unwrap(),
        ["color="]
    );
    assert_eq!(
        serialize_form("color", &string(), true, F).unwrap(),
        ["color=blue"]
    );
    assert_eq!(
        serialize_form("color", &array(), true, F).unwrap(),
        ["color=blue", "color=black", "color=brown"]
    );
    assert_eq!(
        serialize_form("color", &object(), true, F).unwrap(),
        ["B=150", "G=200", "R=100"]
    );
}

#[test]
fn cookie_style_matches_the_specifications_serialization_table() {
    // `style: cookie` is `form` with no escaping; the caller joins the fragments with "; ".
    assert_eq!(
        serialize_form("color", &string(), false, P).unwrap(),
        ["color=blue"]
    );
    assert_eq!(
        serialize_form("color", &array(), false, P).unwrap(),
        ["color=blue,black,brown"]
    );
    assert_eq!(
        serialize_form("color", &array(), true, P).unwrap(),
        ["color=blue", "color=black", "color=brown"]
    );
    assert_eq!(
        serialize_form("color", &object(), true, P).unwrap(),
        ["B=150", "G=200", "R=100"]
    );
    // Passthrough really does pass reserved bytes through untouched.
    assert_eq!(
        serialize_form("color", &serde_json::json!("a b,c"), false, P).unwrap(),
        ["color=a b,c"]
    );
}

#[test]
fn space_delimited_style_matches_the_specifications_serialization_table() {
    assert_eq!(
        serialize_delimited("color", &array(), Delimiter::Space, F).unwrap(),
        ["color=blue%20black%20brown"]
    );
    assert_eq!(
        serialize_delimited("color", &object(), Delimiter::Space, F).unwrap(),
        ["color=B%20150%20G%20200%20R%20100"]
    );
}

#[test]
fn pipe_delimited_style_matches_the_specifications_serialization_table() {
    assert_eq!(
        serialize_delimited("color", &array(), Delimiter::Pipe, F).unwrap(),
        ["color=blue%7Cblack%7Cbrown"]
    );
    assert_eq!(
        serialize_delimited("color", &object(), Delimiter::Pipe, F).unwrap(),
        ["color=B%7C150%7CG%7C200%7CR%7C100"]
    );
}

#[test]
fn delimited_styles_reject_scalars() {
    assert!(matches!(
        serialize_delimited("color", &string(), Delimiter::Pipe, F),
        Err(ParameterError::ExpectedComposite)
    ));
}

#[test]
fn deep_object_style_matches_the_specifications_serialization_table() {
    assert_eq!(
        serialize_deep_object("color", &object(), F).unwrap(),
        ["color%5BB%5D=150", "color%5BG%5D=200", "color%5BR%5D=100"]
    );
}

#[test]
fn deep_object_rejects_non_objects() {
    assert!(matches!(
        serialize_deep_object("color", &array(), F),
        Err(ParameterError::ExpectedObject)
    ));
}

/// A form-encoded `in: querystring` value is serialized under an empty name, exploded, as the
/// whole query. An empty object there (every optional member absent) has no name to keep as
/// undefined, so it renders no fragment and the request carries no query, not `?=`.
#[test]
fn an_empty_whole_query_value_renders_no_fragment() {
    for empty in [serde_json::json!({}), serde_json::json!([])] {
        assert!(
            serialize_form("", &empty, true, F).unwrap().is_empty(),
            "{empty}"
        );
    }
    assert_eq!(
        serialize_form("", &serde_json::json!({ "term": "a b" }), true, F).unwrap(),
        ["term=a%20b"]
    );
}

/// Empty composites and composites holding `null`. RFC 6570 §2.3 counts an empty list or map as
/// undefined, so an empty composite renders as the 3.2 table's `undefined` column: `;c` for both
/// `matrix` rows, `c=` for both `form` rows, `.` for `label` and an empty value for `simple`.
/// `spaceDelimited`, `pipeDelimited` and `deepObject` are `n/a` there, and keep an empty value
/// under the name and no fragment respectively. A `null` member renders as an empty scalar in its
/// place.
#[test]
fn empty_and_null_bearing_composites_render_per_style_and_explode() {
    let empty_array = serde_json::json!([]);
    let empty_object = serde_json::json!({});
    let null_item = serde_json::json!(["a", null]);
    let null_member = serde_json::json!({ "a": null, "b": "x" });

    for empty in [&empty_array, &empty_object] {
        for explode in [false, true] {
            assert_eq!(serialize_simple(empty, explode, U).unwrap(), "", "{empty}");
            assert_eq!(serialize_label(empty, explode, U).unwrap(), ".", "{empty}");
        }
        for explode in [false, true] {
            assert_eq!(
                serialize_matrix("c", empty, explode, U).unwrap(),
                ";c",
                "{empty}"
            );
            assert_eq!(
                serialize_form("c", empty, explode, F).unwrap(),
                ["c="],
                "{empty}"
            );
        }
        for delimiter in [Delimiter::Space, Delimiter::Pipe] {
            assert_eq!(
                serialize_delimited("c", empty, delimiter, F).unwrap(),
                ["c="]
            );
        }
    }
    assert!(serialize_deep_object("c", &empty_object, F)
        .unwrap()
        .is_empty());

    assert_eq!(serialize_simple(&null_item, false, U).unwrap(), "a,");
    assert_eq!(serialize_simple(&null_item, true, U).unwrap(), "a,");
    assert_eq!(serialize_label(&null_item, false, U).unwrap(), ".a,");
    assert_eq!(serialize_label(&null_item, true, U).unwrap(), ".a.");
    assert_eq!(
        serialize_matrix("c", &null_item, false, U).unwrap(),
        ";c=a,"
    );
    assert_eq!(
        serialize_matrix("c", &null_item, true, U).unwrap(),
        ";c=a;c="
    );
    assert_eq!(serialize_form("c", &null_item, false, F).unwrap(), ["c=a,"]);
    assert_eq!(
        serialize_form("c", &null_item, true, F).unwrap(),
        ["c=a", "c="]
    );
    assert_eq!(
        serialize_delimited("c", &null_item, Delimiter::Pipe, F).unwrap(),
        ["c=a%7C"]
    );

    assert_eq!(serialize_simple(&null_member, false, U).unwrap(), "a,,b,x");
    assert_eq!(serialize_simple(&null_member, true, U).unwrap(), "a=,b=x");
    assert_eq!(serialize_label(&null_member, false, U).unwrap(), ".a,,b,x");
    assert_eq!(serialize_label(&null_member, true, U).unwrap(), ".a=.b=x");
    assert_eq!(
        serialize_matrix("c", &null_member, false, U).unwrap(),
        ";c=a,,b,x"
    );
    assert_eq!(
        serialize_matrix("c", &null_member, true, U).unwrap(),
        ";a=;b=x"
    );
    assert_eq!(
        serialize_form("c", &null_member, false, F).unwrap(),
        ["c=a,,b,x"]
    );
    assert_eq!(
        serialize_form("c", &null_member, true, F).unwrap(),
        ["a=", "b=x"]
    );
    assert_eq!(
        serialize_delimited("c", &null_member, Delimiter::Space, F).unwrap(),
        ["c=a%20%20b%20x"]
    );
    assert_eq!(
        serialize_deep_object("c", &null_member, F).unwrap(),
        ["c%5Ba%5D=", "c%5Bb%5D=x"]
    );
}

/// A boolean or number renders as its JSON literal (`true`, `1.5`, `-2`), never quoted, in
/// every style, alone and inside a composite.
#[test]
fn booleans_and_numbers_render_as_json_literals_in_every_style() {
    let flag = serde_json::json!(true);
    let items = serde_json::json!([true, 1.5, -2]);
    let members = serde_json::json!({ "n": 1.5, "t": false });

    assert_eq!(serialize_simple(&flag, false, U).unwrap(), "true");
    assert_eq!(serialize_label(&flag, false, U).unwrap(), ".true");
    assert_eq!(serialize_matrix("v", &flag, false, U).unwrap(), ";v=true");
    assert_eq!(serialize_form("v", &flag, false, F).unwrap(), ["v=true"]);

    assert_eq!(serialize_simple(&items, false, U).unwrap(), "true,1.5,-2");
    assert_eq!(serialize_label(&items, false, U).unwrap(), ".true,1.5,-2");
    assert_eq!(serialize_label(&items, true, U).unwrap(), ".true.1.5.-2");
    assert_eq!(
        serialize_matrix("v", &items, false, U).unwrap(),
        ";v=true,1.5,-2"
    );
    assert_eq!(
        serialize_matrix("v", &items, true, U).unwrap(),
        ";v=true;v=1.5;v=-2"
    );
    assert_eq!(
        serialize_form("v", &items, false, F).unwrap(),
        ["v=true,1.5,-2"]
    );
    assert_eq!(
        serialize_form("v", &items, true, F).unwrap(),
        ["v=true", "v=1.5", "v=-2"]
    );
    assert_eq!(
        serialize_delimited("v", &items, Delimiter::Pipe, F).unwrap(),
        ["v=true%7C1.5%7C-2"]
    );
    assert_eq!(
        serialize_delimited("v", &items, Delimiter::Space, F).unwrap(),
        ["v=true%201.5%20-2"]
    );

    assert_eq!(
        serialize_simple(&members, false, U).unwrap(),
        "n,1.5,t,false"
    );
    assert_eq!(
        serialize_simple(&members, true, U).unwrap(),
        "n=1.5,t=false"
    );
    assert_eq!(
        serialize_form("v", &members, true, F).unwrap(),
        ["n=1.5", "t=false"]
    );
    assert_eq!(
        serialize_deep_object("v", &members, F).unwrap(),
        ["v%5Bn%5D=1.5", "v%5Bt%5D=false"]
    );
}

#[test]
fn delimiters_are_distinguishable_from_data() {
    // The whole point of encoding here rather than through `url`: a comma that joins two items
    // and a comma inside one item must not look alike on the wire.
    let values = serde_json::json!(["a,b", "c"]);
    assert_eq!(
        serialize_form("color", &values, false, F).unwrap(),
        ["color=a%2Cb,c"]
    );
    let piped = serialize_delimited(
        "color",
        &serde_json::json!(["a|b", "c"]),
        Delimiter::Pipe,
        F,
    )
    .unwrap();
    assert_eq!(piped, ["color=a%7Cb%7Cc"]);
    let spaced = serialize_delimited(
        "color",
        &serde_json::json!(["a b", "c"]),
        Delimiter::Space,
        F,
    )
    .unwrap();
    assert_eq!(spaced, ["color=a%20b%20c"]);
}

#[test]
fn each_percent_encoding_escapes_exactly_the_bytes_its_set_excludes() {
    let raw = "a/b?c#d e%f~g,h|i[j]k&l=m+n";
    assert_eq!(
        encode(raw, PercentEncoding::Unreserved),
        "a%2Fb%3Fc%23d%20e%25f~g%2Ch%7Ci%5Bj%5Dk%26l%3Dm%2Bn"
    );
    assert_eq!(
        encode(raw, PercentEncoding::Form),
        "a%2Fb%3Fc%23d%20e%25f%7Eg%2Ch%7Ci%5Bj%5Dk%26l%3Dm%2Bn"
    );
    assert_eq!(
        encode(raw, PercentEncoding::Reserved),
        "a/b?c%23d%20e%25f~g,h%7Ci%5Bj%5Dk%26l%3Dm%2Bn"
    );
    assert_eq!(
        encode(raw, PercentEncoding::ReservedPath),
        "a%2Fb%3Fc%23d%20e%25f~g,h%7Ci[j]k&l=m+n"
    );
    assert_eq!(encode(raw, PercentEncoding::Passthrough), raw);
}

#[test]
fn reserved_expansion_preserves_triples_and_encodes_bare_percent() {
    assert_eq!(encode("%41", PercentEncoding::Reserved), "%41");
    assert_eq!(encode("%zz", PercentEncoding::Reserved), "%25zz");
    assert_eq!(encode("100%", PercentEncoding::Reserved), "100%25");
    // Without reserved expansion a triple is re-encoded, because the value is opaque data.
    assert_eq!(encode("%41", PercentEncoding::Unreserved), "%2541");
}

/// The triple look-ahead at every position relative to the end of the value: a triple cut
/// short by the end is a bare `%`, one that ends exactly at the end or has data after it is
/// passed through, and so is one that starts well into the value. Both reserved sets share it.
#[test]
fn reserved_expansion_reads_a_triple_only_where_all_three_bytes_exist() {
    for encoding in [PercentEncoding::Reserved, PercentEncoding::ReservedPath] {
        for (raw, expected) in [
            ("%", "%25"),
            ("%4", "%254"),
            ("a%4", "a%254"),
            ("a%41", "a%41"),
            ("x%41y", "x%41y"),
            ("abc%41", "abc%41"),
            ("abcde%4", "abcde%254"),
        ] {
            assert_eq!(encode(raw, encoding), expected, "{encoding:?} {raw:?}");
        }
    }
}

#[test]
fn path_values_never_carry_unescaped_slash_question_or_hash() {
    for encoding in [PercentEncoding::Unreserved, PercentEncoding::ReservedPath] {
        let encoded = encode("a/b?c#d", encoding);
        assert!(
            !encoded.contains('/'),
            "{encoding:?} leaked a slash: {encoded}"
        );
        assert!(
            !encoded.contains('?'),
            "{encoding:?} leaked a question mark: {encoded}"
        );
        assert!(
            !encoded.contains('#'),
            "{encoding:?} leaked a hash: {encoded}"
        );
    }
}

/// The ASCII bytes each percent-encoding set emits literally, written out independently of
/// `keeps` so the properties below check the set against its documentation, not itself.
fn literals(encoding: PercentEncoding) -> String {
    let unreserved = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";
    match encoding {
        PercentEncoding::Unreserved => unreserved.to_owned(),
        PercentEncoding::Form => unreserved.replace('~', ""),
        PercentEncoding::Reserved => format!("{unreserved}:/?@!$'()*,;"),
        PercentEncoding::ReservedPath => format!("{unreserved}:@!$'()*,;&=+[]"),
        PercentEncoding::Passthrough => unreachable!("passthrough has no alphabet"),
    }
}

const ENCODING_SETS: [PercentEncoding; 4] = [
    PercentEncoding::Unreserved,
    PercentEncoding::Form,
    PercentEncoding::Reserved,
    PercentEncoding::ReservedPath,
];

/// Decode `%HH` triples, refusing a `%` that does not begin one: the output of `encode` must
/// never contain such a `%`, so this decoder doubles as a well-formedness check.
fn strict_decode(encoded: &str) -> Option<Vec<u8>> {
    let bytes = encoded.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let triple = encoded.get(index + 1..index + 3)?;
            if !triple.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return None;
            }
            out.push(u8::from_str_radix(triple, 16).ok()?);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    Some(out)
}

/// Decode exactly the triples reserved expansion passes through — a `%` followed by two hex
/// digits — and keep every other byte, a bare `%` included.
fn lenient_decode(raw: &str) -> Vec<u8> {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let triple = raw
            .get(index + 1..index + 3)
            .filter(|_| bytes[index] == b'%')
            .filter(|triple| triple.bytes().all(|byte| byte.is_ascii_hexdigit()));
        match triple {
            Some(triple) => {
                out.push(u8::from_str_radix(triple, 16).expect("two hex digits"));
                index += 3;
            }
            None => {
                out.push(bytes[index]);
                index += 1;
            }
        }
    }
    out
}

/// Values biased toward what the encoder treats specially: `%` and hex digits (so triples,
/// cut-short triples and bare `%` all occur), every delimiter, and multi-byte UTF-8.
fn value() -> impl proptest::strategy::Strategy<Value = String> {
    proptest::string::string_regex("([%0-9a-fA-FzZ/?#&=+~ ,;\\[\\]!$'()*:@|.\\-_]|é|🦀|\\PC){0,24}")
        .expect("a valid regex")
}

proptest::proptest! {
    /// Encoding never loses data: decoding the output gives back the value's bytes — exactly
    /// for the opaque sets, and for reserved expansion with the triples it passed through
    /// read as the bytes they spell, since passing a triple through is that expansion's
    /// point.
    #[test]
    fn decoding_the_encoding_recovers_the_value(raw in value()) {
        for encoding in ENCODING_SETS {
            let encoded = encode(&raw, encoding);
            let decoded = strict_decode(&encoded);
            let expected = match encoding {
                PercentEncoding::Reserved | PercentEncoding::ReservedPath => {
                    lenient_decode(&raw)
                }
                _ => raw.as_bytes().to_vec(),
            };
            proptest::prop_assert_eq!(decoded, Some(expected), "{:?} {:?}", encoding, encoded);
        }
        proptest::prop_assert_eq!(encode(&raw, PercentEncoding::Passthrough), raw);
    }

    /// The output alphabet is the set's literals plus `%HH` triples, and nothing else; a
    /// triple the encoder writes itself is upper-case.
    #[test]
    fn the_output_alphabet_is_the_sets_literals_and_triples(raw in value()) {
        for encoding in ENCODING_SETS {
            let literals = literals(encoding);
            let encoded = encode(&raw, encoding);
            let bytes = encoded.as_bytes();
            let mut index = 0;
            while index < bytes.len() {
                if bytes[index] == b'%' {
                    let triple = &bytes[index + 1..index + 3];
                    proptest::prop_assert!(
                        triple.iter().all(u8::is_ascii_hexdigit),
                        "{:?} {:?}", encoding, encoded
                    );
                    index += 3;
                } else {
                    proptest::prop_assert!(
                        literals.as_bytes().contains(&bytes[index]),
                        "{:?} emitted {:?} literally in {:?}",
                        encoding, bytes[index] as char, encoded
                    );
                    index += 1;
                }
            }
            // The opaque sets write every triple themselves; reserved expansion may also pass
            // through a lower-case triple the value already held.
            if matches!(encoding, PercentEncoding::Unreserved | PercentEncoding::Form) {
                let upper = encoded.split('%').skip(1).all(|rest| {
                    rest.as_bytes()[..2].iter().all(|byte| !byte.is_ascii_lowercase())
                });
                proptest::prop_assert!(upper, "{:?} {:?}", encoding, encoded);
            }
        }
    }

    /// A path value can never re-route its request: the two path sets never emit `/`, `?`
    /// or `#` literally, whatever the value holds.
    #[test]
    fn path_sets_never_emit_a_slash_question_mark_or_hash(raw in value()) {
        for encoding in [PercentEncoding::Unreserved, PercentEncoding::ReservedPath] {
            let encoded = encode(&raw, encoding);
            proptest::prop_assert!(
                !encoded.contains(['/', '?', '#']),
                "{:?} {:?}", encoding, encoded
            );
        }
    }
}

#[test]
fn multibyte_values_encode_per_utf8_byte() {
    assert_eq!(encode("é", PercentEncoding::Unreserved), "%C3%A9");
    assert_eq!(encode("🦀", PercentEncoding::Form), "%F0%9F%A6%80");
}

#[test]
fn serialize_form_body_covers_both_encoding_modes() {
    // Media-type mode renders each property by its resolved content type; style mode uses
    // query-style serialization. Both appear in one body here.
    let body = serde_json::json!({
        "id": "f81d4fae",
        "address": { "city": "Somewhere", "zip": "99999+1234" },
        "tags": ["a", "b"]
    });
    let properties = [
        FormProperty {
            name: "id",
            mode: FormMode::Text,
        },
        FormProperty {
            name: "address",
            mode: FormMode::Json,
        },
        FormProperty {
            name: "tags",
            mode: FormMode::Style {
                style: FormStyle::Form,
                explode: true,
                encoding: PercentEncoding::Form,
            },
        },
    ];
    assert_eq!(
        serialize_form_body(&body, &properties, None).unwrap(),
        "id=f81d4fae\
         &address=%7B%22city%22%3A%22Somewhere%22%2C%22zip%22%3A%2299999%2B1234%22%7D\
         &tags=a&tags=b"
    );
}

#[test]
fn form_body_omits_null_properties() {
    let body = serde_json::json!({ "a": "x", "b": null });
    let properties = [
        FormProperty {
            name: "a",
            mode: FormMode::Text,
        },
        FormProperty {
            name: "b",
            mode: FormMode::Text,
        },
    ];
    assert_eq!(
        serialize_form_body(&body, &properties, None).unwrap(),
        "a=x"
    );
}

/// A style-mode property holding an empty array is undefined, as in a query, and keeps its name
/// with an empty value under either `explode`, where a `null` property is omitted.
#[test]
fn form_body_sends_an_empty_style_property_as_undefined() {
    let body = serde_json::json!({ "a": [], "b": [] });
    let style = |explode| FormMode::Style {
        style: FormStyle::Form,
        explode,
        encoding: PercentEncoding::Form,
    };
    let properties = [
        FormProperty {
            name: "a",
            mode: style(true),
        },
        FormProperty {
            name: "b",
            mode: style(false),
        },
    ];
    assert_eq!(
        serialize_form_body(&body, &properties, None).unwrap(),
        "a=&b="
    );
}

#[test]
fn form_body_emits_properties_in_the_declared_order() {
    // Order is codegen's, not the JSON object's, so output stays deterministic.
    let body = serde_json::json!({ "a": "1", "b": "2" });
    let properties = [
        FormProperty {
            name: "b",
            mode: FormMode::Text,
        },
        FormProperty {
            name: "a",
            mode: FormMode::Text,
        },
    ];
    assert_eq!(
        serialize_form_body(&body, &properties, None).unwrap(),
        "b=2&a=1"
    );
}

/// The shape codegen emits for a body with a typed `additionalProperties`: declared fields, an
/// optional one skipped when `None`, then the flattened overflow map.
#[derive(serde::Serialize)]
struct OverflowBody<V> {
    a: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    b: Option<&'static str>,
    #[serde(flatten)]
    overflow: std::collections::BTreeMap<&'static str, V>,
}

impl<V> OverflowBody<V> {
    fn additional(&self, mode: FormMode) -> Option<(FormMode, usize)> {
        Some((mode, self.overflow.len()))
    }
}

const OVERFLOW_PROPERTIES: [FormProperty; 2] = [
    FormProperty {
        name: "a",
        mode: FormMode::Text,
    },
    FormProperty {
        name: "b",
        mode: FormMode::Text,
    },
];

#[test]
fn form_body_sends_additional_members_after_the_declared_ones() {
    // Issue #697: a typed `additionalProperties` overflow was dropped from the wire. Its members
    // follow the declared properties, ordered by key, in the mode lowering resolved for their
    // type, and a `null` member is omitted.
    let body = OverflowBody {
        a: "x",
        b: Some("y"),
        overflow: [("z", Some("last")), ("b c", Some("y&z")), ("n", None)].into(),
    };
    assert_eq!(
        serialize_form_body(&body, &OVERFLOW_PROPERTIES, body.additional(FormMode::Text)).unwrap(),
        "a=x&b=y&b%20c=y%26z&z=last"
    );
    let body = OverflowBody {
        a: "x",
        b: None,
        overflow: [("extra", [1, 2])].into(),
    };
    assert_eq!(
        serialize_form_body(&body, &OVERFLOW_PROPERTIES, body.additional(FormMode::Json)).unwrap(),
        "a=x&extra=%5B1%2C2%5D"
    );
}

#[test]
fn form_body_sends_the_declared_value_under_a_colliding_overflow_key() {
    // An overflow key that repeats a declared property's name is serialized after the declared
    // field, and a `serde_json::Value` map would keep the overflow's entry. The declared value is
    // the one sent, under the declared mode, and nothing is sent when the declared field is
    // `None`, as in a multipart body.
    let body = OverflowBody {
        a: "declared",
        b: None,
        overflow: [("a", "overflow-a"), ("b", "overflow-b"), ("c", "kept")].into(),
    };
    assert_eq!(
        serialize_form_body(&body, &OVERFLOW_PROPERTIES, body.additional(FormMode::Json)).unwrap(),
        "a=declared&c=%22kept%22"
    );
}

#[test]
fn form_body_without_an_additional_mode_sends_only_declared_members() {
    let body = serde_json::json!({ "a": "x", "b": "y" });
    let properties = [FormProperty {
        name: "a",
        mode: FormMode::Text,
    }];
    assert_eq!(
        serialize_form_body(&body, &properties, None).unwrap(),
        "a=x"
    );
}

#[test]
fn form_body_rejects_a_non_object() {
    let properties: [FormProperty; 0] = [];
    assert!(matches!(
        serialize_form_body(&serde_json::json!("scalar"), &properties, None),
        Err(ParameterError::FormBodyNotObject)
    ));
}

#[test]
fn ordered_members_names_a_json_object_as_what_it_expected() {
    // `serialize_form_body` refuses a non-object before it reads members, so only the reader
    // itself can report what it expected.
    let error = match serde_json::from_str::<OrderedMembers>("[]") {
        Ok(_) => panic!("a JSON array was read as an object's members"),
        Err(error) => error,
    };
    assert!(
        error.to_string().contains("expected a JSON object"),
        "{error}"
    );
}

#[test]
fn serialize_multipart_values_yields_one_value_per_array_item() {
    // The part name travels in `Content-Disposition`, so the values carry no `name=` prefix,
    // and multipart values are never percent-encoded.
    let values = serde_json::json!(["a b", "c,d"]);
    assert_eq!(
        serialize_multipart_values(&values, FormStyle::Form, true).unwrap(),
        ["a b", "c,d"]
    );
    assert_eq!(
        serialize_multipart_values(&serde_json::json!("solo"), FormStyle::Form, false).unwrap(),
        ["solo"]
    );
}

/// The specification is explicit: "When using RFC6570-style serialization for
/// `multipart/form-data`, URI percent-encoding MUST NOT be applied." Building these values out
/// of the query fragment builders put `%20`/`%7C` triples into part bodies instead of the
/// literal delimiters.
#[test]
fn multipart_delimiters_are_literal_not_percent_encoded() {
    let values = array();
    assert_eq!(
        serialize_multipart_values(&values, FormStyle::Delimited(Delimiter::Space), false).unwrap(),
        ["blue black brown"]
    );
    assert_eq!(
        serialize_multipart_values(&values, FormStyle::Delimited(Delimiter::Pipe), false).unwrap(),
        ["blue|black|brown"]
    );
    assert_eq!(
        serialize_multipart_values(&values, FormStyle::Form, false).unwrap(),
        ["blue,black,brown"]
    );
}

/// Data bytes are passed through untouched too — a part value is not a query fragment.
#[test]
fn multipart_values_are_never_percent_encoded() {
    let values = serde_json::json!(["a b&c", "d=e"]);
    assert_eq!(
        serialize_multipart_values(&values, FormStyle::Form, true).unwrap(),
        ["a b&c", "d=e"]
    );
}

/// An object has no defined part representation, so it fails loudly rather than yielding one
/// part per member holding only the *value*, with the keys dropped. The frontend rejects the
/// construct before generation, making this the belt to that braces.
#[test]
fn multipart_rejects_object_values_rather_than_dropping_their_keys() {
    assert!(matches!(
        serialize_multipart_values(&object(), FormStyle::Form, true),
        Err(ParameterError::NestedValue)
    ));
    assert!(matches!(
        serialize_multipart_values(&object(), FormStyle::Form, false),
        Err(ParameterError::NestedValue)
    ));
    // `deepObject` is defined only for `in: query`.
    assert!(matches!(
        serialize_multipart_values(&object(), FormStyle::DeepObject, false),
        Err(ParameterError::DeepObjectInMultipart)
    ));
}

#[test]
fn nested_values_are_rejected() {
    let nested = serde_json::json!([["a"]]);
    assert!(matches!(
        serialize_simple(&nested, false, U),
        Err(ParameterError::NestedValue)
    ));
}

/// Each variant's message names the rule it broke, and only a serialization failure has a
/// separate cause: `source` hands back serde's own error, which `Display` leaves to the chain.
#[test]
fn every_parameter_error_displays_its_rule_and_only_serialize_has_a_source() {
    let serde_error = serde_json::from_str::<u8>("x").unwrap_err();
    let serde_message = serde_error.to_string();
    let serialize = ParameterError::from(serde_error);
    assert_eq!(serialize.to_string(), "parameter serialization failed");
    let source = std::error::Error::source(&serialize).expect("serialize carries its cause");
    assert!(source.downcast_ref::<serde_json::Error>().is_some());
    assert_eq!(source.to_string(), serde_message);

    for (error, expected) in [
        (
            ParameterError::NestedValue,
            "nested arrays and objects are not supported by OpenAPI parameter serialization",
        ),
        (
            ParameterError::ExpectedObject,
            "`style: deepObject` requires an object parameter value",
        ),
        (
            ParameterError::FormBodyNotObject,
            "an `application/x-www-form-urlencoded` request body requires an object value",
        ),
        (
            ParameterError::DeepObjectInMultipart,
            "`style: deepObject` is defined only for query parameters, not multipart part values",
        ),
        (
            ParameterError::ExpectedComposite,
            "`style: spaceDelimited` and `style: pipeDelimited` require an array or object \
             parameter value",
        ),
    ] {
        assert_eq!(error.to_string(), expected);
        assert!(std::error::Error::source(&error).is_none(), "{error}");
    }
}

/// A parameter value whose strings are biased to control characters, in every shape a
/// serializer branches on: a scalar, an array of scalars, an array holding an array, an object
/// with scalar members (control characters in its keys too), and an object holding an object.
fn control_value() -> impl proptest::strategy::Strategy<Value = Value> {
    use proptest::prelude::*;
    let text = || {
        proptest::string::string_regex("([\\x00-\\x1f]|\\x7f|[\\u{80}-\\u{9f}]|a|,|=|\\PC){0,6}")
            .expect("a valid regex")
    };
    let scalar = prop_oneof![
        text().prop_map(Value::String),
        Just(Value::Null),
        any::<i64>().prop_map(Value::from),
    ];
    let object = |member: BoxedStrategy<Value>| {
        proptest::collection::btree_map(text(), member, 0..3)
            .prop_map(|members| Value::Object(members.into_iter().collect()))
    };
    prop_oneof![
        scalar.clone(),
        proptest::collection::vec(scalar.clone(), 0..3).prop_map(Value::Array),
        proptest::collection::vec(scalar.clone(), 0..3)
            .prop_map(|items| Value::Array(vec![Value::Array(items)])),
        object(scalar.clone().boxed()),
        object(object(scalar.boxed()).boxed()),
    ]
}

proptest::proptest! {
    /// No `ParameterError` echoes the value it refused: through every serializer, style,
    /// explode setting and encoding, an error's whole `source()` chain stays on one line
    /// whatever control characters the value's strings and keys hold (#452's request-side
    /// sibling).
    #[test]
    fn a_parameter_error_message_is_a_single_line(
        value in control_value(),
        explode in proptest::bool::ANY,
        encoding in proptest::sample::select(vec![
            PercentEncoding::Unreserved,
            PercentEncoding::Form,
            PercentEncoding::Reserved,
            PercentEncoding::ReservedPath,
            PercentEncoding::Passthrough,
        ]),
    ) {
        let mut errors: Vec<ParameterError> = Vec::new();
        errors.extend(serialize_simple(&value, explode, encoding).err());
        errors.extend(serialize_matrix("p", &value, explode, encoding).err());
        errors.extend(serialize_label(&value, explode, encoding).err());
        errors.extend(serialize_form("p", &value, explode, encoding).err());
        errors.extend(serialize_deep_object("p", &value, encoding).err());
        for delimiter in [Delimiter::Space, Delimiter::Pipe] {
            errors.extend(serialize_delimited("p", &value, delimiter, encoding).err());
        }
        for style in [
            FormStyle::Form,
            FormStyle::Delimited(Delimiter::Space),
            FormStyle::DeepObject,
        ] {
            errors.extend(serialize_multipart_values(&value, style, explode).err());
        }
        for error in &errors {
            let messages: Vec<String> = std::iter::successors(
                Some(error as &(dyn std::error::Error + 'static)),
                |error| error.source(),
            )
            .map(ToString::to_string)
            .collect();
            for message in messages {
                proptest::prop_assert!(!message.contains(char::is_control), "{message:?}");
            }
        }
    }
}

/// A join whose items need no escaping fills the one buffer it starts with: one allocation,
/// whatever the item count, where collecting each item into a `Vec<String>` and joining it took
/// one per item, one for the `Vec` and one for the join.
#[test]
fn join_scalars_writes_unescaped_items_into_one_allocation() {
    let Value::Array(values) = serde_json::json!(["blue", 7, true, -1.5, null, "brown"]) else {
        unreachable!()
    };
    let mut joined = None;
    let info = allocation_counter::measure(|| {
        joined = Some(join_scalars(values.iter(), ",", U).unwrap());
    });
    assert_eq!(joined.as_deref(), Some("blue,7,true,-1.5,,brown"));
    assert_eq!(info.count_total, 1, "{info:?}");
    // The buffer is sized, and allocated once, at each item's longest unescaped rendering (a
    // string's length, `false`'s five bytes for a bool, 24 for a number, none for `null`) plus
    // one separator.
    let sized = [4, 24, 5, 24, 0, 5]
        .map(|hint: u64| hint + 1)
        .iter()
        .sum::<u64>();
    assert_eq!(info.bytes_total, sized, "{info:?}");
}

#[test]
fn object_flat_writes_unescaped_members_into_one_allocation() {
    let Value::Object(values) = object() else {
        unreachable!()
    };
    let mut flat = None;
    let info = allocation_counter::measure(|| {
        flat = Some(object_flat(&values, "%20", U).unwrap());
    });
    assert_eq!(flat.as_deref(), Some("B%20150%20G%20200%20R%20100"));
    assert_eq!(info.count_total, 1, "{info:?}");
    // Each member reserves its key, its value's longest unescaped rendering (24 for a number) and
    // two separators.
    let sized = 3 * (1 + 24 + 2 * "%20".len() as u64);
    assert_eq!(info.bytes_total, sized, "{info:?}");
}

/// Escaping lengthens the rendering past the buffer's starting capacity, so the buffer grows;
/// each growth at least doubles it, so the count stays far below one allocation per item.
#[test]
fn join_scalars_and_object_flat_grow_one_buffer_when_items_need_escaping() {
    let values: Vec<Value> = (0..64).map(|_| Value::from("a b")).collect();
    let members: Map<String, Value> = (0..64)
        .map(|index| (format!("k {index:02}"), Value::from("a b")))
        .collect();
    let mut joined = None;
    let mut flat = None;
    let joined_info = allocation_counter::measure(|| {
        joined = Some(join_scalars(values.iter(), ",", U).unwrap());
    });
    let flat_info = allocation_counter::measure(|| {
        flat = Some(object_flat(&members, ",", U).unwrap());
    });
    assert_eq!(joined, Some(vec!["a%20b"; 64].join(",")));
    assert!(flat
        .as_deref()
        .unwrap()
        .starts_with("k%2000,a%20b,k%2001,a%20b,"));
    assert!(joined_info.count_total <= 2, "{joined_info:?}");
    assert!(flat_info.count_total <= 2, "{flat_info:?}");
}

/// A parameter failure reaches the caller as `RequestError::Other`, whose source downcasts to the
/// `ParameterError` the root re-exports.
#[test]
fn a_parameter_failure_downcasts_from_the_request_error() {
    use std::error::Error as _;

    let error = serialize_deep_object("filter", &"scalar", F).unwrap_err();
    let error = crate::Error::<()>::request_construction(error);
    let crate::Error::RequestConstruction(request) = &error else {
        panic!("expected a request-construction error, got {error:?}");
    };
    assert!(
        matches!(request, crate::RequestError::Other(_)),
        "{request:?}"
    );
    let cause = request.source().expect("`Other` carries its cause");
    assert!(
        matches!(
            cause.downcast_ref::<crate::ParameterError>(),
            Some(crate::ParameterError::ExpectedObject)
        ),
        "{cause:?}"
    );
}
