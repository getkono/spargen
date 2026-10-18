//! Interceptor middleware for the transport seam: [`MiddlewareBackend`] wraps any inner
//! [`HttpBackend`] with an ordered chain of [`Middleware`] that can observe/modify a request before
//! it is sent and the response after, short-circuit before the transport runs, and do async work
//! around the call.
//!
//! ## The `Next` continuation (tower-like, no dependencies)
//!
//! A middleware receives the request plus a [`Next`] handle. Calling [`Next::run`] advances to the
//! rest of the chain — the next middleware, or, when the chain is exhausted, the inner
//! [`HttpBackend`]. A middleware may inspect/modify the request before calling `run`, inspect the
//! [`reqwest::Response`] after, or return a response *without* calling `run` at all to
//! short-circuit. This is
//! the classic "onion" middleware shape, expressed with std's `Future`/`Pin`/`Box` — no
//! `async-trait` and no `tower`, so middleware adds no dependency to the generated runtime.
//!
//! ### How the lifetimes work without `async-trait`
//!
//! [`Middleware::handle`] is `handle<'a>(&'a self, request, next: Next<'a>) -> …Future… + 'a`: the
//! `'a` ties the borrow of the middleware, the [`Next`] it is handed, and the boxed future it
//! returns to a single lifetime. [`Next<'a>`] holds only borrows — `&'a [Arc<dyn Middleware>]` for
//! the remaining chain and `&'a Arc<dyn HttpBackend>` for the inner transport — so advancing the
//! chain never clones or reallocates; it just narrows the slice. [`MiddlewareBackend::execute`]
//! returns [`ExecuteFuture<'_>`], whose `'_` is exactly the borrow of `&self` the [`Next`] is built
//! over, so the whole chain's future safely borrows the (Arc-held) middleware and backend for the
//! duration of the call. On native targets `Arc<dyn Middleware>`/`Arc<dyn HttpBackend>` are
//! `Send + Sync` (via the `MaybeSend`/`MaybeSync` supertraits, which collapse to `Send`/`Sync`
//! off wasm), so references to them are `Send` and the boxed futures stay `Send`; on `wasm32`
//! these bounds are relaxed to match reqwest's single-threaded `fetch` backend.
//!
//! ## Ordering
//!
//! The first middleware in the vector runs **outermost**: it sees the request first and the response
//! last. The last middleware runs innermost, closest to the transport. So
//! `MiddlewareBackend::new(inner).layer(a).layer(b)` runs `a` around `b` around `inner`.
//!
//! ## Composability
//!
//! [`MiddlewareBackend`] wraps *any* [`HttpBackend`], so it composes freely with the retry adapter
//! ([`crate::RetryBackend`]) and the default [`crate::ReqwestBackend`] in either order. Wrapping a
//! `RetryBackend` in a `MiddlewareBackend` runs the middleware **once** around the whole retry loop;
//! wrapping a `MiddlewareBackend` in a `RetryBackend` re-runs the middleware **per attempt**. Pick
//! the nesting that matches whether an interceptor should observe each attempt or just the final
//! outcome.
//!
//! ```ignore
//! use std::sync::Arc;
//!
//! let inner: Arc<dyn HttpBackend> = Arc::new(ReqwestBackend::new(reqwest::Client::new()));
//! let backend: Arc<dyn HttpBackend> = Arc::new(
//!     MiddlewareBackend::new(inner).layer(Arc::new(MyLogger)),
//! );
//! let client = Client::with_backend(backend, "https://api.example.com")?;
//! ```

use std::sync::Arc;

use reqwest::Request;

use crate::transport::{ExecuteFuture, HttpBackend};
use crate::{MaybeSend, MaybeSync};

/// One link in the interceptor chain: it observes/modifies a request on the way in, calls
/// [`Next::run`] to proceed (eventually reaching the transport), and observes/modifies the
/// [`reqwest::Response`] on the way out — or returns early to short-circuit.
///
/// Implementations must be [`MaybeSend`] + [`MaybeSync`] — `Send + Sync` on native (the `Client` is
/// shared across tasks), vacuous on `wasm32` — and `Debug` (so [`MiddlewareBackend`] — and thus
/// [`crate::ClientCore`] — stays `Debug`). The returned future is manually boxed so the trait is
/// object-safe without an `async-trait` dependency.
pub trait Middleware: MaybeSend + MaybeSync + std::fmt::Debug {
    /// Handle a request. Call `next.run(request)` to proceed to the rest of the chain (eventually
    /// the transport), or return a response without calling it to short-circuit. May inspect/modify
    /// the request before and the [`reqwest::Response`] after.
    fn handle<'a>(&'a self, request: Request, next: Next<'a>) -> ExecuteFuture<'a>;
}

/// The continuation handed to a [`Middleware`]: it represents "the rest of the chain" — the
/// remaining middlewares followed by the inner transport.
///
/// Holds only borrows (`'a`) into the [`MiddlewareBackend`], so advancing the chain is allocation-
/// free. Call [`Next::run`] to proceed; dropping a `Next` without calling `run` is how a middleware
/// short-circuits.
pub struct Next<'a> {
    /// The middlewares that still have to run, in order; the first is the next to run.
    remaining: &'a [Arc<dyn Middleware>],
    /// The transport reached once `remaining` is empty.
    inner: &'a Arc<dyn HttpBackend>,
}

impl<'a> Next<'a> {
    /// Proceed to the rest of the chain: run the next middleware, or — when no middlewares remain —
    /// execute the request on the inner transport. Returns the eventual [`reqwest::Response`] or the
    /// [`crate::TransportError`] the transport failed with.
    pub fn run(self, request: Request) -> ExecuteFuture<'a> {
        match self.remaining.split_first() {
            // Peel off the outermost remaining middleware and hand it a `Next` over the rest.
            Some((first, rest)) => first.handle(
                request,
                Next {
                    remaining: rest,
                    inner: self.inner,
                },
            ),
            // Chain exhausted: hand the request to the transport. `execute` borrows `*inner` for
            // `'a`, so the future it returns is `'a` — exactly what this method promises.
            None => self.inner.execute(request),
        }
    }
}

/// An [`HttpBackend`] that runs an ordered chain of [`Middleware`] around an inner backend.
///
/// The first middleware in the chain runs outermost (see this module's docs for ordering and
/// composability). Purely additive: it takes effect only when a caller wraps their backend in it and
/// installs it via `Client::with_backend`.
#[derive(Clone)]
pub struct MiddlewareBackend {
    inner: Arc<dyn HttpBackend>,
    middlewares: Vec<Arc<dyn Middleware>>,
}

impl MiddlewareBackend {
    /// Start a chain around `inner` with no middlewares; add them with [`Self::layer`].
    pub fn new(inner: Arc<dyn HttpBackend>) -> Self {
        Self {
            inner,
            middlewares: Vec::new(),
        }
    }

    /// Wrap `inner` with a pre-built ordered list of middlewares (index 0 runs outermost).
    pub fn with_middlewares(
        inner: Arc<dyn HttpBackend>,
        middlewares: Vec<Arc<dyn Middleware>>,
    ) -> Self {
        Self { inner, middlewares }
    }

    /// Append a middleware to the chain and return `self`, for builder-style construction. Each
    /// appended middleware runs *inside* the ones added before it (the first added is outermost).
    #[must_use]
    pub fn layer(mut self, middleware: Arc<dyn Middleware>) -> Self {
        self.middlewares.push(middleware);
        self
    }

    /// The wrapped inner backend.
    pub fn inner(&self) -> &Arc<dyn HttpBackend> {
        &self.inner
    }

    /// The middleware chain, in run order (index 0 is outermost).
    pub fn middlewares(&self) -> &[Arc<dyn Middleware>] {
        &self.middlewares
    }
}

// `Middleware: Debug`, so the chain is `Debug`; a manual impl keeps the field names tidy and matches
// `RetryBackend`'s non-exhaustive style.
impl std::fmt::Debug for MiddlewareBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MiddlewareBackend")
            .field("inner", &self.inner)
            .field("middlewares", &self.middlewares)
            .finish()
    }
}

impl HttpBackend for MiddlewareBackend {
    fn execute(&self, request: Request) -> ExecuteFuture<'_> {
        // Build a `Next` over the full chain and run it. The returned future borrows `&self` for the
        // `'_` of `execute`, which is exactly the `'a` the `Next` (and every middleware future in the
        // chain) is tied to — no `Arc` clones needed to satisfy the borrow checker.
        Next {
            remaining: &self.middlewares,
            inner: &self.inner,
        }
        .run(request)
    }
}

#[cfg(test)]
mod tests;
