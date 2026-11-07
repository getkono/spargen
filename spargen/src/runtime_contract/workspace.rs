//! Locating the workspace manifest a `workspace = true` dependency inherits its declaration from,
//! and the `Cargo.lock` Cargo writes beside it.

use camino::{Utf8Path, Utf8PathBuf};
use semver::Version;

/// Which manifest a `workspace = true` dependency resolves its declaration against.
///
/// Each outcome carries only what is read from it: the path searched from exists only where no
/// root was found, the one place it is named (#202).
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
    // root further up still wins, and nothing here can tell whether this file
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

/// Whether the audited package sets `publish = false`, so no registry user ever resolves its
/// requirements and its workspace's `Cargo.lock` decides every version it builds with.
///
/// Cargo spells "never publish" as `publish = false` or as an empty registry list, `publish = []`;
/// `publish.workspace = true` takes either from `[workspace.package]`. A list naming a registry,
/// `publish = true`, or no `publish` at all leaves the package publishable.
pub(super) fn unpublished(manifest: &toml::Value, workspace: Option<&toml::Value>) -> bool {
    let never = |value: &toml::Value| match value {
        toml::Value::Boolean(publish) => !publish,
        toml::Value::Array(registries) => registries.is_empty(),
        _ => false,
    };
    match manifest
        .get("package")
        .and_then(|package| package.get("publish"))
    {
        Some(publish) if publish.get("workspace").and_then(toml::Value::as_bool) == Some(true) => {
            workspace
                .and_then(|root| root.get("workspace"))
                .and_then(|root| root.get("package"))
                .and_then(|package| package.get("publish"))
                .is_some_and(never)
        }
        Some(publish) => never(publish),
        None => false,
    }
}

/// The `Cargo.lock` Cargo resolves the audited package with: beside the workspace root's
/// manifest, or beside the package's own when no workspace root was found, since such a package
/// is the root of a workspace of its own.
pub(super) fn lockfile_path(root: &WorkspaceRoot) -> Utf8PathBuf {
    let manifest = match root {
        WorkspaceRoot::SelfRooted(path) | WorkspaceRoot::Separate(path) => path,
        WorkspaceRoot::NotFound { searched_from, .. } => searched_from,
    };
    manifest
        .parent()
        .unwrap_or_else(|| Utf8Path::new(""))
        .join("Cargo.lock")
}

/// Where the audited package is built from when Cargo builds it as a dependency of another
/// workspace, with that workspace's `Cargo.lock`, so that a lockfile committed in the package's
/// own sources decides nothing.
pub(super) enum ForeignSource {
    /// The directory of Cargo's source cache the manifest lies in: `git/checkouts` (a git
    /// dependency) or `registry/src` (a registry dependency) under Cargo's home.
    Cache(Utf8PathBuf),
    /// The package's own directory, which holds the `.cargo-checksum.json` Cargo requires of every
    /// package in a directory source: a `cargo vendor` directory or another `[source]` replacement
    /// of kind `directory`.
    Directory(Utf8PathBuf),
}

/// The checksum file Cargo reads beside every package of a directory source, and refuses to build
/// the package without. `cargo vendor` writes one into each crate it vendors.
const DIRECTORY_SOURCE_CHECKSUM: &str = ".cargo-checksum.json";

/// Where the audited package is built from, when that is a source Cargo builds only as a
/// dependency of another workspace.
///
/// Cargo's source cache is `git/checkouts` or `registry/src` under `cargo_home`, compared
/// component by component against the absolutized manifest path, and is checked first. A
/// directory source is recognised by its [`DIRECTORY_SOURCE_CHECKSUM`] beside the manifest, which
/// Cargo requires there whichever `.cargo/config.toml` declared the source.
pub(super) fn foreign_source(
    manifest_path: &Utf8Path,
    cargo_home: Option<&Utf8Path>,
) -> Option<ForeignSource> {
    let manifest = absolutized(manifest_path);
    let cache = cargo_home.and_then(|cargo_home| {
        [["git", "checkouts"], ["registry", "src"]]
            .into_iter()
            .map(|[first, second]| cargo_home.join(first).join(second))
            .find(|cache| manifest.starts_with(cache))
    });
    if let Some(cache) = cache {
        return Some(ForeignSource::Cache(cache));
    }
    let package = manifest.parent()?;
    package
        .join(DIRECTORY_SOURCE_CHECKSUM)
        .is_file()
        .then(|| ForeignSource::Directory(package.to_path_buf()))
}

/// What the workspace's `Cargo.lock` says about the audited package's direct dependencies.
pub(super) enum Lock {
    /// The package is built from this source, as a dependency of a workspace whose own lockfile
    /// resolves it, so no lockfile is read.
    Foreign(ForeignSource),
    /// No file exists at this path.
    Absent(Utf8PathBuf),
    /// The file exists, and could not be read or parsed or does not lock the audited package.
    Unusable { path: Utf8PathBuf, reason: String },
    /// The file locks the audited package, whose direct dependencies resolve to these versions:
    /// every kind (normal, build and dev) under every target, as the lockfile does not tell them
    /// apart.
    Locked {
        path: Utf8PathBuf,
        dependencies: Vec<(String, Version)>,
    },
}

impl Lock {
    /// The lockfile's path, when a file exists there to be read: the one a rebuild has to watch.
    pub(super) fn existing(&self) -> Option<&Utf8Path> {
        match self {
            Lock::Absent(_) | Lock::Foreign(_) => None,
            Lock::Unusable { path, .. } | Lock::Locked { path, .. } => Some(path),
        }
    }
}

/// Read the lockfile at `path` for `package`, the audited package's name.
///
/// The audited package is the `[[package]]` entry of that name with no `source`, which is how a
/// lockfile records a workspace member. Each of its `dependencies` is `name`, `name version`, or
/// `name version (source)`; the bare form is written only when one version of `name` is locked,
/// which is then read from that crate's own entry.
pub(super) fn read_lock(path: Utf8PathBuf, package: Option<&str>) -> Lock {
    if !path.is_file() {
        return Lock::Absent(path);
    }
    match locked_dependencies(&path, package) {
        Ok(dependencies) => Lock::Locked { path, dependencies },
        Err(reason) => Lock::Unusable { path, reason },
    }
}

fn locked_dependencies(
    path: &Utf8Path,
    package: Option<&str>,
) -> Result<Vec<(String, Version)>, String> {
    let contents = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
    let lock = toml::from_str::<toml::Value>(&contents).map_err(|error| {
        let mut reason = error.to_string();
        reason.truncate(reason.trim_end().len());
        reason
    })?;
    let package = package.ok_or_else(|| "the audited manifest names no package".to_owned())?;
    let packages = lock
        .get("package")
        .and_then(toml::Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    let field = |entry: &'_ toml::Value, key: &str| {
        entry
            .get(key)
            .and_then(toml::Value::as_str)
            .map(str::to_owned)
    };
    let audited = packages
        .iter()
        .find(|entry| {
            field(entry, "name").as_deref() == Some(package) && entry.get("source").is_none()
        })
        .ok_or_else(|| format!("it locks no workspace package named `{package}`"))?;
    let mut dependencies = Vec::new();
    for dependency in audited
        .get("dependencies")
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(toml::Value::as_str)
    {
        let mut words = dependency.split_whitespace();
        let Some(name) = words.next() else { continue };
        let version = match words.next() {
            Some(version) => Some(version.to_owned()),
            None => packages
                .iter()
                .find(|entry| field(entry, "name").as_deref() == Some(name))
                .and_then(|entry| field(entry, "version")),
        };
        if let Some(version) = version.and_then(|version| Version::parse(&version).ok()) {
            dependencies.push((name.to_owned(), version));
        }
    }
    Ok(dependencies)
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
