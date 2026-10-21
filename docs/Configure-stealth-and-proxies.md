## Stealth mode

```bash
obscura fetch https://example.com --stealth
obscura serve --stealth
obscura scrape url1 url2 --stealth
obscura mcp --stealth
```

`--stealth` is a global flag, so it works before or after the subcommand and applies to `fetch`, `serve`, `scrape`, and `mcp`. In a `scrape` run each worker inherits it.

An explicit `--user-agent` applies to HTTP requests and `navigator.userAgent`
in either transport; a subcommand value overrides the value before the
subcommand. `scrape` workers inherit it. The two CDP user-agent override
spellings also retain platform and weighted language settings across native
worlds, frames and subsequent documents, without replacing their V8 state.
Stealth changes the TLS persona, not the selected request identity. Classic
scripts, forms (including empty POST bodies) and CORS OPTIONS use the same
selected native client as navigation, modules and rendering resources.

What `--stealth` changes:

- Uses the wreq HTTP client with browser-matching TLS fingerprints (ClientHello, ALPN, cipher order).
- Loads a tracker blocklist that drops requests to known analytics and fingerprinting endpoints.
- Bundles webpki roots instead of relying on the system store.

Requires a build that includes the stealth feature. Use a `-stealth` archive
with rendering or a `-no-render-stealth` archive without it. To build the
rendering variant yourself:

```bash
cargo build --release -p obscura-cli --bins --features render,stealth
```

Omit rendering with `cargo build --release -p obscura-cli --bins --no-default-features --features stealth`.

## What stealth handles

- Basic bot detection that checks TLS fingerprint or User-Agent.
- Sites that rely on third-party analytics being reachable.

## What stealth does not handle

- Cloudflare interactive challenges.
- Datadome and Akamai bot manager active challenges.
- CAPTCHAs.
- IP-based rate limiting (use proxies).
- **Passive TCP/IP stack fingerprinting.** Classifiers that inspect the raw SYN
  packet (tcp_options order, window size, window scaling, ip_id) can distinguish
  Linux from macOS even when the TLS and HTTP layers are identical, because those
  fields are set by the OS kernel, not by Obscura. The same binary on Linux
  (`tcp_options: "M1460,S,T,N,W10"`, window 64240) and macOS
  (`tcp_options: "M1460,N,W6,N,N,T,S,E,E"`, window 65535) produces a different
  passive fingerprint. Obscura operates in userspace and cannot override these
  kernel defaults. If TCP/IP passive detection is a concern, run Obscura inside
  a container or VM whose kernel matches the target fingerprint, or route
  connections through a proxy whose TCP stack presents the desired profile.

## Proxies

HTTP proxy:

```bash
obscura fetch https://example.com --proxy http://proxy.example.com:8080
obscura serve --proxy http://proxy.example.com:8080
```

With auth:

```bash
obscura fetch https://example.com --proxy http://user:pass@proxy.example.com:8080
```

SOCKS5:

```bash
obscura fetch https://example.com --proxy socks5://proxy.example.com:1080
```

## Local file and address permissions

File access is denied by default, including raw `fetch --dump original`.
Use `--allow-file-access` for an explicit local file workflow or a trusted
service. The grant is checked before filesystem metadata or contents are
read and is copied with the browser context. It does not let a network,
data or blob document redirect, submit a form or rewrite a request into a
file URL.

`--allow-network CIDR` may be repeated or contain comma-separated ranges.
`OBSCURA_ALLOW_NETWORK` uses the same syntax; command-line ranges are added
to it. These are narrow exemptions from the shared private/reserved-address
deny-set, not a firewall for otherwise allowed public addresses. URL literals,
actual DNS answers and every redirect must be authorized; one forbidden DNS
answer rejects the lookup. Invalid ranges fail startup and `/0` emits a
warning. `--allow-private-network` / `OBSCURA_ALLOW_PRIVATE_NETWORK` retain
their broader opt-in behavior.

```bash
obscura --allow-network 127.0.0.1/32,::1/128 fetch http://localhost:8080
obscura --allow-file-access fetch file:///path/to/local.html
```

Proxy routes perform a local destination-policy check too. A remote proxy's
DNS resolution cannot be pinned by this local check; the proxy itself remains
part of the trusted, explicitly selected route. Native clients do not silently
fall back to a direct connection or a different TLS transport.

## Request interception

Public Page interception and CDP Fetch pause native navigation, forms,
scripts, styles, imports, images, fonts and fetch/XHR before target I/O.
Continue overrides are revalidated before sending, while Network blocked-URL
patterns remain a separate deny operation. A disconnected or unanswered
interceptor terminates without sending the request. Public Fulfill bodies are
`Vec<u8>`; only the CDP wire encodes/decodes base64. JavaScript consumers receive
the original response bytes, with MIME-aware text decoding only at text
consumers and the usual CORS/opaque-response visibility rules.


## Certificate-chain recovery

Ordinary and stealth TLS connections can recover a missing intermediate from
the certificate's HTTP(S) AIA issuer URLs. Recovery happens during the original
verified handshake, before any navigation, fetch, OPTIONS or POST bytes are
sent. There is no verification-disabled HEAD probe, alternate transport, or
application-request replay.

Fetched DER, PEM and PKCS#7 certificates are signature-checked against the
certificate they issue and supplied only as **untrusted intermediates** to a
fresh full native validation. Hostname, validity, purpose, signatures and the
existing trust-root selection still apply. `SSL_CERT_FILE` / `SSL_CERT_DIR`
remain the custom-root configuration; AIA downloads never become roots.

Material retrieval uses the selected native transport and its effective proxy
and address permissions, without page cookies, authorization or request body.
Each redirect is checked again. Retrieval is limited to three issuer rounds,
three redirects per fetch, 64 KiB per decoded response, five seconds per fetch
and a shared 15-second material-recovery deadline. The per-client URL cache has
64 entries and caches failures for five minutes. AIA HTTPS downloads themselves
do not recursively trigger recovery.

On Windows the native verifier's implicit AIA issuer retrieval is disabled so
issuer downloads cannot bypass this transport. Platform roots and the existing
native revocation/root-update policy are otherwise preserved; this is not an
offline-revocation mode. Material deadlines cancel their network work. During
recovery, native cumulative URL-retrieval timeouts are capped to the remaining
budget and late success is rejected; the initial verifier keeps its normal
timeout and all native revocation checks.

## Custom User-Agent

```bash
obscura fetch https://example.com --user-agent "Mozilla/5.0 (...) ..."
obscura serve --user-agent "Mozilla/5.0 (...) ..."
```

Default UA matches a recent Chrome on the build platform.

## Browser profile, timezone, and geolocation

The engine presents one of a built-in pool of realistic browser profiles (a mix of Windows and macOS, recent Chrome versions). Each profile keeps `navigator.platform`, `navigator.userAgentData` (platform and platform version), and the UA string internally consistent, so the surfaces a site fingerprints agree with each other. There is no GPU renderer among them: `canvas.getContext('webgl')` returns `null`, so a page cannot read a renderer string at all.

A single stable profile is used by default. One IP cycling through different identities is itself a signal, so rotation is opt-in:

```bash
OBSCURA_PROFILE=2 obscura serve          # pin a specific profile by index
OBSCURA_ROTATE_PROFILE=1 obscura serve   # random profile per browser context
```

## Regional identity, location permission and RTC routing

`--regional-config FILE` (or `OBSCURA_REGIONAL_CONFIG` JSON) selects identity
before the first request and isolate. `--geoip off|auto|IP` overrides the mode;
`--regional-info` prints resolved fields and provenance without opening a page.
Rust builders expose `.regional(obscura::regional::RegionalConfig { ... })`.
MCP uses the same startup configuration and exposes `browser_regional_profile`.

```json
{
  "geoip":"off", "locale":"ja-JP", "timezone":"Asia/Tokyo",
  "languages":["ja-JP","ja"], "seed":42,
  "position":{"latitude":35.6762,"longitude":139.6503,"accuracy":2500}
}
```

- `off` never probes egress or opens regional data; explicit fields still apply.
- A literal IPv4/IPv6 address is looked up locally, without discovery. It selects
  regional identity, **not ownership of that transport IP**.
- `auto` requires an explicit `discovery_url` returning one literal IP. A bounded,
  cookie-free request uses the selected ordinary/stealth context route, including
  authenticated proxy, TLS, redirect and network policy. No public provider or
  direct retry is implicit. Provenance identifies only the observed address
  family/time, not all future rotating exits. A genuinely new context probes
  again; a new connection to the same context does not.

GeoIP requires local `database` (city MMDB), `territory_data` (official CLDR
`supplementalData.xml`), and corresponding `database_manifest` /
`territory_manifest` paths. No data is bundled or silently downloaded. Each
manifest has this schema:

```json
{
  "supplier":"Licensed supplier", "version":"Exact release/date",
  "license":"Applicable licence", "attribution":"Required attribution",
  "source":"Primary source URL", "sha256":"64 hex digits for the file",
  "update_obligations":"Actual supplier requirements, or none",
  "usable_until":null
}
```

`usable_until` is a Unix-seconds cutoff only when the licence/update policy
requires one; nonexpiring licences use `null`. Preserve supplier attribution and
licences. Publish updated local data/manifests atomically and validate with
`--regional-info`. Invalid hashes/data, absent records, expired declared use,
missing timezone/coordinates or unknown accuracy fail instead of inventing a
location. Output includes the MMDB's actual build epoch. Official test databases
exercise parsing/configuration, **not real geographic accuracy**.

Country selects CLDR positive territory-population weights (overlapping weights
need not sum to 100). A versioned, unbiased seed-stable cumulative sampler keeps
one selection per context. Zero weight is excluded; parsed data is content-hash
cached, not reloaded per page. MMDB accuracy radius is converted from kilometers
to meters; city centroids are never represented as meter-level positions.
Explicit `locale`, `timezone`, `languages`, `position` override their own fields;
an explicit language list also supplies locale when locale is absent.

`Date`, `Intl.DateTimeFormat` and `Intl.NumberFormat` use **native ICU**, including
frames, popups and workers. ICU defaults are process-global: incompatible later
locale/timezone configuration returns an error instead of changing live realms.
Use a separate process for a different native locale/timezone. Nonblank
`OBSCURA_TIMEZONE` is trimmed on both Windows and Unix.

Coordinates do **not** grant geolocation permission. Secure tuple origins begin
at prompt; opaque, insecure or Permissions-Policy/iframe-delegation-blocked
documents cannot inherit authorization. Native per-context/origin decisions
drive asynchronous `getCurrentPosition`, `PermissionStatus.change` and watches.
CDP `Browser.grantPermissions`/`resetPermissions` take `browserContextId` and an
optional origin. MCP exposes `browser_geolocation_permission`,
`browser_reset_geolocation_permissions` and `browser_geolocation_position`.
Position override and permission grant are separate; reset revokes access,
`clearWatch` stops callbacks, and retired documents cannot continue delivery.
Camera/microphone capture is not added or faked.

RTC uses native ICE/DTLS/SCTP, STUN/TURN, text/binary data channels and statistics;
GeoIP never rewrites candidate/related addresses or stats. Proxied connections
use relay policy. SOCKS5 supports authenticated UDP association; HTTP proxies
require TURN/TCP (`?transport=tcp` or `turns:`) over CONNECT. Secure STUN/TURN
validates original server identity and trust roots; unsupported routing fails
closed with a visible error and no direct socket retry.

Proxied hostname endpoints require `OBSCURA_RTC_DOH_URL`, an operator-selected
RFC 8484 resolver with a **literal bootstrap IP** and valid TLS IP identity
(explicitly authorized loopback HTTP is available for fixtures). A fresh
cookie-free client uses the selected route/policy; no ambient destination or
resolver DNS, public-provider default or direct fallback is used. Direct mode
may use normal DNS. Every resolved and packet destination remains subject to
private-address/CIDR policy. A dead/bad-auth proxy or invalid TLS is not consent
to bypass that route. See [Environment variables](Environment-variables.md).

## Combine

```bash
obscura serve \
  --stealth \
  --proxy http://user:pass@proxy.example.com:8080 \
  --user-agent "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 ..."
```
