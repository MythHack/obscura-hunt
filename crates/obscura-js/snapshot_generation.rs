//! Startup-snapshot generation shared by the crate build script and the
//! target-native `obscura-js-snapshot` producer.
//!
//! A V8 startup snapshot is tied to the exact V8 build that creates it, and
//! this crate's snapshot also has to serialize the global object template
//! installed by [`crate::native_window`]. A stock `mksnapshot` cannot provide
//! either: it has no deno_core ops/extension seam and no way to encode this
//! crate's native callback table. Cross builds therefore consume an artifact
//! produced by running this exact code on the target (or a runner for it).
//!
//! Both producers call the same functions below, so an external artifact and
//! an inline host snapshot have the same V8/deno_core layout, the same stitched
//! bootstrap, the same extension, and the same external-reference schema. The
//! metadata sidecar records the identity a consumer must match; a mismatch is
//! an error, never a fallback.

use std::collections::BTreeMap;
use std::env;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::native_window;

/// Format of the metadata sidecar written next to the snapshot blob. Bump
/// this whenever the fields or their meaning change.
pub const SIDECAR_FORMAT: &str = "obscura-v8-snapshot/2";

/// Name of the snapshot blob inside an artifact directory.
pub const SNAPSHOT_FILE: &str = "OBSCURA_SNAPSHOT.bin";

/// The sidecar sits next to the blob with this suffix appended.
const SIDECAR_SUFFIX: &str = ".json";

/// `deno_core`'s internal snapshot/sidecar format version for the pinned
/// vendored deno_core 0.412. The blob is `[v8 blob][deno sidecar][v8 len]`,
/// and the deno sidecar starts with this little-endian `u32`. Keep this in
/// sync with `vendor/deno_core-0.412.0/snapshot_format.rs`.
pub const DENO_SNAPSHOT_FORMAT: u32 = 2;

/// V8 release the native Window headers and callback table were compiled
/// against. `V8::get_version()` may append build metadata after a `-`.
pub const LINKED_V8_VERSION: &str = "15.2.124.1";

const SIDECAR_MAX_BYTES: u64 = 64 * 1024;

/// Files whose exact bytes determine the serialized snapshot schema. These are
/// compile-time includes, so a generated artifact records the producer's
/// sources rather than whatever happens to be checked out where it is consumed.
const SOURCE_FILES: &[(&str, &[u8])] = &[
    (
        "crates/obscura-js/src/native_window.rs",
        include_bytes!("src/native_window.rs"),
    ),
    (
        "crates/obscura-js/snapshot_generation.rs",
        include_bytes!("snapshot_generation.rs"),
    ),
    (
        "crates/obscura-js/native/incumbent_context.cc",
        include_bytes!("native/incumbent_context.cc"),
    ),
    (
        "crates/obscura-js/native/window_proxy.cc",
        include_bytes!("native/window_proxy.cc"),
    ),
    (
        "crates/obscura-v8-native/src/lib.rs",
        include_bytes!("../obscura-v8-native/src/lib.rs"),
    ),
];

const SIDECAR_KEYS: &[&str] = &[
    "format",
    "target",
    "v8_version",
    "v8_abi",
    "build_fingerprint",
    "bootstrap_sha256",
    "source_sha256",
    "extension",
    "external_references",
    "global_template_middleware",
    "snapshot_index",
    "deno_snapshot_format",
    "deno_sidecar_len",
    "word_bytes",
    "blob_sha256",
    "blob_len",
];

pub struct GeneratedSnapshot {
    pub blob: PathBuf,
    pub files_loaded_during_snapshot: Vec<PathBuf>,
}

/// The bootstrap source for this checkout, stitched exactly like the runtime
/// expects. `bootstrap.js` holds the Window facade and IndexedDB shim inside
/// its private closure through these two markers.
fn bootstrap_js() -> String {
    include_str!("js/bootstrap.js")
        .replace("/* INDEXED_DB */", include_str!("js/indexed_db.js"))
        .replace("/* OBSCURA_WINDOWS */", include_str!("js/windows.js"))
}

pub fn sidecar_path(blob: &Path) -> PathBuf {
    let mut path = blob.as_os_str().to_os_string();
    path.push(SIDECAR_SUFFIX);
    PathBuf::from(path)
}

/// Default artifact directory for `target`: Cargo's target directory when set,
/// otherwise `<workspace>/target`, namespaced so it cannot collide with normal
/// build output.
pub fn artifact_dir(target: &str, workspace_root: &Path) -> PathBuf {
    let target_dir = env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace_root.join("target"));
    target_dir.join("obscura-snapshot").join(target)
}

/// Reject a producer or build script whose linked V8 does not match the native
/// headers used to compile the Window callback table.
pub fn ensure_v8_version(version: &str) -> Result<(), String> {
    if version.split('-').next() != Some(LINKED_V8_VERSION) {
        return Err(format!(
            "linked V8 {version} does not match the native Window headers for V8 \
             {LINKED_V8_VERSION}; refresh crates/obscura-js/native/v8-152.2.0 \
             and the obscura-v8-native callback table before generating snapshots"
        ));
    }
    Ok(())
}

/// Create the snapshot and write `OBSCURA_SNAPSHOT.bin` plus its metadata
/// sidecar into `out_dir`.
///
/// `manifest_dir` is deno_core's `cargo_manifest_dir`. `usize_bytes` is the
/// target's pointer width, because deno_core terminates the blob with a
/// native-width little-endian length.
pub fn generate_artifact(
    out_dir: &Path,
    target: &str,
    build_fingerprint: &str,
    v8_abi: u32,
    manifest_dir: &'static str,
    usize_bytes: usize,
) -> Result<GeneratedSnapshot, String> {
    let v8_version = deno_core::v8::V8::get_version();
    ensure_v8_version(v8_version)?;

    let bootstrap = bootstrap_js();
    let bootstrap_sha256 = sha256_hex(bootstrap.as_bytes());
    let source_sha256 = source_sha256();
    let schema = extension_schema();

    // Exactly one extension: the native Window template middleware and its
    // external-reference table. `skip_op_registration` keeps DOM/browser ops
    // out of this build-time realm; bootstrap installs classes and shims only.
    let output = deno_core::snapshot::create_snapshot(
        deno_core::snapshot::CreateSnapshotOptions {
            cargo_manifest_dir: manifest_dir,
            startup_snapshot: None,
            skip_op_registration: true,
            extensions: vec![native_window::native_window_extension()],
            extension_transpiler: None,
            with_runtime_cb: Some(Box::new(move |runtime| {
                // The named/indexed handlers are part of the serialized global
                // template; the failed-access handler is isolate state and is
                // not encoded in the blob. Install it here too so bootstrap
                // runs under the same security behavior as the target runtime.
                native_window::install_failed_access_check(runtime.v8_isolate());
                runtime
                    .execute_script("<obscura:bootstrap>", bootstrap.clone())
                    .expect("bootstrap.js should not fail during snapshot creation");
            })),
        },
        None,
    )
    .map_err(|error| format!("V8 snapshot creation failed: {error}"))?;

    let (deno_sidecar_len, deno_format) = deno_sidecar(&output.output, usize_bytes)?;
    if deno_format != DENO_SNAPSHOT_FORMAT {
        return Err(format!(
            "deno_core produced snapshot sidecar format {deno_format}, expected \
             {DENO_SNAPSHOT_FORMAT}; update snapshot_generation.rs with the vendored deno_core"
        ));
    }

    let blob_sha256 = sha256_hex(&output.output);
    std::fs::create_dir_all(out_dir)
        .map_err(|error| format!("cannot create {}: {error}", out_dir.display()))?;
    let blob = out_dir.join(SNAPSHOT_FILE);
    let sidecar = sidecar_path(&blob);

    std::fs::write(&blob, &*output.output)
        .map_err(|error| format!("cannot write {}: {error}", blob.display()))?;

    let mut fields = Map::new();
    let mut field = |key: &str, value: &str| {
        fields.insert(key.to_string(), Value::String(value.to_string()));
    };
    field("format", SIDECAR_FORMAT);
    field("target", target);
    field("v8_version", v8_version);
    field("build_fingerprint", build_fingerprint);
    field("bootstrap_sha256", &bootstrap_sha256);
    field("source_sha256", &source_sha256);
    field("extension", schema.name);
    field("external_references", &schema.external_references.to_string());
    field(
        "global_template_middleware",
        if schema.global_template_middleware { "true" } else { "false" },
    );
    field("snapshot_index", &schema.snapshot_index.to_string());
    field("deno_snapshot_format", &deno_format.to_string());
    field("deno_sidecar_len", &deno_sidecar_len.to_string());
    field("word_bytes", &usize_bytes.to_string());
    field("blob_sha256", &blob_sha256);
    field("blob_len", &(output.output.len() as u64).to_string());
    drop(field);
    // Integer, not a formatted signature string: the runtime compares the
    // linked helper's u32 contract directly.
    fields.insert(
        "v8_abi".to_string(),
        Value::Number(serde_json::Number::from(v8_abi)),
    );

    let text = serde_json::to_string_pretty(&Value::Object(fields))
        .map_err(|error| format!("cannot serialize {}: {error}", sidecar.display()))?;
    std::fs::write(&sidecar, text)
        .map_err(|error| format!("cannot write {}: {error}", sidecar.display()))?;

    Ok(GeneratedSnapshot {
        blob,
        files_loaded_during_snapshot: output.files_loaded_during_snapshot,
    })
}

/// Validate a snapshot artifact against this checkout, `target`, and the V8
/// linked into the caller. Every mismatch is an explicit incompatibility; no
/// host blob, index, or version substitution happens here.
pub fn validate_artifact(
    blob_path: &Path,
    target: &str,
    v8_version: &str,
    build_fingerprint: &str,
    expected_v8_abi: Option<u32>,
    usize_bytes: usize,
) -> Result<u32, String> {
    ensure_v8_version(v8_version)?;

    if !blob_path.is_file() {
        return Err(format!(
            "snapshot artifact {} does not exist; generate it with the \
             obscura-js-snapshot producer for {target}",
            blob_path.display()
        ));
    }

    let sidecar_path = sidecar_path(blob_path);
    let metadata = std::fs::metadata(&sidecar_path).map_err(|error| {
        format!(
            "snapshot sidecar {} is unavailable ({error}); a snapshot without its \
             identity sidecar cannot be validated",
            sidecar_path.display()
        )
    })?;
    if metadata.len() > SIDECAR_MAX_BYTES {
        return Err(format!(
            "snapshot sidecar {} is {} bytes, larger than the {} byte limit",
            sidecar_path.display(),
            metadata.len(),
            SIDECAR_MAX_BYTES
        ));
    }
    let text = std::fs::read_to_string(&sidecar_path)
        .map_err(|error| format!("cannot read {}: {error}", sidecar_path.display()))?;
    let fields = parse_sidecar(&text, &sidecar_path)?;

    let schema = extension_schema();
    let bootstrap_sha256 = sha256_hex(bootstrap_js().as_bytes());
    let source_sha256 = source_sha256();
    let expected = [
        ("format", SIDECAR_FORMAT.to_string()),
        ("target", target.to_string()),
        ("v8_version", v8_version.to_string()),
        ("build_fingerprint", build_fingerprint.to_string()),
        ("bootstrap_sha256", bootstrap_sha256),
        ("source_sha256", source_sha256),
        ("extension", schema.name.to_string()),
        ("external_references", schema.external_references.to_string()),
        (
            "global_template_middleware",
            schema.global_template_middleware.to_string(),
        ),
        ("snapshot_index", schema.snapshot_index.to_string()),
        ("deno_snapshot_format", DENO_SNAPSHOT_FORMAT.to_string()),
        ("word_bytes", usize_bytes.to_string()),
    ];
    for (key, expected_value) in expected {
        require_equal(&fields, &sidecar_path, key, &expected_value)?;
    }
    // `ensure_v8_version` above is the native-header prefix assertion. The
    // exact full V8 version (including any embedder suffix) must also match:
    // V8 rejects a snapshot whose recorded version differs in any way.
    let recorded_v8_abi = require_u32(&fields, &sidecar_path, "v8_abi")?;
    let recorded_pointer_bytes = ((recorded_v8_abi >> 8) & 0xff) as usize;
    if recorded_pointer_bytes != usize_bytes {
        return Err(incompatible(
            &sidecar_path,
            "v8_abi pointer width",
            &usize_bytes.to_string(),
            &recorded_pointer_bytes.to_string(),
        ));
    }
    if let Some(expected_v8_abi) = expected_v8_abi {
        if recorded_v8_abi != expected_v8_abi {
            return Err(incompatible(
                &sidecar_path,
                "v8_abi",
                &format!("{expected_v8_abi:#010x}"),
                &format!("{recorded_v8_abi:#010x}"),
            ));
        }
    }

    let blob = std::fs::read(blob_path)
        .map_err(|error| format!("cannot read {}: {error}", blob_path.display()))?;
    let expected_len = require_number(&fields, &sidecar_path, "blob_len")?;
    let actual_len = blob.len() as u64;
    if expected_len != actual_len {
        return Err(incompatible(
            &sidecar_path,
            "blob_len",
            &expected_len.to_string(),
            &actual_len.to_string(),
        ));
    }
    let expected_sha = require_field(&fields, &sidecar_path, "blob_sha256")?;
    let actual_sha = sha256_hex(&blob);
    if expected_sha != actual_sha {
        return Err(incompatible(
            &sidecar_path,
            "blob_sha256",
            expected_sha,
            &actual_sha,
        ));
    }

    let (deno_sidecar_len, deno_format) = deno_sidecar(&blob, usize_bytes)?;
    if deno_format != DENO_SNAPSHOT_FORMAT {
        return Err(format!(
            "snapshot {} has deno_core sidecar format {deno_format}, expected \
             {DENO_SNAPSHOT_FORMAT}; regenerate it with the current producer",
            blob_path.display()
        ));
    }
    let recorded_sidecar_len = require_number(&fields, &sidecar_path, "deno_sidecar_len")?;
    if recorded_sidecar_len != deno_sidecar_len {
        return Err(incompatible(
            &sidecar_path,
            "deno_sidecar_len",
            &recorded_sidecar_len.to_string(),
            &deno_sidecar_len.to_string(),
        ));
    }

    Ok(recorded_v8_abi)
}

struct ExtensionSchema {
    name: &'static str,
    external_references: usize,
    global_template_middleware: bool,
    snapshot_index: usize,
}

fn extension_schema() -> ExtensionSchema {
    let extension = native_window::native_window_extension();
    ExtensionSchema {
        name: extension.name,
        external_references: extension.external_references.len(),
        global_template_middleware: extension.global_template_middleware.is_some(),
        snapshot_index: native_window::SNAPSHOT_INDEX,
    }
}

/// Split deno_core's snapshot envelope and return the sidecar length and its
/// format word.
fn deno_sidecar(blob: &[u8], usize_bytes: usize) -> Result<(u64, u32), String> {
    if usize_bytes != 4 && usize_bytes != 8 {
        return Err(format!(
            "unsupported target pointer width of {usize_bytes} bytes for a deno_core \
             snapshot envelope"
        ));
    }
    if blob.len() < usize_bytes + 4 {
        return Err(format!(
            "snapshot is {} bytes, too short for a deno_core snapshot envelope",
            blob.len()
        ));
    }
    let mut length = [0u8; 8];
    length[..usize_bytes].copy_from_slice(&blob[blob.len() - usize_bytes..]);
    let v8_len = usize::try_from(u64::from_le_bytes(length))
        .map_err(|_| "snapshot V8 blob length does not fit this address space".to_string())?;
    let sidecar_end = blob.len() - usize_bytes;
    if v8_len == 0 || v8_len + 4 > sidecar_end {
        return Err(format!(
            "snapshot envelope declares a {v8_len} byte V8 blob, which does not fit \
             in a {} byte file",
            blob.len()
        ));
    }
    let sidecar = &blob[v8_len..sidecar_end];
    let format = u32::from_le_bytes(sidecar[..4].try_into().expect("checked length"));
    Ok((sidecar.len() as u64, format))
}

fn source_sha256() -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"obscura-v8-snapshot-source/1");
    for (path, bytes) in SOURCE_FILES {
        hasher.update((path.len() as u64).to_le_bytes());
        hasher.update(path.as_bytes());
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(bytes);
    }
    hex_digest(hasher.finalize())
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex_digest(Sha256::digest(bytes))
}

fn hex_digest(bytes: impl AsRef<[u8]>) -> String {
    let bytes = bytes.as_ref();
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    encoded
}

fn parse_sidecar(
    text: &str,
    path: &Path,
) -> Result<BTreeMap<String, String>, String> {
    let value: Value = serde_json::from_str(text).map_err(|error| {
        format!(
            "snapshot sidecar {} is not valid JSON: {error}",
            path.display()
        )
    })?;
    let Value::Object(fields) = value else {
        return Err(format!(
            "snapshot sidecar {} must be a JSON object",
            path.display()
        ));
    };
    let mut parsed = BTreeMap::new();
    for (key, value) in &fields {
        if !SIDECAR_KEYS.contains(&key.as_str()) {
            return Err(format!(
                "snapshot sidecar {} has unknown field {key:?}",
                path.display()
            ));
        }
        if key == "v8_abi" {
            let abi = value.as_u64().ok_or_else(|| {
                format!(
                    "snapshot sidecar {} field \"v8_abi\" must be an unsigned integer",
                    path.display()
                )
            })?;
            let abi = u32::try_from(abi).map_err(|_| {
                format!(
                    "snapshot sidecar {} field \"v8_abi\" is not a uint32: {abi}",
                    path.display()
                )
            })?;
            // Normalize to the same decimal string surface as the other
            // integer fields; the JSON value itself is a number.
            parsed.insert(key.clone(), abi.to_string());
            continue;
        }
        let Value::String(value) = value else {
            return Err(format!(
                "snapshot sidecar {} field {key:?} must be a string",
                path.display()
            ));
        };
        parsed.insert(key.clone(), value.clone());
    }
    for key in SIDECAR_KEYS {
        if !parsed.contains_key(*key) {
            return Err(format!(
                "snapshot sidecar {} is missing field {key:?}",
                path.display()
            ));
        }
    }
    Ok(parsed)
}

fn require_field<'a>(
    fields: &'a BTreeMap<String, String>,
    path: &Path,
    key: &str,
) -> Result<&'a str, String> {
    fields
        .get(key)
        .map(String::as_str)
        .ok_or_else(|| format!("snapshot sidecar {} is missing field {key:?}", path.display()))
}

fn require_equal(
    fields: &BTreeMap<String, String>,
    path: &Path,
    key: &str,
    expected: &str,
) -> Result<(), String> {
    let found = require_field(fields, path, key)?;
    if found != expected {
        return Err(incompatible(path, key, expected, found));
    }
    Ok(())
}

fn require_number(
    fields: &BTreeMap<String, String>,
    path: &Path,
    key: &str,
) -> Result<u64, String> {
    let value = require_field(fields, path, key)?;
    value.parse::<u64>().map_err(|_| {
        format!(
            "snapshot sidecar {} field {key:?} is not an unsigned integer: {value:?}",
            path.display()
        )
    })
}

fn require_u32(
    fields: &BTreeMap<String, String>,
    path: &Path,
    key: &str,
) -> Result<u32, String> {
    let value = require_field(fields, path, key)?;
    u32::from_str_radix(value, 10).map_err(|_| {
        format!(
            "snapshot sidecar {} field {key:?} is not a uint32: {value:?}",
            path.display()
        )
    })
}

fn incompatible(path: &Path, key: &str, expected: &str, found: &str) -> String {
    format!(
        "snapshot artifact is incompatible: {} records {key}={found:?}, expected \
         {expected:?}. Regenerate it with the obscura-js-snapshot producer from this \
         checkout and target.",
        path.display()
    )
}
