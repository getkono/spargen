use std::process::ExitCode;

use crate::{check, explain, generate, Config, Outcome, OutputTarget};

use super::config::{self, CliOverrides, ConfigError, OmitFlags, Settings};
use super::{Cli, Command, ExitStatus, Format};

/// Execute a parsed CLI invocation and return the process exit code.
///
/// Delegates to the crate facade — [`generate`](crate::generate), [`check`](crate::check),
/// [`explain`](crate::explain) — renders diagnostics in the requested [`Format`](super::Format),
/// and maps the outcome onto the [`ExitStatus`](super::ExitStatus) contract. Per the DAG, the CLI
/// depends only on the facade.
pub fn run(cli: Cli) -> ExitCode {
    match cli.command {
        Command::Generate(args) => {
            let overrides = CliOverrides {
                // `--no-uuid`/`--no-time`/`--as-crate` are presence flags: set only when given, so
                // they override the config file, and stay `None` (config/default wins) otherwise.
                uuid: args.no_uuid.then_some(false),
                time: args.no_time.then_some(false),
                as_crate: args.as_crate.then_some(true),
                carve: args.carve.then_some(true),
            };
            let flags = OmitFlags {
                paths: args.omit_path,
                operations: args.omit_operation,
                components: args.omit_component,
                pointers: args.omit_pointer,
            };
            let settings =
                match config::resolve(&args.spec, args.config.as_deref(), &overrides, &flags) {
                    Ok(settings) => settings,
                    Err(error) => return config_error(error),
                };

            let output = if settings.as_crate {
                let name = args
                    .out
                    .file_name()
                    .map(str::to_owned)
                    .unwrap_or_else(|| "generated-api".to_owned());
                OutputTarget::Crate {
                    dir: args.out.clone(),
                    name,
                }
            } else {
                OutputTarget::Module(args.out.clone())
            };
            let mut config = Config::new(args.spec, output);
            apply_settings(&mut config, settings);
            config.check_only = args.check;
            let report = generate(&config);
            render_report(&report, args.format);
            status_for_report(&report).into()
        }
        Command::Check(args) => {
            let flags = OmitFlags {
                paths: args.omit_path,
                operations: args.omit_operation,
                components: args.omit_component,
                pointers: args.omit_pointer,
            };
            let overrides = CliOverrides {
                carve: args.carve.then_some(true),
                ..CliOverrides::default()
            };
            let settings =
                match config::resolve(&args.spec, args.config.as_deref(), &overrides, &flags) {
                    Ok(settings) => settings,
                    Err(error) => return config_error(error),
                };
            let mut config =
                Config::new(args.spec, OutputTarget::Module("__spargen_check.rs".into()));
            apply_settings(&mut config, settings);
            let report = check(&config);
            render_report(&report, args.format);
            status_for_report(&report).into()
        }
        Command::Lock(args) => {
            let config = Config::new(args.spec, OutputTarget::Module("__spargen_lock.rs".into()));
            let outcome = crate::vendor(&config);
            let has_errors = outcome
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.severity == crate::Severity::Error);
            match args.format {
                Format::Human => {
                    render_diagnostics_human(&outcome.diagnostics);
                    if let Some(report) = &outcome.report {
                        if report.refs.is_empty() {
                            println!("no remote $refs found; wrote {}", report.lock_path);
                        } else {
                            println!(
                                "vendored {} remote document(s) under {}:",
                                report.refs.len(),
                                report.vendor_dir
                            );
                            for vendored in &report.refs {
                                println!("  {} -> {}", vendored.url, vendored.path);
                            }
                            println!("wrote {}", report.lock_path);
                        }
                    }
                }
                Format::Json => {
                    println!(
                        "{}",
                        serde_json::json!({
                            "lock": outcome.report.as_ref().map(|report| report.lock_path.to_string()),
                            "vendor_dir": outcome.report.as_ref().map(|report| report.vendor_dir.to_string()),
                            "vendored": outcome.report.as_ref().map(|report| {
                                report.refs.iter().map(|vendored| {
                                    serde_json::json!({
                                        "url": vendored.url,
                                        "path": vendored.path,
                                        "sha256": vendored.sha256,
                                    })
                                }).collect::<Vec<_>>()
                            }).unwrap_or_default(),
                            "diagnostics": diagnostics_json(&outcome.diagnostics),
                        })
                    );
                }
            }
            if outcome.report.is_none() || has_errors {
                ExitStatus::Diagnostics.into()
            } else {
                ExitStatus::Ok.into()
            }
        }
        Command::Explain(args) => match explain(&args.code) {
            Some(text) => {
                match args.format {
                    Format::Human => println!("{text}"),
                    Format::Json => {
                        println!(
                            "{}",
                            serde_json::json!({
                                "code": args.code,
                                "explain": text,
                            })
                        );
                    }
                }
                ExitStatus::Ok.into()
            }
            None => {
                eprintln!("unknown diagnostic code: {}", args.code);
                ExitStatus::Usage.into()
            }
        },
    }
}

/// Fold resolved [`Settings`] into the library [`Config`]. The `Config` API itself is unchanged;
/// this is the CLI's config-file + omit-flag plumbing.
fn apply_settings(config: &mut Config, settings: Settings) {
    config.features.uuid = settings.uuid;
    config.features.time = settings.time;
    config.error_body_cap = settings.error_body_cap;
    config.batch_cap = settings.batch_cap;
    config.omit = settings.omit;
    config.carve = settings.carve;
}

/// Render a config/flag error to stderr and exit with a usage status — never a panic.
fn config_error(error: ConfigError) -> ExitCode {
    eprintln!("error: {error}");
    ExitStatus::Usage.into()
}

fn status_for_report(report: &crate::Report) -> ExitStatus {
    match report.outcome {
        Outcome::Generated | Outcome::Clean => {
            if report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.severity == crate::Severity::Error)
            {
                ExitStatus::Diagnostics
            } else {
                ExitStatus::Ok
            }
        }
        Outcome::Drifted => ExitStatus::Drift,
        Outcome::Rejected => ExitStatus::Diagnostics,
    }
}

fn render_report(report: &crate::Report, format: Format) {
    match format {
        Format::Human => render_diagnostics_human(&report.diagnostics),
        Format::Json => {
            println!(
                "{}",
                serde_json::json!({
                    "outcome": format!("{:?}", report.outcome),
                    "diagnostics": diagnostics_json(&report.diagnostics),
                })
            );
        }
    }
}

/// Render diagnostics to stderr in the rustc-style human format (also used by `spargen lock`).
fn render_diagnostics_human(diagnostics: &[crate::Diagnostic]) {
    for diagnostic in diagnostics {
        let severity = match diagnostic.severity {
            crate::Severity::Error => "error",
            crate::Severity::Warning => "warning",
        };
        eprintln!(
            "{severity}[{}]: {}\n  pointer: {}",
            diagnostic.code, diagnostic.message, diagnostic.pointer
        );
        if let Some(remedy) = &diagnostic.remedy {
            eprintln!("  help: {remedy}");
        }
    }
}

fn diagnostics_json(diagnostics: &[crate::Diagnostic]) -> Vec<serde_json::Value> {
    diagnostics
        .iter()
        .map(|diagnostic| {
            serde_json::json!({
                "code": diagnostic.code.as_str(),
                "severity": diagnostic.severity,
                "pointer": diagnostic.pointer,
                "span": diagnostic.span,
                "message": diagnostic.message,
                "remedy": diagnostic.remedy,
            })
        })
        .collect()
}
