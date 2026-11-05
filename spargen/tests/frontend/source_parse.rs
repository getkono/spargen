//! What the source parsers make of a document before lowering reads it: a duplicate key, a YAML
//! mapping key that is no string, a non-finite YAML float, the parser an extensionless document
//! falls back to, and the YAML alias expansion budget and nesting cap.

use super::*;

#[test]
fn e022_duplicate_object_key_is_rejected() {
    // A mapping that declares the same key twice used to be silently collapsed (JSON: last-wins;
    // YAML: `YamlLoader` errored) — now it is uniformly rejected with a stable code and a precise
    // span at the second (duplicate) occurrence, so a duplicated `type`/`properties` name cannot
    // silently reach lowering. Both parsers report it at the duplicate key itself (#533), through
    // `check` and `generate` alike, so the location oracle finds nothing to report.
    let yaml = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Foo:
      type: object
      type: string
"##;
    let json = r#"{"openapi": "3.1.0", "info": {"title": "T", "version": "1.0.0"}, "paths": {}, "components": {"schemas": {"Foo": {"type": "object", "type": "string"}}}}"#;
    // The parser is chosen by extension, and the shared helpers write `openapi.yaml`, so the JSON
    // document is written as `openapi.json` here to reach the JSON parser. The duplicate `type`
    // sits on line 9 of the YAML document and on the JSON document's only line, not on line 1 of
    // a whole-file span.
    for (name, spec, line) in [("openapi.yaml", yaml, 9), ("openapi.json", json, 1)] {
        let temp = tempfile::tempdir().unwrap();
        let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
        std::fs::write(dir.join(name), spec).unwrap();
        let generated = run_generate(&build(dir.join(name), dir.join("client.rs")));
        let checked = run_check(&Spec::new(dir.join(name)));
        for report in [checked, generated] {
            assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
            let duplicate = report
                .diagnostics()
                .iter()
                .find(|d| d.code == Code::DuplicateObjectKey)
                .expect("duplicate-key diagnostic");
            assert_eq!(
                duplicate.pointer.as_str(),
                "/components/schemas/Foo/type",
                "{duplicate:#?}"
            );
            assert_eq!(
                duplicate.span.expect("span").start.line,
                line,
                "{duplicate:#?}"
            );
            let violations = oracles::location_violations(report.diagnostics(), spec.as_bytes());
            assert!(violations.is_empty(), "{violations:?}");
        }
    }
}

#[test]
fn e011_a_yaml_mapping_key_that_is_not_a_string_is_addressed_by_its_mapping() {
    // Issue #923: a mapping key that resolves to no string (`1`, `true`, a flow sequence) has no
    // pointer segment of its own, and used to be reported with the empty pointer wherever it sat.
    // It is addressed by the mapping that holds it, in the root document and in a referenced
    // file alike, and spanned at the key itself.
    let root = |schema: &str| {
        format!(
            "openapi: 3.1.0\ninfo: {{ title: T, version: 1.0.0 }}\npaths: {{}}\ncomponents:\n  \
             schemas:\n    A: {schema}\n"
        )
    };
    let properties = root("{ type: object, properties: { 1: { type: string } } }");
    let tagged = "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\npaths: {}\ntags:\n  - name: \
                  t\n    true: yes\n";
    // The root mapping itself holds the key, so the root document is the mapping that holds it.
    let top_level = "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\npaths: {}\n1: one\n";
    let referencing = root("{ $ref: './a.yaml' }");
    let referenced = "type: object\nproperties:\n  ? [x]\n  : { type: string }\n";
    // The files, the expected pointer, whether the key lies in the root document, and its line
    // and column.
    type Case<'a> = (&'a [(&'a str, &'a str)], &'a str, bool, (u32, u32));
    let cases: [Case; 4] = [
        (
            &[("openapi.yaml", &properties)],
            "/components/schemas/A/properties",
            true,
            (6, 38),
        ),
        (&[("openapi.yaml", tagged)], "/tags/0", true, (6, 5)),
        (&[("openapi.yaml", top_level)], "", true, (4, 1)),
        (
            &[("openapi.yaml", &referencing), ("a.yaml", referenced)],
            "/properties",
            false,
            (3, 5),
        ),
    ];
    for (files, pointer, in_root, (line, col)) in cases {
        let (generated, checked, _) = generate_and_check_files(files);
        for report in [&generated, &checked] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "{files:?}: {report:#?}"
            );
            let diag = report
                .diagnostics()
                .iter()
                .find(|d| d.message == "YAML object keys must be strings")
                .unwrap_or_else(|| panic!("{files:?}: no non-string-key E011 in {report:#?}"));
            assert_eq!(diag.code, Code::InvalidInput, "{diag:#?}");
            assert_eq!(diag.pointer.as_str(), pointer, "{files:?}: {diag:#?}");
            let span = diag.span.expect("key span");
            assert_eq!(span.file.get() == 0, in_root, "{files:?}: {diag:#?}");
            assert_eq!(
                (span.start.line, span.start.col),
                (line, col),
                "{files:?}: {diag:#?}"
            );
        }
    }
}

/// A document whose one schema's `maximum` is the YAML scalar `value`, on line 8.
fn maximum_document(value: &str) -> String {
    format!(
        "openapi: 3.1.0\ninfo: {{ title: T, version: 1.0.0 }}\npaths: {{}}\ncomponents:\n  \
         schemas:\n    N:\n      type: number\n      maximum: {value}\n"
    )
}

#[test]
fn e011_a_non_finite_yaml_float_is_rejected_as_the_json_parser_rejects_it() {
    // Issue #707: YAML's special floats used to fail Rust's `f64` parse and fall back to `0.0`, so
    // `maximum: .inf` silently read as a maximum of 0, and an overflowing `1e999` read as infinity
    // where the JSON parser rejects it. OpenAPI's JSON data model has neither infinities nor NaN,
    // so every non-finite value is `E011`, at the member that holds it, in both entry points.
    for value in [
        ".inf",
        ".Inf",
        "+.INF",
        "-.inf",
        ".nan",
        ".NaN",
        "1e999",
        "-1e999",
        "!!float .inf",
        "!!float 1e999",
    ] {
        let spec = maximum_document(value);
        let (generated, checked, _) = generate_and_check_files(&[("openapi.yaml", &spec)]);
        for report in [&generated, &checked] {
            assert_eq!(report.outcome(), Outcome::Rejected, "{value}: {report:#?}");
            let literal = value.trim_start_matches("!!float ");
            let diag = report
                .diagnostics()
                .iter()
                .find(|d| d.code == Code::InvalidInput)
                .unwrap_or_else(|| panic!("{value}: no E011 in {report:#?}"));
            assert_eq!(
                diag.pointer.as_str(),
                "/components/schemas/N/maximum",
                "{value}: {diag:#?}"
            );
            assert!(
                diag.message.contains(&format!("`{literal}`")),
                "{value}: {diag:#?}"
            );
            assert_eq!(diag.span.expect("span").start.line, 8, "{value}: {diag:#?}");
        }
    }

    // The JSON parser's verdict on the overflowing literal, which the YAML one now shares.
    let json = r#"{"openapi": "3.1.0", "info": {"title": "T", "version": "1.0.0"}, "paths": {},
        "components": {"schemas": {"N": {"type": "number", "maximum": 1e999}}}}"#;
    let (generated, checked, _) = generate_and_check_files(&[("openapi.json", json)]);
    for report in [&generated, &checked] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(report, Code::InvalidInput), "{report:#?}");
    }

    // A finite float in the same position, tagged or plain, up to the edge of `f64`, still passes.
    for value in ["1.5e3", "-0.5", "!!float 5", "1e308"] {
        let spec = maximum_document(value);
        let (generated, checked, _) = generate_and_check_files(&[("openapi.yaml", &spec)]);
        assert_eq!(
            generated.outcome(),
            Outcome::Generated,
            "{value}: {generated:#?}"
        );
        assert_eq!(checked.outcome(), Outcome::Clean, "{value}: {checked:#?}");
    }
}

/// A file name with no `.json`/`.yaml`/`.yml` extension is parsed as YAML, then as JSON (#717).
/// The YAML attempt's diagnostics are discarded when the JSON parse succeeds, so JSON spelling a
/// character as a UTF-16 surrogate pair, which YAML refuses (its `\u` escape names one code
/// point), generates cleanly under `spec.txt`.
#[test]
fn e011_an_extensionless_json_document_yaml_refuses_falls_back_to_json() {
    let json = r#"{"openapi": "3.1.0", "info": {"title": "T\ud83d\ude00", "version": "1.0.0"}, "paths": {}}"#;
    // The premise: the same text under a YAML name is refused, so the fallback is what accepts it.
    let (yaml_named, _, _) = generate_and_check_files(&[("openapi.yaml", json)]);
    assert_eq!(yaml_named.outcome(), Outcome::Rejected, "{yaml_named:#?}");
    let (generated, checked, _) = generate_and_check_files(&[("spec.txt", json)]);
    assert_eq!(generated.outcome(), Outcome::Generated, "{generated:#?}");
    assert_eq!(checked.outcome(), Outcome::Clean, "{checked:#?}");
}

/// An extensionless document neither parser accepts is reported once, with the YAML attempt's
/// `E011`, rather than once per attempt (#717).
#[test]
fn e011_an_extensionless_document_neither_parser_accepts_reports_one_e011() {
    let (generated, checked, _) =
        generate_and_check_files(&[("spec.txt", "{\n\t\"openapi\": [\n")]);
    for report in [&generated, &checked] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        let messages = messages_for(report, Code::InvalidInput);
        assert_eq!(messages.len(), 1, "{report:#?}");
        assert!(messages[0].starts_with("malformed YAML"), "{report:#?}");
    }
}

#[test]
fn e011_yaml_aliases_that_expand_past_the_node_budget_are_rejected_in_the_levels() {
    // Issue #708: every alias expanded to a deep clone of its anchored node with no budget, so
    // ten levels of anchors, each aliasing the level below ten times ("billion laughs"), grew a
    // document of a few hundred bytes into 10^10 nodes and exhausted memory before any later
    // stage ran. The expansion is now budgeted, and the copy that crosses it is `E011`: here the
    // copy of a level kept for the aliases of the next, reported at that level.
    let mut levels = String::from("  l0: &l0 [x, x, x, x, x, x, x, x, x, x]\n");
    for level in 1..10 {
        let previous = level - 1;
        let aliases = vec![format!("*l{previous}"); 10].join(", ");
        levels.push_str(&format!("  l{level}: &l{level} [{aliases}]\n"));
    }
    let spec = format!(
        "openapi: 3.1.0\ninfo: {{ title: T, version: 1.0.0 }}\npaths: {{}}\nx-laughs:\n{levels}"
    );
    let (generated, checked, _) = generate_and_check_files(&[("openapi.yaml", &spec)]);
    for report in [&generated, &checked] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        let diag = report
            .diagnostics()
            .iter()
            .find(|d| d.code == Code::InvalidInput)
            .unwrap_or_else(|| panic!("no E011 in {report:#?}"));
        assert!(
            diag.message.contains("the copy kept for aliases"),
            "{diag:#?}"
        );
        let level = diag.pointer.as_str().strip_prefix("/x-laughs/l");
        assert!(
            level.is_some_and(|level| level.parse::<u32>().is_ok()),
            "{diag:#?}"
        );
    }

    // Aliases that repeat a subtree a bounded number of times still expand and generate.
    let spec = format!(
        "openapi: 3.1.0\ninfo: {{ title: T, version: 1.0.0 }}\npaths: {{}}\nx-reused:\n  \
         base: &base [x, x, x, x, x, x, x, x, x, x]\n  copies: [{}]\n",
        vec!["*base"; 50].join(", ")
    );
    let (generated, checked, _) = generate_and_check_files(&[("openapi.yaml", &spec)]);
    assert_eq!(generated.outcome(), Outcome::Generated, "{generated:#?}");
    assert_eq!(checked.outcome(), Outcome::Clean, "{checked:#?}");
}

/// Four levels of anchors under `x-laughs`, each aliasing the level below ten times, so `*l3`
/// expands to 11,111 nodes from about 50 parser events.
fn yaml_laughs_to_level_3() -> String {
    let mut levels = String::from("x-laughs:\n  l0: &l0 [x, x, x, x, x, x, x, x, x, x]\n");
    for level in 1..4 {
        let aliases = vec![format!("*l{}", level - 1); 10].join(", ");
        levels.push_str(&format!("  l{level}: &l{level} [{aliases}]\n"));
    }
    levels
}

/// 250 nested anchored lists `&n1 [&n2 [... &n250 [...]]]` under `x-nest`, the innermost holding
/// 90 `*l3` aliases and 10,000 padding scalars: about a million nodes, inside the budget, each
/// level of which kept a full copy of everything beneath it in the anchor table.
fn yaml_nested_anchors(depth: usize) -> String {
    let mut nest = String::from("x-nest: ");
    for level in 1..=depth {
        nest.push_str(&format!("&n{level} ["));
    }
    nest.push_str(&vec!["*l3"; 90].join(", "));
    nest.push_str(", ");
    nest.push_str(&vec!["x"; 10_000].join(", "));
    nest.push_str(&"]".repeat(depth));
    nest.push('\n');
    nest
}

#[test]
fn e011_nested_yaml_anchors_nothing_aliases_keep_no_copy_and_generate() {
    // Issue #708 review: each completed anchored node was copied into the anchor table without
    // being charged against the budget, so 250 nested anchors around a million-node core kept
    // 250 copies of it (hundreds of millions of nodes) and ran out of memory, although the tree
    // itself is within the budget. Nothing aliases `n1`..`n250`, so no copy is needed at all.
    let spec = format!(
        "openapi: 3.1.0\ninfo: {{ title: T, version: 1.0.0 }}\npaths: {{}}\n{}{}",
        yaml_laughs_to_level_3(),
        yaml_nested_anchors(250)
    );
    let (generated, checked, _) = generate_and_check_files(&[("openapi.yaml", &spec)]);
    assert_eq!(generated.outcome(), Outcome::Generated, "{generated:#?}");
    assert_eq!(checked.outcome(), Outcome::Clean, "{checked:#?}");
}

#[test]
fn e011_nested_yaml_anchors_whose_kept_copies_cross_the_node_budget_are_rejected() {
    // The same document with every nested anchor aliased once afterwards: each anchor's copy is
    // kept for its alias, and those copies are charged against the budget as they are taken, so
    // the anchor whose copy crosses it is `E011` before the table outgrows the budget.
    let refs: Vec<String> = (1..=250).map(|level| format!("*n{level}")).collect();
    let spec = format!(
        "openapi: 3.1.0\ninfo: {{ title: T, version: 1.0.0 }}\npaths: {{}}\n{}{}x-refs: [{}]\n",
        yaml_laughs_to_level_3(),
        yaml_nested_anchors(250),
        refs.join(", ")
    );
    let (generated, checked, _) = generate_and_check_files(&[("openapi.yaml", &spec)]);
    for report in [&generated, &checked] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        let diag = report
            .diagnostics()
            .iter()
            .find(|d| d.code == Code::InvalidInput)
            .unwrap_or_else(|| panic!("no E011 in {report:#?}"));
        assert!(diag.message.contains("anchored node"), "{diag:#?}");
        // The innermost anchor completes first and is the one whose copy crosses the budget.
        assert_eq!(
            diag.pointer.as_str(),
            format!("/x-nest{}", "/0".repeat(249)),
            "{diag:#?}"
        );
    }
}

#[test]
fn e011_a_yaml_alias_key_past_the_node_budget_is_reported_at_its_mapping() {
    // A mapping key has no pointer of its own, so the alias key whose copy crosses the budget is
    // reported at the mapping that holds it, not at a member path.
    let fill = ["*l2"; 4].join(", ");
    let spec = format!(
        "openapi: 3.1.0\ninfo: {{ title: T, version: 1.0.0 }}\npaths: {{}}\n\
         x-laughs:\n  l0: &l0 [x, x, x, x, x, x, x, x, x, x]\n  l1: &l1 [{}]\n  l2: &l2 [{}]\n\
         x-fill: [{fill}]\nx-key: {{ *l2 : 1 }}\n",
        ["*l0"; 10].join(", "),
        ["*l1"; 10].join(", "),
    );
    let (generated, checked, _) = generate_and_check_files(&[("openapi.yaml", &spec)]);
    for report in [&generated, &checked] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        let diag = report
            .diagnostics()
            .iter()
            .find(|d| d.code == Code::InvalidInput)
            .unwrap_or_else(|| panic!("no E011 in {report:#?}"));
        assert!(diag.message.contains("the alias at"), "{diag:#?}");
        assert_eq!(diag.pointer.as_str(), "/x-key", "{diag:#?}");
        // The span starts on the `*` that spells the alias.
        let start = diag.span.expect("span").start;
        let line = spec.lines().nth(start.line as usize - 1).expect("line");
        assert_eq!(
            line.chars().nth(start.col as usize - 1),
            Some('*'),
            "{line}: {diag:#?}"
        );
    }
}

/// A document whose `x-deep` holds `depth` nested block sequences around the scalar `x`, written
/// compactly on one line (`- - - x`), so the scalar sits at depth `depth + 1`.
fn yaml_block_sequences(depth: usize) -> String {
    format!(
        "openapi: 3.1.0\ninfo: {{ title: T, version: 1.0.0 }}\npaths: {{}}\nx-deep:\n  {}x\n",
        "- ".repeat(depth)
    )
}

#[test]
fn e011_yaml_nesting_past_the_depth_cap_is_rejected_at_the_node_that_crosses_it() {
    // Issue #706: the JSON parser capped nesting at 256 levels, but the YAML builder recursed once
    // per block sequence or mapping with no cap, and block-style nesting is not bounded by the
    // event parser, so this million-level document overflowed even the frontend thread's 64 MiB
    // stack and aborted the process. It is now `E011`, at the first node past the cap.
    let spec = yaml_block_sequences(1_000_000);
    let (generated, checked, _) = generate_and_check_files(&[("openapi.yaml", &spec)]);
    for report in [&generated, &checked] {
        assert_eq!(
            report.outcome(),
            Outcome::Rejected,
            "{:?}",
            report.outcome()
        );
        let diag = report
            .diagnostics()
            .iter()
            .find(|d| d.code == Code::InvalidInput)
            .unwrap_or_else(|| panic!("no E011 among {} diagnostics", report.diagnostics().len()));
        assert!(
            diag.message.contains("YAML nesting is too deep"),
            "{}",
            diag.message
        );
        // `x-deep` is the first sequence, at depth 1, so the 257th, at depth 257, is the first
        // node past the cap, and it is item 0 of each of the 256 sequences that hold it.
        assert_eq!(
            diag.pointer.as_str(),
            format!("/x-deep{}", "/0".repeat(256)),
            "{}",
            diag.message
        );
        let start = diag.span.expect("span").start;
        assert_eq!(
            (start.line, start.col),
            (5, 3 + 2 * 256),
            "{}",
            diag.message
        );
    }

    // The deepest nesting the cap admits, a scalar at depth 256, still generates.
    let spec = yaml_block_sequences(255);
    let (generated, checked, _) = generate_and_check_files(&[("openapi.yaml", &spec)]);
    assert_eq!(generated.outcome(), Outcome::Generated, "{generated:#?}");
    assert_eq!(checked.outcome(), Outcome::Clean, "{checked:#?}");
}

/// A document whose extensions `x-a`, `x-b`, and `x-c` each hold flow lists nested 100, 100, and
/// `c_depth` levels deep, around `x`, `*a`, and `*b`: each anchor holds the alias of the one
/// before, so the copies stack, and `*b` reaches 200 levels below where it is written.
fn yaml_stacked_aliases(c_depth: usize) -> String {
    let nested = |n: usize, inner: &str| format!("{}{inner}{}", "[".repeat(n), "]".repeat(n));
    format!(
        "openapi: 3.1.0\ninfo: {{ title: T, version: 1.0.0 }}\npaths: {{}}\n\
         x-a: &a {}\nx-b: &b {}\nx-c: {}\n",
        nested(100, "x"),
        nested(100, "*a"),
        nested(c_depth, "*b"),
    )
}

#[test]
fn e011_a_yaml_alias_whose_copy_reaches_past_the_nesting_cap_is_rejected_at_the_alias() {
    // Issue #706: no node of the source text sits deeper than 101, but `*b` written 101 levels
    // down expands to a copy reaching 200 levels below it, past the 256-level cap, and later
    // stages recurse over the expanded tree, so the alias is refused where it is written.
    let spec = yaml_stacked_aliases(100);
    let (generated, checked, _) = generate_and_check_files(&[("openapi.yaml", &spec)]);
    for report in [&generated, &checked] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        let diag = report
            .diagnostics()
            .iter()
            .find(|d| d.code == Code::InvalidInput)
            .unwrap_or_else(|| panic!("no E011 in {report:#?}"));
        assert!(
            diag.message.contains("YAML nesting is too deep"),
            "{diag:#?}"
        );
        assert_eq!(
            diag.pointer.as_str(),
            format!("/x-c{}", "/0".repeat(100)),
            "{diag:#?}"
        );
        let start = diag.span.expect("span").start;
        let line = spec.lines().nth(start.line as usize - 1).expect("line");
        assert_eq!(
            line.chars().nth(start.col as usize - 1),
            Some('*'),
            "{line}"
        );
    }

    // The same alias written 50 levels down reaches depth 251, within the cap, and expands.
    let spec = yaml_stacked_aliases(50);
    let (generated, checked, _) = generate_and_check_files(&[("openapi.yaml", &spec)]);
    assert_eq!(generated.outcome(), Outcome::Generated, "{generated:#?}");
    assert_eq!(checked.outcome(), Outcome::Clean, "{checked:#?}");
}
