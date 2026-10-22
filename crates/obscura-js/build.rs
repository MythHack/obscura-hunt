use std::env;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

#[path = "src/native_window.rs"]
mod native_window;
#[path = "snapshot_generation.rs"]
mod snapshot_generation;
#[path = "snapshot_fingerprint.rs"]
mod snapshot_fingerprint;

use snapshot_generation as snapshot;

enum ArtifactSource {
    /// The build script runs on the target: generate the snapshot inline.
    Generate,
    /// Consume a target-native snapshot produced by `obscura-js-snapshot`.
    Prebuilt(PathBuf),
}

fn main() {
    println!("cargo:rustc-check-cfg=cfg(obscura_external_bootstrap)");
    println!("cargo:rerun-if-changed=js/bootstrap.js");
    println!("cargo:rerun-if-changed=js/indexed_db.js");
    println!("cargo:rerun-if-changed=js/webcrypto.js");
    println!("cargo:rerun-if-changed=js/windows.js");
    println!("cargo:rerun-if-changed=js/worker.js");
    println!("cargo:rerun-if-changed=js/text_decoder.js");
    println!("cargo:rerun-if-changed=js/streams.js");
    println!("cargo:rerun-if-changed=snapshot_generation.rs");
    println!("cargo:rerun-if-changed=src/native_window.rs");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=native");
    println!("cargo:rerun-if-changed=../obscura-v8-native/src/lib.rs");
    println!("cargo:rerun-if-env-changed=OBSCURA_SNAPSHOT_BIN");
    println!("cargo:rerun-if-env-changed=OBSCURA_SNAPSHOT_DIR");
    println!("cargo:rerun-if-env-changed=OBSCURA_SNAPSHOT_PRODUCER");

    compile_incumbent_context();

    let workspace_root = workspace_root().unwrap_or_else(|error| fail(&error));
    for path in snapshot_fingerprint::input_paths(&workspace_root) {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    for key in snapshot_fingerprint::target_env_keys() {
        println!("cargo:rerun-if-env-changed={key}");
    }
    let build_fingerprint = snapshot_fingerprint::build_fingerprint(&workspace_root)
        .unwrap_or_else(|error| fail(&error));

    let out_dir = absolute(PathBuf::from(env::var("OUT_DIR").expect("Cargo sets OUT_DIR")))
        .unwrap_or_else(|error| fail(&error));
    let target = env::var("TARGET").expect("Cargo sets TARGET");
    let host = env::var("HOST").expect("Cargo sets HOST");
    let is_cross = target != host;
    let usize_bytes = target_usize_bytes();
    let v8_version = deno_core::v8::V8::get_version();
    // Host helper contract. For a native build this is the target contract;
    // for a cross build it must not be compared with the target artifact.
    let host_v8_abi = obscura_v8_native::native_window_abi();

    match resolve_artifact(&target, is_cross, &workspace_root)
        .unwrap_or_else(|error| fail(&error))
    {
        ArtifactSource::Generate => {
            let generated = snapshot::generate_artifact(
                &out_dir,
                &target,
                &build_fingerprint,
                host_v8_abi,
                env!("CARGO_MANIFEST_DIR"),
                usize_bytes,
            )
            .unwrap_or_else(|error| fail(&error));
            let snapshot_v8_abi = snapshot::validate_artifact(
                &generated.blob,
                &target,
                v8_version,
                &build_fingerprint,
                Some(host_v8_abi),
                usize_bytes,
            )
            .unwrap_or_else(|error| fail(&error));
            for file in &generated.files_loaded_during_snapshot {
                println!("cargo:rerun-if-changed={}", file.display());
            }
            println!(
                "cargo:rustc-env=OBSCURA_SNAPSHOT_PATH={}",
                generated.blob.display()
            );
            println!("cargo:rustc-env=OBSCURA_SNAPSHOT_NATIVE_WINDOW_ABI={snapshot_v8_abi}");
        }
        ArtifactSource::Prebuilt(blob) => {
            let expected_v8_abi = if is_cross { None } else { Some(host_v8_abi) };
            let snapshot_v8_abi = snapshot::validate_artifact(
                &blob,
                &target,
                v8_version,
                &build_fingerprint,
                expected_v8_abi,
                usize_bytes,
            )
            .unwrap_or_else(|error| fail(&error));
            println!("cargo:rerun-if-changed={}", blob.display());
            println!(
                "cargo:rerun-if-changed={}",
                snapshot::sidecar_path(&blob).display()
            );
            println!("cargo:rustc-env=OBSCURA_SNAPSHOT_PATH={}", blob.display());
            println!("cargo:rustc-env=OBSCURA_SNAPSHOT_NATIVE_WINDOW_ABI={snapshot_v8_abi}");
        }
    }
}

fn compile_incumbent_context() {
    // The native archive lives in obscura-v8-native so both this build script
    // process and the target runtime link the same C++ callback table.
    let version = deno_core::v8::V8::get_version();
    snapshot::ensure_v8_version(version).unwrap_or_else(|error| fail(&error));
}

fn target_usize_bytes() -> usize {
    let bits = env::var("CARGO_CFG_TARGET_POINTER_WIDTH")
        .expect("Cargo sets CARGO_CFG_TARGET_POINTER_WIDTH for build scripts");
    match bits.parse::<usize>() {
        Ok(bits @ (32 | 64)) => bits / 8,
        _ => fail(&format!(
            "unsupported target pointer width {bits:?}; deno_core snapshot envelopes \
             are supported on 32-bit and 64-bit targets"
        )),
    }
}

fn resolve_artifact(
    target: &str,
    is_cross: bool,
    workspace_root: &Path,
) -> Result<ArtifactSource, String> {
    let explicit_bin = env::var_os("OBSCURA_SNAPSHOT_BIN");
    let explicit_dir = env::var_os("OBSCURA_SNAPSHOT_DIR");
    if explicit_bin.is_some() && explicit_dir.is_some() {
        return Err(
            "set only one of OBSCURA_SNAPSHOT_BIN or OBSCURA_SNAPSHOT_DIR; \
             OBSCURA_SNAPSHOT_BIN names a snapshot blob, OBSCURA_SNAPSHOT_DIR names \
             the directory holding OBSCURA_SNAPSHOT.bin"
                .to_string(),
        );
    }
    if let Some(path) = explicit_bin {
        let blob = absolute(PathBuf::from(path))?;
        if !blob.is_file() {
            return Err(format!(
                "OBSCURA_SNAPSHOT_BIN={} does not exist; the blob and its \
                 {}.json sidecar must be produced by obscura-js-snapshot for {target}",
                blob.display(),
                blob.display()
            ));
        }
        return Ok(ArtifactSource::Prebuilt(blob));
    }

    let artifact_dir = match explicit_dir {
        Some(dir) => absolute(PathBuf::from(dir))?,
        None if is_cross => absolute(snapshot::artifact_dir(target, workspace_root))?,
        None => return Ok(ArtifactSource::Generate),
    };
    let blob = artifact_dir.join(snapshot::SNAPSHOT_FILE);
    if blob.is_file() {
        return Ok(ArtifactSource::Prebuilt(blob));
    }

    if let Some(producer) = env::var_os("OBSCURA_SNAPSHOT_PRODUCER") {
        run_producer(&producer, &artifact_dir)?;
        if !blob.is_file() {
            return Err(format!(
                "snapshot producer {} completed but did not write {}",
                Path::new(&producer).display(),
                blob.display()
            ));
        }
        return Ok(ArtifactSource::Prebuilt(blob));
    }

    if is_cross {
        Err(format!(
            "no target-native V8 snapshot for {target} at {}. The host build script \
             cannot create one: the blob and its global-template handlers are tied to \
             the target V8. On a native {target} runner (or a compatible runner) run:\n  \
             cargo run -p obscura-js-snapshot --release -- --out-dir {}\n\
             then copy that directory here, or set OBSCURA_SNAPSHOT_PRODUCER to a runner \
             that executes `obscura-js-snapshot --out-dir <directory>` for {target}.",
            artifact_dir.display(),
            artifact_dir.display()
        ))
    } else {
        Err(format!(
            "OBSCURA_SNAPSHOT_DIR={} does not contain {}. Generate it with \
             `cargo run -p obscura-js-snapshot --release -- --out-dir {}` or unset \
             OBSCURA_SNAPSHOT_DIR to generate the host snapshot inline.",
            artifact_dir.display(),
            snapshot::SNAPSHOT_FILE,
            artifact_dir.display()
        ))
    }
}

fn run_producer(producer: &OsStr, out_dir: &Path) -> Result<(), String> {
    let producer = absolute(PathBuf::from(producer))?;
    if !producer.is_file() {
        return Err(format!(
            "OBSCURA_SNAPSHOT_PRODUCER={} is not a file; point it at an \
             obscura-js-snapshot binary or an executable runner for the target",
            producer.display()
        ));
    }
    println!("cargo:rerun-if-changed={}", producer.display());
    let status = Command::new(&producer)
        .arg("--out-dir")
        .arg(out_dir)
        .status()
        .map_err(|error| {
            format!(
                "cannot start snapshot producer {}: {error}. A prebuilt target binary \
                 needs a runner for this host; otherwise copy the artifact directory \
                 and use OBSCURA_SNAPSHOT_DIR/OBSCURA_SNAPSHOT_BIN.",
                producer.display()
            )
        })?;
    if !status.success() {
        return Err(format!(
            "snapshot producer {} failed with {status}",
            producer.display()
        ));
    }
    Ok(())
}

/// `include_bytes!` in runtime.rs resolves relative paths against that source
/// file, so prebuilt artifact paths must be absolute before they are exported.
fn absolute(path: PathBuf) -> Result<PathBuf, String> {
    std::path::absolute(&path).map_err(|error| {
        format!(
            "cannot resolve snapshot artifact path {}: {error}",
            path.display()
        )
    })
}

fn workspace_root() -> Result<PathBuf, String> {
    let start = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut directory = start.as_path();
    loop {
        if directory.join("Cargo.lock").is_file() && directory.join("Cargo.toml").is_file() {
            return directory.canonicalize().map_err(|error| {
                format!("cannot resolve workspace root {}: {error}", directory.display())
            });
        }
        directory = directory.parent().ok_or_else(|| {
            format!(
                "cannot locate the workspace root above {}; set OBSCURA_SNAPSHOT_DIR \
                 to the directory holding the target snapshot artifact",
                start.display()
            )
        })?;
    }
}

fn fail(message: &str) -> ! {
    eprintln!("obscura-js build error: {message}");
    std::process::exit(1);
}
