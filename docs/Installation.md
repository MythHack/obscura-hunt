# Installation

## Current main: V8 152 from source

To use the current Obscura-Hunt engine, build
[`MythHack/obscura-hunt`](https://github.com/MythHack/obscura-hunt) from `main`.
It uses rusty_v8 **`152.2.0`**, reporting V8 **`15.2.124.1-rusty`**.
Install the prerequisites in [Build from source](Build-from-source.md), then:

```bash
git clone --branch main https://github.com/MythHack/obscura-hunt.git
cd obscura-hunt
cargo build --release --locked -p obscura-cli --bins --features render
```

The binaries are `target/release/obscura` and `target/release/obscura-worker`
(`.exe` on Windows). This command enables rendering; the other feature choices
are listed below. See [V8, SDK and snapshot alignment](Build-from-source.md#v8-version-sdk-and-snapshots)
for native dependency requirements and
[October 19 upgrade validation](Testing-and-debugging.md#upgrade-validation-october-19)
for the exercised configurations and results.

## Published binaries and source tags

Use the fork's [Releases page](https://github.com/MythHack/obscura-hunt/releases),
not the upstream project's packages. As of October 19, 2026, the latest public
release is [v0.1.1](https://github.com/MythHack/obscura-hunt/releases/tag/v0.1.1),
published October 11. Its binaries predate the V8 152 upgrade.

The `v0.1.2` source tag also predates the upgrade; its release entry is still a
draft, not a publicly available binary release. GitHub's source archives for
that tag are tagged sources, not current `main` or ready-to-run executables.
**Build current `main` for V8 152**; neither an older tag nor an upstream package
selects the current fork engine.

### Choose the platform

The public `v0.1.1` release contains these rendering-enabled default archives:

| Platform | Archive |
| --- | --- |
| Linux x86_64 | [`obscura-x86_64-linux.tar.gz`](https://github.com/MythHack/obscura-hunt/releases/download/v0.1.1/obscura-x86_64-linux.tar.gz) |
| Linux ARM64 | [`obscura-aarch64-linux.tar.gz`](https://github.com/MythHack/obscura-hunt/releases/download/v0.1.1/obscura-aarch64-linux.tar.gz) |
| macOS Apple Silicon | [`obscura-aarch64-macos.tar.gz`](https://github.com/MythHack/obscura-hunt/releases/download/v0.1.1/obscura-aarch64-macos.tar.gz) |
| macOS Intel | [`obscura-x86_64-macos.tar.gz`](https://github.com/MythHack/obscura-hunt/releases/download/v0.1.1/obscura-x86_64-macos.tar.gz) |
| Windows x86_64 | [`obscura-x86_64-windows.zip`](https://github.com/MythHack/obscura-hunt/releases/download/v0.1.1/obscura-x86_64-windows.zip) |

Choose the architecture that matches the machine running the browser. On Unix,
`uname -m` reports the host architecture; ARM64 is also called `aarch64` or
`arm64`. The Linux packages use GNU/glibc targets, not musl targets; do not assume
they run unchanged on Alpine Linux.

### Choose the features

All five platforms in `v0.1.1` have all four archive variants. Insert the suffix
before `.tar.gz` or `.zip`; for example,
`obscura-x86_64-linux-no-render-stealth.tar.gz`.

| Archive suffix | Rendering | Optional transport (`stealth`) | Current source-build flags |
| --- | --- | --- | --- |
| No suffix | Yes | No | `--features render` |
| `-stealth` | Yes | Yes | `--features render,stealth` |
| `-no-render` | No | No | `--no-default-features` |
| `-no-render-stealth` | No | Yes | `--no-default-features --features stealth` |

Apply the flags to `cargo build --release --locked -p obscura-cli --bins`.
Rendering enables layout, screenshots, CDP screencasting and PDF export.
No-render builds retain the CLI, CDP and MCP interfaces without those rendering
surfaces. See [Build from source](Build-from-source.md) for platform-specific
requirements and [Configure stealth and proxies](Configure-stealth-and-proxies.md)
for the optional transport configuration.

### Verify the archive before extracting

Download `SHA256SUMS.txt` from the **same fork release** as the archive. For
example, to verify the public `v0.1.1` Linux x86_64 default package:

```bash
curl -fLO https://github.com/MythHack/obscura-hunt/releases/download/v0.1.1/obscura-x86_64-linux.tar.gz
curl -fLO https://github.com/MythHack/obscura-hunt/releases/download/v0.1.1/SHA256SUMS.txt
sha256sum --ignore-missing -c SHA256SUMS.txt
```

Require an `OK` result for the archive you downloaded. On macOS, compute the
digest with `shasum -a 256 obscura-aarch64-macos.tar.gz` (or the Intel filename)
and compare it with that filename's entry in `SHA256SUMS.txt`. On Windows:

```powershell
Get-FileHash .\obscura-x86_64-windows.zip -Algorithm SHA256
Get-Content .\SHA256SUMS.txt | Select-String "obscura-x86_64-windows.zip"
```

The hash must match the entry for the exact platform and feature variant;
case differences in the hexadecimal digest do not matter. Do not use a checksum
file from another version or from upstream.

### Extract and keep the worker pair together

For the Linux example:

```bash
mkdir obscura-v0.1.1
tar xzf obscura-x86_64-linux.tar.gz -C obscura-v0.1.1
cd obscura-v0.1.1
./obscura --version
```

Use the same `tar` procedure with the matching macOS archive. On Windows:

```powershell
Expand-Archive .\obscura-x86_64-windows.zip -DestinationPath .\obscura-v0.1.1
Set-Location .\obscura-v0.1.1
.\obscura.exe --version
```

The `v0.1.1` archives contain `obscura` and `obscura-worker` (both `.exe` on
Windows). The worker supports parallel `scrape` operations. Keep the two
executables from the **same archive** in the same directory; do not mix
platforms, feature variants or versions. For source builds, keep the pair from
the same build together as well.

### Inspect current build provenance

Current source builds support `--build-info`:

```bash
./target/release/obscura --build-info
```

On Windows use `.\target\release\obscura.exe --build-info`. Inspect the JSON
`source.repository`, `source.commit`, `source.dirty`, `target` and
`features.render` / `features.stealth`. They should identify the intended fork
checkout, target and selected features. A dirty build is not the same source
identity as a clean tagged or committed build. Do not rely on the short
`--version` string alone to establish build provenance.

The historical `v0.1.1` archives do **not** contain `build-info.json`; the
instructions above do not assume that those binaries support `--build-info`.
Build current source when you need the current engine and its provenance
support.

## Local smoke test

From the extracted archive directory (or using `target/release/obscura` for a
source build), exercise JavaScript without depending on a remote site:

```bash
./obscura fetch 'data:text/html,<title>ok</title>' --eval "document.title"
```

The final evaluation output should be `ok`. For a rendering-enabled variant only:

```bash
./obscura fetch 'data:text/html,<title>ok</title><p>render-ok</p>' --screenshot smoke.png
```

This should create a PNG showing `render-ok`, not a blank page. On Windows use
`.\obscura.exe` in place of `./obscura`. No-render variants do not support screenshots.

## Upstream distributions: AUR and Docker

The AUR package `obscura-browser` and the Docker image
[`h4ckf0r0day/obscura`](https://hub.docker.com/r/h4ckf0r0day/obscura) distribute
the upstream project, not Obscura-Hunt. They are not installation methods for
this fork or its current V8 152 engine. This guide does not claim a fork AUR
package or published fork container image; use the fork's public release
assets or build its source instead.

## Troubleshooting

- **Cannot execute binary / wrong architecture:** check the downloaded filename
  against the host architecture and operating system.
- **Missing glibc symbols:** the Linux GNU binary is incompatible with the host
  runtime. Use a compatible host or build from source for the intended system;
  an upstream Docker image is not an equivalent fork build.
- **Missing screenshots, layout or PDF:** check the feature variant. Select a
  rendering-enabled archive or build with `--features render`.
- **Native build failures:** follow the platform prerequisites in
  [Build from source](Build-from-source.md), including the Windows libclang
  guidance for builds that enable the optional transport.
