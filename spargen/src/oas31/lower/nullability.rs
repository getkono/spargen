//! Whether a schema, an `allOf` merge, a union branch or a `$ref` target admits, denies, or
//! leaves undecided a `null`.

use std::collections::HashSet;

use crate::diag::{Diagnostics, Provenance};
use crate::ir::{Ty, TypeKind};
use crate::oas31::{JsonType, RefOr, Schema, SchemaOr};
use crate::source::{is_remote_ref, Node};

use super::combine::{schema_has_union, Contribution};
use super::shape::schema_has_shape_constraint;
use super::{memoised_decision, resolved_identity, LowerCtx, MAX_SCHEMA_DEPTH};

impl<'a, 'doc> LowerCtx<'a, 'doc> {
    /// Whether some member of `schema`'s object `allOf` decides the merge's nullability, as
    /// [`Self::gather_all_of`] records it in each [`Contribution::Object`]: the schema's own
    /// `type`, a member that states one, a `$ref` member whose target decides it
    /// ([`Self::ref_target_decides_null`]; a bundle target expanded in place is read as its own
    /// members are), and the members of a nested `allOf` or of a `$ref` member's siblings. A
    /// member this cannot read as an object (`enum`, `const`, a union) is taken as deciding, which
    /// keeps the lowered nullability. `depth` bounds the walk; a chain past it decides. Each `$ref`
    /// target is read once per pass and its answer replayed
    /// ([`Self::target_decides_null_memo`], [`Self::resolved_all_of_decides_null_memo`]).
    pub(super) fn all_of_decides_null(&self, schema: &Schema, depth: u32) -> bool {
        if depth >= MAX_SCHEMA_DEPTH
            || stated_nullability(schema).is_some()
            || schema.enum_values.is_some()
            || schema.const_value.is_some()
            || schema_has_union(schema)
        {
            return true;
        }
        if let Some(reference) = &schema.reference {
            let target_decides =
                if reference.starts_with("#/components/schemas/") || is_remote_ref(reference) {
                    self.ref_target_decides_null_within(reference, &schema.provenance, depth + 1)
                } else {
                    match self.resolver.resolve(
                        reference,
                        &schema.provenance,
                        &mut Diagnostics::default(),
                    ) {
                        Ok(resolved) => memoised_decision(
                            &self.resolved_all_of_decides_null_memo,
                            &resolved.schema,
                            || self.all_of_decides_null(&resolved.schema, depth + 1),
                        ),
                        Err(_) => true,
                    }
                };
            let mut sibling = schema.clone();
            sibling.reference = None;
            return target_decides
                || (schema_has_shape_constraint(&sibling)
                    && self.all_of_decides_null(&sibling, depth + 1));
        }
        schema.all_of.iter().any(|member| match member {
            SchemaOr::Schema(member) => self.all_of_decides_null(member, depth + 1),
            SchemaOr::Bool(_) => false,
        })
    }

    /// Whether `ty` is the exact JSON `null` (`()`), as a `const: null` or `enum: [null]` branch
    /// lowers: a branch `null` matches without its `Ty` being nullable.
    pub(super) fn is_exact_null(&self, ty: Ty) -> bool {
        matches!(
            self.graph.get(ty.id).map(|def| &def.kind),
            Some(TypeKind::Null)
        )
    }

    /// Whether `member`, a union branch lowered to `ty`, is an object whose lowered non-null
    /// struct decides nothing about `null`: no `type`, and nothing else
    /// [`Self::all_of_decides_null`] reads as deciding, so `null` satisfies it wherever the
    /// conjuncts the union is met with admit it.
    pub(super) fn branch_leaves_null_undecided(&self, member: &SchemaOr, ty: Ty) -> bool {
        let SchemaOr::Schema(member) = member else {
            return false;
        };
        !ty.nullable
            && matches!(
                self.graph.get(ty.id).map(|def| &def.kind),
                Some(TypeKind::Struct(_))
            )
            && !self.all_of_decides_null(member, 0)
    }

    /// Whether `member`, a union branch lowered to `ty`, accepts `null` wherever a conjunct the
    /// union is met with admits it: an untyped object ([`Self::branch_leaves_null_undecided`]), or
    /// a branch that states nothing and lowers to `Value` (`true`, `{}`), which the meet otherwise
    /// leaves as it is (#588, #592). Either is then counted as a branch `null` matches.
    pub(super) fn branch_takes_conjunct_null(&self, member: &SchemaOr, ty: Ty) -> bool {
        self.branch_leaves_null_undecided(member, ty)
            || (!ty.nullable
                && self
                    .graph
                    .get(ty.id)
                    .is_some_and(|def| matches!(def.kind, TypeKind::Any)))
    }

    /// Whether `member`, a union branch lowered to `ty`, takes the `null` its enclosing `type`
    /// array permits (#574): its own keywords leave `null` undecided, so `null` matches it
    /// wherever the union's other sibling keywords admit it. That is a branch lowering to `Value`,
    /// an untyped object ([`Self::branch_leaves_null_undecided`]), a cycle-closing `$ref` whose
    /// target's struct is still a reservation and whose keywords decide nothing, and a nested
    /// union `null` matches through its own branches ([`Self::union_admits_null_undecided`]).
    pub(super) fn branch_takes_permitted_null(&self, member: &SchemaOr, ty: Ty) -> bool {
        // A nested union's branches decide, whatever it lowered to: one that is `Value` can still
        // be a `oneOf` that `null` matches twice.
        if let SchemaOr::Schema(member) = member {
            if schema_has_union(member) {
                return !ty.nullable && self.union_admits_null_undecided(member, 0);
            }
        }
        if self
            .graph
            .get(ty.id)
            .is_some_and(|def| matches!(def.kind, TypeKind::Any))
            || self.branch_leaves_null_undecided(member, ty)
        {
            return true;
        }
        let SchemaOr::Schema(member) = member else {
            return false;
        };
        !ty.nullable && self.is_reservation(ty.id) && !self.all_of_decides_null(member, 0)
    }

    /// Whether `schema` is a `oneOf`/`anyOf` whose other keywords decide nothing about `null` and
    /// whose branches `null` matches by the union's own rule (exactly one for `oneOf`, at least one
    /// for `anyOf`), counting a branch that states `null`, one that is `true`, one whose keywords
    /// leave `null` undecided, and a nested union that admits it in turn. `depth` bounds the walk.
    fn union_admits_null_undecided(&self, schema: &Schema, depth: u32) -> bool {
        let (members, one_of) = match (schema.one_of.is_empty(), schema.any_of.is_empty()) {
            (false, true) => (&schema.one_of, true),
            (true, false) => (&schema.any_of, false),
            _ => return false,
        };
        if depth >= MAX_SCHEMA_DEPTH {
            return false;
        }
        let mut rest = schema.clone();
        rest.one_of.clear();
        rest.any_of.clear();
        rest.discriminator = None;
        if self.all_of_decides_null(&rest, depth + 1) {
            return false;
        }
        let matching = members
            .iter()
            .filter(|member| match member {
                SchemaOr::Bool(accepts) => *accepts,
                SchemaOr::Schema(member) => {
                    stated_nullability(member) == Some(true)
                        || self.union_admits_null_undecided(member, depth + 1)
                        || (!schema_has_union(member)
                            && !self.all_of_decides_null(member, depth + 1))
                }
            })
            .count();
        if one_of {
            matching == 1
        } else {
            matching > 0
        }
    }

    /// [`target_decides_null`] of the schema the `$ref` written at `at` lowered to: a root
    /// component first, as [`Self::ensure_component`] chooses it, then a remote document from the
    /// root, as [`Self::ensure_remote`] resolves it, and otherwise the referring file's target.
    /// A target that is a bare alias (a `$ref` with no shape-bearing sibling) is the schema it
    /// names, so the chain is followed to its body. The target was already lowered from the same
    /// resolution, so this only re-reads it; a target that cannot be read keeps its lowered
    /// nullability as the decision, and reports nothing a second time.
    ///
    /// A target body that is an `allOf` decides exactly when some member of its merge does
    /// ([`Self::all_of_decides_null`], issue #565): a component composed of untyped object members
    /// alone lowers to the non-null struct [`object_all_of_admits_null`] gives a merge no member
    /// decides, and that struct is no decision, as the same members written inline make none.
    pub(super) fn ref_target_decides_null(&self, reference: &str, at: &Provenance) -> bool {
        self.ref_target_decides_null_within(reference, at, 0)
    }

    /// [`Self::ref_target_decides_null`] reached `depth` steps into an
    /// [`Self::all_of_decides_null`] walk, which bounds the two together: a target's `allOf` can
    /// name the `$ref` that reached it.
    fn ref_target_decides_null_within(&self, reference: &str, at: &Provenance, depth: u32) -> bool {
        self.ref_target_body(reference, at).is_none_or(|target| {
            if target.all_of.is_empty() {
                target_decides_null(&target)
            } else {
                memoised_decision(&self.target_decides_null_memo, &target, || {
                    self.all_of_decides_null(&target, depth + 1)
                })
            }
        })
    }

    /// The body of the schema the `$ref` written at `at` names, as
    /// [`Self::ref_target_decides_null`] finds it: a root component first, then a remote document
    /// from the root, and otherwise the referring file's target, with a bare alias (a `$ref` with
    /// no shape-bearing sibling) followed to the schema it names. `None` where the chain cannot be
    /// read, which reports nothing a second time.
    fn ref_target_body(
        &self,
        reference: &str,
        at: &Provenance,
    ) -> Option<std::borrow::Cow<'_, Schema>> {
        let mut reference = reference.to_owned();
        let mut at = at.clone();
        // Lowering the chain already refused an alias cycle; the bound only keeps this total.
        for _ in 0..MAX_SCHEMA_DEPTH {
            let component = reference
                .strip_prefix("#/components/schemas/")
                .and_then(|name| self.document.components.schemas.get(name));
            let target = match component {
                Some(RefOr::Item(target)) => std::borrow::Cow::Borrowed(target),
                Some(RefOr::Ref(alias)) => {
                    reference.clone_from(&alias.reference);
                    at = alias.provenance.clone();
                    continue;
                }
                None => {
                    let from = if is_remote_ref(&reference) {
                        &self.document.provenance
                    } else {
                        &at
                    };
                    let Ok(resolved) =
                        self.resolver
                            .resolve(&reference, from, &mut Diagnostics::default())
                    else {
                        return None;
                    };
                    resolved.schema
                }
            };
            let Some(next) = &target.reference else {
                return Some(target);
            };
            let mut sibling = target.as_ref().clone();
            sibling.reference = None;
            if schema_has_shape_constraint(&sibling) {
                return Some(target);
            }
            reference.clone_from(next);
            at = target.provenance.clone();
        }
        None
    }

    /// Whether a union branch decides `null` for [`undecided_admits_null`], reading a `$ref`
    /// branch through its target (#594): `false` decides, `true` does not, and a schema branch
    /// decides where [`target_decides_null`] of its own keywords does. A `$ref` branch is its
    /// target met with its own sibling keywords, so it decides where either does; a `$ref` to a
    /// `true` or `{}` component then leaves `null` undecided, as the same branch written inline
    /// does. The target body is read as the inline branch is, not through an `allOf` walk, so a
    /// `$ref` to an `allOf` component decides as the inline `allOf` branch does.
    pub(super) fn branch_decides_null(&self, branch: &SchemaOr) -> bool {
        match branch {
            SchemaOr::Bool(admits) => !admits,
            SchemaOr::Schema(schema) => self.ref_schema_decides_null(schema, 0),
        }
    }

    /// [`Self::branch_decides_null`] of a schema node reached `depth` `$ref` steps from the
    /// branch: a `$ref` node decides `null` where its own sibling keywords do or its target does,
    /// read the same way; any other node is [`target_decides_null`]. A boolean target (a `false`
    /// or `true` component) is read as the inline boolean branch is: `false` decides, `true` does
    /// not. A target that cannot be read, or a chain past the bound, decides, which keeps the
    /// lowered nullability.
    fn ref_schema_decides_null(&self, schema: &Schema, depth: u32) -> bool {
        if let Some(admits) = schema.boolean {
            return !admits;
        }
        let Some(reference) = &schema.reference else {
            return target_decides_null(schema);
        };
        let mut sibling = schema.clone();
        sibling.reference = None;
        target_decides_null(&sibling)
            || depth >= MAX_SCHEMA_DEPTH
            || self
                .ref_target_body(reference, &schema.provenance)
                .is_none_or(|target| self.ref_schema_decides_null(&target, depth + 1))
    }

    /// Whether a union branch denies `null` ([`denies_null`]), reading a `$ref` branch through its
    /// target (#590): the branch is its target met with its own sibling keywords, so it denies
    /// `null` where either does by its own keywords alone. A `$ref` to a non-null object
    /// component then decides nothing beside an untyped branch, as the same branch written
    /// inline does. A target body that is itself a `$ref` with shape siblings is read the same
    /// way, recursively, so `{ $ref: W }` with `W: { $ref: U, type: object }` denies `null` as the
    /// inline `{ $ref: U, type: object }` does. A target that cannot be read, or whose body is an
    /// `allOf` or a union, may admit `null`.
    pub(super) fn branch_denies_null(&self, branch: &SchemaOr) -> bool {
        match branch {
            SchemaOr::Schema(schema) => self.ref_schema_denies_null(schema, 0),
            SchemaOr::Bool(_) => denies_null(branch),
        }
    }

    /// [`Self::branch_denies_null`] of a schema node reached `depth` `$ref` steps from the branch:
    /// a `$ref` node denies `null` where its own sibling keywords do or its target does, read
    /// the same way; any other node is [`schema_denies_null`]. A boolean target is read as the
    /// inline boolean branch is ([`denies_null`]): a `$ref` to a `false` component denies `null`,
    /// so beside an untyped branch it decides nothing, as the inline `false` does, now that
    /// [`Self::branch_decides_null`] reads that target as deciding (#594). Lowering already
    /// refused a `$ref` cycle; the bound only keeps this total.
    fn ref_schema_denies_null(&self, schema: &Schema, depth: u32) -> bool {
        if let Some(admits) = schema.boolean {
            return !admits;
        }
        let Some(reference) = &schema.reference else {
            return schema_denies_null(schema);
        };
        let mut sibling = schema.clone();
        sibling.reference = None;
        schema_denies_null(&sibling)
            || (depth < MAX_SCHEMA_DEPTH
                && self
                    .ref_target_body(reference, &schema.provenance)
                    .is_some_and(|target| self.ref_schema_denies_null(&target, depth + 1)))
    }

    /// Apply the enclosing `allOf` schema's own nullability (a `"null"` in its type array) to the
    /// merged type. Set after the final insert — a pure mutate that preserves the last-insert
    /// invariant.
    pub(super) fn with_all_of_nullability(&self, schema: &Schema, mut ty: Ty) -> Ty {
        if schema.types.types.contains(&JsonType::Null) {
            ty.nullable = true;
        }
        ty
    }

    /// The null type for an object `allOf` whose object meet is empty, when `null` still satisfies
    /// it: every member admits `null` and one decides it ([`object_all_of_admits_null`]). The
    /// schema's own object keywords are one of those members ([`Self::gather_all_of`]), so its
    /// `type` listing `null` counts as their answer, and does not override a member that denies
    /// it: no value satisfies such a schema. Its own `type`, `enum` or `const` that is not an
    /// object keyword ([`schema_is_object_like`]) is no contribution, yet constrains every value
    /// all the same, so one that excludes `null` leaves nothing either. So does such a keyword on
    /// a nested `allOf` member [`Self::gather_member`] flattens into this meet, inline or as a
    /// non-component `$ref`'s resolved target, which equally contributes nothing of its own
    /// ([`Self::flattened_keywords_admit_null`], #569). `None` where `null` is excluded, so the
    /// caller reports the empty composition.
    ///
    /// `intersect_types` collapses an empty non-null meet that admits `null` to the null type, so
    /// the `$ref`-sibling spelling of the same conjunction already lowered to `()`; rejecting it
    /// here split the spellings of one conjunction (#542), as #450 removed for a nullable union
    /// refined to nothing but `null`. The meets inserted since `mark` reach nothing the null type
    /// refers to, so they are discarded, and the null type is the final graph insert.
    ///
    /// [`schema_is_object_like`]: super::combine::schema_is_object_like
    pub(super) fn null_only_all_of(
        &mut self,
        schema: &Schema,
        hint: &str,
        contributions: &[Contribution],
        mark: u32,
    ) -> Option<Ty> {
        if !object_all_of_admits_null(contributions) || !self.flattened_keywords_admit_null(schema)
        {
            return None;
        }
        self.discard_meet_intermediates(mark, &TypeKind::Null);
        Some(self.insert_schema_type(schema, hint, TypeKind::Null))
    }

    /// [`own_keywords_admit_null`] over an object `allOf` schema and every schema
    /// [`Self::gather_member`] flattens into its meet: a member with an `allOf` and no union beside
    /// it, whether inline, as a `$ref`'s siblings, or as the resolved target of a non-component
    /// `$ref` member, which [`Self::gather_ref_target`] expands in place rather than lowering.
    /// Flattening gathers only such a schema's members and object keywords, so its own `type`,
    /// `enum` or `const` reaches no contribution, yet still constrains every value of the meet
    /// (#569). Any other member is lowered or read as a contribution of its own, which carries its
    /// nullability already: a component or remote `$ref` lowers to a type, and a plain body is
    /// read by its keywords.
    fn flattened_keywords_admit_null(&self, schema: &Schema) -> bool {
        own_keywords_admit_null(schema)
            && self.flattened_members_admit_null(schema, &mut HashSet::new(), 0)
    }

    /// Whether every schema `schema`'s `allOf` members flatten into its meet admits `null` by its
    /// own keywords: [`Self::flattened_keywords_admit_null`] below the schema itself.
    fn flattened_members_admit_null(
        &self,
        schema: &Schema,
        visited: &mut HashSet<String>,
        depth: u32,
    ) -> bool {
        schema.all_of.iter().all(|member| match member {
            SchemaOr::Schema(member) => self.flattened_member_admits_null(member, visited, depth),
            SchemaOr::Bool(_) => true,
        })
    }

    /// [`Self::flattened_keywords_admit_null`] of one `allOf` member, read the way
    /// [`Self::gather_member`] gathers it. The answer is the conjunction of every flattened
    /// schema's own keywords, so a resolved target already in `visited` adds nothing a second
    /// time: skipping it keeps a target shared along many paths linear. Gathering already refused
    /// a cycle and an over-deep chain, and resolved every target this reaches, so the depth bound
    /// only keeps this total and a target that cannot be read is no constraint here.
    fn flattened_member_admits_null(
        &self,
        member: &Schema,
        visited: &mut HashSet<String>,
        depth: u32,
    ) -> bool {
        if depth >= MAX_SCHEMA_DEPTH {
            return true;
        }
        if let Some(reference) = &member.reference {
            let target_admits = reference.starts_with("#/components/schemas/")
                || is_remote_ref(reference)
                || match self.resolver.resolve(
                    reference,
                    &member.provenance,
                    &mut Diagnostics::default(),
                ) {
                    Ok(resolved) => {
                        let fresh = resolved_identity(&resolved.schema.provenance)
                            .is_none_or(|key| visited.insert(key));
                        !fresh
                            || self.flattened_member_admits_null(
                                &resolved.schema,
                                visited,
                                depth + 1,
                            )
                    }
                    Err(_) => true,
                };
            let mut sibling = member.clone();
            sibling.reference = None;
            return target_admits
                && (!schema_has_shape_constraint(&sibling)
                    || self.flattened_member_admits_null(&sibling, visited, depth + 1));
        }
        if member.all_of.is_empty() || schema_has_union(member) {
            return true;
        }
        own_keywords_admit_null(member)
            && self.flattened_members_admit_null(member, visited, depth + 1)
    }

    /// Whether an already-lowered type admits JSON `null`, resolving its kind out of the graph.
    /// [`type_accepts_null`] needs the kind beside the [`Ty`]; callers outside the intersection
    /// machinery hold only the [`Ty`].
    pub(super) fn ty_accepts_null(&self, ty: Ty) -> bool {
        self.graph
            .get(ty.id)
            .is_some_and(|def| type_accepts_null(ty, &def.kind))
    }
}

pub(super) fn type_accepts_null(ty: Ty, kind: &TypeKind) -> bool {
    ty.nullable || matches!(kind, TypeKind::Null | TypeKind::Any)
}

pub(super) fn non_nullable(mut ty: Ty) -> Ty {
    ty.nullable = false;
    ty
}

/// Whether an object `allOf` merge admits `null` (issue #425): every member that decides its
/// nullability admits it, and at least one decides. An untyped member admits `null` and decides
/// nothing, so a merge of untyped members alone keeps the non-null struct an untyped object
/// schema lowers to on its own; one nullable `$ref` member beside them makes it nullable, as the
/// `$ref`-sibling spelling of the same conjunction does. Only object contributions reach here.
pub(super) fn object_all_of_admits_null(contributions: &[Contribution]) -> bool {
    let mut decided = false;
    for contribution in contributions {
        if let Contribution::Object {
            nullable: Some(admits),
            ..
        } = contribution
        {
            if !admits {
                return false;
            }
            decided = true;
        }
    }
    decided
}

/// An object composition met with a union beside it ([`LowerCtx::lower_all_of_beside_union`],
/// [`LowerCtx::lower_all_of_with_union_member`]), made to admit `null` where no member decides it
/// (issue #541): its members are untyped objects alone — `$ref`s to untyped object components,
/// which [`object_all_of_admits_null`] reads as denying `null` for want of a decision — and the
/// same members written inline are scoped refiners that leave `null` to the union. So the union
/// decides it in either spelling, where it decides it at all: a `union` that states no `type` and
/// whose every branch is untyped decides nothing either, and the meet keeps the non-null answer an
/// `allOf` of untyped members alone gets. Nor does one whose only deciding branches deny `null`
/// beside an untyped branch (#581): the untyped branch leaves `null` undecided, so the inline
/// spelling of the same meet stays non-null. `denies_null` is [`LowerCtx::branch_denies_null`],
/// which reads a `$ref` branch through its target (#590), so a `$ref` to a non-null object
/// component denies `null` there as the same branch written inline does. `decides` is
/// [`LowerCtx::branch_decides_null`], which reads a `$ref` branch through its target (#594), so a
/// `$ref` to a `true` or `{}` component leaves `null` undecided as the inline `true` or `{}`
/// branch does. A composition some member decides, or a scalar one, keeps its nullability.
pub(super) fn undecided_admits_null(
    mut composed: Ty,
    has_object: bool,
    contributions: &[Contribution],
    union: &Schema,
    decides: impl Fn(&SchemaOr) -> bool,
    denies_null: impl Fn(&SchemaOr) -> bool,
) -> Ty {
    let decided = contributions.iter().any(|contribution| {
        matches!(
            contribution,
            Contribution::Object {
                nullable: Some(_),
                ..
            }
        )
    });
    let branches = || union.one_of.iter().chain(&union.any_of);
    // An untyped branch leaves `null` to the union's other branches (#581): beside one, branches
    // that deny `null` (by their own keywords, or a `$ref` branch's target) decide nothing, as
    // the same union met with the same members written inline keeps them non-null.
    let has_undecided = branches().any(|branch| !decides(branch));
    let union_decides = stated_nullability(union).is_some()
        || branches().any(|branch| decides(branch) && !(has_undecided && denies_null(branch)));
    if has_object && !decided && union_decides {
        composed.nullable = true;
    }
    composed
}

/// Whether a branch of `union` admits `null` by its own keywords: one with no `$ref`, `allOf` or
/// union of its own whose stated `type`, `enum` or `const` admits it, such as
/// `type: [object, 'null']` or `type: 'null'` (#586). An untyped conjunct leaves `null` to the
/// union, so where such a branch admits it the conjunct does too, as [`undecided_admits_null`]
/// makes an untyped `allOf` composition admit it: the `$ref`-sibling spelling's untyped target
/// and the inline spellings' untyped refiners then admit `null`, and an untyped object branch
/// takes it from the meet as it does in the `allOf` spellings. The `true` schema states nothing,
/// and a branch whose `null` lies behind a `$ref` or a composition is not read here.
pub(super) fn union_branch_admits_null(union: &Schema) -> bool {
    union.one_of.iter().chain(&union.any_of).any(|branch| {
        matches!(branch, SchemaOr::Schema(branch)
            if branch.reference.is_none()
                && branch.all_of.is_empty()
                && !schema_has_union(branch)
                && (stated_nullability(branch).is_some()
                    || branch.enum_values.is_some()
                    || branch.const_value.is_some())
                && own_keywords_admit_null(branch))
    })
}

/// Whether a union branch denies `null` by its own keywords alone: `false`, or a branch with no
/// `$ref`, `allOf` or union of its own whose stated `type`, `enum` or `const` leaves `null` out
/// ([`own_keywords_admit_null`]). A branch this cannot read so may admit `null`; a `$ref` branch
/// is read through its target by [`LowerCtx::branch_denies_null`].
fn denies_null(branch: &SchemaOr) -> bool {
    match branch {
        SchemaOr::Bool(admits) => !admits,
        SchemaOr::Schema(branch) => schema_denies_null(branch),
    }
}

/// [`denies_null`] of a schema node: one with no `$ref`, `allOf` or union of its own whose stated
/// `type`, `enum` or `const` leaves `null` out.
fn schema_denies_null(schema: &Schema) -> bool {
    schema.reference.is_none()
        && schema.all_of.is_empty()
        && !schema_has_union(schema)
        && !own_keywords_admit_null(schema)
}

/// Whether a schema's own `type` admits `null`: `None` for an untyped schema, which states no
/// category and so decides nothing about `null` in an `allOf` merge.
pub(super) fn stated_nullability(schema: &Schema) -> Option<bool> {
    (!schema.types.types.is_empty()).then(|| schema.types.types.contains(&JsonType::Null))
}

/// Whether a `$ref` member's target decides its own nullability in an `allOf` merge (issue #541).
/// A plain untyped body — no `type`, no `enum` or `const`, no `$ref` and no composition
/// — is the inline untyped member written as a component: it admits `null` and decides nothing, as
/// [`stated_nullability`] reads the inline spelling. Anything else keeps its lowered nullability as
/// the decision it was before, except that [`LowerCtx::ref_target_decides_null`] reads a `$ref`
/// target's `allOf` body through its members (issue #565).
fn target_decides_null(schema: &Schema) -> bool {
    stated_nullability(schema).is_some()
        || schema.enum_values.is_some()
        || schema.const_value.is_some()
        || schema.reference.is_some()
        || !schema.all_of.is_empty()
        || schema_has_union(schema)
}

/// Whether a union member is a null-only schema (`{type: "null"}`) — stripped from the union and
/// folded into its nullability, exactly like a `"null"` in a type array. A bare `$ref` member is
/// never null-only here (it names a component with its own shape); only an inline `type: null`
/// node with no other constraints counts.
pub(super) fn member_is_null_only(member: &SchemaOr) -> bool {
    let SchemaOr::Schema(schema) = member else {
        return false;
    };
    schema.reference.is_none()
        && schema.types.types == [JsonType::Null]
        && schema.one_of.is_empty()
        && schema.any_of.is_empty()
        && schema.all_of.is_empty()
        && schema.enum_values.is_none()
        && schema.const_value.is_none()
        && schema.properties.is_empty()
}

/// Whether a schema accepts `null`: a `"null"` member of its type array, or a `null` `enum` member
/// or `const`. Computed at component reserve time so `$ref` consumers wrap the type in `Option`,
/// and it agrees with the `nullable` that [`LowerCtx::lower_schema`]/[`LowerCtx::lower_enum`]
/// compute from the same schema.
pub(super) fn schema_is_nullable(schema: &Schema) -> bool {
    schema.types.types.contains(&JsonType::Null)
        || schema
            .enum_values
            .as_ref()
            .is_some_and(|values| values.iter().any(|value| matches!(value.node, Node::Null)))
        || schema
            .const_value
            .as_ref()
            .is_some_and(|value| matches!(value.node, Node::Null))
}

/// Whether every one of a schema's own `type`, `enum` and `const` that it states admits `null`
/// (vacuously so where it states none). Unlike [`schema_is_nullable`], which asks whether any of
/// them lists `null`, this is their conjunction: `{type: string, enum: [null]}` admits no `null`.
fn own_keywords_admit_null(schema: &Schema) -> bool {
    let type_admits = stated_nullability(schema).unwrap_or(true);
    let enum_admits = schema
        .enum_values
        .as_ref()
        .is_none_or(|values| values.iter().any(|value| matches!(value.node, Node::Null)));
    let const_admits = schema
        .const_value
        .as_ref()
        .is_none_or(|value| matches!(value.node, Node::Null));
    type_admits && enum_admits && const_admits
}
