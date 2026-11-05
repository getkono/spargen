use super::*;

fn parse_json_ok(text: &str) -> SpannedValue {
    let mut diags = Diagnostics::default();
    let value = parse_json(FileId(0), text, &mut diags).expect("json parses");
    assert!(!diags.has_errors(), "unexpected diagnostics: {diags:?}");
    value
}

fn parse_yaml_ok(text: &str) -> SpannedValue {
    let mut diags = Diagnostics::default();
    let value = parse_yaml(FileId(0), text, &mut diags).expect("yaml parses");
    assert!(!diags.has_errors(), "unexpected diagnostics: {diags:?}");
    value
}

#[test]
fn json_nested_node_span_is_precise() {
    // A property three levels deep sits on a known line/column; its span must point there,
    // not at line 1.
    let text = "{\n  \"a\": {\n    \"b\": {\n      \"c\": 42\n    }\n  }\n}\n";
    let root = parse_json_ok(text);
    let c = root
        .get("a")
        .and_then(|value| value.get("b"))
        .and_then(|value| value.get("c"))
        .expect("nested `c` exists");
    // `      "c": 42` is on line 4; the value `42` begins at column 12.
    assert_eq!(c.span().start.line, 4, "value line");
    assert_eq!(c.span().start.col, 12, "value column");
    assert_eq!(c.node, Node::Number(Number::Int(42)));

    // The key span points at the key token, also on line 4 (column 7 for the opening quote).
    let b = root.get("a").and_then(|value| value.get("b")).unwrap();
    let (key, _) = b.as_object().unwrap().iter().next().unwrap();
    assert_eq!(key.name, "c");
    assert_eq!(key.span.start.line, 4);
    assert_eq!(key.span.start.col, 7);
}

#[test]
fn json_number_classification_matches_serde() {
    let root = parse_json_ok("{\"i\": -7, \"u\": 18446744073709551615, \"f\": 1.5, \"e\": 1e3}");
    assert_eq!(root.get("i").unwrap().node, Node::Number(Number::Int(-7)));
    assert_eq!(
        root.get("u").unwrap().node,
        Node::Number(Number::UInt(18446744073709551615))
    );
    assert_eq!(
        root.get("f").unwrap().node,
        Node::Number(Number::Float(1.5))
    );
    assert_eq!(
        root.get("e").unwrap().node,
        Node::Number(Number::Float(1000.0))
    );
}

#[test]
fn yaml_integer_classification_matches_the_json_parser() {
    // Issue #540: `yaml_rust2` reads an integer literal beyond `i64::MAX` as a `Real`; it must
    // still classify as the JSON parser classifies it, `Int`, else `UInt`, else `Float`.
    let text = "i: -7\nmax: 9223372036854775807\nu: 18446744073709551615\n\
                plus: +18446744073709551615\nover: 18446744073709551616\n\
                under: -9223372036854775809\nf: 1.5\ntagged: !!float 5\n";
    let root = parse_yaml_ok(text);
    let json = parse_json_ok(
        "{\"i\": -7, \"max\": 9223372036854775807, \"u\": 18446744073709551615, \
         \"over\": 18446744073709551616, \"under\": -9223372036854775809, \"f\": 1.5}",
    );
    for key in ["i", "max", "u", "over", "under", "f"] {
        assert_eq!(
            root.get(key).unwrap().node,
            json.get(key).unwrap().node,
            "{key}"
        );
    }
    assert_eq!(
        root.get("u").unwrap().node,
        Node::Number(Number::UInt(u64::MAX))
    );
    assert_eq!(
        root.get("plus").unwrap().node,
        Node::Number(Number::UInt(u64::MAX))
    );
    assert_eq!(
        root.get("over").unwrap().node,
        Node::Number(Number::Float(18446744073709551616.0))
    );
    // An explicit `!!float` stays a float over integer text.
    assert_eq!(
        root.get("tagged").unwrap().node,
        Node::Number(Number::Float(5.0))
    );
}

#[test]
fn json_string_escapes_and_surrogate_pairs() {
    let root = parse_json_ok(r#"{"s": "line\n\t\"q\"A😀"}"#);
    assert_eq!(
        root.get("s").unwrap().as_str(),
        Some("line\n\t\"q\"A\u{1F600}"),
    );
}

#[test]
fn json_malformed_error_span_points_at_error_line() {
    let mut diags = Diagnostics::default();
    // The value for `b` is missing; the error is detected on line 4, at the closing `}`.
    let text = "{\n  \"a\": 1,\n  \"b\":\n}\n";
    let outcome = parse_json(FileId(0), text, &mut diags);
    assert!(outcome.is_err());
    let diag = diags
        .items()
        .iter()
        .find(|diag| diag.code == Code::InvalidInput)
        .expect("invalid-input diagnostic");
    let span = diag.span.expect("error span");
    assert_eq!(span.start.line, 4, "error reported on line 4: {diag:?}");
}

fn parse_json_err_code(text: &str) -> Code {
    let mut diags = Diagnostics::default();
    assert!(
        parse_json(FileId(0), text, &mut diags).is_err(),
        "expected `{text}` to be rejected"
    );
    diags
        .items()
        .iter()
        .map(|diag| diag.code)
        .next()
        .unwrap_or_else(|| panic!("no diagnostic for `{text}`"))
}

#[test]
fn json_rejects_malformed_numbers_like_serde() {
    // Leading zeros, a fraction/exponent with no digit, and an overflow-to-infinity literal
    // are all malformed and must be rejected.
    for bad in [
        "01",
        "00",
        "-01",
        "1.",
        "1.e5",
        "1e",
        "1e+",
        "-",
        "1e999",
        "[01]",
        "{\"a\": 1.}",
    ] {
        assert_eq!(
            parse_json_err_code(bad),
            Code::InvalidInput,
            "`{bad}` should be rejected as malformed"
        );
    }
}

#[test]
fn json_still_accepts_valid_numbers() {
    // The tightened grammar must not reject well-formed numbers.
    let root = parse_json_ok("{\"a\": 0, \"b\": -0, \"c\": 0.5, \"d\": 1e3, \"e\": -12.5e-2}");
    assert_eq!(root.get("a").unwrap().node, Node::Number(Number::Int(0)));
    assert_eq!(root.get("b").unwrap().node, Node::Number(Number::Int(0)));
    assert_eq!(
        root.get("c").unwrap().node,
        Node::Number(Number::Float(0.5))
    );
    assert_eq!(
        root.get("d").unwrap().node,
        Node::Number(Number::Float(1000.0))
    );
    assert_eq!(
        root.get("e").unwrap().node,
        Node::Number(Number::Float(-0.125))
    );
}

/// The one diagnostic `diags` holds, which must be `E011`.
fn the_invalid_input(diags: &Diagnostics) -> &Diagnostic {
    let items = diags.items();
    assert_eq!(items.len(), 1, "{items:#?}");
    assert_eq!(items[0].code, Code::InvalidInput, "{items:#?}");
    &items[0]
}

#[test]
fn json_nesting_is_capped_at_the_maximum_depth() {
    // `n` nested arrays put the innermost one at depth `n - 1`, so the deepest accepted
    // nesting is one past the cap and the next is refused, at the array that crosses it.
    let nested = |n: usize| format!("{}{}", "[".repeat(n), "]".repeat(n));
    let deepest = MAX_NESTING_DEPTH as usize + 1;
    parse_json_ok(&nested(deepest));

    let mut diags = Diagnostics::default();
    assert!(parse_json(FileId(0), &nested(deepest + 1), &mut diags).is_err());
    let diag = the_invalid_input(&diags);
    assert!(
        diag.message.contains("JSON nesting is too deep"),
        "{diag:?}"
    );
    let span = diag.span.expect("depth diagnostic span");
    assert_eq!((span.start.line, span.start.col), (1, 258), "{diag:?}");
}

#[test]
fn yaml_nesting_is_capped_at_the_maximum_depth() {
    // `n` block sequences written `- - x` put the scalar at depth `n`, so the scalar at the
    // cap is accepted and the one past it is refused, at that scalar.
    let nested = |n: usize| format!("{}x\n", "- ".repeat(n));
    let deepest = MAX_NESTING_DEPTH as usize;
    parse_yaml_ok(&nested(deepest));

    let mut diags = Diagnostics::default();
    assert!(parse_yaml(FileId(0), &nested(deepest + 1), &mut diags).is_err());
    let diag = the_invalid_input(&diags);
    assert!(
        diag.message.contains("YAML nesting is too deep"),
        "{diag:?}"
    );
    assert_eq!(diag.pointer.as_str(), "/0".repeat(deepest + 1), "{diag:?}");
    let span = diag.span.expect("depth diagnostic span");
    assert_eq!((span.start.line, span.start.col), (1, 515), "{diag:?}");
}

#[test]
fn yaml_alias_expansion_past_the_node_budget_is_rejected() {
    // `[&a [0 ×198], *a ×n, 0 ×94]`: the outer sequence and the anchored one with its
    // scalars are 200 nodes, the copy kept for the aliases 199, and each alias 199 more. At
    // 299 aliases that charge is exactly the budget of 599 events; the 300th alias crosses it.
    let doc = |aliases: usize| {
        let run = |item: &str, n: usize| vec![item; n].join(", ");
        format!(
            "[&a [{}], {}, {}]",
            run("0", 198),
            run("*a", aliases),
            run("0", 94)
        )
    };
    let budget = |text: &str| yaml_events(text).unwrap().len() * ALIAS_EXPANSION_BUDGET_PER_EVENT;
    let fits = doc(299);
    assert_eq!(200 + 199 + 199 * 299, budget(&fits));
    parse_yaml_ok(&fits);

    let over = doc(300);
    let mut diags = Diagnostics::default();
    assert!(parse_yaml(FileId(0), &over, &mut diags).is_err());
    let diag = the_invalid_input(&diags);
    let past = format!(
        "the alias at line 1 column {} expands",
        over.rfind("*a").unwrap() + 1
    );
    assert!(diag.message.contains(&past), "{diag:?}");
    let stated = format!("past {} nodes", budget(&over));
    assert!(diag.message.contains(&stated), "{diag:?}");
    assert_eq!(diag.pointer.as_str(), "/300", "{diag:?}");
}

#[test]
fn yaml_with_more_than_one_document_is_rejected() {
    for text in ["a: 1\n---\nb: 2\n", "---\na: 1\n...\n---\na: 1\n"] {
        let mut diags = Diagnostics::default();
        assert!(parse_yaml(FileId(0), text, &mut diags).is_err(), "{text}");
        let diag = the_invalid_input(&diags);
        assert_eq!(
            diag.message, "YAML input must contain exactly one document",
            "{diag:?}"
        );
        assert_eq!(diag.pointer.as_str(), "", "{diag:?}");
    }
}

#[test]
fn a_yaml_mapping_key_that_is_not_a_string_is_rejected_at_the_key() {
    // `1` and `true` are plain scalars that resolve to a number and a boolean, and a flow
    // sequence is a key no JSON object can hold; each is refused at its own span, addressed
    // by the mapping that holds it (#923).
    for (text, pointer, line, col) in [
        ("a: 1\n1: one\n", "", 2, 1),
        ("a:\n  true: yes\n", "/a", 2, 3),
        ("a: 1\n? [x]\n: v\n", "", 2, 3),
        ("a:\n  - b: 1\n    1.5: x\n", "/a/0", 3, 5),
    ] {
        let mut diags = Diagnostics::default();
        assert!(parse_yaml(FileId(0), text, &mut diags).is_err(), "{text}");
        let diag = the_invalid_input(&diags);
        assert_eq!(diag.message, "YAML object keys must be strings", "{diag:?}");
        assert_eq!(diag.pointer.as_str(), pointer, "{text}: {diag:?}");
        let span = diag.span.expect("key span");
        assert_eq!(
            (span.start.line, span.start.col),
            (line, col),
            "{text}: {diag:?}"
        );
    }
}

#[test]
fn json_rejects_duplicate_object_key_at_the_second_occurrence() {
    let mut diags = Diagnostics::default();
    // Two `"type"` keys on line 2; the second (the duplicate) opening quote is at column 21.
    let text = "{\n  \"type\": \"string\", \"type\": \"integer\"\n}\n";
    assert!(parse_json(FileId(0), text, &mut diags).is_err());
    let diag = diags
        .items()
        .iter()
        .find(|diag| diag.code == Code::DuplicateObjectKey)
        .expect("duplicate-key diagnostic");
    let span = diag.span.expect("duplicate-key span");
    assert_eq!(span.start.line, 2, "{diag:?}");
    assert_eq!(span.start.col, 21, "{diag:?}");
    assert_eq!(diag.pointer.as_str(), "/type", "{diag:?}");
}

/// The duplicate key's pointer, or `None` when `diags` holds no `E022`.
fn duplicate_key_pointer(diags: &Diagnostics) -> Option<String> {
    diags
        .items()
        .iter()
        .find(|diag| diag.code == Code::DuplicateObjectKey)
        .map(|diag| diag.pointer.as_str().to_owned())
}

#[test]
fn duplicate_object_key_points_at_the_key_through_objects_and_arrays() {
    // The path runs through an object member, an array element, and a member whose name
    // needs RFC 6901 escaping, so each kind of segment and the escape are reached.
    let json = r#"{"a": {"x/y": [0, {"k": 1, "k": 2}]}}"#;
    let yaml = "a:\n  x/y:\n    - 0\n    - k: 1\n      k: 2\n";
    let mut diags = Diagnostics::default();
    assert!(parse_json(FileId(0), json, &mut diags).is_err());
    assert_eq!(
        duplicate_key_pointer(&diags).as_deref(),
        Some("/a/x~1y/1/k")
    );
    let mut diags = Diagnostics::default();
    assert!(parse_yaml(FileId(0), yaml, &mut diags).is_err());
    assert_eq!(
        duplicate_key_pointer(&diags).as_deref(),
        Some("/a/x~1y/1/k")
    );
}

#[test]
fn a_json_syntax_error_keeps_the_root_pointer() {
    // Only a duplicate key collects a path; a syntax error stays document-level.
    let mut diags = Diagnostics::default();
    assert!(parse_json(FileId(0), r#"{"a": [1, }"#, &mut diags).is_err());
    let diag = diags
        .items()
        .iter()
        .find(|diag| diag.code == Code::InvalidInput)
        .expect("syntax diagnostic");
    assert_eq!(diag.pointer.as_str(), "", "{diag:?}");
}

#[test]
fn a_duplicate_inside_a_yaml_complex_key_points_at_the_mapping_holding_the_key() {
    // A key has no pointer of its own, so the mapping that holds it addresses the duplicate.
    let mut diags = Diagnostics::default();
    let text = "a:\n  ? {k: 1, k: 2}\n  : v\n";
    assert!(parse_yaml(FileId(0), text, &mut diags).is_err());
    assert_eq!(duplicate_key_pointer(&diags).as_deref(), Some("/a"));
}

#[test]
fn a_non_finite_yaml_float_points_at_its_element_or_at_the_mapping_holding_its_key() {
    // An array element is addressed by its index; a key has no pointer of its own, so a
    // non-finite float used as a key is addressed by the mapping that holds it, and is
    // refused for its value before the key's type is ever looked at.
    for (text, pointer, message) in [
        (
            "a:\n  enum: [1, .inf]\n",
            "/a/enum/1",
            "malformed YAML: number `.inf` is infinite, which JSON cannot represent \
             at line 2 column 13",
        ),
        (
            "a:\n  - 0\n  - -1e999\n",
            "/a/1",
            "malformed YAML: number `-1e999` is out of range at line 3 column 5",
        ),
        (
            "a:\n  .nan: x\n",
            "/a",
            "malformed YAML: number `.nan` is NaN, which JSON cannot represent \
             at line 2 column 3",
        ),
        (
            "1e999: x\n",
            "",
            "malformed YAML: number `1e999` is out of range at line 1 column 1",
        ),
    ] {
        let mut diags = Diagnostics::default();
        assert!(parse_yaml(FileId(0), text, &mut diags).is_err(), "{text}");
        let diag = the_invalid_input(&diags);
        assert_eq!(diag.pointer.as_str(), pointer, "{text}: {diag:?}");
        assert_eq!(diag.message, message, "{text}: {diag:?}");
    }
}

#[test]
fn yaml_rejects_duplicate_mapping_key() {
    let mut diags = Diagnostics::default();
    // `a` appears twice; the duplicate is on line 2.
    let text = "a: 1\na: 2\n";
    assert!(parse_yaml(FileId(0), text, &mut diags).is_err());
    let diag = diags
        .items()
        .iter()
        .find(|diag| diag.code == Code::DuplicateObjectKey)
        .expect("duplicate-key diagnostic");
    assert_eq!(diag.span.expect("span").start.line, 2, "{diag:?}");
    assert_eq!(diag.pointer.as_str(), "/a", "{diag:?}");
}

#[test]
fn yaml_nested_node_span_is_precise() {
    // `c` is three levels deep on line 4 (1-based; line 1 is the leading newline).
    let text = "\na:\n  b:\n    c: hello\n";
    let root = parse_yaml_ok(text);
    let c = root
        .get("a")
        .and_then(|value| value.get("b"))
        .and_then(|value| value.get("c"))
        .expect("nested `c` exists");
    assert_eq!(c.node, Node::String("hello".to_owned()));
    assert_eq!(c.span().start.line, 4, "value line");
    // `    c: hello` — the value `hello` starts at column 8.
    assert_eq!(c.span().start.col, 8, "value column");

    // The key `c` is at column 5 on the same line.
    let b = root.get("a").and_then(|value| value.get("b")).unwrap();
    let (key, _) = b.as_object().unwrap().iter().next().unwrap();
    assert_eq!(key.name, "c");
    assert_eq!(key.span.start.line, 4);
    assert_eq!(key.span.start.col, 5);
}

#[test]
fn yaml_alias_is_expanded_like_yamlloader() {
    // Aliases expand into concrete values, as `YamlLoader` expands them: the alias `*anchor`
    // resolves to a clone of the anchored `1` rather than being rejected.
    let root = parse_yaml_ok("a: &anchor 1\nb: *anchor\n");
    assert_eq!(root.get("a").unwrap().node, Node::Number(Number::Int(1)));
    assert_eq!(root.get("b").unwrap().node, Node::Number(Number::Int(1)));
}

#[test]
fn the_e011_explain_text_states_the_alias_expansion_budget() {
    // `spargen explain E011` restates the budget as a literal, so it follows the constant.
    let stated = format!("{ALIAS_EXPANSION_BUDGET_PER_EVENT} nodes per parse event");
    assert!(Code::InvalidInput.explain().contains(&stated), "{stated}");
}

#[test]
fn the_e011_explain_text_states_the_nesting_cap() {
    // `spargen explain E011` restates the cap as a literal, so it follows the constant.
    let stated = format!("nested more than {MAX_NESTING_DEPTH} levels deep");
    assert!(Code::InvalidInput.explain().contains(&stated), "{stated}");
}

#[test]
fn yaml_multibyte_column_counts_scalar_values() {
    // A multibyte key precedes the value on the same line; the value column must count
    // characters, not bytes.
    let text = "\"café\": tail\n";
    let root = parse_yaml_ok(text);
    let value = root.get("café").expect("multibyte key present");
    assert_eq!(value.as_str(), Some("tail"));
    // `"café": tail` — 7 characters (`"`,`c`,`a`,`f`,`é`,`"`,`:`) then a space, so `tail`
    // starts at column 9.
    assert_eq!(value.span().start.col, 9);
}
