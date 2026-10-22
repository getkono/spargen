//! What the published crate and its test targets carry: every file a source includes ships in
//! the package, and every test that spawns the `cli`-gated binary is itself gated on `cli`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::{read, rust_files};

/// The path literal of every `include_str!` and `include_bytes!` in `source`: every `include_str!`
/// target in source order, then every `include_bytes!` target in source order. Lines that
/// open with `//` are skipped, so prose citing the macro contributes nothing; whitespace between
/// the parenthesis and the literal is allowed, since rustfmt breaks a long call there.
fn include_targets(source: &str) -> Vec<String> {
    let code: String = source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut targets = Vec::new();
    for marker in ["include_str!(", "include_bytes!("] {
        for (at, _) in code.match_indices(marker) {
            let Some(literal) = code[at + marker.len()..].trim_start().strip_prefix('"') else {
                continue;
            };
            let path = literal.split('"').next().expect("a closing quote");
            targets.push(path.to_owned());
        }
    }
    targets
}

#[test]
fn the_include_scanner_reads_path_literals_and_skips_comments() {
    let source = r#"
//! Embedded with `include_str!("prose.txt")`, which this line only cites.
const A: &str = include_str!("a.txt");
const B: &[u8] = include_bytes!("../b.bin");
const C: (&str, &str) = (
    "c",
    include_str!(
        "sub/c.rs"
    ),
);
    // include_str!("commented.txt")
const D: &str = include_str!(concat!("not", "a", "literal"));
"#;
    assert_eq!(include_targets(source), ["a.txt", "sub/c.rs", "../b.bin"]);
}

/// Every file an `include_str!` or `include_bytes!` in `spargen/src` names is in the package
/// `cargo publish` uploads.
///
/// `cargo publish --dry-run` builds the packaged crate, so an include the library compiles is
/// already held there. An include under `#[cfg(test)]` is not: the library builds without its
/// target, so an `exclude` in `Cargo.toml` or a `.gitignore` pattern could drop the file from the
/// `.crate` and every packaging gate stays green, while `cargo test` on the published crate no
/// longer compiles. `src/runtime_contract_e023_explain.txt`, the second copy of `E023`'s explain
/// body, is such a target (#203). The rule is stated over every include rather than over that file,
/// so a new test fixture read the same way is held without anyone remembering to list it.
#[test]
fn every_included_file_ships_in_the_published_crate() {
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let src = crate_dir.join("src");

    let sources = rust_files(&src, &[]);

    // Resolved lexically against the including file's directory and never canonicalized: the
    // `src/support/runtime/` entries are symlinks, and the package lists them at their own paths.
    let mut included = BTreeMap::new();
    for source in &sources {
        let dir = source
            .parent()
            .expect("a source file has a directory")
            .strip_prefix(crate_dir)
            .expect("sources live under the crate directory");
        for target in include_targets(&read(source)) {
            let mut resolved = dir.to_path_buf();
            for component in Path::new(&target).components() {
                match component {
                    std::path::Component::ParentDir => {
                        assert!(
                            resolved.pop(),
                            "{source:?} includes {target:?} above the crate"
                        );
                    }
                    std::path::Component::CurDir => {}
                    other => resolved.push(other),
                }
            }
            let resolved = resolved
                .to_str()
                .expect("UTF-8 path")
                .replace(std::path::MAIN_SEPARATOR, "/");
            let site = source
                .strip_prefix(crate_dir)
                .expect("under the crate directory");
            included.insert(resolved, site.to_path_buf());
        }
    }
    assert!(
        !included.is_empty(),
        "no include found under {src:?}: the scanner has stopped reading the sources"
    );

    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = std::process::Command::new(cargo)
        .args([
            "package",
            "--list",
            "--allow-dirty",
            "--offline",
            "-p",
            "spargen",
        ])
        .current_dir(crate_dir)
        .output()
        .expect("cargo runs");
    assert!(
        output.status.success(),
        "`cargo package --list` failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let packaged: BTreeSet<String> = String::from_utf8(output.stdout)
        .expect("UTF-8 file list")
        .lines()
        .map(str::to_owned)
        .collect();

    let missing: Vec<String> = included
        .iter()
        .filter(|(path, _)| !packaged.contains(*path))
        .map(|(path, source)| format!("{path} (included by {})", source.display()))
        .collect();
    assert!(
        missing.is_empty(),
        "these files are included by spargen's sources but absent from `cargo package --list`, so \
         the published crate does not carry them: {missing:#?}"
    );
}

/// The compile-time variable cargo sets to the `spargen` binary's path for integration tests.
const SPAWN_VARIABLE: &str = "CARGO_BIN_EXE_spargen";

/// The one attribute accepted as a `cli` gate, with its whitespace removed.
const CLI_GATE: &str = "#[cfg(feature=\"cli\")]";

/// A source file with every comment and every literal's contents blanked to spaces, byte for byte,
/// so an offset into `code` is the same offset into the source.
struct Masked {
    code: Vec<u8>,
    /// Each string literal, as the offset its token starts at (the `r` of a raw string, otherwise
    /// the opening quote) and its contents, escapes left unprocessed.
    literals: Vec<(usize, String)>,
}

fn is_ident_byte(byte: u8) -> bool {
    byte == b'_' || byte.is_ascii_alphanumeric()
}

fn mask(source: &str) -> Masked {
    let bytes = source.as_bytes();
    let mut code = bytes.to_vec();
    let mut literals = Vec::new();
    let blank = |code: &mut Vec<u8>, from: usize, to: usize| {
        for byte in &mut code[from..to] {
            if *byte != b'\n' {
                *byte = b' ';
            }
        }
    };
    let mut at = 0;
    while at < bytes.len() {
        let rest = &bytes[at..];
        if rest.starts_with(b"//") {
            let end = rest
                .iter()
                .position(|&byte| byte == b'\n')
                .map_or(bytes.len(), |offset| at + offset);
            blank(&mut code, at, end);
            at = end;
        } else if rest.starts_with(b"/*") {
            let (mut end, mut depth) = (at + 2, 1);
            while end < bytes.len() && depth > 0 {
                if bytes[end..].starts_with(b"/*") {
                    depth += 1;
                    end += 2;
                } else if bytes[end..].starts_with(b"*/") {
                    depth -= 1;
                    end += 2;
                } else {
                    end += 1;
                }
            }
            blank(&mut code, at, end);
            at = end;
        } else if bytes[at] == b'r'
            && (at == 0
                || !is_ident_byte(bytes[at - 1])
                || (bytes[at - 1] == b'b' && (at < 2 || !is_ident_byte(bytes[at - 2]))))
            && rest[1..]
                .iter()
                .find(|&&byte| byte != b'#')
                .is_some_and(|&byte| byte == b'"')
        {
            let hashes = rest[1..].iter().take_while(|&&byte| byte == b'#').count();
            let open = at + 1 + hashes + 1;
            let mut closing = vec![b'"'];
            closing.extend(std::iter::repeat_n(b'#', hashes));
            let close = bytes[open..]
                .windows(closing.len())
                .position(|window| window == closing.as_slice())
                .map_or(bytes.len(), |offset| open + offset);
            literals.push((at, source[open..close].to_owned()));
            blank(&mut code, open, close);
            at = (close + closing.len()).min(bytes.len());
        } else if bytes[at] == b'"' {
            let mut close = at + 1;
            while close < bytes.len() && bytes[close] != b'"' {
                close += if bytes[close] == b'\\' { 2 } else { 1 };
            }
            let close = close.min(bytes.len());
            literals.push((at, source[at + 1..close].to_owned()));
            blank(&mut code, at + 1, close);
            at = close + 1;
        } else if bytes[at] == b'\'' {
            // A char literal is blanked; a lifetime, which has no closing quote, is left alone.
            if bytes.get(at + 1) == Some(&b'\\') {
                let from = (at + 3).min(bytes.len());
                let close = bytes[from..]
                    .iter()
                    .position(|&byte| byte == b'\'')
                    .map_or(bytes.len(), |offset| from + offset);
                blank(&mut code, at + 1, close);
                at = close + 1;
            } else {
                let width = source[at + 1..].chars().next().map_or(0, char::len_utf8);
                if width > 0 && bytes.get(at + 1 + width) == Some(&b'\'') {
                    blank(&mut code, at + 1, at + 1 + width);
                    at += width + 2;
                } else {
                    at += 1;
                }
            }
        } else {
            at += 1;
        }
    }
    Masked { code, literals }
}

/// The offset of every `env!("CARGO_BIN_EXE_spargen")` or `option_env!` of it in the code, raw
/// literals, whitespace inside the invocation, and each macro delimiter (`(`, `[`, `{`) included.
/// A comment or a string that only cites the invocation is blanked by [`mask`], so it names no
/// site.
fn spawn_sites(masked: &Masked) -> Vec<usize> {
    fn trim_end(code: &[u8]) -> &[u8] {
        let kept = code
            .iter()
            .rposition(|byte| !byte.is_ascii_whitespace())
            .map_or(0, |last| last + 1);
        &code[..kept]
    }
    masked
        .literals
        .iter()
        .filter(|(_, contents)| contents == SPAWN_VARIABLE)
        .filter_map(|&(at, _)| {
            let (&open, before) = trim_end(&masked.code[..at]).split_last()?;
            if !matches!(open, b'(' | b'[' | b'{') {
                return None;
            }
            let before = trim_end(before);
            let before = trim_end(before.strip_suffix(b"!")?);
            let name_start = before
                .iter()
                .rposition(|&byte| !is_ident_byte(byte))
                .map_or(0, |last| last + 1);
            matches!(&before[name_start..], b"env" | b"option_env").then_some(at)
        })
        .collect()
}

/// The span of each item at the top level of `code`, as delimited by a `;` or a closing `}` at
/// bracket depth zero. The file's inner attributes and comments ride at the front of the first.
fn top_level_items(code: &[u8]) -> Vec<(usize, usize)> {
    let (mut items, mut depth, mut start) = (Vec::new(), 0usize, 0);
    for (at, &byte) in code.iter().enumerate() {
        match byte {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' => depth = depth.saturating_sub(1),
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    items.push((start, at + 1));
                    start = at + 1;
                }
            }
            b';' if depth == 0 => {
                items.push((start, at + 1));
                start = at + 1;
            }
            _ => {}
        }
    }
    if start < code.len() {
        items.push((start, code.len()));
    }
    items
}

/// The attributes leading the item that starts at `start`, as `(inner, whitespace-free text)`.
fn leading_attributes(source: &str, code: &[u8], start: usize) -> Vec<(bool, String)> {
    let skip_whitespace = |mut at: usize| {
        while code.get(at).is_some_and(u8::is_ascii_whitespace) {
            at += 1;
        }
        at
    };
    let mut attributes = Vec::new();
    let mut at = skip_whitespace(start);
    while code.get(at) == Some(&b'#') {
        let mut open = skip_whitespace(at + 1);
        let inner = code.get(open) == Some(&b'!');
        if inner {
            open = skip_whitespace(open + 1);
        }
        if code.get(open) != Some(&b'[') {
            break;
        }
        let mut depth = 0usize;
        let mut end = open;
        while end < code.len() {
            match code[end] {
                b'[' => depth += 1,
                b']' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            end += 1;
        }
        let text: String = source[at..(end + 1).min(source.len())]
            .chars()
            .filter(|char| !char.is_whitespace())
            .collect();
        attributes.push((inner, text));
        at = skip_whitespace(end + 1);
    }
    attributes
}

/// The line of every spawn of the `spargen` binary in `source` that no `cli` gate covers.
///
/// A spawn is gated when the file carries `#![cfg(feature = "cli")]`, or when the top-level item
/// containing it carries `#[cfg(feature = "cli")]`. Only that literal attribute counts (whitespace
/// aside), and only on the top-level item: a gate on an item nested inside an ungated module, or a
/// predicate such as `all(feature = "cli", ...)`, is reported, so the rule is checkable without a
/// cfg evaluator. Gate the outer item instead.
fn ungated_spawns(source: &str) -> Vec<usize> {
    let masked = mask(source);
    let sites = spawn_sites(&masked);
    let items = top_level_items(&masked.code);
    let inner_gate = format!("#!{}", &CLI_GATE[1..]);
    let attributes: Vec<Vec<(bool, String)>> = items
        .iter()
        .map(|&(start, _)| leading_attributes(source, &masked.code, start))
        .collect();
    if attributes
        .iter()
        .flatten()
        .any(|(inner, text)| *inner && *text == inner_gate)
    {
        return Vec::new();
    }
    sites
        .into_iter()
        .filter(|&site| {
            let item = items
                .iter()
                .position(|&(start, end)| (start..end).contains(&site))
                .expect("every offset lies in some top-level span");
            !attributes[item]
                .iter()
                .any(|(inner, text)| !*inner && text == CLI_GATE)
        })
        .map(|site| source[..site].matches('\n').count() + 1)
        .collect()
}

/// The source path of every `[[test]]` target `manifest` declares with `cli` among its
/// `required-features`, relative to the crate directory.
fn cli_gated_targets(manifest: &str) -> BTreeSet<String> {
    let manifest: toml::Table = toml::from_str(manifest).expect("the manifest parses");
    let Some(toml::Value::Array(tests)) = manifest.get("test") else {
        return BTreeSet::new();
    };
    tests
        .iter()
        .filter_map(toml::Value::as_table)
        .filter(|target| {
            target
                .get("required-features")
                .and_then(toml::Value::as_array)
                .is_some_and(|features| {
                    features
                        .iter()
                        .any(|feature| feature.as_str() == Some("cli"))
                })
        })
        .map(
            |target| match target.get("path").and_then(toml::Value::as_str) {
                Some(path) => path.to_owned(),
                None => format!(
                    "tests/{}.rs",
                    target
                        .get("name")
                        .and_then(toml::Value::as_str)
                        .expect("a [[test]] target without a path has a name")
                ),
            },
        )
        .collect()
}

#[test]
fn the_spawn_gate_scanner_rejects_every_ungated_spawn() {
    let source = r##"
//! Spawns `env!("CARGO_BIN_EXE_spargen")`, which this line only cites.
#![cfg(feature = "remote-fetch")]
use std::process::Command;

/* A block comment citing env!("CARGO_BIN_EXE_spargen") /* nested */ names no site. */
#[cfg(feature = "cli")]
fn gated() -> Command {
    Command::new(env!("CARGO_BIN_EXE_spargen"))
}

const BRACES: &str = "}}{";
const CHAR: char = '}';

#[test]
#[cfg(feature = "remote-fetch")]
fn gated_on_another_feature<'a>() {
    let _ = Command::new(env ! ( "CARGO_BIN_EXE_spargen" ));
}

#[test]
fn ungated_raw() {
    let _ = option_env!(r#"CARGO_BIN_EXE_spargen"#);
}

fn cites_it() -> &'static str {
    "env!(\"CARGO_BIN_EXE_spargen\")"
}

mod nested {
    #[cfg(feature = "cli")]
    fn gated_below_an_ungated_module() {
        let _ = std::process::Command::new(core::env!("CARGO_BIN_EXE_spargen"));
    }
}

#[cfg(all(feature = "cli", unix))]
fn a_predicate_is_not_the_gate() {
    let _ = env!("CARGO_BIN_EXE_spargen");
}

#[test]
fn ungated_brackets() {
    let _ = env!["CARGO_BIN_EXE_spargen"];
    let _ = option_env! [ "CARGO_BIN_EXE_spargen" ];
}

#[test]
fn ungated_braces() {
    let _ = env! { "CARGO_BIN_EXE_spargen" };
}

const A_TUPLE: (&str,) = ("CARGO_BIN_EXE_spargen",);
const AN_ARRAY: [&str; 1] = ["CARGO_BIN_EXE_spargen"];
"##;
    assert_eq!(ungated_spawns(source), [18, 23, 33, 39, 44, 45, 50]);
}

#[test]
fn the_spawn_gate_scanner_accepts_a_file_or_item_gate() {
    let file_gated = r#"
//! Every test here spawns the binary.
#![cfg(feature = "cli")]
fn spawn() {
    let _ = env!("CARGO_BIN_EXE_spargen");
}
"#;
    assert_eq!(ungated_spawns(file_gated), Vec::<usize>::new());

    let item_gated = r#"
/// A documented, gated module.
#[cfg( feature = "cli" )]
mod spawning {
    fn spawn() {
        let _ = env!("CARGO_BIN_EXE_spargen");
    }
}

#[cfg(feature = "cli")]
#[test]
fn spawns() {
    let _ = env!("CARGO_BIN_EXE_spargen");
}

#[cfg(feature = "cli")]
fn spawns_through_other_delimiters() {
    let _ = env!["CARGO_BIN_EXE_spargen"];
    let _ = option_env! { "CARGO_BIN_EXE_spargen" };
}
"#;
    assert_eq!(ungated_spawns(item_gated), Vec::<usize>::new());
}

#[test]
fn the_target_gate_reader_reads_required_features() {
    let manifest = r#"
[package]
name = "example"

[[bin]]
name = "spargen"
required-features = ["cli"]

[[test]]
name = "cli"
required-features = ["cli"]

[[test]]
name = "elsewhere"
path = "tests/elsewhere/main.rs"
required-features = ["remote-fetch", "cli"]

[[test]]
name = "other"
required-features = ["remote-fetch"]
"#;
    assert_eq!(
        cli_gated_targets(manifest),
        BTreeSet::from([
            "tests/cli.rs".to_owned(),
            "tests/elsewhere/main.rs".to_owned()
        ])
    );
}

/// Every integration test that spawns the `spargen` binary compiles only under the `cli` feature.
///
/// `env!("CARGO_BIN_EXE_spargen")` expands to a path even when `cli` is off and the binary, whose
/// target requires `cli`, is not built, so an ungated spawn passes `mise run test`
/// (`--all-features`) and then fails, or runs a stale binary left at that path, under a `cargo
/// test` without the feature (#391). A spawning file is held either by its `[[test]]` target
/// carrying `required-features = ["cli"]` in `spargen/Cargo.toml` (as `tests/cli.rs` does), or by
/// [`ungated_spawns`]'s item-level rule (as `tests/carve.rs` and `tests/config.rs` are).
#[test]
fn every_test_that_spawns_the_binary_is_gated_on_the_cli_feature() {
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let gated_targets = cli_gated_targets(&read(&crate_dir.join("Cargo.toml")));

    let sources = rust_files(&crate_dir.join("tests"), &[]);

    let (mut by_target, mut by_item, mut violations) = (0, 0, Vec::new());
    for path in &sources {
        let source = read(path);
        if spawn_sites(&mask(&source)).is_empty() {
            continue;
        }
        let relative = path
            .strip_prefix(crate_dir)
            .expect("under the crate directory")
            .to_str()
            .expect("UTF-8 path")
            .replace(std::path::MAIN_SEPARATOR, "/");
        if gated_targets.contains(&relative) {
            by_target += 1;
            continue;
        }
        by_item += 1;
        violations.extend(
            ungated_spawns(&source)
                .into_iter()
                .map(|line| format!("{relative}:{line}")),
        );
    }
    assert!(
        by_target > 0 && by_item > 0,
        "expected spawning tests gated both by target and by item, found {by_target} and \
         {by_item}: the scanner has stopped reading the tests"
    );
    assert!(
        violations.is_empty(),
        "these spawns of `{SPAWN_VARIABLE}` compile without the `cli` feature, where the binary \
         is not built; gate the top-level item with `#[cfg(feature = \"cli\")]`, the file with \
         `#![cfg(feature = \"cli\")]`, or the target with `required-features = [\"cli\"]`: \
         {violations:#?}"
    );
}
