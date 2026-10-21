## Requirements

- Current stable Rust ([rustup.rs](https://rustup.rs)); the V8 dependency uses Rust 2024
- C++20 compiler (GCC, Clang, or Visual Studio C++ Build Tools)
- Several GB of free disk space for native dependencies and release artifacts
- Public-key WebCrypto builds maintained vendored OpenSSL 3 sources. Windows
  builds need Perl, NASM and the MSVC C/C++ environment; Unix builds need Perl and
  a supported C compiler. Secret-key algorithms retain their existing RustCrypto
  implementations.
- Stealth additionally needs CMake, Clang and libclang/LLVM development libraries.
  Its btls/BoringSSL symbol-prefix build needs Go 1.24 or newer on `PATH` on
  Windows and macOS.

Build time depends on the machine and cache state. Cargo reuses unchanged
dependencies, but a release source edit can still require substantial compilation
and linking. For the scoped incremental runtime workflow, see
[Fast edit/test loop](Testing-and-debugging.md#fast-edittest-loop).

For the current V8 152 engine, build a recorded revision of this fork's `main`
branch. Older tagged release archives do not acquire later engine changes.
See [Installation](Installation.md) for public package availability and
[upgrade validation](Testing-and-debugging.md#upgrade-validation-october-19)
for the exercised source revision.

JavaScript `crypto.subtle` supports RSA-OAEP, RSASSA-PKCS1-v1_5, RSA-PSS,
ECDSA/ECDH (P-256/P-384/P-521), Ed25519 and X25519: native key generation,
SPKI/PKCS8/JWK import/export (plus raw EC/OKP public keys), signatures,
encryption/agreement, derived AES/HMAC keys and existing AES/RSA wrapping paths.
Private material and immutable key policy remain in native garbage-collected
handles, including structured clones. Native key generation is admission-bounded
and runs off the V8 callback thread; key and message sizes are bounded separately
from script watchdogs.
RSA keys are capped at 16,384 bits, encoded key imports at 64 KiB, and signed or
verified messages and OAEP labels at 16 MiB. Two native key-generation jobs may
run concurrently; admission and result waits each have a 15-second deadline.
Raw private keys, raw RSA keys, unsupported algorithms/curves/hashes, invalid
key usages, incompatible JWK policy and non-extractable export/wrap requests are
rejected rather than emulated. These are implementation resource bounds, not a
claim that every WebCrypto extension or arbitrary-size native key is supported.

## Build

```bash
git clone https://github.com/MythHack/obscura-hunt.git
cd obscura-hunt
export RUSTC_WRAPPER=sccache
cargo build --release --locked -p obscura-cli --bins --features render
```

Binary is at `./target/release/obscura`.

This produces the release binary with geometry, screenshots, screencasting,
and PDF export.

## Rendering and stealth

```bash
cargo build --release --locked -p obscura-cli --bins --features render,stealth
```

This is the complete rendering build with the stealth wreq/BoringSSL transport,
TLS fingerprint randomization, browser-identity protections, and tracker
blocklist. See [Configure stealth and proxies](Configure-stealth-and-proxies.md).

### Windows (PowerShell)

Install the Visual Studio C++ Build Tools, CMake, LLVM, Perl and NASM. For
stealth, also install Go 1.24 or newer on `PATH`. The stealth transport uses
bindgen, which must load `libclang.dll`; having `clang.exe` on `PATH` alone does
not guarantee that the DLL can be found. Set `LIBCLANG_PATH` to the directory
containing the DLL, not to the DLL file. For an LLVM installation at `E:\LLVM`:

```powershell
$env:RUSTC_WRAPPER = "sccache"
$env:LIBCLANG_PATH = "E:\LLVM\bin" # Replace with your actual LLVM bin directory.
cargo build --release --locked -p obscura-cli --bins --features render,stealth
```

These variables apply to the current terminal. If BoringSSL produced `crypto.lib`
and `ssl.lib` but bindgen reports `Unable to find libclang`, fix this DLL path and
rerun the build. Do not run `cargo clean`: the completed native libraries remain
reusable.

To avoid setting the variable in every terminal, add a machine-local default to
`$CARGO_HOME/config.toml` (normally `~/.cargo/config.toml`), merging with an existing
`[env]` table if present:

```toml
[env]
LIBCLANG_PATH = { value = "E:/LLVM/bin", force = false }
```

Use your actual LLVM directory. Explicit shell values still take precedence.
Do not commit an absolute machine path to the project's `.cargo/config.toml`:
Cargo applies `[env]` on every platform, and an explicit `LIBCLANG_PATH` disables
clang-sys's normal library search outside that directory.

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

`vendor/deno_v8-0.4.0` is the second engine-related patch. It pins the underlying
raw `v8` package to `=152.2.0` for both `deno_core` and `serde_v8`; keep it together
with the browser-realm patch. The following section describes the matching SDK,
native ABI and snapshots.

The network patches are `vendor/rustls-platform-verifier-0.7.1`,
`vendor/wreq-6.0.0-rc.31` and `vendor/btls-sys-0.5.6`. They retain the selected
transport's certificate validation and bounded missing-intermediate recovery.
With stealth enabled, BoringSSL symbols are isolated from the OpenSSL backend
used by public-key WebCrypto. On Windows (PE) and Apple targets (Mach-O), the
btls build first compiles bootstrap archives, uses the bundled Go
`read_symbols.go` utility to collect symbols, then rebuilds through BoringSSL's
native CMake prefix mechanism, including assembly symbols. No external
`objcopy` replacement is used on those platforms. Linux/Android retain the
existing `nm`/`objcopy` path. This describes the build implementation, not a
claim of runtime verification on each platform.

### V8 version, SDK and snapshots

The locked dependency chain is `deno_core 0.412.0` and `serde_v8 0.321.0`, both
using the `deno_v8 0.4.0` facade. The local facade patch pins the underlying
`v8` package (rusty_v8) to **exactly `152.2.0`**, whose engine reports
**V8 `15.2.124.1-rusty`**. `deno_v8 0.4.0` is a Rust facade version, not a Deno CLI
version. Deno CLI `2.9.7` uses this same core/serde/facade family, but its
unpatched facade selects rusty_v8 `150.4.0`; installing that CLI does not select
Obscura's engine.

The core's optional `include_icu_data` feature uses `deno_core_icudata 0.78.0`
and V8's ICU 78 data-loading API. ICU 77 data must not be paired with this engine.

The native SDK is pinned at `crates/obscura-js/native/v8-152.2.0/include`.
Its include tree comes unchanged from the official `v8 152.2.0` crate, with the
V8 license from commit `c4ca1eccb90c5464d826b7713dc27178a53b0cfe` alongside it.
Keep the SDK, C++ shims, patched facade, build-time V8, standalone snapshot
producer and runtime consumer aligned on this engine and native ABI, including
pointer-compression settings. Updating only `Cargo.lock` or only the headers is
not a supported engine upgrade.

By default rusty_v8 downloads a matching prebuilt archive; building Obscura from
source does not mean compiling V8 itself. `V8_FROM_SOURCE=1` requests a V8 source
build. In `152.2.0`, simdutf is always built in: the Cargo `simdutf` feature is a
compatibility no-op, and archive names no longer vary by simdutf selection.
Any archive or mirror override must supply the matching `152.2.0` target and ABI,
not an older cached archive.

Snapshots must be regenerated for this engine. Native builds generate them
automatically; cross builds require the target-native producer's blob and JSON
sidecar from the same sources and configuration. See
[V8 dependency and snapshot alignment](Testing-and-debugging.md#v8-dependency-and-snapshot-alignment).
Embedded Rust consumers must also select these patches at their own Cargo
workspace root; see [Add the dependency](Use-as-a-Rust-library.md#add-the-dependency).

Use the scoped `-p obscura-cli --bins` commands above to build `obscura` and
`obscura-worker`. An unscoped workspace build also builds auxiliary programs such
as `paint_file`, the upstream HTML-to-PNG rendering harness. Those auxiliary
programs are not required to use the browser CLI.


## Without rendering

```bash
cargo build --release -p obscura-cli --bins --no-default-features
cargo build --release -p obscura-cli --bins --no-default-features --features stealth
```

The second command keeps stealth while excluding layout, screenshots,
screencasting, and PDF export.

The renderer crate separately supports layout without its `paint` feature:

```bash
cargo check --release -p obscura-render --no-default-features
```

That layout-only build does not load page fonts. Its ascent/descent proportions
come from the bundled sans fallback, normalized to the existing one-em font box;
CSS line-height and leading remain separate. Paint builds use selected-face metrics.

The stealth feature builds BoringSSL and generates Rust bindings. In addition
to the default requirements, install CMake, Clang, and the libclang/LLVM
development libraries. On Ubuntu/Debian:

```bash
sudo apt-get install build-essential perl cmake clang libclang-dev llvm-dev
```

On macOS, install the Xcode Command Line Tools, CMake, Perl, and, for stealth,
Go 1.24 or newer. For the Windows toolchain and DLL search path, see
[Windows (PowerShell)](#windows-powershell).

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

For focused browser and CDP integration coverage, select the affected crates:

```bash
cargo nextest run --release --locked --features render -p obscura-browser -p obscura-cdp --no-fail-fast
```

For actual CLI/CDP/MCP behavior, reproduce the scenario with the release
executable and a local fixture. See [Testing and debugging](Testing-and-debugging.md)
for the runtime and rendering workflow.

Use `cargo nextest`, not `cargo test`: runtime tests require process isolation
because the engine owns a single V8 isolate per process.
