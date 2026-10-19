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

The PR checks workflow sets `RUSTC_WRAPPER` to an empty string: it uses the Rust
dependency cache without installing `sccache`. This overrides the repository's
wrapper default for cache repair, builds and tests. The release workflow installs
and enables `sccache` separately.

Linux-only storage tests use POSIX FIFOs; Windows results do not cover those
tests. Keep the Linux CI gate when changing storage persistence.

No-render CI must retain click, form and hover-state coverage. Computed CSS and
native child-frame coordinate input require `render`; the no-render frame
contract verifies the unsupported-input boundary and uses DOM activation for
same-document navigation.
CSS-positioned isolated-world fixtures likewise use DOM activation without
rendering while retaining shared event cancellation and checked-state assertions.

Renderer-only CSS/font resource helpers are excluded from no-render production
builds. Parsing helpers remain enabled in unit tests so the same edge cases are
covered without enabling the renderer.

The layout-only renderer is separate from a CLI no-render build. Check it with
`cargo check --release -p obscura-render --no-default-features`. Its approximate
font box stays font-sized; ascent/descent exclude CSS line-height and leading.
Keep paint-only retained metadata separate from shared layout work, including
generated content, tables, transforms and scrolling.

Native C++ builds classify the pinned V8 SDK as external/system headers while
retaining warnings for local shims. Linux validation covers both normal and
`v8_enable_pointer_compression` builds with `CXXFLAGS=-Werror`; a first-party
unused-parameter negative control must still fail compilation.

Unix UV registrations use Tokio's explicit unsafe `AsyncFd::register` API. Keep
the descriptor valid and unchanged until deregistration, and retain the existing
deregister-before-close or transfer ordering. Registration failures preserve the
existing UV error-class mapping.

### V8 dependency and snapshot alignment

The current locked family is `deno_core 0.412.0` / `serde_v8 0.321.0` /
`deno_v8 0.4.0` / raw `v8 152.2.0`, with engine version `15.2.124.1-rusty`.
The local `deno_v8` patch selects that raw engine for both core and serde;
the facade's `0.4.0` is not a Deno CLI version. Deno CLI `2.9.7` uses the same
core/serde/facade family but raw V8 `150.4.0`, not this fork's engine.

Treat an engine upgrade as a single compatibility change across the facade,
linked archive, `native/v8-152.2.0/include` SDK, C++ shims, build script,
standalone producer and runtime. The SDK and archive must have matching native
ABI settings, including pointer compression. In 152, simdutf is built in;
the Cargo feature is a compatibility no-op, not an archive variant. Remove
obsolete simdutf-specific archive overrides from external caches and mirrors.

Native builds generate a snapshot inline. To produce an external artifact,
run this from the fork workspace on the intended target or a compatible runner:

```bash
cargo run --release --locked -p obscura-js-snapshot -- --out-dir ./target/v8-snapshot
```

The producer writes `OBSCURA_SNAPSHOT.bin` and `OBSCURA_SNAPSHOT.bin.json`.
Copy both together to the build machine. For a cross build, use an artifact
produced for that build's target triple, not the build-script host. Point the
consumer at its directory with `OBSCURA_SNAPSHOT_DIR`, or at the blob with
`OBSCURA_SNAPSHOT_BIN`, but do not set both. For example, from the same checkout
and target configuration:

```bash
OBSCURA_SNAPSHOT_DIR=./target/v8-snapshot cargo build --release --locked -p obscura-cli --bins --features render
```

Producer and consumer must use the same dependency resolution, patched source
trees, bootstrap, native callback schema and effective target configuration.
Enable the producer's `v8_enable_pointer_compression` feature if the consumer
uses that configuration. The sidecar validation checks engine version, target,
native ABI, build/source fingerprints and blob identity; a mismatch is a build
error, not permission to reuse an old snapshot or substitute stock `mksnapshot`.
Regenerate artifacts after upgrading from 150.4 to 152.2, and do not edit the
sidecar to bypass its checks.

For upgrade acceptance, exercise the inline snapshot and standalone producer /
consumer paths, native Window/related-realm behavior and the existing runtime,
CDP and CLI gates in each supported build configuration. Engine alignment is
not itself a passing behavior-test result. External Rust consumers also need
the [consumer-root patches](Use-as-a-Rust-library.md#add-the-dependency).

#### Upgrade validation (October 19)

V8 152 removes `--no-validate-asm`. Keeping that obsolete leading flag made V8
ignore later initialization flags, including `--enable-queue-microtask`, and
caused valid IndexedDB transactions to abort. The core removes only the obsolete
flag; it does not replace the native microtask queue or mask transaction errors.
The consumer regression covers a committed record surviving a later transaction
abort and document replacement. The existing core snapshot test now enters its
required Tokio runtime before using the foreground-task platform. The render
resource regression holds a real SVG response body: cold and in-flight layout
must retain placeholder geometry, make one page-transport request, and update
to the decoded intrinsic geometry only after body release. It intentionally
does not use a wall-clock layout threshold, because cold font/layout setup is
not evidence of a synchronous asset wait.

Local release-mode acceptance uses isolated Windows and WSL Linux build targets:

| Gate | Windows x86_64 | Linux x86_64 |
| --- | --- | --- |
| Full `render` workspace nextest | 2494 passed, 5 skipped | 2502 passed, 5 skipped |
| No-render workspace nextest | 1490 passed, 3 skipped | 1498 passed, 3 skipped |
| No-render embeddable API nextest | 31 passed | 31 passed |
| Stealth network nextest | 138 passed | 138 passed |
| CLI release configurations | All four built | All four built |
| Layout-only renderer check | Passed | Passed |

The four CLI configurations are `render`, `render,stealth`, no default features,
and no default features with `stealth`. No-render workspace coverage excludes
`obscura` and `obscura-render`; the API suite and layout-only check above cover
those separately without enabling rendering through workspace feature unification.
The loopback gzip test runs separately with `OBSCURA_ALLOW_PRIVATE_NETWORK=1`;
the other 137 network tests retain the default private-network policy. The listed
builds and checks completed without compiler warnings.

Additional exercised paths:

- Windows focused browser/engine nextest: 138 passed. The selected upstream-core
  runtime, snapshot, inspector, value and op regressions passed 45/45 both with
  default features and with the matching optional ICU 78 data.
- Actual CLI/CDP/MCP scenarios: 11/11 in both render transport configurations.
  Mode-specific MCP input smoke passed in all four CLI configurations.
- Linux standalone snapshots were generated, consumed and executed in both
  normal and pointer-compressed builds, including native Window behavior.
  Wrong engine versions and wrong build fingerprints were both rejected.
- Date/Intl probes passed six canonical-zone, DST, precedence and fallback cases
  in both Linux snapshot modes. Windows also passed its existing whitespace
  normalization behavior. This upgrade does not add Unix timezone normalization.
- Offline obstacle course: 33/33. All 77 deterministic baseline/candidate render
  pairs succeeded, were nonblank and were pixel-identical to the 150.4.0 baseline.

The Windows performance comparison uses the tracked
`scripts/ci/perf_smoke.py` DOM and storage scenarios, identical loopback inputs,
one warmup and nine alternating baseline/candidate release-process rounds.
`--wait 0` is used for this latency-only check, not the rendering evidence above.
Each cell below is minimum / median / maximum; memory is peak working set.

| Scenario | 150.4.0 latency (ms) | 152.2.0 latency (ms) | 150.4.0 memory (MiB) | 152.2.0 memory (MiB) |
| --- | --- | --- | --- | --- |
| DOM build | 154.1 / 163.9 / 189.3 | 157.0 / 161.9 / 186.0 | 50.33 / 50.59 / 50.99 | 50.81 / 50.89 / 51.50 |
| Storage | 55.0 / 56.8 / 60.5 | 55.9 / 57.9 / 62.5 | 39.89 / 39.98 / 40.11 | 40.14 / 40.22 / 40.43 |

No material regression was observed in these two scenarios. The timing changes
are within the repository's roughly 10% noise floor, not evidence of a general
engine speedup.


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

#### Closed popovers and independent script readiness

Popover visibility is native DOM state shared by selectors, computed style and
layout. Closed popovers receive the UA hidden default; existing show/hide
transitions invalidate rendering, preserve cancelled `beforetoggle` events and
respect author display overrides. This does not add a top-layer framework.

Parser scripts no longer wait for every external classic fetch to finish.
Already-ready work advances while unrelated async bodies remain pending, using
the existing concurrency cap and absolute deadline. The same bounded fetch
stream keeps progressing during module preparation without executing JavaScript
through an already borrowed runtime. Pending bodies are cancelled at expiry.

The focused consumer regressions are
`popover_transitions_share_native_selectors_styles_and_header_layout` and
`pending_async_classic_does_not_block_ready_parser_and_deferred_scripts`.
An async script's earlier DOM position does not establish response readiness;
the old import-map test that depended on global prefetch completion was removed,
not changed to another timing-dependent expected result. Other real-site
differences remain outside this bounded repair.

Final release verification for these two fixes: 242/242 focused tests and
2493/2493 workspace tests passed, with 5 skipped. Both `render` and
`render,stealth` binaries passed the actual popover transition and held-body
script-order scenarios, including native connection cancellation. The existing
CDP window boundary smoke also passed with `--stealth`. The offline obstacle
course passed 33/33; all 77 deterministic rendering fixtures passed the behavior
checker and produced identical output to the pre-two-fix baseline. No new live
site fidelity claims are made for this bounded change.

#### Native WindowProxy, percentage margins and default fonts (October 18)

Percentage margins retain their percentage/auto type through native layout and
resolve against the actual containing block. Used values are shared with CSSOM
and painting instead of freezing an estimated parent width before layout.

Rendering discovers installed system fonts by default and caches the font
database per process. Body text, measurement, form controls and SVG use the same
policy. Global, repeatable `--font-dir` and `--no-system-fonts` options apply to
`fetch`, `serve`, `scrape` workers and `mcp`; explicit font directories remain
available in fixed-font mode. Default output is platform-dependent by design.

Window identity now uses a stable native V8 global proxy, not a JavaScript
facade that only normalizes direct return values. Snapshot-restored access
handlers retain cross-origin restrictions, and DOM grants do not authorize
foreign globals. The live root context owns evaluation and queued work after
document replacement.

Frame navigation coverage assigns `iframe.src` over real HTTP, rather than only
calling native document replacement directly. It retains window identity across
foreign documents, same-origin return and same-URL reload, and preserves the
committed document after a failed or superseded load. The loading shim no longer
constructs a redundant parent-realm document after queuing a native commit.

Mixed inline SVG/text coverage includes adjacent links with leading and trailing
whitespace, nested text nodes, inline margins and shrink-to-fit flex items. A
six-case CLI comparison against Chromium catches duplicate collapsible spaces
across node boundaries; all six cases pass after sharing formatting-context
state instead of normalizing each text node independently.

Nested-table coverage checks the parent cell's exact vertical extent and origin,
not only a minimum height. Formatting-only indentation around block children
must not add a shaped text line when an internal flex box represents block flow.
The unchanged screenshot fixture verifies the 24px header and 15px following row.

Fresh Windows x86_64 release processes passed the three original popup, margin
and CJK counterexamples. The popup probe covers nested values, function returns,
`defaultView`, Map keys, same-origin to cross-origin to same-origin navigation,
message sources and retained references after close. Font-policy coverage spans
CLI/CDP/MCP/scrape, two-worker propagation, custom fonts, CJK text, controls, SVG
and invalid-directory rejection before target requests.

| Earlier verification snapshot | Observed result |
| --- | --- |
| Release/render workspace nextest | 2492 passed, 0 failed, 5 skipped |
| Focused window/frame/font/margin/whitespace/table/observer nextest | 298 passed |
| CLI release binaries | `render` and `render,stealth` built successfully |
| Native popup identity | 32/32 in each transport mode |
| Actual CLI/CDP/MCP protocol scenarios | 11/11 in each transport mode |
| Font-policy acceptance | 91/91 render; 92/92 with `--stealth` |
| Offline obstacle course | 33/33 |
| Deterministic rendering corpus | 77 current/baseline/Chromium triples captured, all nonblank; behavior checker passed |
| Standalone native snapshot producer | Wrote the snapshot blob and metadata sidecar |

Stealth acceptance runs explicitly pass `--stealth`. Rendering fixture results
are behavior and geometry gates, not a claim of pixel-identical browser output.

The final live footer captures show Bootstrap's five columns on one row, Ember's
five sponsor SVGs on one row with at most 0.5px horizontal deviation from Chromium,
and readable Korean, Japanese and Chinese labels on Angular. The live corpus is
not an all-pass gate: failed requests, skipped scripts, blank pages, different
menu/regional states and unstable capture boundaries are excluded from fidelity
conclusions rather than counted as passes. Truncated subtrees are likewise not
used for full-subtree geometry verdicts.

Observer regression fixtures await callback/task completion with a bounded
failure watchdog. They do not assume that first system-font layout completes
inside a fixed wall-clock interval; pre-render and exact delivery-order
assertions remain. CDP cancellation probes wait only for events after the
current navigation's cursor, never an earlier blank-document completion.

#### Earlier browser-repair validation after synchronization

The pre-cutover October 17 worktree passed the release/render workspace gate:
`cargo nextest run --release --features render --no-fail-fast` reports **2460
passed, 0 failed, 5 skipped**. A focused frame, popup, script, font, sandbox,
observer, interception and rendering selection passes **345/345** without
changing the workspace feature combination. The merge-stage failures below
are historical results, not the status of this repaired tree.

The repairs retain parked V8 runtimes rather than reconstructing their state,
use Script completion values for evaluation, preserve native frame ownership
and resource bases, and keep script-triggered CDP navigation on the interception
reply path. Sandbox property assignment reflects into the attribute; a
script-disabled child cannot run an inline handler through a parent-initiated
click. IntersectionObserver delivery remains asynchronous; regression tests
wait for observable transitions instead of a fixed timer race.

Window messaging captures its sender at a native entry before serialization.
Each realm receives its own native function so bare calls use that Window and
borrowed calls retain their explicit receiver. Isolated-world universal-access
grants retain their authorized backing adapter; they do not expose a foreign
raw V8 global. No initial-document identity cache or navigation fallback was
added.

Both exact release CLI builds completed with `--features render` and
`--features render,stealth`. Fresh CLI/CDP/MCP processes exercised both binaries,
with `--stealth` explicitly enabled for the latter:

| Verification | Result |
|---|---|
| CLI/CDP/MCP consumer scenarios | 11/11 in each mode |
| Native safety, file navigation, retained realms and GC | 17/17 in each mode |
| DOM ownership and cross-origin boundaries | 12/12 in each mode |
| Redirect modes and target request receipts | 5/5 in each mode |
| Profile isolation and persistence | 23/23 checks in each mode |
| Late child CSS, scripts, fonts and images | 7/7 in each mode; one HTTP hit per resource |
| Sandbox/observer DOM scenarios | 4/4 in each mode |
| Isolated-world grant upgrade and ordinary-world isolation | Passed in each mode |
| Offline obstacle course | 33/33 |

The CDP interception stress probe also completed five iterations of each of
`Page.navigate`, `Page.reload`, `Runtime.evaluate` and `Runtime.callFunctionOn`,
servicing 64 paused requests per command with collision-free request IDs.

Rendering produced nonblank output for all 75 deterministic fixtures with no
configured geometry-check failures. Seventy-two captures are pixel-identical
to the retained pre-repair baseline; the three changed cases are form-control
geometry, quirks controls and modern containing-block triggers. Native control
appearance and quirks-mode input sizing are not pixel-identical to Chromium.
The 15-site top/bottom suite used 1440x1000 viewports, 3000 ms settling and a
fixed animation boundary. Of 30 pairs, 21 were eligible for comparison and 9
were excluded for contentless images or incomparable page state. Visual review
still shows real-site layout, text and footer differences. Capture eligibility
and pixel-distance metrics are not a claim of complete rendering parity.

At that stage, an additional messaging probe recorded a limitation: the stable
facade returned by same-origin `window.open()` can differ from the popup's own
Window object exposed through a nested value or a self-message. The same
identity mismatch was observed in the pre-repair binary. Popup message data,
sender attribution and realm-local self-messaging pass; full cross-world
WindowProxy identity is not certified by this repair. Diagnostic artifacts
and generated captures remain outside the repository.

#### Merge-stage record: upstream `bd95294`

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

Separate real CDP server checks passed 23/23 in each transport mode, including
`--stealth`: concurrent connections keep Web Storage and IndexedDB isolated,
disconnect merges persist both clients' deltas, page reloads do not overwrite
the shared profile, and restart preserves edits and deletions. This server
profile path is distinct from the failing CLI persistence scenario below.

Actual CLI/CDP/MCP process checks passed six scenarios, failed four and skipped
one user-WIP scenario in each transport mode, using `--stealth` for the stealth
executable. These binaries contain only the merge. Passing paths
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

After restoring the four original worktree changes, focused coverage again
passed 36/36 and both release CLI builds completed. The full release nextest
run reported 2239 passed, 209 failed and five skipped. The additional failure,
`intercepted_request_bursts_and_script_navigation_keep_servicing_replies`, also
failed in isolation at `Runtime.evaluate` after zero intercepted requests.
Retained pre-merge runs also show this test failing with zero intercepted
requests, so the failure predates this merge. That does not establish its root
cause or whether the restored changes affect its frequency. The restored
worktree result is not green.

Actual restored-worktree CLI boundary checks passed 12/12 in each transport
mode. Redirect checks passed 5/5 in each mode: manual redirects expose an opaque
response without issuing the target request, error mode rejects, and follow
mode alone reaches the target. Blob/File popup transport also passed in both
modes. The complete CLI/CDP/MCP smoke passed seven scenarios and failed the same
four listed above in each mode, without skipping the user-WIP scenario. The
restored render executable again scored 32/33 in the offline obstacle course,
with the same timezone mismatch.

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
snapshot as needed. If `OBSCURA_SNAPSHOT_BIN` or `OBSCURA_SNAPSHOT_DIR` is set,
touching the bootstrap does not replace that external artifact: regenerate its
blob and sidecar with the matching target-native producer, or unset the override
for an inline native build. Do not copy generated snapshots between engines,
revisions or feature configurations. See
[V8 dependency and snapshot alignment](#v8-dependency-and-snapshot-alignment).

### `Target.createTarget timed out`

Lock contention in the dispatcher. Should not happen on current main. If it does, run with `RUST_LOG=obscura_cdp=trace`, look for handlers that hold `v8_lock` across long awaits.

### `page.goto()` returns `null` from Puppeteer

Means `Network.requestWillBeSent` for the main document did not arrive with `requestId == loaderId`. Check `do_navigate` in `crates/obscura-cdp/src/domains/page.rs`.

### `Cannot find context with specified id`

Playwright's local context counter diverged from the server's `valid_context_ids`. Each navigation must allocate a fresh `executionContextId`. Check `ctx.next_isolated_context()` is called on every nav.

### `V8_Fatal: heap->isolate() == Isolate::TryGetCurrent()`

Two pages tried to use V8 concurrently. The `v8_lock` was bypassed, or direct `JsRuntime` access entered a different isolate while one was active. Parking a page retains its runtime and does not enter or drop the isolate. Search for direct `JsRuntime` access outside the lock.

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
