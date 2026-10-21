The `obscura` crate embeds the engine in a Rust program with a `Browser` / `Page` / `Element` API plus a cookie store, no CDP round-trips. This fork is used from a source checkout rather than a crates.io release; its native V8 dependency normally uses a prebuilt archive.

## Add the dependency

Check out this fork next to your consumer project. From their common parent
directory:

```bash
git clone https://github.com/MythHack/obscura-hunt.git
```

With sibling directories `my-app/` and `obscura-hunt/`, put the following in
`my-app/Cargo.toml`. If `my-app` is a workspace member, put `[patch.crates-io]`
in the consumer workspace's root `Cargo.toml` instead, and adjust its paths
relative to that manifest:

```toml
[dependencies]
obscura = { path = "../obscura-hunt/crates/obscura" }
tokio = { version = "1", features = ["rt", "macros"] }
anyhow = "1"

[patch.crates-io]
deno_core = { path = "../obscura-hunt/vendor/deno_core-0.412.0" }
deno_v8 = { path = "../obscura-hunt/vendor/deno_v8-0.4.0" }
taffy = { path = "../obscura-hunt/vendor/taffy" }
cosmic-text = { path = "../obscura-hunt/vendor/cosmic-text" }
rustls-platform-verifier = { path = "../obscura-hunt/vendor/rustls-platform-verifier-0.7.1" }
wreq = { path = "../obscura-hunt/vendor/wreq-6.0.0-rc.31" }
btls-sys = { path = "../obscura-hunt/vendor/btls-sys-0.5.6" }
```

Cargo ignores patches declared in a dependency's workspace root, including
when the dependency is fetched through git. Selecting the fork alone therefore
does not inherit its patches. The consumer root must explicitly select both
`deno_core` (the browser-realm integration) and `deno_v8` (the facade pinning raw
`v8` to `=152.2.0`). The `taffy` and `cosmic-text` patches preserve this fork's
layout and font behavior when `obscura`'s `render` feature is enabled. The verifier,
`wreq` and `btls-sys` patches retain network validation, certificate-chain recovery
and stealth/OpenSSL symbol isolation. Keep the complete patch table for the
corresponding features. These paths refer to the explicit checkout above, not to
an assumed `vendor/` directory inside your application or Cargo's git cache.

The locked engine family is `deno_core 0.412.0`, `serde_v8 0.321.0`,
`deno_v8 0.4.0` and raw `v8 152.2.0` (V8 `15.2.124.1-rusty`). `deno_v8` is a facade,
not Deno CLI `2.9.7`; that CLI uses the same facade family but raw V8 `150.4.0`.
Keep the fork checkout at a recorded commit and commit the consumer's own
`Cargo.lock` with the patched versions selected; the dependency's lockfile does
not pin your application's resolution. Use the same checkout for the library
and every patch.

The first build includes native dependencies and needs the tools described in
[Build from source](Build-from-source.md). The native SDK, snapshot producer and
embedded runtime must agree on V8 and its ABI; do not reuse a snapshot from the
old engine. See [V8 dependency and snapshot alignment](Testing-and-debugging.md#v8-dependency-and-snapshot-alignment).

## Quickstart

```rust
use obscura::Browser;
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let browser = Browser::builder()
        .stealth(true)
        .build()?;

    let mut page = browser.new_page().await?;
    page.goto("https://example.com").await?;

    println!("URL: {}", page.url());
    println!("HTML bytes: {}", page.content().len());

    let el = page.wait_for_selector("h1", Duration::from_secs(5)).await?;
    println!("Heading: {}", el.text());

    let title = page.evaluate("document.title");
    println!("Title: {}", title);

    Ok(())
}
```

## API surface

`Browser::builder()` configures the engine: `.stealth(bool)`, `.proxy(url)`, `.user_agent(ua)`, `.storage_dir(dir)`, `.regional(RegionalConfig)`, then `.build()`. `Browser::new()` uses defaults. Both construction paths return a `Result`; profile load and permission errors propagate from `.build()` instead of opening an empty replacement profile. See [persistent context construction and saves](Persist-cookies-and-storage.md#rust-context-construction-and-saves) for the lower-level `BrowserContext` API.

`Page`:
- `goto(url).await` navigate and wait for load
- `content()` rendered HTML
- `url()` current URL
- `evaluate(js)` run JavaScript, returns a `serde_json::Value`
- `query_selector(css)` first match as an `Element`, or `None`
- `wait_for_selector(css, Duration).await` poll until present
- `settle(max_ms).await` drive the event loop so async work (`fetch`, timers) completes
- `on_request(cb)` / `on_response(cb)` passive callbacks for every request and response
- `enable_interception()` channel to block, mock, or rewrite requests
- `add_preload_script(js)` run a script before the page's own scripts

`Element`: `text()`, `attribute(name)`, `click()`.

`CookieStore`: `set`, `get_all`, `get_for_url`, `save_to_file`, `load_from_file`.

## Intercept requests

The interception API observes, blocks, mocks, and rewrites the requests a page makes, including JavaScript `fetch()` and XHR. Use it to capture API payloads while crawling, block trackers, or mock responses in tests.

### Passive callbacks

`on_request` and `on_response` fire for every request and response (navigation and JS `fetch()`/XHR) and are non-blocking. `on_response` is the main path for capturing the JSON an SPA loads asynchronously. Both return a stable id; pass it to `off_request` / `off_response` to detach the callback when a crawl phase is done. Callbacks are scoped to the page that registered them: they never fire for another page's requests and are dropped with the page.

```rust
use obscura::{Browser, ResourceType};
use std::sync::Arc;

let browser = Browser::new()?;
let mut page = browser.new_page().await?;

page.on_response(Arc::new(|info, resp| {
    if info.resource_type == ResourceType::Fetch {
        println!("{} -> {} bytes", info.url, resp.body.len());
    }
}));

page.goto("https://example.com").await?;
page.settle(2000).await;   // let in-page fetch() calls resolve
```

### Active interception

`enable_interception()` returns a channel of native page requests, including document navigation, subresources and JS `fetch()`/XHR, paused before target I/O. Resolve each through its `resolver` to pass, block, mock, or rewrite it. `Fail` and `Fulfill` do not send the target request; a disconnected or unanswered interceptor fails closed.

```rust
use obscura::{Browser, InterceptResolution};

let mut page = browser.new_page().await?;
let mut rx = page.enable_interception();

tokio::spawn(async move {
    while let Some(req) = rx.recv().await {
        let action = if req.url.contains("/ads") {
            InterceptResolution::Fail { reason: "blocked".into() }
        } else if req.url.ends_with("/api/flags") {
            InterceptResolution::Fulfill {
                status: 200,
                headers: Default::default(),
                body: br#"{"newDashboard":true}"#.to_vec(),
            }
        } else {
            // Pass through, or rewrite by setting url/method/headers/body.
            InterceptResolution::Continue { url: None, method: None, headers: None, body: None }
        };
        let _ = req.resolver.send(action);
    }
});

page.goto("https://example.com").await?;
page.settle(2000).await;
```

A `Continue` with `url: Some(...)` rewrites the target. The new URL and each redirect are re-checked against the file/address permissions; rewriting cannot bypass the configured CIDR exemptions or private-network gate.

`Continue.body` takes raw bytes as `Option<Vec<u8>>`, not a base64 string. Migrate text overrides from `Some(text)` to `Some(text.into_bytes())`. `None` preserves the original request body; `Some(Vec::new())` replaces it with an empty body. CDP clients still send base64 `postData` as specified by the protocol; Obscura decodes it before sending the request.

`Fulfill.body` is also raw `Vec<u8>`. Use `text.into_bytes()` for text or pass
binary bytes directly; do not base64-encode the public Rust body. Only CDP
`Fetch.fulfillRequest` uses a base64 body on the wire.

### Preload scripts

`add_preload_script` runs a script before any of the page's own `<script>` tags (the CDP `Page.addScriptToEvaluateOnNewDocument` contract), so it can install hooks before the page bootstraps. Call it before `goto`.

```rust
let mut page = browser.new_page().await?;
page.add_preload_script("window.__patched = true;");
page.goto("https://example.com").await?;
```

Passive `RequestInfo.resource_type` distinguishes `ResourceType::Fetch` from `ResourceType::Xhr`. The active `InterceptedRequest.resource_type` uses CDP-style strings such as `"Fetch"`, `"XHR"`, `"Document"` and `"Script"`.

## When to use which interface

- Embedding the engine in a Rust service: this crate.
- Driving from Node/Python with existing Puppeteer/Playwright code: the [CDP server](Connect-Puppeteer-or-Playwright.md).
- Giving an AI agent browser tools: the [MCP server](Use-the-MCP-server.md).
- One-off fetches and scraping from the shell: the [CLI](CLI-reference.md).
