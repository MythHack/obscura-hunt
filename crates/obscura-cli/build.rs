use std::env;
use std::path::Path;

fn git_output(root: &Path, arguments: &[&str]) -> Option<String> {
    std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
}

fn git_value(root: &Path, arguments: &[&str]) -> Option<String> {
    git_output(root, arguments)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty() && !value.contains('\r') && !value.contains('\n'))
}

fn env_value(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn normalize_version(version: String) -> String {
    version.strip_prefix('v').unwrap_or(&version).to_string()
}

fn github_tag_version() -> Option<String> {
    if env_value("GITHUB_REF_TYPE").as_deref() == Some("tag") {
        return env_value("GITHUB_REF_NAME").map(normalize_version);
    }

    env_value("GITHUB_REF")
        .and_then(|value| value.strip_prefix("refs/tags/").map(str::to_owned))
        .map(normalize_version)
}

/// Local builds are not cut from a tag, so the bare workspace version cannot
/// tell two checkouts apart. Append the commit when git is there to answer;
/// tarball and minimal-image builds without git keep the bare version.
fn local_version(sha: Option<&str>) -> String {
    let package = env::var("CARGO_PKG_VERSION").expect("Cargo sets CARGO_PKG_VERSION");
    match sha {
        Some(sha) => format!("{package}-dev+{sha}"),
        None => package,
    }
}

fn main() {
    let manifest = env::var("CARGO_MANIFEST_DIR").expect("Cargo sets CARGO_MANIFEST_DIR");
    let root = Path::new(&manifest).parent().and_then(Path::parent)
        .expect("obscura-cli is in crates/obscura-cli");
    // Include Git HEAD/ref changes and dirty source edits, not only build.rs.
    // A cached build script must never report a previous checkout as this build.
    println!("cargo:rerun-if-changed={}", root.display());
    let commit = git_value(root, &["rev-parse", "HEAD"]);
    let short = git_value(root, &["rev-parse", "--short", "HEAD"]);
    let repository = git_value(root, &["config", "--get", "remote.origin.url"]);
    let dirty = git_output(root, &["status", "--porcelain", "--untracked-files=all"])
        .map(|status| if status.trim().is_empty() { "false" } else { "true" })
        .unwrap_or("unknown");
    println!("cargo:rerun-if-env-changed=OBSCURA_VERSION");
    println!("cargo:rerun-if-env-changed=GITHUB_REF_TYPE");
    println!("cargo:rerun-if-env-changed=GITHUB_REF_NAME");
    println!("cargo:rerun-if-env-changed=GITHUB_REF");

    let version = env_value("OBSCURA_VERSION")
        .map(normalize_version)
        .or_else(github_tag_version)
        .unwrap_or_else(|| local_version(short.as_deref()));

    println!("cargo:rustc-env=OBSCURA_BUILD_VERSION={version}");
    println!("cargo:rustc-env=OBSCURA_BUILD_COMMIT={}", commit.as_deref().unwrap_or(""));
    println!("cargo:rustc-env=OBSCURA_BUILD_REPOSITORY={}", repository.as_deref().unwrap_or(""));
    println!("cargo:rustc-env=OBSCURA_BUILD_DIRTY={dirty}");
    println!("cargo:rustc-env=OBSCURA_BUILD_TARGET={}", env::var("TARGET").expect("Cargo sets TARGET"));
}
