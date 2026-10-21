# Low-level bindings to BoringSSL

[BoringSSL](https://boringssl.googlesource.com/boringssl) is Google's fork of OpenSSL for Chrome/Chromium and Android.

This crate builds the BoringSSL library (or optionally links a pre-built version) and generates FFI bindings for it.
It supports [FIPS-compatible builds of BoringSSL](https://boringssl.googlesource.com/boringssl/+/master/crypto/fipsmodule/FIPS.md),
as well as [Post-Quantum crypto](https://datatracker.ietf.org/doc/draft-ietf-tls-ecdhe-mlkem/)
and [Raw Public Key](https://docs.rs/btls/latest/btls/ssl/struct.SslRef.html#method.peer_pubkey) extensions.

To use BoringSSL from Rust, prefer the [higher-level safe API](https://docs.rs/btls).

## Native symbol prefixing

The vendored Windows/macOS/iOS `prefix-symbols` build requires Go on `PATH`
(Go 1.24 or newer, as specified by the bundled BoringSSL module). It first builds
unprefixed libraries in a separate bootstrap output directory, collects PE or
Mach-O symbols with BoringSSL's Go utility, then lets CMake prefix native symbols,
including Windows NASM assembly, in the final libraries. Bindgen continues to
read ordinary headers and uses the existing link-name callback. Linux/Android
prefixing is unchanged.

The PE/COFF collector keeps section-defined external symbols with strong
linkage. Mergeable COMDAT definitions, including CRT inline functions and
compiler-generated constants, remain unprefixed; `NODUPLICATES` COMDAT functions
remain eligible. Mach-O symbol collection and its leading-underscore handling
are unchanged. Each native source build regenerates the symbol list from the
bootstrap archives, and CMake regenerates its C, assembly, and NASM prefix
headers from that list.

Applying bundled source patches is normal build progress, not a Cargo warning.
It is recorded on the build script's stderr; actionable build warnings retain
their Cargo warning diagnostics.

## Contribution

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the Apache-2.0
license, shall be dual licensed under the terms of both the Apache License,
Version 2.0 and the MIT license without any additional terms or conditions.
