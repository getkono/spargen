#[path = "src/source/sha256.rs"]
mod sha256;

use std::path::{Path, PathBuf};

fn main() {
    let manifest_dir = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let mut files = vec![
        manifest_dir.join("Cargo.toml"),
        manifest_dir.join("build.rs"),
    ];
    collect_files(&manifest_dir.join("src"), &mut files);
    let workspace_lock = manifest_dir
        .parent()
        .map(|parent| parent.join("Cargo.lock"));
    if let Some(lock) = workspace_lock.filter(|path| path.is_file()) {
        files.push(lock);
    }
    files.sort();
    files.dedup();

    let mut input = b"spargen-build-fingerprint-v1\0".to_vec();
    for path in files {
        println!("cargo:rerun-if-changed={}", path.display());
        let relative = path.strip_prefix(&manifest_dir).unwrap_or(&path);
        append(&mut input, relative.to_string_lossy().as_bytes());
        append(&mut input, &std::fs::read(&path).unwrap());
    }
    println!(
        "cargo:rustc-env=SPARGEN_BUILD_FINGERPRINT={}",
        sha256::sha256_hex(&input)
    );
}

fn collect_files(dir: &Path, files: &mut Vec<PathBuf>) {
    let mut entries = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect_files(&path, files);
        } else if path.is_file() {
            files.push(path);
        }
    }
}

fn append(output: &mut Vec<u8>, bytes: &[u8]) {
    output.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
    output.extend_from_slice(bytes);
}
