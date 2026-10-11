//! Response-header frame-ancestors matching, not a general CSP implementation.
//!
//! CSP3's navigation response check parses each serialized ancestor ORIGIN as a
//! URL and matches it with redirect count zero. A response redirect therefore
//! does not discard a source path, and an ancestor document's path is not used.
//! Scheme/port upgrade and literal loopback host behavior follow Chromium's
//! native CSPSource matcher, rather than claiming full draft CSP conformance.
//! https://github.com/chromium/chromium/blob/main/services/network/public/cpp/content_security_policy/csp_source.cc
//! https://w3c.github.io/webappsec-csp/#frame-ancestors-navigation-response
//! https://w3c.github.io/webappsec-csp/#match-url-to-source-expression

use std::borrow::Cow;
use std::fmt;
use std::marker::PhantomData;

use serde::de::{IgnoredAny, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use url::Url;

#[derive(Deserialize)]
struct HeaderText<'a>(#[serde(borrow)] Cow<'a, str>);

#[derive(Default)]
struct PolicyHeaders<'a> {
    csp: Vec<Cow<'a, str>>,
    xfo: Vec<Cow<'a, str>>,
}

impl<'de: 'a, 'a> Deserialize<'de> for PolicyHeaders<'a> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct HeadersVisitor<'a>(PhantomData<&'a ()>);
        impl<'de: 'a, 'a> Visitor<'de> for HeadersVisitor<'a> {
            type Value = PolicyHeaders<'a>;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("native response headers")
            }

            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut headers = PolicyHeaders::default();
                while let Some(HeaderText(name)) = map.next_key::<HeaderText<'de>>()? {
                    if name.eq_ignore_ascii_case("content-security-policy") {
                        headers.csp.push(map.next_value::<HeaderText<'de>>()?.0);
                    } else if name.eq_ignore_ascii_case("x-frame-options") {
                        headers.xfo.push(map.next_value::<HeaderText<'de>>()?.0);
                    } else {
                        map.next_value::<IgnoredAny>()?;
                    }
                }
                Ok(headers)
            }
        }
        deserializer.deserialize_map(HeadersVisitor(PhantomData))
    }
}

#[derive(Deserialize)]
struct PolicyResponse<'a> {
    #[serde(borrow)]
    url: Cow<'a, str>,
    #[serde(borrow)]
    headers: PolicyHeaders<'a>,
    status: u16,
    #[serde(default)]
    blocked: bool,
}

pub(crate) struct FrameEmbeddingPolicy<'a> {
    protected: Url,
    headers: PolicyHeaders<'a>,
    has_frame_ancestors: bool,
}

impl<'a> FrameEmbeddingPolicy<'a> {
    /// Parse the original private fetch result, before any author JSON/URL APIs.
    /// Unrelated headers and the response body are skipped, not copied.
    pub(crate) fn from_response(raw: &'a str) -> Option<Self> {
        let response: PolicyResponse<'a> = serde_json::from_str(raw).ok()?;
        if response.blocked || response.status == 0 || response.status >= 400 {
            return None;
        }
        let protected = Url::parse(&response.url).ok()?;
        let has_frame_ancestors = response.headers.csp.iter()
            .flat_map(|value| value.split(','))
            .any(|policy| frame_ancestors(policy).is_some());
        Some(Self { protected, headers: response.headers, has_frame_ancestors })
    }

    pub(crate) fn allows_ancestor(&self, origin: &str) -> bool {
        if !self.has_frame_ancestors && self.headers.xfo.is_empty() {
            return true;
        }
        // Native opaque origins serialize as "null" and cannot authorize framing.
        let Ok(ancestor) = Url::parse(origin) else { return false; };
        if !matches!(ancestor.origin(), url::Origin::Tuple(..)) || ancestor.host_str().is_none() {
            return false;
        }
        if self.has_frame_ancestors {
            return self.headers.csp.iter().flat_map(|value| value.split(','))
                .filter_map(frame_ancestors)
                .all(|sources| sources.split(is_csp_whitespace)
                    .filter(|source| !source.is_empty())
                    .any(|source| source_matches(source, &ancestor, &self.protected)));
        }
        // Preserve XFO's existing fail-closed behavior, only in the absence of
        // an enforced frame-ancestors directive. Report-only does not override it.
        self.headers.xfo.iter().flat_map(|value| value.split(','))
            .all(|value| value.trim_matches(is_csp_whitespace).eq_ignore_ascii_case("SAMEORIGIN"))
            && ancestor.origin() == self.protected.origin()
    }
}

fn is_csp_whitespace(value: char) -> bool {
    matches!(value, '\t' | '\n' | '\u{000c}' | '\r' | ' ')
}

fn frame_ancestors(policy: &str) -> Option<&str> {
    for directive in policy.split(';') {
        let directive = directive.trim_matches(is_csp_whitespace);
        let end = directive.find(is_csp_whitespace).unwrap_or(directive.len());
        if directive[..end].eq_ignore_ascii_case("frame-ancestors") {
            // Invalid source tokens must not erase the recognized directive,
            // including non-ASCII tokens or a first directive with no sources.
            // Only the first directive with this name applies within a policy.
            return Some(directive[end..].trim_matches(is_csp_whitespace));
        }
    }
    None
}

fn valid_scheme(scheme: &str) -> bool {
    let mut bytes = scheme.bytes();
    bytes.next().is_some_and(|byte| byte.is_ascii_alphabetic())
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.'))
}

fn scheme_matches(source: &str, target: &str) -> bool {
    source.eq_ignore_ascii_case(target)
        || (source.eq_ignore_ascii_case("http") && target == "https")
        || (source.eq_ignore_ascii_case("ws") && target == "wss")
}

fn self_matches(ancestor: &Url, protected: &Url) -> bool {
    if ancestor.origin() == protected.origin() { return true; }
    ancestor.host() == protected.host()
        && ancestor.port() == protected.port()
        && (matches!(ancestor.scheme(), "https" | "wss")
            || (protected.scheme() == "http" && matches!(ancestor.scheme(), "http" | "ws")))
}

fn valid_host(host: &str) -> bool {
    if host == "*" { return true; }
    let host = host.strip_prefix("*.").unwrap_or(host);
    let host = host.strip_suffix('.').unwrap_or(host);
    !host.is_empty() && host.split('.').all(|label|
        !label.is_empty() && label.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'))
}

fn host_matches(pattern: &str, ancestor: &Url) -> bool {
    let Some(host) = ancestor.host_str() else { return false; };
    if pattern == "*" { return true; }
    if let Some(suffix) = pattern.strip_prefix('*') {
        return host.len() > suffix.len()
            && host.get(host.len() - suffix.len()..).is_some_and(|host| host.eq_ignore_ascii_case(suffix));
    }
    // Compare CSP's literal ASCII host with the already normalized native URL.
    // URL-only aliases such as 127.1, percent-encoded hosts and userinfo are not
    // CSP host grammar and must not be normalized into an authorization.
    pattern.eq_ignore_ascii_case(host)
}

fn valid_path(path: &str) -> bool {
    if path.is_empty() { return true; }
    // RFC3986 path-absolute: "/" [ segment-nz *( "/" segment ) ].
    if !path.starts_with('/') || path.starts_with("//") { return false; }
    let mut bytes = path.bytes();
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            if !bytes.next().is_some_and(|byte| byte.is_ascii_hexdigit())
                || !bytes.next().is_some_and(|byte| byte.is_ascii_hexdigit()) {
                return false;
            }
        } else if !byte.is_ascii_alphanumeric()
            && !matches!(byte, b'-' | b'.' | b'_' | b'~' | b'!' | b'$' | b'&' | b'\''
                | b'(' | b')' | b'*' | b'+' | b'=' | b':' | b'@' | b'/') {
            // CSP excludes ';' and ',' from path-absolute, and has no query,
            // fragment, backslash or userinfo extension to host-source grammar.
            return false;
        }
    }
    true
}

#[derive(Clone, Copy)]
enum SourcePort {
    Default,
    Any,
    Explicit(u16),
}

fn port_matches(port: SourcePort, scheme: &str, ancestor: &Url) -> bool {
    let upgrade = !scheme.eq_ignore_ascii_case(ancestor.scheme());
    match port {
        SourcePort::Any => true,
        SourcePort::Default => ancestor.port().is_none(),
        SourcePort::Explicit(port) => {
            let target = ancestor.port_or_known_default();
            // A host-source's insecure scheme and port upgrade together. A
            // scheme-only source has no port restriction. Retain browser CSP
            // behavior: http://host:80 can authorize https://host:443, not
            // https://host:80 or an unrelated custom port.
            if upgrade {
                port == 80 && target == Some(443)
                    && matches!(ancestor.scheme(), "https" | "wss")
            } else {
                target == Some(port)
            }
        }
    }
}

fn source_matches(source: &str, ancestor: &Url, protected: &Url) -> bool {
    if source.eq_ignore_ascii_case("'self'") { return self_matches(ancestor, protected); }
    if source == "*" {
        return matches!(ancestor.scheme(), "http" | "https") || ancestor.scheme() == protected.scheme();
    }
    if let Some(scheme) = source.strip_suffix(':') {
        if valid_scheme(scheme) { return scheme_matches(scheme, ancestor.scheme()); }
    }
    let (scheme, host_source) = match source.split_once("://") {
        Some((scheme, rest)) if valid_scheme(scheme) => (scheme, rest),
        Some(_) => return false,
        None => (protected.scheme(), source),
    };
    let (authority, path) = match host_source.find('/') {
        Some(index) => (&host_source[..index], &host_source[index..]),
        None => (host_source, ""),
    };
    if !valid_path(path) { return false; }
    let (host, port) = match authority.split_once(':') {
        Some((host, "*")) => (host, SourcePort::Any),
        Some((host, port)) if !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit()) => {
            let Ok(port) = port.parse::<u16>() else { return false; };
            (host, SourcePort::Explicit(port))
        }
        Some(_) => return false,
        None => (authority, SourcePort::Default),
    };
    // The matched URL is a serialized tuple origin, with a root path. Under
    // CSP path-part matching (redirect count 0), ONLY an absent path or '/' can
    // match it. Do not URL-normalize dot segments or discard valid nonroot paths.
    valid_host(host) && (path.is_empty() || path == "/")
        && scheme_matches(scheme, ancestor.scheme())
        && host_matches(host, ancestor)
        && port_matches(port, scheme, ancestor)
}

#[cfg(test)]
mod tests {
    use super::FrameEmbeddingPolicy;

    #[test]
    fn nonascii_sources_do_not_erase_enforced_frame_ancestors() {
        for (sources, allowed) in [
            ("\u{00e9}", false),
            ("http:\u{00a0}", false),
            ("\u{00e9} http:", true),
            ("\u{00e9}; frame-ancestors http:", false),
        ] {
            let response = serde_json::json!({
                "url": "http://protected.example/", "status": 200,
                "headers": {"content-security-policy": format!("frame-ancestors {sources}")},
            }).to_string();
            let policy = FrameEmbeddingPolicy::from_response(&response).unwrap();
            assert_eq!(policy.allows_ancestor("http://127.0.0.1:9000"), allowed, "{sources}");
        }
    }

    #[test]
    fn malformed_native_response_never_creates_a_policy() {
        for response in [
            "{", "null", r#"{"url":"http://protected.example/","status":200}"#,
            r#"{"url":"not a URL","status":200,"headers":{}}"#,
            r#"{"url":"http://protected.example/","status":0,"headers":{}}"#,
            r#"{"url":"http://protected.example/","status":403,"headers":{}}"#,
            r#"{"url":"http://protected.example/","status":200,"blocked":true,"headers":{}}"#,
            r#"{"url":"http://protected.example/","status":200,"headers":{"content-security-policy":null}}"#,
        ] {
            assert!(FrameEmbeddingPolicy::from_response(response).is_none(), "{response}");
        }
    }

    #[test]
    fn chromium_scheme_port_upgrades_and_subdomain_hosts_match_native_origins() {
        for (source, ancestor, allowed) in [
            ("http:", "https://127.0.0.1:9000", true),
            ("http://127.0.0.1", "https://127.0.0.1", true),
            ("http://127.0.0.1:80", "https://127.0.0.1:443", true),
            ("http://127.0.0.1:80", "https://127.0.0.1:80", false),
            ("http://127.0.0.1:9000", "https://127.0.0.1:9000", false),
            ("http://127.0.0.1:*", "https://127.0.0.1:9000", true),
            ("127.0.0.1:*", "https://127.0.0.1:9000", true),
            ("https:", "http://127.0.0.1", false),
            ("ws:", "wss://127.0.0.1:9000", true),
            ("ws://127.0.0.1:80", "wss://127.0.0.1:443", true),
            ("http://*.example.test:*", "http://sub.example.test:9000", true),
            ("http://*.example.test:*", "http://deep.sub.example.test:9000", true),
            ("http://*.example.test:*", "http://example.test:9000", false),
            ("http://*.example.test:*", "http://badexample.test:9000", false),
            ("http://*.example.test:*", "http://sub.example.test.evil:9000", false),
        ] {
            let response = serde_json::json!({
                "url": "http://protected.example/", "status": 200,
                "headers": {"content-security-policy": format!("frame-ancestors {source}")},
            }).to_string();
            let policy = FrameEmbeddingPolicy::from_response(&response).unwrap();
            assert_eq!(policy.allows_ancestor(ancestor), allowed, "{source} against {ancestor}");
        }
    }
}
