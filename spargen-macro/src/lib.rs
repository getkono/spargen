#![doc = include_str!("../README.md")]

use proc_macro::TokenStream;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::{braced, parenthesized, Ident, LitBool, LitInt, LitStr, Token};

/// Generate a typed OpenAPI 3.1.x/3.2.x client in place. See the [crate docs](crate) for forms and
/// caveats.
#[proc_macro]
pub fn generate_api(input: TokenStream) -> TokenStream {
    let args = syn::parse_macro_input!(input as Args);
    match expand(&args) {
        // `expand` hands back source text, built off the compiler bridge (see `FallbackGuard`).
        // Tokenizing it here, once, on the real macro server thread, is what binds real spans to
        // the output.
        Ok(source) => match source.parse() {
            Ok(stream) => stream,
            Err(error) => syn::Error::new(
                args.spec.span(),
                format!("spargen produced code that failed to tokenize: {error}"),
            )
            .to_compile_error()
            .into(),
        },
        Err(error) => error.to_compile_error().into(),
    }
}

/// Forces proc-macro2's thread-safe fallback token implementation for its lifetime, restoring the
/// real compiler bridge on drop (even across a panic).
///
/// spargen builds tokens with proc-macro2 internally, on its own worker thread. Inside a
/// proc-macro, proc-macro2 otherwise routes to the real compiler bridge — whose API panics when
/// touched off the macro server thread ("procedural macro API is used outside of a procedural
/// macro"). The fallback is spargen's normal mode under `build.rs`/CLI, so this changes nothing
/// about the output; it just keeps generation off the bridge.
struct FallbackGuard;

impl FallbackGuard {
    fn force() -> Self {
        proc_macro2::fallback::force();
        FallbackGuard
    }
}

impl Drop for FallbackGuard {
    fn drop(&mut self) {
        proc_macro2::fallback::unforce();
    }
}

/// Parsed macro arguments: a spec path (positional string or `spec = "..."`) plus optional flags.
///
/// Each switch holds the value of the `Spec` setting it controls, and is `None` when the macro was
/// not given it, so the config file's value stands.
struct Args {
    spec: LitStr,
    uuid: Option<spargen::UuidMapping>,
    datetime: Option<spargen::DateTimeBackend>,
    carve: Option<spargen::Carve>,
    open_narrowing: Option<spargen::Narrowing>,
    open_unions: Option<spargen::UnionTags>,
    verbatim_docs: Option<spargen::CarriedDocs>,
    testing: Option<spargen::TestingModule>,
    error_body_cap: Option<usize>,
    batch_cap: Option<usize>,
    omit: spargen::Omit,
    /// The operations a `keep { … }` block names, in the order it names them.
    keep: Vec<(spargen::OmitMethod, String)>,
    /// The header names each `bind_header = "…"` argument binds, in the order given.
    bind_headers: Vec<String>,
}

/// Every switch `generate_api!` takes, in the order its errors list them. Each is spelled `<name>`
/// (alone, `= true` or `= false`) or `no_<name>`.
const SWITCHES: [&str; 6] = [
    "uuid",
    "carve",
    "open_narrowing",
    "open_unions",
    "verbatim_docs",
    "testing",
];

/// The switch `key` spells, as its name in [`SWITCHES`] and whether `key` is its `no_<name>`
/// form, or `None` when `key` names no switch.
fn switch_key(key: &str) -> Option<(&'static str, bool)> {
    let (name, negated) = match key.strip_prefix("no_") {
        Some(name) => (name, true),
        None => (key, false),
    };
    SWITCHES
        .iter()
        .find(|switch| **switch == name)
        .map(|name| (*name, negated))
}

/// Parse the switch `name`, spelled `key`, into `slot`: `on` for the bare key or `= true`, `off`
/// for `= false` or the `no_` form (`negated`), which takes no value. A switch set twice, in
/// either spelling, is an error rather than a silent last-wins.
fn parse_switch<T>(
    input: ParseStream,
    key: &Ident,
    (name, negated): (&str, bool),
    slot: &mut Option<T>,
    on: T,
    off: T,
) -> syn::Result<()> {
    let value = if negated {
        if input.peek(Token![=]) {
            return Err(syn::Error::new(
                key.span(),
                format!("`{key}` takes no value; write `{name} = true` or `{name} = false`"),
            ));
        }
        false
    } else if input.peek(Token![=]) {
        input.parse::<Token![=]>()?;
        input.parse::<LitBool>()?.value
    } else {
        true
    };
    if slot.is_some() {
        return Err(syn::Error::new(
            key.span(),
            format!("`{name}` given more than once (as `{name}`, `{name} = …` or `no_{name}`)"),
        ));
    }
    *slot = Some(if value { on } else { off });
    Ok(())
}

impl Parse for Args {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        use spargen::{CarriedDocs, Carve, Narrowing, TestingModule, UnionTags, UuidMapping};

        let mut spec: Option<LitStr> = None;
        let mut uuid = None;
        let mut datetime = None;
        let mut carve = None;
        let mut open_narrowing = None;
        let mut open_unions = None;
        let mut verbatim_docs = None;
        let mut testing = None;
        let mut error_body_cap = None;
        let mut batch_cap = None;
        let mut omit = spargen::Omit::default();
        let mut keep = Vec::new();
        let mut bind_headers = Vec::new();

        while !input.is_empty() {
            if input.peek(LitStr) {
                let lit: LitStr = input.parse()?;
                if spec.is_some() {
                    return Err(syn::Error::new(
                        lit.span(),
                        "spec path given more than once",
                    ));
                }
                spec = Some(lit);
            } else {
                let key: Ident = input.parse()?;
                let text = key.to_string();
                if let Some(switch) = switch_key(&text) {
                    match switch.0 {
                        "uuid" => parse_switch(
                            input,
                            &key,
                            switch,
                            &mut uuid,
                            UuidMapping::Uuid,
                            UuidMapping::String,
                        )?,
                        "carve" => {
                            parse_switch(input, &key, switch, &mut carve, Carve::Auto, Carve::Off)?
                        }
                        "open_narrowing" => parse_switch(
                            input,
                            &key,
                            switch,
                            &mut open_narrowing,
                            Narrowing::Open,
                            Narrowing::Exact,
                        )?,
                        "open_unions" => parse_switch(
                            input,
                            &key,
                            switch,
                            &mut open_unions,
                            UnionTags::Open,
                            UnionTags::Closed,
                        )?,
                        "verbatim_docs" => parse_switch(
                            input,
                            &key,
                            switch,
                            &mut verbatim_docs,
                            CarriedDocs::Verbatim,
                            CarriedDocs::Rewritten,
                        )?,
                        "testing" => parse_switch(
                            input,
                            &key,
                            switch,
                            &mut testing,
                            TestingModule::Emitted,
                            TestingModule::Omitted,
                        )?,
                        other => unreachable!("switch `{other}` has no slot"),
                    }
                } else {
                    match text.as_str() {
                        "spec" => {
                            input.parse::<Token![=]>()?;
                            let lit: LitStr = input.parse()?;
                            if spec.is_some() {
                                return Err(syn::Error::new(
                                    lit.span(),
                                    "spec path given more than once",
                                ));
                            }
                            spec = Some(lit);
                        }
                        "datetime" => parse_datetime(input, &key, &mut datetime)?,
                        "error_body_cap" => parse_cap(input, &key, &mut error_body_cap)?,
                        "batch_cap" => parse_cap(input, &key, &mut batch_cap)?,
                        "omit" => parse_omit(input, &mut omit)?,
                        "keep" => {
                            let body;
                            braced!(body in input);
                            keep.extend(parse_method_paths(&body)?);
                        }
                        "bind_header" => {
                            input.parse::<Token![=]>()?;
                            bind_headers.push(input.parse::<LitStr>()?.value());
                        }
                        other => {
                            return Err(syn::Error::new(
                                key.span(),
                                format!(
                                    "unknown argument `{other}`; expected a spec path, a switch \
                                 ({}, each also as `no_<switch>`), or one of: datetime, \
                                 error_body_cap, batch_cap, omit, keep, bind_header",
                                    SWITCHES.join(", ")
                                ),
                            ));
                        }
                    }
                }
            }

            if input.is_empty() {
                break;
            }
            input.parse::<Token![,]>()?;
        }

        let spec = spec.ok_or_else(|| {
            input.error("expected a spec path, e.g. generate_api!(\"openapi.yaml\")")
        })?;
        Ok(Args {
            spec,
            uuid,
            datetime,
            carve,
            open_narrowing,
            open_unions,
            verbatim_docs,
            testing,
            error_body_cap,
            batch_cap,
            omit,
            keep,
            bind_headers,
        })
    }
}

/// The source `generate_api!` expands to: the generated module followed by its `tracking` items,
/// as text for `generate_api` to tokenize.
fn expand(args: &Args) -> syn::Result<String> {
    let raw = args.spec.value();
    let consumer = Consumer::locate(
        std::env::var_os("CARGO_MANIFEST_PATH"),
        std::env::var_os("CARGO_MANIFEST_DIR"),
    )
    .map_err(|message| syn::Error::new(args.spec.span(), message))?;
    let config = spec_for(args, &consumer)?;

    // Keep spargen's codegen (and the `tracking` tokens below) off the compiler bridge; restored
    // on drop.
    let _fallback = FallbackGuard::force();
    let preview = spargen::__private::preview_for_macro(&config, &consumer.manifest);

    if let Some(message) = failure_message(&raw, &preview) {
        return Err(syn::Error::new(args.spec.span(), message));
    }

    let source = preview.contents.as_ref().expect("checked above");
    // The newline ends a line comment the module may close on, so it cannot swallow the items.
    Ok(format!("{source}\n{}", tracking(&preview)))
}

/// The `Spec` `expand` generates from: the spec path resolved against the consumer, the
/// `spargen.toml` beside it applied, then the macro arguments over that — the same precedence the
/// CLI and `build.rs` use. A config file that cannot be read or parsed is an error at the spec
/// literal.
fn spec_for(args: &Args, consumer: &Consumer) -> syn::Result<spargen::Spec> {
    let spec_path = consumer.resolve_spec_path(&args.spec.value());
    let discovered = match spargen::Spec::new(spec_path).discover_config_file() {
        Ok(spec) => spec,
        Err(error) => {
            // `Display` leaves a read failure's I/O error to `source`, so the chain is written
            // out here, as the CLI writes it.
            let mut message = error.to_string();
            let mut source = std::error::Error::source(&error);
            while let Some(cause) = source {
                message.push_str(&format!(": {cause}"));
                source = cause.source();
            }
            return Err(syn::Error::new(args.spec.span(), message));
        }
    };
    Ok(config(args, discovered))
}

/// The `compile_error!` text for a preview that produced no client, or `None` when it produced
/// one. `raw` is the spec path as the macro was given it. Each error-severity diagnostic follows
/// the heading in `Diagnostic`'s own `Display`, the rendering the CLI and `Report` share; warnings
/// are left out.
fn failure_message(raw: &str, preview: &spargen::__private::MacroPreview) -> Option<String> {
    if preview.report.outcome() == spargen::Outcome::Generated && preview.contents.is_some() {
        return None;
    }
    let errors: Vec<&spargen::Diagnostic> = preview.report.errors().collect();
    let mut message = format!("spargen could not generate a client from `{raw}`");
    if errors.is_empty() {
        message.push_str(": generation did not succeed");
    } else {
        for diagnostic in &errors {
            message.push_str(&format!("\n{diagnostic}"));
        }
    }
    Some(message)
}

/// One `const _: &[u8] = include_bytes!(..);` item per file in `preview.source_files`, in that
/// order, which is what makes Cargo re-expand the macro when one of them changes (proc-macros
/// cannot emit `rerun-if-changed`). Every path exists: generation already read it.
fn tracking(preview: &spargen::__private::MacroPreview) -> proc_macro2::TokenStream {
    let tracks = preview
        .source_files
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    quote! {
        #(const _: &[u8] = include_bytes!(#tracks);)*
    }
}

/// Apply the macro arguments over `discovered`, the `Spec` the config file beside the spec yields.
/// A switch overrides the file in whichever direction it was given; an argument the macro was not
/// given leaves the config file's value in place, and omit rules and header bindings are appended
/// to the file's own.
fn config(args: &Args, discovered: spargen::Spec) -> spargen::Spec {
    let mut config = discovered;
    if let Some(uuid) = args.uuid {
        config = config.with_uuid(uuid);
    }
    if let Some(backend) = args.datetime {
        config = config.with_datetime(backend);
    }
    if let Some(carve) = args.carve {
        config = config.with_carve(carve);
    }
    if let Some(narrowing) = args.open_narrowing {
        config = config.with_open_narrowing(narrowing);
    }
    if let Some(tags) = args.open_unions {
        config = config.with_open_unions(tags);
    }
    if let Some(docs) = args.verbatim_docs {
        config = config.with_verbatim_docs(docs);
    }
    if let Some(testing) = args.testing {
        config = config.with_testing(testing);
    }
    for rule in &args.omit.rules {
        config = config.omit_rule(rule.clone());
    }
    for (method, path) in &args.keep {
        config = config.keep_operation(*method, path.clone());
    }
    for name in &args.bind_headers {
        config = config.bind_header(name.clone());
    }
    if let Some(cap) = args.error_body_cap {
        config = config.with_error_body_cap(cap);
    }
    if let Some(cap) = args.batch_cap {
        config = config.with_batch_cap(cap);
    }
    config
}

/// Parse `datetime = "<backend>"` into `slot`: the date-time backend by the spelling
/// `spargen.toml` and the CLI take. An unknown spelling, or the argument given twice, is an error
/// at the argument.
fn parse_datetime(
    input: ParseStream,
    key: &Ident,
    slot: &mut Option<spargen::DateTimeBackend>,
) -> syn::Result<()> {
    input.parse::<Token![=]>()?;
    let value: LitStr = input.parse()?;
    let backend = value
        .value()
        .parse()
        .map_err(|error: spargen::UnknownDateTimeBackend| syn::Error::new(value.span(), error))?;
    if slot.is_some() {
        return Err(syn::Error::new(
            key.span(),
            format!("`{key}` given more than once"),
        ));
    }
    *slot = Some(backend);
    Ok(())
}

/// Parse the cap `key` (`= <integer>`) into `slot`. A cap set twice is an error rather than a
/// silent last-wins, as a repeated switch is.
fn parse_cap(input: ParseStream, key: &Ident, slot: &mut Option<usize>) -> syn::Result<()> {
    input.parse::<Token![=]>()?;
    let value: usize = input.parse::<LitInt>()?.base10_parse()?;
    if slot.is_some() {
        return Err(syn::Error::new(
            key.span(),
            format!("`{key}` given more than once"),
        ));
    }
    *slot = Some(value);
    Ok(())
}

fn parse_omit(input: ParseStream, omit: &mut spargen::Omit) -> syn::Result<()> {
    let body;
    braced!(body in input);
    while !body.is_empty() {
        let section: Ident = body.parse()?;
        match section.to_string().as_str() {
            "operations" => parse_operations(&body, omit)?,
            "paths" => parse_paths(&body, omit)?,
            "components" => parse_components(&body, omit)?,
            "pointers" => parse_pointers(&body, omit, None)?,
            "file" => {
                let argument;
                parenthesized!(argument in body);
                let file: LitStr = argument.parse()?;
                let file_body;
                braced!(file_body in body);
                let pointers: Ident = file_body.parse()?;
                if pointers != "pointers" {
                    return Err(syn::Error::new(pointers.span(), "expected `pointers`"));
                }
                parse_pointers(&file_body, omit, Some(file.value().into()))?;
            }
            other => {
                return Err(syn::Error::new(
                    section.span(),
                    format!("unknown omit section `{other}`"),
                ));
            }
        }
    }
    Ok(())
}

fn parse_operations(input: ParseStream, omit: &mut spargen::Omit) -> syn::Result<()> {
    let body;
    braced!(body in input);
    for (method, path) in parse_method_paths(&body)? {
        omit.rules.push(spargen::OmitRule::Operation {
            method,
            path: path.into(),
        });
    }
    Ok(())
}

/// Every `method "/path";` entry of a block's body, as an `operations` or `keep` block writes them.
fn parse_method_paths(body: ParseStream) -> syn::Result<Vec<(spargen::OmitMethod, String)>> {
    let mut entries = Vec::new();
    while !body.is_empty() {
        let method: Ident = body.parse()?;
        // Delegated to `OmitMethod`'s own `FromStr` rather than matched here, so the macro cannot
        // fall behind the method set the generator supports.
        let method = method
            .to_string()
            .parse::<spargen::OmitMethod>()
            .map_err(|error| syn::Error::new(method.span(), error))?;
        let path: LitStr = body.parse()?;
        body.parse::<Token![;]>()?;
        entries.push((method, path.value()));
    }
    Ok(entries)
}

fn parse_paths(input: ParseStream, omit: &mut spargen::Omit) -> syn::Result<()> {
    let body;
    braced!(body in input);
    while !body.is_empty() {
        let path: LitStr = body.parse()?;
        body.parse::<Token![;]>()?;
        omit.rules.push(spargen::OmitRule::Path {
            path: path.value().into(),
        });
    }
    Ok(())
}

fn parse_components(input: ParseStream, omit: &mut spargen::Omit) -> syn::Result<()> {
    let body;
    braced!(body in input);
    while !body.is_empty() {
        let kind: Ident = body.parse()?;
        // `ComponentKind`'s own `FromStr`, shared with the CLI and `spargen.toml`, so all three
        // accept the same spellings and answer an unknown one with the same list.
        let kind = kind
            .to_string()
            .parse::<spargen::ComponentKind>()
            .map_err(|error| syn::Error::new(kind.span(), error))?;
        let names;
        braced!(names in body);
        while !names.is_empty() {
            let name: LitStr = names.parse()?;
            names.parse::<Token![;]>()?;
            omit.rules
                .push(spargen::OmitRule::component(kind, name.value()));
        }
    }
    Ok(())
}

fn parse_pointers(
    input: ParseStream,
    omit: &mut spargen::Omit,
    file: Option<std::borrow::Cow<'static, str>>,
) -> syn::Result<()> {
    let body;
    braced!(body in input);
    while !body.is_empty() {
        let pointer: LitStr = body.parse()?;
        body.parse::<Token![;]>()?;
        omit.rules.push(spargen::OmitRule::Pointer {
            file: file.clone(),
            pointer: pointer.value().into(),
        });
    }
    Ok(())
}

/// The crate `generate_api!` is expanding for, as the build driver names it.
///
/// Both the spec path and the manifest the runtime-dependency audit (`E023`) reads come from here,
/// and neither ever falls back to the working directory. Under a Cargo workspace build the working
/// directory is the **workspace root**, whose `Cargo.toml` is usually virtual: auditing it reports
/// every runtime dependency missing at spargen's floor version, a confident, specific, wrong
/// diagnostic about a file the consumer never pointed at. Not knowing the crate is an error.
#[derive(Debug, PartialEq, Eq)]
struct Consumer {
    /// The directory a relative spec path resolves against.
    dir: String,
    /// The manifest the audit reads.
    manifest: String,
}

impl Consumer {
    /// Locate the consumer from `CARGO_MANIFEST_PATH` and `CARGO_MANIFEST_DIR`, passed in rather
    /// than read here so the decision is testable without mutating the process environment.
    ///
    /// `CARGO_MANIFEST_PATH` names the manifest when set; otherwise it is `Cargo.toml` inside
    /// `CARGO_MANIFEST_DIR`. The spec directory is `CARGO_MANIFEST_DIR`, or the manifest's parent
    /// when only the path is set. An empty value is treated as unset, since joining onto it would
    /// reproduce the working-directory guess. Paths must be UTF-8: a lossy conversion would name
    /// a different file.
    fn locate(
        manifest_path: Option<std::ffi::OsString>,
        manifest_dir: Option<std::ffi::OsString>,
    ) -> Result<Self, String> {
        let utf8 =
            |name: &str, value: Option<std::ffi::OsString>| -> Result<Option<String>, String> {
                match value {
                    None => Ok(None),
                    Some(value) if value.is_empty() => Ok(None),
                    Some(value) => value.into_string().map(Some).map_err(|value| {
                        format!(
                        "spargen cannot locate the consuming crate: `{name}` is not valid UTF-8 \
                         ({})",
                        std::path::Path::new(&value).display()
                    )
                    }),
                }
            };
        let manifest_path = utf8("CARGO_MANIFEST_PATH", manifest_path)?;
        let manifest_dir = utf8("CARGO_MANIFEST_DIR", manifest_dir)?;
        match (manifest_path, manifest_dir) {
            (Some(manifest), Some(dir)) => Ok(Consumer { dir, manifest }),
            (Some(manifest), None) => {
                let dir = match std::path::Path::new(&manifest).parent() {
                    Some(parent) if !parent.as_os_str().is_empty() => {
                        parent.to_string_lossy().into_owned()
                    }
                    _ => ".".to_owned(),
                };
                Ok(Consumer { dir, manifest })
            }
            (None, Some(dir)) => {
                let manifest = std::path::Path::new(&dir)
                    .join("Cargo.toml")
                    .to_string_lossy()
                    .into_owned();
                Ok(Consumer { dir, manifest })
            }
            (None, None) => Err("spargen cannot locate the consuming crate: neither \
                 `CARGO_MANIFEST_PATH` nor `CARGO_MANIFEST_DIR` is set, so there is no \
                 `Cargo.toml` to resolve the spec against or to audit runtime dependencies in. \
                 Expand `generate_api!` under Cargo, or have the build driver set \
                 `CARGO_MANIFEST_DIR` to the consuming crate's directory."
                .to_owned()),
        }
    }

    /// Resolve a spec path relative to the consumer crate's directory (as `build.rs` does from
    /// that crate root), so `generate_api!("openapi.yaml")` finds a spec beside the caller's
    /// `Cargo.toml`. Absolute paths pass through unchanged.
    fn resolve_spec_path(&self, raw: &str) -> String {
        let path = std::path::Path::new(raw);
        if path.is_absolute() {
            return raw.to_owned();
        }
        std::path::Path::new(&self.dir)
            .join(path)
            .to_string_lossy()
            .into_owned()
    }
}

#[cfg(test)]
mod tests;
