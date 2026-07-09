use super::Api;

/// Render a stable, human-diffable textual dump of the IR.
///
/// This is the frontend/backend contract seam: the `oas31` suite snapshots one dump per fixture,
/// pinning frontend behavior at the IR boundary independent of codegen (PRD §7.5). The format is
/// deterministic — same IR ⇒ byte-identical dump.
pub fn dump(api: &Api) -> String {
    let mut out = String::new();
    use std::fmt::Write as _;

    let _ = writeln!(out, "api {} {}", api.info.title, api.info.version);
    let _ = writeln!(out, "servers {}", api.servers.len());
    for server in &api.servers {
        let _ = writeln!(out, "  server {}", server.url);
    }
    let _ = writeln!(out, "types {}", api.types.len());
    for (id, def) in api.types.iter() {
        let _ = writeln!(out, "  type {} {} {:?}", id.0, def.name_hint, def.kind);
    }
    let _ = writeln!(out, "operations {}", api.operations.len());
    for operation in &api.operations {
        let _ = writeln!(
            out,
            "  op {:?} {} {}",
            operation.method, operation.path.raw, operation.id.0
        );
    }
    out
}
