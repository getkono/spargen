//! Generic, freestanding pagination helpers usable with any operation.
//!
//! OpenAPI has no standard machine-readable pagination declaration, so per-operation
//! auto-paginators cannot be synthesised from a spec. These helpers are instead *generic* runtime
//! utilities that a caller drives explicitly, in the same futures-free, manual async-iterator style
//! as [`crate::EventStream`] — no `futures` crate, no new dependencies.
//!
//! # Link-header pagination (RFC 5988 / RFC 8288)
//!
//! [`LinkPaginator<T>`] follows the common `Link: <url>; rel="next"` scheme (the GitHub-style
//! convention). It is detectable purely at runtime with no spec metadata: each page's response
//! carries the URL of the next page in its `Link` header. Drive it manually:
//!
//! ```ignore
//! let first = reqwest::Url::parse("https://api.example.com/items?page=1")?;
//! let mut pages = client.core().paginate_links::<Vec<Item>>(first);
//! while let Some(page) = pages.next_page().await {
//!     let items = page?.into_inner(); // a decoded `Vec<Item>` for this page
//! }
//! ```
//!
//! The generic paginator issues a plain `GET` for each page and does **not** attach per-operation
//! security credentials — it has no operation context to know which scheme applies. To authenticate
//! follow-up page requests, inject a [`reqwest::Client`] pre-configured with the appropriate default
//! headers (e.g. an `Authorization` header) via `Client::with_client`; every page request then
//! carries them. A page that comes back non-`2xx` (e.g. a `401` on an unauthenticated page 2)
//! terminates the paginator as an [`Error::UnexpectedStatus`](crate::Error), never a decoded page.
//!
//! Termination is otherwise entirely driven by the server's `rel="next"` links, so a buggy or
//! malicious server can advertise a self-referential or cyclic `next`, yielding an unbounded chain.
//! The iterator is pull-based (nothing is fetched until you call `next_page`), so bound your own loop
//! — e.g. cap the number of pages you will follow.
//!
//! # Cursor / offset / page-number pagination (bring your own)
//!
//! Schemes the `Link` header cannot express — an opaque cursor or a `next_offset` field in the
//! response body — are driven by the caller with a small loop over the ordinary generated operation
//! method, using the fluent parameter setters to advance the cursor:
//!
//! ```ignore
//! let mut cursor: Option<String> = None;
//! loop {
//!     let mut params = ListThingsParams::default();
//!     if let Some(c) = &cursor {
//!         params = params.cursor(c.clone());
//!     }
//!     let page = client.list_things(params).await?.into_inner();
//!     handle(&page.items);
//!     match page.next_cursor {
//!         Some(next) => cursor = Some(next), // more pages
//!         None => break,                     // done
//!     }
//! }
//! ```
//!
//! No generic closure/future combinator is provided for this: Rust closures returning futures are
//! awkward to type, and the plain loop over the already-public [`crate::send`] /
//! [`crate::decode_success`] primitives (which the generated methods use) is clearer and fully
//! typed. The loop pattern above is the supported shape.

use std::convert::Infallible;
use std::marker::PhantomData;

use reqwest::header::HeaderMap;
use reqwest::{Response, Url};
use serde::de::DeserializeOwned;

use crate::{decode_success, send, unexpected_status, ClientCore, Error, ResponseValue};

/// A manual async iterator over the pages of a `Link`-header-paginated collection.
///
/// Holds the [`ClientCore`] and the URL of the next page to fetch (if any). Each [`Self::next_page`]
/// `GET`s the current URL, decodes its body as `T`, reads the response's `Link: <…>; rel="next"`
/// header to find the following page, and yields the decoded page. It returns `None` once a response
/// carries no `rel="next"` link — or after any error, which terminates the paginator.
///
/// `T` is the decoded body type of one page: typically a `Vec<Item>` for an array collection, or a
/// wrapper struct whose fields include the items. Dropping the paginator is safe (standard HTTP drop
/// semantics).
pub struct LinkPaginator<T> {
    core: ClientCore,
    /// The next page to fetch; `None` once the chain is exhausted or an error terminated it.
    next: Option<Url>,
    /// `T` is produced, never consumed; the `fn() -> T` marker keeps `LinkPaginator<T>: Send + Sync`
    /// regardless of `T`.
    _marker: PhantomData<fn() -> T>,
}

impl<T> LinkPaginator<T> {
    /// Seed a paginator from the URL of the first page. No request is made until the first
    /// [`Self::next_page`] call.
    pub fn new(core: ClientCore, first: Url) -> Self {
        Self {
            core,
            next: Some(first),
            _marker: PhantomData,
        }
    }

    /// Seed a paginator from the headers of an already-fetched first response (the `rel="next"`
    /// link), so a caller who fetched page one through a normal operation method can continue from
    /// its headers. Only an absolute `next` URL is followed; a relative one yields no next page (use
    /// [`Self::new`] with the resolved URL instead).
    pub fn from_headers(core: ClientCore, headers: &HeaderMap) -> Self {
        Self {
            core,
            next: next_link(headers),
            _marker: PhantomData,
        }
    }

    /// Whether another page will be fetched by the next [`Self::next_page`] call.
    pub fn has_next(&self) -> bool {
        self.next.is_some()
    }
}

impl<T: DeserializeOwned> LinkPaginator<T> {
    /// Fetch and decode the next page, or return `None` at the end of the chain.
    ///
    /// `GET`s the current `next` URL, parses its response `Link` header for `rel="next"` (resolving a
    /// relative target against the fetched URL) to arm the following page, then decodes the body as
    /// `T`. Three things terminate the paginator (subsequent calls return `None`):
    ///
    /// * a **transport failure** → `Some(Err(..))`;
    /// * a **non-success HTTP status** (anything outside `2xx`) → `Some(Err(Error::UnexpectedStatus))`
    ///   — the body is *never* decoded as `T`, so an error payload can't masquerade as a page. The
    ///   generic paginator has no per-operation error taxonomy, and it attaches no per-operation
    ///   credentials, so a follow-up page that `401`s (see the auth caveat in the [module docs](self))
    ///   surfaces here rather than being silently mis-decoded;
    /// * a **decode failure** on a `2xx` body → `Some(Err(Error::Decode))`.
    ///
    /// When a `2xx` page carries no `rel="next"` link, the page is yielded and the following call
    /// returns `None`.
    pub async fn next_page(&mut self) -> Option<Result<ResponseValue<T>, Error<Infallible>>> {
        // Take the pending URL: the paginator is now disarmed and only re-armed on a successful
        // decode, so any error path below leaves `next` as `None` and terminates the chain.
        let url = self.next.take()?;
        let request = match self.core.http().get(url.clone()).build() {
            Ok(request) => request,
            Err(error) => return Some(Err(Error::request_construction(error))),
        };
        let response = match send(&self.core, request).await {
            Ok(response) => response,
            Err(error) => return Some(Err(error)),
        };
        self.handle_response(url, response).await
    }

    /// The post-`send` half of [`Self::next_page`]: gate on status, follow the `Link` header, decode.
    /// Split out so it can be driven directly over an in-memory [`Response`] in the unit tests (the
    /// `send` above needs a live socket, which the runtime's poll-once harness cannot provide). On
    /// entry `self.next` has already been taken (is `None`), so every non-success path here leaves the
    /// paginator disarmed; only a successful decode re-arms it.
    async fn handle_response(
        &mut self,
        url: Url,
        response: Response,
    ) -> Option<Result<ResponseValue<T>, Error<Infallible>>> {
        // `send` returns `Ok` for 4xx/5xx too (only transport failures error), so gate on status
        // exactly as every generated operation does: a non-2xx page is NOT decoded as `T`, it
        // terminates the chain as an unexpected-status error (the generic paginator carries no per-op
        // error taxonomy). This is the natural terminal for e.g. a 401 on an unauthenticated page 2.
        if !response.status().is_success() {
            return Some(Err(
                unexpected_status::<Infallible>(&self.core, response).await
            ));
        }
        // Capture the next-page link before the body is consumed, resolving a relative URI-reference
        // against the URL just fetched (RFC 8288 permits relative targets; GitHub uses absolute ones).
        let next = next_link_target(response.headers()).and_then(|target| url.join(&target).ok());
        match decode_success::<T>(&self.core, response).await {
            Ok(value) => {
                self.next = next;
                Some(Ok(value))
            }
            Err(error) => Some(Err(error)),
        }
    }
}

impl ClientCore {
    /// Start a [`LinkPaginator<T>`] over a `Link`-header-paginated collection, beginning at `first`.
    /// See the [module docs](self) for the full pattern and the authentication caveat.
    pub fn paginate_links<T>(&self, first: Url) -> LinkPaginator<T> {
        LinkPaginator::new(self.clone(), first)
    }
}

/// Extract the `rel="next"` target from a response's `Link` header(s) as an absolute [`Url`], or
/// `None` when there is no usable next link. Pure and side-effect-free — the building block generated
/// clients and callers can reuse directly. A relative target is not resolved here (there is no base);
/// [`LinkPaginator::next_page`] resolves relatives against the fetched page URL.
pub fn next_link(headers: &HeaderMap) -> Option<Url> {
    next_link_target(headers).and_then(|target| Url::parse(&target).ok())
}

/// The RFC 8288 core: find the first `rel="next"` link across all `Link` header values and return its
/// raw URI-reference (the text between `<` and `>`). Tolerant of quoted/unquoted `rel`, extra
/// parameters, arbitrary whitespace, multiple links per header line and multiple header lines, and
/// space-separated multi-value `rel` (e.g. `rel="next foo"`). Malformed input yields `None` — never a
/// panic.
fn next_link_target(headers: &HeaderMap) -> Option<String> {
    for value in headers.get_all(reqwest::header::LINK) {
        let Ok(field) = value.to_str() else {
            continue;
        };
        for entry in split_link_entries(field) {
            if let Some((uri, is_next)) = parse_link_entry(entry) {
                if is_next {
                    return Some(uri);
                }
            }
        }
    }
    None
}

/// Split one `Link` header field value into its comma-separated entries, treating commas inside the
/// `<…>` URI-reference or inside a quoted parameter string as literal (not separators).
fn split_link_entries(field: &str) -> Vec<&str> {
    let bytes = field.as_bytes();
    let mut entries = Vec::new();
    let mut start = 0;
    let mut in_angle = false;
    let mut in_quote = false;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if in_quote => {
                // A backslash escapes the next character inside a quoted string; skip both.
                i += 1;
            }
            b'<' if !in_quote => in_angle = true,
            b'>' if !in_quote => in_angle = false,
            b'"' if !in_angle => in_quote = !in_quote,
            b',' if !in_angle && !in_quote => {
                entries.push(&field[start..i]);
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    entries.push(&field[start..]);
    entries
}

/// Parse a single `Link` entry into its URI-reference plus whether its `rel` names `next`. Returns
/// `None` if no `<…>` URI is present.
fn parse_link_entry(entry: &str) -> Option<(String, bool)> {
    let entry = entry.trim();
    let open = entry.find('<')?;
    let close = entry[open + 1..].find('>')? + open + 1;
    let uri = entry[open + 1..close].trim().to_owned();
    if uri.is_empty() {
        return None;
    }
    let mut is_next = false;
    for param in split_params(&entry[close + 1..]) {
        let param = param.trim();
        let Some(eq) = param.find('=') else {
            continue;
        };
        if !param[..eq].trim().eq_ignore_ascii_case("rel") {
            continue;
        }
        let mut value = param[eq + 1..].trim();
        if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
            value = &value[1..value.len() - 1];
        }
        // `rel` is a space-separated list of relation types; relation types are case-insensitive.
        if value
            .split_whitespace()
            .any(|rel| rel.eq_ignore_ascii_case("next"))
        {
            is_next = true;
        }
    }
    Some((uri, is_next))
}

/// Split an entry's parameter section on `;`, treating semicolons inside a quoted string as literal.
fn split_params(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    let mut parts = Vec::new();
    let mut start = 0;
    let mut in_quote = false;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if in_quote => i += 1,
            b'"' => in_quote = !in_quote,
            b';' if !in_quote => {
                parts.push(&text[start..i]);
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    parts.push(&text[start..]);
    parts
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::task::{Context, Poll, Waker};

    use reqwest::header::{HeaderMap, HeaderValue};

    use super::{next_link, next_link_target, LinkPaginator};
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

    // A poll-once driver (noop waker, no async runtime) proving `next_page` GETs a page, decodes it,
    // advances `next` from the `Link` header, and terminates when the header is absent.
    fn poll_ready<F: Future>(future: F) -> F::Output {
        let mut future = std::pin::pin!(future);
        match future
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
        {
            Poll::Ready(value) => value,
            Poll::Pending => panic!("future was not immediately ready"),
        }
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

    /// A real end-to-end drive of `next_page` over an in-memory response would need a live socket for
    /// `send()`, which the runtime's poll-once test harness cannot provide; the step-wise test above
    /// exercises every non-socket branch (`next_link_target` + `decode_success` + re-arm) that
    /// `next_page` composes. This test pins that `next_page` is a thin await-chain around them by
    /// constructing the paginator and asserting it is armed/disarmed as expected.
    #[test]
    fn paginator_disarms_when_seeded_empty() {
        let empty: LinkPaginator<Vec<i64>> = LinkPaginator::from_headers(core(), &HeaderMap::new());
        let mut empty = empty;
        assert!(poll_ready(empty.next_page()).is_none());
    }
}
