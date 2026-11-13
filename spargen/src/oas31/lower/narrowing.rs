//! Narrowing a string or enum type a meet produced, and the open / closed narrowing modes.

use crate::ir::{Docs, Openness, Prim, ScalarEnum, ScalarRepr, ScalarValue, Ty, TypeKind};

use super::meet::{intersect_primitives, NoMeet};
use super::nullability::non_nullable;
use super::LowerCtx;

/// The members of `variants` a set held in `width` keeps, in their order: an integer member that
/// width holds, and every string and boolean one.
pub(super) fn held_values(variants: &[ScalarValue], width: Prim) -> Vec<ScalarValue> {
    variants
        .iter()
        .filter(|value| match value {
            ScalarValue::Int(int) => width.holds_integer(*int),
            ScalarValue::Bool(_) | ScalarValue::String(_) => true,
        })
        .cloned()
        .collect()
}

impl<'a, 'doc> LowerCtx<'a, 'doc> {
    /// The meet of the scalar set `set` (whose type is `enum_ty`) with the primitive `primitive` of
    /// its JSON category: the set itself, or — for a closed set narrowing a plain `string`,
    /// `integer` or `boolean` where [`Self::narrowing_opens`] holds — an open set listing the same
    /// values, whose domain is the primitive it narrowed. That is the set itself, opened, when it
    /// is one of [`Self::open_candidates`], and a new open copy otherwise (a set that came from a
    /// `$ref` target, or from another intersection, may be reached from where it must stay
    /// closed).
    ///
    /// Of the strings, only a plain `string` widens it: `uuid` and the date formats have a decoded
    /// representation of their own that an arbitrary string is not. Under `open_narrowing` (in a
    /// response body's own schema or not), a string set meeting one of them is
    /// [`Openness::Locked`] instead, so no plain `string` met before or after opens it: the set an
    /// `allOf` lowers to does not depend on where its formatted member sits.
    ///
    /// An integer set meets an integer primitive at the narrower of the two widths
    /// ([`intersect_primitives`]), keeping the members that width holds: none left is
    /// [`NoMeet::Empty`], and a narrower width is a new set under `hint`.
    ///
    /// A set the response already opened, met where [`Self::narrowing_opens`] does not hold (a
    /// union variant `intersect_union` meets it with), is a new closed copy under `hint`, for the
    /// reason the enum-meet arm of [`Self::intersect_non_null`] gives.
    pub(super) fn narrowed_set(
        &mut self,
        enum_ty: Ty,
        set: &ScalarEnum,
        primitive: Prim,
        hint: &str,
    ) -> Result<Ty, NoMeet> {
        let openness = match (set.repr, set.openness, primitive) {
            (ScalarRepr::String, _, Prim::Uuid | Prim::Date | Prim::DateTime)
                if self.open_narrowing =>
            {
                Openness::Locked
            }
            (ScalarRepr::String, Openness::Closed, Prim::String)
            | (ScalarRepr::Int | ScalarRepr::Bool, Openness::Closed, _)
                if self.narrowing_opens =>
            {
                Openness::Open
            }
            (_, Openness::Open, _) if !self.narrowing_opens => Openness::Closed,
            (_, openness, _) => openness,
        };
        let width = match set.repr {
            ScalarRepr::Int => intersect_primitives(set.width, primitive).unwrap_or(set.width),
            ScalarRepr::String | ScalarRepr::Bool => set.width,
        };
        if width == set.width {
            return Ok(self.reopened_set(enum_ty, set, openness, hint));
        }
        let variants = held_values(&set.variants, width);
        if variants.is_empty() {
            return Err(NoMeet::Empty);
        }
        Ok(self.insert_type(
            hint,
            TypeKind::Enum(ScalarEnum {
                repr: set.repr,
                variants,
                width,
                openness,
            }),
            Docs::default(),
            None,
        ))
    }

    /// The set `set` (whose type is `enum_ty`) with `openness`: the set itself when it already has
    /// it, the set opened as [`Self::opened_set`] does, and otherwise a closed or locked one. That
    /// is the set itself, changed in place, when it is one of [`Self::open_candidates`] met where
    /// [`Self::narrowing_opens`] holds, and a new copy under `hint` otherwise, since a set reached
    /// from anywhere else may be reached from where it must keep its own openness.
    pub(super) fn reopened_set(
        &mut self,
        enum_ty: Ty,
        set: &ScalarEnum,
        openness: Openness,
        hint: &str,
    ) -> Ty {
        if openness == set.openness {
            return non_nullable(enum_ty);
        }
        if openness == Openness::Open {
            return self.opened_set(enum_ty, set);
        }
        if self.narrowing_opens && self.reopen_in_place(enum_ty, openness) {
            return non_nullable(enum_ty);
        }
        self.insert_type(
            hint,
            TypeKind::Enum(ScalarEnum {
                openness,
                ..set.clone()
            }),
            Docs::default(),
            None,
        )
    }

    /// Give the set `enum_ty` `openness` in place, when it is one of [`Self::open_candidates`]:
    /// whether it was.
    fn reopen_in_place(&mut self, enum_ty: Ty, openness: Openness) -> bool {
        if !self.open_candidates.contains(&enum_ty.id) {
            return false;
        }
        if let Some(TypeKind::Enum(own)) = self.graph.get_mut(enum_ty.id).map(|def| &mut def.kind) {
            own.openness = openness;
            return true;
        }
        false
    }

    /// The closed set `set` (whose type is `enum_ty`), opened: in place when it is one of
    /// [`Self::open_candidates`], and as a new open copy otherwise, as [`Self::narrowed_set`]
    /// describes.
    fn opened_set(&mut self, enum_ty: Ty, set: &ScalarEnum) -> Ty {
        if self.reopen_in_place(enum_ty, Openness::Open) {
            return non_nullable(enum_ty);
        }
        // Named for the closed set it opens, which stays in the graph where it came from.
        let (name_hint, docs, provenance) = match self.graph.get(enum_ty.id) {
            Some(def) => (
                format!("{}Open", def.name_hint),
                def.docs.clone(),
                Some(def.provenance.clone()),
            ),
            None => return non_nullable(enum_ty),
        };
        self.insert_type(
            &name_hint,
            TypeKind::Enum(ScalarEnum {
                openness: Openness::Open,
                ..set.clone()
            }),
            docs,
            provenance,
        )
    }

    /// Run `lower` with `open_narrowing` out of effect, restoring the enclosing position's answer
    /// afterwards. Every `$ref` target and every union is lowered through this.
    pub(super) fn closed_narrowing<T>(&mut self, lower: impl FnOnce(&mut Self) -> T) -> T {
        let enclosing = std::mem::replace(&mut self.narrowing_opens, false);
        let lowered = lower(self);
        self.narrowing_opens = enclosing;
        lowered
    }

    /// Run `lower` with `open_narrowing` in effect when the option is on, restoring the enclosing
    /// answer afterwards. Used over a response body's own schema, and by
    /// [`Self::intersect_union`] to meet the one branch a union narrows to where the enclosing
    /// position's narrowing was open, so that branch is met as the position itself would be.
    pub(super) fn response_narrowing<T>(&mut self, lower: impl FnOnce(&mut Self) -> T) -> T {
        let enclosing = std::mem::replace(&mut self.narrowing_opens, self.open_narrowing);
        let lowered = lower(self);
        self.narrowing_opens = enclosing;
        lowered
    }
}
