//! Choosing the one `content` entry a body is generated from, and the `W014` naming what it
//! passed over.

use indexmap::IndexMap;

use crate::diag::{Code, Diagnostic, Diagnostics};
use crate::ir::MediaType;
use crate::oas31::media::{classify_media, classify_media_range, media_essence};

pub(super) fn lower_media_type(
    media: &str,
    provenance: &crate::diag::Provenance,
    diags: &mut Diagnostics,
) -> Option<MediaType> {
    let essence = media_essence(media);
    match classify_media(essence) {
        Some((media, _)) => Some(media),
        None => {
            Diagnostic::error(Code::UnsupportedMediaType, provenance.clone())
                .message(format!("media type `{essence}` is not supported"))
                .emit(diags);
            None
        }
    }
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

/// `opaque` answers, without lowering anything, whether an entry's body constrains nothing — the
/// proof that an ignored alternative would decode exactly like the selection.
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
        // selected and rejected by `lower_request_body`, never silently skipped.
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
        // An alternative that decodes to the very same thing narrows nothing, though. An
        // opaque-octets body that constrains nothing is `bytes::Bytes`, so a ranged media response
        // offering `video/*`, `image/png` and `application/octet-stream` gives up nothing by
        // generating one of them, and saying otherwise is noise on a common shape.
        //
        // The rule is deliberately confined to octet-stream, and the confinement is load-bearing
        // rather than conservatism waiting to be relaxed. Octet-stream is the one codec whose gate
        // collapses every body it admits onto a single type: `opaque_octets` maps an absent or
        // empty schema to `Bytes`, `format: binary` and `contentEncoding: base64` are `Bytes`, and
        // anything else is rejected outright. No other codec has that property, and three things
        // go wrong the moment the rule is widened on the strength of "both sides constrain
        // nothing":
        //
        // - *Constrains nothing* has two spellings that do not agree outside this gate. A media
        //   type with no `schema` at all lowers to `()`; one with `schema: {}` lowers to `Any`,
        //   i.e. `serde_json::Value`. Suppressing between them makes the *order* of two content
        //   keys decide the response type, silently.
        // - `itemSchema` lives outside the body schema entirely. Two sequential entries can both
        //   constrain nothing and still stream different item types.
        // A *request* narrows at the wire whatever the types do, so nothing is suppressed there at
        // all. The chosen media key becomes the `Content-Type` verbatim, and a server documented as
        // accepting `application/octet-stream` and `video/*` is only ever sent the first — a real
        // narrowing even though both decode to `Bytes`. It is tempting to think ranges cannot reach
        // a request anyway, since one is rejected as a request `Content-Type` below; that rejection
        // fires on the media actually *selected*, and a suppressed alternative is never selected.
        //
        // "Constrains nothing" also has to be proved, not assumed from the media type: an
        // octet-classified alternative carrying an object schema would be *rejected* by the octet
        // gate, not turned into bytes, so suppressing it would be the silent fourth behavior
        // nothing is allowed.
        let suppressible =
            position == BodyPosition::Response && classified == MediaType::OctetStream;
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
    Diagnostic::error(Code::UnsupportedMediaType, provenance.clone())
        .message(format!("media type `{media}` is not supported"))
        .emit(diags);
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
