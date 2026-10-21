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
fn matrix_table_rows() {
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
fn label_table_rows() {
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
fn simple_table_rows() {
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
fn form_table_rows() {
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
fn cookie_table_rows() {
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
fn space_delimited_table_rows() {
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
fn pipe_delimited_table_rows() {
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
fn deep_object_table_row() {
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
fn percent_encoding_sets() {
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
        Err(ParameterError::ExpectedObject)
    ));
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

/// An object has no defined part representation. Reusing the query builders silently produced
/// one part per member holding only the *value* — the keys were dropped on the floor — so this
/// now fails loudly. The frontend rejects the construct before generation, making this the
/// belt to that braces.
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
        Err(ParameterError::ExpectedObject)
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
/// separate cause: `source` hands back serde's own error, which `Display` also quotes.
#[test]
fn every_parameter_error_displays_its_rule_and_only_serialize_has_a_source() {
    let serde_error = serde_json::from_str::<u8>("x").unwrap_err();
    let serde_message = serde_error.to_string();
    let serialize = ParameterError::from(serde_error);
    assert_eq!(
        serialize.to_string(),
        format!("parameter serialization failed: {serde_message}")
    );
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
