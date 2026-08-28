//! Resolving a library [`Spec`] from CLI flags and a `spargen.toml`.
//!
//! Precedence, low → high: built-in defaults < `spargen.toml` < CLI flags. The config file itself
//! is parsed by the library ([`Spec::config_file`]), so `build.rs`, the `generate_api!` macro, and
//! this CLI all read the same schema. What lives here is only the flag *syntax* — the compact
//! `"METHOD /path"`, `"kind:name"`, and `"[file#]/pointer"` spellings that exist because a shell
//! has no nested tables.

use std::borrow::Cow;

use camino::Utf8PathBuf;

use spargen::{ComponentKind, ConfigError, OmitMethod, OmitRule, Spec};

use super::args::SpecArgs;

/// Resolve the effective [`Spec`] for one spec path.
///
/// The config file is either the explicit `--config` path or, when that is absent, `spargen.toml`
/// beside the spec (a missing auto-discovered file is fine). Flags are applied last so they win,
/// and omit flags are unioned with the file's `[[omit]]` rules rather than replacing them.
pub(crate) fn resolve(path: Utf8PathBuf, args: &SpecArgs) -> Result<Spec, ConfigError> {
    let mut spec = match &args.config {
        Some(config) => Spec::new(path).config_file(config)?,
        None => Spec::new(path).discover_config_file()?,
    };
    if args.carve {
        spec = spec.carve(true);
    }
    if args.no_uuid {
        spec = spec.uuid(false);
    }
    if args.no_time {
        spec = spec.time(false);
    }
    if let Some(cap) = args.error_body_cap {
        spec = spec.error_body_cap(cap);
    }
    if let Some(cap) = args.batch_cap {
        spec = spec.batch_cap(cap);
    }
    for path in &args.omit_path {
        spec = spec.omit_rule(OmitRule::path(path.clone()));
    }
    for value in &args.omit_operation {
        spec = spec.omit_rule(parse_operation_flag(value)?);
    }
    for value in &args.omit_component {
        spec = spec.omit_rule(parse_component_flag(value)?);
    }
    for value in &args.omit_pointer {
        spec = spec.omit_rule(parse_pointer_flag(value));
    }
    Ok(spec)
}

/// Parse a `--omit-operation "METHOD /path"` value into an [`OmitRule::Operation`].
fn parse_operation_flag(value: &str) -> Result<OmitRule, ConfigError> {
    let mut parts = value.split_whitespace();
    let method = parts.next().ok_or_else(|| {
        error(format!(
            "--omit-operation `{value}`: expected `METHOD /path` (e.g. `get /pets`)"
        ))
    })?;
    let path = parts.next().ok_or_else(|| {
        error(format!(
            "--omit-operation `{value}`: missing path; expected `METHOD /path` (e.g. `get /pets`)"
        ))
    })?;
    if parts.next().is_some() {
        return Err(error(format!(
            "--omit-operation `{value}`: too many parts; expected `METHOD /path`"
        )));
    }
    Ok(OmitRule::operation(parse_method(method)?, path.to_owned()))
}

/// Parse a `--omit-component "kind:name"` value into an [`OmitRule::Component`].
fn parse_component_flag(value: &str) -> Result<OmitRule, ConfigError> {
    let (kind, name) = value.split_once(':').ok_or_else(|| {
        error(format!(
            "--omit-component `{value}`: expected `kind:name` (e.g. `schema:LegacyPet`)"
        ))
    })?;
    let name = name.trim();
    if name.is_empty() {
        return Err(error(format!(
            "--omit-component `{value}`: empty component name"
        )));
    }
    Ok(OmitRule::component(
        parse_component_kind(kind.trim())?,
        name.to_owned(),
    ))
}

/// Parse a `--omit-pointer "[file#]/pointer"` value into an [`OmitRule::Pointer`]. A leading
/// `file#` selects a file-local pointer; without it the pointer targets the root document.
fn parse_pointer_flag(value: &str) -> OmitRule {
    match value.split_once('#') {
        Some((file, pointer)) if !file.is_empty() => {
            OmitRule::pointer(Some(Cow::Owned(file.to_owned())), pointer.to_owned())
        }
        // `#/pointer` (empty file) or a bare `/pointer` both target the root document.
        Some((_, pointer)) => OmitRule::pointer(None, pointer.to_owned()),
        None => OmitRule::pointer(None, value.to_owned()),
    }
}

/// Parse an omit HTTP method, mapping the shared `FromStr` error into a CLI-shaped one.
fn parse_method(method: &str) -> Result<OmitMethod, ConfigError> {
    method.parse().map_err(|parse_error| {
        error(format!(
            "{parse_error}; expected one of get/put/post/delete/options/head/patch/trace/query"
        ))
    })
}

/// Parse an omit component kind. Snake_case is canonical (it is what `omit!` writes), and the
/// singular and camelCase OAS spellings are accepted so one rule reads the same everywhere.
fn parse_component_kind(kind: &str) -> Result<ComponentKind, ConfigError> {
    kind.parse().map_err(|parse_error| {
        error(format!(
            "{parse_error}; expected one of \
             schemas/responses/parameters/request_bodies/headers/security_schemes/path_items/media_types"
        ))
    })
}

fn error(message: String) -> ConfigError {
    ConfigError::new(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_each_omit_flag_kind() {
        assert_eq!(
            parse_operation_flag("get /pets").unwrap(),
            OmitRule::operation(OmitMethod::Get, "/pets")
        );
        assert_eq!(
            parse_component_flag("schema:LegacyPet").unwrap(),
            OmitRule::component(ComponentKind::Schemas, "LegacyPet")
        );
        assert_eq!(
            parse_pointer_flag("extra.yaml#/components/schemas/X"),
            OmitRule::pointer(Some("extra.yaml".into()), "/components/schemas/X")
        );
        assert_eq!(
            parse_pointer_flag("/paths/~1legacy"),
            OmitRule::pointer(None, "/paths/~1legacy")
        );
    }

    #[test]
    fn bad_omit_flag_syntax_errors() {
        assert!(parse_operation_flag("get").is_err());
        assert!(parse_operation_flag("frobnicate /x").is_err());
        assert!(parse_component_flag("LegacyPet").is_err());
        assert!(parse_component_flag("bogus:Name").is_err());
    }
}
