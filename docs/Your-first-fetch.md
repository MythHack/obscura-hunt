`obscura fetch` loads a URL, runs its JavaScript, and prints the result.

## Load a page

```bash
obscura fetch https://example.com
```

Prints the rendered HTML.

## Run JavaScript with `--eval`

```bash
obscura fetch https://example.com --eval "document.title"
```

```
"Example Domain"
```

Returns JSON:

```bash
obscura fetch https://news.ycombinator.com \
  --eval "Array.from(document.querySelectorAll('.titleline a')).slice(0, 5).map(a => a.textContent)"
```

## Multi-statement eval

`--eval` accepts JavaScript Script statements and expressions and returns the final completion value:

```bash
obscura fetch https://example.com --eval "const links = document.querySelectorAll('a');
  Array.from(links).map(a => a.href)"
```

A declaration-only script has an empty completion value, returned as `null`. An IIFE is optional when you want an explicit return scope: `(function(){ ...; return result; })()`.

## Wait for the right moment

CLI default is `load`. For faster returns on slow sites:

```bash
obscura fetch https://my-spa.example --wait-until domcontentloaded --eval "document.title"
```

| Level              | Returns when                                  |
| ------------------ | --------------------------------------------- |
| `domcontentloaded` | HTML parsed, scripts ran                      |
| `load`             | All subresources finished (default)           |
| `networkidle2`     | ≤2 network connections active for 500ms       |
| `networkidle0`     | 0 network connections active for 500ms        |

(When driving obscura via Puppeteer or Playwright the default is `domcontentloaded` to match client expectations.)

## Scripted `fetch()`

Page JavaScript uses the current document's native origin for `fetch()` and
XHR. `mode: 'same-origin'` rejects cross-origin network URLs before a request
is sent, including interception URL rewrites and every redirect. This applies
to both the regular and stealth transports; an isolated CDP world's universal
DOM access does not disable request-mode or private-network protection.

`data:` URLs are decoded locally into a 200 response with their content type
and exact body bytes. They also work with `mode: 'same-origin'`; fragments are
excluded from the response URL and body. Decoding observes
`OBSCURA_FETCH_MAX_BODY_BYTES`. Other non-HTTP schemes are not accepted by
scripted fetch. Network and policy failures reject with `TypeError`, while an
aborted signal retains its abort reason (`AbortError` by default).

## Common flags

```
--user-agent "..."        Override the User-Agent
--timeout 30                Navigation timeout in seconds (default 30)
--wait 5                    Extra wait after the page settles, in seconds (default 5)
--selector ".main"          CSS selector to narrow output to
--proxy http://host:port    Route through a proxy
--stealth                   Stealth client (TLS fingerprint, tracker blocking)
-o, --output file.html      Write output to a file
-q, --quiet                 Suppress info logging
```

Full list: [CLI reference](CLI-reference.md).
