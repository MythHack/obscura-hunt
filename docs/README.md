# Obscura-Hunt

Obscura-Hunt is a fork of [Obscura](https://github.com/h4ckf0r0day/obscura),
a native headless browser engine written in Rust with embedded V8. Use it through
the CLI, the Chrome DevTools Protocol (CDP), the MCP server for agent workflows,
or the Rust library. Puppeteer and Playwright can connect through CDP; supported
behavior and limitations are documented in the guides below.

Current `main` uses rusty_v8 **`152.2.0`**, reporting engine version
**`15.2.124.1-rusty`**, with matching native SDK, C++ ABI and snapshots.
To get that engine, [build the fork from source](Build-from-source.md).
The `v0.1.2` tag predates this upgrade; it is not a V8 152 build. See the
[October 19 upgrade validation](Testing-and-debugging.md#upgrade-validation-october-19)
for the exercised configurations and results, rather than treating an engine
version change as a performance claim.

Rendering builds support screenshots, scroll-aware layout, CDP screencasting
and raster PDF export. Rendering and the optional transport feature are selected
independently; see [Installation](Installation.md) for all four configurations.

## Quickstart

- [Installation](Installation.md)
- [Your first fetch](Your-first-fetch.md)
- [Extract data](Extract-data.md)
- [Connect Puppeteer or Playwright](Connect-Puppeteer-or-Playwright.md)

## Guides

- [Build from source](Build-from-source.md)
- [Configure stealth and proxies](Configure-stealth-and-proxies.md)
- [Markdown extraction](Markdown-extraction.md)
- [Use with Puppeteer](Use-with-Puppeteer.md)
- [Use with Playwright](Use-with-Playwright.md)
- [Use the MCP server](Use-the-MCP-server.md)
- [Watch agent sessions live](Watch-agent-sessions-live.md)
- [Use as a Rust library](Use-as-a-Rust-library.md)
- [Persist cookies and storage](Persist-cookies-and-storage.md)
- [Intercept and modify requests](Intercept-and-modify-requests.md)
- [Run in production at scale](Run-in-production-at-scale.md)

## Reference

- [CLI reference](CLI-reference.md)
- [Environment variables](Environment-variables.md)
- [CDP media preference emulation](CDP-media.md)

## Contributing

- [Architecture overview](Architecture-overview.md)
- [Adding a CDP method or Web API](Adding-a-CDP-method-or-Web-API.md)
- [Testing and debugging](Testing-and-debugging.md)

## Links

- Source: https://github.com/MythHack/obscura-hunt
- Releases and available assets: https://github.com/MythHack/obscura-hunt/releases
- Issues: https://github.com/MythHack/obscura-hunt/issues
- Upstream project and original browser engine: https://github.com/h4ckf0r0day/obscura

License: Apache-2.0.
