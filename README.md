# Obscura-Hunt

**English** | [简体中文](README.zh-CN.md)

**Obscura-Hunt** is an independent headless browser written in Rust for web
scraping, browser automation, and AI agents. It runs real JavaScript via V8,
renders pages natively, and provides CDP and MCP interfaces, including support
for Puppeteer and Playwright clients.

Choose it for workflows that move between popups, embedded pages, and navigation,
not just a single page fetch. Maintained by
[MythHack](https://github.com/MythHack/obscura-hunt), Obscura-Hunt builds on
[official Obscura](https://github.com/h4ckf0r0day/obscura), keeps its native engine
and stealth options, and brings upstream updates together with its own browser
workflow improvements.

The latest upstream sync reaches
[`8b49681`](https://github.com/h4ckf0r0day/obscura/commit/8b49681), adding 14 commits
since [`bd95294`](https://github.com/h4ckf0r0day/obscura/commit/bd9529418b7fa975b50eabeac415cfaaa865a588).
It adds same-document fragment navigation, ordered-list attribute reflection,
AES-KW wrapping, native JWK processing and page-task progression during MCP
function waits, while preserving native popup and child-frame ownership.
The current branch also upgrades raw
`rusty_v8` from `150.4.0` to **`152.2.0`** (engine **`15.2.124.1-rusty`**).
The native SDK, C++ shims, startup snapshots and optional ICU 78 data are aligned.
See the [latest upstream validation](docs/Testing-and-debugging.md#upstream-sync-8b49681)
for this integration. The separate
[October 19 V8 validation](docs/Testing-and-debugging.md#upgrade-validation-october-19)
and [V8 upgrade CI run](https://github.com/MythHack/obscura-hunt/actions/runs/39230174839)
cover the earlier engine migration.
The [merge-stage report](docs/Testing-and-debugging.md#merge-stage-record-upstream-bd95294)
is retained as a historical record, not the current validation status.

## Why choose Obscura-Hunt?

If you already use official Obscura, these improvements help your automation
complete multi-step workflows without changing your CDP or MCP client:

- **Keep popup workflows connected.** Open another window, exchange messages
  with the original page, and detect when it closes. Navigating or closing one
  page leaves the other pages' JavaScript modules intact.
- **Automate real embedded documents.** Default and child-frame isolated worlds
  keep their own globals and prototypes while sharing the native DOM. Clicks,
  focus, form state, and event cancellation stay connected across worlds;
  cross-origin and page-embedding protections remain in place.
- **Wait for results across navigation.** Timers and asynchronous scripts keep
  running after a page loads a new URL. MCP waits give the page time to execute
  the work you are waiting for.
- **Click where the page actually draws.** CSS `zoom`, transformed iframe
  coordinates, and 3D `perspective` use the same native geometry for layout,
  projected painting, element bounds, and hit testing.
- **Keep request interception predictable.** Popups inherit the selected URL
  filters instead of unexpectedly pausing every request.
- **Use real font-loading results.** SFNT, WOFF, and WOFF2 fonts are decoded and
  validated; malformed sources reject instead of becoming fake loaded fonts.
  Font subsets are selected from the text the document actually uses.
- **Get meaningful Web API state.** `DOMMatrix` operations, visibility checks,
  writable media properties, and live text tracks reflect their actual state.

Keep using Puppeteer, Playwright, or an MCP agent with the native Rust/V8 engine,
screenshots, PDF output, and optional stealth transport inherited from upstream.
This project adds the workflow improvements above, refreshes key dependencies,
and continues integrating official Obscura updates.

## Dependency upgrades

Compared with official Obscura commit
[`2bda71a`](https://github.com/h4ckf0r0day/obscura/commit/2bda71a557247bd2d6fabf01fa66eb4b07dcb2e8),
this fork includes the following dependency upgrades. Versions below come from
the committed lockfiles and describe direct browser dependencies, not every
transitive update.

| Area | Version changes |
| --- | --- |
| JavaScript engine | raw `v8` (rusty_v8) 150.4.0 → 152.2.0 |
| HTML parsing | `html5ever` 0.39.0 → 0.40.1; `markup5ever` 0.39.0 → 0.40.0 |
| CSS and selectors | `cssparser` 0.34.0 → 0.38.0; `selectors` 0.26.0 → 0.41.0; `servo_arc` 0.4.3 → 0.5.0 |
| HTTP and CDP transport | `reqwest` 0.12.28 → 0.13.5; `ureq` 2.12.1 → 3.4.2; `tokio-tungstenite` 0.26.2 → 0.30.0 |
| Stealth transport | `wreq` 6.0.0-rc.29 → 6.0.0-rc.31; `wreq-util` 3.0.0-rc.12 → 3.0.0-rc.14 |
| Font and SVG rendering | `swash` 0.2.0 → 0.2.10; `resvg` and `usvg` 0.47.0 → 0.48.1 |
| WebCrypto hashes and MACs | `sha1` 0.10.6 → 0.11.0; `sha2` 0.10.9 → 0.11.0; `hmac` 0.12.1 → 0.13.0 |
| WebCrypto encryption | `aes` 0.8.4 → 0.9.3; `aes-gcm` 0.10.3 → 0.11.1; `cbc` 0.1.2 → 0.2.1; `ctr` 0.9.2 → 0.10.1 |
| Key derivation and randomness | `pbkdf2` 0.12.2 → 0.13.0; `hkdf` 0.12.4 → 0.13.0; `getrandom` 0.2.17 → 0.4.3 |
| Shared utilities | `base64` 0.22.1 → 0.23.1; `thiserror` 2.0.18 → 2.0.21; the library crate also moves from `thiserror` 1.x to 2.x |
| TLS test support | `rcgen` 0.13.2 → 0.14.10; `tokio-rustls` 0.26.4 → 0.26.6 |
| Native build support | `cc` 1.2.64 → 1.7.0, now also a direct build dependency of the JavaScript runtime |

`deno_core` remains at `0.412.0` with the browser-realm and V8 compatibility
patches. The patched `deno_v8 0.4.0` facade selects raw `v8 =152.2.0` for both
the core and `serde_v8 0.321.0`; its version is not a Deno CLI version. The
existing patched `cosmic-text` 0.14.2 and `taffy` 0.12.1 are retained. Older
versions of some utilities still appear as transitive dependencies.

Sources: [Cargo.lock](Cargo.lock), [workspace dependencies](Cargo.toml), and the
[JavaScript](crates/obscura-js/Cargo.toml), [network](crates/obscura-net/Cargo.toml),
[renderer](crates/obscura-render/Cargo.toml), and [library](crates/obscura/Cargo.toml)
manifests. See [Build from source](docs/Build-from-source.md#fork-dependency-patches)
for the checked-in patches.

## Build from source

Build from this repository to get the current changes. The project name is
Obscura-Hunt; commands and executables are still named `obscura` and `obscura-worker`.

### Requirements

- Current stable Rust and a C++20 toolchain.
- For stealth: CMake, Clang, and the libclang/LLVM development libraries.
- On Windows x64: Visual Studio C++ Build Tools, CMake, LLVM/Clang, and NASM on
  `PATH`. Before building, set `LIBCLANG_PATH` to the actual LLVM directory
  containing `libclang.dll`. Installing Clang or putting `clang.exe` on `PATH`
  does not by itself make that DLL discoverable by bindgen. See the
  [Windows build guidance](docs/Build-from-source.md).

On Ubuntu/Debian, install the native tools with:

```bash
sudo apt-get install build-essential cmake clang libclang-dev llvm-dev
```

### Build with rendering and stealth

```bash
git clone https://github.com/MythHack/obscura-hunt.git
cd obscura-hunt

# Recommended compiler cache; skip installation if already available.
cargo install sccache --locked
```

In a POSIX shell:

```bash
export RUSTC_WRAPPER=sccache
cargo build --release --locked -p obscura-cli --bins --features render,stealth
```

In PowerShell:

```powershell
$env:RUSTC_WRAPPER = "sccache"
# Adjust this to your LLVM installation directory containing libclang.dll.
$env:LIBCLANG_PATH = "C:/Program Files/LLVM/bin"
cargo build --release --locked -p obscura-cli --bins --features render,stealth
```

By default, the executables are in `target/release` (`.exe` on Windows). Keep `obscura` and
`obscura-worker` from the same build together, including when copying them to
another directory. Parallel scraping and multi-worker serving need the worker.
Add that directory to `PATH`, or use the full executable path in commands and
MCP client configuration.

Cargo automatically uses this fork's checked-in dependency patches. Keep the
patches, native SDK and snapshots aligned on the selected engine; an old V8
snapshot or archive is not interchangeable. See
[V8 version, SDK and snapshots](docs/Build-from-source.md#v8-version-sdk-and-snapshots)
for the build and embedding requirements. `sccache` does not replace the native
build tools.

For rendering without stealth, use `--features render`. To omit rendering,
use `--no-default-features`, optionally adding `--features stealth`; keep
`--release -p obscura-cli --bins` in each build command. No-render builds do not
provide screenshots, screencasts, or PDF output.

### Downloads

Find published binaries on the
[fork Releases page](https://github.com/MythHack/obscura-hunt/releases/latest).
Native packages cover Linux x64/ARM64, macOS x64/ARM64, and Windows x64.
Choose the default rendering build, `-stealth` (rendering + stealth),
`-no-render`, or `-no-render-stealth`. Keep the matching
`obscura`/`obscura-worker` pair together and verify archives against the release's
`SHA256SUMS.txt`. Current source builds also support `obscura --build-info` to
inspect the source commit and compiled features. Do not assume older archives
include that flag or a `build-info.json` file. Upstream packages and Docker
images do not include this fork's changes.

As of October 19, the latest public release is `v0.1.1`; `v0.1.2` remains a draft.
Both tags predate the V8 152 upgrade on `main`. Build from source for that
upgrade, rather than assuming a release archive contains the current checkout.
See [Installation](docs/Installation.md) for package provenance and verification.

## Use it

The examples below assume this fork's executable is on `PATH`.

### Fetch a page or take a screenshot

```bash
obscura fetch https://example.com --eval "document.title"
obscura fetch https://example.com --dump links
obscura fetch https://example.com --dump text --output page.txt
obscura fetch https://example.com --screenshot page.png
```

Screenshots require rendering. They are produced natively, without starting Chromium.

### Connect Puppeteer or Playwright

```bash
obscura serve --port 9222

# Enable stealth at runtime with a stealth-enabled build.
obscura serve --port 9222 --stealth
```

Puppeteer connects to `ws://127.0.0.1:9222/devtools/browser`.
Playwright's `chromium.connectOverCDP` connects to `ws://127.0.0.1:9222`.
See the [Puppeteer](docs/Use-with-Puppeteer.md) and
[Playwright](docs/Use-with-Playwright.md) guides for working examples.
See [media preference emulation](docs/CDP-media.md) for color-scheme and
reduced-motion overrides, and [production operation](docs/Run-in-production-at-scale.md)
for graceful worker recycling.

### Scrape several pages

```bash
obscura scrape https://example.com https://news.ycombinator.com --concurrency 2 --format json
```

Keep `obscura-worker` beside `obscura`.
`scrape` and batch `fetch` keep at most `--concurrency` jobs active and preserve
input order in their results.

### Use an MCP client

```bash
# stdio: for clients that launch a subprocess
obscura mcp

# HTTP: endpoint http://127.0.0.1:8080/mcp
obscura mcp --http --port 8080
```

Start with `browser_navigate`, read the page with `browser_snapshot`, then use
`browser_click`, `browser_fill`, or other tools. For asynchronous JavaScript,
`browser_evaluate` supports `awaitPromise: true`. For a condition that changes
later, use `browser_wait_for_function`; its `timeout` and `pollIntervalMs` are
in milliseconds, and an unmet condition returns a timeout error.

MCP supports still-image and PDF output with rendering enabled; streaming
screencasts use CDP. See the [MCP guide](docs/Use-the-MCP-server.md) for client
configuration, tools, and HTTP authentication. Non-loopback HTTP binding
requires authentication. Select this fork's executable explicitly if other
Obscura builds are installed.

### Access your local development server

Private/internal network requests are blocked by default. For services you control:

```bash
obscura fetch http://127.0.0.1:3000 --allow-private-network --dump text
obscura serve --port 9222 --allow-private-network
```

See [Environment variables](docs/Environment-variables.md) for the complete network rules.

## Local benchmarks (2026-10-11)

These are local measurements, not upstream's published Linux/no-render numbers.
The host was Windows x64 (10.0.26300), Intel Core Ultra 7 265K, 20 logical CPUs,
47.28 GiB usable RAM, and Python 3.13.12. Both Obscura builds use release
`render,stealth`; all measured pages are loopback fixtures, not public targets.
The suite is [official obscura-benchmark at
`2d7bc304`](https://github.com/h4ckf0r0day/obscura-benchmark/tree/2d7bc304fbb7ebefc44f770938192e3cede87dbd),
with the checked-in [Windows measurement patch](render-repros/benchmark-windows.patch).

These pinned October 11 results are historical, not measurements of the current
V8 152 build. See the [latest integration gates](docs/Testing-and-debugging.md#upstream-sync-8b49681)
and the earlier [bounded V8 upgrade comparison](docs/Testing-and-debugging.md#upgrade-validation-october-19).

### Capability and stealth

Official Obscura was built from
[`2bda71a`](https://github.com/h4ckf0r0day/obscura/commit/2bda71a557247bd2d6fabf01fa66eb4b07dcb2e8)
(`0.1.0-dev+2bda71a`); Obscura-Hunt was built from
[`2816993`](https://github.com/MythHack/obscura-hunt/commit/2816993483232d99495a28e7ba946713d0035481).
The fork executable reports `0.1.1-dev+f9624e3`: that version suffix differs from
the recorded source revision, so use the source revision and binary hashes below.

| Assertion suite | Official Obscura | Obscura-Hunt |
| --- | ---: | ---: |
| Obstacle course | 32/33 | 32/33 |
| Stealth (`--stealth`) | 89/112 (79.5%) | 88/112 (78.6%) |

Both obstacle-course runs failed only `fingerprint`: expected `Europe/Berlin`,
got `Asia/Shanghai`, despite setting `OBSCURA_TIMEZONE=Europe/Berlin`.
The 23 shared stealth failures comprise an enumerable engine global, two empty
WebGL vendor/renderer values, a heap-limit mismatch, and 19 Linux-UA/platform
expectations that did not match these Windows builds. The only changed verdict
was `fp_device_pixel_ratio_consistent`: official expected/actual **1/1**,
fork **2/1**. The suite infers expected DPR 2 from screen widths ≥2560, otherwise 1;
the different expected values are a profile/heuristic limitation, not proof of a
browser regression or real-world detection result. Both capability runners exited
1 for the failed assertion; both stealth runners exited 0 despite failed assertions.

### Cold-process DOM-output comparison

Chromium was Playwright's `chromium-1223`, SHA-256
`290fa7018fda22c52ada5eddb0113baf3ebc41fd0fde6085eddb19793606c635`.

| Measurement | Official Obscura | Chromium batch 1 (with official) | Obscura-Hunt | Chromium batch 2 (with fork) |
| --- | ---: | ---: | ---: | ---: |
| Median of 33 page-median cold-process times | 3,098.4 ms | 439.3 ms | 3,186.7 ms | 754.0 ms |
| Median of 33 page-median sampled tree RSS peaks | 28.2 MiB | 414.8 MiB | 29.4 MiB | 463.1 MiB |
| Valid / rejected timed DOM-output samples (of 99) | 99 / 0 | 99 / 0 | 99 / 0 | 99 / 0 |

The two Chromium columns are separate measured batches of the same executable,
not pooled samples. The reference itself varied between batches, so these runs
do not isolate the effect of fork changes. Each column includes 33 warmups plus
99 timed attempts; across all 528 attempts, timeouts, nonzero exits, RSS sampling
errors, and missing RSS values were all **0**. Both comparison runners exited 0.

Maximum recorded DOM-output lengths differed on these four pages (characters):

| Page | Official Obscura | Chromium batch 1 | Obscura-Hunt | Chromium batch 2 |
| --- | ---: | ---: | ---: | ---: |
| `ssr-hydrate` | 2,157 | 2,151 | 2,157 | 2,151 |
| `es-modules` | 1,344 | 1,341 | 1,344 | 1,341 |
| `observer-intersection` | 3,599 | 2,359 | 3,599 | 2,669 |
| `spa-mini-app` | 3,771 | 3,768 | 3,771 | 3,768 |

Lengths alone cannot prove equivalent DOM content or rendering.

### What the comparison measures

- Each of the 33 pages gets one warmup and three timed runs per engine, with
  100 ms between runs and a 60 s process timeout. Each run starts a cold process;
  Chromium uses an isolated temporary incognito profile, with no browser/tab reuse.
- Time includes process startup, page loading, JavaScript, and DOM serialization
  (`fetch --dump html` vs `--headless --dump-dom`), not pixel/layout correctness.
  Each page uses the median of accepted timed samples; headline time and memory
  are medians across those 33 page medians, not throughput or a concurrency test.
- Obscura waits **3 real seconds**; Chromium gets a **3,000 ms virtual-time budget**.
  These are not equivalent wall-clock settling policies. Chromium's lower time
  and Obscura's lower RSS in both batches do not establish a general engine speedup.
  Median per-page Chromium/Obscura time ratios were 0.14 (official) and 0.24 (fork);
  RSS ratios were 14.48 and 15.09. A median of ratios is not a ratio of aggregate medians.
- Windows RSS is the maximum sampled sum of root/descendant resident memory,
  sampled every 30 ms, then aggregated by median. It can miss shorter spikes and
  is not GNU `time -v` process peak RSS, allocation totals, or steady-state memory.
- The original non-empty/output-length guard selects samples; it is neither a
  semantic DOM assertion nor an exit-code gate. Exit/timeout/RSS counts are
  reported separately. Only the obstacle course checks the manifest's expected
  values/content, using its final timed output (one warmup, five timed runs).
- Chromium requests 1280×720 and device scale 1; the actual CSS viewport is not
  inspected. Profile 0 and `Europe/Berlin` are requested for Obscura, with profile
  rotation disabled; stealth is enabled only in the stealth suite.
- This run does not measure Firefox, Chromium capability/stealth scores, WPT,
  screenshots, production tab reuse, public-site success, or Cloudflare bypass.

### Reproduce on Windows

Start in this fork's root after building browser source revision `2816993` with
the release command above. Use the same Chromium executable for both Obscura
batches; a different build changes the comparison. Adjust executable paths for
`CARGO_TARGET_DIR` or a different Chromium installation. Run all three suites for
each Obscura executable, keeping separate JSON and recording source revision,
`obscura --version`, executable/worker SHA-256, and Chromium SHA-256.

```powershell
$fork = (Get-Location).Path
$suite = Join-Path (Split-Path $fork) "obscura-benchmark"
git clone https://github.com/h4ckf0r0day/obscura-benchmark.git $suite
git -C $suite checkout 2d7bc304fbb7ebefc44f770938192e3cede87dbd
git -C $suite apply "$fork/render-repros/benchmark-windows.patch"
python -m pip install psutil

$env:OBSCURA_BIN = "$fork/target/release/obscura.exe"
$env:CHROME_BIN = "$env:LOCALAPPDATA/ms-playwright/chromium-1223/chrome-win64/chrome.exe"
$env:OBSCURA_ALLOW_PRIVATE_NETWORK = "1"
$env:OBSCURA_PROFILE = "0"
$env:OBSCURA_TIMEZONE = "Europe/Berlin"
$env:PYTHONUTF8 = "1"
Remove-Item Env:OBSCURA_ROTATE_PROFILE -ErrorAction SilentlyContinue
Push-Location $suite
python obstacle-course/run.py --json | Set-Content -Encoding utf8 fork-final-capability.json
python stealth-bench/run.py --stealth --json | Set-Content -Encoding utf8 fork-final-stealth.json
python compare/head-to-head.py --json --runs 3 --warmup 1 --wait 3 --label fork-final-vs-chromium --output fork-final-comparison.json
Pop-Location
```

For the official baseline, clone the official source at the pinned revision and
build it separately; keep its matching worker beside the executable:

```powershell
$official = Join-Path (Split-Path $fork) "obscura-official"
git clone https://github.com/h4ckf0r0day/obscura.git $official
git -C $official checkout 2bda71a557247bd2d6fabf01fa66eb4b07dcb2e8
Push-Location $official
cargo build --release --locked -p obscura-cli --bins --features render,stealth
Pop-Location
$env:OBSCURA_BIN = "$official/target/release/obscura.exe"
Push-Location $suite
python obstacle-course/run.py --json | Set-Content -Encoding utf8 official-2bda71a-capability.json
python stealth-bench/run.py --stealth --json | Set-Content -Encoding utf8 official-2bda71a-stealth.json
python compare/head-to-head.py --json --runs 3 --warmup 1 --wait 3 --label official-2bda71a-vs-chromium --output official-2bda71a-comparison.json
Pop-Location
```

### Evidence and binary provenance

| SHA-256 | Official Obscura | Obscura-Hunt |
| --- | --- | --- |
| `obscura.exe` | `5b59abfb535b8d0784787b6fa09b9d64cafd4a0f4a41e3a3f533869ceab306ae` | `aa18963fcfda4dc568eae40f7f220fad27906460cc43c173bc5e47c9b0d425fa` |
| `obscura-worker.exe` | `787126df746bd27caf4218514544329d17d7e2dd2e4dd71edd9e4adeb22161e8` | `230137e21e7a2c74c6cfd67bcf34ebde9096396a99ecc57334934a5735a3410f` |

Both batches used the same patched `compare/head-to-head.py`, SHA-256
`92ba3bbea5367145af3b6a41b7362549c26160037c0d3ec153d236180d801424`.
Retained evidence is `official-2bda71a-{metadata,capability,stealth,comparison}.json`
and `fork-final-{metadata,capability,stealth,comparison}.json` under
`D:/code/.bench-results/20261011-local/`; metadata records commands,
binary/worker hashes, and all runner/manifest hashes. Comparison labels are
`official-2bda71a-vs-chromium` (batch 1) and `fork-final-vs-chromium` (batch 2).
These local artifacts are not upstream's published results.

## Known limits

- This build has no audio/video codec or playback pipeline. Media properties
  and text tracks are supported; unsupported sources fail instead of reporting
  fabricated metadata or successful playback.
- Correct browser behavior removes specific inconsistencies; it does not make
  this engine indistinguishable from Chromium. Stealth does not guarantee
  bypassing Cloudflare or other anti-bot systems, and local benchmark scores
  are not challenge-pass rates.

## More guides

- [Documentation index](docs/README.md)
- [Latest upstream validation](docs/Testing-and-debugging.md#upstream-sync-8b49681)
- [CLI reference](docs/CLI-reference.md)
- [Cookies and storage](docs/Persist-cookies-and-storage.md)
- [Build from source](docs/Build-from-source.md)

## Credits and license

Obscura-Hunt builds on [h4ckf0r0day/obscura](https://github.com/h4ckf0r0day/obscura).
Upstream contributors remain credited, and upstream copyright and license
notices are retained. Licensed under [Apache License 2.0](LICENSE).
