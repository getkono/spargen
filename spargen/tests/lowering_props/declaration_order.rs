//! Declaration order decides nothing (#999): a set of components recursive through an `allOf`
//! reaches one verdict, one code multiset and one declaration per schema in every order its
//! `components.schemas` keys are written in, and one verdict again moved into a referenced file.

use super::*;

/// A set of components, each `(name, body)` with the body's lines indented under its name, and
/// whether the set generates.
struct CyclicCase {
    what: &'static str,
    components: &'static [(&'static str, &'static str)],
    generates: bool,
}

/// `A`, an object whose field `b` is `B`.
const A_HOLDS_B: (&str, &str) = (
    "A",
    "type: object\nproperties:\n  b:\n    $ref: '#/components/schemas/B'",
);

/// `B`, which is `A` and more.
const B_IS_A_AND_MORE: (&str, &str) = (
    "B",
    "allOf:\n  - $ref: '#/components/schemas/A'\n  - type: object\n    properties:\n      extra:\n        type: string",
);

/// `C`, which is `B` and more.
const C_IS_B_AND_MORE: (&str, &str) = (
    "C",
    "allOf:\n  - $ref: '#/components/schemas/B'\n  - type: object\n    properties:\n      more:\n        type: integer",
);

/// `A`, an object whose fields are `B` and `C`.
const A_HOLDS_B_AND_C: (&str, &str) = (
    "A",
    "type: object\nproperties:\n  b:\n    $ref: '#/components/schemas/B'\n  c:\n    $ref: '#/components/schemas/C'",
);

/// `Z`, an object whose field `a` is `A`.
const Z_HOLDS_A: (&str, &str) = (
    "Z",
    "type: object\nproperties:\n  a:\n    $ref: '#/components/schemas/A'",
);

/// `A` holds a field to each of `B` and `C`, and each is an `allOf` of `A`.
const DIAMOND: &str = "a diamond: two compositions of one holder of a field to each";

/// The cases whose schemas, moved into a referenced file, generate in every order though as root
/// components they are rejected in every order: the two paths disagree. A case leaves this list
/// when the root components generate it too.
const GENERATES_RELOCATED: &[&str] = &[DIAMOND];

const CYCLIC_CASES: &[CyclicCase] = &[
    CyclicCase {
        what: "a field to a component that is the field's holder and more",
        components: &[A_HOLDS_B, B_IS_A_AND_MORE],
        generates: true,
    },
    CyclicCase {
        what: "the member reached through an inline `allOf` on a field",
        components: &[
            A_HOLDS_B,
            (
                "B",
                "type: object\nproperties:\n  inner:\n    allOf:\n      - $ref: '#/components/schemas/A'\n      - type: object\n        properties:\n          extra:\n            type: string",
            ),
        ],
        generates: true,
    },
    CyclicCase {
        what: "a chain of compositions, each member reached back through a field",
        components: &[A_HOLDS_B_AND_C, B_IS_A_AND_MORE, C_IS_B_AND_MORE],
        generates: true,
    },
    CyclicCase {
        what: "the chain entered through an unrelated composition",
        components: &[
            A_HOLDS_B_AND_C,
            B_IS_A_AND_MORE,
            C_IS_B_AND_MORE,
            ("D", "allOf:\n  - $ref: '#/components/schemas/E'"),
            (
                "E",
                "type: object\nproperties:\n  x:\n    $ref: '#/components/schemas/B'",
            ),
        ],
        generates: true,
    },
    CyclicCase {
        what: "the field a nullable union over the composed component",
        components: &[
            (
                "A",
                "type: object\nproperties:\n  b:\n    oneOf:\n      - $ref: '#/components/schemas/B'\n      - type: 'null'",
            ),
            B_IS_A_AND_MORE,
        ],
        generates: true,
    },
    CyclicCase {
        what: "the composition a variant of a union its member's field holds",
        components: &[
            (
                "A",
                "oneOf:\n  - $ref: '#/components/schemas/B'\n  - type: string",
            ),
            (
                "B",
                "allOf:\n  - $ref: '#/components/schemas/C'\n  - type: object\n    properties:\n      extra:\n        type: string",
            ),
            (
                "C",
                "type: object\nproperties:\n  a:\n    $ref: '#/components/schemas/A'\n  b:\n    $ref: '#/components/schemas/B'",
            ),
        ],
        generates: true,
    },
    CyclicCase {
        what: "a property two members both declare, one typed by the holder of a field to it",
        components: &[
            A_HOLDS_B,
            (
                "B",
                "allOf:\n  - type: object\n    properties:\n      x:\n        $ref: '#/components/schemas/A'\n  - type: object\n    properties:\n      x:\n        type: object\n        properties:\n          q:\n            type: string",
            ),
        ],
        generates: true,
    },
    CyclicCase {
        what: "an `additionalProperties` value two members both constrain, one the holder of a \
               field to it",
        components: &[
            A_HOLDS_B,
            (
                "B",
                "allOf:\n  - type: object\n    additionalProperties:\n      $ref: '#/components/schemas/A'\n  - type: object\n    additionalProperties:\n      type: object\n      properties:\n        q:\n          type: string",
            ),
        ],
        generates: true,
    },
    CyclicCase {
        what: "two components each composed of the other",
        components: &[
            (
                "A",
                "allOf:\n  - $ref: '#/components/schemas/B'\n  - type: object\n    properties:\n      a:\n        type: string",
            ),
            (
                "B",
                "allOf:\n  - $ref: '#/components/schemas/A'\n  - type: object\n    properties:\n      b:\n        type: string",
            ),
        ],
        generates: false,
    },
    CyclicCase {
        what: "a composition cycle reached through a field",
        components: &[
            ("A", "allOf:\n  - $ref: '#/components/schemas/B'"),
            ("B", "allOf:\n  - $ref: '#/components/schemas/A'"),
            Z_HOLDS_A,
        ],
        generates: false,
    },
    // An exact merge exists, but as root components no order lowers it: whichever of `B` and `C`
    // is opened first opens `A` for its member, and `A`'s field to the other one reads `A` still
    // open. Relocated, the same schemas generate that merge ([`GENERATES_RELOCATED`]). Pinned as
    // rejected in every order, so a fix that generates it moves this case to `generates: true`.
    CyclicCase {
        what: DIAMOND,
        components: &[
            A_HOLDS_B_AND_C,
            B_IS_A_AND_MORE,
            (
                "C",
                "allOf:\n  - $ref: '#/components/schemas/A'\n  - type: object\n    properties:\n      more:\n        type: integer",
            ),
        ],
        generates: false,
    },
    CyclicCase {
        what: "a field composed of its own holder",
        components: &[
            (
                "A",
                "type: object\nproperties:\n  c:\n    allOf:\n      - $ref: '#/components/schemas/A'",
            ),
            Z_HOLDS_A,
        ],
        generates: false,
    },
];

/// Every ordering of `0..len`, in lexicographic order.
fn permutations(len: usize) -> Vec<Vec<usize>> {
    if len == 0 {
        return vec![Vec::new()];
    }
    let mut all = Vec::new();
    for first in 0..len {
        for rest in permutations(len - 1) {
            let mut order = vec![first];
            order.extend(rest.into_iter().map(|at| at + usize::from(at >= first)));
            all.push(order);
        }
    }
    all
}

/// The orders of `0..len` a case is lowered in: every permutation up to three components, and
/// past that each rotation of the declared order and of its reverse, so every component is
/// declared first, and last, at least once.
fn declaration_orders(len: usize) -> Vec<Vec<usize>> {
    if len <= 3 {
        return permutations(len);
    }
    let forward: Vec<usize> = (0..len).collect();
    let backward: Vec<usize> = (0..len).rev().collect();
    [forward, backward]
        .iter()
        .flat_map(|order| {
            (0..len).map(move |shift| {
                let mut rotated = order.clone();
                rotated.rotate_left(shift);
                rotated
            })
        })
        .collect()
}

/// The document declaring `components` in `order`.
fn ordered_components_spec(components: &[(&str, &str)], order: &[usize]) -> String {
    let mut spec = String::from(
        "openapi: 3.1.0\ninfo:\n  title: Order\n  version: 1.0.0\npaths: {}\ncomponents:\n  schemas:\n",
    );
    for &at in order {
        let (name, body) = components[at];
        spec.push_str(&format!("    {name}:\n"));
        for line in body.lines() {
            spec.push_str(&format!("      {line}\n"));
        }
    }
    spec
}

/// The declaration of `name` in the generated module `source`, its lines trimmed and joined: a
/// `pub type` line, or a `pub struct` / `pub enum` with its body. Empty when nothing declares it.
/// Read rather than [`oracles::shape`], which expands every name a recursive type reaches to a
/// fixed depth, so its cost grows with the number of fields that recurse.
fn declaration(source: &str, name: &str) -> String {
    let module = oracles::types_module(source);
    let mut lines = module.lines().map(str::trim);
    let starts = [
        format!("pub struct {name} {{"),
        format!("pub enum {name} {{"),
        format!("pub type {name} ="),
    ];
    let Some(first) =
        lines.find(|line| starts.iter().any(|start| line.starts_with(start.as_str())))
    else {
        return String::new();
    };
    if first.starts_with("pub type ") {
        return first.to_owned();
    }
    let mut declared = first.to_owned();
    for line in lines {
        declared.push_str(line);
        if line.starts_with('}') {
            break;
        }
    }
    declared
}

/// A declaration with every `Box<T>` read as `T`. A recursive type is boxed at the reference that
/// closes its cycle, and which reference that is follows where lowering entered the cycle, so
/// declaration order moves it; `Box` adds no value and removes none.
fn unboxed(declared: &str) -> String {
    let mut out = String::with_capacity(declared.len());
    let mut opened = Vec::new();
    let mut rest = declared;
    while let Some(c) = rest.chars().next() {
        if let Some(after) = rest.strip_prefix("Box<") {
            opened.push(true);
            rest = after;
            continue;
        }
        rest = &rest[c.len_utf8()..];
        match c {
            '<' => opened.push(false),
            '>' if opened.pop() == Some(true) => continue,
            _ => {}
        }
        out.push(c);
    }
    out
}

/// Whether a component generates, and which diagnostics it gets, does not depend on the order its
/// `components.schemas` keys are written in (#999). Lowering visits the keys in document order, so
/// a target found mid-lowering through a field in one order is not mid-lowering in another. The
/// verdict, the code multiset, and each schema's generated [`declaration`] (read [`unboxed`])
/// agree in every order, and so does the verdict with the schemas moved into a referenced file
/// (`oracles::relocate`), where each is a bundle target rather than a root component, except for
/// the cases [`GENERATES_RELOCATED`] names, which generate there in every order.
#[test]
fn cyclic_compositions_do_not_depend_on_declaration_order() {
    for case in CYCLIC_CASES {
        let mut names: Vec<&str> = case.components.iter().map(|(name, _)| *name).collect();
        names.sort_unstable();
        let mut first: Option<(String, Vec<&'static str>, Vec<String>)> = None;
        for order in declaration_orders(case.components.len()) {
            let spec = ordered_components_spec(case.components, &order);
            let checked = check(&spec);
            let (generated, source) = generate_module(&spec);
            assert_eq!(
                rejected(&checked),
                rejected(&generated),
                "{}: `check` and `generate` disagree:\n{spec}\n{checked:#?}\n{generated:#?}",
                case.what
            );
            assert_eq!(
                !rejected(&generated),
                case.generates,
                "{}: in order {order:?}:\n{spec}\n{generated:#?}",
                case.what
            );
            let temp = tempfile::tempdir().unwrap();
            let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
            let root = oracles::write_relocated(&dir, &spec).expect("every case declares schemas");
            let relocated = check_at(&root);
            assert_eq!(
                !rejected(&relocated),
                case.generates || GENERATES_RELOCATED.contains(&case.what),
                "{}: relocated, in order {order:?}:\n{spec}\n{relocated:#?}",
                case.what
            );
            let declared: Vec<String> = names
                .iter()
                .map(|name| unboxed(&declaration(&source, name)))
                .collect();
            assert!(
                !case.generates || declared.iter().all(|declared| !declared.is_empty()),
                "{}: a generated schema declares nothing:\n{spec}\n{source}",
                case.what
            );
            let codes = codes(&generated);
            let Some((first_spec, first_codes, first_declared)) = &first else {
                first = Some((spec, codes, declared));
                continue;
            };
            assert_eq!(
                &codes, first_codes,
                "{}: the codes differ between\n{first_spec}\nand\n{spec}",
                case.what
            );
            assert_eq!(
                &declared, first_declared,
                "{}: the declarations differ between\n{first_spec}\nand\n{spec}",
                case.what
            );
        }
    }
}
