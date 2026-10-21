//! Process-wide font policy shared by the CLI and the scrape worker.
//!
//! Native font selection caches its database at the first metric lookup or
//! render, so policy is applied once before any page is created. Every entry
//! point validates the requested directories first: an explicit bad path is a
//! command error, never an ignored option.

use std::path::PathBuf;

/// Environment variable the scrape parent uses to forward `--font-dir`
/// values to its worker processes, as a JSON array of paths.
pub const FONT_DIRS_ENV: &str = "OBSCURA_FONT_DIRS";

/// Environment variable the scrape parent uses to forward
/// `--no-system-fonts` to its worker processes.
pub const NO_SYSTEM_FONTS_ENV: &str = "OBSCURA_NO_SYSTEM_FONTS";

/// Apply the font policy for this process.
///
/// Additional directories are loaded on top of the browser-like default: the
/// operating system's installed fonts plus the bundled faces, unless
/// `no_system_fonts` suppresses the former. Returns a message when the policy
/// cannot be applied, including a directory that does not exist and a call
/// made after the font database has already been initialized.
pub fn configure(font_dirs: &[PathBuf], no_system_fonts: bool) -> Result<(), String> {
    for directory in font_dirs {
        if !directory.is_dir() {
            return Err(format!(
                "Font directory does not exist or is not a directory: {}",
                directory.display()
            ));
        }
    }

    if obscura_js::configure_fonts(font_dirs.to_vec(), !no_system_fonts) {
        return Ok(());
    }
    Err("Font configuration must be applied before the first font lookup; the font database is already initialized".to_string())
}

/// Read the scrape worker's font policy from the environment and apply it.
///
/// The parent validates the CLI arguments before spawning workers, so a
/// malformed value here means the environment was replaced or a requested
/// directory disappeared between validation and startup. Both are reported
/// rather than replaced with a different policy.
// The CLI binary compiles this module too, but only the worker entry point
// reads the forwarded policy from the environment.
#[allow(dead_code)]
pub fn configure_from_env() -> Result<(), String> {
    let font_dirs = match std::env::var(FONT_DIRS_ENV) {
        Ok(raw) if !raw.trim().is_empty() => {
            serde_json::from_str::<Vec<PathBuf>>(&raw).map_err(|error| {
                format!("Invalid {} value {:?}: {}", FONT_DIRS_ENV, raw, error)
            })?
        }
        _ => Vec::new(),
    };
    configure(&font_dirs, env_flag(NO_SYSTEM_FONTS_ENV))
}

fn env_flag(name: &str) -> bool {
    std::env::var(name)
        .map(|value| matches!(value.trim(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}
