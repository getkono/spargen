//! Locating the workspace manifest a `workspace = true` dependency inherits its declaration from.

use camino::{Utf8Path, Utf8PathBuf};

/// Which manifest a `workspace = true` dependency resolves its declaration against.
///
/// Each outcome carries only what is read from it: the path searched from exists only where no
/// root was found, the one place it is named. It was once a field of every outcome, and on the
/// others it was never read, so nothing could observe what it held (#202).
pub(super) enum WorkspaceRoot {
    /// The consumer manifest itself — `[package]` and `[workspace]` in one file — at its
    /// absolutized path. It is already parsed and already in `manifests`, so it must not be read or
    /// recorded a second time.
    SelfRooted(Utf8PathBuf),
    /// A separate manifest carrying `[workspace.dependencies]`, still to be read.
    Separate(Utf8PathBuf),
    /// None was found.
    NotFound {
        /// The absolutized consumer manifest the search ran from. A diagnostic that names the
        /// caller's raw spelling would say "found nothing above `Cargo.toml`", which tells the
        /// reader nothing at all.
        searched_from: Utf8PathBuf,
        /// The nearest ancestor manifest that exists and does not parse, with the failure that
        /// stopped it.
        ///
        /// A hint appended to "no workspace manifest was found", never a replacement for it: it
        /// names a file the reader can open and says what is wrong with it, conditionally, because
        /// nothing here knows it was the workspace root — the walk gave up on it precisely because
        /// it could not tell, and it may as well be a sibling crate or a stray file far above the
        /// project. It is never treated as a manifest and never joins `manifests`: treating it as a
        /// root would turn an ordinary crate that happens to sit under an unparseable `Cargo.toml`
        /// into a hard `E023`, and naming it as one sends the reader to repair a file unrelated to
        /// their build.
        unreadable: Option<(Utf8PathBuf, String)>,
    },
}

/// Locate the workspace manifest a `workspace = true` dependency inherits from.
///
/// Three layouts reach here, and Cargo accepts all three:
///
/// - the manifest is itself the workspace root — `[package]` and `[workspace]` in one file, the
///   single-crate repository — and inheritance resolves against the file already in hand;
/// - `package.workspace` names the workspace root *directory*, which is Cargo's own spelling of
///   that field;
/// - otherwise the nearest *lexical* ancestor manifest that parses and declares `[workspace]` wins.
///
/// The ancestor walk runs over an absolutized path. `manifest_path` can legitimately be relative —
/// a build driver other than Cargo may set `CARGO_MANIFEST_DIR` to a relative directory, which
/// `generate_api!` passes on as given — and a relative path has no
/// ancestors to walk, which would report every inherited dependency as unresolvable. Absolutizing
/// is lexical and keeps any `..`, since folding those away changes which file a path names when a
/// component is a symlink; the walk therefore climbs the path as written, not as the filesystem
/// would resolve it. For the same reason it is never canonicalized: Cargo climbs the manifest path
/// it hands the build, symlinks unresolved, so a canonical walk from a member reached through a
/// symlinked directory would find a different root than Cargo did.
///
/// `ceiling` bounds the walk: with `Some(directory)` it reads only ancestors that lie lexically
/// within `directory` (that directory included) and stops at the first one that does not, so a
/// manifest above it is neither adopted as the root nor reported as an unreadable candidate. It is
/// compared component by component against the absolutized path, so it has to be spelled the way
/// that path is. [`audit`](super::audit) passes `None` and climbs to the filesystem root as Cargo
/// does; the fixtures pass their own temporary directory, so no file outside it can change their
/// outcome (#214). `package.workspace` is not a walk and names its root explicitly, so it is not
/// bounded.
pub(super) fn workspace_root(
    manifest_path: &Utf8Path,
    manifest: &toml::Value,
    ceiling: Option<&Utf8Path>,
) -> WorkspaceRoot {
    let absolute = absolutized(manifest_path);
    if manifest.get("workspace").is_some() {
        // Absolutized for the same reason `searched_from` is: this path is what an unresolvable
        // inheritance names, and `./Cargo.toml` tells the reader nothing. It is not added to
        // `manifests` — a self-rooted manifest is the consumer manifest, already recorded — so
        // naming it fully cannot duplicate a `rerun-if-changed` directive.
        return WorkspaceRoot::SelfRooted(absolute);
    }
    if let Some(relative) = manifest
        .get("package")
        .and_then(|value| value.get("workspace"))
        .and_then(toml::Value::as_str)
    {
        return match absolute.parent() {
            Some(parent) => WorkspaceRoot::Separate(parent.join(relative).join("Cargo.toml")),
            None => WorkspaceRoot::NotFound {
                searched_from: absolute,
                unreadable: None,
            },
        };
    }
    let mut directory = absolute.parent().and_then(Utf8Path::parent);
    // The nearest candidate that exists and does not parse. Remembered, never acted on: a valid
    // root further up still wins, exactly as before, and nothing here can tell whether this file
    // was the root at all — the walk skipped it precisely because it could not read it.
    let mut unreadable = None;
    while let Some(candidate_dir) = directory {
        if ceiling.is_some_and(|ceiling| !candidate_dir.starts_with(ceiling)) {
            break;
        }
        let candidate = candidate_dir.join("Cargo.toml");
        if candidate.is_file() {
            // One read, keeping the failure rather than discarding it: it is the only account of
            // what is wrong with this file that anything will ever print.
            match std::fs::read_to_string(&candidate)
                .map_err(|error| error.to_string())
                .and_then(|contents| {
                    toml::from_str::<toml::Value>(&contents).map_err(|error| error.to_string())
                }) {
                Ok(value) if value.get("workspace").is_some() => {
                    return WorkspaceRoot::Separate(candidate);
                }
                // A manifest that parses but declares no `[workspace]` is an ordinary member or an
                // unrelated crate: keep climbing.
                Ok(_) => {}
                Err(reason) => unreadable = unreadable.or(Some((candidate, reason))),
            }
        }
        directory = candidate_dir.parent();
    }
    // Nothing on the path declared `[workspace]`, so there is no root to audit. The unreadable
    // candidate rides along as `unreadable` rather than as a root: it adds a hint to the not-found
    // message an unresolvable inheritance prints, and nothing else. Handing it back as a root
    // would have it audited and recorded as a dependency of the build, turning an ordinary crate
    // that merely sits beneath a broken `Cargo.toml` into a hard `E023`.
    WorkspaceRoot::NotFound {
        searched_from: absolute,
        unreadable,
    }
}

/// `path` made absolute lexically, as [`workspace_root`] describes: any `..` is kept and nothing is
/// canonicalized. A path that cannot be absolutized, or whose absolute form is not UTF-8, is
/// returned as given.
pub(super) fn absolutized(path: &Utf8Path) -> Utf8PathBuf {
    std::path::absolute(path)
        .ok()
        .and_then(|path| Utf8PathBuf::from_path_buf(path).ok())
        .unwrap_or_else(|| path.to_path_buf())
}
