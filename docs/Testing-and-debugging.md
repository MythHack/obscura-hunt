## Test suites

### Rust unit and integration

```bash
cargo nextest run --release --features render --no-fail-fast
```

Crate-scoped:

```bash
cargo nextest run --release --features render -p obscura-cdp
cargo nextest run --release --features render -p obscura-browser
```

By name, also selecting the crate and test target:

```bash
rtk proxy env CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo nextest run --release --features render -p obscura-js --lib completed_scripts_do_not_wait_for_watchdog_deadline
```

Use `cargo nextest`, not `cargo test`. Runtime tests require process isolation
because the engine owns one V8 isolate per process. Render tests must run in
release mode; debug builds are not a fidelity or performance gate.

### Fast edit/test loop

For runtime unit tests, the `test-js` Cargo alias enables incremental compilation
only for `obscura-js`, retaining release optimization and reusing dependencies:

```bash
rtk proxy env -u CARGO_INCREMENTAL CARGO_BUILD_JOBS=2 cargo test-js -E 'test(classic_script_url_is_dynamic_import_referrer)'
```

The first invocation warms an extra compiler cache. Later source edits reuse it.
Unset `CARGO_INCREMENTAL`: an exported `0` overrides the alias's package setting.
This is a local feedback command, not the production benchmark build. Cargo
defaults to more code-generation units with incremental compilation, so do not
use its timings as performance evidence. No global release setting changes.

For other crates, select the affected binary before applying a test-name filter.
CDP integration tests share one binary; select a module within it:

```bash
rtk proxy env CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo nextest run --release --features render -p obscura-cdp --test integration -E 'test(page_frame_contract::)'
```

`-p` selects packages; `--lib` or `--test NAME` selects binaries to compile.
A bare test name or `-E` filters test execution, not the Cargo build target set.
Keep the same package/features while iterating. Workspace tests and the CLI can
resolve different dependency features, requiring distinct engine artifacts.
Do not enable extra production features merely to unify those artifacts.

Run the focused repro after each edit. At a stable candidate, run the full
release nextest gate, exact CLI build and obstacle course from `AGENTS.md`, plus
render/stealth verification when applicable. Do not repeat the whole gate at
each red/green step, and do not omit it when declaring a candidate validated.

For build attribution, append `--timings` to Cargo or nextest; inspect
`target/cargo-timings/`. Preserve caches: avoid `cargo clean`, changing Rust flags,
or switching profiles unless those changes are intentional. Incremental caches
consume extra disk. See Cargo's [profile settings](https://doc.rust-lang.org/cargo/reference/profiles.html)
and nextest's [target selection](https://nexte.st/docs/running/).

### CDP parity tests

`crates/obscura-cdp/tests/*.rs` exercise CDP methods end-to-end with a real
`dispatch` call and an in-process HTTP server. `integration.rs` registers these
files as modules in one binary to avoid repeatedly linking the browser engine.
Nextest still runs each test in its own process. Register new files in its
`integration_tests!` list; a guard fails if a sibling Rust test file is omitted.
Old `--test FILE` commands become `--test integration -E 'test(FILE::)'`.
The tradeoff is that editing one CDP test file rebuilds the shared test binary;
the consolidation targets engine-change rebuild fan-out and artifact duplication.

Pattern:

```rust
#[tokio::test(flavor = "current_thread")]
async fn my_test() {
    std::env::set_var("OBSCURA_ALLOW_PRIVATE_NETWORK", "1");
    let url = serve_once().await;
    let mut ctx = CdpContext::new();
    let page_id = ctx.create_page();
    let session_id = "session-1";
    ctx.sessions.insert(session_id.to_string(), page_id.clone());

    cdp(&mut ctx, 1, "Page.navigate", json!({"url": url}), session_id).await;
    // assertions
}
```

`serve_once` and `cdp` helpers are copied across the parity tests; reuse them.

### Upstream synchronization verification

#### Latest update: upstream `bd95294`

The October 17 synchronization incorporates 69 upstream commits into fork
`85f3d10`, retaining the fork's native window/frame ownership and cancellation
boundaries. The four pre-existing worktree changes were kept separate from the
merge. Both release CLI builds completed with `render` and `render,stealth`.
Focused IndexedDB, frame lifecycle, sandbox, document ownership and transport
coverage passed 36/36. This includes trusted IndexedDB events through the real
bootstrap, connected native owner checks for queued frame cleanup, opaque frame
constructor selection, import/adopt ownership and malformed-XML error documents.
The integration uses the fork's closure-private event state rather than the
removed upstream global helper.

The final merge-only full release nextest run was not green: 2240 passed, 208
failed and five were skipped. The failures include two GC-stress teardown
process aborts also present in the earlier report. Of the failing names, 187
appeared in that report and 21 were newly tested; none had previously passed.
Name comparisons do not establish identical causes or certify inherited status.
All nine previously passing tests that failed in the earlier merge candidate
passed after the integration repairs and an offline run with proxy environment
variables cleared. No test assertion was weakened or re-pinned.

Actual loopback CLI boundary checks passed 12/12 in each transport mode,
including `--stealth`: imported and adopted nodes retain the right document,
malformed XML exposes a document-owned `parsererror` without an HTML body,
and opaque sandbox frames retain a non-null WindowProxy, load successfully,
and deny access in both directions.

Actual CLI/CDP/MCP process checks passed seven of eleven scenarios in each
transport mode, using `--stealth` for the stealth executable; one user-WIP check
was skipped because these binaries contain only the merge. Passing paths
include IndexedDB transactions across navigation, popup messaging, first-load
cancellation and MCP tab/timer preservation. Profile localStorage persistence,
nested srcdoc access, removed Attr ownership and Fetch interception remained
failing; the same four scenarios also failed with the saved committed pre-merge
executable. No failed assertion was converted into an expected pass. The
offline obstacle course remained 32/33, with the same Windows timezone mismatch
(`Asia/Shanghai` instead of `Europe/Berlin`) as the pre-merge control.

All 75 deterministic render fixtures produced nonblank paired captures against
the pre-merge executable and Chromium. Of these, 74 were pixel-identical to the
pre-merge output. The remaining inline-block fixture improved to match Chromium.
The behavior checker still reported ten Chromium-side expectation failures and
the existing Obscura perspective-containing-block failure. Representative
captures attempted all 15 sites at both top and bottom positions; nine pairs at
each position met the comparison gates and matched pre-merge pixels.
Blank captures, different live documents, resource outcomes and unstable
capture boundaries were excluded, not counted as fidelity passes. The merged
Porkbun captures ended at a tracker URL. A loopback named-form-target control
navigated the top-level page with both executables, proving that defect exists
before the merge, but not explaining its activation on the live site. Termius
produced a blank top capture and an unstable bottom comparison. These live-site
observations remain unresolved, not certified as harmless noise.

#### Earlier update: upstream `8edfe48`

The sync of upstream [`8edfe48`](https://github.com/h4ckf0r0day/obscura/commit/8edfe480133b383f70e17f9d4ea55e3592cdfd24)
into fork `bed6fc42` was verified on Windows with release builds and the retained
local lockfile. Both `render` and `render,stealth` CLI builds completed.
The final focused lifecycle, object-handle, module-network and native-window
selection passed 47/48, including first-navigation cancellation without an
existing runtime. Its remaining module-receipt test also failed before the
sync on JSON number representation (`42.0` versus `42`). Actual CLI/CDP
processes passed eleven loopback scenarios
in each transport mode, including cancellation of held document/script
connections, first navigation without enabling Runtime, beforeunload
dismissal/acceptance, popup messaging and nested frame access.

The final full release nextest run was not green: 2106 passed, 189 failed and
five were skipped. Every final failing test name also appeared in pre-sync
baseline failures. Timing-sensitive tests varied across parallel and serial
runs; a failure-name comparison does not establish identical causes or certify
inherited failures as acceptable.
The obstacle course passed 32/33: its fingerprint fixture expected
`Europe/Berlin`, while Windows returned `Asia/Shanghai` despite
`OBSCURA_TIMEZONE=Europe/Berlin`. The saved pre-sync executable failed the
same fingerprint control.

All 73 deterministic render fixtures produced paired screenshots. The behavior
checker reported ten Chromium-side assertion failures and one Obscura-side
perspective containing-block failure. That Obscura fixture produced identical
pixels with an exact pre-sync production-source build. Representative captures
attempted all 15 sites per position, with ten top and eleven bottom page pairs
initially eligible. Failed requests, blank pages, unequal or unstable live
states, and differing resource outcomes are excluded from fidelity conclusions.
The original Qwik bottom capture had unequal image readiness and is excluded.
A bounded paired capture with the same timing and viewport completed all
36 images in both native builds and produced pixel-identical avatar crops.

## Logging

```bash
RUST_LOG=obscura=info  obscura serve
RUST_LOG=obscura=debug obscura serve
RUST_LOG=obscura_cdp=trace,obscura_browser=debug obscura serve
```

Logs go to stderr.

`--verbose` on any subcommand is equivalent to `RUST_LOG=obscura=info`.

## Driving the CDP server manually

```bash
obscura serve --port 9222 --verbose
```

In another shell:

```bash
wscat -c ws://127.0.0.1:9222
> {"id":1,"method":"Target.createTarget","params":{"url":"about:blank"}}
> {"id":2,"method":"Target.attachToTarget","params":{"targetId":"...","flatten":true}}
> {"id":3,"sessionId":"...-session","method":"Page.navigate","params":{"url":"https://example.com"}}
> {"id":4,"sessionId":"...-session","method":"Runtime.evaluate","params":{"expression":"document.title"}}
```

Useful for reproducing what Puppeteer or Playwright is doing without their abstraction.

## Common failure modes

### JavaScript behavior disagrees with the current bootstrap source

The runtime embeds a generated V8 snapshot, not `bootstrap.js` directly.
Restored build artifacts can contain an older snapshot even when the Rust ops
are current. Confirm which `OBSCURA_SNAPSHOT.bin` the binary's dependency file
under `target/release/deps/` references before changing engine behavior.
To regenerate the snapshot while retaining dependency caches:

```bash
rtk proxy touch crates/obscura-js/js/bootstrap.js
rtk proxy env CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo build --release -p obscura-cli --bins --features render
```

Then rerun the failing nextest command, which rebuilds its own feature-specific
snapshot as needed. Do not copy generated snapshots between revisions or
feature configurations.

### `Target.createTarget timed out`

Lock contention in the dispatcher. Should not happen on current main. If it does, run with `RUST_LOG=obscura_cdp=trace`, look for handlers that hold `v8_lock` across long awaits.

### `page.goto()` returns `null` from Puppeteer

Means `Network.requestWillBeSent` for the main document did not arrive with `requestId == loaderId`. Check `do_navigate` in `crates/obscura-cdp/src/domains/page.rs`.

### `Cannot find context with specified id`

Playwright's local context counter diverged from the server's `valid_context_ids`. Each navigation must allocate a fresh `executionContextId`. Check `ctx.next_isolated_context()` is called on every nav.

### `V8_Fatal: heap->isolate() == Isolate::TryGetCurrent()`

Two pages tried to use V8 concurrently. The `v8_lock` was bypassed, or a handler suspended a JS runtime while another isolate was entered. Search for direct `JsRuntime` access outside the lock.

### Test hangs

A handler is awaiting something that never resolves. Run with `RUST_LOG=obscura=trace` and check the last log line before the hang.

## Reproducing user bug reports

The integration suite in `tests/test_all.py` is the fastest path from a one-line repro to a regression test. Add the failing case as a new test function, get it failing, then fix.

For Puppeteer / Playwright bug reports, the user's repro script usually drops straight in. Save it as `tests/repro_<issue>.js`, run with `node`, fix until it passes.

### Rendering regressions

Start with the committed deterministic fixtures, then use the representative
real-site suite at both the top and bottom of pages:

```bash
RUN_ROOT="$(mktemp -d)"
OBSCURA_BIN=./target/release/obscura render-repros/run.sh "$RUN_ROOT/fixtures"
OBSCURA_BIN=./target/release/obscura render-repros/representative-suite/run.sh "$RUN_ROOT/top"
OBSCURA_BIN=./target/release/obscura render-repros/representative-suite/run.sh "$RUN_ROOT/bottom" bottom
```

Set `BASELINE_BIN` or `CHROMIUM_BIN` when producing paired captures. Keep the
viewport, user agent, settle policy, scroll position, animation sample, and
capture boundary identical. A pixel-distance score is a regression tripwire,
not a verdict: verify both engines succeeded and produced nonblank output,
then inspect missing resources, geometry, structural edges, and a reduced
fixture. Do not add hostname-specific render branches.

## Profiling

CPU with `perf` and a flamegraph:

```bash
cargo build --release --features render
perf record -F 99 -g -- ./target/release/obscura fetch https://heavy-spa.example
perf script | flamegraph.pl > flame.svg
```

Memory with heaptrack:

```bash
heaptrack ./target/release/obscura serve
```

Tokio task inspection:

```bash
RUSTFLAGS="--cfg tokio_unstable" cargo build --release --features render
./target/release/obscura serve
# in another shell
tokio-console
```

Requires the workspace `tokio` dependency to be built with the `tracing` feature; not enabled by default, add it in the relevant `Cargo.toml` before profiling.
