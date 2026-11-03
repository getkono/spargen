//! Resolving a library [`Spec`] from CLI flags and a `spargen.toml`.
//!
//! Precedence, low → high: built-in defaults < `spargen.toml` < CLI flags. The config file itself
//! is parsed by the library ([`Spec::config_file`]), so `build.rs`, the `generate_api!` macro, and
//! this CLI all read the same schema. What lives here is only the flag *syntax* — the compact
//! `"METHOD /path"`, `"kind:name"`, and `"[file#]/pointer"` spellings that exist because a shell
//! has no nested tables.

use std::borrow::Cow;

use camino::Utf8PathBuf;

use spargen::{ComponentKind, ConfigError, OmitMethod, OmitRule, Spec, UnknownOmitToken};

use super::args::SpecArgs;

/// Resolve the effective [`Spec`] for one spec path.
///
/// The config file is either the explicit `--config` path or, when that is absent, `spargen.toml`
/// beside the spec (a missing auto-discovered file is fine). Flags are applied last so they win,
/// each switch pair in either direction and only when one of its flags was given, and omit flags are unioned with the file's `[[omit]]` rules rather than replacing them.
pub(crate) fn resolve(path: Utf8PathBuf, args: &SpecArgs) -> Result<Spec, ConfigError> {
    let mut spec = match &args.config {
        Some(config) => Spec::new(path).config_file(config)?,
        None => Spec::new(path).discover_config_file()?,
    };
    if let Some(carve) = switch(args.carve, args.no_carve) {
        spec = spec.carve(carve);
    }
    if let Some(uuid) = switch(args.uuid, args.no_uuid) {
        spec = spec.uuid(uuid);
    }
    if let Some(time) = switch(args.time, args.no_time) {
        spec = spec.time(time);
    }
    if let Some(cap) = args.error_body_cap {
        spec = spec.error_body_cap(cap);
    }
    if let Some(cap) = args.batch_cap {
        spec = spec.batch_cap(cap);
    }
    if let Some(open) = switch(args.open_narrowing, args.no_open_narrowing) {
        spec = spec.open_narrowing(open);
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

/// The value a `--X` / `--no-X` flag pair sets, or `None` when neither was given, so the config
/// file's value stands. Clap rejects the two together, so `on` and `off` are never both set.
fn switch(on: bool, off: bool) -> Option<bool> {
    match (on, off) {
        (true, _) => Some(true),
        (false, true) => Some(false),
        (false, false) => None,
    }
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

/// Parse an omit HTTP method through the shared `FromStr`, whose error already names every
/// accepted method.
fn parse_method(method: &str) -> Result<OmitMethod, ConfigError> {
    method
        .parse()
        .map_err(|parse_error: UnknownOmitToken| error(parse_error.to_string()))
}

/// Parse an omit component kind. Snake_case is canonical (it is what `omit!` writes), and the
/// singular and camelCase OAS spellings are accepted so one rule reads the same everywhere.
fn parse_component_kind(kind: &str) -> Result<ComponentKind, ConfigError> {
    kind.parse()
        .map_err(|parse_error: UnknownOmitToken| error(parse_error.to_string()))
}

fn error(message: String) -> ConfigError {
    ConfigError::new(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    use clap::Parser;

    use super::super::args::{Cli, Command};

    /// The [`Spec`] `spargen check` resolves for `openapi.yaml` in `dir` under `flags`.
    fn resolved(dir: &camino::Utf8Path, flags: &[&str]) -> Spec {
        let spec = dir.join("openapi.yaml");
        let cli = Cli::try_parse_from(
            ["spargen", "check", spec.as_str()]
                .into_iter()
                .chain(flags.iter().copied()),
        )
        .unwrap_or_else(|error| panic!("{flags:?} did not parse: {error}"));
        let Command::Check(args) = cli.command else {
            unreachable!("parsed as `check`")
        };
        resolve(spec, &args.options).unwrap()
    }

    /// A temporary directory holding `spargen.toml` with `config`, as a UTF-8 path.
    fn with_config(config: &str) -> (tempfile::TempDir, Utf8PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
        std::fs::write(dir.join("spargen.toml"), config).unwrap();
        (temp, dir)
    }

    /// Each switch pair overrides the file in both directions, and leaves the file's value alone
    /// when neither of its flags is given.
    #[test]
    fn each_switch_pair_overrides_the_config_file_either_way() {
        let (_on, on) =
            with_config("carve = true\nuuid = true\ntime = true\nopen_narrowing = true\n");
        let (_off, off) =
            with_config("carve = false\nuuid = false\ntime = false\nopen_narrowing = false\n");
        let file_on = Spec::new(on.join("openapi.yaml"))
            .config_file(on.join("spargen.toml"))
            .unwrap();
        let file_off = Spec::new(off.join("openapi.yaml"))
            .config_file(off.join("spargen.toml"))
            .unwrap();

        assert_eq!(resolved(&on, &[]), file_on);
        assert_eq!(resolved(&off, &[]), file_off);
        assert_eq!(
            resolved(
                &on,
                &[
                    "--no-carve",
                    "--no-uuid",
                    "--no-time",
                    "--no-open-narrowing"
                ]
            ),
            file_on
                .clone()
                .carve(false)
                .uuid(false)
                .time(false)
                .open_narrowing(false)
        );
        assert_eq!(
            resolved(&off, &["--carve", "--uuid", "--time", "--open-narrowing"]),
            file_off
                .clone()
                .carve(true)
                .uuid(true)
                .time(true)
                .open_narrowing(true)
        );
        // One flag of a pair moves only its own setting.
        assert_eq!(resolved(&off, &["--uuid"]), file_off.uuid(true));
    }

    #[test]
    fn both_flags_of_a_switch_pair_are_a_usage_error() {
        for (on, off) in [
            ("--carve", "--no-carve"),
            ("--uuid", "--no-uuid"),
            ("--time", "--no-time"),
            ("--open-narrowing", "--no-open-narrowing"),
        ] {
            let error = Cli::try_parse_from(["spargen", "check", "openapi.yaml", on, off])
                .expect_err(&format!("`{on} {off}` must not parse"));
            assert_eq!(
                error.kind(),
                clap::error::ErrorKind::ArgumentConflict,
                "{on} {off}: {error}"
            );
        }
    }

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
        // An empty `file` before the `#` is the root document, the same rule as a bare pointer,
        // and not a file named the empty string.
        assert_eq!(
            parse_pointer_flag("#/paths/~1legacy"),
            OmitRule::pointer(None, "/paths/~1legacy")
        );
    }

    /// The message a rejected flag value reports, which is what the user reads on stderr.
    fn message(result: Result<OmitRule, ConfigError>) -> String {
        result
            .expect_err("the flag value must be rejected")
            .to_string()
    }

    #[test]
    fn bad_omit_flag_syntax_errors() {
        assert_eq!(
            message(parse_operation_flag("")),
            "--omit-operation ``: expected `METHOD /path` (e.g. `get /pets`)"
        );
        assert_eq!(
            message(parse_operation_flag("get")),
            "--omit-operation `get`: missing path; expected `METHOD /path` (e.g. `get /pets`)"
        );
        assert_eq!(
            message(parse_operation_flag("get /pets extra")),
            "--omit-operation `get /pets extra`: too many parts; expected `METHOD /path`"
        );
        assert_eq!(
            message(parse_operation_flag("frobnicate /x")),
            "unknown HTTP method: `frobnicate`; \
             expected one of get/put/post/delete/options/head/patch/trace/query"
        );
        assert_eq!(
            message(parse_component_flag("LegacyPet")),
            "--omit-component `LegacyPet`: expected `kind:name` (e.g. `schema:LegacyPet`)"
        );
        assert_eq!(
            message(parse_component_flag("schema: ")),
            "--omit-component `schema: `: empty component name"
        );
        assert_eq!(
            message(parse_component_flag("bogus:Name")),
            "unknown component kind: `bogus`; expected one of \
             schemas/responses/parameters/request_bodies/headers/security_schemes/path_items/media_types"
        );
    }
}
