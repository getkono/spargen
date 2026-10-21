use reqwest::header::{HeaderMap, HeaderValue};

use super::{next_link, next_link_target, LinkPaginator};
use crate::tests::poll_ready;
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

/// Synthesize an in-memory `reqwest::Response` (no server, no runtime) with a status, JSON body,
/// and optional `Link` header, so `next_page`'s post-`send` half can be driven poll-once.
fn response_with(status: u16, body: &str, link: Option<&str>) -> reqwest::Response {
    let mut builder = http::Response::builder().status(status);
    if let Some(link) = link {
        builder = builder.header(reqwest::header::LINK, link);
    }
    reqwest::Response::from(
        builder
            .body(body.to_owned())
            .expect("valid synthetic response"),
    )
}

fn ok_page(body: &str, link: Option<&str>) -> reqwest::Response {
    response_with(200, body, link)
}

/// Drive `next_page`'s state machine directly over in-memory responses via `handle_response` —
/// `next_page` only adds the socket-bound `send`, so feeding the synthetic responses to
/// `handle_response` (after mirroring the `self.next.take()`) exercises the real Link-following,
/// decode, and re-arm logic end to end.
#[test]
fn next_page_advances_next_from_link_header_then_terminates() {
    let mut pager: LinkPaginator<Vec<i64>> = LinkPaginator::new(
        core(),
        reqwest::Url::parse("https://x/items?page=1").unwrap(),
    );

    // Page 1: 200 carrying a `rel="next"` → decodes and re-arms `next` to page 2.
    let url1 = pager.next.take().expect("seeded first url");
    let page1 = ok_page(r#"[1,2]"#, Some(r#"<https://x/items?page=2>; rel="next""#));
    let decoded1 = poll_ready(pager.handle_response(url1, page1))
        .unwrap()
        .unwrap();
    assert_eq!(decoded1.into_inner(), vec![1, 2]);
    assert_eq!(
        pager.next.as_ref().map(reqwest::Url::as_str),
        Some("https://x/items?page=2")
    );

    // Page 2: 200 with no `rel="next"` → decodes and disarms; the chain then ends.
    let url2 = pager.next.take().expect("re-armed second url");
    let page2 = ok_page(r#"[3,4]"#, None);
    let decoded2 = poll_ready(pager.handle_response(url2, page2))
        .unwrap()
        .unwrap();
    assert_eq!(decoded2.into_inner(), vec![3, 4]);
    assert!(!pager.has_next());
    // A further pull makes no request (no armed url) and returns `None`.
    assert!(poll_ready(pager.next_page()).is_none());
}

#[test]
fn non_success_page_terminates_as_unexpected_status_not_decoded() {
    // `send` returns `Ok` for 4xx/5xx, so `next_page` gates on status before decoding. A follow-up
    // page that 401s or 500s (plausible: the generic paginator attaches no per-op credentials)
    // must surface as `UnexpectedStatus` and terminate the chain — never decoded as `T`, never a
    // `Decode` error. The error body here structurally fits the lenient page type `Vec<Value>`, so
    // without the status gate `decode_success` would happily accept it as a page.
    for status in [401u16, 500u16] {
        let mut pager: LinkPaginator<Vec<serde_json::Value>> =
            LinkPaginator::new(core(), reqwest::Url::parse("https://x/1").unwrap());
        // Mirror `next_page`'s `self.next.take()` (the send() it wraps needs a live socket).
        let url = pager.next.take().expect("seeded");
        let resp = response_with(
            status,
            r#"[{"message":"denied"}]"#,
            Some(r#"<https://x/2>; rel="next""#),
        );
        let outcome = poll_ready(pager.handle_response(url, resp));
        assert!(
            matches!(&outcome, Some(Err(Error::UnexpectedStatus { .. }))),
            "status {status}: expected UnexpectedStatus, got {outcome:?}"
        );
        // Terminated: the `rel="next"` link was NOT followed, and the next pull returns `None`
        // without issuing a request.
        assert!(!pager.has_next());
        assert!(poll_ready(pager.next_page()).is_none());
    }
}

fn core() -> ClientCore {
    ClientCore::new("https://x").unwrap()
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
        let mut pager: LinkPaginator<Vec<i64>> = LinkPaginator::new(
            core(),
            reqwest::Url::parse("https://x/api/items?page=1").unwrap(),
        );
        let url = pager.next.take().expect("seeded first url");
        let link = format!(r#"<{target}>; rel="next""#);
        let page = ok_page("[1]", Some(&link));
        let decoded = poll_ready(pager.handle_response(url, page))
            .unwrap()
            .unwrap();
        assert_eq!(decoded.into_inner(), vec![1]);
        assert_eq!(
            pager.next.as_ref().map(reqwest::Url::as_str),
            Some(resolved),
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
    let mut pager: LinkPaginator<Vec<i64>> =
        LinkPaginator::new(core(), reqwest::Url::parse("https://x/1").unwrap());
    let url = pager.next.take().expect("seeded");
    let page = response_with(
        203,
        r#"{"not":"a list"}"#,
        Some(r#"<https://x/2>; rel="next""#),
    );
    match poll_ready(pager.handle_response(url, page)) {
        Some(Err(Error::Decode { status, .. })) => assert_eq!(status.as_u16(), 203),
        other => panic!("expected a Decode error, got {other:?}"),
    }
    assert!(!pager.has_next());
    assert!(poll_ready(pager.next_page()).is_none());
}

/// A paginator seeded with no next link answers `None` on its first pull without building or
/// sending a request. A `next_page` that does have a URL to fetch reaches `send`, which needs
/// a live socket this poll-once harness cannot provide, so its post-`send` half is driven
/// through `handle_response` in the tests above instead.
#[test]
fn paginator_disarms_when_seeded_empty() {
    let empty: LinkPaginator<Vec<i64>> = LinkPaginator::from_headers(core(), &HeaderMap::new());
    let mut empty = empty;
    assert!(poll_ready(empty.next_page()).is_none());
}
