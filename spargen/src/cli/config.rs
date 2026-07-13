//! `spargen.toml` config-file loading and the CLI omit-profile surface.
//!
//! This is CLI-level plumbing only: it resolves a set of [`Settings`] from three layers —
//! built-in defaults, an optional `spargen.toml`, and CLI flags — with **CLI flags overriding
//! config-file values, which override defaults**. The resolved settings are then folded into the
//! library [`Config`](crate::Config) by [`run`](super::run), whose public API is unchanged.
//!
//! The `spargen.toml` schema mirrors [`Config`]:
//!
//! ```toml
//! [features]
//! uuid = true            # default true; false ⇒ same as `--no-uuid`
//! time = true            # default true; false ⇒ same as `--no-time`
//! error_body_cap = 65536 # optional; bytes of an error body retained (default 65536)
//! batch_cap = 100        # optional; max diagnostics collected (default 100)
//! as_crate = false       # optional; generate a standalone crate instead of a module
//!
//! # Zero or more omit rules. The rule KIND is discriminated by field presence:
//! [[omit]]
//! path = "/pets/{id}"                    # → OmitRule::Path
//! [[omit]]
//! method = "get"                         # `method` + `path` → OmitRule::Operation
//! path = "/pets"
//! [[omit]]
//! component = "schema"                   # `component` + `name` → OmitRule::Component
//! name = "LegacyPet"                     #   component ∈ schema/response/parameter/requestBody/header/securityScheme
//! [[omit]]
//! pointer = "/components/schemas/X"      # `pointer` → OmitRule::Pointer
//! file = "extra.yaml"                    #   `file` optional (file-local pointer)
//! ```

use camino::Utf8Path;

use crate::{ComponentKind, Omit, OmitMethod, OmitRule};

/// A clear, user-facing config/flag error. Rendered to stderr by [`run`](super::run), which then
/// exits with a usage status — never a panic.
#[derive(Debug)]
pub struct ConfigError {
    message: String,
}

impl ConfigError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// The omit-rule CLI flags, collected repeatably. Each vector holds the raw flag values in the
/// order given; they are parsed and unioned with the config-file omit rules.
#[derive(Debug, Default)]
pub struct OmitFlags {
    /// `--omit-path <PATH>`
    pub paths: Vec<String>,
    /// `--omit-operation <"METHOD /path">`
    pub operations: Vec<String>,
    /// `--omit-component <"kind:name">`
    pub components: Vec<String>,
    /// `--omit-pointer <"[file#]/pointer">`
    pub pointers: Vec<String>,
}

/// Feature/crate/cap overrides expressed by CLI flags (as opposed to the config file). A `Some`
/// value means the flag was explicitly given and takes precedence over the config file.
#[derive(Debug, Default)]
pub struct CliOverrides {
    /// `--no-uuid` present ⇒ `Some(false)`.
    pub uuid: Option<bool>,
    /// `--no-time` present ⇒ `Some(false)`.
    pub time: Option<bool>,
    /// `--as-crate` present ⇒ `Some(true)`.
    pub as_crate: Option<bool>,
}

/// The fully resolved settings after merging defaults, config file, and CLI flags.
#[derive(Debug)]
pub struct Settings {
    /// `format: uuid` mapping (default on).
    pub uuid: bool,
    /// `format: date-time`/`date` mapping (default on).
    pub time: bool,
    /// Emit a standalone crate rather than a module.
    pub as_crate: bool,
    /// Max bytes of an error response body retained.
    pub error_body_cap: usize,
    /// Max diagnostics collected before batching stops.
    pub batch_cap: usize,
    /// The union of config-file and CLI omit rules.
    pub omit: Omit,
}

/// Resolve the effective settings for a run.
///
/// Precedence (low → high): built-in defaults < `spargen.toml` < CLI flags. The config file is
/// either the explicit `config_path` or, when that is `None`, auto-discovered as `spargen.toml`
/// beside the spec. A missing auto-discovered file is fine (defaults are used); an explicit
/// `--config` path that does not exist, a malformed file, or bad omit-flag syntax is a clear
/// [`ConfigError`].
pub fn resolve(
    spec: &Utf8Path,
    config_path: Option<&Utf8Path>,
    overrides: &CliOverrides,
    flags: &OmitFlags,
) -> Result<Settings, ConfigError> {
    let file = load_file(spec, config_path)?;

    // Defaults, then config-file overrides.
    let mut uuid = true;
    let mut time = true;
    let mut as_crate = false;
    let mut error_body_cap = 64 * 1024;
    let mut batch_cap = 100;
    let mut rules = Vec::new();

    if let Some(file) = &file {
        if let Some(features) = &file.features {
            if let Some(value) = features.uuid {
                uuid = value;
            }
            if let Some(value) = features.time {
                time = value;
            }
            if let Some(value) = features.as_crate {
                as_crate = value;
            }
            if let Some(value) = features.error_body_cap {
                error_body_cap = value;
            }
            if let Some(value) = features.batch_cap {
                batch_cap = value;
            }
        }
        for (index, entry) in file.omit.iter().enumerate() {
            rules.push(entry.to_rule().map_err(|message| {
                ConfigError::new(format!("spargen.toml: omit rule #{}: {message}", index + 1))
            })?);
        }
    }

    // CLI flags win over the config file.
    if let Some(value) = overrides.uuid {
        uuid = value;
    }
    if let Some(value) = overrides.time {
        time = value;
    }
    if let Some(value) = overrides.as_crate {
        as_crate = value;
    }

    // CLI omit flags are unioned with the config-file rules.
    for path in &flags.paths {
        rules.push(OmitRule::Path {
            path: leak(path.clone()),
        });
    }
    for spec in &flags.operations {
        rules.push(parse_operation_flag(spec)?);
    }
    for spec in &flags.components {
        rules.push(parse_component_flag(spec)?);
    }
    for spec in &flags.pointers {
        rules.push(parse_pointer_flag(spec));
    }

    Ok(Settings {
        uuid,
        time,
        as_crate,
        error_body_cap,
        batch_cap,
        omit: Omit { rules },
    })
}

/// Load and parse the config file: explicit `--config` (must exist), else auto-discover
/// `spargen.toml` beside the spec (absence is fine).
fn load_file(
    spec: &Utf8Path,
    config_path: Option<&Utf8Path>,
) -> Result<Option<FileConfig>, ConfigError> {
    let path = match config_path {
        Some(path) => path.to_owned(),
        None => {
            let discovered = spec
                .parent()
                .unwrap_or_else(|| Utf8Path::new(""))
                .join("spargen.toml");
            if !discovered.as_std_path().is_file() {
                return Ok(None);
            }
            discovered
        }
    };

    let text = std::fs::read_to_string(path.as_std_path())
        .map_err(|error| ConfigError::new(format!("cannot read config file `{path}`: {error}")))?;
    let parsed = toml::from_str::<FileConfig>(&text)
        .map_err(|error| ConfigError::new(format!("invalid config file `{path}`: {error}")))?;
    Ok(Some(parsed))
}

/// Parse a `--omit-operation "METHOD /path"` value into an [`OmitRule::Operation`].
fn parse_operation_flag(value: &str) -> Result<OmitRule, ConfigError> {
    let mut parts = value.split_whitespace();
    let method = parts.next().ok_or_else(|| {
        ConfigError::new(format!(
            "--omit-operation `{value}`: expected `METHOD /path` (e.g. `get /pets`)"
        ))
    })?;
    let path = parts.next().ok_or_else(|| {
        ConfigError::new(format!(
            "--omit-operation `{value}`: missing path; expected `METHOD /path` (e.g. `get /pets`)"
        ))
    })?;
    if parts.next().is_some() {
        return Err(ConfigError::new(format!(
            "--omit-operation `{value}`: too many parts; expected `METHOD /path`"
        )));
    }
    Ok(OmitRule::Operation {
        method: parse_method(method)?,
        path: leak(path.to_owned()),
    })
}

/// Parse a `--omit-component "kind:name"` value into an [`OmitRule::Component`].
fn parse_component_flag(value: &str) -> Result<OmitRule, ConfigError> {
    let (kind, name) = value.split_once(':').ok_or_else(|| {
        ConfigError::new(format!(
            "--omit-component `{value}`: expected `kind:name` (e.g. `schema:LegacyPet`)"
        ))
    })?;
    let name = name.trim();
    if name.is_empty() {
        return Err(ConfigError::new(format!(
            "--omit-component `{value}`: empty component name"
        )));
    }
    Ok(OmitRule::Component {
        kind: parse_component_kind(kind.trim())?,
        name: leak(name.to_owned()),
    })
}

/// Parse a `--omit-pointer "[file#]/pointer"` value into an [`OmitRule::Pointer`]. A leading
/// `file#` selects a file-local pointer; without it the pointer targets the root document.
fn parse_pointer_flag(value: &str) -> OmitRule {
    match value.split_once('#') {
        Some((file, pointer)) if !file.is_empty() => OmitRule::Pointer {
            file: Some(leak(file.to_owned())),
            pointer: leak(pointer.to_owned()),
        },
        // `#/pointer` (empty file) or a bare `/pointer` both target the root document.
        Some((_, pointer)) => OmitRule::Pointer {
            file: None,
            pointer: leak(pointer.to_owned()),
        },
        None => OmitRule::Pointer {
            file: None,
            pointer: leak(value.to_owned()),
        },
    }
}

fn parse_method(method: &str) -> Result<OmitMethod, ConfigError> {
    Ok(match method.to_ascii_lowercase().as_str() {
        "get" => OmitMethod::Get,
        "put" => OmitMethod::Put,
        "post" => OmitMethod::Post,
        "delete" => OmitMethod::Delete,
        "options" => OmitMethod::Options,
        "head" => OmitMethod::Head,
        "patch" => OmitMethod::Patch,
        "trace" => OmitMethod::Trace,
        other => {
            return Err(ConfigError::new(format!(
                "unknown HTTP method `{other}`; expected one of get/put/post/delete/options/head/patch/trace"
            )))
        }
    })
}

fn parse_component_kind(kind: &str) -> Result<ComponentKind, ConfigError> {
    Ok(match kind {
        "schema" | "schemas" => ComponentKind::Schemas,
        "response" | "responses" => ComponentKind::Responses,
        "parameter" | "parameters" => ComponentKind::Parameters,
        "requestBody" | "requestBodies" => ComponentKind::RequestBodies,
        "header" | "headers" => ComponentKind::Headers,
        "securityScheme" | "securitySchemes" => ComponentKind::SecuritySchemes,
        other => {
            return Err(ConfigError::new(format!(
                "unknown component kind `{other}`; expected one of \
                 schema/response/parameter/requestBody/header/securityScheme"
            )))
        }
    })
}

/// Leak an owned string to `&'static str`. The library omit-rule types borrow `'static` (they are
/// designed for the compile-time `omit!` macro), and the CLI process is short-lived, so leaking a
/// bounded number of small config-derived strings for the duration of the run is acceptable.
fn leak(value: String) -> &'static str {
    Box::leak(value.into_boxed_str())
}

// --- TOML DTOs ---------------------------------------------------------------------------------

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    features: Option<FeaturesToml>,
    #[serde(default)]
    omit: Vec<OmitToml>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct FeaturesToml {
    uuid: Option<bool>,
    time: Option<bool>,
    error_body_cap: Option<usize>,
    batch_cap: Option<usize>,
    as_crate: Option<bool>,
}

/// An `[[omit]]` entry. The rule kind is discriminated by which fields are present (TOML has no
/// native enums): `pointer` ⇒ Pointer, `component`+`name` ⇒ Component, `method`+`path` ⇒
/// Operation, `path` alone ⇒ Path.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct OmitToml {
    path: Option<String>,
    method: Option<String>,
    component: Option<String>,
    name: Option<String>,
    pointer: Option<String>,
    file: Option<String>,
}

impl OmitToml {
    fn to_rule(&self) -> Result<OmitRule, String> {
        if let Some(pointer) = &self.pointer {
            if self.path.is_some() || self.method.is_some() || self.component.is_some() {
                return Err("`pointer` cannot be combined with path/method/component".to_owned());
            }
            return Ok(OmitRule::Pointer {
                file: self.file.clone().map(leak),
                pointer: leak(pointer.clone()),
            });
        }
        if let Some(component) = &self.component {
            let name = self.name.as_ref().ok_or("`component` requires a `name`")?;
            return Ok(OmitRule::Component {
                kind: parse_component_kind(component).map_err(|error| error.message)?,
                name: leak(name.clone()),
            });
        }
        if let Some(path) = &self.path {
            return match &self.method {
                Some(method) => Ok(OmitRule::Operation {
                    method: parse_method(method).map_err(|error| error.message)?,
                    path: leak(path.clone()),
                }),
                None => Ok(OmitRule::Path {
                    path: leak(path.clone()),
                }),
            };
        }
        Err("must specify one of `path`, `component` (+`name`), or `pointer`".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_each_omit_flag_kind() {
        assert_eq!(
            parse_operation_flag("get /pets").unwrap(),
            OmitRule::Operation {
                method: OmitMethod::Get,
                path: "/pets"
            }
        );
        assert_eq!(
            parse_component_flag("schema:LegacyPet").unwrap(),
            OmitRule::Component {
                kind: ComponentKind::Schemas,
                name: "LegacyPet"
            }
        );
        assert_eq!(
            parse_pointer_flag("extra.yaml#/components/schemas/X"),
            OmitRule::Pointer {
                file: Some("extra.yaml"),
                pointer: "/components/schemas/X"
            }
        );
        assert_eq!(
            parse_pointer_flag("/paths/~1legacy"),
            OmitRule::Pointer {
                file: None,
                pointer: "/paths/~1legacy"
            }
        );
    }

    #[test]
    fn bad_omit_flag_syntax_errors() {
        assert!(parse_operation_flag("get").is_err());
        assert!(parse_operation_flag("frobnicate /x").is_err());
        assert!(parse_component_flag("LegacyPet").is_err());
        assert!(parse_component_flag("bogus:Name").is_err());
    }

    #[test]
    fn toml_omit_entries_map_by_field_presence() {
        let file: FileConfig = toml::from_str(
            r#"
            [features]
            uuid = false
            time = false
            as_crate = true
            error_body_cap = 4096
            batch_cap = 7

            [[omit]]
            path = "/pets/{id}"

            [[omit]]
            method = "post"
            path = "/pets"

            [[omit]]
            component = "schema"
            name = "LegacyPet"

            [[omit]]
            pointer = "/components/schemas/X"
            file = "extra.yaml"
            "#,
        )
        .unwrap();
        let features = file.features.unwrap();
        assert_eq!(features.uuid, Some(false));
        assert_eq!(features.as_crate, Some(true));
        assert_eq!(features.error_body_cap, Some(4096));
        assert_eq!(features.batch_cap, Some(7));
        let rules: Vec<OmitRule> = file.omit.iter().map(|e| e.to_rule().unwrap()).collect();
        assert_eq!(rules[0], OmitRule::Path { path: "/pets/{id}" });
        assert_eq!(
            rules[1],
            OmitRule::Operation {
                method: OmitMethod::Post,
                path: "/pets"
            }
        );
        assert_eq!(
            rules[2],
            OmitRule::Component {
                kind: ComponentKind::Schemas,
                name: "LegacyPet"
            }
        );
        assert_eq!(
            rules[3],
            OmitRule::Pointer {
                file: Some("extra.yaml"),
                pointer: "/components/schemas/X"
            }
        );
    }

    #[test]
    fn ambiguous_or_empty_omit_entry_errors() {
        let file: FileConfig = toml::from_str("[[omit]]\ncomponent = \"schema\"\n").unwrap();
        assert!(file.omit[0].to_rule().is_err(), "component without name");
        let empty: FileConfig = toml::from_str("[[omit]]\n").unwrap();
        assert!(empty.omit[0].to_rule().is_err(), "no discriminating field");
    }
}
