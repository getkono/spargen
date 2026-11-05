//! Readers over a run's report and over the module it wrote, shared by the fixture suite
//! (`frontend/`), the fuzz harness (`fuzz_frontend.rs`), the lowering properties
//! (`lowering_props.rs`) and the framework recipes (`recipes.rs`). The first three reach it as
//! `oracles::readers`; `recipes.rs` declares this file alone, with `#[path]`.
//!
//! Each test crate that declares this module uses the part of it its fixtures need, so an item one
//! of them leaves unused carries `#[allow(dead_code)]` of its own.

use spargen::{Code, Report};

/// The sorted diagnostic codes a report carries, duplicates kept: a stage that fires the same
/// warning twice differs from one that fires it once.
#[allow(dead_code)]
pub fn codes(report: &Report) -> Vec<&'static str> {
    let mut codes: Vec<&'static str> = report
        .diagnostics()
        .iter()
        .map(|diagnostic| diagnostic.code.as_str())
        .collect();
    codes.sort_unstable();
    codes
}

/// Whether `report` carries a `code` diagnostic.
#[allow(dead_code)]
pub fn has_code(report: &Report, code: Code) -> bool {
    report.diagnostics().iter().any(|d| d.code == code)
}

/// Every message a report carries for one code, in report order. A code being right is not the
/// same as its message being true — a diagnostic that fires on the correct construct while
/// asserting something false about it is still a defect — so the fixtures that pin wording assert
/// on this, not on the code.
#[allow(dead_code)]
pub fn messages_for(report: &Report, code: Code) -> Vec<&str> {
    report
        .diagnostics()
        .iter()
        .filter(|d| d.code == code)
        .map(|d| d.message.as_str())
        .collect()
}

/// The name of the `pub struct` that declares the first field line starting with `field`.
///
/// A type count plus "both fields exist somewhere" is satisfied by either assignment of two names to
/// two schemas, so it cannot see a swap. This answers the question the count cannot: which generated
/// type a given field belongs to.
#[allow(dead_code)]
pub fn field_owner(code: &str, field: &str) -> Option<String> {
    let mut current: Option<String> = None;
    for line in code.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("pub struct ") {
            current = rest
                .split([' ', '<', '{', '(', ';'])
                .next()
                .filter(|name| !name.is_empty())
                .map(str::to_owned);
        }
        if trimmed.starts_with(field) {
            return current;
        }
    }
    None
}

/// The declared type of the first field line starting with `field`, trimmed of its trailing comma.
///
/// [`field_owner`] says which type *declares* a field; this says which type the field *is*. Asking
/// only whether a type name appears somewhere in the module cannot tell two same-named schemas
/// apart, because both are emitted — so a fixture that must pin *which* of them a reference bound
/// has to read the field's own right-hand side.
#[allow(dead_code)]
pub fn field_type(code: &str, field: &str) -> Option<String> {
    code.lines()
        .map(str::trim_start)
        .find(|line| line.starts_with(field))
        .and_then(|line| line.split_once(':'))
        .map(|(_, ty)| ty.trim().trim_end_matches(',').to_owned())
}

/// The field names `pub struct ty` declares, in source order.
///
/// The inverse of [`field_owner`], and the answer to "which schema is this type", which a name
/// alone cannot give when two declarations share a name and the emitter disambiguates one of them —
/// or when the alias path re-emits a target's *kind* under a third name.
#[allow(dead_code)]
pub fn declared_fields(code: &str, ty: &str) -> Vec<String> {
    let mut lines = code
        .lines()
        .map(str::trim_start)
        .skip_while(|line| !line.starts_with(&format!("pub struct {ty} ")));
    lines.next();
    lines
        .take_while(|line| !line.starts_with('}'))
        .filter_map(|line| line.strip_prefix("pub "))
        .filter_map(|rest| rest.split_once(':'))
        .map(|(name, _)| name.to_owned())
        .collect()
}

/// The right-hand side of `pub type ty = …;`, the type an alias names, or `None` when no such alias
/// is declared.
#[allow(dead_code)]
pub fn alias_target(code: &str, ty: &str) -> Option<String> {
    let head = format!("pub type {ty} = ");
    code.lines()
        .map(str::trim_start)
        .find_map(|line| line.strip_prefix(&head))
        .map(|rest| rest.trim_end().trim_end_matches(';').to_owned())
}

/// The variant declarations `pub enum ty` carries, in source order, each trimmed of its trailing
/// comma, attributes and doc lines skipped.
///
/// The union counterpart of [`declared_fields`], and the only way to see a union *member* go
/// missing. Binding the enum's name says the union was represented; it does not say how many
/// branches survived, and a collapse that erases one member leaves a type with the right name and
/// the wrong contents.
#[allow(dead_code)]
pub fn enum_variants(code: &str, ty: &str) -> Vec<String> {
    let mut lines = code
        .lines()
        .map(str::trim_start)
        .skip_while(|line| !line.starts_with(&format!("pub enum {ty} ")));
    lines.next();
    lines
        .take_while(|line| !line.starts_with('}'))
        .filter(|line| !line.is_empty() && !line.starts_with("#[") && !line.starts_with("///"))
        .map(|line| line.trim_end_matches(',').to_owned())
        .collect()
}

/// The names of `pub struct`s in generated source that begin with `prefix` and whose remainder
/// satisfies `suffix_ok`, in source order.
///
/// Counting `code.matches("pub struct Foo")` is the obvious thing and is wrong twice over: the
/// generated module embeds the runtime, whose own items can share a prefix (`pub struct L` also
/// matches `LinkPaginator`), and a count alone cannot say *which* types were emitted when it
/// disagrees. Returning the names makes a failure legible and makes an off-by-a-constant bound
/// impossible to mistake for a bound.
#[allow(dead_code)]
pub fn declared_types(code: &str, prefix: &str, suffix_ok: impl Fn(&str) -> bool) -> Vec<String> {
    code.lines()
        .filter_map(|line| line.trim_start().strip_prefix("pub struct "))
        .filter_map(|rest| rest.split([' ', '<', '{', '(', ';']).next())
        .filter(|name| !name.is_empty())
        .filter_map(|name| name.strip_prefix(prefix).map(|tail| (name, tail)))
        .filter(|(_, tail)| suffix_ok(tail))
        .map(|(name, _)| name.to_owned())
        .collect()
}
