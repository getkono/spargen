//! Conditional `Send`/`Sync` bounds so a generated client compiles on both native targets and
//! `wasm32-unknown-unknown` (the browser, via reqwest's `fetch` backend).
//!
//! On native, reqwest's client/request/response futures are `Send` and a generated `Client` is
//! shared across threads and tasks — so the transport seam ([`crate::HttpBackend`]) and its helpers
//! ([`crate::Middleware`], [`crate::RetryPolicy`], the auth token provider) require `Send`/`Sync`.
//! On wasm the browser is single-threaded and reqwest's `fetch`-backed futures are `!Send`; those
//! same bounds would refuse to compile.
//!
//! [`MaybeSend`]/[`MaybeSync`] bridge the two: on every non-wasm target they are exactly `Send`/
//! `Sync` (a blanket impl plus a `Send`/`Sync` supertrait, so a `dyn Trait: MaybeSend` object is
//! still `Send`), and on wasm they are vacuous marker traits implemented for every type. Because the
//! native definition *is* `Send`/`Sync`, native behavior, bounds, and trait-object auto-traits are
//! unchanged — the abstraction collapses away off wasm.
//!
//! These relax *supertrait* and *generic* bounds. The boxed trait-object futures the seam returns
//! (`Pin<Box<dyn Future + Send>>`) cannot use a non-auto trait as an extra object bound, so those
//! aliases (`ExecuteFuture`, `RetryWait`, `TokenFuture`, `TokenProvider`) are instead `cfg`-gated to
//! carry `+ Send`/`+ Sync` off wasm and drop it on wasm. The two mechanisms compose to one set of
//! source that builds on both targets.

/// `Send` on native targets; a vacuous marker on `wasm32`.
///
/// On native it has `Send` as a supertrait with a blanket impl for every `Send` type, so a bound of
/// `MaybeSend` is byte-for-byte equivalent to `Send` (including making `dyn Trait: MaybeSend` trait
/// objects `Send`). On wasm it is implemented for all types and imposes nothing.
#[cfg(not(target_arch = "wasm32"))]
pub trait MaybeSend: Send {}
#[cfg(not(target_arch = "wasm32"))]
impl<T: Send + ?Sized> MaybeSend for T {}

/// `Send` on native targets; a vacuous marker on `wasm32` (the single-threaded browser target).
#[cfg(target_arch = "wasm32")]
pub trait MaybeSend {}
#[cfg(target_arch = "wasm32")]
impl<T: ?Sized> MaybeSend for T {}

/// `Sync` on native targets; a vacuous marker on `wasm32`.
///
/// The `Sync` counterpart to [`MaybeSend`]: on native, `Sync` with a blanket impl for every `Sync`
/// type; on wasm, implemented for all types and imposing nothing.
#[cfg(not(target_arch = "wasm32"))]
pub trait MaybeSync: Sync {}
#[cfg(not(target_arch = "wasm32"))]
impl<T: Sync + ?Sized> MaybeSync for T {}

/// `Sync` on native targets; a vacuous marker on `wasm32` (the single-threaded browser target).
#[cfg(target_arch = "wasm32")]
pub trait MaybeSync {}
#[cfg(target_arch = "wasm32")]
impl<T: ?Sized> MaybeSync for T {}

#[cfg(test)]
mod tests {
    use super::{MaybeSend, MaybeSync};

    fn assert_maybe_send<T: MaybeSend + ?Sized>() {}
    fn assert_maybe_sync<T: MaybeSync + ?Sized>() {}

    #[test]
    fn ordinary_types_are_maybe_send_and_maybe_sync() {
        assert_maybe_send::<i32>();
        assert_maybe_sync::<i32>();
        assert_maybe_send::<String>();
        assert_maybe_sync::<String>();
    }

    // On native, `MaybeSend`/`MaybeSync` must be *exactly* `Send`/`Sync`, so a trait carrying them as
    // supertraits yields a `Send + Sync` trait object — this is what keeps a generated `Client`
    // shareable across threads unchanged. (On wasm these bounds are vacuous, so the assertion is
    // native-only.)
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn native_maybe_bounds_make_trait_objects_send_and_sync() {
        trait Marker: MaybeSend + MaybeSync {}
        fn assert_send_sync<T: Send + Sync + ?Sized>() {}
        assert_send_sync::<dyn Marker>();
    }
}
