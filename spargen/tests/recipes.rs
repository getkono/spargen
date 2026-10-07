//! Framework round-trip recipes: proves spargen turns the OpenAPI document EMITTED BY a
//! Rust server framework (utoipa, aide, poem-openapi) into a client, i.e. the round-trip
//! `Rust server → OpenAPI → spargen client`.
//!
//! Each case reads a vendored spec under `corpus/recipes/` that mirrors that framework's OUTPUT
//! IDIOMS (see `corpus/recipes/README.md` for provenance and the verified version constants) and
//! asserts spargen's outcome and, for each idiom a generating case lists, the generated type it
//! lowers to, so the recipes in `docs/recipes.md` stay honest:
//!
//! * `utoipa` (emits OpenAPI 3.1.0) — generates cleanly; exercises `type: [T, null]` nullables,
//!   nullable `$ref` via a `oneOf` `null` member, `allOf`-composed models, and a `discriminator`ed
//!   union across tag-grouped, multi-status operations.
//! * `aide` (emits OpenAPI 3.1.0, schemars schemas) — generates with only validation-only warnings
//!   (`W001`); exercises `anyOf`/`type`-array nullables, an externally-tagged (content-dispatched)
//!   union, a by-JSON-type disjoint union, and an `allOf` flatten.
//! * `poem-openapi` (emits OpenAPI 3.0.0) — REJECTED with `E001`, proving the 3.1.x requirement and
//!   motivating the "upgrade to 3.1" step of its recipe.
//! * a utoipa `#[serde(untagged)]` numeric union with overlapping `integer | number` branches —
//!   generates as a typed trial-matching enum instead of requiring a compatibility carve.
//!
//! Everything is deterministic and offline: the specs are vendored into the repo and the test only
//! reads local files (no network).

use camino::Utf8PathBuf;
use spargen::{CargoIntegration, Code, Outcome, Report, Spec};

/// Absolute path to a vendored recipe spec (workspace root is one level up from this crate).
fn recipe_path(name: &str) -> Utf8PathBuf {
    Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("corpus")
        .join("recipes")
        .join(name)
}

/// Run `check` on a vendored recipe spec.
fn check(name: &str) -> Report {
    spargen::check(&Spec::new(recipe_path(name)))
}

/// Run `generate` on a vendored recipe spec (optionally with `--carve`), returning the report and
/// the emitted module text (when generation ran). Output goes to a throwaway tempdir.
fn generate(name: &str, carve: bool) -> (Report, Option<String>) {
    let temp = tempfile::tempdir().unwrap();
    let out = Utf8PathBuf::from_path_buf(temp.path().join("client.rs")).unwrap();
    let build = Spec::new(recipe_path(name))
        .carve(carve)
        .build(out.clone())
        .cargo(CargoIntegration::Off);
    let report = spargen::generate(&build);
    let text = std::fs::read_to_string(out).ok();
    (report, text)
}

fn has_code(report: &Report, code: Code) -> bool {
    report.diagnostics().iter().any(|d| d.code == code)
}

/// One field of `pub struct ty`: its declared type and the attribute text written above it.
/// The attributes are what tell an optional field apart from a nullable one: both are
/// `Option<T>`, but an optional non-nullable field decodes a present value through
/// `decode_present` (so a `null` is refused), and a nullable one reads `null` as `None`.
struct Field {
    ty: String,
    attrs: String,
}

impl Field {
    fn is_nullable_option(&self) -> bool {
        self.ty.starts_with("Option<") && !self.attrs.contains("decode_present")
    }
}

/// The fields `pub struct ty` declares, by name, in source order.
fn struct_fields(text: &str, ty: &str) -> Vec<(String, Field)> {
    let head = format!("pub struct {ty} {{");
    let mut lines = text.lines().map(str::trim).skip_while(|line| *line != head);
    lines.next();
    let mut fields = Vec::new();
    let mut attrs = String::new();
    for line in lines.take_while(|line| !line.starts_with('}')) {
        match line
            .strip_prefix("pub ")
            .and_then(|rest| rest.split_once(':'))
        {
            Some((name, field_ty)) => fields.push((
                name.to_owned(),
                Field {
                    ty: field_ty.trim().trim_end_matches(',').to_owned(),
                    attrs: std::mem::take(&mut attrs),
                },
            )),
            None => attrs.push_str(line),
        }
    }
    fields
}

/// The field `field` of `pub struct ty`, failing the test when it is not declared.
fn field(text: &str, ty: &str, field: &str) -> Field {
    struct_fields(text, ty)
        .into_iter()
        .find(|(name, _)| name == field)
        .map(|(_, declared)| declared)
        .unwrap_or_else(|| panic!("`{ty}` declares no `{field}` field"))
}

fn field_names(text: &str, ty: &str) -> Vec<String> {
    struct_fields(text, ty)
        .into_iter()
        .map(|(name, _)| name)
        .collect()
}

/// The right-hand side of `pub type ty = …;`.
fn alias_target(text: &str, ty: &str) -> Option<String> {
    let head = format!("pub type {ty} = ");
    text.lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix(&head))
        .map(|rest| rest.trim_end_matches(';').to_owned())
}

/// The variant declarations of `pub enum ty`, attributes skipped.
fn enum_variants(text: &str, ty: &str) -> Vec<String> {
    let head = format!("pub enum {ty} {{");
    let mut lines = text.lines().map(str::trim).skip_while(|line| *line != head);
    lines.next();
    lines
        .take_while(|line| !line.starts_with('}'))
        .filter(|line| !line.is_empty() && !line.starts_with("#[") && !line.starts_with("///"))
        .map(|line| line.trim_end_matches(',').to_owned())
        .collect()
}

fn error_codes(report: &Report) -> Vec<&'static str> {
    report
        .diagnostics()
        .iter()
        .filter(|d| d.code.as_str().starts_with('E'))
        .map(|d| d.code.as_str())
        .collect()
}

// --- utoipa: emits OpenAPI 3.1.0, generates cleanly ---------------------------------------------

#[test]
fn utoipa_document_generates_cleanly() {
    // `check` and `generate` must agree (the recipe tells users `check` is a safe pre-flight).
    let checked = check("utoipa.json");
    assert_eq!(
        checked.outcome(),
        Outcome::Clean,
        "{:?}",
        checked.diagnostics()
    );
    assert!(
        error_codes(&checked).is_empty(),
        "no errors expected: {:?}",
        checked.diagnostics()
    );

    let (report, text) = generate("utoipa.json", false);
    assert_eq!(
        report.outcome(),
        Outcome::Generated,
        "{:?}",
        report.diagnostics()
    );
    assert!(
        error_codes(&report).is_empty(),
        "no errors expected: {:?}",
        report.diagnostics()
    );
    let text = text.expect("utoipa generation wrote a module");
    // The idiomatic operations (tag-grouped, multi-status, union return) all lower to methods.
    for op in [
        "fn list_pets",
        "fn create_pet",
        "fn get_pet",
        "fn latest_event",
    ] {
        assert!(text.contains(op), "missing operation {op}");
    }

    // `type: [string, null]`: a nullable field, which reads `null` as `None`, of the plain type.
    let tag = field(&text, "Pet", "tag");
    assert!(
        tag.is_nullable_option(),
        "`tag` is not nullable: {}",
        tag.ty
    );
    assert_eq!(alias_target(&text, "Pettag").as_deref(), Some("String"));
    // Nullable `$ref` through a `oneOf` `null` member: nullable, of the referenced object.
    let category = field(&text, "Pet", "category");
    assert!(
        category.is_nullable_option(),
        "`category` is not nullable: {}",
        category.ty
    );
    assert_eq!(field_names(&text, "Petcategory"), ["id", "name"]);
    // The control: an optional, non-nullable field refuses a present `null`.
    assert!(
        !field(&text, "Pet", "status").is_nullable_option(),
        "an optional non-nullable field reads null as absence"
    );
    // `allOf: [$ref Pet, {created_at}]`: one struct with `Pet`'s fields and the inline member's.
    assert_eq!(
        field_names(&text, "PetWithMeta"),
        ["id", "name", "tag", "category", "status", "created_at"]
    );
    assert_eq!(
        field(&text, "PetWithMeta", "created_at").ty,
        "PetWithMetaMember1createdAt"
    );
    // The `discriminator`ed union: one variant per mapped member, dispatched on `event_type` by
    // the mapping's values.
    assert_eq!(
        enum_variants(&text, "Event"),
        ["PetCreated(Box<PetCreated>)", "PetSold(Box<PetSold>)"]
    );
    for dispatch in [
        ".get(\"event_type\")",
        "\"created\" | \"PetCreated\" =>",
        "\"sold\" | \"PetSold\" =>",
    ] {
        assert!(
            text.contains(dispatch),
            "`Event` does not dispatch with `{dispatch}`"
        );
    }
}

// --- aide: emits OpenAPI 3.1.0 (schemars), generates with only validation-only warnings ----------

#[test]
fn aide_document_generates_with_only_validation_warnings() {
    let (report, text) = generate("aide.json", false);
    assert_eq!(
        report.outcome(),
        Outcome::Generated,
        "{:?}",
        report.diagnostics()
    );
    assert!(
        error_codes(&report).is_empty(),
        "no errors expected: {:?}",
        report.diagnostics()
    );
    // schemars emits `minimum`/`format` validation hints that spargen faithfully ignores (W001);
    // any other diagnostic class would be a surprise the recipe should mention.
    assert!(
        report
            .diagnostics()
            .iter()
            .all(|d| d.code == Code::ValidationKeywordIgnored),
        "only validation-only warnings expected: {:?}",
        report.diagnostics()
    );
    let text = text.expect("aide generation wrote a module");
    for op in [
        "fn list_items",
        "fn get_item",
        "fn add_shape",
        "fn get_scalar",
    ] {
        assert!(text.contains(op), "missing operation {op}");
    }

    // `anyOf: [string, null]` and `type: [integer, null]`: both nullable, of the plain type.
    for (name, alias, target) in [
        ("label", "Itemlabel", "String"),
        ("count", "Itemcount", "i32"),
    ] {
        let declared = field(&text, "Item", name);
        assert!(
            declared.is_nullable_option(),
            "`{name}` is not nullable: {}",
            declared.ty
        );
        assert_eq!(declared.ty, format!("Option<{alias}>"));
        assert_eq!(alias_target(&text, alias).as_deref(), Some(target));
    }
    // The `allOf` flatten: `Item`'s fields plus the inline member's `note`.
    assert_eq!(
        field_names(&text, "DetailedItem"),
        ["id", "label", "count", "note"]
    );
    // The externally tagged union: one variant per single-key object, each wrapping its payload.
    assert_eq!(
        enum_variants(&text, "Shape"),
        [
            "ShapeVariant0(Box<ShapeVariant0>)",
            "ShapeVariant1(Box<ShapeVariant1>)"
        ]
    );
    assert_eq!(field(&text, "ShapeVariant0", "circle").ty, "Circle");
    assert_eq!(field(&text, "ShapeVariant1", "square").ty, "Square");
    // The by-JSON-type disjoint union: a string branch and an integer branch.
    assert_eq!(
        enum_variants(&text, "Scalar"),
        [
            "ScalarVariant0(Box<ScalarVariant0>)",
            "ScalarVariant1(Box<ScalarVariant1>)"
        ]
    );
    assert_eq!(
        alias_target(&text, "ScalarVariant0").as_deref(),
        Some("String")
    );
    assert_eq!(
        alias_target(&text, "ScalarVariant1").as_deref(),
        Some("i64")
    );
}

// --- poem-openapi: emits OpenAPI 3.0.0, rejected with E001 --------------------------------------

#[test]
fn poem_openapi_document_is_rejected_e001() {
    // poem-openapi pins OPENAPI_VERSION = "3.0.0"; spargen requires 3.1.x/3.2.x, so the document is
    // rejected loudly with E001 (never silently degraded). check/generate agree.
    let checked = check("poem-openapi.json");
    assert_eq!(checked.outcome(), Outcome::Rejected);
    assert!(has_code(&checked, Code::UnsupportedOpenApiVersion));

    let (report, _) = generate("poem-openapi.json", false);
    assert_eq!(report.outcome(), Outcome::Rejected);
    assert!(
        has_code(&report, Code::UnsupportedOpenApiVersion),
        "E001 expected: {:?}",
        report.diagnostics()
    );
}

// --- overlapping untagged framework union -------------------------------------------------------

#[test]
fn utoipa_untagged_overlap_generates_a_typed_union() {
    let (report, text) = generate("utoipa-untagged-overlap.json", false);
    assert_eq!(report.outcome(), Outcome::Generated);
    assert!(
        !has_code(&report, Code::NonDisjointUnion),
        "overlapping numeric branches are supported: {:?}",
        report.diagnostics()
    );

    let text = text.expect("untagged-union generation wrote a module");
    assert!(text.contains("fn ping"), "the sibling operation is emitted");
    assert!(
        text.contains("fn measure"),
        "the union operation is emitted"
    );
    assert!(
        text.contains("ResponseBodyVariant0(Box<ResponseBodyVariant0>)")
            && text.contains("ResponseBodyVariant1(Box<ResponseBodyVariant1>)"),
        "the overlapping response remains a typed, boxed enum: {text}"
    );
}
