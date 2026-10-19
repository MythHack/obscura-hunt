//! Target-native startup-snapshot producer for `obscura-js`.
//!
//! The crate build script can create the snapshot only when it runs on the
//! target architecture. This binary mounts exactly the same native Window
//! extension, bootstrap sources, and generation code, so an artifact produced
//! here is byte-compatible with an inline host snapshot and carries the
//! metadata a cross build validates before embedding it.

use std::path::{Path, PathBuf};

#[path = "../../obscura-js/src/native_window.rs"]
mod native_window;
#[path = "../../obscura-js/snapshot_generation.rs"]
mod snapshot_generation;

fn main() {
    if let Err(error) = run() {
        eprintln!("obscura-js-snapshot: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut out_dir: Option<PathBuf> = None;
    let mut arguments = std::env::args_os().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.to_str() {
            Some("--out-dir") => {
                let value = arguments
                    .next()
                    .ok_or_else(|| "--out-dir requires a directory".to_string())?;
                out_dir = Some(PathBuf::from(value));
            }
            Some("--print-target") => {
                println!("{}", env!("OBSCURA_PRODUCER_TARGET"));
                return Ok(());
            }
            Some("--help" | "-h") => {
                print_help();
                return Ok(());
            }
            _ => {
                return Err(format!(
                    "unrecognized argument {}\n\n{}",
                    argument.to_string_lossy(),
                    USAGE
                ));
            }
        }
    }

    let target = env!("OBSCURA_PRODUCER_TARGET");
    let out_dir = out_dir.unwrap_or_else(|| {
        snapshot_generation::artifact_dir(
            target,
            Path::new(env!("OBSCURA_WORKSPACE_ROOT")),
        )
    });
    let usize_bytes = std::mem::size_of::<usize>();
    let v8_version = deno_core::v8::V8::get_version();
    let build_fingerprint = env!("OBSCURA_SNAPSHOT_BUILD_FINGERPRINT");
    let v8_abi = obscura_v8_native::native_window_abi();
    snapshot_generation::ensure_v8_version(v8_version)?;

    let generated = snapshot_generation::generate_artifact(
        &out_dir,
        target,
        build_fingerprint,
        v8_abi,
        env!("OBSCURA_JS_MANIFEST_DIR"),
        usize_bytes,
    )?;
    snapshot_generation::validate_artifact(
        &generated.blob,
        target,
        v8_version,
        build_fingerprint,
        Some(v8_abi),
        usize_bytes,
    )?;

    for file in &generated.files_loaded_during_snapshot {
        eprintln!(
            "obscura-js-snapshot: note: snapshot loaded {} from disk; the consumer \
             cannot track it",
            file.display()
        );
    }
    println!("target={target}");
    println!("v8={v8_version}");
    println!("snapshot={}", generated.blob.display());
    println!("sidecar={}", snapshot_generation::sidecar_path(&generated.blob).display());
    Ok(())
}

const USAGE: &str = "Usage: obscura-js-snapshot [--out-dir DIR] [--print-target]

Writes OBSCURA_SNAPSHOT.bin and its identity sidecar OBSCURA_SNAPSHOT.bin.json.
The artifact is valid only for the target triple this binary was built for.
Without --out-dir the artifact goes to
$CARGO_TARGET_DIR/obscura-snapshot/<target> (or <workspace>/target/...).";

fn print_help() {
    println!("obscura-js-snapshot - produce the target-native obscura-js V8 snapshot\n");
    println!("{}", USAGE);
    println!(
        "\nConsume it from a build on another machine with:\n  \
         OBSCURA_SNAPSHOT_DIR=<artifact directory> cargo build -p obscura-js\n\
         or\n  OBSCURA_SNAPSHOT_BIN=<path to OBSCURA_SNAPSHOT.bin> cargo build -p obscura-js"
    );
}
