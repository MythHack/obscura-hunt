//! Deterministic fingerprint of the dependency resolution, vendored
//! deno_core fork, native V8 headers, and effective target configuration.
//! Both the `obscura-js` build script and the target-native producer mount
//! this module.
//!
//! The hash covers file contents, workspace-relative names, and the target
//! cfg/rustflags. It contains no absolute host paths, mtimes, or machine-local
//! config contents, and it reads no build artifacts, so it is safe to compute
//! before Cargo has built the target V8 dependency.

use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

const FINGERPRINT_DOMAIN: &[u8] = b"obscura-v8-snapshot-build/1";

/// Complete source trees whose contents can change the snapshot format or its
/// external-reference table.
const TREES: &[&str] = &[
    // The patched deno_core fork owns snapshot serialization, the sidecar
    // format, and the extension/external-reference encoding.
    "vendor/deno_core-0.412.0",
    // The deno_v8 facade pins the raw V8 dependency and re-exports its API.
    "vendor/deno_v8-0.4.0",
    // The authoritative V8 headers and the native callback table source.
    "crates/obscura-js/native",
    // The runtime consumer's extension/order/index assumptions and ops source.
    "crates/obscura-js/src",
    // Bootstrap, Window facade, and IndexedDB shim sources.
    "crates/obscura-js/js",
    // The helper shim that declares the callback count and links the archive.
    "crates/obscura-v8-native",
];

/// Individual files outside those trees that participate in the same
/// compatibility surface.
const FILES: &[&str] = &[
    "Cargo.lock",
    "Cargo.toml",
    "crates/obscura-js/Cargo.toml",
    "crates/obscura-js/build.rs",
    "crates/obscura-js/snapshot_generation.rs",
    "crates/obscura-js/snapshot_fingerprint.rs",
    "crates/obscura-js-snapshot/Cargo.toml",
    "crates/obscura-js-snapshot/build.rs",
    "crates/obscura-js-snapshot/src/main.rs",
];

/// Target ABI inputs that Cargo derives from the selected target and the
/// effective rustflags (`crt-static`, target features, environment). These are
/// hashed instead of `.cargo/config.toml`, which is machine-local.
const TARGET_ENV_KEYS: &[&str] = &[
    "TARGET",
    "CARGO_CFG_TARGET_ARCH",
    "CARGO_CFG_TARGET_OS",
    "CARGO_CFG_TARGET_ENV",
    "CARGO_CFG_TARGET_VENDOR",
    "CARGO_CFG_TARGET_ABI",
    "CARGO_CFG_TARGET_FAMILY",
    "CARGO_CFG_TARGET_ENDIAN",
    "CARGO_CFG_TARGET_POINTER_WIDTH",
    "CARGO_CFG_TARGET_FEATURE",
];

/// Repo paths a Cargo build script must watch so the fingerprint is recomputed.
pub fn input_paths(workspace_root: &Path) -> Vec<PathBuf> {
    TREES
        .iter()
        .copied()
        .chain(FILES.iter().copied())
        .map(|relative| workspace_root.join(relative))
        .collect()
}

pub fn target_env_keys() -> &'static [&'static str] {
    TARGET_ENV_KEYS
}

/// Hash the exact compatibility inputs. No build outputs are read.
pub fn build_fingerprint(workspace_root: &Path) -> Result<String, String> {
    let mut hasher = Sha256::new();
    hasher.update(FINGERPRINT_DOMAIN);

    hasher.update(b"target-config/1");
    for key in TARGET_ENV_KEYS.iter().copied() {
        // Cargo omits cfg variables that are not present for a target (for
        // example `target_abi` on most triples); absent is a stable empty.
        let value = std::env::var(key).unwrap_or_default();
        hash_entry(&mut hasher, key, value.as_bytes());
    }

    for relative in FILES.iter().copied() {
        let path = workspace_root.join(relative);
        let bytes = std::fs::read(&path).map_err(|error| {
            format!(
                "cannot hash snapshot compatibility input {}: {error}",
                path.display()
            )
        })?;
        hash_entry(&mut hasher, relative, &bytes);
    }

    for tree in TREES.iter().copied() {
        let root = workspace_root.join(tree);
        let mut files = Vec::new();
        collect_files(&root, &root, &mut files)?;
        files.sort_by(|left, right| left.0.cmp(&right.0));
        for (relative, path) in files {
            let bytes = std::fs::read(&path).map_err(|error| {
                format!(
                    "cannot hash snapshot compatibility input {}: {error}",
                    path.display()
                )
            })?;
            hash_entry(&mut hasher, &format!("{tree}/{relative}"), &bytes);
        }
    }

    Ok(hex_digest(hasher.finalize()))
}

fn hash_entry(hasher: &mut Sha256, relative: &str, bytes: &[u8]) {
    hasher.update((relative.len() as u64).to_le_bytes());
    hasher.update(relative.as_bytes());
    hasher.update((bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

fn collect_files(
    root: &Path,
    directory: &Path,
    out: &mut Vec<(String, PathBuf)>,
) -> Result<(), String> {
    let entries = std::fs::read_dir(directory).map_err(|error| {
        format!(
            "cannot read snapshot compatibility tree {}: {error}",
            directory.display()
        )
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            format!(
                "cannot read an entry in snapshot compatibility tree {}: {error}",
                directory.display()
            )
        })?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
        if file_type.is_dir() {
            collect_files(root, &path, out)?;
        } else if file_type.is_file() {
            out.push((relative_path(root, &path)?, path));
        } else {
            return Err(format!(
                "snapshot compatibility tree contains an unsupported entry: {}",
                path.display()
            ));
        }
    }
    Ok(())
}

fn relative_path(root: &Path, path: &Path) -> Result<String, String> {
    let relative = path.strip_prefix(root).map_err(|_| {
        format!(
            "snapshot compatibility file {} is outside {}",
            path.display(),
            root.display()
        )
    })?;
    let mut normalized = String::new();
    for component in relative.components() {
        let Component::Normal(part) = component else {
            return Err(format!(
                "snapshot compatibility path {} is not a plain relative path",
                relative.display()
            ));
        };
        if !normalized.is_empty() {
            normalized.push('/');
        }
        normalized.push_str(&part.to_string_lossy());
    }
    if normalized.is_empty() {
        return Err(format!(
            "snapshot compatibility file {} has an empty relative path",
            path.display()
        ));
    }
    Ok(normalized)
}

fn hex_digest(bytes: impl AsRef<[u8]>) -> String {
    let bytes = bytes.as_ref();
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    encoded
}
