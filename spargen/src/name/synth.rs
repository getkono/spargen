use crate::ir::{Method, PathTemplate};

/// Synthesize an `operationId` from an operation's method and path template when the spec omits one
/// (PRD D9). Deterministic, e.g. `GET /users/{id}` → `get_users_by_id`.
pub fn synth_operation_id(method: Method, path: &PathTemplate) -> String {
    todo!()
}
