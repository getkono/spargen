//! Choosing the one `content` entry a body is generated from, and the `W014` naming what it
//! passed over.

use indexmap::IndexMap;

use crate::diag::{Code, Diagnostic, Diagnostics};
use crate::ir::MediaType;
use crate::oas31::media::{classify_media, classify_media_range, is_cbor, media_essence};

/// The codec a `content` key's essence classifies as, or `None` with an `E009` for one spargen
/// does not support.
pub(super) fn lower_media_type(
    media: &str,
    provenance: &crate::diag::Provenance,
    diags: &mut Diagnostics,
) -> Option<MediaType> {
    let essence = media_essence(media);
    match classify_media(essence) {
        Some((media, _)) => Some(media),
        None => {
            media_type_not_supported(essence, provenance, diags);
            None
        }
    }
}

/// Report `E009` for a media type spargen has no codec for, spelled as `spelling`: the essence
/// where one key is classified, the raw `content` key where none of them is.
fn media_type_not_supported(
    spelling: &str,
    provenance: &crate::diag::Provenance,
    diags: &mut Diagnostics,
) {
    // E009 case: no-codec
    Diagnostic::error(Code::UnsupportedMediaType, provenance.clone())
        .message(format!("media type `{spelling}` is not supported"))
        .emit(diags);
}

/// Where a body sits, which decides whether an alternative that decodes identically may go
/// unreported. A response narrows only at the type; a request narrows at the wire as well.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum BodyPosition {
    Request,
    Response,
}

/// The `content` entry [`choose_media`] selected, and the `W014` disclosing what it passed over.
pub(super) struct ChosenMedia<'a, T> {
    pub(super) media: &'a str,
    pub(super) value: &'a T,
    /// Built but not emitted. The narrowing `W014` discloses is only real once the caller's own
    /// gates accept the selection, so the caller emits this when — and only when — the selected
    /// entry lowers. A selection those gates then reject is reported by its `E009` alone.
    pub(super) narrowing: Option<Diagnostic>,
}

/// Select the one `content` entry a body is generated from, among the keys that classify as a
/// supported media type: the best-ranked, and the first in source order on a tie. In a request
/// a media range ranks after every concrete key, since `Content-Type` must be concrete; one that
/// is still selected is refused by the caller. A key that does not classify is skipped.
///
/// `None` with nothing reported for an empty `content`, and `None` with an `E009` naming the first
/// key when no key classifies. Otherwise the selection, with the `W014` naming every other key
/// built but not emitted ([`ChosenMedia::narrowing`]); `None` there when nothing else is offered.
/// A response whose selection classifies as octet-stream — `application/octet-stream`, a
/// non-text range such as `video/*`, or an `image`/`audio`/`video` member such as `image/png` —
/// leaves out of that `W014` each other octet-classified alternative `opaque` proves constrains
/// nothing, since it would decode exactly like the selection. `opaque` answers that without
/// lowering anything. A selected CBOR key is not exempt: it is sent as `Accept`, so its
/// alternatives are narrowed at the wire.
pub(super) fn choose_media<'a, T>(
    content: &'a IndexMap<String, T>,
    provenance: &crate::diag::Provenance,
    diags: &mut Diagnostics,
    position: BodyPosition,
    opaque: impl Fn(&T) -> bool,
) -> Option<ChosenMedia<'a, T>> {
    if content.is_empty() {
        return None;
    }
    let mut selected: Option<(bool, u8, usize, &str, &T, MediaType)> = None;
    for (source_index, (media, value)) in content.iter().enumerate() {
        let Some((classified, rank)) = classify_media(media_essence(media)) else {
            continue;
        };
        // A request sends its media key as `Content-Type`, which must be concrete (RFC 9110 § 8.3),
        // so a range is only a candidate there once no concrete key classifies — and then it is
        // selected and rejected by `lower_chosen_request_body`, never silently skipped.
        let unsendable = position == BodyPosition::Request
            && classify_media_range(media_essence(media)).is_some();
        let candidate = (
            unsendable,
            rank,
            source_index,
            media.as_str(),
            value,
            classified,
        );
        if selected.as_ref().is_none_or(|current| {
            (unsendable, rank, source_index) < (current.0, current.1, current.2)
        }) {
            selected = Some(candidate);
        }
    }
    if let Some((_, _, _, media, value, classified)) = selected {
        // A generated method sends and decodes exactly one media type, so the alternatives are not
        // generated. That narrows the documented surface — a server that also accepts XML will only
        // ever be sent JSON — so it is reported rather than dropped in silence.
        //
        // A response's octet-stream alternatives that constrain nothing are not reported: each
        // decodes to `bytes::Bytes`, the one type the octet gate (`opaque_octets`) admits, so
        // generating one of them gives up nothing. The rule stays confined to that case:
        //
        // - Elsewhere, *constrains nothing* has two spellings (no `schema` lowers to `()`,
        //   `schema: {}` to `Any`), so suppressing would let content-key order pick the type.
        // - `itemSchema` sits outside the body schema, so two sequential entries that constrain
        //   nothing can still stream different item types.
        // - A request's chosen media key is its `Content-Type`, so it narrows at the wire whatever
        //   the types do, and nothing is suppressed there.
        // - An octet alternative carrying an object schema is rejected by the octet gate, not
        //   decoded as bytes, so suppressing it would hide a rejection.
        // - A selected CBOR key's alternatives are always reported: the choice is made one
        //   response at a time, before the operation's `Accept` (`Responses::accept`) is known.
        let suppressible = position == BodyPosition::Response
            && classified == MediaType::OctetStream
            && !is_cbor(media_essence(media));
        let ignored: Vec<&str> = content
            .iter()
            .filter(|(candidate, _)| candidate.as_str() != media)
            .filter(|(candidate, candidate_value)| {
                !suppressible
                    || !opaque(candidate_value)
                    || classify_media(media_essence(candidate)).map(|(media, _)| media)
                        != Some(MediaType::OctetStream)
            })
            .map(|(candidate, _)| candidate.as_str())
            .collect();
        return Some(ChosenMedia {
            media,
            value,
            narrowing: alternative_media_ignored(media, &ignored, provenance),
        });
    }
    let (media, _) = content.first()?;
    media_type_not_supported(media, provenance, diags);
    None
}

/// The `W014` saying `media` is selected and `ignored` is not, or `None` when nothing was ignored.
/// Built rather than emitted: see [`ChosenMedia::narrowing`].
///
/// The message asserts only what is decided here — which entry was selected — and never that it
/// "is generated": whether anything is generated depends on the rest of the document and on the
/// entry point (`check` generates nothing), neither of which this site can see (#174).
pub(super) fn alternative_media_ignored(
    media: &str,
    ignored: &[&str],
    provenance: &crate::diag::Provenance,
) -> Option<Diagnostic> {
    if ignored.is_empty() {
        return None;
    }
    Some(
        Diagnostic::warning(Code::AlternativeMediaIgnored, provenance.clone())
            .message(format!(
                "`{media}` is selected; the alternative media type(s) `{}` are not",
                ignored.join("`, `")
            ))
            .remedy(
                "remove the alternatives, or omit this API segment with spargen::omit! and \
                 hand-write the call",
            )
            .build(),
    )
}
