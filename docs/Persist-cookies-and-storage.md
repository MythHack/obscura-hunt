`--storage-dir` persists cookies and `localStorage` so they survive across runs.
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

The second invocation starts with the cookies and `localStorage` left by the first.
It does not restore `sessionStorage` from the previous process.

## Server

```bash
obscura serve --storage-dir ./obscura-data
```

Pages in the default browser context share its cookie jar and live localStorage.
Separate CDP browser contexts have isolated mutable state. Use different
`--storage-dir` paths for separately persisted cookie profiles.

For MCP, put the profile flag before the subcommand:

```bash
obscura --storage-dir ./obscura-data mcp
```

Both MCP transports accept `OBSCURA_STORAGE_DIR`; explicit `--storage-dir`
takes precedence. Without either setting, storage remains in memory and no
profile is written. Existing cookies and origin localStorage load before the
first request or page script. A corrupt, unreadable, invalid-origin or
over-quota profile is a startup error, not an empty replacement session.

## Rust context construction and saves

The lower-level `obscura_browser::BrowserContext::with_storage`,
`with_storage_full`, and `with_storage_and_network` constructors return
`std::io::Result<BrowserContext>`. Handle or propagate the result before creating
pages; invalid profiles are not silently replaced with empty state. The
memory-only `BrowserContext::new` constructor still returns the context directly.

```rust
let context = obscura_browser::BrowserContext::with_storage(
    "profile".into(),
    Some(std::path::PathBuf::from("./obscura-data")),
)?;
// After the embedding application's page work has completed:
context.save_storage()?;
```

`save_storage()`, `save_cookies()` and `save_local_storage()` return
`std::io::Result<()>`; callers must handle commit failures. `save_storage()`
attempts both stores, but is not an all-or-nothing transaction across files.
Successful units remain committed, failed localStorage origins remain dirty,
and an error does not roll back the live browser state. Embedders must explicitly
order their final save after page work; dropping a Rust context is not a
fallible clean-shutdown API. The `obscura::Browser` builder's `.build()` also
propagates persistent-profile startup failures.

## Layout

Inside `./obscura-data`:

- `cookies.json`: cookie jar in a stable format with `same_site`, `expires`, `http_only`, `secure`.
- `localStorage/<origin>-<digest>.json`: one file per origin, holding
  `{"origin": "...", "entries": [["key", "value"], ...]}`. The digest is an FNV-1a
  hash of the origin, avoiding collisions caused by filename sanitization.
  This implementation retains entries in insertion order.

Inspect one with `jq`:

```bash
jq '.entries' ./obscura-data/localStorage/*.json
```

The format is stable. Inspect with `jq`:

```bash
jq '.[] | select(.domain == "example.com")' ./obscura-data/cookies.json
```

## When state is written

- After a successful navigation, cookies and origin `localStorage` are committed
  before the navigation operation reports success. Unchanged localStorage
  origins are not rewritten.
- On clean process exit: MCP stdio EOF, and handled Ctrl-C/SIGTERM for MCP HTTP
  and the CDP server. Completed page work and post-navigation evaluation changes
  are ordered before this final commit. Future timers and infinite tasks are
  not an unbounded shutdown barrier.
- Existing CDP cookie/storage mutations and explicit storage-state imports
  report commit errors through the operation that requested them.

Each storage file is replaced atomically, not truncated in place. A failed
write or replacement leaves the previous committed unit loadable, and an
unsaved origin stays dirty for a later normal commit. This is not a transaction
across cookies and every origin file. Deleting cookies, removing items and
clearing an origin are persisted too, so deleted data does not return on restart.

Profile load, creation, write, replacement, removal and permission failures are
operator-visible errors. CLI and clean-exit failures return nonzero and print
an error to stderr; MCP/CDP operations return an error. A commit failure means
the page's memory may already have changed but was not fully saved. Error
messages identify the affected file without printing cookie values.

Managed profile directories use mode `0700` and files `0600` on Unix. Windows
uses a protected DACL granting only the current user and LocalSystem access,
including replacement of existing permissive files. Permissions are established
and checked explicitly; unsupported mounts or denied permission changes fail
rather than silently retaining broad access. Unrelated parent directories are
not repermissioned. Use a dedicated profile directory, not a shared data folder.

An unhandleable forced kill only preserves units whose commit already
succeeded. It does not guarantee uncommitted memory changes.

`sessionStorage` is deliberately not written to disk. It belongs to one tab and
is dropped when the tab goes away, the same as a real browser, but it does
survive navigation and CDP target switching within the process
([#678](https://github.com/h4ckf0r0day/obscura/issues/678)).

Web Storage is limited to 5 MiB and 16,384 entries per origin, 64 MiB per
backing store and 256 origins. Failed profile writes and removals stay pending
for a later flush. The native budgets count UTF-16 key/value bytes together
with per-entry and per-origin metadata. Strings containing unpaired UTF-16 units
or starting with NUL use a NUL-prefixed JSON string escape internally, including
in profile files; the JavaScript API decodes them losslessly. Normal strings are
stored unchanged. Encoded strings consume more of the byte budget.
Shared contexts serialize flushes with storage mutations. Use separate profile
directories for independent browser processes; this is not a cross-process
database. Opaque-origin documents cannot access either store.

Automatic cross-document `storage` events are not implemented by this change.

## IndexedDB

IndexedDB data lives in memory, shared by pages in the same browser context and
origin. It survives navigation, but not process restart. Incognito contexts do
not share it. `--storage-dir` does not persist IndexedDB.

Transactions stage record changes and publish them atomically on completion;
aborting also rolls back schema changes and generated keys. Values use structured
serialization, including typed keys, cyclic graphs, files and symmetric crypto
keys. Readonly writes and uncloneable values fail instead of reporting success.
Upgrades notify existing connections and wait for them to close.

The context has a 64 MiB budget for committed records, schema and pending writes,
including serialized-value and record overhead. Individual serialized values
are limited to 16 MiB and encoded keys to 64 KiB. Quota failures abort an
unhandled write transaction, leaving its committed records unchanged.

This is not full IndexedDB conformance. Transactions currently serialize per
database, including readonly transactions; index queries and cursor steps scan
records rather than using a native index. Store/index renaming,
`continuePrimaryKey()`, the newer bulk-read options dictionaries, and on-disk
durability are not implemented. Database and schema names containing unpaired
UTF-16 units are not supported. These limits do not change the behavior of
cookies or Web Storage.

## Login once, scrape many

```bash
obscura serve --storage-dir ./session-1
```

Drive a login flow once via Puppeteer or Playwright, then stop the server
gracefully. Subsequent runs restore its cookies and `localStorage`.
`sessionStorage` and IndexedDB must be recreated after a process restart.

## Multiple identities

```bash
obscura serve --port 9222 --storage-dir ./identity-a
obscura serve --port 9223 --storage-dir ./identity-b
```

## Clear state

```bash
rm -rf ./obscura-data
```
