use super::Api;

/// Render a stable, human-diffable textual dump of the IR.
///
/// This is the frontend/backend contract seam: the `oas31` suite snapshots one dump per fixture,
/// pinning frontend behavior at the IR boundary independent of codegen (PRD §7.5). The format is
/// deterministic — same IR ⇒ byte-identical dump.
pub fn dump(api: &Api) -> String {
    todo!()
}
