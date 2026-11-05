//! # Subsystem: cli
//! layer-deps: facade
//!
//! The optional analysis binary. It is not a `mod` of the library: `src/bin/spargen.rs` pulls these
//! files in with `#[path]`, so the CLI reaches only spargen's `pub` items, as any consumer does.

use std::process::ExitCode;

use spargen::{check, explain, ConfigError, Report, Spec};

use super::args::{Cli, Command, Format};
use super::config;
use super::exit::ExitStatus;

/// Execute a parsed CLI invocation and return the process exit code.
///
/// Delegates to the crate facade, renders diagnostics in the requested [`Format`],
/// and maps the outcome onto the [`ExitStatus`] contract.
pub(crate) fn run(cli: Cli) -> ExitCode {
    match cli.command {
        Command::Check(args) => with_spec(args.spec, &args.options, |spec| {
            let report = check(&spec);
            emit(&report, args.format, Stream::Stderr);
            if report.succeeded() {
                ExitStatus::Ok.into()
            } else {
                ExitStatus::Diagnostics.into()
            }
        }),
        Command::Deps(args) => with_spec(args.spec, &args.options, |spec| {
            let requirements = spargen::requirements(&spec).map(|(requirements, report)| {
                emit_aside(&report, !report.diagnostics().is_empty(), args.format);
                requirements
            });
            report_or(requirements, args.format)
        }),
        Command::Lock(args) => {
            warn_ignored(&args.options.flags_lock_ignores(), args.format);
            with_spec(args.spec, &args.options, |spec| {
                report_or(spargen::vendor(&spec), args.format)
            })
        }
        Command::Diff(args) => {
            let (old, new) = match spec_pair(args.old, args.new, &args.options) {
                Ok(pair) => pair,
                Err(error) => return config_error(error),
            };
            match spargen::diff(&old, &new) {
                // A spec that fails to lower is a hard error regardless of `--exit-code`;
                // a breaking diff fails only when the caller opted into the CI gate.
                Err(rejection) => {
                    emit(&rejection, args.format, Stream::Stderr);
                    ExitStatus::Diagnostics.into()
                }
                Ok((report, sides)) => {
                    let warned = sides
                        .reports()
                        .any(|(_, side)| !side.diagnostics().is_empty());
                    emit_aside(&sides, warned, args.format);
                    let breaking = report.bump == spargen::Impact::Major;
                    emit(&report, args.format, Stream::Stdout);
                    if args.exit_code && breaking {
                        ExitStatus::Diagnostics.into()
                    } else {
                        ExitStatus::Ok.into()
                    }
                }
            }
        }
        Command::Explain(args) => match explain(&args.code) {
            Ok(text) => {
                match args.format {
                    Format::Human => println!("{text}"),
                    Format::Json => println!(
                        "{}",
                        serde_json::json!({ "code": args.code, "explain": text })
                    ),
                }
                ExitStatus::Ok.into()
            }
            Err(error) => {
                eprintln!("error: {error}");
                ExitStatus::Usage.into()
            }
        },
    }
}

/// Resolve `spec` with `options` and run `then` over it, or report the config/flag error and exit
/// with a usage status.
fn with_spec(
    spec: camino::Utf8PathBuf,
    options: &super::args::SpecArgs,
    then: impl FnOnce(Spec) -> ExitCode,
) -> ExitCode {
    match config::resolve(spec, options) {
        Ok(spec) => then(spec),
        Err(error) => config_error(error),
    }
}

/// Emit `result`'s value and exit cleanly, or emit its rejection [`Report`] and exit with the
/// diagnostics status. Human output puts the value on stdout and the rejection on stderr.
fn report_or<T: std::fmt::Display + serde::Serialize>(
    result: Result<T, Report>,
    format: Format,
) -> ExitCode {
    match result {
        Ok(value) => {
            emit(&value, format, Stream::Stdout);
            ExitStatus::Ok.into()
        }
        Err(report) => {
            emit(&report, format, Stream::Stderr);
            ExitStatus::Diagnostics.into()
        }
    }
}

/// Resolve both sides of a diff. The shared options apply to each side, but config-file discovery
/// is per spec — the two documents may well live in different directories.
fn spec_pair(
    old: camino::Utf8PathBuf,
    new: camino::Utf8PathBuf,
    options: &super::args::SpecArgs,
) -> Result<(Spec, Spec), ConfigError> {
    Ok((
        config::resolve(old, options)?,
        config::resolve(new, options)?,
    ))
}

/// Warn on stderr that `spargen lock` ignores `flags`, when any was given. Human output is one
/// `warning:` line; JSON output is one object (`warning` and the `ignored` flags), so stderr stays
/// parseable as JSON, as every report beside a JSON run is. The run itself is unaffected.
fn warn_ignored(flags: &[&str], format: Format) {
    if flags.is_empty() {
        return;
    }
    let warning = format!(
        "`spargen lock` ignores {}: it reads only --config and --batch-cap, and fetches every \
         remote $ref the spec reaches",
        flags.join(", ")
    );
    match format {
        Format::Human => eprintln!("warning: {warning}"),
        Format::Json => eprintln!(
            "{}",
            serde_json::json!({ "warning": warning, "ignored": flags })
        ),
    }
}

/// Render a config/flag error to stderr and exit with a usage status — never a panic.
fn config_error(error: ConfigError) -> ExitCode {
    eprintln!("error: {error}");
    ExitStatus::Usage.into()
}

/// Where a value goes. `Stdout` and `Stderr` choose the stream of the *human* rendering of a
/// run's one document (its result on stdout, its rejection on stderr); the JSON rendering of that
/// document ignores them and goes to stdout. `Aside` is a report beside that document, the
/// warnings of a run that succeeded, and goes to stderr in both formats, so stdout keeps exactly
/// the one document a script parses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stream {
    Stdout,
    Stderr,
    Aside,
}

/// Emit `value`, the warnings of a run that succeeded, to [`Stream::Aside`] when `warned` says it
/// carries a diagnostic; a run with none emits nothing beside its one document.
fn emit_aside<T: std::fmt::Display + serde::Serialize>(value: &T, warned: bool, format: Format) {
    if warned {
        emit(value, format, Stream::Aside);
    }
}

/// Render one value in the requested format.
///
/// Every renderable type implements both `Display` (the human form) and `Serialize` (the JSON
/// form), so every command but `explain` renders through the library's own forms. `explain`
/// prints its text, or builds its `{ "code", "explain" }` JSON object, inline in [`run`].
/// [`Stream`] says where each rendering goes.
fn emit<T: std::fmt::Display + serde::Serialize>(value: &T, format: Format, stream: Stream) {
    match format {
        Format::Json => {
            let rendered = serde_json::to_string(value).unwrap_or_else(|error| {
                format!("{{\"error\":\"failed to render JSON: {error}\"}}")
            });
            match stream {
                Stream::Stdout | Stream::Stderr => println!("{rendered}"),
                Stream::Aside => eprintln!("{rendered}"),
            }
        }
        Format::Human => match stream {
            Stream::Stdout => println!("{value}"),
            Stream::Stderr | Stream::Aside => eprintln!("{value}"),
        },
    }
}
