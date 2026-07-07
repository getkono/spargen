use crate::diag::Diagnostics;

use super::Api;

/// Check the IR's well-formedness invariants, reporting any violation through `diags`.
///
/// Run after every lowering in tests and debug builds (PRD §7.5): every [`Ty`](super::Ty)'s
/// `TypeId` resolves in the [`TypeGraph`](super::TypeGraph), discriminator properties exist on
/// their variants, path parameters are declared, and there are no dangling references. A failure
/// here is a frontend bug, not a spec problem.
pub fn check_invariants(api: &Api, diags: &mut Diagnostics) {
    todo!()
}
