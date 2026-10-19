use std::path::{Path, PathBuf};

#[path = "../obscura-js/snapshot_fingerprint.rs"]
mod snapshot_fingerprint;

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let workspace = workspace_root(&manifest);
    // Keep this path in the same form Cargo gives obscura-js's build script
    // (`CARGO_MANIFEST_DIR`), so both producers pass identical
    // `cargo_manifest_dir` values to deno_core's create_snapshot.
    let js = manifest
        .parent()
        .expect("obscura-js-snapshot is in crates/")
        .join("obscura-js");
    let target = std::env::var("TARGET").expect("Cargo sets TARGET");
    let fingerprint = snapshot_fingerprint::build_fingerprint(&workspace)
        .unwrap_or_else(|error| panic!("obscura-js-snapshot build error: {error}"));
    for path in snapshot_fingerprint::input_paths(&workspace) {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    for key in snapshot_fingerprint::target_env_keys() {
        println!("cargo:rerun-if-env-changed={key}");
    }

    // The producer must stamp the architecture it executes on, not a
    // user-supplied string: a snapshot is only valid for the V8 it ran on.
    println!("cargo:rustc-env=OBSCURA_PRODUCER_TARGET={target}");
    println!(
        "cargo:rustc-env=OBSCURA_WORKSPACE_ROOT={}",
        workspace.display()
    );
    println!("cargo:rustc-env=OBSCURA_JS_MANIFEST_DIR={}", js.display());
    println!(
        "cargo:rustc-env=OBSCURA_SNAPSHOT_BUILD_FINGERPRINT={fingerprint}"
    );
    println!("cargo:rerun-if-changed=build.rs");
}

fn workspace_root(start: &Path) -> PathBuf {
    let mut directory = start;
    loop {
        if directory.join("Cargo.lock").is_file() && directory.join("Cargo.toml").is_file() {
            return directory
                .canonicalize()
                .unwrap_or_else(|_| directory.to_path_buf());
        }
        directory = directory
            .parent()
            .expect("obscura-js-snapshot must live inside the workspace");
    }
}
