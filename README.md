# Obscura-Hunt

**English** | [简体中文](README.zh-CN.md)

Obscura is a headless browser engine written in Rust, built for web scraping and
AI agent automation. It runs real JavaScript via V8 and supports the Chrome
DevTools Protocol (CDP) for Puppeteer and Playwright clients.

**Obscura-Hunt** is the [MythHack fork](https://github.com/MythHack/obscura-hunt)
of [Obscura](https://github.com/h4ckf0r0day/obscura), focused on making browser
workflows more reliable for HuntForge and other automation clients. It keeps
upstream's native rendering, stealth options, and CDP/MCP interfaces. It can
replace headless Chrome for supported workflows, but is not fully compatible
with every Chrome API or website.

## What is better in this fork?

- **More complete popup workflows.** Open a popup, exchange messages with the
  original page, and detect when it closes. Navigating or closing one page does
  not clear another page's loaded JavaScript modules.
- **Real interactions inside iframes.** Click buttons and submit forms in an
  embedded page, with input and requests reaching the correct frame. These
  interactions still respect cross-origin and page-embedding restrictions.
- **Fixes for waits after navigation.** Timers and asynchronous scripts keep
  running when a page loads a new URL. MCP waits also let the page do its work,
  rather than waiting for a timer that cannot run.
- **Zoom that affects the page, not just a reported value.** CSS `zoom` changes
  layout, drawing, and click positions together. CSS `perspective` also affects
  computed styles and positioning; 3D perspective drawing is not yet supported.
- **Request filters carry over to popups.** If an automation client intercepts
  only selected URLs, opening a popup does not unexpectedly pause every request.

These improvements describe the current source. The engine, V8, renderer,
stealth transport, and general automation interfaces are upstream work, not
inventions of this fork.

## Build from source

Build from this repository to get the current changes. The project name is
Obscura-Hunt; commands and executables are still named `obscura` and `obscura-worker`.

### Requirements

- Current stable Rust and a C/C++ toolchain.
- For stealth: CMake, Clang, and the libclang/LLVM development libraries.
- On Windows x64: Visual Studio C++ Build Tools, CMake, LLVM/Clang, and NASM on
  `PATH`. Set `LIBCLANG_PATH` to LLVM's `bin` directory if libclang is not found.

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
cargo build --release --locked -p obscura-cli --bins --features render,stealth
```

By default, the executables are in `target/release` (`.exe` on Windows). Keep `obscura` and
`obscura-worker` from the same build together, including when copying them to
another directory. Parallel scraping and multi-worker serving need the worker.
Add that directory to `PATH`, or use the full executable path in commands and
MCP client configuration.

Cargo automatically uses this fork's checked-in dependency patches; keep them
when building. See [Build from source](docs/Build-from-source.md) for the details
and additional platform guidance. `sccache` does not replace the native build tools.

For rendering without stealth, use `--features render`. To omit rendering,
use `--no-default-features`, optionally adding `--features stealth`; keep
`--release -p obscura-cli --bins` in each build command. No-render builds do not
provide screenshots, screencasts, or PDF output.

### Downloads

Published builds will appear on the
[fork Releases page](https://github.com/MythHack/obscura-hunt/releases).
For now, build from source. Upstream packages and Docker images do not include
this fork's changes.

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

### Scrape several pages

```bash
obscura scrape https://example.com https://news.ycombinator.com --concurrency 2 --format json
```

Keep `obscura-worker` beside `obscura`.

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

## Known limits

- Some CSS, Web APIs, media, and font rendering differ from Chromium. Related
  windows are supported, but arbitrary child isolated-world execution is not.
- Unsupported frame transforms and embedding-policy syntax fail closed rather
  than reporting successful interaction. CSS `perspective` does not render 3D projection.
- Stealth does not guarantee bypassing Cloudflare or other anti-bot systems.

## More guides

- [CLI reference](docs/CLI-reference.md)
- [Cookies and storage](docs/Persist-cookies-and-storage.md)
- [Build from source](docs/Build-from-source.md)

## Credits and license

Obscura-Hunt builds on [h4ckf0r0day/obscura](https://github.com/h4ckf0r0day/obscura).
Upstream contributors remain credited, and upstream copyright and license
notices are retained. Licensed under [Apache License 2.0](LICENSE).
