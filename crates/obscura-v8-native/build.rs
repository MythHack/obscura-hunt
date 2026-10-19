use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let native = manifest.join("../obscura-js/native");
    let include = native.join("v8-152.2.0/include");

    println!("cargo:rerun-if-changed={}", native.join("incumbent_context.cc").display());
    println!("cargo:rerun-if-changed={}", native.join("window_proxy.cc").display());
    println!("cargo:rerun-if-changed={}", include.display());

    let mut build = cc::Build::new();
    build
        .cpp(true)
        .std("c++20")
        .file(native.join("incumbent_context.cc"))
        .file(native.join("window_proxy.cc"))
        .cpp_link_stdlib(None);

    // Match rusty_v8 152.2.0's .gn defaults and V8's external_config.
    // MicrotaskQueue is cppgc-managed in the release archive; omitting this
    // define would declare the old owning factory and a different vtable.
    build
        .define("V8_CPPGC_MICROTASK_QUEUE", None)
        .define("V8_ARRAY_BUFFER_INTERNAL_FIELD_COUNT", "2")
        .define("V8_ARRAY_BUFFER_VIEW_INTERNAL_FIELD_COUNT", "2")
        .define("V8_PROMISE_INTERNAL_FIELD_COUNT", "1")
        .define("V8_USE_DEFAULT_HASHER_SECRET", "0");

    // Keep the helper's V8 type layout identical to the archive selected by
    // deno_core's `v8_enable_pointer_compression` feature. The prebuilt V8
    // configuration enables the shared cage and 31-bit Smis with compression;
    // deno_core 0.412 exposes no sandbox feature, so none is invented here.
    if std::env::var_os("CARGO_FEATURE_V8_ENABLE_POINTER_COMPRESSION").is_some() {
        build
            .define("V8_COMPRESS_POINTERS", None)
            .define("V8_COMPRESS_POINTERS_IN_SHARED_CAGE", None)
            .define("V8_31BIT_SMIS_ON_64BIT_ARCH", None);
    }

    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        let features = std::env::var("CARGO_CFG_TARGET_FEATURE").unwrap_or_default();
        build
            .static_crt(features.split(',').any(|feature| feature == "crt-static"))
            .flag("/Zc:__cplusplus")
            .flag("/Zl");
    }

    // Only the pinned SDK is external; keep warnings enabled for our shims.
    if build.get_compiler().is_like_msvc() {
        let mut external_include = std::ffi::OsString::from("/external:I");
        external_include.push(&include);
        build.flag(external_include).flag("/external:W0");
    } else {
        build.flag("-isystem").flag(&include);
    }

    build.compile("obscura_v8_native");
}
