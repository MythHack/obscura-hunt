## Requirements

- Rust 1.75+ ([rustup.rs](https://rustup.rs))
- C compiler (gcc or clang)
- ~5 GB free disk space (V8 compiles from source on first build)

Build time depends on the machine and cache state. Cargo reuses unchanged
dependencies, but a release source edit can still require substantial compilation
and linking. For the scoped incremental runtime workflow, see
[Fast edit/test loop](Testing-and-debugging.md#fast-edittest-loop).

## Build

```bash
git clone https://github.com/MythHack/obscura-hunt.git
cd obscura-hunt
export RUSTC_WRAPPER=sccache
cargo build --release -p obscura-cli --bins --features render
```

Binary is at `./target/release/obscura`.

This produces the release binary with geometry, screenshots, screencasting,
and PDF export.

## Rendering and stealth

```bash
cargo build --release -p obscura-cli --bins --features render,stealth
```

This is the complete rendering build with the stealth wreq/BoringSSL transport,
TLS fingerprint randomization, browser-identity protections, and tracker
blocklist. See [Configure stealth and proxies](Configure-stealth-and-proxies.md).

### Fork dependency patches

This fork selects its local dependency patches through the workspace
`[patch.crates-io]`; Cargo builds them automatically, in place of the original
packages. They are not additional browser processes or separately built modules.

`vendor/deno_core-0.412.0` supplies the native `BrowserRealm` interface used by
related windows. Each document has its own module registry inside the shared V8
isolate, and navigating or closing one document retires only its module work.
The original dependency keeps `JsRealm` and `ModuleMap` private, so this integration
cannot be implemented through its public interface alone. Keep the patch until
an upstream interface can replace it without losing those browser semantics.

Use the scoped `-p obscura-cli --bins` commands above to build `obscura` and
`obscura-worker`. An unscoped workspace build also builds auxiliary programs such
as `paint_file`, the upstream HTML-to-PNG rendering harness; HuntForge does not
require that executable.


## Without rendering

```bash
cargo build --release -p obscura-cli --bins --no-default-features
cargo build --release -p obscura-cli --bins --no-default-features --features stealth
```

The second command keeps stealth while excluding layout, screenshots,
screencasting, and PDF export.

The stealth feature builds BoringSSL and generates Rust bindings. In addition
to the default requirements, install CMake, Clang, and the libclang/LLVM
development libraries. On Ubuntu/Debian:

```bash
sudo apt-get install build-essential cmake clang libclang-dev llvm-dev
```

On macOS, install the Xcode Command Line Tools and CMake. On Windows, install
the Visual Studio C++ Build Tools, CMake, and LLVM/Clang. Ensure the directory
containing `libclang` is available through `LIBCLANG_PATH` if bindgen cannot
locate it automatically.

On macOS 26 with the standalone Command Line Tools, Apple Clang may not find
libc++ while compiling BoringSSL. Use the active SDK for that build:

```bash
SDK_PATH="$(xcrun --show-sdk-path)"
SDKROOT="$SDK_PATH" CXXFLAGS="-isystem $SDK_PATH/usr/include/c++/v1" \
  cargo build --release -p obscura-cli --bins --features render,stealth
```

## OpenSSL on older systems

If the build fails on the vendored OpenSSL with an AVX-512 assembler error (common on older VPS hosts):

```bash
OPENSSL_NO_VENDOR=1 cargo build --release -p obscura-cli --bins --features render
```

Uses the system OpenSSL instead.

## Run from the build

```bash
./target/release/obscura --version
./target/release/obscura fetch https://example.com --eval "document.title"
```

Install system-wide:

```bash
cargo install --path crates/obscura-cli --features render
```

## Tests

```bash
cargo nextest run --release --features render --no-fail-fast
```

Integration suite:

```bash
python3 tests/test_all.py
```

Use `cargo nextest`, not `cargo test`: runtime tests require process isolation
because the engine owns a single V8 isolate per process.
