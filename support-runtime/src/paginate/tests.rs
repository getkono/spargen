use std::sync::Arc;

use reqwest::header::{HeaderMap, HeaderValue};

use super::{next_link, next_link_target, LinkPaginator};
use crate::tests::{core, poll_ready, Reply, SequenceBackend};
use crate::{ClientCore, Error};

fn link_headers(values: &[&str]) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for value in values {
        headers.append(
            reqwest::header::LINK,
            HeaderValue::from_str(value).expect("valid header value"),
        );
    }
    headers
}

fn next_of(value: &str) -> Option<String> {
    next_link_target(&link_headers(&[value]))
}

#[test]
fn single_link_with_rel_next() {
    assert_eq!(
        next_of(r#"<https://api.example.com/items?page=2>; rel="next""#),
        Some("https://api.example.com/items?page=2".to_owned())
    );
}

#[test]
fn picks_next_among_prev_last_first() {
    let field = concat!(
        r#"<https://api.example.com/items?page=1>; rel="prev", "#,
        r#"<https://api.example.com/items?page=3>; rel="next", "#,
        r#"<https://api.example.com/items?page=9>; rel="last", "#,
        r#"<https://api.example.com/items?page=1>; rel="first""#,
    );
    assert_eq!(
        next_of(field),
        Some("https://api.example.com/items?page=3".to_owned())
    );
}

#[test]
fn unquoted_rel_is_accepted() {
    assert_eq!(
        next_of("<https://x/2>; rel=next"),
        Some("https://x/2".to_owned())
    );
}

#[test]
fn extra_params_are_ignored() {
    assert_eq!(
        next_of(r#"<https://x/2>; title="Page 2"; rel="next"; type="text/html""#),
        Some("https://x/2".to_owned())
    );
}

#[test]
fn tolerates_irregular_whitespace() {
    assert_eq!(
        next_of("  <https://x/2>   ;    rel=\"next\"   "),
        Some("https://x/2".to_owned())
    );
}

#[test]
fn multi_value_rel_containing_next() {
    assert_eq!(
        next_of(r#"<https://x/2>; rel="next foo""#),
        Some("https://x/2".to_owned())
    );
    assert_eq!(
        next_of(r#"<https://x/2>; rel="foo next""#),
        Some("https://x/2".to_owned())
    );
}

#[test]
fn rel_is_case_insensitive() {
    assert_eq!(
        next_of(r#"<https://x/2>; rel="NEXT""#),
        Some("https://x/2".to_owned())
    );
    assert_eq!(
        next_of(r#"<https://x/2>; REL="next""#),
        Some("https://x/2".to_owned())
    );
}

#[test]
fn no_next_relation_yields_none() {
    assert_eq!(
        next_of(r#"<https://x/1>; rel="prev", <https://x/9>; rel="last""#),
        None
    );
}

#[test]
fn comma_inside_uri_is_not_a_separator() {
    // A comma inside the angle-bracketed URI must not split the entry.
    assert_eq!(
        next_of(r#"<https://x/items?ids=1,2,3&page=2>; rel="next""#),
        Some("https://x/items?ids=1,2,3&page=2".to_owned())
    );
}

#[test]
fn comma_inside_quoted_param_is_not_a_separator() {
    assert_eq!(
        next_of(r#"<https://x/2>; title="a, b, c"; rel="next""#),
        Some("https://x/2".to_owned())
    );
}

#[test]
fn angle_bracket_inside_quoted_param_opens_no_uri() {
    // Entry splitting respects `<…>` only outside a quoted string: the `<` in the title is text,
    // so the comma after the closing quote still separates the two entries.
    assert_eq!(
        next_of(r#"<https://x/1>; title="a<b", <https://x/2>; rel="next""#),
        Some("https://x/2".to_owned())
    );
}

#[test]
fn angle_bracket_in_param_section_does_not_hide_a_semicolon() {
    // Parameter splitting ignores angle brackets: an unclosed `<` in a parameter value leaves the
    // following `;` a separator, so the `rel` after it is still read.
    assert_eq!(
        next_of("<https://x/2>; title=<x; rel=next"),
        Some("https://x/2".to_owned())
    );
}

#[test]
fn an_escaped_quote_inside_a_quoted_param_does_not_close_it() {
    // The `\"` is escaped, so the quoted title runs on past the comma and swallows what looks
    // like a second entry: the `rel="next"` belongs to the first entry.
    assert_eq!(
        next_of(r#"<https://x/1>; title="a\", <https://x/3>"; rel="next""#),
        Some("https://x/1".to_owned())
    );
}

#[test]
fn a_backslash_outside_a_quoted_string_escapes_nothing() {
    // Outside a quoted string a backslash is ordinary text, so the comma after it still
    // separates the entries and the `rel="next"` belongs to the second one.
    assert_eq!(
        next_of(r#"<https://x/1>; title=a\, <https://x/2>; rel="next""#),
        Some("https://x/2".to_owned())
    );
}

#[test]
fn a_quote_inside_the_uri_opens_no_quoted_string() {
    // The `"` sits inside `<…>`, so it is part of the URI-reference: the comma after the first
    // entry's `rel="prev"` still separates the entries.
    assert_eq!(
        next_of(r#"<https://x/"a>; rel="prev", <https://x/2>; rel="next""#),
        Some("https://x/2".to_owned())
    );
}

#[test]
fn a_rel_value_is_unquoted_only_when_quotes_open_and_close_it() {
    // Only a value that both starts and ends with `"` loses them: a value that merely starts
    // with one is a relation type that is not `next`, and a lone `"` is not a quoted string.
    assert_eq!(next_of(r#"<https://x/2>; rel="nextx"#), None);
    assert_eq!(next_of(r#"<https://x/2>; rel=""#), None);
    assert_eq!(next_of(r#"<https://x/2>; rel=next""#), None);
}

#[test]
fn text_before_the_uri_reference_does_not_shift_it() {
    // The URI-reference is whatever sits between the first `<` and the `>` after it, wherever
    // that `<` falls in the entry.
    assert_eq!(
        next_of("junk <https://x/2>; rel=next"),
        Some("https://x/2".to_owned())
    );
}

#[test]
fn parameters_are_read_from_just_past_the_closing_angle() {
    // The parameter section starts right after `>`: the URI's last character is not part of
    // it (a trailing `"` there would open a quoted string that hides the `;`), and the `>`
    // itself is not either, so a first parameter written straight after it keeps its name.
    assert_eq!(
        next_of(r#"<https://x/2">; rel=next"#),
        Some(r#"https://x/2""#.to_owned())
    );
    assert_eq!(
        next_of("<https://x/2>rel=next"),
        Some("https://x/2".to_owned())
    );
}

#[test]
fn malformed_input_yields_none_without_panic() {
    assert_eq!(next_of("garbage without brackets; rel=next"), None);
    assert_eq!(next_of("<unterminated; rel=next"), None);
    assert_eq!(next_of("<>; rel=next"), None); // empty URI
    assert_eq!(next_of(""), None);
    assert_eq!(next_of(";;;,,,"), None);
}

#[test]
fn scans_across_multiple_header_lines() {
    // Two separate `Link` header lines; the `next` lives on the second.
    let headers = link_headers(&[
        r#"<https://x/1>; rel="prev""#,
        r#"<https://x/3>; rel="next""#,
    ]);
    assert_eq!(next_link_target(&headers), Some("https://x/3".to_owned()));
}

#[test]
fn next_link_returns_parsed_absolute_url() {
    let headers = link_headers(&[r#"<https://api.example.com/items?page=2>; rel="next""#]);
    let url = next_link(&headers).expect("absolute next url");
    assert_eq!(url.as_str(), "https://api.example.com/items?page=2");
    // No Link header at all → no next url.
    assert!(next_link(&HeaderMap::new()).is_none());
}

/// A page answered with `status` and `body`, carrying `link` as its `Link` header when given.
fn page(status: u16, body: &str, link: Option<&str>) -> Reply {
    let reply = Reply::status(status, body);
    match link {
        Some(link) => reply.header("link", link),
        None => reply,
    }
}

/// A paginator seeded at `first` whose pages come from `pages`, in order.
fn paginator<T>(
    pages: impl IntoIterator<Item = Reply>,
    first: &str,
) -> (LinkPaginator<T>, Arc<SequenceBackend>) {
    let backend = SequenceBackend::new(pages);
    let core = ClientCore::with_backend(backend.clone(), "https://x").unwrap();
    let pager = core.paginate_links(reqwest::Url::parse(first).unwrap());
    (pager, backend)
}

/// `next_page` fetches the armed URL, decodes the page, and re-arms from its `rel="next"` link;
/// a page without one is yielded and ends the chain, and the pull after it sends nothing.
#[test]
fn next_page_advances_next_from_link_header_then_terminates() {
    let (mut pager, backend) = paginator::<Vec<i64>>(
        [
            page(
                200,
                "[1,2]",
                Some(r#"<https://x/items?page=2>; rel="next""#),
            ),
            page(200, "[3,4]", None),
        ],
        "https://x/items?page=1",
    );

    let page1 = poll_ready(pager.next_page()).unwrap().unwrap();
    assert_eq!(page1.into_inner(), vec![1, 2]);
    assert!(pager.has_next());

    let page2 = poll_ready(pager.next_page()).unwrap().unwrap();
    assert_eq!(page2.into_inner(), vec![3, 4]);
    assert!(!pager.has_next());

    assert!(poll_ready(pager.next_page()).is_none());
    assert_eq!(
        backend.urls(),
        ["https://x/items?page=1", "https://x/items?page=2"]
    );
}

/// `send` returns `Ok` for 4xx/5xx, so `next_page` gates on status before decoding. A follow-up
/// page that 401s or 500s (plausible: the generic paginator attaches no per-op credentials)
/// surfaces as `UnexpectedStatus` and terminates the chain — never decoded as `T`, never a
/// `Decode` error. The error body here structurally fits the lenient page type `Vec<Value>`, so
/// without the status gate `decode_success` would accept it as a page.
#[test]
fn non_success_page_terminates_as_unexpected_status_not_decoded() {
    for status in [401u16, 500u16] {
        let (mut pager, backend) = paginator::<Vec<serde_json::Value>>(
            [page(
                status,
                r#"[{"message":"denied"}]"#,
                Some(r#"<https://x/2>; rel="next""#),
            )],
            "https://x/1",
        );
        let outcome = poll_ready(pager.next_page());
        assert!(
            matches!(&outcome, Some(Err(Error::UnexpectedStatus { .. }))),
            "status {status}: expected UnexpectedStatus, got {outcome:?}"
        );
        // Terminated: the `rel="next"` link was not followed, and the next pull returns `None`
        // without issuing a request.
        assert!(!pager.has_next());
        assert!(poll_ready(pager.next_page()).is_none());
        assert_eq!(backend.calls(), 1, "{status}");
    }
}

/// A page whose request fails at the transport is yielded as that failure and ends the chain:
/// nothing re-arms the paginator, and the next pull sends nothing.
#[test]
fn a_transport_failure_terminates_the_paginator() {
    let (mut pager, backend) = paginator::<Vec<i64>>([Reply::failure(503)], "https://x/1");
    match poll_ready(pager.next_page()) {
        Some(Err(error @ Error::Transport(_))) => assert!(error.is_transient()),
        other => panic!("expected a Transport error, got {other:?}"),
    }
    assert!(!pager.has_next());
    assert!(poll_ready(pager.next_page()).is_none());
    assert_eq!(backend.calls(), 1);
}

#[test]
fn constructors_seed_the_next_url() {
    let first = reqwest::Url::parse("https://x/items?page=1").unwrap();
    let paginator: LinkPaginator<Vec<i64>> = LinkPaginator::new(core(), first);
    assert!(paginator.has_next());

    let seeded: LinkPaginator<Vec<i64>> =
        core().paginate_links(reqwest::Url::parse("https://x/items?page=1").unwrap());
    assert!(seeded.has_next());

    // from_headers with an absolute next link arms the paginator; without one it is empty.
    let armed: LinkPaginator<Vec<i64>> =
        LinkPaginator::from_headers(core(), &link_headers(&[r#"<https://x/2>; rel="next""#]));
    assert!(armed.has_next());
    let empty: LinkPaginator<Vec<i64>> = LinkPaginator::from_headers(core(), &HeaderMap::new());
    assert!(!empty.has_next());
}

/// A relative `next` target is resolved against the URL of the page just fetched, as RFC 3986
/// reference resolution reads it: a path-absolute target keeps the origin, and a query-only
/// target keeps the path too.
#[test]
fn a_relative_next_link_resolves_against_the_fetched_page_url() {
    for (target, resolved) in [
        ("/items?page=2", "https://x/items?page=2"),
        ("?page=3", "https://x/api/items?page=3"),
        ("more?page=4", "https://x/api/more?page=4"),
    ] {
        let link = format!(r#"<{target}>; rel="next""#);
        let (mut pager, backend) = paginator::<Vec<i64>>(
            [page(200, "[1]", Some(&link)), page(200, "[2]", None)],
            "https://x/api/items?page=1",
        );
        let first = poll_ready(pager.next_page()).unwrap().unwrap();
        assert_eq!(first.into_inner(), vec![1]);
        let second = poll_ready(pager.next_page()).unwrap().unwrap();
        assert_eq!(second.into_inner(), vec![2]);
        assert_eq!(
            backend.urls(),
            ["https://x/api/items?page=1", resolved],
            "{target}"
        );
    }
}

/// `from_headers` has no fetched URL to resolve against, so it follows only an absolute
/// `next`: a relative one leaves the paginator with nothing to fetch.
#[test]
fn from_headers_refuses_a_relative_next_link() {
    for target in ["/items?page=2", "?page=2", "items/2"] {
        let headers = link_headers(&[&format!(r#"<{target}>; rel="next""#)]);
        assert_eq!(next_link_target(&headers).as_deref(), Some(target));
        assert!(next_link(&headers).is_none(), "{target}");
        let pager: LinkPaginator<Vec<i64>> = LinkPaginator::from_headers(core(), &headers);
        assert!(!pager.has_next(), "{target}");
    }
}

/// A `2xx` page whose body does not decode as `T` surfaces as `Decode` with the page's status,
/// and terminates the chain: the `rel="next"` link it carried is not followed.
#[test]
fn a_success_page_that_fails_to_decode_terminates_as_a_decode_error() {
    let (mut pager, backend) = paginator::<Vec<i64>>(
        [page(
            203,
            r#"{"not":"a list"}"#,
            Some(r#"<https://x/2>; rel="next""#),
        )],
        "https://x/1",
    );
    match poll_ready(pager.next_page()) {
        Some(Err(Error::Decode { status, .. })) => assert_eq!(status.as_u16(), 203),
        other => panic!("expected a Decode error, got {other:?}"),
    }
    assert!(!pager.has_next());
    assert!(poll_ready(pager.next_page()).is_none());
    assert_eq!(backend.calls(), 1);
}

/// An armed paginator's `next_page` sends the request, yields the decoded page, and re-arms
/// from the page's `Link` header.
#[test]
fn next_page_fetches_and_decodes_the_armed_url() {
    let (mut pager, _) = paginator::<Vec<i64>>(
        [page(
            200,
            "[7]",
            Some(r#"<https://x/items?page=2>; rel="next""#),
        )],
        "https://x/items?page=1",
    );
    let page = poll_ready(pager.next_page())
        .expect("an armed paginator yields a page")
        .expect("the page decodes");
    assert_eq!(page.into_inner(), vec![7]);
    assert_eq!(
        pager.next.as_ref().map(reqwest::Url::as_str),
        Some("https://x/items?page=2")
    );
}

/// A page whose `rel="next"` names its own URL is yielded, and the pull after it ends the chain
/// as `PaginationCycle` without sending the request again. The link carries a fragment, which is
/// never sent, so it names the same page.
#[test]
fn a_self_link_ends_the_chain_as_a_pagination_cycle() {
    let (mut pager, backend) = paginator::<Vec<i64>>(
        [page(
            200,
            "[1]",
            Some(r#"<https://x/items?page=1#top>; rel="next""#),
        )],
        "https://x/items?page=1",
    );
    let first = poll_ready(pager.next_page()).unwrap().unwrap();
    assert_eq!(first.into_inner(), vec![1]);
    assert!(pager.has_next(), "the cycle is still reported");
    match poll_ready(pager.next_page()) {
        Some(Err(error @ Error::PaginationCycle { .. })) => {
            assert!(!error.is_transient(), "{error}");
            assert!(error.status().is_none(), "{error}");
            let Error::PaginationCycle { url } = error else {
                unreachable!()
            };
            assert_eq!(url.as_str(), "https://x/items?page=1#top");
        }
        other => panic!("expected a PaginationCycle error, got {other:?}"),
    }
    assert!(!pager.has_next());
    assert!(poll_ready(pager.next_page()).is_none());
    assert_eq!(backend.urls(), ["https://x/items?page=1"]);
}

/// A longer cycle (page 1 → page 2 → page 1) is caught too, not only a self-link: both pages are
/// yielded, and the link back to page 1 ends the chain without refetching it. A relative link
/// is compared after it resolves against the page that carried it.
#[test]
fn a_link_back_to_an_earlier_page_ends_the_chain_as_a_pagination_cycle() {
    let (mut pager, backend) = paginator::<Vec<i64>>(
        [
            page(200, "[1]", Some(r#"<https://x/items?page=2>; rel="next""#)),
            page(200, "[2]", Some(r#"</items?page=1>; rel="next""#)),
        ],
        "https://x/items?page=1",
    );
    let first = poll_ready(pager.next_page()).unwrap().unwrap();
    assert_eq!(first.into_inner(), vec![1]);
    let second = poll_ready(pager.next_page()).unwrap().unwrap();
    assert_eq!(second.into_inner(), vec![2]);
    match poll_ready(pager.next_page()) {
        Some(Err(Error::PaginationCycle { url })) => {
            assert_eq!(url.as_str(), "https://x/items?page=1");
        }
        other => panic!("expected a PaginationCycle error, got {other:?}"),
    }
    assert!(!pager.has_next());
    assert!(poll_ready(pager.next_page()).is_none());
    assert_eq!(
        backend.urls(),
        ["https://x/items?page=1", "https://x/items?page=2"]
    );
}

/// A `next` link to another host, an `https` page linking to `http`, and a link to another port
/// each end the chain as `PaginationOriginChanged` once the page carrying the link is yielded,
/// without sending anything to the refused URL. The error names both URLs without their query,
/// and its `Display` shows their origins alone, so the first page's query reaches a log line
/// through neither `{error}` nor `{error:?}`.
#[test]
fn a_next_link_to_another_origin_ends_the_chain_without_a_request() {
    for (link, origin) in [
        ("https://evil.example/items?page=2", "https://evil.example"),
        ("http://x/items?page=2", "http://x"),
        ("https://x:8443/items?page=2", "https://x:8443"),
    ] {
        let (mut pager, backend) = paginator::<Vec<i64>>(
            [page(200, "[1]", Some(&format!("<{link}>; rel=\"next\"")))],
            "https://x/items?page=1&key=secret",
        );
        let first = poll_ready(pager.next_page()).unwrap().unwrap();
        assert_eq!(first.into_inner(), vec![1], "{link}");
        assert!(pager.has_next(), "the refusal is still reported for {link}");
        match poll_ready(pager.next_page()) {
            Some(Err(error @ Error::PaginationOriginChanged { .. })) => {
                assert!(!error.is_transient(), "{error}");
                assert!(error.status().is_none(), "{error}");
                let rendered = error.to_string();
                assert_eq!(
                    rendered,
                    format!(
                        "pagination origin changed: the next link leaves https://x for {origin}"
                    )
                );
                assert!(!rendered.contains("secret"), "{rendered}");
                let debug = format!("{error:?}");
                assert!(!debug.contains("secret"), "{debug}");
                let Error::PaginationOriginChanged { from, to } = error else {
                    unreachable!()
                };
                assert_eq!(from.as_str(), "https://x/items");
                assert_eq!(Some(to.as_str()), link.split('?').next());
            }
            other => panic!("expected PaginationOriginChanged for {link}, got {other:?}"),
        }
        assert!(!pager.has_next(), "{link}");
        assert!(poll_ready(pager.next_page()).is_none(), "{link}");
        assert_eq!(
            backend.urls(),
            ["https://x/items?page=1&key=secret"],
            "{link} was requested"
        );
    }
}

/// The origin is compared as the URL parser normalises it: a host in another case and the
/// scheme's default port spelled out name the first page's origin, so the link is followed.
#[test]
fn a_next_link_spelling_the_same_origin_differently_is_followed() {
    let (mut pager, backend) = paginator::<Vec<i64>>(
        [
            page(
                200,
                "[1]",
                Some(r#"<HTTPS://X:443/items?page=2>; rel="next""#),
            ),
            page(200, "[2]", None),
        ],
        "https://x/items?page=1",
    );
    assert_eq!(
        poll_ready(pager.next_page()).unwrap().unwrap().into_inner(),
        vec![1]
    );
    assert_eq!(
        poll_ready(pager.next_page()).unwrap().unwrap().into_inner(),
        vec![2]
    );
    assert!(poll_ready(pager.next_page()).is_none());
    assert_eq!(
        backend.urls(),
        ["https://x/items?page=1", "https://x/items?page=2"]
    );
}

/// A paginator seeded from headers has no first page URL of its own: the first URL it fetches,
/// the seeded link, fixes the origin, and a later link leaving it is refused. The error keeps
/// neither the seeded link's userinfo nor its query, so its `Debug` prints neither credential.
#[test]
fn a_header_seeded_paginator_holds_the_origin_of_its_first_fetch() {
    let backend = SequenceBackend::new([page(
        200,
        "[2]",
        Some(r#"<https://elsewhere.example/3>; rel="next""#),
    )]);
    let core = ClientCore::with_backend(backend.clone(), "https://x").unwrap();
    let mut pager: LinkPaginator<Vec<i64>> = LinkPaginator::from_headers(
        core,
        &link_headers(&[r#"<https://u:pw@x/2?key=secret>; rel="next""#]),
    );
    assert_eq!(
        poll_ready(pager.next_page()).unwrap().unwrap().into_inner(),
        vec![2]
    );
    match poll_ready(pager.next_page()) {
        Some(Err(error @ Error::PaginationOriginChanged { .. })) => {
            let debug = format!("{error:?}");
            assert!(
                !debug.contains("secret") && !debug.contains("pw"),
                "{debug}"
            );
            let Error::PaginationOriginChanged { from, to } = error else {
                unreachable!()
            };
            assert_eq!(from.as_str(), "https://x/2");
            assert_eq!(to.as_str(), "https://elsewhere.example/3");
        }
        other => panic!("expected PaginationOriginChanged, got {other:?}"),
    }
    assert_eq!(backend.urls(), ["https://x/2?key=secret"]);
}

/// A paginator seeded with no next link answers `None` on its first pull without building or
/// sending a request.
#[test]
fn paginator_disarms_when_seeded_empty() {
    let empty: LinkPaginator<Vec<i64>> = LinkPaginator::from_headers(core(), &HeaderMap::new());
    let mut empty = empty;
    assert!(poll_ready(empty.next_page()).is_none());
}
