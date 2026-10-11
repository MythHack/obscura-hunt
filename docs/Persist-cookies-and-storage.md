`--storage-dir` persists cookies to disk. Web Storage is currently in memory:
`localStorage` is shared by same-origin pages in one browser context, while
`sessionStorage` belongs to each top-level window. A related new window copies
its opener's session namespace once; later changes are independent. Navigation
preserves each window's origin-partitioned namespace. `noopener` starts empty,
and opaque-origin documents cannot access Web Storage.

## CLI

```bash
obscura fetch https://example.com --storage-dir ./obscura-data
obscura fetch https://example.com --storage-dir ./obscura-data
```

The second invocation restores the cookies left by the first. It does not restore
`localStorage` or `sessionStorage` from the previous process.

## Server

```bash
obscura serve --storage-dir ./obscura-data
```

Pages in the default browser context share its cookie jar and live localStorage.
Separate CDP browser contexts have isolated mutable state. Use different
`--storage-dir` paths for separately persisted cookie profiles.

## Layout

Inside `./obscura-data`:

- `cookies.json`: cookie jar in a stable format with `same_site`, `expires`, `http_only`, `secure`.

The format is stable. Inspect with `jq`:

```bash
jq '.[] | select(.domain == "example.com")' ./obscura-data/cookies.json
```

## When state is written

- Cookies are saved through the browser context's persistence path, including
  graceful shutdown. Forced process termination is not a persistence guarantee.
- Neither Web Storage namespace is written to disk.

## Login once, scrape many

```bash
obscura serve --storage-dir ./session-1
```

Drive a cookie-based login flow once via Puppeteer or Playwright, then stop the
server gracefully. Subsequent runs restore those cookies; logins that also need
Web Storage must recreate that state.

## Multiple identities

```bash
obscura serve --port 9222 --storage-dir ./identity-a
obscura serve --port 9223 --storage-dir ./identity-b
```

## Clear state

```bash
rm -rf ./obscura-data
```
