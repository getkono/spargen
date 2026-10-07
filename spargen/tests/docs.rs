//! The repository's documentation held to what it claims about itself.
//!
//! Two claims drifted with nothing reading them (#464). The `docs` task and its CI job said they
//! failed on a broken link or include, but `mdbook build` checks no link and exits 0 on an
//! include it cannot read, and the book shipped dead links;
//! [`every_link_in_the_built_book_resolves`] is the check they now run after the build. And the
//! install lines still pinned `0.4` a release after the workspace moved to `0.5`;
//! [`every_install_line_names_the_released_minor`] holds them to `Cargo.toml`'s version, so a
//! release bump that leaves one behind fails.

use std::collections::{BTreeMap, BTreeSet};

use camino::{Utf8Path, Utf8PathBuf};

fn workspace_root() -> Utf8PathBuf {
    Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the crate directory has a parent")
        .to_owned()
}

fn read(path: &Utf8Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|error| panic!("{path} must be readable: {error}"))
}

/// Every file under `dir` whose extension is `extension`, skipping hidden directories (`.git`,
/// and the worktrees a checkout may hold under `.claude/`) and `skip`, relative to `dir`.
fn files(dir: &Utf8Path, extension: &str, skip: &[&str]) -> Vec<Utf8PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_owned()];
    while let Some(next) = pending.pop() {
        let entries = next
            .read_dir_utf8()
            .unwrap_or_else(|error| panic!("{next} must be listable: {error}"));
        for entry in entries {
            let entry = entry.unwrap_or_else(|error| panic!("{next} must be listable: {error}"));
            let path = entry.path();
            let relative = path
                .strip_prefix(dir)
                .expect("a listed entry is under its root");
            if entry.file_name().starts_with('.') || skip.contains(&relative.as_str()) {
                continue;
            }
            let kind = entry
                .file_type()
                .unwrap_or_else(|error| panic!("{path} must be statable: {error}"));
            if kind.is_dir() {
                pending.push(path.to_owned());
            } else if path.extension() == Some(extension) {
                found.push(path.to_owned());
            }
        }
    }
    found.sort();
    found
}

/// The version requirement a TOML dependency line gives `spargen` or `spargen-macro`, as
/// `(crate, requirement)`: `spargen = "0.5"`, or `spargen = { version = "0.5", … }`. A line
/// naming the crate by `path` alone, as the examples' manifests do, gives none.
fn install_requirement(line: &str) -> Option<(&str, &str)> {
    let (name, value) = line.trim().split_once('=')?;
    let name = name.trim();
    if name != "spargen" && name != "spargen-macro" {
        return None;
    }
    let value = value.trim();
    let quoted = match value.strip_prefix('{') {
        Some(table) => {
            let (_, after) = table.split_once("version")?;
            after.trim_start().strip_prefix('=')?.trim_start()
        }
        None => value,
    };
    let requirement = quoted.strip_prefix('"')?.split('"').next()?;
    Some((name, requirement))
}

#[test]
fn install_lines_are_recognised_in_both_spellings() {
    assert_eq!(
        install_requirement(r#"spargen-macro = "0.4""#),
        Some(("spargen-macro", "0.4"))
    );
    assert_eq!(
        install_requirement(r#"spargen = { version = "0.4", features = ["remote-fetch"] }"#),
        Some(("spargen", "0.4"))
    );
    assert_eq!(
        install_requirement(r#"spargen = { path = "../../spargen", default-features = false }"#),
        None
    );
    assert_eq!(install_requirement(r#"spargen-cli = "0.4""#), None);
    assert_eq!(
        install_requirement("spargen_macro::generate_api!(spec = \"a\");"),
        None
    );
}

#[test]
fn every_install_line_names_the_released_minor() {
    // `version.workspace = true`, so this is the workspace `Cargo.toml`'s version: the one the
    // next release publishes. A caret requirement on its minor is what a reader should copy.
    let expected = format!(
        "{}.{}",
        env!("CARGO_PKG_VERSION_MAJOR"),
        env!("CARGO_PKG_VERSION_MINOR")
    );
    let root = workspace_root();
    let mut lines = 0usize;
    let mut stale = Vec::new();
    for path in files(&root, "md", &["target", "docs/book/book", "references"]) {
        // A changelog records what each past release required; it is history, not an install line.
        if path.file_name() == Some("CHANGELOG.md") {
            continue;
        }
        for (number, line) in read(&path).lines().enumerate() {
            let Some((name, requirement)) = install_requirement(line) else {
                continue;
            };
            lines += 1;
            if requirement != expected {
                let relative = path
                    .strip_prefix(&root)
                    .expect("a listed file is under the root");
                stale.push(format!(
                    "{relative}:{}: {name} = \"{requirement}\"",
                    number + 1
                ));
            }
        }
    }
    assert!(
        lines > 0,
        "found no `spargen`/`spargen-macro` install line in any Markdown file; the scan reads nothing"
    );
    assert!(
        stale.is_empty(),
        "these install lines do not require the workspace's current minor `{expected}` \
         (Cargo.toml `version`); a reader copying them gets an older release:\n{}",
        stale.join("\n")
    );
}

/// The values of every `href="…"` and `src="…"` attribute in an HTML page.
fn link_targets(html: &str) -> Vec<String> {
    let mut targets = Vec::new();
    for attribute in [" href=\"", " src=\""] {
        let mut rest = html;
        while let Some(start) = rest.find(attribute) {
            rest = &rest[start + attribute.len()..];
            let Some(end) = rest.find('"') else {
                break;
            };
            targets.push(rest[..end].replace("&amp;", "&"));
            rest = &rest[end..];
        }
    }
    targets
}

/// The `id="…"` anchors an HTML page defines.
fn anchors(html: &str) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    let mut rest = html;
    while let Some(start) = rest.find(" id=\"") {
        rest = &rest[start + 5..];
        let Some(end) = rest.find('"') else {
            break;
        };
        ids.insert(rest[..end].to_owned());
        rest = &rest[end..];
    }
    ids
}

/// Undo percent-encoding, which mdBook applies to non-ASCII and reserved characters in a link.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let escape = (bytes[index] == b'%')
            .then(|| text.get(index + 1..index + 3))
            .flatten()
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match escape {
            Some(byte) => {
                decoded.push(byte);
                index += 3;
            }
            None => {
                decoded.push(bytes[index]);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

/// Lexically resolve `target` against `page`'s directory inside `book`. `None` where it climbs
/// out of the book: the published site has nothing above its root, so such a link is dead even
/// when the checkout happens to hold a file there.
fn resolve(book: &Utf8Path, page: &Utf8Path, target: &str) -> Option<Utf8PathBuf> {
    let mut parts: Vec<&str> = if target.starts_with('/') {
        Vec::new()
    } else {
        let dir = page.parent().expect("a page has a directory");
        dir.strip_prefix(book)
            .expect("a page is inside the book")
            .components()
            .map(|part| part.as_str())
            .collect()
    };
    for part in target.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            part => parts.push(part),
        }
    }
    let mut path = book.to_owned();
    path.extend(parts);
    Some(path)
}

#[test]
fn link_resolution_stays_inside_the_book() {
    let book = Utf8Path::new("/book");
    let page = Utf8Path::new("/book/recipes.html");
    assert_eq!(
        resolve(book, page, "./errors.html").as_deref(),
        Some(Utf8Path::new("/book/errors.html"))
    );
    assert_eq!(
        resolve(book, page, "/css/a.css").as_deref(),
        Some(Utf8Path::new("/book/css/a.css"))
    );
    assert_eq!(resolve(book, page, "../corpus/recipes/README.html"), None);
    assert_eq!(percent_decode("a%20b%zz"), "a b%zz");
    assert_eq!(
        link_targets(r#"<a href="x.html#y&amp;z">x</a><img src="i.png">"#),
        ["x.html#y&z", "i.png"]
    );
    assert_eq!(
        anchors(r#"<h2 id="one"></h2><p id="two">"#),
        BTreeSet::from(["one".to_owned(), "two".to_owned()])
    );
}

#[test]
#[ignore = "reads the site `mdbook build docs/book` writes; run by `mise run docs` and its CI job"]
fn every_link_in_the_built_book_resolves() {
    let book = workspace_root().join("docs/book/book");
    assert!(
        book.join("index.html").is_file(),
        "{book} holds no built site: run `mdbook build docs/book` first"
    );
    let pages = files(&book, "html", &[]);
    let mut page_anchors = BTreeMap::new();
    for page in &pages {
        page_anchors.insert(page.clone(), anchors(&read(page)));
    }
    let mut checked = 0usize;
    let mut broken = BTreeSet::new();
    for page in &pages {
        let shown = page.strip_prefix(&book).expect("a page is inside the book");
        let html = read(page);
        // mdBook logs an `{{#include}}` it cannot read and still exits 0, leaving the directive
        // in the page as literal text. Every standalone `docs/*.md` page reaches the book that way.
        if html.contains("{{#include") {
            broken.insert(format!("{shown}: an unresolved {{{{#include}}}} directive"));
        }
        for target in link_targets(&html) {
            // A scheme (`https:`, `mailto:`) or a protocol-relative `//host` leaves the site.
            let external = target.starts_with("//")
                || target
                    .split_once(':')
                    .is_some_and(|(scheme, _)| !scheme.is_empty() && !scheme.contains('/'));
            if external {
                continue;
            }
            checked += 1;
            let target = percent_decode(&target);
            let (path, fragment) = match target.split_once('#') {
                Some((path, fragment)) => (path, Some(fragment)),
                None => (target.as_str(), None),
            };
            let path = path.split('?').next().unwrap_or_default();
            let file = if path.is_empty() {
                Some(page.clone())
            } else {
                resolve(&book, page, path)
            };
            // A link to a directory (`/`, `./`) is served as its `index.html`.
            let file = file.map(|file| {
                if file.is_dir() {
                    file.join("index.html")
                } else {
                    file
                }
            });
            let Some(file) = file.filter(|file| file.is_file()) else {
                broken.insert(format!("{shown}: {target} (no such file in the book)"));
                continue;
            };
            let missing_anchor =
                fragment
                    .filter(|fragment| !fragment.is_empty())
                    .filter(|fragment| {
                        page_anchors
                            .get(&file)
                            .is_some_and(|ids| !ids.contains(*fragment))
                    });
            if missing_anchor.is_some() {
                broken.insert(format!("{shown}: {target} (no such anchor)"));
            }
        }
    }
    assert!(
        checked > 0,
        "found no relative link in {book}; the scan reads nothing"
    );
    assert!(
        broken.is_empty(),
        "the built book has broken links or includes. A standalone `docs/*.md` page is included into the book, \
         so a relative link out of `docs/` resolves on GitHub but not on the site: link to the \
         repository URL instead.\n{}",
        broken.into_iter().collect::<Vec<_>>().join("\n")
    );
}
