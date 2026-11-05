//! The `allOf` merge recurses into the objects its members nest: an `allOf` of objects whose
//! properties nest objects and scalars has the shape of the object its meet ([`Met`]) declares
//! outright.

use super::strategies::{nested_member_strategy, Nested, NestedMember};
use super::*;

impl Nested {
    /// The type as a YAML flow mapping.
    fn schema(&self) -> String {
        match self {
            Nested::Scalar(ty) => ty.schema().to_owned(),
            Nested::Object(props) => object_schema(
                props
                    .iter()
                    .map(|(&key, (ty, required))| (key, ty.schema(), *required)),
            ),
        }
    }
}

/// An object schema as a YAML flow mapping, from its properties: (index into [`KEYS`], schema,
/// required).
fn object_schema(props: impl Iterator<Item = (usize, String, bool)>) -> String {
    let mut required = Vec::new();
    let mut properties = Vec::new();
    for (key, schema, is_required) in props {
        if is_required {
            required.push(KEYS[key]);
        }
        properties.push(format!("{}: {schema}", KEYS[key]));
    }
    let required = if required.is_empty() {
        String::new()
    } else {
        format!("required: [{}], ", required.join(", "))
    };
    format!(
        "{{ type: object, {required}properties: {{ {} }} }}",
        properties.join(", ")
    )
}

/// The document whose component `Merged` is the `allOf` of `members`.
fn nested_all_of_spec(members: &[NestedMember]) -> String {
    let mut spec = String::from(
        "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\npaths: {}\ncomponents:\n  schemas:\n    Merged:\n      allOf:\n",
    );
    for member in members {
        let props = member
            .iter()
            .map(|(&key, (ty, required))| (key, ty.schema(), *required));
        spec.push_str(&format!("        - {}\n", object_schema(props)));
    }
    spec
}

/// What the members' types of one property meet in: the reference oracle for the recursive merge.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Met {
    Scalar(PropType),
    /// An object property every declaring member writes as an object: their meet, property by
    /// property, with `required` the union of theirs.
    Object(BTreeMap<usize, (Met, bool)>),
    /// No value is of every member's type: two different scalars, a scalar beside an object, or
    /// objects whose meet some required property leaves empty.
    Empty,
}

impl Met {
    /// The meet of the objects `members`, property by property.
    fn of_objects(members: &[&NestedMember]) -> BTreeMap<usize, (Met, bool)> {
        let mut declared: BTreeMap<usize, (Vec<&Nested>, bool)> = BTreeMap::new();
        for member in members {
            for (&key, (ty, required)) in *member {
                let entry = declared.entry(key).or_default();
                entry.0.push(ty);
                entry.1 |= *required;
            }
        }
        declared
            .into_iter()
            .map(|(key, (types, required))| (key, (Met::of(&types), required)))
            .collect()
    }

    /// The meet of the types one property is declared with.
    fn of(types: &[&Nested]) -> Met {
        let objects: Vec<&NestedMember> = types
            .iter()
            .filter_map(|ty| match ty {
                Nested::Object(props) => Some(props),
                Nested::Scalar(_) => None,
            })
            .collect();
        if objects.len() == types.len() {
            let props = Met::of_objects(&objects);
            if Met::irreconcilable(&props) {
                return Met::Empty;
            }
            return Met::Object(props);
        }
        match types[0] {
            Nested::Scalar(ty) if types.iter().all(|other| *other == types[0]) => Met::Scalar(*ty),
            _ => Met::Empty,
        }
    }

    /// Whether an object meet requires a property no value is of: no object satisfies it.
    fn irreconcilable(props: &BTreeMap<usize, (Met, bool)>) -> bool {
        props
            .values()
            .any(|(met, required)| *required && *met == Met::Empty)
    }

    /// The type as a YAML flow mapping that declares it outright, with no `allOf`: the empty type
    /// is the `false` schema.
    fn schema(&self) -> String {
        match self {
            Met::Scalar(ty) => ty.schema().to_owned(),
            Met::Object(props) => object_schema(
                props
                    .iter()
                    .map(|(&key, (met, required))| (key, met.schema(), *required)),
            ),
            Met::Empty => "false".to_owned(),
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    /// An `allOf` of objects whose properties nest objects and scalars merges recursively, as
    /// [`Met`] says: `Merged` has the shape of the object declared outright with the members'
    /// meet, an object property every member writes as an object being their meet in turn, and a
    /// property no value can fill uninhabited where it is optional. Where such a property is
    /// required, at the top level, no object satisfies the members (`E013`); one level down, the
    /// object holding it is the property no value can fill.
    #[test]
    fn an_all_of_of_nested_objects_merges_recursively(
        members in proptest::collection::vec(nested_member_strategy(), 2..=3)
    ) {
        let spec = nested_all_of_spec(&members);
        let met = Met::of_objects(&members.iter().collect::<Vec<_>>());
        if Met::irreconcilable(&met) {
            let report = check(&spec);
            prop_assert_eq!(report.outcome(), Outcome::Rejected, "{:#?}\n{}", report, spec);
            prop_assert!(has_code(&report, Code::AllOfIrreconcilable), "{:#?}\n{}", report, spec);
            return assert_relocation_changes_nothing(&spec, None);
        }

        let generated = generate_module(&spec);
        assert_relocation_changes_nothing(&spec, Some(&generated))?;
        let (report, source) = generated;
        prop_assert_eq!(report.outcome(), Outcome::Generated, "{:#?}\n{}", report, spec);

        let outright = format!(
            "openapi: 3.1.0\ninfo: {{ title: T, version: 1.0.0 }}\npaths: {{}}\ncomponents:\n  schemas:\n    Merged: {}\n",
            Met::Object(met).schema()
        );
        let (control, control_source) = generate_module(&outright);
        prop_assert_eq!(control.outcome(), Outcome::Generated, "{:#?}\n{}", control, outright);
        prop_assert_eq!(
            shape::named(&source, "Merged"),
            shape::named(&control_source, "Merged"),
            "the merge of\n{}\nis not the object declared outright as\n{}",
            spec,
            outright
        );
    }
}
