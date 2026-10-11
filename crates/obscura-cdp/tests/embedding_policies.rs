//! Embedding checks use the protected response and every native ancestor origin.

use obscura_cdp::dispatch::{dispatch, CdpContext};
use obscura_cdp::types::CdpRequest;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

struct Case {
    name: &'static str,
    headers: Vec<u8>,
    expected_csp: Option<&'static str>,
    allowed: bool,
    nested: bool,
    opaque: bool,
    sandbox: bool,
    cross_origin_middle: bool,
    redirect: bool,
    script: &'static str,
}

impl Case {
    fn policy(name: &'static str, source: impl AsRef<str>, allowed: bool) -> Self {
        Self::headers(name, format!("Content-Security-Policy: frame-ancestors {}\r\n", source.as_ref()), allowed)
    }

    fn headers(name: &'static str, headers: impl Into<String>, allowed: bool) -> Self {
        Self { name, headers: headers.into().into_bytes(), expected_csp: None, allowed, nested: false, opaque: false,
            sandbox: false, cross_origin_middle: false, redirect: false, script: "" }
    }

    fn redirect(mut self) -> Self {
        self.redirect = true;
        self
    }
}

async fn cdp(ctx: &mut CdpContext, method: &str, params: Value, session: Option<&str>) -> Value {
    let response = dispatch(&CdpRequest {
        id: 1, method: method.into(), params, session_id: session.map(str::to_owned),
    }, ctx).await;
    assert!(response.error.is_none(), "CDP {method}: {:?}", response.error);
    response.result.unwrap_or_else(|| json!({}))
}

async fn attached_session(ctx: &mut CdpContext) -> String {
    let created = cdp(ctx, "Target.createTarget", json!({"url":"about:blank"}), None).await;
    let attached = cdp(ctx, "Target.attachToTarget",
        json!({"targetId":created["targetId"],"flatten":true}), None).await;
    attached["sessionId"].as_str().unwrap().to_owned()
}

async fn run_cases(make_cases: impl FnOnce(&str, u16) -> Vec<Case>) {
    std::env::set_var("OBSCURA_ALLOW_PRIVATE_NETWORK", "1");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let origin = format!("http://127.0.0.1:{port}");
    let cases = std::sync::Arc::new(make_cases(&origin, port));
    let served = cases.clone();
    let (request_tx, mut request_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let mut requests = Vec::new();
    let server = tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let cases = served.clone();
            let request_tx = request_tx.clone();
            tokio::spawn(async move {
                let mut bytes = [0u8; 4096];
                let length = socket.read(&mut bytes).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&bytes[..length]);
                let path = request.split_ascii_whitespace().nth(1).unwrap_or("");
                let _ = request_tx.send(path.to_owned());
                let index = path.split('/').nth(2).and_then(|value| value.parse::<usize>().ok());
                let Some(case) = index.and_then(|index| cases.get(index).map(|case| (index, case))) else {
                    let _ = socket.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                    return;
                };
                let (index, case) = case;
                if path.ends_with("/redirect.html") {
                    let response = format!("HTTP/1.1 302 Found\r\nLocation: /app/{index}/protected.html\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                    let _ = socket.write_all(response.as_bytes()).await;
                    return;
                }
                let (headers, body) = if path.ends_with("/protected.html") {
                    (case.headers.as_slice(), "<html><body><p id=marker>protected</p><script>globalThis.protectedExecuted=true;</script></body></html>".to_owned())
                } else if path.ends_with("/middle.html") {
                    (&b""[..], format!("<html><body><iframe src=\"/app/{index}/protected.html\"></iframe></body></html>"))
                } else {
                    let source = if case.nested { "middle.html" } else if case.redirect { "redirect.html" } else { "protected.html" };
                    let sandbox = if case.sandbox { " sandbox=\"allow-scripts\"" } else { "" };
                    let source = if case.cross_origin_middle {
                        format!("http://localhost:{port}/app/{index}/{source}")
                    } else {
                        format!("/app/{index}/{source}")
                    };
                    (&b""[..], format!("<html><body><script>{}</script><iframe{sandbox} src=\"{source}\"></iframe></body></html>", case.script))
                };
                let mut response = b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n".to_vec();
                response.extend_from_slice(headers);
                response.extend_from_slice(format!("Content-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes());
                let _ = socket.write_all(&response).await;
            });
        }
    });

    let mut ctx = CdpContext::new();
    let session = attached_session(&mut ctx).await;
    cdp(&mut ctx, "Network.enable", json!({}), Some(&session)).await;
    cdp(&mut ctx, "Runtime.enable", json!({}), Some(&session)).await;
    for (index, case) in cases.iter().enumerate() {
        ctx.pending_events.clear();
        let protected_origin = if case.cross_origin_middle {
            format!("http://localhost:{port}")
        } else {
            origin.clone()
        };
        let protected_url = format!("{protected_origin}/app/{index}/protected.html");
        let parent_url = if case.opaque {
            format!("data:text/html,<html><body><iframe src='{protected_url}'></iframe></body></html>")
        } else {
            format!("{origin}/app/{index}/parent.html")
        };
        cdp(&mut ctx, "Page.navigate", json!({
            "url":parent_url, "waitUntil":"load",
        }), Some(&session)).await;
        while let Ok(path) = request_rx.try_recv() { requests.push(path); }
        if case.redirect {
            assert!(requests.iter().any(|path| path == &format!("/app/{index}/redirect.html")),
                "{}: redirect source request was not served", case.name);
            assert!(requests.iter().any(|path| path == &format!("/app/{index}/protected.html")),
                "{}: redirect final request was not served", case.name);
        }
        let response = ctx.pending_events.iter().find(|event|
            event.method == "Network.responseReceived"
                && event.params["response"]["url"] == protected_url
                && event.params["response"]["status"] == 200)
            .unwrap_or_else(|| panic!("{}: actual protected response was not observed; requests={requests:?}", case.name));
        if let Some(expected_csp) = case.expected_csp {
            assert_eq!(response.params["response"]["headers"]["content-security-policy"], expected_csp,
                "{}: actual HTTP header bytes must survive as a Fetch ByteString", case.name);
        }
        let request_id = response.params["requestId"].as_str().unwrap().to_owned();
        let body = cdp(&mut ctx, "Network.getResponseBody", json!({"requestId":request_id}), Some(&session)).await;
        assert!(body["body"].as_str().unwrap().contains("globalThis.protectedExecuted=true"),
            "{}: response provenance must include the protected script", case.name);
        let tree = cdp(&mut ctx, "Page.getFrameTree", json!({}), Some(&session)).await;
        let owner = if case.nested { &tree["frameTree"]["childFrames"][0] } else { &tree["frameTree"] };
        let children = owner["childFrames"].as_array().unwrap();
        if !case.allowed {
            assert!(children.is_empty(), "{}: denied response created a live child: {tree}", case.name);
            let defaults = ctx.pending_events.iter().filter(|event|
                event.method == "Runtime.executionContextCreated"
                    && event.session_id.as_deref() == Some(session.as_str())
                    && event.params["context"]["auxData"]["isDefault"] == true).count();
            assert_eq!(defaults, if case.nested { 2 } else { 1 },
                "{}: denied response published an executable context", case.name);
            continue;
        }
        assert_eq!(children.len(), 1, "{}: valid policy omitted the child: {tree}", case.name);
        let child = &children[0]["frame"];
        assert_eq!(child["url"], protected_url, "{}: protected document URL", case.name);
        let context = ctx.pending_events.iter().find(|event|
            event.method == "Runtime.executionContextCreated"
                && event.session_id.as_deref() == Some(session.as_str())
                && event.params["context"]["auxData"]["frameId"] == child["id"]
                && event.params["context"]["auxData"]["isDefault"] == true)
            .unwrap_or_else(|| panic!("{}: no native child default context", case.name))
            .params["context"]["id"].clone();
        let evaluated = cdp(&mut ctx, "Runtime.evaluate", json!({
            "expression":"[globalThis.protectedExecuted,document.getElementById('marker').textContent,location.href]",
            "contextId":context,"returnByValue":true,
        }), Some(&session)).await;
        assert!(evaluated.get("exceptionDetails").is_none(), "{}: {evaluated}", case.name);
        assert_eq!(evaluated["result"]["value"], json!([true,"protected",protected_url]),
            "{}: the allowed response must execute in its real native document", case.name);
        if case.sandbox {
            let boundary = cdp(&mut ctx, "Runtime.evaluate", json!({
                "expression":"(() => { try { parent.document; return false; } catch (error) { return error.name === 'SecurityError'; } })()",
                "contextId":context,"returnByValue":true,
            }), Some(&session)).await;
            assert_eq!(boundary["result"]["value"], true,
                "{}: loading the sandboxed document must not grant parent DOM access", case.name);
        }
    }
    server.abort();
}

#[tokio::test(flavor = "current_thread")]
async fn scheme_schemeless_and_wildcard_ports_load_real_children() {
    run_cases(|origin, port| vec![
        Case::policy("scheme", "http:", true),
        Case::policy("case insensitive scheme", "HTTP:", true),
        Case::policy("wildcard port", "http://127.0.0.1:*", true),
        Case::policy("explicit port", origin, true),
        Case::policy("schemeless host", format!("127.0.0.1:{port}"), true),
        Case::policy("wildcard host and port", "http://*:*", true),
        Case::policy("wildcard source", "*", true),
        Case::policy("wrong scheme", "https:", false),
        Case::policy("wrong port", format!("http://127.0.0.1:{}", if port == 65535 { 65534 } else { port + 1 }), false),
        Case::policy("default port does not match random listener", "http://127.0.0.1", false),
        Case::policy("wrong origin", format!("http://localhost:{port}"), false),
    ]).await;
}

#[tokio::test(flavor = "current_thread")]
async fn malformed_source_grammar_never_grants_embedding() {
    run_cases(|origin, port| vec![
        Case::policy("none", "'none'", false),
        Case::policy("empty source list", "", false),
        Case::policy("userinfo", format!("http://user@127.0.0.1:{port}"), false),
        Case::policy("empty userinfo", format!("http://@127.0.0.1:{port}"), false),
        Case::policy("query", format!("{origin}/?allow"), false),
        Case::policy("empty query", format!("{origin}/?"), false),
        Case::policy("fragment", format!("{origin}/#allow"), false),
        Case::policy("empty fragment", format!("{origin}/#"), false),
        Case::policy("backslash URL normalization", format!("http:\\\\127.0.0.1:{port}"), false),
        Case::policy("encoded hostname", format!("http://%31%32%37.0.0.1:{port}"), false),
        Case::policy("short IPv4 URL normalization", format!("http://127.1:{port}"), false),
        Case::policy("missing scheme separator", format!("http:/127.0.0.1:{port}"), false),
        Case::policy("empty port", "http://127.0.0.1:", false),
        Case::policy("out of range port", "http://127.0.0.1:65536", false),
        Case::policy("signed port", format!("http://127.0.0.1:+{port}"), false),
        Case::policy("partial wildcard", format!("http://*127.0.0.1:{port}"), false),
        Case::policy("empty hostname label", format!("http://127..0.0.1:{port}"), false),
        Case::policy("invalid path escape", format!("{origin}/%"), false),
        Case::policy("invalid path double leading slash", format!("{origin}//"), false),
    ]).await;
}

#[tokio::test(flavor = "current_thread")]
async fn obs_text_frame_ancestors_is_preserved_and_enforced() {
    run_cases(|_, _| vec![Case {
        // Send one Latin-1 byte verbatim, not UTF-8 or a percent-encoded token.
        headers: b"Content-Security-Policy: frame-ancestors \xE9\r\n".to_vec(),
        expected_csp: Some("frame-ancestors \u{00e9}"),
        ..Case::policy("obs-text source must not erase the enforced directive", "", false)
    }]).await;
}

#[tokio::test(flavor = "current_thread")]
async fn paths_match_serialized_ancestor_origin_not_document_path() {
    // CSP3 frame-ancestors checks URL(serialized ancestor ORIGIN), redirect count 0.
    // https://w3c.github.io/webappsec-csp/#frame-ancestors-navigation-response
    run_cases(|origin, _| vec![
        Case::policy("root path", format!("{origin}/"), true),
        Case::policy("matching ancestor document directory is not authority", format!("{origin}/app/"), false),
        Case::policy("matching ancestor document path is not authority", format!("{origin}/app/2/parent.html"), false),
        Case::policy("dot path is not URL normalized", format!("{origin}/./"), false),
        Case::policy("encoded dot path is not URL normalized", format!("{origin}/%2e/"), false),
        Case::policy("encoded slash is a segment not a separator", format!("{origin}/%2f"), false),
        Case::policy("redirect retains root path matching", format!("{origin}/"), true).redirect(),
        Case::policy("redirect does not waive nonroot path", format!("{origin}/app/"), false).redirect(),
    ]).await;
}

#[tokio::test(flavor = "current_thread")]
async fn policy_conjunction_first_directive_and_xfo_precedence() {
    run_cases(|_, _| vec![
        Case::headers("CSP supersedes XFO", "Content-Security-Policy: frame-ancestors http:\r\nX-Frame-Options: DENY\r\n", true),
        Case::headers("XFO deny", "X-Frame-Options: DENY\r\n", false),
        Case::headers("XFO sameorigin", "X-Frame-Options: SAMEORIGIN\r\n", true),
        Case::headers("conflicting XFO fields", "X-Frame-Options: SAMEORIGIN\r\nX-Frame-Options: DENY\r\n", false),
        Case::headers("unrelated CSP has no fallback", "Content-Security-Policy: default-src 'none'\r\n", true),
        Case::headers("unrelated CSP preserves XFO", "Content-Security-Policy: default-src 'none'\r\nX-Frame-Options: DENY\r\n", false),
        Case::headers("report only does not supersede XFO", "Content-Security-Policy-Report-Only: frame-ancestors http:\r\nX-Frame-Options: DENY\r\n", false),
        Case::headers("duplicate enforced fields conjunct", "Content-Security-Policy: frame-ancestors http:\r\nContent-Security-Policy: frame-ancestors 'none'\r\n", false),
        Case::headers("duplicate enforced fields reverse order", "Content-Security-Policy: frame-ancestors 'none'\r\nContent-Security-Policy: frame-ancestors http:\r\n", false),
        Case::headers("combined enforced policies conjunct", "Content-Security-Policy: frame-ancestors http:, frame-ancestors 'none'\r\n", false),
        Case::headers("combined enforced policies both allow", "Content-Security-Policy: frame-ancestors http:, frame-ancestors 'self'\r\n", true),
        Case::headers("empty first directive remains enforced", "Content-Security-Policy: frame-ancestors; frame-ancestors http:\r\n", false),
        Case::headers("first directive allows", "Content-Security-Policy: frame-ancestors http:; frame-ancestors 'none'\r\n", true),
        Case::headers("first directive denies", "Content-Security-Policy: frame-ancestors 'none'; frame-ancestors http:\r\n", false),
        Case::policy("sources OR", "https://wrong.example 'self'", true),
        Case::policy("invalid token does not poison valid source", "http://user@127.0.0.1:* http:", true),
        Case::policy("none ignored beside valid source", "'none' http:", true),
    ]).await;
}

#[tokio::test(flavor = "current_thread")]
async fn opaque_native_ancestor_remains_denied_and_all_ancestors_are_checked() {
    run_cases(|_, _| {
        let mut allowed = Case::policy("all nonopaque ancestors", "http:", true);
        allowed.nested = true;
        let mut cross_origin = Case::policy("all distinct tuple ancestors", "http:", true);
        cross_origin.nested = true;
        cross_origin.cross_origin_middle = true;
        let mut only_nearest = Case::policy("nearest ancestor match cannot skip top", "http://localhost:*", false);
        only_nearest.nested = true;
        only_nearest.cross_origin_middle = true;
        let mut opaque = Case::policy("opaque top ancestor", "http:", false);
        opaque.opaque = true;
        let mut opaque_star = Case::policy("star cannot authorize opaque top ancestor", "*", false);
        opaque_star.opaque = true;
        let mut sandbox = Case::policy("CSP permits loading without removing the sandbox origin fence", "http:", true);
        sandbox.sandbox = true;
        vec![allowed, cross_origin, only_nearest, opaque, opaque_star, sandbox]
    }).await;
}

#[tokio::test(flavor = "current_thread")]
async fn author_url_json_and_prototype_tampering_cannot_replace_response_policy() {
    run_cases(|_, _| {
        let mut url = Case::policy("replace URL constructor", "https://wrong.example", false);
        url.script = "const NativeURL=URL;globalThis.URL=class extends NativeURL {get origin(){return 'http://allowed.example';}};";
        let mut prototype = Case::policy("replace source predicates", "'none'", false);
        prototype.script = "Array.prototype.some=function(){return true;};Array.prototype.every=function(){return true;};";
        let mut json = Case::policy("replace response JSON parse", "'none'", false);
        json.script = "const parse=JSON.parse;JSON.parse=function(s){const v=parse(s);if(v&&v.headers){v.headers={};}return v;};";
        let mut provenance = Case::policy("allowed response URL and body remain native", "http:", true);
        provenance.script = "const parse=JSON.parse;JSON.parse=function(s){const v=parse(s);if(v&&v.headers){v.url='http://wrong.example/';v.body='<p>fake</p>';}return v;};";
        let mut receiver = Case::policy("policy receiver is the captured lexical global", "http:", true);
        receiver.script = "const nativeGlobal=globalThis;Object.defineProperty(nativeGlobal,'globalThis',{configurable:true,get(){return String(new Error().stack).includes('_iframeEmbeddingAllowed')?{}:nativeGlobal;}});";
        vec![url, prototype, json, provenance, receiver]
    }).await;
}
