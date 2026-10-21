# Intercept and modify requests

CDP `Fetch.enable` pauses matching requests **before native I/O**. Continue,
rewrite, Fulfill and Fail use the selected transport and its network policy.
Fulfill accepts base64-encoded bytes; it does not send a request to the target.
Rewritten URLs and redirects still pass the address and file-access checks.

Authentication-challenge and response-stage interception are not implemented
and are explicitly rejected. Some Puppeteer and Playwright versions enable
`handleAuthRequests` automatically in `page.setRequestInterception()` or
`page.route()`. Use their public CDP session API for the supported request-stage
interception instead of enabling an unsupported mode.

## Open a CDP session

Puppeteer:

```js
const client = await page.createCDPSession();
```

Playwright:

```js
const client = await context.newCDPSession(page);
```

Install one request handler below, then enable interception before navigation:

```js
await client.send('Fetch.enable', {
  patterns: [{ urlPattern: '*', requestStage: 'Request' }],
});
```

## Block resources

```js
client.on('Fetch.requestPaused', async ({ requestId, resourceType }) => {
  if (['Image', 'Media', 'Font'].includes(resourceType)) {
    await client.send('Fetch.failRequest', { requestId, errorReason: 'BlockedByClient' });
  } else {
    await client.send('Fetch.continueRequest', { requestId });
  }
});
```

For URL selection, supply `urlPattern` in `Fetch.enable`, or inspect
`event.request.url` in the handler. Only selected requests pause.

## Modify headers

```js
client.on('Fetch.requestPaused', async ({ requestId, request }) => {
  const headers = Object.entries({ ...request.headers, 'X-Custom': 'value' })
    .map(([name, value]) => ({ name, value: String(value) }));
  await client.send('Fetch.continueRequest', { requestId, headers });
});
```

A continuation may also supply `url`, `method` and base64 `postData`. Omitted
fields preserve the actual request rather than replacing its method or body.

## Fulfill a local response

```js
client.on('Fetch.requestPaused', async ({ requestId, request }) => {
  if (new URL(request.url).pathname === '/api/feature-flags') {
    await client.send('Fetch.fulfillRequest', {
      requestId,
      responseCode: 200,
      responseHeaders: [{ name: 'Content-Type', value: 'application/json' }],
      body: Buffer.from(JSON.stringify({ newDashboard: true })).toString('base64'),
    });
  } else {
    await client.send('Fetch.continueRequest', { requestId });
  }
});
```

## Release interception

Resolve each paused request once. The native pause deadline is 30 seconds;
an unanswered pause fails without sending to the target. Disabling Fetch,
detaching the owning session or cancelling the document also releases pending
work. A CDP evaluation awaiting that failure needs a longer command timeout
than the pause itself.

```js
await client.send('Fetch.disable');
await client.detach();
```

## From the Rust library

The embeddable `obscura::Page` exposes `on_request` / `on_response`, an
`enable_interception()` channel, and `add_preload_script`. It uses the same
native request decisions and byte-preserving responses. See
[Use as a Rust library](Use-as-a-Rust-library.md#intercept-requests).
