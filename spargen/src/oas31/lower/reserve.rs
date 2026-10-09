//! Component, remote and bundle-target reservations: typing a `$ref` target once, and the
//! back-edges and multi-pass nullability settlement a recursive reference needs.

use std::collections::HashMap;

use crate::diag::{Code, Diagnostic, Provenance};
use crate::ir::{Ty, TypeId, TypeKind};
use crate::oas31::{RefOr, Schema, SchemaOr};
use crate::source::is_remote_ref;

use super::defaults::default_display_for;
use super::nullability::{member_is_null_only, schema_is_nullable};
use super::shape::schema_has_shape_constraint;
use super::{append_doc_note, resolved_hint, resolved_identity, LowerCtx, Reservation};

/// One of a reservation frame's two memos: a key to its root id and nullability.
type ReservationMemo = HashMap<String, (TypeId, bool)>;

impl<'a, 'doc> LowerCtx<'a, 'doc> {
    /// Lower the schema `$ref` `reference`, written at `at`, to its shared, cycle-safe type through
    /// the memo its spelling routes to: a `#/components/schemas/<name>` to
    /// [`Self::ensure_component`], a remote (`http`/`https`) reference to [`Self::ensure_remote`]
    /// (keyed by `url#fragment`), and every other reference to [`Self::ensure_resolved`] (keyed by
    /// the resolved `file#pointer`, which is why the ordinary spelling and the explicit
    /// `./lib.yaml#/…` spelling of one target share one type rather than two). `hint` names a
    /// bundle target that has no final pointer token of its own.
    ///
    /// [`Self::open_reservation_for_ref`] mirrors this dispatch for a question that lowers nothing.
    pub(super) fn ensure_reference(
        &mut self,
        reference: &str,
        at: &Provenance,
        hint: &str,
    ) -> Option<Ty> {
        if let Some(name) = reference.strip_prefix("#/components/schemas/") {
            self.ensure_component(name, Some(reference), at)
        } else if is_remote_ref(reference) {
            self.ensure_remote(reference)
        } else {
            self.ensure_resolved(reference, at, hint)
        }
    }

    /// The root component the schema at `pointer` in `file` is: `Some(name)` when `file` is the root
    /// document and `pointer` is `/components/schemas/<name>` for a `name` the root declares.
    ///
    /// That map is such a schema's identity — [`Self::ensure_resolved`] routes a target there back
    /// to [`Self::ensure_component`] — so every question asked by resolved `(file, pointer)` asks
    /// this first. The name is the pointer token verbatim, never unescaped, as the component map's
    /// own lookup by a reference's stripped fragment is. A deeper pointer (`Tree/properties/x`) or
    /// an empty token is no component: structural validation admits only `^[a-zA-Z0-9._-]+$` as a
    /// root component key, so neither could be declared, and the filter says so without relying on
    /// it.
    pub(super) fn root_component_at<'p>(
        &self,
        file: crate::diag::FileId,
        pointer: &'p crate::diag::JsonPointer,
    ) -> Option<&'p str> {
        if file != self.resolver.root_id() {
            return None;
        }
        pointer
            .as_str()
            .strip_prefix("/components/schemas/")
            .filter(|name| !name.is_empty() && !name.contains('/'))
            .filter(|name| self.document.components.schemas.contains_key(*name))
    }

    /// Lower `#/components/schemas/{name}` to its shared type, lowering it on first use and
    /// returning the cached type on every later one.
    ///
    /// `at` is the provenance of the `$ref` site asking for the component, not the component's own:
    /// when `name` is not declared at all there is no component to point at, so the diagnostic has
    /// to name the reference that could not be followed. That pointer is also what
    /// [`crate::compat`]'s auto-carve maps back to an enclosing operation, so a root-level
    /// provenance here would make the rejection un-carvable.
    ///
    /// Carvability holds for a `$ref` site in a referenced sub-file too, provided `at` carries that
    /// file's span: `compat::carve_rules` reads the pointer in the file the span lies in, and
    /// carves a sub-file construct as a file-scoped pointer rule.
    ///
    /// A name the root document does not declare is handed to [`Self::ensure_resolved`], because a
    /// `$ref` written inside a sub-file spells that file's own components the same way. That is a
    /// re-entry into the resolver from a function the resolver's own component path can call back
    /// into, so it owes a cycle-safety argument, and here it is: `ensure_resolved` reserves the
    /// target's id under its resolved `file#pointer` *before* lowering its body, so a re-entry on
    /// the same target — self-recursion, mutual recursion, an alias loop, or a diamond — finds the
    /// reservation and returns a boxed back-edge rather than descending again. Every cycle closes in
    /// one step, every target is lowered once, and only genuinely new targets consume depth. The two
    /// memos do not compete for one target: a resolved reference that lands on a root component
    /// comes straight back here by name, so `components` stays the single identity for those.
    pub(super) fn ensure_component(
        &mut self,
        name: &str,
        reference: Option<&str>,
        at: &crate::diag::Provenance,
    ) -> Option<Ty> {
        self.closed_narrowing(|ctx| ctx.ensure_component_closed(name, reference, at))
    }

    /// [`Self::ensure_component`]'s body, run with `open_narrowing` out of effect: a component is
    /// a `$ref` target, lowered once and shared by every use, whichever position first reached it.
    fn ensure_component_closed(
        &mut self,
        name: &str,
        reference: Option<&str>,
        at: &crate::diag::Provenance,
    ) -> Option<Ty> {
        self.warn_if_root_shadows_the_referring_file(name, reference, at);
        if let Some(ty) = self.reserved_type(Reservation::Component(name.to_owned())) {
            return Some(ty);
        }
        // No such component. Report it against the referring site rather than dropping the
        // construct that named it: a silently-dropped `$ref` takes its request body, response, or
        // parameter with it, which is exactly the silent degradation the taxonomy forbids.
        let Some(component) = self.document.components.schemas.get(name) else {
            let reference = format!("#/components/schemas/{name}");
            // The name missed the ROOT document's component map — but a `$ref` written inside a
            // referenced sub-file spells that file's own components exactly the same way, and a
            // JSON Pointer fragment addresses the document it appears in. `Resolver::resolve`
            // already implements that: it keys on the provenance's file and shortcuts to the parsed
            // component map only for the root. `ensure_component` is reached by callers that strip
            // the `#/components/schemas/` prefix before any file is considered, so a sub-file's
            // sibling reference never got there. Hand it back to the resolver.
            //
            // Root first, file second: the root map was already consulted above, so a document that
            // resolves today keeps selecting the same component and only a name the root does not
            // declare reaches the sub-file reading. Which namespace *should* win when both declare
            // the name is a separate question; this deliberately does not change the answer.
            if self.resolver.written_in(at) != self.resolver.root_id() {
                // The resolver reports its own failure, so a miss here is already diagnosed. Going
                // through `ensure_resolved` rather than straight to `resolve`/`lower_schema` is what
                // makes this re-entry safe *and* finite: see that method and the note above.
                return self.ensure_resolved(&reference, at, name);
            }
            // A raw `/` here is always a further pointer segment, never part of a component name: a
            // literal slash in a key is spelled `~1`. So when the segment it starts from is a
            // declared component, the fragment is a JSON Pointer into that component's body — a
            // *subschema* — and RFC 6901 gives it exactly the meaning the relative-file spelling
            // (`./lib.yaml#/components/schemas/Envelope/properties/payload`) already has. Both go
            // to the resolver, which walks the pointer and lowers its target once per resolved
            // `file#pointer` with its own reservation, so a subschema that refers back to itself
            // or to its enclosing component is boxed against that reservation rather than
            // re-entered. A pointer that walks off the declared body is the resolver's to report.
            //
            // Only when the leading segment is declared: otherwise the fault is the missing
            // component, not the fragment's shape, and that keeps the plain-name wording below.
            let into_a_declared_component = name
                .split_once('/')
                .is_some_and(|(root, _)| self.document.components.schemas.contains_key(root));
            if into_a_declared_component {
                return self.ensure_resolved(&reference, at, name);
            }
            // The wording matches the parameter/request-body/response component arms, which
            // already reject.
            return self.reject_component_alias(at, "schema", &reference);
        };
        let RefOr::Item(schema) = component else {
            let reference = match component {
                RefOr::Ref(reference) => reference.clone(),
                RefOr::Item(_) => return None,
            };
            return self.chain_component_alias(name, &reference.reference, &reference.provenance);
        };
        // A `$ref` whose siblings bear no shape — `description`, `title`, a validation keyword such
        // as `maxLength` — is the same alias spelled with annotations beside it: the parser keeps any
        // sibling key as an inline schema, but `lower_schema_inner`'s `$ref` arm returns the TARGET
        // for it without inserting anything. Through the reserve/pop machinery below that is two
        // faults, one per declaration order: a target lowered earlier leaves this frame's
        // reservation as the last insert and the invariant assertion aborts the process; a target
        // lowered inside this frame is the last insert, so its def is lifted into this reservation
        // and the target's own component entry is left naming an id that no longer holds it.
        // Chaining exactly as the bare spelling does gives both the one answer that spelling gives,
        // cycle check included. The siblings are still acknowledged where they always were: the
        // audit reports an ignored validation keyword (`W001`) independently of lowering. A
        // `default` is the one sibling this frame used to carry (as a doc note on the root's own
        // def); an alias has no def to carry it, so it is reported as `W005` in the parser's words
        // for the bare `$ref`+`default` spelling rather than dropped silently.
        if let Some(reference) = schema.reference.as_deref() {
            let mut sibling = schema.clone();
            sibling.reference = None;
            if !schema_has_shape_constraint(&sibling) {
                // The alias never reaches `lower_schema_inner`, which reports this elsewhere.
                self.diagnose_standalone_discriminator(schema);
                if let Some(default) = &schema.default {
                    let at = crate::diag::Provenance::new(
                        schema.provenance.pointer.push("default"),
                        Some(default.span),
                    );
                    Diagnostic::warning(Code::SchemaDefaultNotApplied, at)
                        .message(
                            "a schema `default` declared alongside `$ref` is dropped when the \
                             reference resolves and is not applied",
                        )
                        .remedy(
                            "move the default onto the referenced schema, or set the value \
                             explicitly",
                        )
                        .emit(self.diags);
                }
                let reference = reference.to_owned();
                let provenance = schema.provenance.clone();
                return self.chain_component_alias(name, &reference, &provenance);
            }
        }
        // A component whose whole body is `oneOf`/`anyOf` over one `$ref` and one or more `null`
        // members names no shape of its own: it is a **nullable alias** for its target, the union
        // spelling of `B: {$ref: A}` with a null branch added. Recognised here, before anything is
        // reserved, and only while the target's own body is still being lowered.
        //
        // That is mutual recursion — `A.b: {$ref: B}` with `B: {oneOf: [{$ref: A}, {type: "null"}]}`
        // — one of the commonest recursive spellings there is. The union then collapses to the
        // target's reservation and has no def to hand back as this component's root: cloning the
        // reservation's kind inserts a second reservation nothing fills, and returning the
        // reservation itself breaks the last-insert invariant asserted below. Chaining to the
        // target, exactly as the bare-`$ref` alias arm above does, sidesteps both and yields the
        // `Option<Box<A>>` the direct spelling already yields.
        if let Some(alias) = self.nullable_alias_back_edge(schema) {
            return Some(alias);
        }
        self.lower_into_reservation(Reservation::Component(name.to_owned()), schema, name)
    }

    /// The two memos `reservation`'s frame keeps, each keyed by [`Reservation::key`]: the types it
    /// has finished, and the reserved ids (with their provisional nullability) of the bodies it is
    /// still lowering.
    fn reservation_memos(
        &mut self,
        reservation: &Reservation,
    ) -> (&mut ReservationMemo, &mut ReservationMemo) {
        match reservation {
            Reservation::Component(_) => (&mut self.components, &mut self.in_progress),
            Reservation::Remote(_) => (&mut self.remote_components, &mut self.remote_in_progress),
            Reservation::Resolved(_) => (
                &mut self.resolved_components,
                &mut self.resolved_in_progress,
            ),
        }
    }

    /// The reserved id and provisional nullability of `reservation` when its body is being
    /// lowered right now, recording that the provisional value was read: a body that then decides
    /// otherwise makes this pass stale ([`Self::settle_reservation`]).
    fn read_open_reservation(&mut self, reservation: Reservation) -> Option<(TypeId, bool)> {
        let entry = self
            .reservation_memos(&reservation)
            .1
            .get(reservation.key())
            .copied();
        if entry.is_some() {
            self.guessed.insert(reservation);
        }
        entry
    }

    /// The type `reservation` already answers with, before anything is lowered for it: its
    /// finished type, or — when it is re-entered while its own body is still being lowered — a
    /// cycle-closing back-edge. The back-edge is boxed so the recursive type has a finite size
    /// instead of being rejected; the reserved id holds the root def once the in-progress body
    /// finishes, and its nullability is provisional, so the read is recorded for the body to check
    /// when it finishes. `None` for a reservation never opened.
    fn reserved_type(&mut self, reservation: Reservation) -> Option<Ty> {
        if let Some(&(id, nullable)) = self
            .reservation_memos(&reservation)
            .0
            .get(reservation.key())
        {
            return Some(Ty {
                id,
                nullable,
                boxed: false,
            });
        }
        let (id, nullable) = self.read_open_reservation(reservation)?;
        Some(Ty {
            id,
            nullable,
            boxed: true,
        })
    }

    /// Lower `schema`, the body `reservation` names, once: reserve its root id, lower the body
    /// against that reservation, lift the root def into the reserved slot, and record the finished
    /// type in the frame's memo. The one lifecycle a root component, a remote target and a bundle
    /// target share; `hint` names the root def.
    fn lower_into_reservation(
        &mut self,
        reservation: Reservation,
        schema: &Schema,
        hint: &str,
    ) -> Option<Ty> {
        // A PROVISIONAL answer, needed before the body finishes so a back-edge encountered mid-body
        // has something to carry. It is not the final one: `schema_is_nullable` is three disjuncts
        // over `types`, `enum_values` and `const_value` and never looks at `oneOf`/`anyOf`/`$ref`/
        // `allOf`, so for any composed body it is a guess. Writing it back over the lowered result
        // discarded every decision `lower_union` makes about null the moment a union was spelled as
        // a named component — the dominant spelling in real descriptions. A guess a back-edge read
        // and the body then contradicted is reported by `settle_reservation`, and the next pass
        // opens this reservation with the body's answer instead (see [`lower`]).
        let provisional_nullable = self.provisional_nullability(&reservation, schema);
        // Reserve the root id before lowering the body so any back-edge encountered mid-body can
        // box a reference to it. The root's def is inserted last (children first) and then lifted
        // into this reserved slot, which keeps ids dense and stable.
        let root_id = self.graph.reserve();
        self.reservation_memos(&reservation).1.insert(
            reservation.key().to_owned(),
            (root_id, provisional_nullable),
        );
        let lowered = self.lower_reserved_body(schema, hint);
        self.reservation_memos(&reservation)
            .1
            .remove(reservation.key());
        self.settle_reservation(
            reservation.clone(),
            provisional_nullable,
            lowered.map(|ty| ty.nullable),
        );
        let mut ty = lowered?;
        let noun = reservation.noun();
        let (popped_id, mut def) = self
            .pop_last_type()
            .unwrap_or_else(|| panic!("{noun} root def"));
        // Hard invariant (release too): a reserved root's def is always the last graph insert
        // during its own body lowering (children insert first). If future lowering (allOf/union
        // wrappers) ever inserts a derived type *after* the root, this fails loudly here instead
        // of silently relocating the wrong def and dangling the memo's entry.
        assert_eq!(
            popped_id, ty.id,
            "{noun} root was not the last inserted def"
        );
        // A `default` on the reserved schema itself has no field to carry it; document it on the
        // named type's rustdoc rather than dropping it. (A component that is a bare `$ref`+`default`
        // never reaches here — it parses to `RefOr::Ref` and is acknowledged as W005 at parse time
        // — so this only sees inline schemas.) Pure pop-then-mutate: no graph insert happens here,
        // so the last-insert invariant asserted above still holds.
        if let Some(raw) = &schema.default {
            let note = format!("Default: `{}`.", default_display_for(raw, Some(&def.kind)));
            append_doc_note(&mut def.docs, note);
        }
        self.graph.fill(root_id, def);
        ty.id = root_id;
        // The BODY's answer, not the provisional one: a composed body knows things
        // `schema_is_nullable` cannot see, and naming a schema must not change what it means. Cached
        // under the same value, so a direct return and a later cache hit still yield an identical
        // `Ty`.
        let key = reservation.key().to_owned();
        self.reservation_memos(&reservation)
            .0
            .insert(key, (root_id, ty.nullable));
        Some(ty)
    }

    /// The nullability a reservation opens with: an earlier pass's settled answer for it when there
    /// is one, [`schema_is_nullable`]'s guess otherwise.
    fn provisional_nullability(&self, reservation: &Reservation, schema: &Schema) -> bool {
        self.settled
            .get(reservation)
            .copied()
            .unwrap_or_else(|| schema_is_nullable(schema))
    }

    /// Close a reservation's nullability bookkeeping once its body is lowered: when a back-edge read
    /// the `provisional` value and the body decided otherwise, record the body's answer, which makes
    /// this pass stale (see [`lower`]). A body that failed to lower decides nothing.
    ///
    /// [`lower`]: super::lower
    fn settle_reservation(
        &mut self,
        reservation: Reservation,
        provisional: bool,
        lowered: Option<bool>,
    ) {
        let read = self.guessed.remove(&reservation);
        if let Some(lowered) = lowered {
            if read && lowered != provisional {
                self.revisions.push((reservation, lowered));
            }
        }
    }

    /// Resolve the component `name`, whose root is an alias for `reference`, to the target's type and
    /// record it under `name`. An alias has no body of its own, so nothing is reserved for it; the
    /// alias stack is what makes a chain of aliases that loops back terminate, as `E004`.
    fn chain_component_alias(
        &mut self,
        name: &str,
        reference: &str,
        at: &crate::diag::Provenance,
    ) -> Option<Ty> {
        if !self.component_alias_stack.insert(name.to_owned()) {
            // E004 case: cycle
            Diagnostic::error(Code::UnresolvedRef, at.clone())
                .message(format!(
                    "schema component alias `{name}` forms a reference cycle"
                ))
                .emit(self.diags);
            return None;
        }
        let ty = self.ensure_reference(reference, at, name);
        self.component_alias_stack.remove(name);
        if let Some(ty) = ty {
            self.components
                .insert(name.to_owned(), (ty.id, ty.nullable));
        }
        ty
    }

    /// Acknowledge a sub-file's own component declaration that a same-named root declaration
    /// shadows.
    ///
    /// A JSON Pointer fragment addresses the document it appears in, so a `$ref` written inside
    /// `lib.yaml` as `#/components/schemas/Shared` asks for `lib.yaml`'s `Shared`. spargen consults
    /// the root document's component map first, so when the root declares the name too, the root's
    /// wins and the sub-file's declaration is never read.
    ///
    /// The precedence is kept — changing it would retype every split description that relies on it
    /// — but until now nothing said it. Before the sub-file branch existed the reference did not
    /// resolve at all, so only one of the two declarations was ever live and no choice had to be
    /// made; making the reference resolve makes the choice, and it is consequential: adding one
    /// unrelated component to the root document silently retargets a reference written in another
    /// file, and `spargen diff` across that pair reports a breaking change.
    ///
    /// `W011` is what the shadowed declaration is — a declaration with no effect — so no ordinal
    /// moves and the code keeps the matrix cell and `errors.md` row it already has. Emitted per
    /// reference site rather than once per name, because the site is what the reader has to find.
    ///
    /// `reference` is the `$ref` **as the site wrote it**, and only the bare-fragment spelling can
    /// be shadowed: `#/components/schemas/<name>` addresses the document it appears in, so writing
    /// it inside a sub-file that declares `<name>` asks for that file's declaration and is given
    /// the root's instead — which is the entire warning. A reference that names its own document
    /// (`./openapi.yaml#/components/schemas/<name>`) asked for one declaration and got that one:
    /// nothing is shadowed, the message would quote a spelling the site does not contain, and the
    /// remedy — "address the file-local one explicitly with a relative-file reference" — would tell
    /// the author to do what they have already done. `None` is the root's own pre-lowering pass,
    /// which walks declarations rather than references and has no spelling to judge.
    fn warn_if_root_shadows_the_referring_file(
        &mut self,
        name: &str,
        reference: Option<&str>,
        at: &crate::diag::Provenance,
    ) {
        if reference.and_then(|reference| reference.strip_prefix("#/components/schemas/"))
            != Some(name)
        {
            return;
        }
        let file = self.resolver.written_in(at);
        if file == self.resolver.root_id() || !self.document.components.schemas.contains_key(name) {
            return;
        }
        let Some(path) = self.resolver.declares_locally(file, name) else {
            return;
        };
        let message = format!(
            "`#/components/schemas/{name}` here reads the root document's `{name}`; the `{name}` \
             declared in `{path}` is shadowed by it and has no effect on this reference"
        );
        // W011 case: shadowed-component
        Diagnostic::warning(Code::DeclarationHasNoEffect, at.clone())
            .message(message)
            .remedy(
                "rename one of the two declarations, or address the file-local one explicitly with \
                 a relative-file reference, if the root's is not the one you meant",
            )
            .emit(self.diags);
    }

    /// The cycle-closing back-edge a **nullable alias** component resolves to, when its target's
    /// body is still being lowered.
    ///
    /// `Some` only for a body that is a `oneOf`/`anyOf` over exactly one bare `$ref` plus any
    /// number of null-only members, carrying no shape, discriminator, sibling `$ref` or `default`
    /// of its own — and only when that reference resolves to a target whose body is currently being
    /// lowered, in any of the three frames. The question is asked of the resolved *target*, not of
    /// the reference's spelling: see [`Self::open_reservation_for_ref`].
    ///
    /// Both halves of that narrowness are load-bearing. Recognising an alias whose target is
    /// **finished** would change what is generated for a document that already generates: the
    /// ordinary path re-emits the target's kind under this component's own name, and that named
    /// type is part of the published API, so deleting it is a breaking change to output with
    /// nothing wrong with it. And the answer is only true while the target is open, which is why
    /// nothing is written to [`Self::components`]: a cached hit returns `boxed: false`, and a second
    /// reference taken during the same cycle would then emit an infinitely sized type. Each
    /// reference re-derives it; the memo stays the target's own name.
    fn nullable_alias_back_edge(&mut self, schema: &Schema) -> Option<Ty> {
        if schema.default.is_some() || schema.discriminator.is_some() {
            return None;
        }
        let members = match (schema.one_of.is_empty(), schema.any_of.is_empty()) {
            (false, true) => &schema.one_of,
            (true, false) => &schema.any_of,
            // Neither, or both — the second is rejected by `lower_union` as an intersected
            // applicator and must reach it to be reported.
            _ => return None,
        };
        // Everything the component says apart from the union itself. A `type`, a `properties`, an
        // `enum`, a sibling `$ref`: anything at all makes it a constrained schema rather than
        // another name for its target.
        let mut without_union = schema.clone();
        without_union.one_of.clear();
        without_union.any_of.clear();
        if schema_has_shape_constraint(&without_union) {
            return None;
        }
        let mut real = members.iter().filter(|member| !member_is_null_only(member));
        let SchemaOr::Schema(only) = real.next()? else {
            return None;
        };
        if real.next().is_some() {
            return None;
        }
        let target = only.reference.as_deref()?;
        // A bare reference and nothing else: a member carrying its own keywords is a `$ref` with
        // siblings, which is an intersection and not an alias.
        let mut member_without_ref = only.clone();
        member_without_ref.reference = None;
        if schema_has_shape_constraint(&member_without_ref) {
            return None;
        }
        let (id, target_nullable) = self.open_reservation_for_ref(target, &only.provenance)?;
        Some(Ty {
            id,
            // The target's *own* nullability is as much a fact about this alias as a `"null"`
            // member is. `ensure_component` computes it at reserve time so that every `$ref`
            // consumer agrees on it without waiting for the body to finish, and reading only the
            // members disagrees: an alias with no `"null"` member whose target is nullable emitted
            // a non-`Option` field where the direct `{$ref: T}` spelling of that same target
            // emitted an optional one.
            nullable: target_nullable || members.iter().any(member_is_null_only),
            // The target is mid-lowering, so this is a cycle-closing reference and needs the box
            // for the recursive type to have a finite size.
            boxed: true,
        })
    }

    /// The still-open reservation `reference`, written at `at`, refers to — under any spelling.
    ///
    /// A `$ref` is identified by the target it resolves to, not by the characters used to write it.
    /// The three in-progress maps are each keyed by a different spelling of that identity, so this
    /// mirrors [`Self::lower_schema`]'s own `$ref` dispatch exactly: whichever `ensure_*` the
    /// reference would be lowered through is the map consulted for it. Keying on the literal
    /// `#/components/schemas/` prefix instead made a target's identity depend on the reference
    /// site's spelling, which is how one schema came to be both an alias and an unrepresentable
    /// shape in the same document.
    ///
    /// `None` for a target that is finished, absent, or was never a reservation — every one of
    /// which the ordinary lowering path handles and reports for itself. It resolves no node and
    /// lowers nothing, so asking costs the lowering that follows nothing; the one thing it does
    /// besides look up is raise the shadowed-component `W011` when it answers `Some` for a name
    /// a sub-file also declares, because answering `Some` is answering *instead of*
    /// [`Self::ensure_component`], which is where that warning otherwise lives.
    fn open_reservation_for_ref(
        &mut self,
        reference: &str,
        at: &Provenance,
    ) -> Option<(TypeId, bool)> {
        if let Some(name) = reference.strip_prefix("#/components/schemas/") {
            // A root component the root document declares: `ensure_component`'s own key, and its
            // own precedence — root map first, and only a name the root does **not** declare is
            // handed to `ensure_resolved` against the referring file.
            //
            // The gate is the *declaration*, not the reservation. Falling through whenever the
            // name is merely not open resolves it against the referring file, which for a
            // reference written inside a sub-file finds that file's own declaration — the one the
            // root shadows, and one `lower_schema` would never have bound. That answered the same
            // reference string two ways in one document: the direct `{$ref: T}` spelling read the
            // root's component and raised `W011`, while the alias spelling silently read the
            // sub-file's and raised nothing at all.
            if self.document.components.schemas.contains_key(name) {
                // `None` when it is not currently open is the right answer and not a fall-through:
                // a finished or not-yet-started root component is exactly the case the ordinary
                // `ensure_component` path handles, and the case in which it must, because that is
                // where the shadowing warning is raised.
                //
                // Reading it here is also answering instead of `ensure_component`'s back-edge arm,
                // which is where a read of the provisional nullability is otherwise recorded.
                let entry = self.read_open_reservation(Reservation::Component(name.to_owned()));
                if entry.is_some() {
                    // Answering here is answering *instead of* `ensure_component`, which is where
                    // the shadowing is acknowledged. Say it on the way past, or a reference the
                    // root wins silently retargets a sub-file's own declaration — supported as the
                    // matrix describes, but unreported, which the matrix also promises against.
                    self.warn_if_root_shadows_the_referring_file(name, Some(reference), at);
                }
                return entry;
            }
        } else if is_remote_ref(reference) {
            // `ensure_remote` keys on the absolute URL, and a reference inside a vendored document
            // has already been rewritten absolute, so the reference *is* the key.
            return self.read_open_reservation(Reservation::Remote(reference.to_owned()));
        }
        let (file, pointer) = self.resolver.reference_identity(reference, at)?;
        // `ensure_resolved` routes a target inside the root's component map back to
        // `ensure_component`, whose identity is the name; ask the map that actually holds it.
        if let Some(name) = self.root_component_at(file, &pointer) {
            return self.read_open_reservation(Reservation::Component(name.to_owned()));
        }
        self.read_open_reservation(Reservation::Resolved(format!("{}#{}", file.0, pointer)))
    }

    /// Lower a remote (`http`/`https`) `$ref` to a shared, cycle-safe type — the remote analogue of
    /// [`Self::ensure_component`], keyed by the absolute `url#fragment`. Resolution is hermetic (the
    /// schema comes from the vendored, hash-pinned copy already in the bundle; no network). A remote
    /// ref re-entered while its own body is still lowering — a self- or mutually-recursive vendored
    /// schema — returns a boxed back-edge against the reserved root id, so recursion terminates and
    /// generates a finite (boxed) type instead of overflowing the stack.
    pub(super) fn ensure_remote(&mut self, reference: &str) -> Option<Ty> {
        self.closed_narrowing(|ctx| ctx.ensure_remote_closed(reference))
    }

    /// [`Self::ensure_remote`]'s body, run with `open_narrowing` out of effect, as for a component.
    fn ensure_remote_closed(&mut self, reference: &str) -> Option<Ty> {
        if let Some(ty) = self.reserved_type(Reservation::Remote(reference.to_owned())) {
            return Some(ty);
        }
        let resolved = self
            .resolver
            .resolve(reference, &self.document.provenance, self.diags)
            .ok()?;
        let schema = resolved.schema.into_owned();

        // A vendored document that is itself a bare `$ref` is an alias with no body to reserve a
        // root for. Chain to its target under a cycle guard (so an alias loop terminates) rather
        // than through the reserve/pop machinery, which assumes the body inserts a fresh root.
        if schema.reference.is_some() {
            if !self.remote_alias_stack.insert(reference.to_owned()) {
                // E004 case: cycle
                Diagnostic::error(Code::UnresolvedRef, self.document.provenance.clone())
                    .message(format!("remote $ref `{reference}` forms an alias cycle"))
                    .emit(self.diags);
                return None;
            }
            let ty = self.lower_schema(&schema, reference);
            self.remote_alias_stack.remove(reference);
            return ty;
        }
        // The union spelling of that same alias: a body that is `oneOf`/`anyOf` over one `$ref`
        // back into a frame still being lowered, plus `null`. It has no more shape of its own than
        // the bare `$ref` above does, and — exactly as above — no body to reserve a root for: the
        // collapse resolves it to the target's reservation, which is not this frame's, so the
        // `pop_last()` below would lift the wrong def and the `assert_eq!` after it would abort the
        // process. Recognised here, before anything is reserved, so the frame never opens and there
        // is nothing to pop. `ensure_component` has always done this; the other two frames had the
        // guard only downstream, where it could not see a remote frame's reservations.
        if let Some(alias) = self.nullable_alias_back_edge(&schema) {
            return Some(alias);
        }
        self.lower_into_reservation(
            Reservation::Remote(reference.to_owned()),
            &schema,
            reference,
        )
    }

    /// Lower a `$ref` the bundle resolver has to follow — a relative-file reference, a whole-file
    /// reference, a non-component fragment, or a sub-file's own `#/components/schemas/<name>` — to a
    /// shared, cycle-safe type. The bundle analogue of [`Self::ensure_component`] and
    /// [`Self::ensure_remote`], and it exists for the reason `ensure_remote` states: `resolve` parses
    /// a *fresh owned schema* on every call, so a bundle reference has no `document`-level identity
    /// the way a root component does.
    ///
    /// Without one, each reference site re-resolved and re-lowered its target from scratch. That is
    /// three faults at once, not one: a shared component became one generated type per *use* instead
    /// of per *declaration* (two public Rust types for one schema, two items in the `surface` semver
    /// surface); a reuse graph cost 2^N lowerings rather than N, so a 40-line two-file description
    /// produced no output and no diagnostic; and a recursive schema had nothing to terminate
    /// against but [`MAX_SCHEMA_DEPTH`], rejecting with `E014` a document `docs/support-matrix.md`
    /// promises is supported.
    ///
    /// The identity is the resolved target's own `file#pointer` ([`resolved_identity`]), read from
    /// the parsed schema's provenance rather than from the `$ref` spelling, so every way of writing
    /// one target lands on one key: a sub-file's bare `#/components/schemas/Inner` and the root's
    /// `./lib.yaml#/components/schemas/Inner` resolve to the same file and pointer and share one
    /// type. Keying on the spelling — or on `(file, name)` — would give one target two identities,
    /// which is how these two paths came to behave differently in the first place. A target inside
    /// the root document's own component map is routed back to [`Self::ensure_component`] for the
    /// same reason: that map is already its identity, and a second one beside it would re-create the
    /// divergence in a new place.
    ///
    /// **Cycle safety.** The reservation is inserted *before* the body is lowered, so any re-entry
    /// on the same key — self-recursion, mutual recursion, or a diamond — finds it and returns a
    /// boxed back-edge instead of descending again. Every cycle therefore closes in one step. A
    /// chain of bare-`$ref` aliases has no body to reserve against and is guarded separately by
    /// [`Self::resolved_alias_stack`], exactly as `remote_alias_stack` guards the remote one. Only
    /// genuinely new targets descend, so the depth counter still bounds a real chain (`E014`) and
    /// nothing repeated can accumulate against it.
    ///
    /// **Semver.** spargen's semver surface is the public API of *generated output*, and this
    /// changes it for any description that reaches a target through more than one reference. Types
    /// that existed only because one schema was lowered once per use site are gone, and a type is
    /// now named for the schema it resolves from rather than for whichever site reached it first.
    /// `surface`'s own classifier calls a removed public item `ChangeKind::TypeRemoved`, which its
    /// impact policy grades **Major**, so regenerating against an unchanged description can stop
    /// compiling a consumer that named one of the removed types. That is the correct outcome — the
    /// removed types were artefacts of lowering the same schema repeatedly — but it is a breaking
    /// change to the generated API and is released as one.
    ///
    /// [`MAX_SCHEMA_DEPTH`]: super::MAX_SCHEMA_DEPTH
    pub(super) fn ensure_resolved(
        &mut self,
        reference: &str,
        at: &Provenance,
        hint: &str,
    ) -> Option<Ty> {
        self.closed_narrowing(|ctx| ctx.ensure_resolved_closed(reference, at, hint))
    }

    /// [`Self::ensure_resolved`]'s body, run with `open_narrowing` out of effect, as for a
    /// component.
    fn ensure_resolved_closed(
        &mut self,
        reference: &str,
        at: &Provenance,
        hint: &str,
    ) -> Option<Ty> {
        let resolved = self.resolver.resolve(reference, at, self.diags).ok()?;
        let schema = resolved.schema.into_owned();
        let Some(key) = resolved_identity(&schema.provenance) else {
            // No span, so no identity to key on. Lower it un-deduplicated rather than share a type
            // under a key that does not identify it: a duplicated type is wrong, a wrongly shared
            // one is worse. Every schema the parser produces carries a span, so this is defensive.
            return self.lower_schema(&schema, hint);
        };
        // A resolved target that is a root component already has an identity — its name. An
        // unspanned target was never read from a file (`key` above requires the span), so it is
        // never one.
        if let Some(name) = schema
            .provenance
            .span
            .and_then(|span| self.root_component_at(span.file, &schema.provenance.pointer))
        {
            return self.ensure_component(name, Some(reference), at);
        }
        if let Some(ty) = self.reserved_type(Reservation::Resolved(key.clone())) {
            return Some(ty);
        }
        // Name the type for the schema it came from, not for whichever use site reached it first:
        // once one type serves every site, a per-site hint would make the generated name depend on
        // lowering order. A whole-file reference has no final pointer token, so the caller's hint
        // stands there.
        let hint = resolved_hint(&schema.provenance, hint);

        // A target that is itself a bare `$ref` is an alias with no body to reserve a root for.
        // Chain to its target under a cycle guard rather than through the reserve/pop machinery,
        // which assumes the body inserts a fresh root.
        if schema.reference.is_some() {
            if !self.resolved_alias_stack.insert(key.clone()) {
                return self.reject_schema_alias_cycle(at.clone(), reference);
            }
            let ty = self.lower_schema(&schema, &hint);
            self.resolved_alias_stack.remove(&key);
            return ty;
        }
        // The union spelling of that same alias — see the matching arm in [`Self::ensure_remote`].
        // This is the split-description case: a sub-file whose `MaybeNode` is nothing but "a
        // `Node`, or null", which is the namespace shape issue #107 exists to make resolve.
        if let Some(alias) = self.nullable_alias_back_edge(&schema) {
            return Some(alias);
        }
        self.lower_into_reservation(Reservation::Resolved(key), &schema, &hint)
    }

    /// Whether this definition is a named component root — reachable by name from anywhere else in
    /// the document, rather than owned by the single use site that produced it.
    pub(super) fn is_component_root(&self, id: TypeId) -> bool {
        self.components
            .values()
            .chain(self.in_progress.values())
            .chain(self.remote_components.values())
            .chain(self.remote_in_progress.values())
            .chain(self.resolved_components.values())
            .chain(self.resolved_in_progress.values())
            .any(|&(root, _)| root == id)
    }

    /// Whether `id` is a reservation whose body is still being lowered, so its definition is the
    /// placeholder [`TypeGraph::reserve`] inserted rather than the schema's own shape.
    ///
    /// This matters because [`Self::push_ref_member`] classifies an `allOf` member by reading
    /// `graph.get(id).kind`. That kind is now [`TypeKind::Reserved`] — it was `TypeKind::Any` until
    /// the dedicated variant landed, which is why reading one answered "scalar" for a type that is
    /// not a scalar and the member silently became `serde_json::Value`. `push_ref_member` now names
    /// `Reserved` in its own `match` and refuses it, so the refusal lives in the function rather
    /// than in each caller. Every id in the three in-progress maps is such a placeholder, and the
    /// only safe thing to do with one is refuse to read it.
    ///
    /// This asks "is `id` **any** open reservation". A caller that needs "is `id` the reservation
    /// belonging to the schema at *this* provenance" wants [`Self::reservation_at`] instead; the two
    /// coincide only when the construct being lowered is the component's whole body, and confusing
    /// them rejects every recursive schema whose reference sits inside a property.
    ///
    /// [`TypeGraph::reserve`]: crate::ir::TypeGraph::reserve
    pub(super) fn is_in_progress_root(&self, id: TypeId) -> bool {
        self.in_progress
            .values()
            .chain(self.remote_in_progress.values())
            .chain(self.resolved_in_progress.values())
            .any(|&(root, _)| root == id)
    }

    /// Whether the graph currently holds `id` as a [`TypeKind::Reserved`] placeholder.
    ///
    /// A third question, narrower than [`Self::is_in_progress_root`] in one way and wider in
    /// another: it asks what the graph *holds* rather than which maps are open, so it answers for a
    /// reservation taken by any of the three in-progress maps without having to name them, and it
    /// answers `false` for an id whose body has since been filled. It exists so a caller can refuse
    /// to **clone** a placeholder's kind: a clone inserts a second reservation that nothing will
    /// ever `fill`, and `Api::check_invariants` reports that as `E011` against a document that is
    /// not malformed.
    pub(super) fn is_reservation(&self, id: TypeId) -> bool {
        matches!(
            self.graph.get(id).map(|def| &def.kind),
            Some(TypeKind::Reserved)
        )
    }

    /// The reserved id of the schema *at* `provenance`, when that schema is one whose body is
    /// currently being lowered.
    ///
    /// This answers a strictly narrower question than [`Self::is_in_progress_root`], and the
    /// difference matters. `is_in_progress_root` answers "is this id **any** open reservation";
    /// this answers "is the schema written **here** the one that reservation belongs to". They
    /// coincide only when the construct being lowered *is* the component's whole body, which is why
    /// a guard that needs the second and asks the first over-rejects every case where a recursive
    /// reference is nested inside a property rather than being the component itself.
    pub(super) fn reservation_at(&self, provenance: &Provenance) -> Option<TypeId> {
        if let Some(key) = resolved_identity(provenance) {
            if let Some(&(id, _)) = self.resolved_in_progress.get(&key) {
                return Some(id);
            }
            // A remote frame keys on the URL it was reached by rather than on its target's
            // `file#pointer`, so a string comparison against this provenance can never match one.
            // Canonicalise its keys to the same identity instead of reconstructing a URL from a
            // file id: a URL is only one of the spellings that reaches a vendored document, and
            // the map holds at most one entry per open recursion frame. Omitting this frame is what
            // let a remote body whose union collapses onto an open remote reservation past the
            // guard below, to be aborted by `TypeDefs::fill`'s `fill of an unreserved id` instead
            // of reported.
            //
            // That abort is a `debug_assert!`, so it is an enforcement point that degrades: it
            // holds under `cargo test` and not in a consumer's release `build.rs`. Both halves are
            // measured with this loop deleted. Debug assertions on: the process aborts at
            // `TypeDefs::fill`. Debug assertions off: it neither aborts nor emits — `fill` writes
            // the unreserved id, the reservation survives, and `check_invariants` rejects with
            // `E011`, "type `` is still a reservation, so its body was never lowered", naming no
            // type and carrying no pointer. So the release outcome is a poor diagnostic rather
            // than silent wrong output, and the second net is `check_invariants`, not `fill`.
            //
            // Only one shape reaches here: a vendored document whose **whole body** is the union,
            // because only then does the provenance canonicalise to a frame's own `file#pointer`.
            // `remote::a_vendored_remote_schema_that_is_a_union_over_itself_is_rejected` is that
            // document and the only thing in the suite that executes this loop; deleting the loop
            // turns it red. Every other remote recursion sits at a property or an `allOf` member,
            // whose pointer is not the frame's, so it reaches here and matches nothing.
            for (reference, &(id, _)) in &self.remote_in_progress {
                let matches = self
                    .resolver
                    .reference_identity(reference, &self.document.provenance)
                    .is_some_and(|(file, pointer)| key == format!("{}#{}", file.0, pointer));
                if matches {
                    return Some(id);
                }
            }
        }
        // A target inside the root document's component map has its identity there instead —
        // `ensure_resolved` routes such a reference back to `ensure_component` — so consult that
        // map too, or a root component addressed by file reference escapes the check.
        provenance
            .span
            .and_then(|span| self.root_component_at(span.file, &provenance.pointer))
            .and_then(|name| self.in_progress.get(name))
            .map(|&(id, _)| id)
    }
}
