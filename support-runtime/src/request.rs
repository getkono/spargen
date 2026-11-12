//! Building a request as the transport seam carries it: [`RequestBuilder`] assembles an
//! [`http::Request`] whose body is the runtime's own [`HttpBody`], and [`MultipartForm`] encodes a
//! `multipart/form-data` body for it. Nothing here depends on the backend that will execute the
//! request, so no HTTP client is built to build a request.

use std::convert::Infallible;
use std::hash::{BuildHasher, Hasher};

use bytes::{Bytes, BytesMut};
use http::header::{HeaderMap, HeaderName, HeaderValue, CONTENT_TYPE};
use http::Method;
use url::Url;

use crate::{Error, HttpBody};

/// A request under construction: a method, an absolute URL, headers and a body, built into an
/// `http::Request<HttpBody>` by [`Self::build`].
///
/// Like the builders of the HTTP stacks it replaces, it defers failure: a header that does not
/// convert leaves the builder failed, every later step leaves it as it is, and [`Self::build`]
/// reports the first failure as [`Error::RequestConstruction`].
#[derive(Debug)]
pub struct RequestBuilder {
    request: Result<Parts, Error<Infallible>>,
}

/// What a [`RequestBuilder`] holds until it is built.
#[derive(Debug)]
struct Parts {
    method: Method,
    url: Url,
    headers: HeaderMap,
    body: HttpBody,
}

impl RequestBuilder {
    /// Start a `method` request to `url`, with no headers and an empty body.
    pub fn new(method: Method, url: Url) -> Self {
        Self {
            request: Ok(Parts {
                method,
                url,
                headers: HeaderMap::new(),
                body: HttpBody::empty(),
            }),
        }
    }

    /// Add a header, after any value the request already carries under `name`. A name or value
    /// that does not convert fails the builder.
    pub fn header<K, V>(mut self, name: K, value: V) -> Self
    where
        HeaderName: TryFrom<K>,
        <HeaderName as TryFrom<K>>::Error: Into<http::Error>,
        HeaderValue: TryFrom<V>,
        <HeaderValue as TryFrom<V>>::Error: Into<http::Error>,
    {
        if let Ok(parts) = &mut self.request {
            let header = HeaderName::try_from(name)
                .map_err(Into::into)
                .and_then(|name| {
                    HeaderValue::try_from(value)
                        .map_err(Into::into)
                        .map(|value| (name, value))
                });
            match header {
                Ok((name, value)) => {
                    parts.headers.append(name, value);
                }
                Err(error) => self.request = Err(Error::request_construction(error)),
            }
        }
        self
    }

    /// Set the body, replacing any set before.
    pub fn body(mut self, body: impl Into<HttpBody>) -> Self {
        if let Ok(parts) = &mut self.request {
            parts.body = body.into();
        }
        self
    }

    /// Set a `multipart/form-data` body encoded from `form`, and the `Content-Type` naming its
    /// boundary, replacing any `Content-Type` set before. The body is held in memory, so the
    /// request clones for a retry, a credential refresh or a reconnect.
    pub fn multipart(mut self, form: MultipartForm) -> Self {
        if let Ok(parts) = &mut self.request {
            let (content_type, body) = form.encode();
            // The boundary is hexadecimal, so the value always converts; were it refused, the
            // builder fails rather than sending a body its `Content-Type` does not describe.
            match HeaderValue::from_bytes(&content_type) {
                Ok(content_type) => {
                    parts.headers.insert(CONTENT_TYPE, content_type);
                    parts.body = HttpBody::from(body);
                }
                Err(error) => self.request = Err(Error::request_construction(error)),
            }
        }
        self
    }

    /// A copy of the builder, or `None` when its body is a stream, which can be read only once,
    /// or the builder has failed.
    pub fn try_clone(&self) -> Option<Self> {
        let parts = self.request.as_ref().ok()?;
        Some(Self {
            request: Ok(Parts {
                method: parts.method.clone(),
                url: parts.url.clone(),
                headers: parts.headers.clone(),
                body: parts.body.try_clone()?,
            }),
        })
    }

    /// The request's URL, for a step that rewrites it (an `apiKey` query credential); `None` once
    /// the builder has failed.
    pub(crate) fn url_mut(&mut self) -> Option<&mut Url> {
        self.request.as_mut().ok().map(|parts| &mut parts.url)
    }

    /// The request's headers, for a step that merges into one (a cookie `apiKey`); `None` once
    /// the builder has failed.
    pub(crate) fn headers_mut(&mut self) -> Option<&mut HeaderMap> {
        self.request.as_mut().ok().map(|parts| &mut parts.headers)
    }

    /// The request, or the first failure the builder met. A URL that is not a valid URI is
    /// refused here, as [`Error::RequestConstruction`].
    pub fn build(self) -> Result<http::Request<HttpBody>, Error<Infallible>> {
        let Parts {
            method,
            url,
            headers,
            body,
        } = self.request?;
        let uri = http::Uri::try_from(url.as_str()).map_err(Error::request_construction)?;
        let mut request = http::Request::new(body);
        *request.method_mut() = method;
        *request.uri_mut() = uri;
        *request.headers_mut() = headers;
        Ok(request)
    }
}

/// A `multipart/form-data` body (RFC 7578): named parts, encoded in the order they were added.
///
/// Every part is held in memory, so the encoded body is too. The boundary is chosen when the form
/// is encoded, and is one that no part contains.
#[derive(Debug, Default)]
pub struct MultipartForm {
    parts: Vec<(String, MultipartPart)>,
}

impl MultipartForm {
    /// A form with no parts.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a part named `name`, after every part added before; a name may repeat.
    pub fn part(mut self, name: impl Into<String>, part: MultipartPart) -> Self {
        self.parts.push((name.into(), part));
        self
    }

    /// The `Content-Type` naming the boundary, and the encoded body.
    fn encode(self) -> (Vec<u8>, Bytes) {
        self.encode_with(boundary)
    }

    /// [`Self::encode`], drawing candidate boundaries from `candidate`, seed by seed from zero,
    /// until one occurs in no part.
    fn encode_with(self, mut candidate: impl FnMut(u64) -> Vec<u8>) -> (Vec<u8>, Bytes) {
        let heads: Vec<Vec<u8>> = self
            .parts
            .iter()
            .map(|(name, part)| part.head(name))
            .collect();
        let mut seed = 0_u64;
        let boundary = loop {
            let boundary = candidate(seed);
            let clashes = self.parts.iter().zip(&heads).any(|((_, part), head)| {
                contains(head, &boundary) || contains(&part.body, &boundary)
            });
            if !clashes {
                break boundary;
            }
            seed += 1;
        };
        let length = self
            .parts
            .iter()
            .zip(&heads)
            .map(|((_, part), head)| boundary.len() + head.len() + part.body.len() + 8)
            .sum::<usize>()
            + boundary.len()
            + 6;
        let mut body = BytesMut::with_capacity(length);
        for ((_, part), head) in self.parts.iter().zip(&heads) {
            body.extend_from_slice(b"--");
            body.extend_from_slice(&boundary);
            body.extend_from_slice(b"\r\n");
            body.extend_from_slice(head);
            body.extend_from_slice(b"\r\n");
            body.extend_from_slice(&part.body);
            body.extend_from_slice(b"\r\n");
        }
        body.extend_from_slice(b"--");
        body.extend_from_slice(&boundary);
        body.extend_from_slice(b"--\r\n");
        let mut content_type = b"multipart/form-data; boundary=".to_vec();
        content_type.extend_from_slice(&boundary);
        (content_type, body.freeze())
    }
}

/// One part of a [`MultipartForm`]: its bytes, and the `Content-Type` and other headers it
/// carries beside its `Content-Disposition`.
#[derive(Debug, Clone)]
pub struct MultipartPart {
    body: Bytes,
    /// The part's own `Content-Type`.
    mime: Option<HeaderValue>,
    headers: HeaderMap,
}

impl MultipartPart {
    /// A part holding `text` as UTF-8, with no `Content-Type` of its own (RFC 7578 §4.4 reads
    /// one without it as `text/plain`).
    pub fn text(text: impl Into<String>) -> Self {
        Self::bytes(text.into())
    }

    /// A part holding `bytes`, with no `Content-Type` of its own.
    pub fn bytes(bytes: impl Into<Bytes>) -> Self {
        Self {
            body: bytes.into(),
            mime: None,
            headers: HeaderMap::new(),
        }
    }

    /// Set the part's `Content-Type`. One that is not a valid header value is refused as
    /// [`Error::RequestConstruction`].
    pub fn mime_str(mut self, mime: &str) -> Result<Self, Error<Infallible>> {
        self.mime = Some(HeaderValue::from_str(mime).map_err(Error::request_construction)?);
        Ok(self)
    }

    /// Set the headers the part carries after its `Content-Disposition` and `Content-Type`,
    /// replacing any set before.
    pub fn headers(mut self, headers: HeaderMap) -> Self {
        self.headers = headers;
        self
    }

    /// The part's header lines, each ending in CRLF: `Content-Disposition` naming it, then its
    /// `Content-Type`, then its other headers.
    ///
    /// The name is written as the WHATWG HTML standard's `multipart/form-data` encoding writes a
    /// form field's: as UTF-8, quoted, with `"`, CR and LF percent-encoded, and nothing else
    /// escaped. RFC 7578 §4.2 rules out the RFC 2231 `name*=` form.
    fn head(&self, name: &str) -> Vec<u8> {
        let mut head = b"Content-Disposition: form-data; name=\"".to_vec();
        for byte in name.bytes() {
            match byte {
                b'"' => head.extend_from_slice(b"%22"),
                b'\r' => head.extend_from_slice(b"%0D"),
                b'\n' => head.extend_from_slice(b"%0A"),
                byte => head.push(byte),
            }
        }
        head.extend_from_slice(b"\"\r\n");
        // `Content-Type` is written in the case RFC 7578's examples use; any other header in the
        // lowercase `HeaderName` holds, which every reader matches without regard to case.
        let mime = self.mime.iter().map(|value| ("Content-Type", value));
        let headers = self
            .headers
            .iter()
            .map(|(name, value)| (name.as_str(), value));
        for (name, value) in mime.chain(headers) {
            head.extend_from_slice(name.as_bytes());
            head.extend_from_slice(b": ");
            head.extend_from_slice(value.as_bytes());
            head.extend_from_slice(b"\r\n");
        }
        head
    }
}

/// A boundary of 32 lowercase hexadecimal digits, from `std`'s randomly keyed hasher (keyed per
/// process where the platform supplies randomness) over `seed`. [`MultipartForm`] moves to the
/// next seed while a part contains the boundary, so a part cannot end the body early whatever the
/// hasher's keys are.
fn boundary(seed: u64) -> Vec<u8> {
    let state = std::collections::hash_map::RandomState::new();
    let mut boundary = Vec::with_capacity(32);
    for half in 0..2_u64 {
        let mut hasher = state.build_hasher();
        hasher.write_u64(seed);
        hasher.write_u64(half);
        boundary.extend_from_slice(format!("{:016x}", hasher.finish()).as_bytes());
    }
    boundary
}

/// Whether `haystack` contains `needle` anywhere.
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

#[cfg(test)]
mod tests;
