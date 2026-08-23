use std::process::ExitCode;

use spargen::{check, explain, Config};

use super::args::{Cli, Command, Format};
use super::config::{self, CliOverrides, ConfigError, OmitFlags, Settings};
use super::exit::ExitStatus;

/// Execute a parsed CLI invocation and return the process exit code.
///
/// Delegates to the crate facade, renders diagnostics in the requested [`Format`](super::Format),
/// and maps the outcome onto the [`ExitStatus`](super::ExitStatus) contract. Per the DAG, the CLI
/// depends only on the facade.
pub(crate) fn run(cli: Cli) -> ExitCode {
    match cli.command {
        Command::Check(args) => {
            let flags = OmitFlags {
                paths: args.omit_path,
                operations: args.omit_operation,
                components: args.omit_component,
                pointers: args.omit_pointer,
            };
            let overrides = CliOverrides {
                carve: args.carve.then_some(true),
            };
            let settings =
                match config::resolve(&args.spec, args.config.as_deref(), &overrides, &flags) {
                    Ok(settings) => settings,
                    Err(error) => return config_error(error),
                };
            let mut config = Config::new(args.spec, "__spargen_check.rs");
            apply_settings(&mut config, settings);
            let report = check(&config);
            emit(&report, args.format, Stream::Stderr);
            if report.succeeded() {
                ExitStatus::Ok.into()
            } else {
                ExitStatus::Diagnostics.into()
            }
        }
        Command::Lock(args) => {
            let config = Config::new(args.spec, "__spargen_lock.rs");
            let outcome = spargen::vendor(&config);
            emit(&outcome, args.format, Stream::Stdout);
            if outcome.succeeded() {
                ExitStatus::Ok.into()
            } else {
                ExitStatus::Diagnostics.into()
            }
        }
        Command::Diff(args) => {
            let old = Config::new(args.old, "__spargen_diff_old.rs");
            let new = Config::new(args.new, "__spargen_diff_new.rs");
            match spargen::diff(&old, &new) {
                // A spec that fails to lower is a hard error regardless of `--exit-code`;
                // a breaking diff fails only when the caller opted into the CI gate.
                Err(rejection) => {
                    emit(&rejection, args.format, Stream::Stderr);
                    ExitStatus::Diagnostics.into()
                }
                Ok(report) => {
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

/// Fold resolved [`Settings`] into the library [`Config`].
fn apply_settings(config: &mut Config, settings: Settings) {
    config.batch_cap = settings.batch_cap;
    config.omit = settings.omit;
    config.carve = settings.carve;
}

/// Render a config/flag error to stderr and exit with a usage status — never a panic.
fn config_error(error: ConfigError) -> ExitCode {
    eprintln!("error: {error}");
    ExitStatus::Usage.into()
}

/// Where a rendered value goes: diagnostics belong on stderr so `--format json` output stays
/// pipeable on stdout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stream {
    Stdout,
    Stderr,
}

/// Render one value in the requested format.
///
/// Every renderable type implements both `Display` (the human form) and `Serialize` (the JSON
/// form), so the CLI holds no formatting logic of its own and cannot drift from the library's.
///
/// `stream` chooses where the *human* rendering goes — diagnostics belong on stderr. JSON is
/// machine output and always goes to stdout so it stays pipeable.
fn emit<T: std::fmt::Display + serde::Serialize>(value: &T, format: Format, stream: Stream) {
    match format {
        Format::Json => {
            let rendered = serde_json::to_string(value).unwrap_or_else(|error| {
                format!("{{\"error\":\"failed to render JSON: {error}\"}}")
            });
            println!("{rendered}");
        }
        Format::Human => match stream {
            Stream::Stdout => println!("{value}"),
            Stream::Stderr => eprintln!("{value}"),
        },
    }
}
