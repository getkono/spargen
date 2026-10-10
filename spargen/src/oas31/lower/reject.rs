//! The rejections and warnings union and `allOf` lowering share, with their remedies.

use crate::diag::{Code, Diagnostic};
use crate::ir::Ty;
use crate::oas31::Schema;

use super::{LowerCtx, MetUnion};

impl<'a, 'doc> LowerCtx<'a, 'doc> {
    /// A union whose applicators or discriminator describe a combination no generated enum can
    /// carry: `oneOf` beside `anyOf`, or a `defaultMapping` whose member the enclosing schema's
    /// sibling keywords excluded, so the fallback has no branch. A `mapping`/`defaultMapping` entry
    /// naming a schema that is not a member is reported at the entry by
    /// [`Self::discriminator_members`].
    pub(super) fn reject_unrepresentable_union<T>(
        &mut self,
        schema: &Schema,
        message: &str,
    ) -> Option<T> {
        // E007 case: unrepresentable-applicators
        Diagnostic::error(Code::NonDisjointUnion, schema.provenance.clone())
            .message(message.to_owned())
            .remedy(
                "split the applicators into separate schemas, make every discriminator mapping name \
                 a member of the union, or omit this API segment with spargen::omit!",
            )
            .emit(self.diags);
        None
    }

    /// A union that resolves to itself, so its generated `Deserialize` would re-enter itself on the
    /// same input with no base case.
    pub(super) fn reject_self_referential_union<T>(
        &mut self,
        schema: &Schema,
        message: &str,
    ) -> Option<T> {
        // E007 case: resolves-to-itself
        Diagnostic::error(Code::NonDisjointUnion, schema.provenance.clone())
            .message(message.to_owned())
            .remedy(
                "break the reference cycle where a member refers to the union it is written in, or \
                 omit this API segment with spargen::omit!",
            )
            .emit(self.diags);
        None
    }

    /// A union the enclosing schema's own sibling keywords leave with no branch at all.
    pub(super) fn reject_branchless_union<T>(
        &mut self,
        schema: &Schema,
        message: &str,
    ) -> Option<T> {
        // E007 case: no-branch-left
        Diagnostic::error(Code::NonDisjointUnion, schema.provenance.clone())
            .message(message.to_owned())
            .remedy(
                "reconcile the enclosing schema's sibling keywords with the union's members, or \
                 omit this API segment with spargen::omit!",
            )
            .emit(self.diags);
        None
    }

    /// An `allOf` that mixes object and scalar members, which no single type can be.
    pub(super) fn reject_all_of_object_scalar_mix<T>(&mut self, schema: &Schema) -> Option<T> {
        // E013 case: object-scalar-mix
        self.reject_irreconcilable(
            Code::AllOfIrreconcilable,
            schema.provenance.clone(),
            "an `allOf` mixes object and scalar members, which cannot form one type",
            ALL_OF_REMEDY,
        )
    }

    /// An all-scalar `allOf` whose members have no common value or no single representable type.
    pub(super) fn reject_all_of_scalars<T>(&mut self, schema: &Schema) -> Option<T> {
        // E013 case: scalar-members
        self.reject_irreconcilable(
            Code::AllOfIrreconcilable,
            schema.provenance.clone(),
            "`allOf` scalar members have an empty or unrepresentable intersection",
            ALL_OF_REMEDY,
        )
    }

    /// `allOf` members whose `additionalProperties` value schemas have no common type.
    pub(super) fn reject_all_of_additional<T>(&mut self, schema: &Schema) -> Option<T> {
        // E013 case: additional-values
        self.reject_irreconcilable(
            Code::AllOfIrreconcilable,
            schema.provenance.clone(),
            "`allOf` members declare conflicting `additionalProperties`",
            ALL_OF_REMEDY,
        )
    }

    /// A property repeated across `allOf` members with types that cannot meet, which a member
    /// requires, so every instance must carry a value no type admits.
    pub(super) fn reject_all_of_required_property<T>(
        &mut self,
        schema: &Schema,
        name: &str,
    ) -> Option<T> {
        // E013 case: required-property
        self.reject_irreconcilable(
            Code::AllOfIrreconcilable,
            schema.provenance.clone(),
            format!(
                "property `{name}` appears in multiple `allOf` members with conflicting types, and \
                 a member requires it"
            ),
            ALL_OF_REMEDY,
        )
    }

    /// A required property no `allOf` member declares, whose members' `additionalProperties` value
    /// schemas share no value, so no instance can carry it.
    pub(super) fn reject_all_of_undeclared_required<T>(
        &mut self,
        schema: &Schema,
        name: &str,
    ) -> Option<T> {
        // E013 case: required-property
        self.reject_irreconcilable(
            Code::AllOfIrreconcilable,
            schema.provenance.clone(),
            format!(
                "property `{name}` is required but no `allOf` member declares it, and the members' \
                 `additionalProperties` value schemas it must satisfy share no value"
            ),
            ALL_OF_REMEDY,
        )
    }

    /// An `allOf` member that is the boolean schema `false`, which admits no value, so neither
    /// does the composition.
    pub(super) fn reject_all_of_false_member<T>(
        &mut self,
        provenance: crate::diag::Provenance,
    ) -> Option<T> {
        // E013 case: false-member
        self.reject_irreconcilable(
            Code::AllOfIrreconcilable,
            provenance,
            "an `allOf` member is `false`",
            ALL_OF_REMEDY,
        )
    }

    /// An `allOf` whose merge would have to read a `$ref` target still being lowered: a member that
    /// is a direct recursive reference, or a property or `additionalProperties` value two members
    /// both constrain that is typed by one. Its body is not known yet, so the composition can be
    /// computed neither against it nor by discarding it.
    pub(super) fn reject_all_of_cycle<T>(
        &mut self,
        provenance: crate::diag::Provenance,
        message: &str,
    ) -> Option<T> {
        // E013 case: cycle
        self.reject_irreconcilable(
            Code::AllOfIrreconcilable,
            provenance,
            message,
            ALL_OF_REMEDY,
        )
    }

    /// Two sides that share values no single Rust type represents ([`NoMeet::Unrepresentable`]),
    /// met where an empty meet would have been typed uninhabited or dropped: a union branch against
    /// the enclosing schema's siblings, or a property repeated across `allOf` members. Either
    /// stand-in would refuse the values the two sides share, so the composition is refused instead.
    ///
    /// [`NoMeet::Unrepresentable`]: super::meet::NoMeet::Unrepresentable
    pub(super) fn reject_unrepresentable_meet<T>(
        &mut self,
        schema: &Schema,
        message: &str,
    ) -> Option<T> {
        // E013 case: unrepresentable-meet
        self.reject_irreconcilable(
            Code::AllOfIrreconcilable,
            schema.provenance.clone(),
            message,
            ALL_OF_REMEDY,
        )
    }

    /// Report a `oneOf` whose sibling keywords meet every branch in the exact `null` alone (#632):
    /// `null` then matches every branch and fails the exactly-one rule, so no value satisfies the
    /// schema. The `allOf` spelling, the sibling written as a member beside the union, is the same
    /// empty intersection of scalar members, and both are `E013`.
    pub(super) fn reject_one_of_null_in_every_branch<T>(&mut self, schema: &Schema) -> Option<T> {
        // E013 case: scalar-members
        self.reject_irreconcilable(
            Code::AllOfIrreconcilable,
            schema.provenance.clone(),
            "this schema's own sibling keywords meet every `oneOf` member in `null` alone, so \
             `null` matches all of them and fails the exactly-one rule, and no value satisfies \
             the schema",
            UNION_SIBLING_REMEDY,
        )
    }

    /// Report that a `$ref` target and its own sibling keywords have no single typed intersection.
    /// `$ref` is a 2020-12 applicator, so this is the same class of irreconcilable composition an
    /// `allOf` reports — `E013` covers both spellings — but the remedy names the construct the
    /// author actually wrote. Its callers report an empty intersection and an inhabited but
    /// unrepresentable one alike, for any of the reasons an `allOf` merge has, so the message
    /// distinguishes no further than that.
    pub(super) fn reject_ref_sibling_intersection(&mut self, schema: &Schema) -> Option<Ty> {
        // E013 case: scalar-members, required-property, additional-values, object-scalar-mix, unrepresentable-meet
        self.reject_irreconcilable(
            Code::AllOfIrreconcilable,
            schema.provenance.clone(),
            "the `$ref` target and this schema's own sibling keywords have an empty or \
                 unrepresentable intersection",
            REF_SIBLING_REMEDY,
        )
    }

    /// Report that a schema's `allOf` composition and the `oneOf`/`anyOf` beside it, or among its
    /// members, have no single typed intersection (see [`Self::meet_union_with_all_of`]): no branch
    /// meets the composition, or one does in a way no single Rust type represents.
    pub(super) fn reject_all_of_union_meet(
        &mut self,
        schema: &Schema,
        spelling: MetUnion,
    ) -> Option<Ty> {
        let (message, remedy) = match spelling {
            MetUnion::AllOfMember => (
                "this `allOf`'s `oneOf`/`anyOf` member and its other members all apply, and their \
                 intersection is empty or unrepresentable",
                ALL_OF_REMEDY,
            ),
            MetUnion::BesideAllOf => (
                "this schema's `allOf` and the `oneOf`/`anyOf` beside it both apply, and their \
                 intersection is empty or unrepresentable",
                ALL_OF_REMEDY,
            ),
            MetUnion::RefSibling => (
                "the `$ref` target, this schema's own sibling keywords and the `oneOf`/`anyOf` \
                 beside them all apply, and their intersection is empty or unrepresentable",
                REF_SIBLING_REMEDY,
            ),
        };
        // E013 case: scalar-members, required-property, additional-values, object-scalar-mix, unrepresentable-meet
        self.reject_irreconcilable(
            Code::AllOfIrreconcilable,
            schema.provenance.clone(),
            message,
            remedy,
        )
    }

    /// Report that the category a `$ref`'s untyped sibling keywords establish (see
    /// [`implied_applicator_category`]) cannot be intersected with the target without either an
    /// empty result or a dropped target branch. `message` says which.
    ///
    /// [`implied_applicator_category`]: super::refiner::implied_applicator_category
    pub(super) fn reject_ref_sibling_category<T>(
        &mut self,
        schema: &Schema,
        message: &str,
    ) -> Option<T> {
        // E013 case: inferred-category
        self.reject_irreconcilable(
            Code::AllOfIrreconcilable,
            schema.provenance.clone(),
            message,
            REF_SIBLING_REMEDY,
        )
    }

    /// Acknowledge that a union's untyped object or array sibling keywords (a [`Refiner::Scoped`]
    /// sibling, beside the union or beside a `$ref` to it) reach no branch of their category. In
    /// 2020-12 they are then vacuously satisfied by every value the union accepts, so the union
    /// generates as it is, and the keywords are reported rather than dropped in silence.
    ///
    /// [`Refiner::Scoped`]: super::Refiner::Scoped
    pub(super) fn warn_unreached_union_sibling(&mut self, schema: &Schema, message: String) {
        // W011 case: unreached-union-sibling
        Diagnostic::warning(Code::DeclarationHasNoEffect, schema.provenance.clone())
            .message(message)
            .emit(self.diags);
    }

    /// Report that a union's untyped object or array sibling keywords (a [`Refiner::Scoped`]
    /// sibling) settle no category for a branch that states none: they are both kinds, or a
    /// deleted multi-type array admits another category beside theirs.
    ///
    /// [`Refiner::Scoped`]: super::Refiner::Scoped
    pub(super) fn reject_unscoped_union_sibling<T>(
        &mut self,
        schema: &Schema,
        message: &str,
    ) -> Option<T> {
        // E013 case: inferred-category
        self.reject_irreconcilable(
            Code::AllOfIrreconcilable,
            schema.provenance.clone(),
            message,
            UNION_SIBLING_REMEDY,
        )
    }

    /// Report that a `$ref` carrying shape-bearing siblings — or a union member, when the union
    /// has siblings of its own — closes a reference cycle back to the schema enclosing it, so the
    /// siblings would have to be intersected with a target whose definition depends on the result.
    pub(super) fn reject_ref_sibling_cycle(
        &mut self,
        schema: &Schema,
        message: &str,
    ) -> Option<Ty> {
        // E013 case: cycle
        self.reject_irreconcilable(
            Code::AllOfIrreconcilable,
            schema.provenance.clone(),
            message,
            REF_SIBLING_REMEDY,
        )
    }

    /// [`Self::reject_ref_sibling_cycle`] for a union member whose `$ref` closes a reference cycle
    /// back to the schema enclosing the union, in the one wording every spelling of that fact uses:
    /// the document-half guard, and the reservation half on a sole member and on several.
    pub(super) fn reject_union_member_cycle(&mut self, schema: &Schema) -> Option<Ty> {
        self.reject_ref_sibling_cycle(
            schema,
            "this union member's `$ref` closes a reference cycle back to the schema that encloses \
             it, so the enclosing schema's own sibling keywords would have to be intersected with \
             a target whose definition depends on the result",
        )
    }

    /// [`Self::reject_self_referential_union`] for a member that is a direct recursive `$ref` to
    /// the union being lowered, on a sole member and on several alike.
    pub(super) fn reject_union_member_is_the_union<T>(&mut self, schema: &Schema) -> Option<T> {
        self.reject_self_referential_union(
            schema,
            "a union member is a direct recursive `$ref` to the union being lowered, so the member \
             is the union itself and decoding it would never terminate",
        )
    }

    /// Report an irreconcilable composition with `message` and `remedy`: the one body every `E013`
    /// reporter above shares. `code` is always `E013`'s; it is the caller's argument so that each
    /// reporter names the code beside its own `// E013 case:` marker, which is how `spargen explain
    /// E013`'s case list is held to the sites that report each case.
    fn reject_irreconcilable<T>(
        &mut self,
        code: Code,
        provenance: crate::diag::Provenance,
        message: impl Into<String>,
        remedy: &str,
    ) -> Option<T> {
        Diagnostic::error(code, provenance)
            .message(message)
            .remedy(remedy)
            .emit(self.diags);
        None
    }
}

/// The remedy every `allOf` rejection (`E013`) gives.
const ALL_OF_REMEDY: &str =
    "restructure the composition so members agree, or omit this API segment with spargen::omit!";

/// The remedy every `$ref`-sibling intersection rejection (`E013`) gives, naming the construct the
/// author wrote rather than an `allOf` they did not.
const REF_SIBLING_REMEDY: &str = "restructure the schema so the `$ref` target and its sibling \
                                  keywords describe one representable type, or omit this API \
                                  segment with spargen::omit!";

/// The remedy for a union's untyped sibling keywords that cannot be applied to its branches.
const UNION_SIBLING_REMEDY: &str = "give the sibling keywords a `type`, move them into the \
                                    branches they constrain, or omit this API segment with \
                                    spargen::omit!";
