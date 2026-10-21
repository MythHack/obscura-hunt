use openssl_src;
use std::path::PathBuf;

use super::env;

pub fn get_openssl(target: &str) -> (Vec<PathBuf>, PathBuf) {
    let openssl_config_dir = env("OPENSSL_CONFIG_DIR");

    let mut openssl_src_build = openssl_src::Build::new();
    if let Some(value) = openssl_config_dir {
        openssl_src_build.openssl_dir(PathBuf::from(value));
    }

    let is_msvc = target.contains("windows-msvc");
    let msvc_pdb = if is_msvc {
        let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR is not set"))
            .join("openssl-build");
        openssl_src_build.out_dir(&out_dir);
        Some(out_dir.join("build").join("src").join("ossl_static.pdb"))
    } else {
        None
    };
    let original_cflags = if is_msvc {
        let original = std::env::var_os("CFLAGS");
        let mut cflags = env("CFLAGS").unwrap_or_default();
        if !cflags.is_empty() {
            cflags.push(" ");
        }
        // OpenSSL's static install requires this PDB. Keep /Zi, but embed an
        // absolute path so Rust's rlib relocation does not lose the symbols.
        cflags.push("/Zi /Fd\"");
        cflags.push(msvc_pdb.as_ref().unwrap().as_os_str());
        cflags.push("\"");
        std::env::set_var("CFLAGS", cflags);
        original
    } else {
        None
    };

    let artifacts = openssl_src_build.build();
    if is_msvc {
        match original_cflags {
            Some(cflags) => std::env::set_var("CFLAGS", cflags),
            None => std::env::remove_var("CFLAGS"),
        }
    }
    if let Some(pdb) = msvc_pdb {
        // openssl-src installs the real compiler PDB, then removes build/src.
        // Retain that same artifact at the absolute path recorded in the objects.
        let installed_pdb = artifacts.lib_dir().join("ossl_static.pdb");
        let pdb_dir = pdb.parent().unwrap();
        std::fs::create_dir_all(pdb_dir).unwrap_or_else(|error| {
            panic!(
                "failed to create OpenSSL debug-symbol directory {}: {error}",
                pdb_dir.display()
            )
        });
        std::fs::copy(&installed_pdb, &pdb).unwrap_or_else(|error| {
            panic!(
                "failed to retain OpenSSL debug symbols from {} at {}: {error}",
                installed_pdb.display(),
                pdb.display()
            )
        });
    }

    println!("cargo:vendored=1");
    println!(
        "cargo:root={}",
        artifacts.lib_dir().parent().unwrap().display()
    );

    (
        vec![artifacts.lib_dir().to_path_buf()],
        artifacts.include_dir().to_path_buf(),
    )
}
