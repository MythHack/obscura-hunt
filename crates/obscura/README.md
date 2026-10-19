# obscura

Embeddable Rust API for the [Obscura-Hunt fork](https://github.com/MythHack/obscura-hunt)
headless browser. Drive a real V8 + DOM browser (`Browser`, `Page`, `Element`,
`CookieStore`) directly from Rust, with no separate process or CDP round-trips.

## Install

This crate is not published to crates.io. Check out this fork alongside your
consumer project (run from their common parent directory):

```bash
git clone https://github.com/MythHack/obscura-hunt.git
```

For sibling `my-app/` and `obscura-hunt/` directories, add this to
`my-app/Cargo.toml`:

```toml
[dependencies]
obscura = { path = "../obscura-hunt/crates/obscura", features = ["api"] }
tokio = { version = "1", features = ["rt", "macros"] }
anyhow = "1"

[patch.crates-io]
deno_core = { path = "../obscura-hunt/vendor/deno_core-0.412.0" }
deno_v8 = { path = "../obscura-hunt/vendor/deno_v8-0.4.0" }
taffy = { path = "../obscura-hunt/vendor/taffy" }
cosmic-text = { path = "../obscura-hunt/vendor/cosmic-text" }
```

The patch table belongs in the consumer's **workspace root** manifest, with
paths relative to that manifest. Cargo does not inherit a dependency's patches,
even for a git dependency. Both Deno patches are required: `deno_core 0.412.0`
supplies browser realms, and `deno_v8 0.4.0` pins raw `v8` to `=152.2.0` (engine
V8 `15.2.124.1`), shared with `serde_v8 0.321.0`. `deno_v8` is a Rust facade,
not a Deno CLI version. Keep the `taffy` and `cosmic-text` patches when enabling
`render` to retain this fork's layout and font behavior.

Record the checkout commit, use that same checkout for every path above, and
commit the consumer's own `Cargo.lock`. The first build is large; V8 normally
uses a prebuilt archive, while local C++ shims are compiled against the pinned
152 SDK. Snapshots must match that engine, target and native ABI. See the
[embedding guide](../../docs/Use-as-a-Rust-library.md#add-the-dependency) and
[build requirements](../../docs/Build-from-source.md#v8-version-sdk-and-snapshots).

## Usage

```rust,no_run
use obscura::Browser;
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let browser = Browser::builder()
        .stealth(true)
        .storage_dir("/tmp/cookies")
        .build()?;

    let mut page = browser.new_page().await?;
    page.goto("https://example.com").await?;

    let el = page.wait_for_selector("a", Duration::from_secs(5)).await?;
    println!("{} -> {:?}", el.text(), el.attribute("href"));

    Ok(())
}
```

See `examples/basic.rs` for a runnable version (`cargo run --example basic`).
