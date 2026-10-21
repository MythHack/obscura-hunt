## Setup

```bash
obscura serve --port 9222
npm install puppeteer-core
```

## Connect

```js
const puppeteer = require('puppeteer-core');

const browser = await puppeteer.connect({
  browserWSEndpoint: 'ws://127.0.0.1:9222/devtools/browser',
});
```

Use `puppeteer-core`, not `puppeteer`. The `puppeteer` package bundles a Chrome download.

`GET /json/version` and `GET /json/version/` return the browser WebSocket URL.
The trailing-slash endpoint supports Playwright's HTTP `connect_over_cdp`
discovery. Routes remain exact: extra suffixes and query strings are refused.
Discovery retains the same Host validation, browser-Origin rejection, and
configured bearer authentication as WebSocket connections.

`GET /json/protocol` reports the protocol version and implemented domain names.
It does not publish command/type schemas or advertise Chromium domains that
Obscura only accepts as compatibility no-ops.

`GET /json/list` lists the browser's real targets, including its initial
`about:blank` page. Each advertised `/devtools/page/<targetId>` socket is bound
to that page; commands on a page socket do not need a `sessionId`. Browser
sockets attach with `Target.attachToTarget` and use its returned session ID.
Targets and browser contexts are shared across browser connections. Closing a
client connection detaches only its sessions; it does not close its pages.

## Navigate

```js
const page = await browser.newPage();
await page.goto('https://example.com');
await page.goto('https://example.com', { waitUntil: 'load' });
await page.goto('https://example.com', { waitUntil: 'networkidle0', timeout: 60000 });
```

Default `waitUntil` is `domcontentloaded`. Other values: `load`, `networkidle2`, `networkidle0`.

## Evaluate

```js
const title = await page.evaluate(() => document.title);

const items = await page.evaluate(() => {
  return Array.from(document.querySelectorAll('.item')).map(el => ({
    text: el.textContent,
    href: el.querySelector('a')?.href,
  }));
});
```

## Interact

```js
await page.click('#login-button');
await page.type('#username', 'alice');
await page.fill('#password', 'secret');  // alias of .type for compat

await page.waitForSelector('#dashboard');
await page.waitForFunction(() => window.appReady === true);
```

## Cookies

```js
await page.setCookie({
  name: 'session',
  value: 'abc123',
  domain: 'example.com',
  path: '/',
  httpOnly: true,
  secure: true,
});

const cookies = await page.cookies();
```

For session persistence across runs see [Persist cookies and storage](Persist-cookies-and-storage.md).

## Intercept requests

Use a public CDP session and request-stage `Fetch.enable` for blocking,
rewriting or fulfilling requests. Authentication-challenge interception is
unsupported; Puppeteer versions that enable it automatically through
`page.setRequestInterception()` are rejected rather than silently accepting an
unimplemented mode. See [Intercept and modify requests](Intercept-and-modify-requests.md)
for the supported session setup and byte-preserving examples.

Scripted fetch/XHR requests in stealth mode report Network events and retained
response bodies using the same request IDs as the normal transport. The existing
`OBSCURA_NETWORK_BODY_BUFFER_ENTRIES` and `OBSCURA_NETWORK_BODY_BUFFER_BYTES`
settings still control retention; disabling retention does not disable events.

Raw CDP sessions subscribe independently with `Page.enable`, `Runtime.enable`,
`Network.enable`, and `Page.setLifecycleEventsEnabled`. `Network.disable` or a
session detach releases only that session's response-body view. Other enabled
sessions keep their events and bounded retained bodies. Request events include
the method, body, and headers of the actual native send and arrive before a
slow response finishes. Request interception pauses before I/O, not by
reconstructing a pause from a completed response.

## Expose a Node callback

```js
await page.exposeFunction('logFromPage', (msg) => {
  console.log('page:', msg);
});

await page.evaluate(() => {
  window.logFromPage('hello from the browser');
});
```

## Multiple pages

```js
const page1 = await browser.newPage();
const page2 = await browser.newPage();

await Promise.all([
  page1.goto('https://a.example.com'),
  page2.goto('https://b.example.com'),
]);
```

Pages stay on their native owner threads; a server-wide directory routes
commands to those owners without sharing live V8 objects or a global V8 lock.
CPU-bound JavaScript can delay other pages on the same owner thread, but does
not block another owner's JavaScript or the HTTP discovery endpoints.

## Screenshots, scrolling, and PDF

```js
await page.setViewport({ width: 1440, height: 1000, deviceScaleFactor: 1 });
await page.screenshot({ path: 'viewport.png' });

await page.evaluate(() => window.scrollTo(0, 1200));
await page.screenshot({ path: 'scrolled.png' });

await page.screenshot({ path: 'full-page.png', fullPage: true });
await page.pdf({ path: 'page.pdf', format: 'A4', printBackground: true });
```

A normal screenshot captures the live viewport and scroll position;
`fullPage: true` captures document space. PDF export uses print-media layout,
shaped text with embedded subset fonts, vector paint and images.

## Screencasting

Attach a raw CDP session to the page, acknowledge every frame, and detach it
when finished:

```js
const client = await page.createCDPSession();

client.on('Page.screencastFrame', async ({ data, sessionId }) => {
  const jpeg = Buffer.from(data, 'base64');
  // Consume or forward `jpeg` here.
  await client.send('Page.screencastFrameAck', { sessionId });
});

await client.send('Page.startScreencast', {
  format: 'jpeg',
  quality: 80,
  maxWidth: 1280,
  maxHeight: 720,
});

// ...navigate, scroll, and interact...

await client.send('Page.stopScreencast');
await client.detach();
```

Frames are activity-driven page captures, not fixed-rate desktop video.

## Disconnect

```js
await browser.disconnect();  // leaves obscura serve running
```

## Page lifecycle

`Page.stopLoading` cancels an in-flight document or parser-resource request,
clears queued document navigation, and leaves the target usable. Rendering
builds also cancel pending image/font loads, including `Image`/`decode()`
requests, without restarting those URLs on the next render turn. Already-loaded
resources are preserved. Ordinary JavaScript `fetch()` calls are not canceled.

If cancellation happens before a document is installed, the previous document
is retained and navigation returns `net::ERR_ABORTED`. Once installation has
committed, the partial document is retained, including on the first navigation
before a JavaScript runtime existed. That cancellation does not report the
committed document as aborted.

`Page.close` runs the top-level document's `beforeunload` handlers. A document
activated through native input can request confirmation, reported through
`Page.javascriptDialogOpening` with type `beforeunload`. Use
`Page.handleJavaScriptDialog` to accept or dismiss it. `Target.closeTarget`
remains a force-close path. Other dialog types and child-frame unload hooks
are not implemented by this change.

## Current limits

- Some device emulation, service-worker, native media, long-tail CSS, and
  compositor behavior remains incomplete relative to Chromium.
- Pages share one V8 isolate; CPU-bound JavaScript serializes across pages.
- Tagged PDF, headers/footers, outlines and full CSS paged media are not implemented.

Network events are shared by sessions attached to the same target. An additional
CDP session can observe navigation and later fetch/XHR traffic without issuing a
second navigation. Request IDs and loader IDs match the driving session, so
`Network.getResponseBody` uses the same ID. Fetch interception control and the
navigation lifecycle keep their existing owner; sessions on other targets do
not receive this traffic.

ES-module roots, dependencies, and dynamic imports report their actual HTTP
status, response headers, and body size. Retained bodies remain readable by
request ID while a page is parked with its runtime retained, subject to
`OBSCURA_NETWORK_BODY_BUFFER_ENTRIES` and `OBSCURA_NETWORK_BODY_BUFFER_BYTES`.
Cached imports do not produce another network request.
