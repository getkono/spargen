//! Generic, freestanding pagination helpers usable with any operation.
//!
//! OpenAPI has no standard machine-readable pagination declaration, so per-operation
//! auto-paginators cannot be synthesised from a spec. These helpers are instead *generic* runtime
//! utilities that a caller drives explicitly through an inherent async method, with no additional
//! stream-combinator dependency.
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
    ///   credentials, so a follow-up page that `401`s (see the auth caveat in this module's docs)
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
    /// See this module's docs for the full pattern and the authentication caveat.
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
            if let Some((uri, true)) = parse_link_entry(entry) {
                return Some(uri);
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
mod tests;
