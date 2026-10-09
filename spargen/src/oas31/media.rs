//! Media type classification: a `content` key's essence, its parameters, and the IR
//! [`MediaType`] it decodes as. Pure string and Media Type Object predicates, shared by lowering,
//! the audit, and the SSE recognizer.

use indexmap::IndexMap;

use crate::ir::MediaType;

use super::{MediaTypeObject, RefOr};

/// Whether a Media Type Object constrains nothing about the body it describes.
///
/// Only ever asked of an octet-classified entry, where "constrains nothing" and every other body
/// the gate admits mean the same type, `bytes::Bytes`. It is deliberately not a general answer to
/// "do these two entries decode alike" — see the caller for why that question needs more than
/// this one does.
///
/// A `$ref` is never taken as opaque — proving it would mean resolving it here, and answering
/// "unknown" as "not opaque" only costs a warning that was already being reported. That holds for
/// both places a reference can appear: a `schema: {$ref: …}`, and a 3.2 Media Type Object that is
/// *itself* a Reference Object, which parses with `schema: None` and would otherwise take the
/// no-schema arm and be called opaque on the strength of a field the `$ref` spelling never sets.
pub(super) fn media_object_is_opaque(object: &MediaTypeObject) -> bool {
    // Destructured exhaustively, like the schema predicates it delegates to. A Media Type Object
    // carries four fields besides `schema` that can describe the body, and reading only `schema`
    // is how `itemSchema` — which is where a sequential media's item type actually lives — slipped
    // past this question entirely. A field added here must be classified, not silently ignored.
    let MediaTypeObject {
        reference,
        schema,
        item_schema,
        encoding,
        prefix_encoding,
        item_encoding,
        provenance: _,
    } = object;
    if reference.is_some()
        || item_schema.is_some()
        || !encoding.is_empty()
        || !prefix_encoding.is_empty()
        || item_encoding.is_some()
    {
        return false;
    }
    match schema {
        None => true,
        Some(RefOr::Item(schema)) => schema.constrains_nothing(),
        Some(RefOr::Ref(_)) => false,
    }
}

/// Whether a media type essence is a media type or range at all: exactly one `/` between two
/// RFC 6838 § 4.2 `restricted-name`s, with `*` allowed only as the whole key (`*/*`), as the whole
/// subtype (`type/*`), or in front of a structured syntax suffix (`application/*+json`, the range
/// over every subtype carrying that suffix).
///
/// [`classify_media`] asks this first, so no arm can accept a key on the strength of a prefix or a
/// suffix alone: not `text/plain/extra`, not `application/vnd.a/b+json`, and not the range `a/b/*`.
/// Parameters are already gone, because every caller passes [`media_essence`] output. A key that
/// fails is not a media type, so it classifies as nothing and takes the existing unsupported path:
/// `E009` when it is the only candidate, or an ignored alternative under `W014` otherwise. An
/// Encoding Object's `contentType` is asked this directly and is `E009` when it fails, since it is
/// sent verbatim even when it names no codec spargen has; its parameters are then held to
/// [`media_type_with_parameters`].
pub(super) fn media_type_is_well_formed(essence: &str) -> bool {
    /// `restricted-name = restricted-name-first *126restricted-name-chars` (RFC 6838 § 4.2). ASCII
    /// letters of either case are accepted; case sensitivity is left to the arms that match names.
    fn restricted_name(name: &str) -> bool {
        let bytes = name.as_bytes();
        matches!(bytes.first(), Some(first) if first.is_ascii_alphanumeric())
            && bytes.len() <= 127
            && bytes.iter().all(|byte| {
                byte.is_ascii_alphanumeric()
                    || matches!(
                        byte,
                        b'!' | b'#' | b'$' | b'&' | b'-' | b'^' | b'_' | b'.' | b'+'
                    )
            })
    }
    let Some((kind, subtype)) = essence.split_once('/') else {
        return false;
    };
    match (kind, subtype) {
        ("*", "*") => true,
        (_, "*") => restricted_name(kind),
        _ => {
            restricted_name(kind)
                && match subtype.strip_prefix("*+") {
                    Some(suffix) => restricted_name(suffix),
                    None => restricted_name(subtype),
                }
        }
    }
}

/// Whether a well-formed essence is a structured-suffix media **range** such as
/// `application/*+json`, the range over every subtype carrying that suffix.
///
/// Unlike `type/*` and `*/*`, which [`classify_media_range`] gives their own family codec, a suffix
/// range needs no codec of its own: the suffix arms of [`classify_media`] already read it the way
/// the suffix says. It is still a range, though, and so it can no more be a request's
/// `Content-Type` than `video/*` can.
pub(super) fn media_essence_is_suffix_range(essence: &str) -> bool {
    essence
        .split_once('/')
        .is_some_and(|(_, subtype)| subtype.starts_with("*+"))
}

/// The request body `content` entries `choose_media` may select from, plus the structured-suffix
/// ranges withheld from that choice.
///
/// A suffix range is withheld only while another entry is *sendable*: it classifies, it is neither
/// kind of range, and it is not streaming media. Then the range, which a request cannot send,
/// never outranks something it could. With no sendable sibling, nothing is withheld, the range is
/// selected as before, and the request range check refuses it. Keys keep their document order.
pub(super) fn request_media_candidates<T>(
    content: &IndexMap<String, T>,
) -> (IndexMap<String, &T>, Vec<&str>) {
    let suffix_range =
        |essence: &str| media_essence_is_suffix_range(essence) && classify_media(essence).is_some();
    let sendable = content.keys().any(|media| {
        let essence = media_essence(media);
        !suffix_range(essence)
            && classify_media_range(essence).is_none()
            && classify_media(essence)
                .is_some_and(|(classified, _)| classified.stream_framing().is_none())
    });
    let mut candidates = IndexMap::new();
    let mut withheld = Vec::new();
    for (media, value) in content {
        if sendable && suffix_range(media_essence(media)) {
            withheld.push(media.as_str());
        } else {
            candidates.insert(media.clone(), value);
        }
    }
    (candidates, withheld)
}

pub(super) fn media_essence(media: &str) -> &str {
    media.split(';').next().unwrap_or(media).trim()
}

/// The element of a comma-separated media type list a client sends: the first, ended by the first
/// comma outside an RFC 9110 § 5.6.4 quoted-string, so `text/plain; name="a, b"` is one element.
/// An unterminated quoted-string runs to the end of the list, where [`media_type_with_parameters`]
/// rejects it.
pub(super) fn first_list_element(list: &str) -> &str {
    let mut quoted = false;
    let mut escaped = false;
    for (index, byte) in list.bytes().enumerate() {
        if escaped {
            escaped = false;
            continue;
        }
        match byte {
            b'\\' if quoted => escaped = true,
            b'"' => quoted = !quoted,
            b',' if !quoted => return list[..index].trim(),
            _ => {}
        }
    }
    list.trim()
}

/// Why a media type's parameter list cannot be sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ParameterFault {
    /// Not `parameters = *( OWS ";" OWS [ parameter ] )` with
    /// `parameter = token "=" ( token / quoted-string )` (RFC 9110 §§ 5.6.6, 5.6.4).
    Malformed,
    /// Well-formed, but a quoted value the multipart transport's parser (`mime` 0.3, behind
    /// reqwest's `Part::mime_str`) refuses: empty, or holding a `"` (as a quoted-pair) or a tab.
    /// Carries the canonical form, for a caller that never sends the value.
    Unsendable(String),
}

/// A media type whose essence is already well-formed, with its parameter list checked against
/// RFC 9110 § 5.6.6 and re-serialized as `type/subtype; name=value; …`.
///
/// Names and values keep their spelling, quoted-strings included. What changes is only what the
/// grammar leaves free: whitespace around `;` and empty parameters (`text/plain;;a=b`) are
/// dropped, since the multipart transport's parser refuses whitespace before a `;` and an empty
/// parameter, both of which RFC 9110 admits.
pub(super) fn media_type_with_parameters(media: &str) -> Result<String, ParameterFault> {
    fn tchar(byte: u8) -> bool {
        byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'!' | b'#'
                    | b'$'
                    | b'%'
                    | b'&'
                    | b'\''
                    | b'*'
                    | b'+'
                    | b'-'
                    | b'.'
                    | b'^'
                    | b'_'
                    | b'`'
                    | b'|'
                    | b'~'
            )
    }
    fn ows(bytes: &[u8], mut at: usize) -> usize {
        while matches!(bytes.get(at), Some(b' ' | b'\t')) {
            at += 1;
        }
        at
    }
    fn token(bytes: &[u8], from: usize) -> Result<usize, ParameterFault> {
        let end = from
            + bytes[from..]
                .iter()
                .take_while(|byte| tchar(**byte))
                .count();
        if end == from {
            Err(ParameterFault::Malformed)
        } else {
            Ok(end)
        }
    }
    /// The end of the quoted-string opening at `from`, and whether the transport can send it.
    fn quoted_string(bytes: &[u8], from: usize) -> Result<(usize, bool), ParameterFault> {
        let mut sendable = true;
        let mut at = from + 1;
        loop {
            match bytes.get(at).copied() {
                None => return Err(ParameterFault::Malformed),
                Some(b'"') => return Ok((at + 1, sendable && at > from + 1)),
                Some(b'\\') => {
                    // quoted-pair = "\" ( HTAB / SP / VCHAR / obs-text )
                    match bytes.get(at + 1).copied() {
                        Some(b'\t' | b'"') => sendable = false,
                        Some(b' ' | 0x21..=0x7e | 0x80..=0xff) => {}
                        _ => return Err(ParameterFault::Malformed),
                    }
                    at += 2;
                }
                // qdtext = HTAB / SP / %x21 / %x23-5B / %x5D-7E / obs-text
                Some(b'\t') => {
                    sendable = false;
                    at += 1;
                }
                Some(b' ' | 0x21 | 0x23..=0x5b | 0x5d..=0x7e | 0x80..=0xff) => at += 1,
                Some(_) => return Err(ParameterFault::Malformed),
            }
        }
    }

    let essence = media_essence(media);
    let Some((_, parameters)) = media.split_once(';') else {
        return Ok(essence.to_owned());
    };
    let bytes = parameters.as_bytes();
    let mut canonical = essence.to_owned();
    let mut unsendable = false;
    let mut at = 0;
    loop {
        at = ows(bytes, at);
        match bytes.get(at) {
            None => break,
            Some(b';') => {
                at += 1;
                continue;
            }
            Some(_) => {}
        }
        let name_end = token(bytes, at)?;
        if bytes.get(name_end) != Some(&b'=') {
            return Err(ParameterFault::Malformed);
        }
        let value_start = name_end + 1;
        let value_end = if bytes.get(value_start) == Some(&b'"') {
            let (end, sendable) = quoted_string(bytes, value_start)?;
            unsendable |= !sendable;
            end
        } else {
            token(bytes, value_start)?
        };
        canonical.push_str("; ");
        canonical.push_str(&parameters[at..value_end]);
        at = ows(bytes, value_end);
        match bytes.get(at) {
            None => break,
            Some(b';') => at += 1,
            Some(_) => return Err(ParameterFault::Malformed),
        }
    }
    if unsendable {
        Err(ParameterFault::Unsendable(canonical))
    } else {
        Ok(canonical)
    }
}

/// Classify a content type into its wire codec and deterministic preference rank. Structured JSON
/// suffixes use the JSON codec; textual types use raw UTF-8 except for the two streaming framings.
/// GitHub's documented octocat representation is a textual vendor media type. Concrete members of
/// the `image`, `audio`, and `video` families are opaque octets, like the ranges naming them.
pub(super) fn classify_media(essence: &str) -> Option<(MediaType, u8)> {
    if !media_type_is_well_formed(essence) {
        return None;
    }
    if let Some(range) = classify_media_range(essence) {
        return Some(range);
    }
    let classified = match essence {
        "application/json" => (MediaType::Json, 0),
        media if media.starts_with("application/") && media.ends_with("+json") => {
            (MediaType::Json, 0)
        }
        "application/xml" | "text/xml" => (MediaType::Xml, 1),
        "multipart/form-data" => (MediaType::Multipart, 2),
        "application/x-www-form-urlencoded" => (MediaType::FormUrlEncoded, 3),
        "application/octet-stream" => (MediaType::OctetStream, 4),
        // The sequential kinds are matched before the `text/` prefix arm so `text/event-stream` is
        // a stream, not text; their rank (6) still sits below text (5).
        "text/event-stream" => (MediaType::EventStream, 6),
        "application/x-ndjson" | "application/jsonl" => (MediaType::Ndjson, 6),
        "application/json-seq" => (MediaType::JsonSequence, 6),
        media if media.starts_with("application/") && media.ends_with("+json-seq") => {
            (MediaType::JsonSequence, 6)
        }
        "application/octocat-stream" => (MediaType::Text, 5),
        media if media.starts_with("text/") => (MediaType::Text, 5),
        // Rank 9, the end of the ladder, is a concrete member of a binary family
        // (`classify_binary_family`): below every codec listed above and below both range ranks
        // (7 and 8), so on a response a family key is generated only when it is the sole key that
        // classifies; a request additionally prefers it to a range, which it cannot send
        // (`choose_media`).
        _ => return classify_binary_family(essence),
    };
    Some(classified)
}

/// Classify a media **range** — `type/*`, or `*/*` — which the specification permits as a `content`
/// key and which describes a whole family rather than one type.
///
/// `text/*` is the family read as raw UTF-8; every other family, `*/*` included, is opaque octets,
/// which is the only honest reading of "whatever this server detected". A range ranks below every
/// codec spargen has (`text/*` at 7, every other family at 8), so a concrete sibling outranks it —
/// with one deliberate exception on responses: a concrete `image`/`audio`/`video` member (9,
/// `classify_binary_family`) sits *below* both, because a response listing a range beside
/// `image/png` generated from the range before that family classified and must keep doing so. A
/// request never selects a range while a concrete key classifies (`choose_media`), since a range
/// is not a `Content-Type`.
///
/// The type before the slash must be present — `/*` names no family and stays unsupported — and is
/// matched case-insensitively, because media types are (RFC 9110 § 8.3.1) and reading `TEXT/*` as
/// binary would be silently wrong rather than loudly unsupported.
pub(super) fn classify_media_range(essence: &str) -> Option<(MediaType, u8)> {
    let family = essence
        .strip_suffix("/*")
        .filter(|family| !family.is_empty())?;
    Some(if family.eq_ignore_ascii_case("text") {
        (MediaType::Text, 7)
    } else {
        (MediaType::OctetStream, 8)
    })
}

/// Classify a concrete member of a family whose every subtype is an opaque payload — `image/jpeg`,
/// `audio/mpeg`, `video/mp4`. RFC 6838 registers `image`, `audio`, and `video` as top-level types
/// for non-textual data, so bytes is the only faithful reading of any member, exactly as it is for
/// the family's range (`image/*`); the octet gate still demands a schema that collapses to
/// `bytes::Bytes`. It sits at the very end of the ladder, below every other key spargen can
/// classify — octet-stream, text, the sequential kinds, and the ranges, `*/*` included — so on a
/// response a family key is generated only when it is the sole key that classifies, which is
/// exactly the shape #82 reports (`image/jpeg` as the only content key), and no response that
/// generated before the family rule existed changes its selection or body type. A request body is
/// the one place a family key outranks a range: a range cannot be sent as `Content-Type`, and
/// every such request was rejected before, so preferring the concrete key only turns a rejection
/// into a client.
///
/// `application/*` is deliberately not a family here: it mixes binary (`application/pdf`) with
/// textual (`application/sdp`, `application/sql`) subtypes, and reading SDP as bytes would be
/// silently wrong rather than loudly unsupported. For the same reason a subtype carrying an RFC
/// 6838 structured-syntax suffix (`image/svg+xml`, or any `+suffix`) is not claimed: the suffix
/// says the payload is a text syntax, so reading SVG as bytes would be silently wrong, and it stays
/// unsupported until a codec for the suffix exists in this position. The family is matched
/// case-insensitively for the same reason the range is (RFC 9110 § 8.3.1): `IMAGE/*` and
/// `IMAGE/JPEG` must agree.
fn classify_binary_family(essence: &str) -> Option<(MediaType, u8)> {
    let (family, subtype) = essence.split_once('/')?;
    if subtype.is_empty() || subtype.contains('*') || subtype.contains('+') {
        return None;
    }
    ["image", "audio", "video"]
        .iter()
        .any(|binary| family.eq_ignore_ascii_case(binary))
        .then_some((MediaType::OctetStream, 9))
}
