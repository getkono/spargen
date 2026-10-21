use std::collections::BTreeSet;

use super::{ChangeKind, Impact};

/// Every change kind, produced by an exhaustive match so a new variant does not compile until
/// it is listed — which is what makes the coverage assertions below closed over the enum.
/// `ChangeKind` is `#[non_exhaustive]`, so this list can only live inside the crate; the
/// fixtures that drive each kind live in `tests/diff.rs`, which cannot match exhaustively.
fn all_kinds() -> Vec<ChangeKind> {
    let every = [
        ChangeKind::OperationAdded,
        ChangeKind::OperationRemoved,
        ChangeKind::MethodRenamed,
        ChangeKind::RequestBodyAdded,
        ChangeKind::RequestBodyRemoved,
        ChangeKind::RequestBodyTypeChanged,
        ChangeKind::SuccessTypeChanged,
        ChangeKind::ErrorTypeChanged,
        ChangeKind::ApiErrorBodyAdded,
        ChangeKind::ApiErrorBodyRemoved,
        ChangeKind::RequiredParamAdded,
        ChangeKind::OptionalParamAdded,
        ChangeKind::ParamRemoved,
        ChangeKind::ParamTypeChanged,
        ChangeKind::ParamRequirednessChanged,
        ChangeKind::TypeAdded,
        ChangeKind::TypeRemoved,
        ChangeKind::TypeKindChanged,
        ChangeKind::FieldAdded,
        ChangeKind::RequiredFieldAdded,
        ChangeKind::FieldRemoved,
        ChangeKind::FieldTypeChanged,
        ChangeKind::FieldRequirednessChanged,
        ChangeKind::VariantAdded,
        ChangeKind::VariantRemoved,
        ChangeKind::VariantTypeChanged,
    ];
    for kind in every {
        // Exhaustive by construction: adding a variant to `ChangeKind` fails to compile here
        // until it is named, and the length assertion then fails until it joins `every`.
        match kind {
            ChangeKind::OperationAdded
            | ChangeKind::OperationRemoved
            | ChangeKind::MethodRenamed
            | ChangeKind::RequestBodyAdded
            | ChangeKind::RequestBodyRemoved
            | ChangeKind::RequestBodyTypeChanged
            | ChangeKind::SuccessTypeChanged
            | ChangeKind::ErrorTypeChanged
            | ChangeKind::ApiErrorBodyAdded
            | ChangeKind::ApiErrorBodyRemoved
            | ChangeKind::RequiredParamAdded
            | ChangeKind::OptionalParamAdded
            | ChangeKind::ParamRemoved
            | ChangeKind::ParamTypeChanged
            | ChangeKind::ParamRequirednessChanged
            | ChangeKind::TypeAdded
            | ChangeKind::TypeRemoved
            | ChangeKind::TypeKindChanged
            | ChangeKind::FieldAdded
            | ChangeKind::RequiredFieldAdded
            | ChangeKind::FieldRemoved
            | ChangeKind::FieldTypeChanged
            | ChangeKind::FieldRequirednessChanged
            | ChangeKind::VariantAdded
            | ChangeKind::VariantRemoved
            | ChangeKind::VariantTypeChanged => {}
        }
    }
    every.to_vec()
}

/// The `variant -> Rust identifier` mapping, for the source scan below.
fn variant_name(kind: ChangeKind) -> &'static str {
    match kind {
        ChangeKind::OperationAdded => "OperationAdded",
        ChangeKind::OperationRemoved => "OperationRemoved",
        ChangeKind::MethodRenamed => "MethodRenamed",
        ChangeKind::RequestBodyAdded => "RequestBodyAdded",
        ChangeKind::RequestBodyRemoved => "RequestBodyRemoved",
        ChangeKind::RequestBodyTypeChanged => "RequestBodyTypeChanged",
        ChangeKind::SuccessTypeChanged => "SuccessTypeChanged",
        ChangeKind::ErrorTypeChanged => "ErrorTypeChanged",
        ChangeKind::ApiErrorBodyAdded => "ApiErrorBodyAdded",
        ChangeKind::ApiErrorBodyRemoved => "ApiErrorBodyRemoved",
        ChangeKind::RequiredParamAdded => "RequiredParamAdded",
        ChangeKind::OptionalParamAdded => "OptionalParamAdded",
        ChangeKind::ParamRemoved => "ParamRemoved",
        ChangeKind::ParamTypeChanged => "ParamTypeChanged",
        ChangeKind::ParamRequirednessChanged => "ParamRequirednessChanged",
        ChangeKind::TypeAdded => "TypeAdded",
        ChangeKind::TypeRemoved => "TypeRemoved",
        ChangeKind::TypeKindChanged => "TypeKindChanged",
        ChangeKind::FieldAdded => "FieldAdded",
        ChangeKind::RequiredFieldAdded => "RequiredFieldAdded",
        ChangeKind::FieldRemoved => "FieldRemoved",
        ChangeKind::FieldTypeChanged => "FieldTypeChanged",
        ChangeKind::FieldRequirednessChanged => "FieldRequirednessChanged",
        ChangeKind::VariantAdded => "VariantAdded",
        ChangeKind::VariantRemoved => "VariantRemoved",
        ChangeKind::VariantTypeChanged => "VariantTypeChanged",
    }
}

#[test]
fn the_kind_list_covers_the_whole_enum() {
    assert_eq!(
        all_kinds().len(),
        26,
        "a `ChangeKind` variant reached the exhaustive match without joining the list"
    );
}

/// The codes are the JSON surface of `spargen diff`, so they are consumed by scripts: two
/// kinds sharing one, or a code that is not kebab-case, would break a caller silently.
#[test]
fn every_kind_has_a_distinct_kebab_case_code() {
    let codes: BTreeSet<&str> = all_kinds().iter().map(|kind| kind.code()).collect();
    assert_eq!(codes.len(), all_kinds().len(), "two kinds share a code");
    for code in codes {
        assert!(!code.is_empty());
        assert!(
            code.chars()
                .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-'),
            "`{code}` is not kebab-case"
        );
        assert!(!code.starts_with('-') && !code.ends_with('-'), "`{code}`");
    }
}

/// The codes are only *actually* the JSON surface if they are what `Serialize` emits. The
/// derive would emit the Rust variant name instead, silently disagreeing with both `code` and
/// the human rendering — so pin the two together over the whole enum.
#[test]
fn every_kind_serializes_as_its_code() {
    for kind in all_kinds() {
        let json = serde_json::to_string(&kind).expect("a kind serializes");
        assert_eq!(
            json,
            format!("\"{}\"", kind.code()),
            "{kind:?} does not serialize as its code"
        );
    }
}

/// Same contract one level up: `impact` and `bump` are read by scripts that branch on the
/// semver bump, and `--format human` spells it lowercase. The derive would emit `"Major"`.
#[test]
fn every_impact_serializes_as_its_lowercase_label() {
    for impact in [Impact::Patch, Impact::Minor, Impact::Major] {
        let json = serde_json::to_string(&impact).expect("an impact serializes");
        assert_eq!(json, format!("\"{}\"", impact.as_str()), "{impact:?}");
    }
}

/// Additive changes are minor and everything else is major. Stated as a closed list so that
/// reclassifying a kind — which changes the version bump spargen recommends — is a deliberate
/// edit here rather than a silent consequence of moving a match arm.
#[test]
fn only_the_six_additive_kinds_are_minor() {
    const MINOR: &[ChangeKind] = &[
        ChangeKind::OperationAdded,
        ChangeKind::ApiErrorBodyAdded,
        ChangeKind::OptionalParamAdded,
        ChangeKind::TypeAdded,
        ChangeKind::FieldAdded,
        ChangeKind::VariantAdded,
    ];
    for kind in all_kinds() {
        let expected = if MINOR.contains(&kind) {
            Impact::Minor
        } else {
            Impact::Major
        };
        assert_eq!(
            kind.impact(),
            expected,
            "{} is classified {:?}",
            kind.code(),
            kind.impact()
        );
    }
}

/// CLAUDE.md asks `tests/diff.rs` for "semver classification per change kind". Nine of the 24
/// had a fixture; the rest were classified by policy alone, with nothing proving the
/// classifier ever produces them. This makes "per change kind" mean what it says.
///
/// A bare mention is not a fixture: `kinds(&report).contains(&ChangeKind::X)` passes beside
/// spurious extra changes, and a filter on `X` asserts nothing about whether `X` was produced.
/// So each kind must be named inside an `assert_eq!` — the exact change list a fixture expects.
#[test]
fn every_kind_has_a_fixture_in_the_diff_suite() {
    // A packaged `.crate` is tested outside the workspace, so the check is skipped there —
    // gated on the workspace marker, not on the file, so that inside the repository a missing
    // or renamed suite fails instead of passing with a line on stderr nobody reads.
    let Some(root) = repo_root() else {
        eprintln!("skipping: not tested from the workspace");
        return;
    };
    let suite = read_repo_file(&root, "spargen/tests/diff.rs");
    let asserted = assert_eq_spans(&suite);
    for kind in all_kinds() {
        let variant = variant_name(kind);
        let needle = format!("ChangeKind::{variant}");
        let pinned = suite.match_indices(&needle).any(|(at, _)| {
            let whole = suite[at + needle.len()..]
                .chars()
                .next()
                .is_none_or(|next| !next.is_alphanumeric() && next != '_');
            whole && asserted.iter().any(|span| span.contains(&at))
        });
        assert!(
            pinned,
            "{} has no fixture: `{needle}` appears in no `assert_eq!` in spargen/tests/diff.rs",
            kind.code()
        );
    }
}

/// The byte span of every `assert_eq!(…)` argument list in `source`, found by bracket depth.
/// String literals (plain and raw) are skipped, so a bracket inside a message or an inline
/// spec does not end a span early.
fn assert_eq_spans(source: &str) -> Vec<std::ops::Range<usize>> {
    const OPEN: &str = "assert_eq!(";
    let bytes = source.as_bytes();
    let mut spans = Vec::new();
    for (start, _) in source.match_indices(OPEN) {
        let mut at = start + OPEN.len();
        let mut depth = 1_usize;
        while depth > 0 {
            let Some(&byte) = bytes.get(at) else {
                panic!("unbalanced `assert_eq!(` at byte {start} of spargen/tests/diff.rs")
            };
            match byte {
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => depth -= 1,
                b'"' => at = skip_string(bytes, at, 0),
                // A character literal such as `'}'` or `'\''`; a lifetime has no closing `'`.
                b'\'' if bytes.get(at + 2) == Some(&b'\'') => at += 2,
                b'\'' if bytes.get(at + 1) == Some(&b'\\') && bytes.get(at + 3) == Some(&b'\'') => {
                    at += 3;
                }
                b'r' if matches!(bytes.get(at + 1), Some(b'#' | b'"'))
                    && !bytes[at - 1].is_ascii_alphanumeric()
                    && bytes[at - 1] != b'_' =>
                {
                    let hashes = bytes[at + 1..].iter().take_while(|b| **b == b'#').count();
                    at = skip_string(bytes, at + 1 + hashes, hashes);
                }
                _ => {}
            }
            at += 1;
        }
        spans.push(start..at);
    }
    spans
}

/// The index of the closing `"` (and its `hashes` trailing `#`s) of the string literal whose
/// opening `"` is at `open`; a plain literal (`hashes == 0`) honours backslash escapes.
fn skip_string(bytes: &[u8], open: usize, hashes: usize) -> usize {
    let mut at = open + 1;
    loop {
        match bytes.get(at) {
            None => panic!("unterminated string literal at byte {open}"),
            Some(b'\\') if hashes == 0 => at += 2,
            Some(b'"')
                if bytes[at + 1..].iter().take(hashes).all(|b| *b == b'#')
                    && bytes.len() > at + hashes =>
            {
                return at + hashes;
            }
            Some(_) => at += 1,
        }
    }
}

/// The workspace root, or `None` when this crate is tested from a packaged `.crate`, which
/// carries no workspace manifest. Gating on the *workspace marker* rather than on the file
/// under inspection is deliberate: inside the repository a missing `tests/diff.rs` must fail
/// the test, not silently skip it.
fn repo_root() -> Option<std::path::PathBuf> {
    let root = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/..")).to_path_buf();
    let manifest = std::fs::read_to_string(root.join("Cargo.toml")).ok()?;
    manifest.contains("[workspace]").then_some(root)
}

/// Read a file the repository must carry, failing — never skipping — when it is absent.
/// Only call this under [`repo_root`], which is what decides whether the file must exist.
fn read_repo_file(root: &std::path::Path, relative: &str) -> String {
    std::fs::read_to_string(root.join(relative)).unwrap_or_else(|error| {
        panic!("{relative} must exist in the repository, and reading it failed: {error}")
    })
}

/// Once the workspace gate has passed, a missing input is a failure: a skip keyed on the file
/// itself turned a renamed `tests/diff.rs` into a silent pass (#409).
#[test]
#[should_panic(expected = "must exist in the repository")]
fn a_missing_repository_file_fails_rather_than_skips() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    read_repo_file(root, "tests/no-such-suite.rs");
}

/// One node of a synthetic type tree: every [`crate::ir::TypeKind`] `canon_ty` renders, with
/// the nullability and boxing a reference to it may carry.
#[derive(Debug, Clone)]
struct Node {
    shape: Shape,
    nullable: bool,
    boxed: bool,
}

#[derive(Debug, Clone)]
enum Shape {
    Primitive(crate::ir::Prim),
    /// A struct, string enum, union or uninhabited type: rendered by its allocated name.
    Nominal(NominalKind),
    IntEnum,
    BoolEnum,
    Bytes,
    Null,
    Any,
    Array(Box<Node>),
    Tuple(Vec<Node>),
}

#[derive(Debug, Clone, Copy)]
enum NominalKind {
    Struct,
    StringEnum,
    Union,
    Never,
}

fn node() -> impl proptest::strategy::Strategy<Value = Node> {
    use crate::ir::Prim;
    use proptest::prelude::*;
    let leaf = prop_oneof![
        proptest::sample::select(vec![
            Prim::Bool,
            Prim::String,
            Prim::I32,
            Prim::I64,
            Prim::F64,
            Prim::Uuid,
            Prim::DateTime,
            Prim::Date,
        ])
        .prop_map(Shape::Primitive),
        proptest::sample::select(vec![
            NominalKind::Struct,
            NominalKind::StringEnum,
            NominalKind::Union,
            NominalKind::Never,
        ])
        .prop_map(Shape::Nominal),
        Just(Shape::IntEnum),
        Just(Shape::BoolEnum),
        Just(Shape::Bytes),
        Just(Shape::Null),
        Just(Shape::Any),
    ];
    let wrap = |shape| {
        (shape, any::<bool>(), any::<bool>()).prop_map(|(shape, nullable, boxed)| Node {
            shape,
            nullable,
            boxed,
        })
    };
    wrap(leaf.boxed()).prop_recursive(4, 24, 4, move |inner| {
        wrap(
            prop_oneof![
                inner.clone().prop_map(|item| Shape::Array(Box::new(item))),
                proptest::collection::vec(inner, 0..4).prop_map(Shape::Tuple),
            ]
            .boxed(),
        )
    })
}

/// Insert `node` into `api`'s type graph bottom-up, naming each nominal definition, and return
/// the reference to it. Definitions are made through `reserve` and filled in place, which is
/// how a graph is built without naming `crate::diag` (a layer this subsystem does not take).
fn insert(node: &Node, api: &mut crate::ir::Api, names: &mut crate::name::Names) -> crate::ir::Ty {
    use crate::ir::{
        AdditionalProps, Openness, ScalarEnum, ScalarRepr, Struct, TypeKind, Union, UnionMode,
        UnionStrategy,
    };
    let scalar_enum = |repr| {
        TypeKind::Enum(ScalarEnum {
            repr,
            variants: Vec::new(),
            openness: Openness::Closed,
        })
    };
    let kind = match &node.shape {
        Shape::Primitive(prim) => TypeKind::Primitive(*prim),
        Shape::Nominal(NominalKind::Struct) => TypeKind::Struct(Struct {
            fields: Vec::new(),
            additional: AdditionalProps::Allow,
        }),
        Shape::Nominal(NominalKind::StringEnum) => scalar_enum(ScalarRepr::String),
        Shape::Nominal(NominalKind::Union) => TypeKind::Union(Union {
            variants: Vec::new(),
            strategy: UnionStrategy::Trial {
                mode: UnionMode::OneOf,
                priorities: Vec::new(),
            },
        }),
        Shape::Nominal(NominalKind::Never) => TypeKind::Never,
        Shape::IntEnum => scalar_enum(ScalarRepr::Int),
        Shape::BoolEnum => scalar_enum(ScalarRepr::Bool),
        Shape::Bytes => TypeKind::Bytes,
        Shape::Null => TypeKind::Null,
        Shape::Any => TypeKind::Any,
        Shape::Array(item) => TypeKind::Array(Box::new(insert(item, api, names))),
        Shape::Tuple(items) => {
            TypeKind::Tuple(items.iter().map(|item| insert(item, api, names)).collect())
        }
    };
    let id = api.types.reserve();
    api.types.get_mut(id).expect("just reserved").kind = kind;
    if let Shape::Nominal(_) = node.shape {
        names.types.insert(
            id,
            crate::name::Ident::new(format!("Nominal{}", names.types.len())),
        );
    }
    crate::ir::Ty {
        id,
        nullable: node.nullable,
        boxed: node.boxed,
    }
}

/// The single generic argument of `ty` when it is the bare path `wrapper<…>`.
fn generic_argument<'a>(ty: &'a syn::Type, wrapper: &str) -> Option<&'a syn::Type> {
    let syn::Type::Path(path) = ty else {
        return None;
    };
    let [segment] = path.path.segments.iter().collect::<Vec<_>>()[..] else {
        return None;
    };
    let syn::PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return None;
    };
    if segment.ident != wrapper {
        return None;
    }
    match arguments.args.iter().collect::<Vec<_>>()[..] {
        [syn::GenericArgument::Type(inner)] => Some(inner),
        _ => None,
    }
}

/// The bare single-identifier path `ty` names, if it is one.
fn bare_name(ty: &syn::Type) -> Option<String> {
    let syn::Type::Path(path) = ty else {
        return None;
    };
    match path.path.segments.iter().collect::<Vec<_>>()[..] {
        [segment] if segment.arguments.is_empty() => Some(segment.ident.to_string()),
        _ => None,
    }
}

/// Hold the parsed rendering of `node` to its structure: `Option<…>` exactly when nullable,
/// never a `Box` (boxing is not a surface distinction), a tuple of exactly N elements exactly
/// when the kind is `Tuple(N)` (`Null` is the one other `()`, the unit type codegen also emits
/// for it), `Vec<…>` around an array's item, and otherwise the scalar label or allocated name.
fn check(
    node: &Node,
    ty: crate::ir::Ty,
    parsed: &syn::Type,
    api: &crate::ir::Api,
    names: &crate::name::Names,
) -> Result<(), proptest::test_runner::TestCaseError> {
    use crate::ir::TypeKind;
    use proptest::{prop_assert, prop_assert_eq};
    let parsed = if node.nullable {
        let inner = generic_argument(parsed, "Option");
        prop_assert!(inner.is_some(), "a nullable reference renders `Option<…>`");
        inner.expect("checked")
    } else {
        prop_assert!(generic_argument(parsed, "Option").is_none());
        parsed
    };
    prop_assert!(
        generic_argument(parsed, "Box").is_none(),
        "boxing is rendered"
    );
    let kind = &api.types.get(ty.id).expect("inserted").kind;
    let tuple = match parsed {
        syn::Type::Tuple(tuple) => Some(tuple.elems.len()),
        _ => None,
    };
    let expected_tuple = match kind {
        TypeKind::Tuple(items) => Some(items.len()),
        TypeKind::Null => Some(0),
        TypeKind::Reserved => unreachable!("`insert` fills every reservation it makes"),
        _ => None,
    };
    prop_assert_eq!(tuple, expected_tuple, "tuple arity of {:?}", kind);
    prop_assert!(
        !matches!(parsed, syn::Type::Paren(_)),
        "a parenthesized type"
    );
    let expected = match &node.shape {
        Shape::Tuple(nodes) => {
            let (syn::Type::Tuple(tuple), TypeKind::Tuple(items)) = (parsed, kind) else {
                unreachable!("arity checked above")
            };
            for ((node, item), element) in nodes.iter().zip(items).zip(&tuple.elems) {
                check(node, *item, element, api, names)?;
            }
            return Ok(());
        }
        Shape::Array(node) => {
            let TypeKind::Array(item) = kind else {
                unreachable!("`insert` builds an array for an array node")
            };
            let inner = generic_argument(parsed, "Vec");
            prop_assert!(inner.is_some(), "an array renders `Vec<…>`");
            return check(node, **item, inner.expect("checked"), api, names);
        }
        // The arity check above is all there is to `()`.
        Shape::Null => return Ok(()),
        Shape::Primitive(prim) => super::prim_label(*prim).to_owned(),
        Shape::Nominal(_) => names.types[&ty.id].as_str().to_owned(),
        Shape::IntEnum => "i64".to_owned(),
        Shape::BoolEnum => "bool".to_owned(),
        Shape::Bytes => "Bytes".to_owned(),
        Shape::Any => "Value".to_owned(),
    };
    prop_assert_eq!(bare_name(parsed), Some(expected));
    Ok(())
}

proptest::proptest! {
    /// `canon_ty` renders every type tree as a Rust type that parses, and whose structure is
    /// the tree's: in particular a one-position tuple keeps the trailing comma that makes it
    /// `(T,)` rather than a parenthesized `T` (#449), at any depth.
    #[test]
    fn canon_ty_parses_as_the_type_its_tree_describes(node in node()) {
        let mut api = crate::ir::Api {
            info: crate::ir::Info {
                title: "T".to_owned(),
                version: "1".to_owned(),
                description: None,
            },
            servers: Vec::new(),
            operations: Vec::new(),
            types: Default::default(),
            security_schemes: Default::default(),
        };
        let mut names = crate::name::Names::default();
        let ty = insert(&node, &mut api, &mut names);
        let rendered = super::canon_ty(ty, &api, &names);
        let parsed = syn::parse_str::<syn::Type>(&rendered);
        proptest::prop_assert!(parsed.is_ok(), "{rendered:?} does not parse as a type");
        check(&node, ty, &parsed.expect("checked"), &api, &names)?;
    }
}
