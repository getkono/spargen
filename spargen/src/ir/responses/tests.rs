use super::{ApiErrorBodyImpl, ErrorShape, Response, Responses, StatusSpec, SuccessShape, Ty};
use crate::diag::{JsonPointer, Provenance};
use crate::ir::{
    AdditionalProps, Docs, Openness, Prim, ScalarEnum, ScalarRepr, ScalarValue, Struct, TypeDef,
    TypeGraph, TypeId, TypeKind,
};

fn ty(id: u32) -> Ty {
    Ty {
        id: TypeId(id),
        nullable: false,
        boxed: false,
    }
}

fn resp(body: Option<u32>) -> Response {
    Response {
        media: body.map(|_| super::MediaType::Json),
        body: body.map(ty),
        stream: None,
        headers: Vec::new(),
    }
}

fn stream_resp(body: Option<u32>) -> Response {
    Response {
        media: Some(super::MediaType::EventStream),
        body: body.map(ty),
        stream: Some(super::Framing::Sse),
        headers: Vec::new(),
    }
}

#[test]
fn entries_walk_by_status_in_order_then_default_and_get_finds_each() {
    let responses = Responses {
        by_status: vec![
            (StatusSpec::Exact(404), resp(Some(1))),
            (StatusSpec::Range(2), resp(Some(2))),
        ],
        default: Some(resp(Some(3))),
    };
    let walked: Vec<(StatusSpec, Option<Ty>)> = responses
        .entries()
        .map(|(status, response)| (status, response.body))
        .collect();
    assert_eq!(
        walked,
        [
            (StatusSpec::Exact(404), Some(ty(1))),
            (StatusSpec::Range(2), Some(ty(2))),
            (StatusSpec::Default, Some(ty(3))),
        ]
    );
    let body = |status| responses.get(status).and_then(|response| response.body);
    assert_eq!(body(StatusSpec::Exact(404)), Some(ty(1)));
    assert_eq!(body(StatusSpec::Range(2)), Some(ty(2)));
    assert_eq!(body(StatusSpec::Default), Some(ty(3)));
    assert!(responses.get(StatusSpec::Exact(200)).is_none());

    // Without a `default`, the walk is `by_status` alone and `get` has no `default` to find.
    let responses = Responses {
        by_status: vec![(StatusSpec::Exact(200), resp(None))],
        default: None,
    };
    let walked: Vec<StatusSpec> = responses.entries().map(|(status, _)| status).collect();
    assert_eq!(walked, [StatusSpec::Exact(200)]);
    assert!(responses.get(StatusSpec::Default).is_none());
}

#[test]
fn a_bodied_stream_is_admitted_only_as_the_single_bodied_success() {
    let case = |by_status: Vec<(StatusSpec, Response)>, default: Option<Response>| {
        Responses { by_status, default }.stream_outside_single_success()
    };
    // Supported: the single bodied success, beside a bodyless success and whole-body errors.
    assert!(!case(
        vec![
            (StatusSpec::Exact(200), stream_resp(Some(1))),
            (StatusSpec::Exact(204), resp(None)),
            (StatusSpec::Exact(404), resp(Some(2))),
        ],
        Some(resp(Some(3))),
    ));
    // A bodyless stream is never read, wherever it sits.
    assert!(!case(
        vec![
            (StatusSpec::Exact(200), resp(Some(1))),
            (StatusSpec::Range(4), stream_resp(None)),
        ],
        None,
    ));
    // A lone streaming `default` is the success source and also offered to the error side.
    assert!(case(Vec::new(), Some(stream_resp(Some(1)))));
    // So is one beside only non-2xx statuses.
    assert!(case(
        vec![(StatusSpec::Exact(404), resp(None))],
        Some(stream_resp(Some(1))),
    ));
    // A streaming error status, and a streaming `default` beside a declared success.
    assert!(case(
        vec![
            (StatusSpec::Exact(200), resp(Some(1))),
            (StatusSpec::Range(4), stream_resp(Some(2))),
        ],
        None,
    ));
    assert!(case(
        vec![(StatusSpec::Exact(200), resp(Some(1)))],
        Some(stream_resp(Some(2))),
    ));
    // A stream in a multi-status success enum.
    assert!(case(
        vec![
            (StatusSpec::Exact(200), stream_resp(Some(1))),
            (StatusSpec::Exact(202), resp(Some(2))),
        ],
        None,
    ));
}

#[test]
fn success_enum_sorts_exact_before_range_and_keeps_bodyless_unit() {
    // Document order lists the 2XX range BEFORE the exact 200 (and mixes in a bodyless 204).
    // Precedence must reorder to exact-before-range so a real HTTP 200 decodes into its exact
    // variant, not the overlapping range one; the bodyless 204 survives as a payload-free entry.
    let responses = Responses {
        by_status: vec![
            (StatusSpec::Range(2), resp(Some(1))),
            (StatusSpec::Exact(200), resp(Some(2))),
            (StatusSpec::Exact(204), resp(None)),
        ],
        default: None,
    };
    match responses.success() {
        SuccessShape::Enum(entries) => {
            let shape: Vec<_> = entries.iter().map(|(s, b)| (*s, b.is_some())).collect();
            assert_eq!(
                shape,
                vec![
                    (StatusSpec::Exact(200), true),
                    (StatusSpec::Exact(204), false),
                    (StatusSpec::Range(2), true),
                ]
            );
            // The exact 200 body (id 2) precedes the range 2XX body (id 1).
            assert_eq!(entries[0].1.unwrap().id, TypeId(2));
        }
        other => panic!("expected Enum, got {other:?}"),
    }
}

#[test]
fn error_enum_sorts_exact_before_range_before_default() {
    // Document order: range 4XX, then exact 409, then a default — all must reorder to
    // exact < range < default. A `Range(0)`, which the frontend never admits, is included to
    // pin that a range prefix of `0` is an ordinary range here, sorted among the ranges and
    // distinct from the `default` entry (issue #233: it once *was* the `default` entry).
    let responses = Responses {
        by_status: vec![
            (StatusSpec::Range(4), resp(Some(1))),
            (StatusSpec::Exact(409), resp(Some(2))),
            (StatusSpec::Range(0), resp(Some(4))),
        ],
        default: Some(resp(Some(3))),
    };
    match responses.error() {
        ErrorShape::Enum(entries) => {
            let specs: Vec<_> = entries.iter().map(|(s, _)| *s).collect();
            assert_eq!(
                specs,
                vec![
                    StatusSpec::Exact(409),
                    StatusSpec::Range(0),
                    StatusSpec::Range(4),
                    StatusSpec::Default,
                ]
            );
        }
        other => panic!("expected Enum, got {other:?}"),
    }
}

#[test]
fn success_enum_sorts_exact_codes_ascending_whatever_the_document_order() {
    // Issue #138: a key that orders only by class (exact, range, default) is a stable sort
    // that keeps document order within each class, so only exacts listed out of order can
    // observe "exact code ascending"; this case states it on purpose rather than leaving it
    // to a fixture that happens to list a `204` before a `200`. (The success side can hold at most one
    // range — `2XX` is the only success selector the frontend admits — so range order is
    // pinned on the error side below.)
    let responses = Responses {
        by_status: vec![
            (StatusSpec::Range(2), resp(Some(1))),
            (StatusSpec::Exact(204), resp(None)),
            (StatusSpec::Exact(201), resp(Some(2))),
            (StatusSpec::Exact(200), resp(Some(3))),
        ],
        default: None,
    };
    match responses.success() {
        SuccessShape::Enum(entries) => assert_eq!(
            statuses(&entries),
            vec![
                StatusSpec::Exact(200),
                StatusSpec::Exact(201),
                StatusSpec::Exact(204),
                StatusSpec::Range(2),
            ]
        ),
        other => panic!("expected Enum, got {other:?}"),
    }
}

#[test]
fn error_enum_sorts_exact_codes_then_ranges_ascending_whatever_the_document_order() {
    // Issue #138: exacts and ranges both in descending document order, `default` first. The
    // error side is where two ranges share one sort (`4XX` and `5XX` are both error
    // selectors), so this is the only place "range ascending" is observable.
    let responses = Responses {
        by_status: vec![
            (StatusSpec::Range(5), resp(Some(1))),
            (StatusSpec::Range(4), resp(Some(2))),
            (StatusSpec::Exact(409), resp(None)),
            (StatusSpec::Exact(404), resp(Some(3))),
        ],
        default: Some(resp(Some(4))),
    };
    match responses.error() {
        ErrorShape::Enum(entries) => assert_eq!(
            statuses(&entries),
            vec![
                StatusSpec::Exact(404),
                StatusSpec::Exact(409),
                StatusSpec::Range(4),
                StatusSpec::Range(5),
                StatusSpec::Default,
            ]
        ),
        other => panic!("expected Enum, got {other:?}"),
    }
}

#[test]
fn single_bodied_success_with_bodyless_sibling_is_an_enum() {
    // Issue #121: the common `T`-plus-`204` case is two documented outcomes. A plain `T` would
    // decode the `204`'s empty body as a malformed `T`, so the bodyless status gets its own
    // unit variant — whether the sibling is exact or a range, and on either side of the body.
    for (by_status, expected) in [
        (
            vec![
                (StatusSpec::Exact(204), resp(None)),
                (StatusSpec::Exact(200), resp(Some(1))),
            ],
            vec![
                (StatusSpec::Exact(200), true),
                (StatusSpec::Exact(204), false),
            ],
        ),
        (
            vec![
                (StatusSpec::Range(2), resp(Some(1))),
                (StatusSpec::Exact(204), resp(None)),
            ],
            vec![
                (StatusSpec::Exact(204), false),
                (StatusSpec::Range(2), true),
            ],
        ),
        (
            vec![
                (StatusSpec::Exact(200), resp(Some(1))),
                (StatusSpec::Range(2), resp(None)),
            ],
            vec![
                (StatusSpec::Exact(200), true),
                (StatusSpec::Range(2), false),
            ],
        ),
    ] {
        let responses = Responses {
            by_status,
            default: Some(resp(Some(2))),
        };
        match responses.success() {
            SuccessShape::Enum(entries) => {
                let shape: Vec<_> = entries.iter().map(|(s, b)| (*s, b.is_some())).collect();
                assert_eq!(shape, expected);
            }
            other => panic!("expected Enum, got {other:?}"),
        }
        // Still one body: the XML and streaming limits on a *second* body do not apply.
        assert!(!responses.multiple_success_bodies());
    }

    // A lone bodied success, and a bodied success beside only error statuses, stay plain.
    for by_status in [
        vec![(StatusSpec::Exact(200), resp(Some(1)))],
        vec![
            (StatusSpec::Exact(200), resp(Some(1))),
            (StatusSpec::Exact(404), resp(None)),
        ],
    ] {
        let responses = Responses {
            by_status,
            default: Some(resp(None)),
        };
        assert!(matches!(responses.success(), SuccessShape::Plain(_)));
    }
}

#[test]
fn every_bodyless_entry_beside_one_body_is_its_own_unit_variant() {
    // Issue #211: "each bodyless entry" in the plural. Two bodyless entries beside one body
    // are two unit variants on either side, neither merged into the other nor dropped, and
    // they sort among the bodied entry by precedence like any other entry.
    let responses = Responses {
        by_status: vec![
            (StatusSpec::Exact(205), resp(None)),
            (StatusSpec::Exact(200), resp(Some(1))),
            (StatusSpec::Exact(204), resp(None)),
            (StatusSpec::Range(5), resp(None)),
            (StatusSpec::Exact(404), resp(Some(2))),
            (StatusSpec::Exact(403), resp(None)),
        ],
        default: None,
    };
    match responses.success() {
        SuccessShape::Enum(entries) => {
            let shape: Vec<_> = entries
                .iter()
                .map(|(status, body)| (*status, body.map(|body| body.id.0)))
                .collect();
            assert_eq!(
                shape,
                vec![
                    (StatusSpec::Exact(200), Some(1)),
                    (StatusSpec::Exact(204), None),
                    (StatusSpec::Exact(205), None),
                ]
            );
        }
        other => panic!("expected Enum, got {other:?}"),
    }
    assert!(!responses.multiple_success_bodies());
    assert_eq!(
        shape(responses.error()),
        Shape::Enum(vec![
            (StatusSpec::Exact(403), None),
            (StatusSpec::Exact(404), Some(2)),
            (StatusSpec::Range(5), None),
        ])
    );
    assert!(!responses.multiple_error_bodies());
}

#[test]
fn a_streaming_body_beside_a_bodyless_sibling_stays_plain() {
    // An empty body is a well-formed empty stream, so the `204` needs no variant of its own;
    // the stream stays the operation's single success body and `EventStream<T>`.
    let responses = Responses {
        by_status: vec![
            (StatusSpec::Exact(200), stream_resp(Some(1))),
            (StatusSpec::Exact(204), resp(None)),
        ],
        default: None,
    };
    assert!(matches!(responses.success(), SuccessShape::Plain(body) if body.id == TypeId(1)));
    assert!(responses.stream_success().is_some());
    assert!(!responses.stream_outside_single_success());
}

#[test]
fn a_lone_xml_body_beside_a_bodyless_sibling_is_not_a_multi_body_xml_enum() {
    let xml = |body: Option<u32>| Response {
        media: body.map(|_| super::MediaType::Xml),
        ..resp(body)
    };
    // One XML body beside a bodyless `204`: an enum, but still one body to decode as XML.
    let responses = Responses {
        by_status: vec![
            (StatusSpec::Exact(200), xml(Some(1))),
            (StatusSpec::Exact(204), resp(None)),
        ],
        default: None,
    };
    assert!(matches!(responses.success(), SuccessShape::Enum(_)));
    assert!(!responses.xml_in_multi_status());
    // A second bodied success beside the XML one is still the rejected combination.
    let responses = Responses {
        by_status: vec![
            (StatusSpec::Exact(200), xml(Some(1))),
            (StatusSpec::Exact(201), resp(Some(2))),
            (StatusSpec::Exact(204), resp(None)),
        ],
        default: None,
    };
    assert!(responses.xml_in_multi_status());

    // The error side follows the same rule: one XML error body beside a bodyless `default` is
    // one body, and a second bodied error (here the `default`) makes the rejected combination.
    let errors = |default: Option<u32>| Responses {
        by_status: vec![
            (StatusSpec::Exact(200), resp(None)),
            (StatusSpec::Exact(404), xml(Some(1))),
        ],
        default: Some(resp(default)),
    };
    assert!(matches!(errors(None).error(), ErrorShape::Enum(_)));
    assert!(!errors(None).xml_in_multi_status());
    assert!(errors(Some(2)).xml_in_multi_status());

    // Several bodies with no XML among them are no XML combination, on either side.
    let json_only = Responses {
        by_status: vec![
            (StatusSpec::Exact(200), resp(Some(1))),
            (StatusSpec::Exact(201), resp(Some(2))),
            (StatusSpec::Exact(204), resp(None)),
            (StatusSpec::Exact(404), resp(Some(3))),
        ],
        default: Some(resp(Some(4))),
    };
    assert!(matches!(json_only.success(), SuccessShape::Enum(_)));
    assert!(matches!(json_only.error(), ErrorShape::Enum(_)));
    assert!(!json_only.xml_in_multi_status());
}

/// Each sequential media maps to its own framing, and every other media to none.
#[test]
fn only_the_sequential_media_have_a_stream_framing() {
    use super::{Framing, MediaType};
    let cases = [
        (MediaType::Json, None),
        (MediaType::FormUrlEncoded, None),
        (MediaType::Xml, None),
        (MediaType::OctetStream, None),
        (MediaType::Text, None),
        (MediaType::Multipart, None),
        (MediaType::EventStream, Some(Framing::Sse)),
        (MediaType::Ndjson, Some(Framing::Ndjson)),
        (MediaType::JsonSequence, Some(Framing::JsonSequence)),
    ];
    for (media, framing) in cases {
        assert_eq!(media.stream_framing(), framing, "{media:?}");
    }
}

#[test]
fn only_2xx_selectors_are_success_and_every_other_range_prefix_is_error() {
    // Issue #198: the predicate that splits every generated shape. The emitted method enters
    // its success branch on the transport's `StatusCode::is_success()` — exactly 200..=299 —
    // so this must agree with it at every code RFC 9110 defines a class for, and on every range.
    for code in 100..=599u16 {
        assert_eq!(
            StatusSpec::Exact(code).is_success(),
            (200..=299).contains(&code),
            "exact status {code}",
        );
    }
    for (prefix, success) in [(1, false), (2, true), (3, false), (4, false), (5, false)] {
        assert_eq!(
            StatusSpec::Range(prefix).is_success(),
            success,
            "range {prefix}XX"
        );
    }
    // `default` is never a success selector; it reaches the success side only through
    // `default_is_success_source`.
    assert!(!StatusSpec::Default.is_success());
}

/// The one prose spelling of a selector, which the generated error `Display` and
/// `spargen diff`'s labels share: the Responses Object key, `XX` upper-case.
#[test]
fn a_selector_displays_as_its_responses_object_key() {
    assert_eq!(StatusSpec::Exact(404).display_label(), "404");
    assert_eq!(StatusSpec::Range(5).display_label(), "5XX");
    assert_eq!(StatusSpec::Range(0).display_label(), "0XX");
    assert_eq!(StatusSpec::Default.display_label(), "default");
}

#[test]
fn a_documented_304_or_3xx_is_on_the_error_side_and_promotes_no_success_enum() {
    // A bodied `200` beside a bodyless `304`: the `304` is no success entry, so the lone body
    // stays plain `T` (a bodyless 2xx sibling would have made it an enum), and the `304` is an
    // error-side entry — a unit variant of the error enum beside one bodied error or several.
    let single = Responses {
        by_status: vec![
            (StatusSpec::Exact(200), resp(Some(1))),
            (StatusSpec::Exact(304), resp(None)),
            (StatusSpec::Exact(404), resp(Some(2))),
        ],
        default: None,
    };
    assert!(matches!(single.success(), SuccessShape::Plain(body) if body.id == TypeId(1)));
    assert_eq!(
        shape(single.error()),
        Shape::Enum(vec![
            (StatusSpec::Exact(304), None),
            (StatusSpec::Exact(404), Some(2)),
        ])
    );

    let multi = Responses {
        by_status: vec![
            (StatusSpec::Exact(200), resp(Some(1))),
            (StatusSpec::Range(3), resp(None)),
            (StatusSpec::Exact(304), resp(None)),
            (StatusSpec::Exact(404), resp(Some(2))),
            (StatusSpec::Range(5), resp(Some(3))),
        ],
        default: None,
    };
    assert!(matches!(multi.success(), SuccessShape::Plain(body) if body.id == TypeId(1)));
    match multi.error() {
        ErrorShape::Enum(entries) => {
            assert_eq!(
                statuses(&entries),
                vec![
                    StatusSpec::Exact(304),
                    StatusSpec::Exact(404),
                    StatusSpec::Range(3),
                    StatusSpec::Range(5),
                ],
            );
            assert!(entries[0].1.is_none() && entries[2].1.is_none());
        }
        other => panic!("expected an error enum, got {other:?}"),
    }

    // Only a `304` and a `default` documented: no success status is declared, so `default`
    // is the success source, and the `304` does not claim it. On the error side the bodyless
    // `304` is its own unit variant ahead of `default` (issue #204): as a `Single(default)` it
    // was dropped, and a real `304` matched `default` and had its empty body decoded.
    let conditional = Responses {
        by_status: vec![(StatusSpec::Exact(304), resp(None))],
        default: Some(resp(Some(1))),
    };
    assert!(matches!(conditional.success(), SuccessShape::Plain(body) if body.id == TypeId(1)));
    assert_eq!(
        shape(conditional.error()),
        Shape::Enum(vec![
            (StatusSpec::Exact(304), None),
            (StatusSpec::Default, Some(1)),
        ])
    );
}

/// The statuses of an enum shape, in the order it holds them.
fn statuses(entries: &[(StatusSpec, Option<Ty>)]) -> Vec<StatusSpec> {
    entries.iter().map(|(status, _)| *status).collect()
}

#[test]
fn default_beside_multiple_bodied_successes_types_only_the_error_side() {
    // Two bodied successes and a bodied `default`: the success enum holds exactly the two 2xx
    // entries — `default` is no success variant and no fallback for another 2xx — while the
    // error side is the `default` body alone, so it is `Single`, not an enum.
    let responses = Responses {
        by_status: vec![
            (StatusSpec::Exact(201), resp(Some(2))),
            (StatusSpec::Exact(200), resp(Some(1))),
        ],
        default: Some(resp(Some(3))),
    };
    match responses.success() {
        SuccessShape::Enum(entries) => assert_eq!(
            statuses(&entries),
            vec![StatusSpec::Exact(200), StatusSpec::Exact(201)]
        ),
        other => panic!("expected Enum, got {other:?}"),
    }
    match responses.error() {
        ErrorShape::Single(body) => assert_eq!(body.id, TypeId(3)),
        other => panic!("expected Single, got {other:?}"),
    }
    assert_eq!(responses.single_success_media(), None);
    assert_eq!(responses.single_error_media(), Some(super::MediaType::Json));

    // With a bodied error status beside it, `default` becomes the last error variant — and
    // still never a success one.
    let responses = Responses {
        by_status: vec![
            (StatusSpec::Exact(200), resp(Some(1))),
            (StatusSpec::Exact(201), resp(Some(2))),
            (StatusSpec::Exact(404), resp(Some(4))),
        ],
        default: Some(resp(Some(3))),
    };
    match responses.success() {
        SuccessShape::Enum(entries) => assert_eq!(
            statuses(&entries),
            vec![StatusSpec::Exact(200), StatusSpec::Exact(201)]
        ),
        other => panic!("expected Enum, got {other:?}"),
    }
    match responses.error() {
        ErrorShape::Enum(entries) => assert_eq!(
            statuses(&entries),
            vec![StatusSpec::Exact(404), StatusSpec::Default]
        ),
        other => panic!("expected Enum, got {other:?}"),
    }
}

#[test]
fn a_bodyless_default_is_the_trailing_unit_error_variant_beside_any_error_body() {
    // A bodyless `default` beside one bodied error makes the shape an enum (issue #204), as it
    // does beside two, and is its trailing unit variant either way.
    let single = Responses {
        by_status: vec![
            (StatusSpec::Exact(200), resp(Some(1))),
            (StatusSpec::Exact(404), resp(Some(2))),
        ],
        default: Some(resp(None)),
    };
    assert_eq!(
        shape(single.error()),
        Shape::Enum(vec![
            (StatusSpec::Exact(404), Some(2)),
            (StatusSpec::Default, None),
        ])
    );

    let multi = Responses {
        by_status: vec![
            (StatusSpec::Exact(200), resp(Some(1))),
            (StatusSpec::Exact(404), resp(Some(2))),
            (StatusSpec::Exact(409), resp(Some(3))),
        ],
        default: Some(resp(None)),
    };
    match multi.error() {
        ErrorShape::Enum(entries) => {
            assert_eq!(
                statuses(&entries),
                vec![
                    StatusSpec::Exact(404),
                    StatusSpec::Exact(409),
                    StatusSpec::Default,
                ]
            );
            assert!(
                entries[2].1.is_none(),
                "the bodyless default is a unit variant"
            );
        }
        other => panic!("expected Enum, got {other:?}"),
    }
}

/// An error shape reduced to comparable data: bodies by type id, enum entries in order.
#[derive(Debug, PartialEq)]
enum Shape {
    None,
    Single(u32),
    Enum(Vec<(StatusSpec, Option<u32>)>),
}

fn shape(error: ErrorShape) -> Shape {
    match error {
        ErrorShape::None => Shape::None,
        ErrorShape::Single(body) => Shape::Single(body.id.0),
        ErrorShape::Enum(entries) => Shape::Enum(
            entries
                .into_iter()
                .map(|(status, body)| (status, body.map(|body| body.id.0)))
                .collect(),
        ),
    }
}

#[test]
fn the_error_shape_grid_over_bodied_errors_and_default_body_presence() {
    // Issue #127: every cell of bodied non-default errors (0 / 1 / 2) x `default` (absent /
    // bodied / bodyless), each with and without a bodyless error status beside them. With no
    // body the shape is `None`; a lone bodied entry is `Single`; anything else is an `Enum`
    // with every declared error entry a variant, so a bodyless `default` or `403` beside one
    // body is a unit variant rather than dropped (issue #204). `codegen`'s classification must
    // agree cell for cell (`e2e.rs`, `a_bodyless_error_entry_beside_one_error_body_is_its_own_variant`).
    const DEFAULT: u32 = 9;
    let (e404, e409) = (StatusSpec::Exact(404), StatusSpec::Exact(409));
    let (e403, any) = (StatusSpec::Exact(403), StatusSpec::Default);
    for bodyless_sibling in [false, true] {
        let sibling = || bodyless_sibling.then_some((e403, None));
        // The shape one body takes: `Single` alone, an enum beside the bodyless `403`.
        let lone = |status: StatusSpec, body: u32| {
            if bodyless_sibling {
                Shape::Enum(
                    sibling()
                        .into_iter()
                        .chain([(status, Some(body))])
                        .collect(),
                )
            } else {
                Shape::Single(body)
            }
        };
        let cells: [(usize, Option<Option<u32>>, Shape); 9] = [
            (0, None, Shape::None),
            (0, Some(Some(DEFAULT)), lone(any, DEFAULT)),
            (0, Some(None), Shape::None),
            (1, None, lone(e404, 4)),
            (
                1,
                Some(Some(DEFAULT)),
                Shape::Enum(
                    sibling()
                        .into_iter()
                        .chain([(e404, Some(4)), (any, Some(DEFAULT))])
                        .collect(),
                ),
            ),
            (
                1,
                Some(None),
                Shape::Enum(
                    sibling()
                        .into_iter()
                        .chain([(e404, Some(4)), (any, None)])
                        .collect(),
                ),
            ),
            (
                2,
                None,
                Shape::Enum(
                    sibling()
                        .into_iter()
                        .chain([(e404, Some(4)), (e409, Some(5))])
                        .collect(),
                ),
            ),
            (
                2,
                Some(Some(DEFAULT)),
                Shape::Enum(
                    sibling()
                        .into_iter()
                        .chain([(e404, Some(4)), (e409, Some(5)), (any, Some(DEFAULT))])
                        .collect(),
                ),
            ),
            (
                2,
                Some(None),
                Shape::Enum(
                    sibling()
                        .into_iter()
                        .chain([(e404, Some(4)), (e409, Some(5)), (any, None)])
                        .collect(),
                ),
            ),
        ];
        for (bodied_errors, default, expected) in cells {
            // Document order is deliberately not precedence order: 409 before 404, and the
            // bodyless `403` last, so the enum cells also pin the sort.
            let mut by_status = vec![(StatusSpec::Exact(200), resp(Some(1)))];
            by_status.extend(
                [(e409, resp(Some(5))), (e404, resp(Some(4)))]
                    .into_iter()
                    .skip(2 - bodied_errors),
            );
            if bodyless_sibling {
                by_status.push((e403, resp(None)));
            }
            let responses = Responses {
                by_status,
                default: default.map(resp),
            };
            assert_eq!(
                shape(responses.error()),
                expected,
                "{bodied_errors} bodied error(s), default {default:?}, \
                 bodyless 403: {bodyless_sibling}"
            );
        }
    }
}

#[test]
fn a_default_with_no_explicit_status_is_both_the_success_and_the_error_body() {
    // `by_status` empty: the early return makes `default` the sole success source, and the
    // error side is still offered it as `StatusSpec::Default`, so one body types both sides.
    let bodied = Responses {
        by_status: Vec::new(),
        default: Some(resp(Some(7))),
    };
    assert!(matches!(bodied.success(), SuccessShape::Plain(body) if body.id == TypeId(7)));
    assert!(matches!(bodied.error(), ErrorShape::Single(body) if body.id == TypeId(7)));
    assert_eq!(bodied.single_success_media(), Some(super::MediaType::Json));
    assert_eq!(bodied.single_error_media(), Some(super::MediaType::Json));

    // A bodyless sole `default` types neither side.
    let bodyless = Responses {
        by_status: Vec::new(),
        default: Some(resp(None)),
    };
    assert!(matches!(bodyless.success(), SuccessShape::Unit));
    assert!(matches!(bodyless.error(), ErrorShape::None));

    // And no responses at all is `Unit` / `None`.
    let empty = Responses {
        by_status: Vec::new(),
        default: None,
    };
    assert!(matches!(empty.success(), SuccessShape::Unit));
    assert!(matches!(empty.error(), ErrorShape::None));
}

#[test]
fn default_documents_the_success_side_when_no_success_status_is_declared() {
    // `404` plus a bodied `default` (issue #115): `default` is the only documentation of a 2xx,
    // so it is the success body — not `Unit`, which would accept any 2xx as `()` and discard
    // the body — and it stays the error side's catch-all beside the `404`.
    let responses = Responses {
        by_status: vec![(StatusSpec::Exact(404), resp(Some(1)))],
        default: Some(resp(Some(2))),
    };
    assert!(matches!(responses.success(), SuccessShape::Plain(body) if body.id == TypeId(2)));
    assert_eq!(
        responses.single_success_media(),
        Some(super::MediaType::Json)
    );
    match responses.error() {
        ErrorShape::Enum(entries) => assert_eq!(
            statuses(&entries),
            vec![StatusSpec::Exact(404), StatusSpec::Default]
        ),
        other => panic!("expected Enum, got {other:?}"),
    }

    // Every non-2xx status class counts as "no success declared", ranges included.
    for status in [
        StatusSpec::Exact(304),
        StatusSpec::Range(4),
        StatusSpec::Range(5),
    ] {
        let responses = Responses {
            by_status: vec![(status, resp(None))],
            default: Some(resp(Some(2))),
        };
        assert!(
            matches!(responses.success(), SuccessShape::Plain(body) if body.id == TypeId(2)),
            "{status:?}"
        );
    }

    // A bodyless `default` documents a bodyless 2xx: `Unit` is then the documented shape.
    let responses = Responses {
        by_status: vec![(StatusSpec::Exact(404), resp(Some(1)))],
        default: Some(resp(None)),
    };
    assert!(matches!(responses.success(), SuccessShape::Unit));
    assert_eq!(responses.single_success_media(), None);
}

#[test]
fn a_declared_success_status_keeps_default_off_the_success_side() {
    // Any 2xx entry, bodied or not, exact or range, is the success documentation; `default`
    // then stays on the error side alone.
    for (status, body) in [
        (StatusSpec::Exact(204), None),
        (StatusSpec::Range(2), None),
        (StatusSpec::Exact(200), Some(1)),
    ] {
        let responses = Responses {
            by_status: vec![
                (status, resp(body)),
                (StatusSpec::Exact(404), resp(Some(3))),
            ],
            default: Some(resp(Some(2))),
        };
        match (body, responses.success()) {
            (None, SuccessShape::Unit) => {}
            (Some(_), SuccessShape::Plain(ty)) => assert_eq!(ty.id, TypeId(1)),
            (_, other) => panic!("{status:?}: unexpected success shape {other:?}"),
        }
        assert_eq!(
            responses.single_success_media(),
            body.map(|_| super::MediaType::Json)
        );
    }
}

/// A graph whose ids are the positions of `kinds`.
fn graph(kinds: Vec<TypeKind>) -> TypeGraph {
    let mut graph = TypeGraph::default();
    for kind in kinds {
        graph.insert(TypeDef {
            name_hint: String::new(),
            kind,
            docs: Docs::default(),
            provenance: Provenance::new(JsonPointer::root(), None),
            document: String::new(),
        });
    }
    graph
}

/// A multi-status error shape over exact statuses, in the order given.
fn error_enum(entries: Vec<(u16, Option<Ty>)>) -> ErrorShape {
    ErrorShape::Enum(
        entries
            .into_iter()
            .map(|(code, body)| (StatusSpec::Exact(code), body))
            .collect(),
    )
}

fn object() -> TypeKind {
    TypeKind::Struct(Struct {
        fields: Vec::new(),
        additional: AdditionalProps::Allow,
    })
}

fn int_enum() -> TypeKind {
    TypeKind::Enum(ScalarEnum {
        repr: ScalarRepr::Int,
        variants: vec![ScalarValue::Int(1)],
        openness: Openness::Closed,
    })
}

fn two_bodies(graph: &TypeGraph, a: Ty, b: Ty) -> Option<ApiErrorBodyImpl> {
    error_enum(vec![(404, Some(a)), (409, Some(b))]).api_error_body(graph)
}

#[test]
fn bodies_referencing_one_definition_share_it() {
    let graph = graph(vec![object()]);
    assert_eq!(
        two_bodies(&graph, ty(0), ty(0)),
        Some(ApiErrorBodyImpl::Body(ty(0)))
    );
}

#[test]
fn distinct_definitions_generating_one_rust_type_share_it() {
    // Two string schemas (a `$ref` component and an inline one) are both `String`.
    let strings = graph(vec![
        TypeKind::Primitive(Prim::String),
        TypeKind::Primitive(Prim::String),
    ]);
    assert_eq!(
        two_bodies(&strings, ty(0), ty(1)),
        Some(ApiErrorBodyImpl::Body(ty(0)))
    );
    // An integer enum is a `pub type X = i64` alias, so it is an `i64` body, either way round.
    let ints = graph(vec![int_enum(), TypeKind::Primitive(Prim::I64)]);
    assert!(two_bodies(&ints, ty(0), ty(1)).is_some());
    assert!(two_bodies(&ints, ty(1), ty(0)).is_some());
    // Containers compare their items.
    let arrays = graph(vec![
        TypeKind::Primitive(Prim::String),
        TypeKind::Primitive(Prim::String),
        TypeKind::Array(Box::new(ty(0))),
        TypeKind::Array(Box::new(ty(1))),
    ]);
    assert!(two_bodies(&arrays, ty(2), ty(3)).is_some());
}

/// An array or tuple that closes an alias cycle is emitted as a nominal newtype (#648), so as
/// a Rust type it matches only itself (#650): not a plain `Vec` alias over the same item, and
/// not another member of a cycle of arrays, however alike. Its transparent newtype still
/// decodes what its structure does, so `same_decoded_values` keeps comparing that.
#[test]
fn an_alias_cycle_member_shares_a_body_only_with_itself() {
    let boxed = |id| Ty {
        boxed: true,
        ..ty(id)
    };
    let graph = graph(vec![
        // `Node: [Node]`, a self-referential array.
        TypeKind::Array(Box::new(boxed(0))),
        // `Into: [Node]`, a plain `Vec<Node>` alias over it.
        TypeKind::Array(Box::new(ty(0))),
        // `A: [B]` and `B: [A]`, a two-member cycle of arrays.
        TypeKind::Array(Box::new(boxed(3))),
        TypeKind::Array(Box::new(boxed(2))),
        // `Pair: (Pair,)`, a self-referential one-position tuple.
        TypeKind::Tuple(vec![boxed(4)]),
    ]);
    for id in [0, 2, 3, 4] {
        assert!(graph.closes_alias_cycle(TypeId(id)), "{id} closes a cycle");
    }
    assert!(!graph.closes_alias_cycle(TypeId(1)));
    assert_eq!(
        two_bodies(&graph, ty(0), ty(0)),
        Some(ApiErrorBodyImpl::Body(ty(0)))
    );
    // A `Node` newtype beside a `Vec<Node>` alias: no single `Body`, either way round.
    assert_eq!(two_bodies(&graph, ty(0), ty(1)), None);
    assert_eq!(two_bodies(&graph, ty(1), ty(0)), None);
    assert_eq!(two_bodies(&graph, ty(2), ty(3)), None);
    assert!(!graph.same_generated_type(ty(4), ty(0)));
    assert!(graph.same_decoded_values(ty(2), ty(3)));
}

/// The cycle check never panics on a reservation (#655): a reservation, or an array or tuple
/// whose item is one, may fill to a nominal newtype, so `same_generated_type` answers the
/// sound "not proven one type" instead, the same `false` its `Reserved` arm gives.
#[test]
fn a_reservation_is_not_proven_one_type_by_the_cycle_check() {
    let mut graph = graph(vec![
        TypeKind::Primitive(Prim::String),
        // `[R]` twice and `(R,)`, over the reservation `R` reserved below as id 6.
        TypeKind::Array(Box::new(ty(6))),
        TypeKind::Array(Box::new(ty(6))),
        TypeKind::Tuple(vec![ty(6)]),
        // `[String]` and `(String,)`, plain aliases with no reservation in reach.
        TypeKind::Array(Box::new(ty(0))),
        TypeKind::Tuple(vec![ty(0)]),
    ]);
    assert_eq!(graph.reserve(), TypeId(6));
    assert_eq!(graph.reserve(), TypeId(7));
    // Directly, on either side, and against a second reservation.
    assert!(!graph.same_generated_type(ty(6), ty(0)));
    assert!(!graph.same_generated_type(ty(0), ty(6)));
    assert!(!graph.same_generated_type(ty(6), ty(7)));
    // Through an array or tuple item, on either side.
    assert!(!graph.same_generated_type(ty(1), ty(2)));
    assert!(!graph.same_generated_type(ty(1), ty(4)));
    assert!(!graph.same_generated_type(ty(4), ty(1)));
    assert!(!graph.same_generated_type(ty(3), ty(5)));
    assert!(!graph.same_generated_type(ty(5), ty(3)));
    assert_eq!(two_bodies(&graph, ty(1), ty(2)), None);
    // The same reference is still one type, answered by id before the cycle check.
    assert!(graph.same_generated_type(ty(6), ty(6)));
    assert!(graph.same_generated_type(ty(1), ty(1)));
}

#[test]
fn different_rust_types_share_nothing() {
    let scalars = graph(vec![
        TypeKind::Primitive(Prim::I32),
        TypeKind::Primitive(Prim::I64),
    ]);
    assert_eq!(two_bodies(&scalars, ty(0), ty(1)), None);
    // Each struct (or `Never`) definition is its own nominal item, however alike.
    let nominal = graph(vec![object(), object(), TypeKind::Never, TypeKind::Never]);
    assert_eq!(two_bodies(&nominal, ty(0), ty(1)), None);
    assert_eq!(two_bodies(&nominal, ty(2), ty(3)), None);
    // Tuple items keep their `Box`, so an item boxed on one side is a different Rust type.
    let boxed_item = Ty {
        boxed: true,
        ..ty(0)
    };
    let tuples = graph(vec![
        TypeKind::Primitive(Prim::String),
        TypeKind::Tuple(vec![ty(0)]),
        TypeKind::Tuple(vec![boxed_item]),
        TypeKind::Tuple(vec![ty(0)]),
    ]);
    assert_eq!(two_bodies(&tuples, ty(1), ty(2)), None);
    assert!(two_bodies(&tuples, ty(1), ty(3)).is_some());
}

#[test]
fn per_status_nullability_and_bodyless_statuses_do_not_split_the_body() {
    let graph = graph(vec![object()]);
    let nullable = Ty {
        nullable: true,
        ..ty(0)
    };
    let shape = error_enum(vec![(401, None), (404, Some(nullable)), (409, Some(ty(0)))]);
    // `Body` is the bare definition: the per-variant `Option` is absorbed by the accessor.
    assert_eq!(
        shape.api_error_body(&graph),
        Some(ApiErrorBodyImpl::Body(ty(0)))
    );
}

#[test]
fn the_single_and_uninhabited_shapes_always_implement_it() {
    let graph = graph(vec![object()]);
    assert_eq!(
        ErrorShape::None.api_error_body(&graph),
        Some(ApiErrorBodyImpl::Uninhabited)
    );
    let nullable = Ty {
        nullable: true,
        ..ty(0)
    };
    assert_eq!(
        ErrorShape::Single(nullable).api_error_body(&graph),
        Some(ApiErrorBodyImpl::Body(ty(0)))
    );
}

/// `Body` names the definition, not the `Box` emission wraps a payload in: a boxed body, alone
/// or as a multi-status payload, comes back unboxed.
#[test]
fn a_boxed_body_is_handed_back_unboxed() {
    let graph = graph(vec![object()]);
    let boxed = Ty {
        boxed: true,
        ..ty(0)
    };
    assert_eq!(
        ErrorShape::Single(boxed).api_error_body(&graph),
        Some(ApiErrorBodyImpl::Body(ty(0)))
    );
    assert_eq!(
        two_bodies(&graph, boxed, boxed),
        Some(ApiErrorBodyImpl::Body(ty(0)))
    );
}

/// `api_error_body` bares its inputs before comparing, so it never reaches the nullability
/// check itself; call `same_generated_type` directly. `T` and `Option<T>` over one definition
/// are different Rust types, at the top level and as an array item.
#[test]
fn nullability_distinguishes_otherwise_identical_types() {
    let nullable = Ty {
        nullable: true,
        ..ty(0)
    };
    let graph = graph(vec![
        TypeKind::Primitive(Prim::String),
        TypeKind::Array(Box::new(ty(0))),
        TypeKind::Array(Box::new(nullable)),
    ]);
    assert!(!graph.same_generated_type(ty(0), nullable));
    assert!(!graph.same_generated_type(nullable, ty(0)));
    assert!(graph.same_generated_type(nullable, nullable));
    // `Vec<T>` against `Vec<Option<T>>`.
    assert!(!graph.same_generated_type(ty(1), ty(2)));
}

/// The rules [`Responses::success`] and [`Responses::error`] follow, checked over random
/// status sets rather than chosen examples (issue #477). Each case draws unique exact codes
/// (biased toward 2xx) and ranges `1XX`..`5XX`, each bodied or bodyless and streaming or not,
/// plus an absent, bodied, or bodyless `default`. Every body carries its own type id — the
/// `by_status` position plus one, or [`DEFAULT_BODY`] — so a reduced shape says which response
/// each body came from. The oracles are written from the documented contract, not from the
/// implementation: a model of both shapes, the partition of declared statuses between the two
/// sides, where `default` lands, and independence from `by_status` insertion order.
mod partition_props {
    use super::{resp, shape, stream_resp, Shape};
    use crate::ir::responses::{Response, Responses, StatusSpec, SuccessShape};
    use proptest::prelude::*;

    /// The body type id a bodied `default` carries, distinct from every `by_status` id.
    const DEFAULT_BODY: u32 = 1000;

    fn success_shape(success: SuccessShape) -> Shape {
        match success {
            SuccessShape::Unit => Shape::None,
            SuccessShape::Plain(body) => Shape::Single(body.id.0),
            SuccessShape::Enum(entries) => Shape::Enum(
                entries
                    .into_iter()
                    .map(|(status, body)| (status, body.map(|body| body.id.0)))
                    .collect(),
            ),
        }
    }

    /// A 2xx selector, stated from RFC 9110's classes rather than through `is_success`.
    fn is_2xx(status: StatusSpec) -> bool {
        matches!(status, StatusSpec::Exact(200..=299) | StatusSpec::Range(2))
    }

    /// Decode precedence, built by concatenation rather than a sort key: exact codes
    /// ascending, then ranges ascending, then `default`.
    fn in_precedence(entries: Vec<(StatusSpec, Option<u32>)>) -> Vec<(StatusSpec, Option<u32>)> {
        let mut exact: Vec<_> = entries
            .iter()
            .filter_map(|&(status, body)| match status {
                StatusSpec::Exact(code) => Some((code, body)),
                _ => None,
            })
            .collect();
        exact.sort_unstable_by_key(|&(code, _)| code);
        let mut ranges: Vec<_> = entries
            .iter()
            .filter_map(|&(status, body)| match status {
                StatusSpec::Range(prefix) => Some((prefix, body)),
                _ => None,
            })
            .collect();
        ranges.sort_unstable_by_key(|&(prefix, _)| prefix);
        let defaults = entries
            .iter()
            .filter(|(status, _)| *status == StatusSpec::Default)
            .copied();
        exact
            .into_iter()
            .map(|(code, body)| (StatusSpec::Exact(code), body))
            .chain(
                ranges
                    .into_iter()
                    .map(|(prefix, body)| (StatusSpec::Range(prefix), body)),
            )
            .chain(defaults)
            .collect()
    }

    /// The documented success shape: with no 2xx declared, `default`'s body (or `()`);
    /// otherwise the 2xx entries by body count — none is `()`, one alone (or one streaming
    /// body beside bodyless siblings) is plain, anything else is the sorted enum.
    fn expected_success(responses: &Responses) -> Shape {
        let entries: Vec<(StatusSpec, Option<u32>, bool)> = responses
            .by_status
            .iter()
            .filter(|(status, _)| is_2xx(*status))
            .map(|(status, response)| {
                (
                    *status,
                    response.body.map(|body| body.id.0),
                    response.stream.is_some(),
                )
            })
            .collect();
        if entries.is_empty() {
            return match responses.default.as_ref().and_then(|default| default.body) {
                Some(body) => Shape::Single(body.id.0),
                None => Shape::None,
            };
        }
        let bodied: Vec<_> = entries
            .iter()
            .filter(|(_, body, _)| body.is_some())
            .collect();
        match bodied.as_slice() {
            [] => Shape::None,
            [(_, Some(body), _)] if entries.len() == 1 => Shape::Single(*body),
            [(_, Some(body), true)] => Shape::Single(*body),
            _ => Shape::Enum(in_precedence(
                entries
                    .into_iter()
                    .map(|(status, body, _)| (status, body))
                    .collect(),
            )),
        }
    }

    /// The documented error shape: the non-2xx entries plus a declared `default`, by body
    /// count — none is `None`, one entry carrying the only body is `Single`, anything else is
    /// the sorted enum.
    fn expected_error(responses: &Responses) -> Shape {
        let mut entries: Vec<(StatusSpec, Option<u32>)> = responses
            .by_status
            .iter()
            .filter(|(status, _)| !is_2xx(*status))
            .map(|(status, response)| (*status, response.body.map(|body| body.id.0)))
            .collect();
        if let Some(default) = &responses.default {
            entries.push((StatusSpec::Default, default.body.map(|body| body.id.0)));
        }
        match entries.as_slice() {
            [] => Shape::None,
            [(_, Some(body))] => Shape::Single(*body),
            _ if entries.iter().all(|(_, body)| body.is_none()) => Shape::None,
            _ => Shape::Enum(in_precedence(entries)),
        }
    }

    /// The body type ids a reduced shape carries.
    fn bodies(shape: &Shape) -> Vec<u32> {
        match shape {
            Shape::None => Vec::new(),
            Shape::Single(body) => vec![*body],
            Shape::Enum(entries) => entries.iter().filter_map(|(_, body)| *body).collect(),
        }
    }

    fn response(id: u32, bodied: bool, streaming: bool) -> Response {
        let body = bodied.then_some(id);
        if streaming {
            stream_resp(body)
        } else {
            resp(body)
        }
    }

    /// A case in document order, the same `by_status` entries in a shuffled order, and its
    /// `default`.
    fn arb_case() -> impl Strategy<Value = (Responses, Responses)> {
        let exact = proptest::collection::btree_set(prop_oneof![200u16..=206, 100u16..=599], 0..6);
        let ranges = proptest::collection::btree_set(1u8..=5, 0..3);
        (exact, ranges)
            .prop_map(|(exact, ranges)| {
                exact
                    .into_iter()
                    .map(StatusSpec::Exact)
                    .chain(ranges.into_iter().map(StatusSpec::Range))
                    .collect::<Vec<_>>()
            })
            .prop_flat_map(|statuses| {
                let flags =
                    proptest::collection::vec((any::<bool>(), any::<bool>()), statuses.len());
                (
                    Just(statuses),
                    flags,
                    proptest::option::of((any::<bool>(), any::<bool>())),
                )
            })
            .prop_flat_map(|(statuses, flags, default)| {
                let by_status: Vec<(StatusSpec, Response)> = statuses
                    .into_iter()
                    .zip(flags)
                    .zip(1u32..)
                    .map(|((status, (bodied, streaming)), id)| {
                        (status, response(id, bodied, streaming))
                    })
                    .collect();
                let default =
                    default.map(|(bodied, streaming)| response(DEFAULT_BODY, bodied, streaming));
                (
                    Just(by_status.clone()),
                    Just(by_status).prop_shuffle(),
                    Just(default),
                )
            })
            .prop_map(|(by_status, shuffled, default)| {
                (
                    Responses {
                        by_status,
                        default: default.clone(),
                    },
                    Responses {
                        by_status: shuffled,
                        default,
                    },
                )
            })
    }

    proptest! {
        /// The body count picks unit, single, or enum on each side, and an enum lists that
        /// side's entries in decode precedence.
        #[test]
        fn the_body_count_picks_each_sides_shape((responses, _) in arb_case()) {
            prop_assert_eq!(success_shape(responses.success()), expected_success(&responses));
            prop_assert_eq!(shape(responses.error()), expected_error(&responses));
        }

        /// Success and error partition the declared statuses: every `by_status` body reaches
        /// exactly one side, the success side exactly when its status is 2xx, and an enum on
        /// either side names only that side's statuses.
        #[test]
        fn success_and_error_partition_the_declared_statuses((responses, _) in arb_case()) {
            let success = success_shape(responses.success());
            let error = shape(responses.error());
            let (success_bodies, error_bodies) = (bodies(&success), bodies(&error));
            for ((status, response), id) in responses.by_status.iter().zip(1u32..) {
                if response.body.is_none() {
                    continue;
                }
                prop_assert_eq!(success_bodies.contains(&id), is_2xx(*status));
                prop_assert_eq!(error_bodies.contains(&id), !is_2xx(*status));
            }
            if let Shape::Enum(entries) = &success {
                prop_assert!(entries.iter().all(|(status, _)| is_2xx(*status)));
            }
            if let Shape::Enum(entries) = &error {
                prop_assert!(entries.iter().all(|(status, _)| !is_2xx(*status)));
            }
        }

        /// `default` is on the success side exactly when no 2xx status is declared, and on
        /// the error side whenever it is declared, as the last entry of an error enum.
        #[test]
        fn default_is_the_success_source_iff_no_2xx_is_declared((responses, _) in arb_case()) {
            let success = success_shape(responses.success());
            let error = shape(responses.error());
            let no_2xx = !responses.by_status.iter().any(|(status, _)| is_2xx(*status));
            let bodied_default = responses
                .default
                .as_ref()
                .is_some_and(|default| default.body.is_some());
            prop_assert_eq!(
                bodies(&success).contains(&DEFAULT_BODY),
                bodied_default && no_2xx
            );
            prop_assert_eq!(bodies(&error).contains(&DEFAULT_BODY), bodied_default);
            if let Shape::Enum(entries) = &success {
                prop_assert!(entries.iter().all(|(status, _)| *status != StatusSpec::Default));
            }
            if let Shape::Enum(entries) = &error {
                let defaults: Vec<usize> = entries
                    .iter()
                    .enumerate()
                    .filter(|(_, (status, _))| *status == StatusSpec::Default)
                    .map(|(index, _)| index)
                    .collect();
                let expected: Vec<usize> = if responses.default.is_some() {
                    vec![entries.len() - 1]
                } else {
                    Vec::new()
                };
                prop_assert_eq!(defaults, expected);
            }
        }

        /// Neither shape depends on the order lowering inserted `by_status` entries in.
        #[test]
        fn the_shapes_do_not_depend_on_insertion_order((responses, shuffled) in arb_case()) {
            prop_assert_eq!(
                success_shape(responses.success()),
                success_shape(shuffled.success())
            );
            prop_assert_eq!(shape(responses.error()), shape(shuffled.error()));
        }
    }
}
