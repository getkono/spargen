//! Resolving a library [`Spec`] from CLI flags and a `spargen.toml`.
//!
//! Precedence, low → high: built-in defaults < `spargen.toml` < CLI flags. The config file itself
//! is parsed by the library ([`Spec::config_file`]), so `build.rs`, the `generate_api!` macro, and
//! this CLI all read the same schema. What lives here is only the flag *syntax* — the compact
//! `"METHOD /path"`, `"kind:name"`, and `"[file#]/pointer"` spellings that exist because a shell
//! has no nested tables.

use std::borrow::Cow;

use camino::Utf8PathBuf;

use spargen::{
    CarriedDocs, Carve, ComponentKind, ConfigError, Narrowing, OmitMethod, OmitRule, Spec,
    TestingModule, UnionTags, UnknownOmitToken, UuidMapping,
};

use super::args::SpecArgs;

/// Resolve the effective [`Spec`] for one spec path.
///
/// The config file is either the explicit `--config` path or, when that is absent, `spargen.toml`
/// beside the spec (a missing auto-discovered file is fine). Flags are applied last so they win,
/// each switch pair in either direction and only when one of its flags was given, and omit, keep
/// and bound-header flags are unioned with the file's `[[omit]]`, `[[keep]]` and `bound_headers`
/// rather than replacing them.
pub(crate) fn resolve(path: Utf8PathBuf, args: &SpecArgs) -> Result<Spec, ConfigError> {
    let mut spec = match &args.config {
        Some(config) => Spec::new(path).config_file(config)?,
        None => Spec::new(path).discover_config_file()?,
    };
    if let Some(carve) = switch(args.carve, args.no_carve, Carve::Auto, Carve::Off) {
        spec = spec.with_carve(carve);
    }
    if let Some(uuid) = switch(
        args.uuid,
        args.no_uuid,
        UuidMapping::Uuid,
        UuidMapping::String,
    ) {
        spec = spec.with_uuid(uuid);
    }
    if let Some(backend) = args.datetime {
        spec = spec.with_datetime(backend);
    }
    if let Some(cap) = args.error_body_cap {
        spec = spec.with_error_body_cap(cap);
    }
    if let Some(cap) = args.batch_cap {
        spec = spec.with_batch_cap(cap);
    }
    if let Some(narrowing) = switch(
        args.open_narrowing,
        args.no_open_narrowing,
        Narrowing::Open,
        Narrowing::Exact,
    ) {
        spec = spec.with_open_narrowing(narrowing);
    }
    if let Some(tags) = switch(
        args.open_unions,
        args.no_open_unions,
        UnionTags::Open,
        UnionTags::Closed,
    ) {
        spec = spec.with_open_unions(tags);
    }
    if let Some(docs) = switch(
        args.verbatim_docs,
        args.no_verbatim_docs,
        CarriedDocs::Verbatim,
        CarriedDocs::Rewritten,
    ) {
        spec = spec.with_verbatim_docs(docs);
    }
    if let Some(module) = switch(
        args.testing,
        args.no_testing,
        TestingModule::Emitted,
        TestingModule::Omitted,
    ) {
        spec = spec.with_testing(module);
    }
    for path in &args.omit_path {
        spec = spec.add_omit_rule(OmitRule::path(path.clone()));
    }
    for value in &args.omit_operation {
        spec = spec.add_omit_rule(parse_operation_flag(value)?);
    }
    for value in &args.omit_component {
        spec = spec.add_omit_rule(parse_component_flag(value)?);
    }
    for value in &args.omit_pointer {
        spec = spec.add_omit_rule(parse_pointer_flag(value));
    }
    for value in &args.keep_operation {
        let (method, path) = parse_method_and_path("--keep-operation", value)?;
        spec = spec.add_keep_operation(method, path);
    }
    for name in &args.bound_header {
        spec = spec.add_bound_header(name.clone());
    }
    Ok(spec)
}

/// The value a `--X` / `--no-X` flag pair sets — `on_value` for `--X`, `off_value` for `--no-X` —
/// or `None` when neither was given, so the config file's value stands. Clap rejects the two
/// together, so `on` and `off` are never both set.
fn switch<T>(on: bool, off: bool, on_value: T, off_value: T) -> Option<T> {
    match (on, off) {
        (true, _) => Some(on_value),
        (false, true) => Some(off_value),
        (false, false) => None,
    }
}

/// Parse a `--omit-operation "METHOD /path"` value into an [`OmitRule::Operation`].
fn parse_operation_flag(value: &str) -> Result<OmitRule, ConfigError> {
    let (method, path) = parse_method_and_path("--omit-operation", value)?;
    Ok(OmitRule::operation(method, path))
}

/// Split a `flag`'s `"METHOD /path"` value into its method and path.
fn parse_method_and_path(flag: &str, value: &str) -> Result<(OmitMethod, String), ConfigError> {
    let mut parts = value.split_whitespace();
    let method = parts.next().ok_or_else(|| {
        error(format!(
            "{flag} `{value}`: expected `METHOD /path` (e.g. `get /pets`)"
        ))
    })?;
    let path = parts.next().ok_or_else(|| {
        error(format!(
            "{flag} `{value}`: missing path; expected `METHOD /path` (e.g. `get /pets`)"
        ))
    })?;
    if parts.next().is_some() {
        return Err(error(format!(
            "{flag} `{value}`: too many parts; expected `METHOD /path`"
        )));
    }
    Ok((parse_method(method)?, path.to_owned()))
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
    ConfigError::flag(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    use clap::Parser;

    use super::super::args::{Cli, Command};
    use spargen::DateTimeBackend;

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
        let (_on, on) = with_config(
            "carve = true\nuuid = true\nopen_narrowing = true\nopen_unions = true\n\
             verbatim_docs = true\ntesting = true\n",
        );
        let (_off, off) = with_config(
            "carve = false\nuuid = false\nopen_narrowing = false\nopen_unions = false\n\
             verbatim_docs = false\ntesting = false\n",
        );
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
                    "--no-open-narrowing",
                    "--no-open-unions",
                    "--no-verbatim-docs",
                    "--no-testing"
                ]
            ),
            file_on
                .clone()
                .with_carve(spargen::Carve::Off)
                .with_uuid(spargen::UuidMapping::String)
                .with_open_narrowing(spargen::Narrowing::Exact)
                .with_open_unions(spargen::UnionTags::Closed)
                .with_verbatim_docs(spargen::CarriedDocs::Rewritten)
                .with_testing(TestingModule::Omitted)
        );
        assert_eq!(
            resolved(
                &off,
                &[
                    "--carve",
                    "--uuid",
                    "--open-narrowing",
                    "--open-unions",
                    "--verbatim-docs",
                    "--testing"
                ]
            ),
            file_off
                .clone()
                .with_carve(spargen::Carve::Auto)
                .with_uuid(spargen::UuidMapping::Uuid)
                .with_open_narrowing(spargen::Narrowing::Open)
                .with_open_unions(spargen::UnionTags::Open)
                .with_verbatim_docs(spargen::CarriedDocs::Verbatim)
                .with_testing(TestingModule::Emitted)
        );
        // One flag of a pair moves only its own setting.
        assert_eq!(
            resolved(&off, &["--uuid"]),
            file_off.with_uuid(spargen::UuidMapping::Uuid)
        );
    }

    /// Each of the twelve switch flags, alone, moves its own setting off the file's value and
    /// leaves the other five where the file put them.
    #[test]
    fn each_switch_flag_moves_only_its_own_setting() {
        let (_on, on) = with_config(
            "carve = true\nuuid = true\nopen_narrowing = true\nopen_unions = true\n\
             verbatim_docs = true\ntesting = true\n",
        );
        let (_off, off) = with_config(
            "carve = false\nuuid = false\nopen_narrowing = false\nopen_unions = false\n\
             verbatim_docs = false\ntesting = false\n",
        );
        let file = |dir: &Utf8PathBuf| {
            Spec::new(dir.join("openapi.yaml"))
                .config_file(dir.join("spargen.toml"))
                .unwrap()
        };
        type Moved = fn(Spec) -> Spec;
        let rows: [(&str, &Utf8PathBuf, Moved); 12] = [
            ("--no-carve", &on, |spec| {
                spec.with_carve(spargen::Carve::Off)
            }),
            ("--no-uuid", &on, |spec| {
                spec.with_uuid(spargen::UuidMapping::String)
            }),
            ("--no-open-narrowing", &on, |spec| {
                spec.with_open_narrowing(spargen::Narrowing::Exact)
            }),
            ("--no-open-unions", &on, |spec| {
                spec.with_open_unions(spargen::UnionTags::Closed)
            }),
            ("--no-verbatim-docs", &on, |spec| {
                spec.with_verbatim_docs(spargen::CarriedDocs::Rewritten)
            }),
            ("--no-testing", &on, |spec| {
                spec.with_testing(TestingModule::Omitted)
            }),
            ("--carve", &off, |spec| {
                spec.with_carve(spargen::Carve::Auto)
            }),
            ("--uuid", &off, |spec| {
                spec.with_uuid(spargen::UuidMapping::Uuid)
            }),
            ("--open-narrowing", &off, |spec| {
                spec.with_open_narrowing(spargen::Narrowing::Open)
            }),
            ("--open-unions", &off, |spec| {
                spec.with_open_unions(spargen::UnionTags::Open)
            }),
            ("--verbatim-docs", &off, |spec| {
                spec.with_verbatim_docs(spargen::CarriedDocs::Verbatim)
            }),
            ("--testing", &off, |spec| {
                spec.with_testing(TestingModule::Emitted)
            }),
        ];
        for (flag, dir, moved) in rows {
            let resolved = resolved(dir, &[flag]);
            assert_ne!(resolved, file(dir), "{flag} moves its setting");
            assert_eq!(resolved, moved(file(dir)), "{flag} moves nothing else");
        }
    }

    /// `--datetime` overrides the file's backend with any of the four, leaves it in place when not
    /// given, and refuses a spelling it does not know as a clap value error.
    #[test]
    fn the_datetime_flag_overrides_the_file_backend() {
        let (_dir, dir) = with_config("datetime = \"jiff\"\n");
        let file = Spec::new(dir.join("openapi.yaml"))
            .config_file(dir.join("spargen.toml"))
            .unwrap();
        assert_eq!(file.datetime(), DateTimeBackend::Jiff);
        assert_eq!(resolved(&dir, &[]), file);
        for &backend in DateTimeBackend::ALL {
            assert_eq!(
                resolved(&dir, &["--datetime", backend.as_str()]),
                file.clone().with_datetime(backend),
                "{backend}"
            );
        }
        let error = Cli::try_parse_from(["spargen", "check", "openapi.yaml", "--datetime", "Jiff"])
            .expect_err("an unknown backend must not parse");
        assert_eq!(
            error.kind(),
            clap::error::ErrorKind::ValueValidation,
            "{error}"
        );
        assert!(
            error
                .to_string()
                .contains("expected time, jiff, chrono or string"),
            "{error}"
        );
    }

    /// `lock` names every spec flag but `--config` and `--batch-cap` as ignored, each once
    /// however often it is repeated, and names none when only those two are given.
    #[test]
    fn lock_names_exactly_the_spec_flags_it_ignores() {
        let ignored = |flags: &[&str]| {
            let cli = Cli::try_parse_from(
                ["spargen", "lock", "openapi.yaml"]
                    .into_iter()
                    .chain(flags.iter().copied()),
            )
            .unwrap_or_else(|error| panic!("{flags:?} did not parse: {error}"));
            let Command::Lock(args) = cli.command else {
                unreachable!("parsed as `lock`")
            };
            args.options.flags_lock_ignores()
        };
        assert!(ignored(&[]).is_empty());
        assert!(ignored(&["--config", "spargen.toml", "--batch-cap", "5"]).is_empty());
        for flag in [
            "--carve",
            "--no-carve",
            "--uuid",
            "--no-uuid",
            "--open-narrowing",
            "--no-open-narrowing",
            "--open-unions",
            "--no-open-unions",
            "--verbatim-docs",
            "--no-verbatim-docs",
            "--testing",
            "--no-testing",
        ] {
            assert_eq!(ignored(&[flag]), [flag]);
        }
        for (flag, value) in [
            ("--datetime", "jiff"),
            ("--error-body-cap", "10"),
            ("--omit-path", "/pets"),
            ("--omit-operation", "get /pets"),
            ("--omit-component", "schema:Pet"),
            ("--omit-pointer", "/paths"),
            ("--keep-operation", "get /pets"),
            ("--bound-header", "X-Api-Version"),
        ] {
            assert_eq!(ignored(&[flag, value]), [flag]);
        }
        assert_eq!(
            ignored(&[
                "--omit-path",
                "/a",
                "--uuid",
                "--batch-cap",
                "5",
                "--omit-path",
                "/b",
                "--carve",
            ]),
            ["--carve", "--uuid", "--omit-path"],
            "declaration order, each flag once"
        );
    }

    #[test]
    fn both_flags_of_a_switch_pair_are_a_usage_error() {
        for (on, off) in [
            ("--carve", "--no-carve"),
            ("--uuid", "--no-uuid"),
            ("--open-narrowing", "--no-open-narrowing"),
            ("--open-unions", "--no-open-unions"),
            ("--verbatim-docs", "--no-verbatim-docs"),
            ("--testing", "--no-testing"),
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

    /// `--keep-operation` reads `METHOD /path` as `--omit-operation` does, naming itself in each
    /// syntax error, and its rules follow the config file's `[[keep]]` entries.
    #[test]
    fn the_keep_flag_parses_and_appends_after_the_config_file() {
        assert_eq!(
            parse_method_and_path("--keep-operation", "POST /pets").unwrap(),
            (OmitMethod::Post, "/pets".to_owned())
        );
        for (value, expected) in [
            (
                "get",
                "--keep-operation `get`: missing path; expected `METHOD /path` (e.g. `get /pets`)",
            ),
            (
                "get /a /b",
                "--keep-operation `get /a /b`: too many parts; expected `METHOD /path`",
            ),
        ] {
            assert_eq!(
                parse_method_and_path("--keep-operation", value)
                    .unwrap_err()
                    .to_string(),
                expected
            );
        }

        let (_temp, dir) = with_config("[[keep]]\nmethod = \"get\"\npath = \"/from-file\"\n");
        assert_eq!(
            resolved(&dir, &["--keep-operation", "delete /from-flag"]),
            Spec::new(dir.join("openapi.yaml"))
                .config_file(dir.join("spargen.toml"))
                .unwrap()
                .add_keep_operation(OmitMethod::Delete, "/from-flag")
        );
        assert_eq!(
            resolved(&dir, &["--keep-operation", "delete /from-flag"])
                .omissions()
                .keep
                .len(),
            2,
            "the file's rule and the flag's"
        );
    }

    /// `--bound-header` is repeatable, and its names follow the config file's `bound_headers`, a name
    /// the file already binds in another letter case not bound twice.
    #[test]
    fn the_bind_header_flag_appends_after_the_config_file() {
        let (_temp, dir) = with_config("bound_headers = [\"X-Tenant\"]\n");
        let spec = resolved(
            &dir,
            &[
                "--bound-header",
                "X-Api-Version",
                "--bound-header",
                "x-tenant",
            ],
        );
        assert_eq!(spec.bound_headers(), ["X-Tenant", "X-Api-Version"]);
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
