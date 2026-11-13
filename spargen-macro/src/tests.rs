use super::{config, failure_message, spec_for, tracking, Args, Consumer, SWITCHES};
use spargen::{
    CarriedDocs, Carve, ComponentKind, DateTimeBackend, Narrowing, OmitMethod, OmitRule, Spec,
    TestingModule, UnionTags, UuidMapping,
};

fn os(value: &str) -> Option<std::ffi::OsString> {
    Some(value.into())
}

#[test]
fn an_unnamed_consumer_is_an_error_not_the_working_directory() {
    // #195: with neither variable set, auditing `./Cargo.toml` would read the (usually
    // virtual) workspace root under a workspace build and report every runtime dependency
    // missing at spargen's floor version. Not knowing the crate is an error.
    let error = Consumer::locate(None, None).unwrap_err();
    assert!(
        error.contains("CARGO_MANIFEST_PATH") && error.contains("CARGO_MANIFEST_DIR"),
        "{error}"
    );
    // An empty value joined onto would reproduce the same working-directory guess.
    assert_eq!(Consumer::locate(os(""), os("")).unwrap_err(), error);
}

#[test]
fn the_consumer_comes_from_whichever_variable_cargo_set() {
    let dir = "/work/member";
    let manifest = "/work/member/Cargo.toml";
    let expected = Consumer {
        dir: dir.to_owned(),
        manifest: manifest.to_owned(),
    };
    assert_eq!(Consumer::locate(os(manifest), os(dir)).unwrap(), expected);
    assert_eq!(Consumer::locate(None, os(dir)).unwrap(), expected);
    assert_eq!(Consumer::locate(os(manifest), None).unwrap(), expected);
    // An empty variable beside a set one is ignored, not joined onto.
    assert_eq!(Consumer::locate(os(""), os(dir)).unwrap(), expected);
    assert_eq!(Consumer::locate(os(manifest), os("")).unwrap(), expected);

    let consumer = Consumer::locate(None, os(dir)).unwrap();
    assert_eq!(
        consumer.resolve_spec_path("openapi.yaml"),
        std::path::Path::new(dir)
            .join("openapi.yaml")
            .to_string_lossy()
    );
    assert_eq!(consumer.resolve_spec_path(manifest), manifest);

    // A bare relative manifest path has an empty parent; the spec directory is then the one
    // that path is relative to, spelled `.`, never an empty prefix.
    let bare = Consumer::locate(os("Cargo.toml"), None).unwrap();
    assert_eq!(
        bare,
        Consumer {
            dir: ".".to_owned(),
            manifest: "Cargo.toml".to_owned(),
        }
    );
    assert_eq!(
        bare.resolve_spec_path("openapi.yaml"),
        std::path::Path::new(".")
            .join("openapi.yaml")
            .to_string_lossy()
    );
}

#[cfg(unix)]
#[test]
fn a_non_utf8_location_is_an_error_not_a_lossy_path() {
    use std::os::unix::ffi::OsStringExt;
    let invalid = std::ffi::OsString::from_vec(b"/work/\xFF/Cargo.toml".to_vec());
    let error = Consumer::locate(Some(invalid), None).unwrap_err();
    assert!(
        error.contains("`CARGO_MANIFEST_PATH` is not valid UTF-8"),
        "{error}"
    );
}

/// Every argument the keyed form accepts, each set away from its default.
const EVERY_CONTROL: &str = r#"spec = "openapi.yaml", no_uuid, datetime = "jiff", carve,
               open_narrowing, open_unions, verbatim_docs, testing, error_body_cap = 4096,
               batch_cap = 7,
               omit {
                   operations { post "/legacy"; }
                   paths { "/internal/**"; }
                   components { schemas { "Legacy"; } }
                   pointers { "/webhooks"; }
                   file("shared.yaml") { pointers { "/Legacy"; } }
               },
               keep { get "/pets"; post "/pets"; },
               bind_header = "X-Api-Version", bind_header = "X-Tenant""#;

/// The operations [`EVERY_CONTROL`]'s `keep` block names, in the order it names them.
fn every_kept_operation() -> Vec<(OmitMethod, String)> {
    vec![
        (OmitMethod::Get, "/pets".to_owned()),
        (OmitMethod::Post, "/pets".to_owned()),
    ]
}

/// The rules [`EVERY_CONTROL`]'s `omit` block names, in the order it names them.
fn every_omit_rule() -> Vec<OmitRule> {
    vec![
        OmitRule::operation(OmitMethod::Post, "/legacy"),
        OmitRule::path("/internal/**"),
        OmitRule::component(ComponentKind::Schemas, "Legacy"),
        OmitRule::pointer(None, "/webhooks"),
        OmitRule::pointer(Some("shared.yaml".into()), "/Legacy"),
    ]
}

#[test]
fn parses_every_build_configuration_control() {
    let args: Args = syn::parse_str(EVERY_CONTROL).unwrap();

    assert_eq!(args.spec.value(), "openapi.yaml");
    assert_eq!(args.uuid, Some(UuidMapping::String));
    assert_eq!(args.datetime, Some(DateTimeBackend::Jiff));
    assert_eq!(args.carve, Some(Carve::Auto));
    assert_eq!(args.open_narrowing, Some(Narrowing::Open));
    assert_eq!(args.open_unions, Some(UnionTags::Open));
    assert_eq!(args.verbatim_docs, Some(CarriedDocs::Verbatim));
    assert_eq!(args.testing, Some(TestingModule::Emitted));
    assert_eq!(args.error_body_cap, Some(4096));
    assert_eq!(args.batch_cap, Some(7));
    assert_eq!(args.omit.rules, every_omit_rule());
    assert_eq!(args.keep, every_kept_operation());
    assert_eq!(args.bind_headers, ["X-Api-Version", "X-Tenant"]);
}

#[test]
fn a_bare_spec_path_sets_no_control() {
    let args: Args = syn::parse_str(r#""openapi.yaml""#).unwrap();

    assert_eq!(args.spec.value(), "openapi.yaml");
    assert_eq!(args.uuid, None);
    assert_eq!(args.datetime, None);
    assert_eq!(args.carve, None);
    assert_eq!(args.open_narrowing, None);
    assert_eq!(args.open_unions, None);
    assert_eq!(args.verbatim_docs, None);
    assert_eq!(args.testing, None);
    assert_eq!(args.error_body_cap, None);
    assert_eq!(args.batch_cap, None);
    assert_eq!(args.omit.rules, Vec::new());
    assert_eq!(args.keep, Vec::new());
    assert!(args.bind_headers.is_empty());
}

/// Each argument reaches the `Spec` field it names, and nothing else changes.
#[test]
fn every_argument_maps_onto_its_spec_field() {
    let args: Args = syn::parse_str(EVERY_CONTROL).unwrap();
    let mut expected = Spec::new("openapi.yaml")
        .with_uuid(spargen::UuidMapping::String)
        .with_datetime(DateTimeBackend::Jiff)
        .with_carve(spargen::Carve::Auto)
        .with_open_narrowing(spargen::Narrowing::Open)
        .with_open_unions(spargen::UnionTags::Open)
        .with_verbatim_docs(spargen::CarriedDocs::Verbatim)
        .with_testing(spargen::TestingModule::Emitted)
        .with_error_body_cap(4096)
        .with_batch_cap(7);
    for rule in every_omit_rule() {
        expected = expected.omit_rule(rule);
    }
    for (method, path) in every_kept_operation() {
        expected = expected.keep_operation(method, path);
    }
    expected = expected
        .bind_header("X-Api-Version")
        .bind_header("X-Tenant");
    assert_eq!(config(&args, Spec::new("openapi.yaml")), expected);
}

/// The macro overrides the discovered config file only where it was given an argument: an
/// absent flag keeps the file's value rather than resetting it, and omit rules append.
#[test]
fn an_absent_argument_keeps_the_config_files_value() {
    let discovered = Spec::new("openapi.yaml")
        .with_uuid(spargen::UuidMapping::String)
        .with_datetime(DateTimeBackend::Chrono)
        .with_carve(spargen::Carve::Auto)
        .with_open_narrowing(spargen::Narrowing::Open)
        .with_open_unions(spargen::UnionTags::Open)
        .with_verbatim_docs(spargen::CarriedDocs::Verbatim)
        .with_testing(spargen::TestingModule::Emitted)
        .with_error_body_cap(1024)
        .with_batch_cap(3)
        .omit_rule(OmitRule::path("/from-file"))
        .keep_operation(OmitMethod::Get, "/from-file")
        .bind_header("X-From-File");

    let bare: Args = syn::parse_str(r#""openapi.yaml""#).unwrap();
    assert_eq!(config(&bare, discovered.clone()), discovered);

    let capped: Args = syn::parse_str(
        r#""openapi.yaml", batch_cap = 9, omit { paths { "/from-macro"; } },
           keep { put "/from-macro"; }, bind_header = "X-From-Macro""#,
    )
    .unwrap();
    let configured = config(&capped, discovered.clone());
    assert_eq!(
        configured,
        discovered
            .with_batch_cap(9)
            .omit_rule(OmitRule::path("/from-macro"))
            .keep_operation(OmitMethod::Put, "/from-macro")
            .bind_header("X-From-Macro")
    );
    assert_eq!(
        configured.bound_headers(),
        ["X-From-File", "X-From-Macro"],
        "the macro's binding follows the file's"
    );
}

/// Each switch overrides the config file in both directions: its `= bool` form sets either
/// value, its bare key sets `true`, and its `no_<switch>` form sets `false`.
#[test]
fn each_switch_overrides_the_config_file_either_way() {
    let on = Spec::new("openapi.yaml")
        .with_uuid(spargen::UuidMapping::Uuid)
        .with_carve(spargen::Carve::Auto)
        .with_open_narrowing(spargen::Narrowing::Open)
        .with_open_unions(spargen::UnionTags::Open)
        .with_verbatim_docs(spargen::CarriedDocs::Verbatim)
        .with_testing(spargen::TestingModule::Emitted);
    let off = Spec::new("openapi.yaml")
        .with_uuid(spargen::UuidMapping::String)
        .with_carve(spargen::Carve::Off)
        .with_open_narrowing(spargen::Narrowing::Exact)
        .with_open_unions(spargen::UnionTags::Closed)
        .with_verbatim_docs(spargen::CarriedDocs::Rewritten)
        .with_testing(spargen::TestingModule::Omitted);

    let turn_off = args(
        r#""openapi.yaml", uuid = false, carve = false, open_narrowing = false, open_unions = false,
           verbatim_docs = false, testing = false"#,
    );
    assert_eq!(config(&turn_off, on.clone()), off);
    let turn_on = args(
        r#""openapi.yaml", uuid = true, carve = true, open_narrowing = true, open_unions = true,
           verbatim_docs = true, testing = true"#,
    );
    assert_eq!(config(&turn_on, off.clone()), on);
    let bare =
        args(r#""openapi.yaml", uuid, carve, open_narrowing, open_unions, verbatim_docs, testing"#);
    assert_eq!(config(&bare, off.clone()), on);
    let negated = args(
        r#""openapi.yaml", no_uuid, no_carve, no_open_narrowing, no_open_unions,
           no_verbatim_docs, no_testing"#,
    );
    assert_eq!(config(&negated, on.clone()), off);
    for switch in SWITCHES {
        let one = args(&format!(r#""openapi.yaml", no_{switch}"#));
        let applied = config(&one, on.clone());
        assert_ne!(applied, on, "`no_{switch}` changed nothing");
        assert_eq!(
            config(
                &args(&format!(r#""openapi.yaml", {switch} = false"#)),
                on.clone()
            ),
            applied,
            "`no_{switch}` and `{switch} = false` differ"
        );
    }
}

/// Each `no_<switch>` turns off a switch the `spargen.toml` beside the spec turns on.
#[test]
fn each_negated_switch_overrides_a_config_file_that_enables_it() {
    let scratch = Scratch::new("negated-over-config");
    scratch.write(
        "spargen.toml",
        "uuid = true\ncarve = true\nopen_narrowing = true\nopen_unions = true\n\
         verbatim_docs = true\ntesting = true\n",
    );
    let enabled = spec_for(&args(r#""openapi.yaml""#), &scratch.consumer()).unwrap();
    assert_eq!(enabled.uuid(), UuidMapping::Uuid);
    assert_eq!(enabled.carve(), Carve::Auto);
    assert_eq!(enabled.open_narrowing(), Narrowing::Open);
    assert_eq!(enabled.open_unions(), UnionTags::Open);
    assert_eq!(enabled.verbatim_docs(), CarriedDocs::Verbatim);
    assert_eq!(enabled.testing(), TestingModule::Emitted);

    let negated = spec_for(
        &args(
            r#""openapi.yaml", no_uuid, no_carve, no_open_narrowing, no_open_unions,
               no_verbatim_docs, no_testing"#,
        ),
        &scratch.consumer(),
    )
    .unwrap();
    assert_eq!(
        negated,
        enabled
            .with_uuid(UuidMapping::String)
            .with_carve(Carve::Off)
            .with_open_narrowing(Narrowing::Exact)
            .with_open_unions(UnionTags::Closed)
            .with_verbatim_docs(CarriedDocs::Rewritten)
            .with_testing(TestingModule::Omitted)
    );
}

/// A switch given in both spellings, in either order and with any value, is an error naming it,
/// and a `no_<switch>` takes no value.
#[test]
fn both_spellings_of_a_switch_together_are_an_error() {
    for switch in SWITCHES {
        let repeated = format!(
            "`{switch}` given more than once (as `{switch}`, `{switch} = …` or `no_{switch}`)"
        );
        for source in [
            format!(r#""a.yaml", {switch}, no_{switch}"#),
            format!(r#""a.yaml", no_{switch}, {switch}"#),
            format!(r#""a.yaml", {switch} = false, no_{switch}"#),
            format!(r#""a.yaml", no_{switch}, no_{switch}"#),
        ] {
            assert_eq!(parse_error(&source), repeated, "{source}");
        }
        assert_eq!(
            parse_error(&format!(r#""a.yaml", no_{switch} = false"#)),
            format!("`no_{switch}` takes no value; write `{switch} = true` or `{switch} = false`")
        );
    }
}

/// `datetime = "…"` replaces the config file's backend with any of the four, and leaves it in
/// place when not given.
#[test]
fn the_datetime_argument_overrides_the_config_file() {
    let discovered = Spec::new("openapi.yaml").with_datetime(DateTimeBackend::Chrono);
    assert_eq!(
        config(&args(r#""openapi.yaml""#), discovered.clone()),
        discovered
    );
    for &backend in DateTimeBackend::ALL {
        let given = args(&format!(r#""openapi.yaml", datetime = "{backend}""#));
        assert_eq!(
            config(&given, discovered.clone()),
            discovered.clone().with_datetime(backend),
            "{backend}"
        );
    }
}

/// Each switch sets its own `Spec` field: `uuid` and `carve`, `carve` and `open_narrowing`, and
/// `open_narrowing` and `open_unions`, `open_unions` and `verbatim_docs`, and `verbatim_docs` and
/// `testing`, are given opposite
/// values over a base holding the inverse of each, so a switch wired to another's setting changes
/// the result.
#[test]
fn each_switch_reaches_its_own_field_when_the_values_differ() {
    let base = Spec::new("openapi.yaml")
        .with_uuid(spargen::UuidMapping::Uuid)
        .with_carve(spargen::Carve::Off)
        .with_open_narrowing(spargen::Narrowing::Open)
        .with_open_unions(spargen::UnionTags::Closed)
        .with_verbatim_docs(spargen::CarriedDocs::Verbatim)
        .with_testing(spargen::TestingModule::Omitted);
    let expected = Spec::new("openapi.yaml")
        .with_uuid(spargen::UuidMapping::String)
        .with_carve(spargen::Carve::Auto)
        .with_open_narrowing(spargen::Narrowing::Exact)
        .with_open_unions(spargen::UnionTags::Open)
        .with_verbatim_docs(spargen::CarriedDocs::Rewritten)
        .with_testing(spargen::TestingModule::Emitted);

    let valued = args(
        r#""openapi.yaml", uuid = false, carve = true, open_narrowing = false, open_unions = true,
           verbatim_docs = false, testing = true"#,
    );
    assert_eq!(config(&valued, base.clone()), expected);
    let spelled = args(
        r#""openapi.yaml", no_uuid, carve, open_narrowing = false, open_unions,
           verbatim_docs = false, testing"#,
    );
    assert_eq!(config(&spelled, base), expected);
}

/// A directory under the system temporary directory, removed on drop.
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("spargen-macro-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }

    /// The UTF-8 path of `relative` inside this directory.
    fn path(&self, relative: &str) -> String {
        self.0.join(relative).to_str().unwrap().to_owned()
    }

    /// Write `text` to `relative`, creating its parent, and return its path.
    fn write(&self, relative: &str, text: &str) -> String {
        let path = self.0.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        self.path(relative)
    }

    /// The consumer whose crate directory this is, with its `Cargo.toml` as the manifest.
    fn consumer(&self) -> Consumer {
        Consumer {
            dir: self.path(""),
            manifest: self.path("Cargo.toml"),
        }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A spec that generates, its one schema reached through a relative-file `$ref`.
const SPLIT_SPEC: &str = r#"openapi: 3.1.0
info: { title: Split, version: "1" }
paths:
  /pets:
    get:
      operationId: listPets
      responses:
        "200":
          description: ok
          content:
            application/json:
              schema: { $ref: "schemas.yaml#/Pet" }
"#;

const SPLIT_SCHEMAS: &str = "Pet:\n  type: object\n  properties:\n    name: { type: string }\n";

fn args(source: &str) -> Args {
    match syn::parse_str(source) {
        Ok(args) => args,
        Err(error) => panic!("`{source}` did not parse: {error}"),
    }
}

fn parse_error(source: &str) -> String {
    match syn::parse_str::<Args>(source) {
        Ok(_) => panic!("`{source}` parsed"),
        Err(error) => error.to_string(),
    }
}

#[test]
fn no_config_file_leaves_the_spec_at_its_defaults() {
    let scratch = Scratch::new("no-config");
    let spec = spec_for(&args(r#""openapi.yaml""#), &scratch.consumer()).unwrap();
    assert_eq!(spec, Spec::new(scratch.path("openapi.yaml")));
}

/// The `spargen.toml` read is the one beside the spec, not the one beside the consumer's
/// `Cargo.toml`, and the macro arguments override it.
#[test]
fn the_config_file_beside_the_spec_is_applied_under_the_arguments() {
    let scratch = Scratch::new("config-beside-spec");
    scratch.write("spargen.toml", "open_narrowing = true\n");
    scratch.write("api/spargen.toml", "carve = true\nbatch_cap = 3\n");

    // `Spec` equality covers the config files applied, so this also pins which file was
    // recorded: the one beside the spec, once.
    let applied = Spec::new(scratch.path("api/openapi.yaml"))
        .config_file(scratch.path("api/spargen.toml"))
        .unwrap();
    assert_eq!(
        applied,
        applied
            .clone()
            .with_carve(spargen::Carve::Auto)
            .with_batch_cap(3)
    );
    let spec = spec_for(&args(r#""api/openapi.yaml""#), &scratch.consumer()).unwrap();
    assert_eq!(spec, applied);

    let spec = spec_for(
        &args(r#""api/openapi.yaml", batch_cap = 9"#),
        &scratch.consumer(),
    )
    .unwrap();
    assert_eq!(spec, applied.with_batch_cap(9));
}

#[test]
fn an_invalid_config_file_is_an_error_naming_it() {
    let scratch = Scratch::new("invalid-config");
    let config = scratch.write("spargen.toml", "[features]\ncarve = true\n");
    let error = spec_for(&args(r#""openapi.yaml""#), &scratch.consumer()).unwrap_err();
    assert_eq!(
        error.to_string(),
        format!(
            "invalid config file `{config}`: the `[features]` table was removed; move its \
                 keys (`carve`, `batch_cap`, …) to the top level of the file"
        )
    );
}

#[test]
fn a_generated_preview_is_no_failure() {
    let scratch = Scratch::new("generated");
    let spec = scratch.write("openapi.yaml", SPLIT_SPEC);
    scratch.write("schemas.yaml", SPLIT_SCHEMAS);
    let preview = spargen::__private::preview(&Spec::new(spec));
    assert_eq!(failure_message("openapi.yaml", &preview), None);
}

/// The `compile_error!` text: the spec as the macro was given it, then each error-severity
/// diagnostic in `Diagnostic`'s own `Display`: code and message, span, pointer, and remedy.
#[test]
fn a_rejected_spec_renders_each_error_with_its_code_span_and_remedy() {
    let scratch = Scratch::new("rejected");
    let spec = scratch.write(
        "openapi.yaml",
        "openapi: 3.0.3\ninfo: { title: Old, version: \"1\" }\npaths: {}\n",
    );
    let preview = spargen::__private::preview(&Spec::new(spec.as_str()));
    // A frontend rejection retains no module; the macro raises `compile_error!` on any
    // rejected outcome rather than emitting the code it may hold.
    assert_eq!(preview.report.outcome(), spargen::Outcome::Rejected);
    assert!(
        preview.contents.is_none(),
        "a frontend rejection retains no generated module"
    );
    // A root-level diagnostic's pointer is empty, so no `at` line is rendered. The span names
    // the spec path as the bundle loaded it, not as the macro was given it.
    assert_eq!(
        failure_message("specs/old.yaml", &preview).unwrap(),
        format!(
            "spargen could not generate a client from `specs/old.yaml`\n\
                 error[E001]: unsupported OpenAPI version `3.0.3`; spargen implements 3.1.x and \
                 3.2.x\n  --> {spec}:1:10\n  \
                 help: use an OpenAPI 3.1.x or 3.2.x document; 3.0.x is rejected because it uses \
                 different schema semantics"
        )
    );
}

/// Several errors are each rendered in turn, in the report's order; a warning beside them is
/// not rendered.
#[test]
fn every_error_is_rendered_in_order_and_warnings_are_left_out() {
    let scratch = Scratch::new("several");
    let spec = scratch.write(
        "openapi.yaml",
        r##"openapi: 3.1.0
info: { title: Dangling, version: "1" }
paths:
  /a:
    get:
      operationId: getA
      parameters:
        - { name: q, in: query, schema: { type: string, pattern: "^a" } }
      responses:
        "200":
          description: ok
          content:
            application/json:
              schema: { $ref: "#/components/schemas/Missing" }
  /b:
    get:
      operationId: getB
      responses:
        "200":
          description: ok
          content:
            application/json:
              schema: { $ref: "#/components/schemas/AlsoMissing" }
"##,
    );
    let preview = spargen::__private::preview(&Spec::new(spec.as_str()));
    assert!(
        preview
            .report
            .diagnostics()
            .iter()
            .any(|d| d.severity == spargen::Severity::Warning),
        "the fixture must carry a warning for its omission to be observed"
    );
    assert_eq!(
        failure_message("openapi.yaml", &preview).unwrap(),
        format!(
            "spargen could not generate a client from `openapi.yaml`\n\
                 error[E004]: unresolved schema reference `#/components/schemas/Missing`\n  \
                 --> {spec}:14:23\n  \
                 at /paths/~1a/get/responses/200/content/application~1json/schema\n\
                 error[E004]: unresolved schema reference `#/components/schemas/AlsoMissing`\n  \
                 --> {spec}:23:23\n  \
                 at /paths/~1b/get/responses/200/content/application~1json/schema"
        )
    );
}

/// A warning that fills `batch_cap` before the rejecting error is emitted gives way to it, so
/// the message names the error rather than saying only that generation did not succeed (#1345).
#[test]
fn an_error_emitted_after_the_cap_is_full_of_warnings_is_rendered() {
    let scratch = Scratch::new("capped");
    let spec = scratch.write(
        "openapi.yaml",
        r##"openapi: 3.1.0
info: { title: Capped, version: "1" }
paths:
  /a:
    get:
      operationId: getA
      parameters:
        - { name: q, in: query, schema: { type: string, pattern: "^a" } }
      responses:
        "200":
          description: ok
          content:
            application/json:
              schema: { $ref: "#/components/schemas/Missing" }
"##,
    );
    let uncapped = spargen::__private::preview(&Spec::new(spec.as_str()));
    assert_eq!(
        uncapped.report.diagnostics().first().map(|d| d.severity),
        Some(spargen::Severity::Warning),
        "the fixture's warning must be emitted before its error: {:#?}",
        uncapped.report
    );
    let preview = spargen::__private::preview(&Spec::new(spec.as_str()).with_batch_cap(1));
    assert_eq!(preview.report.outcome(), spargen::Outcome::Rejected);
    assert!(preview.report.truncated());
    assert_eq!(
        failure_message("openapi.yaml", &preview).unwrap(),
        format!(
            "spargen could not generate a client from `openapi.yaml`\n\
                 error[E004]: unresolved schema reference `#/components/schemas/Missing`\n  \
                 --> {spec}:14:23\n  \
                 at /paths/~1a/get/responses/200/content/application~1json/schema"
        )
    );
}

/// A preview that produced no client and retained no error says so in place of a diagnostic
/// list: a rejection whose errors `batch_cap = 0` dropped, and a generated outcome holding no
/// module.
#[test]
fn a_failure_with_no_retained_error_says_generation_did_not_succeed() {
    let scratch = Scratch::new("no-retained-error");
    let rejected = scratch.write(
        "openapi.yaml",
        "openapi: 3.0.3\ninfo: { title: Old, version: \"1\" }\npaths: {}\n",
    );
    let preview = spargen::__private::preview(&Spec::new(rejected.as_str()).with_batch_cap(0));
    assert_eq!(preview.report.outcome(), spargen::Outcome::Rejected);
    assert!(
        preview.report.diagnostics().is_empty(),
        "a zero cap retains no diagnostic"
    );
    let expected = "spargen could not generate a client from `openapi.yaml`: generation did \
                        not succeed";
    assert_eq!(
        failure_message("openapi.yaml", &preview).as_deref(),
        Some(expected)
    );

    let generated = scratch.write("split/openapi.yaml", SPLIT_SPEC);
    scratch.write("split/schemas.yaml", SPLIT_SCHEMAS);
    let mut preview = spargen::__private::preview(&Spec::new(generated));
    assert_eq!(preview.report.outcome(), spargen::Outcome::Generated);
    preview.contents = None;
    assert_eq!(
        failure_message("openapi.yaml", &preview).as_deref(),
        Some(expected)
    );
}

/// The files a successful expansion `include_bytes!`s, and so the files whose edit makes
/// Cargo re-expand it: the spec, every file its `$ref`s load, the manifest the audit read, a
/// `spargen.lock` beside the spec, and the `spargen.toml` `spec_for` applied (#686).
#[test]
fn tracking_includes_the_spec_its_refs_the_manifest_the_lock_and_the_config() {
    let scratch = Scratch::new("tracking");
    let spec = scratch.write("openapi.yaml", SPLIT_SPEC);
    let schemas = scratch.write("schemas.yaml", SPLIT_SCHEMAS);
    let lock = scratch.write("spargen.lock", "version = 1\n");
    let config = scratch.write("spargen.toml", "carve = false\n");
    let (requirements, _) = spargen::requirements(&Spec::new(spec.clone())).unwrap();
    // `[workspace]` makes the fixture its own workspace root, so the audit's walk upward for
    // one stops here instead of reading an ancestor manifest above the temporary directory.
    let manifest = scratch.write(
        "Cargo.toml",
        &format!(
            "[package]\nname = \"consumer\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n\
                 [workspace]\n\n{}",
            requirements.manifest_block()
        ),
    );

    let configured = spec_for(&args(r#""openapi.yaml""#), &scratch.consumer()).unwrap();
    assert_eq!(configured.path().as_str(), spec);
    let preview = spargen::__private::preview_for_macro(&configured, &manifest);
    assert_eq!(failure_message("openapi.yaml", &preview), None);
    let mut expected = vec![manifest, spec, schemas, lock, config];
    expected.sort();
    assert_eq!(
        tracking(&preview).to_string(),
        quote::quote! { #(const _: &[u8] = include_bytes!(#expected);)* }.to_string()
    );
}

/// A remote `$ref` read from its vendored copy is tracked too: the copy under `.spargen/vendor/`
/// and the `spargen.lock` pinning it, so a re-run of `spargen lock` re-expands the macro.
#[test]
fn tracking_includes_a_vendored_remote_document() {
    let scratch = Scratch::new("tracking-vendored");
    let spec = scratch.write(
        "openapi.yaml",
        &SPLIT_SPEC.replace("schemas.yaml#/Pet", "https://example.com/schemas.yaml#/Pet"),
    );
    // The sha256 of `SPLIT_SCHEMAS`, which the build checks the vendored copy against.
    let lock = scratch.write(
        "spargen.lock",
        "version = 1\n\n[[remote]]\nurl = \"https://example.com/schemas.yaml\"\n\
         sha256 = \"2631dc5f74523ef77cd6eb2a5b4c6d3fba915bbded45ad04b1bf5d7008056b02\"\n\
         path = \"example.com/schemas.yaml\"\n",
    );
    let vendored = scratch.write(".spargen/vendor/example.com/schemas.yaml", SPLIT_SCHEMAS);
    let (requirements, _) = spargen::requirements(&Spec::new(spec.clone()))
        .unwrap_or_else(|report| panic!("{report:#?}"));
    let manifest = scratch.write(
        "Cargo.toml",
        &format!(
            "[package]\nname = \"consumer\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n\
                 [workspace]\n\n{}",
            requirements.manifest_block()
        ),
    );

    let configured = spec_for(&args(r#""openapi.yaml""#), &scratch.consumer()).unwrap();
    let preview = spargen::__private::preview_for_macro(&configured, &manifest);
    assert_eq!(failure_message("openapi.yaml", &preview), None);
    let mut expected = vec![manifest, spec, lock, vendored];
    expected.sort();
    assert_eq!(
        tracking(&preview).to_string(),
        quote::quote! { #(const _: &[u8] = include_bytes!(#expected);)* }.to_string()
    );
}

/// A spec that generates but whose consumer manifest fails the runtime-dependency audit is a
/// failure too, though a module was rendered: the message lists each `E023` the audit reported.
#[test]
fn an_audit_only_rejection_renders_its_e023_errors() {
    let scratch = Scratch::new("audit-only");
    let spec = scratch.write("openapi.yaml", SPLIT_SPEC);
    scratch.write("schemas.yaml", SPLIT_SCHEMAS);
    let manifest = scratch.write(
        "Cargo.toml",
        "[package]\nname = \"consumer\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n\
             [workspace]\n",
    );

    let preview = spargen::__private::preview_for_macro(&Spec::new(spec), &manifest);
    assert_eq!(preview.report.outcome(), spargen::Outcome::Rejected);
    assert!(preview.contents.is_some(), "the audit runs after rendering");
    let errors: Vec<&spargen::Diagnostic> = preview.report.errors().collect();
    assert!(!errors.is_empty(), "{:#?}", preview.report);
    assert!(
        errors
            .iter()
            .all(|error| error.code == spargen::Code::RuntimeDependencyContract),
        "{:#?}",
        preview.report
    );
    let mut expected = String::from("spargen could not generate a client from `openapi.yaml`");
    for error in &errors {
        expected.push_str(&format!("\n{error}"));
    }
    let message = failure_message("openapi.yaml", &preview).unwrap();
    assert_eq!(message, expected);
    assert!(message.contains("error[E023]"), "{message}");
}

/// An `omit { file("…") { … } }` path is relative to the spec's directory, as a `$ref` is, not to
/// the crate's `Cargo.toml` the spec path itself is relative to.
#[test]
fn an_omit_file_path_is_relative_to_the_spec() {
    let scratch = Scratch::new("omit-file-base");
    let spec = scratch.write("api/openapi.yaml", SPLIT_SPEC);
    scratch.write("api/schemas.yaml", SPLIT_SCHEMAS);
    let (requirements, _) = spargen::requirements(&Spec::new(spec)).unwrap();
    let manifest = scratch.write(
        "Cargo.toml",
        &format!(
            "[package]\nname = \"consumer\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n\
                 [workspace]\n\n{}",
            requirements.manifest_block()
        ),
    );
    let preview = |source: &str| {
        let configured = spec_for(&args(source), &scratch.consumer()).unwrap();
        let preview = spargen::__private::preview_for_macro(&configured, &manifest);
        assert_eq!(
            failure_message("api/openapi.yaml", &preview),
            None,
            "{source}"
        );
        preview.contents.unwrap()
    };

    // `Pet`'s one property is the pointer omitted, so the struct is empty exactly when the rule
    // found its file.
    let emptied = "pub struct Pet {}";
    assert!(!preview(r#""api/openapi.yaml""#).contains(emptied));
    let omitted = preview(
        r#""api/openapi.yaml",
           omit { file("schemas.yaml") { pointers { "/Pet/properties/name"; } } }"#,
    );
    assert!(omitted.contains(emptied), "{omitted}");
}

#[test]
fn every_argument_parse_error_names_what_was_wrong() {
    let cases: &[(&str, &str)] = &[
        (
            r#""a.yaml", batch_cap = 1, batch_cap = 2"#,
            "`batch_cap` given more than once",
        ),
        (
            r#""a.yaml", error_body_cap = 1, error_body_cap = 2"#,
            "`error_body_cap` given more than once",
        ),
        (r#""a.yaml", "b.yaml""#, "spec path given more than once"),
        (
            r#""a.yaml", spec = "b.yaml""#,
            "spec path given more than once",
        ),
        (r#"spec "a.yaml""#, "expected `=`"),
        (r#"spec = 1"#, "expected string literal"),
        (r#"42"#, "expected identifier"),
        (
            r#""a.yaml", uuids"#,
            "unknown argument `uuids`; expected a spec path, a switch (uuid, carve, \
                 open_narrowing, open_unions, verbatim_docs, testing, each also as \
                 `no_<switch>`), or one of: datetime, error_body_cap, batch_cap, omit, keep, \
                 bind_header",
        ),
        (
            r#""a.yaml", no_datetime"#,
            "unknown argument `no_datetime`; expected a spec path, a switch (uuid, carve, \
                 open_narrowing, open_unions, verbatim_docs, testing, each also as \
                 `no_<switch>`), or one of: datetime, error_body_cap, batch_cap, omit, keep, \
                 bind_header",
        ),
        (r#""a.yaml", uuid = 1"#, "expected boolean literal"),
        (r#""a.yaml", carve = "yes""#, "expected boolean literal"),
        (
            r#""a.yaml", no_uuid = true"#,
            "`no_uuid` takes no value; write `uuid = true` or `uuid = false`",
        ),
        (
            r#""a.yaml", datetime = "Jiff""#,
            "unknown date-time backend `Jiff`; expected time, jiff, chrono or string",
        ),
        (r#""a.yaml", datetime = jiff"#, "expected string literal"),
        (
            r#""a.yaml", no_uuid, uuid = true"#,
            "`uuid` given more than once (as `uuid`, `uuid = …` or `no_uuid`)",
        ),
        (
            r#""a.yaml", datetime = "jiff", datetime = "time""#,
            "`datetime` given more than once",
        ),
        (
            r#""a.yaml", carve, carve = false"#,
            "`carve` given more than once (as `carve`, `carve = …` or `no_carve`)",
        ),
        (
            r#""a.yaml", open_narrowing = true, open_narrowing"#,
            "`open_narrowing` given more than once (as `open_narrowing`, `open_narrowing = …` or \
                 `no_open_narrowing`)",
        ),
        (
            r#""a.yaml", open_unions, open_unions = false"#,
            "`open_unions` given more than once (as `open_unions`, `open_unions = …` or \
                 `no_open_unions`)",
        ),
        (
            r#""a.yaml", verbatim_docs, verbatim_docs = false"#,
            "`verbatim_docs` given more than once (as `verbatim_docs`, `verbatim_docs = …` or \
                 `no_verbatim_docs`)",
        ),
        (
            r#""a.yaml", testing, testing = false"#,
            "`testing` given more than once (as `testing`, `testing = …` or `no_testing`)",
        ),
        (r#""a.yaml" no_uuid"#, "expected `,`"),
        (
            "no_uuid",
            "unexpected end of input, expected a spec path, e.g. \
                 generate_api!(\"openapi.yaml\")",
        ),
        (
            r#""a.yaml", error_body_cap = "big""#,
            "expected integer literal",
        ),
        (
            r#""a.yaml", error_body_cap = 99999999999999999999999"#,
            "number too large to fit in target type",
        ),
        (r#""a.yaml", batch_cap 3"#, "expected `=`"),
        (r#""a.yaml", bind_header "X-A""#, "expected `=`"),
        (r#""a.yaml", bind_header = X"#, "expected string literal"),
        (
            r#""a.yaml", batch_cap = -1"#,
            "invalid digit found in string",
        ),
        (r#""a.yaml", omit ( )"#, "expected curly braces"),
        (
            r#""a.yaml", omit { schemas { } }"#,
            "unknown omit section `schemas`",
        ),
        (
            r#""a.yaml", omit { operations { fetch "/a"; } }"#,
            "unknown HTTP method: `fetch`; \
                 expected one of get/put/post/delete/options/head/patch/trace/query",
        ),
        (
            r#""a.yaml", omit { operations { get "/a" } }"#,
            "expected `;`",
        ),
        (r#""a.yaml", omit { paths { "/a" } }"#, "expected `;`"),
        (
            r#""a.yaml", omit { components { models { "A"; } } }"#,
            "unknown component kind: `models`; expected one of \
                 schemas/responses/parameters/request_bodies/headers/security_schemes/\
                 path_items/media_types",
        ),
        (r#""a.yaml", omit { pointers { "/a" } }"#, "expected `;`"),
        (
            r#""a.yaml", omit { file("b.yaml") { paths { "/a"; } } }"#,
            "expected `pointers`",
        ),
        (
            r#""a.yaml", omit { file { pointers { "/a"; } } }"#,
            "expected parentheses",
        ),
        (r#""a.yaml", keep ( )"#, "expected curly braces"),
        (
            r#""a.yaml", keep { fetch "/a"; }"#,
            "unknown HTTP method: `fetch`; \
                 expected one of get/put/post/delete/options/head/patch/trace/query",
        ),
        (r#""a.yaml", keep { get "/a" }"#, "expected `;`"),
    ];
    let mut mismatches = Vec::new();
    for (source, expected) in cases {
        let actual = parse_error(source);
        if actual != *expected {
            mismatches.push(format!(
                "{source}\n  expected: {expected}\n  actual:   {actual}"
            ));
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}
