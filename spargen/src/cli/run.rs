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

            // `--out -` streams the generated module to stdout (a preview) and writes nothing —
            // the Unix dash convention, so `spargen generate api.yaml --out - | rustfmt` works.
            // It is a single-module view, so it is incompatible with the multi-file / stateful
            // modes below.
            let preview_to_stdout = args.out.as_str() == "-";
            if preview_to_stdout {
                if settings.as_crate {
                    return usage_error(
                        "--out - previews a single module to stdout; it is not supported with \
                         --as-crate (a crate is multiple files). Write the crate to a directory \
                         instead.",
                    );
                }
                if args.check {
                    return usage_error("--out - (stdout preview) cannot be combined with --check");
                }
                if args.watch {
                    return usage_error("--out - (stdout preview) cannot be combined with --watch");
                }
            }

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

            if preview_to_stdout {
                // Render in memory; print the module to stdout and diagnostics to stderr, so the
                // piped code stays pure. `--format` governs diagnostics only, which for a preview
                // are advisory — keep them human-readable on stderr regardless.
                let preview = crate::preview(&config);
                render_diagnostics_human(&preview.report.diagnostics);
                if let Some(file) = preview.files.first() {
                    print!("{}", file.contents);
                }
                return status_for_report(&preview.report).into();
            }

            if args.watch {
                return super::watch::watch(&config, args.config.as_deref(), args.format);
            }
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
        Command::Diff(args) => {
            let old = Config::new(
                args.old,
                OutputTarget::Module("__spargen_diff_old.rs".into()),
            );
            let new = Config::new(
                args.new,
                OutputTarget::Module("__spargen_diff_new.rs".into()),
            );
            let outcome = crate::diff(&old, &new);
            render_diff(&outcome, args.format);
            // A spec that fails to lower is a hard error (status 1) regardless of `--exit-code`;
            // otherwise a breaking diff fails only when the caller opted into the CI gate.
            match &outcome.report {
                None => ExitStatus::Diagnostics.into(),
                Some(report) => {
                    if args.exit_code && report.bump == crate::Impact::Major {
                        ExitStatus::Diagnostics.into()
                    } else {
                        ExitStatus::Ok.into()
                    }
                }
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

/// Render a flag-combination error to stderr and exit with a usage status.
fn usage_error(message: &str) -> ExitCode {
    eprintln!("error: {message}");
    ExitStatus::Usage.into()
}

pub(super) fn status_for_report(report: &crate::Report) -> ExitStatus {
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

pub(super) fn render_report(report: &crate::Report, format: Format) {
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

/// Render a `spargen diff` outcome in the requested format. A spec that failed to lower is reported
/// as such (with its rejection diagnostics); otherwise the classified change list, the overall
/// recommended bump, and a one-line summary are printed.
fn render_diff(outcome: &crate::DiffOutcome, format: Format) {
    match format {
        Format::Human => {
            let mut rejected = false;
            if let Some(report) = &outcome.old_rejection {
                eprintln!("error: the OLD spec failed to lower; cannot diff:");
                render_diagnostics_human(&report.diagnostics);
                rejected = true;
            }
            if let Some(report) = &outcome.new_rejection {
                eprintln!("error: the NEW spec failed to lower; cannot diff:");
                render_diagnostics_human(&report.diagnostics);
                rejected = true;
            }
            if rejected {
                return;
            }
            if let Some(report) = &outcome.report {
                for change in &report.changes {
                    println!(
                        "{:>5} [{}] {}: {}",
                        change.impact.as_str(),
                        change.kind.code(),
                        change.location,
                        change.detail
                    );
                }
                println!("{}", report.summary());
                println!("recommended bump: {}", report.bump.as_str());
            }
        }
        Format::Json => {
            println!("{}", serde_json::to_string(&diff_json(outcome)).unwrap());
        }
    }
}

fn diff_json(outcome: &crate::DiffOutcome) -> serde_json::Value {
    match &outcome.report {
        Some(report) => serde_json::json!({
            "ok": true,
            "bump": report.bump.as_str(),
            "summary": report.summary(),
            "changes": report.changes.iter().map(|change| {
                serde_json::json!({
                    "impact": change.impact.as_str(),
                    "code": change.kind.code(),
                    "location": change.location,
                    "detail": change.detail,
                })
            }).collect::<Vec<_>>(),
        }),
        None => serde_json::json!({
            "ok": false,
            "old_rejected": outcome.old_rejection.is_some(),
            "new_rejected": outcome.new_rejection.is_some(),
            "old_diagnostics": outcome
                .old_rejection
                .as_ref()
                .map(|report| diagnostics_json(&report.diagnostics))
                .unwrap_or_default(),
            "new_diagnostics": outcome
                .new_rejection
                .as_ref()
                .map(|report| diagnostics_json(&report.diagnostics))
                .unwrap_or_default(),
        }),
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
