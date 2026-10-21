use std::sync::Arc;
use tokio::sync::Mutex;

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use obscura_cdp::dispatch::{dispatch, CdpContext};
use obscura_cdp::types::CdpRequest;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct Fixture {
    base: String,
    requests: Arc<Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Fixture {
    fn drop(&mut self) { self.task.abort(); }
}

async fn fixture(redirect: Option<String>) -> Fixture {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let captured = requests.clone();
    let task = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let captured = captured.clone();
            let redirect = redirect.clone();
            tokio::spawn(async move {
                let mut bytes = Vec::new();
                while !bytes.windows(4).any(|chunk| chunk == b"\r\n\r\n") {
                    let mut buffer = [0; 2048];
                    let count = socket.read(&mut buffer).await.unwrap();
                    if count == 0 { return; }
                    bytes.extend_from_slice(&buffer[..count]);
                    assert!(bytes.len() < 16384);
                }
                let request = String::from_utf8(bytes).unwrap();
                let path = request.split_whitespace().nth(1).unwrap().to_string();
                captured.lock().await.push(request);
                let (status, content_type, body, extra) = match path.as_str() {
                    "/module.html" => (200, "text/html", b"<script type='module' src='/entry.js'></script>".to_vec(), "".to_string()),
                    "/entry.js" => (201, "text/javascript", b"import {value} from './dep.js'; globalThis.moduleValue = value;".to_vec(), "X-Receipt: entry\r\n".to_string()),
                    "/dep.js" => (200, "text/javascript", b"export const value = 42;".to_vec(), "X-Receipt: dependency\r\n".to_string()),
                    "/lazy.js" => (200, "text/javascript", b"import {value} from './lazy-dep.js'; export default value;".to_vec(), "".to_string()),
                    "/lazy-dep.js" => (200, "text/javascript", b"export const value = 73;".to_vec(), "".to_string()),
                    "/bad.js" => (404, "text/javascript", b"missing module".to_vec(), "".to_string()),
                    "/gbk" => (200, "text/plain; charset=gbk", vec![0xd6, 0xd0, 0xce, 0xc4], "".to_string()),
                    "/binary" => (200, "application/octet-stream", vec![0, 0xff, 0x80, 0x41], "".to_string()),
                    "/invalid-json" => (200, "application/json", vec![0xff], "".to_string()),
                    "/no-type" => (200, "", vec![0xe9], "".to_string()),
                    "/unknown-charset" => (200, "text/plain; charset=unknown", vec![0xe9], "".to_string()),
                    "/redirect" => (302, "text/plain", Vec::new(), format!("Location: {}echo\r\n", redirect.as_ref().unwrap())),
                    "/policy-redirect" => (302, "text/plain", Vec::new(), "Location: /echo\r\nReferrer-Policy: no-referrer\r\n".to_string()),
                    "/no-referrer.html" => (200, "text/html", b"<meta name='referrer' content='no-referrer'><body>policy</body>".to_vec(), "".to_string()),
                    "/header-policy.html" => (200, "text/html", b"<html><body>header policy</body></html>".to_vec(), "Referrer-Policy: no-referrer\r\n".to_string()),
                    _ => (200, "text/html", b"<html><body>ok</body></html>".to_vec(), "".to_string()),
                };
                let content_type = if content_type.is_empty() { String::new() } else { format!("Content-Type: {content_type}\r\n") };
                let headers = format!("HTTP/1.1 {status} OK\r\n{content_type}Content-Length: {}\r\nAccess-Control-Allow-Origin: *\r\n{extra}Connection: close\r\n\r\n", body.len());
                socket.write_all(headers.as_bytes()).await.unwrap();
                socket.write_all(&body).await.unwrap();
            });
        }
    });
    Fixture { base, requests, task }
}

async fn context(stealth: bool) -> (CdpContext, String) {
    let mut ctx = CdpContext::new();
    ctx.default_context = Arc::new(obscura_browser::BrowserContext::with_storage_and_network(
        "network-ports".into(), None, stealth, None, None, true,
    ).expect("network fixture context"));
    let page = ctx.create_page();
    let session = "network-ports-session".to_string();
    ctx.sessions.insert(session.clone(), page);
    cdp(&mut ctx, &session, "Network.enable", json!({})).await;
    (ctx, session)
}

async fn cdp(ctx: &mut CdpContext, session: &str, method: &str, params: Value) -> Value {
    let response = dispatch(&CdpRequest {
        id: 1, method: method.to_string(), params, session_id: (session != "browser").then(|| session.to_string()),
    }, ctx).await;
    assert!(response.error.is_none(), "{method}: {:?}", response.error);
    response.result.unwrap()
}

async fn evaluate(ctx: &mut CdpContext, session: &str, expression: &str) -> Value {
    let result = cdp(ctx, session, "Runtime.evaluate", json!({
        "expression": expression, "awaitPromise": true, "returnByValue": true,
    })).await;
    assert!(result.get("exceptionDetails").is_none(), "{result}");
    result["result"]["value"].clone()
}

fn header(request: &str, name: &str) -> Option<String> {
    request.lines().filter_map(|line| line.split_once(':'))
        .find(|(key, _)| key.eq_ignore_ascii_case(name)).map(|(_, value)| value.trim().to_string())
}

fn response(ctx: &CdpContext, suffix: &str) -> Value {
    ctx.pending_events.iter().find(|event| event.method == "Network.responseReceived"
        && event.params["response"]["url"].as_str().is_some_and(|url| url.ends_with(suffix)))
        .unwrap_or_else(|| panic!("missing response for {suffix}")).params.clone()
}

async fn module_receipts(stealth: bool) {
    let server = fixture(None).await;
    let (mut ctx, session) = context(stealth).await;
    cdp(&mut ctx, &session, "Page.navigate", json!({"url": format!("{}module.html", server.base)})).await;
    assert_eq!(evaluate(&mut ctx, &session, "moduleValue").await.as_f64(), Some(42.0));
    assert_eq!(evaluate(&mut ctx, &session, "import('/lazy.js').then(m => m.default)").await.as_f64(), Some(73.0));
    assert_eq!(evaluate(&mut ctx, &session, "import('/bad.js').then(() => false, () => true)").await, json!(true));
    for (path, status, body) in [
        ("/entry.js", 201, "import {value} from './dep.js'; globalThis.moduleValue = value;"),
        ("/dep.js", 200, "export const value = 42;"),
        ("/lazy.js", 200, "import {value} from './lazy-dep.js'; export default value;"),
        ("/lazy-dep.js", 200, "export const value = 73;"),
        ("/bad.js", 404, "missing module"),
    ] {
        let event = response(&ctx, path);
        assert_eq!(event["response"]["status"], status);
        let id = event["requestId"].as_str().unwrap();
        let retained = cdp(&mut ctx, &session, "Network.getResponseBody", json!({"requestId": id})).await;
        assert_eq!(retained, json!({"body": body, "base64Encoded": false}));
        let finished = ctx.pending_events.iter().find(|event| event.method == "Network.loadingFinished"
            && event.params["requestId"] == id).unwrap();
        assert_eq!(finished.params["encodedDataLength"], body.len());
    }
    assert_eq!(response(&ctx, "/entry.js")["response"]["headers"]["x-receipt"], "entry");
    ctx.pending_events.clear();
    evaluate(&mut ctx, &session, "new Promise(resolve => { const f = document.createElement('iframe'); f.onload = () => resolve(true); f.src = '/child'; document.body.append(f); })").await;
    assert_eq!(evaluate(&mut ctx, &session, "document.querySelector('iframe').contentWindow.eval(\"import('/lazy.js').then(m => m.default)\")").await.as_f64(), Some(73.0));
    let child = response(&ctx, "/lazy-dep.js");
    assert_eq!(child["frameId"], response(&ctx, "/child")["frameId"]);
    let root_frame = ctx.get_session_page(&Some(session.clone())).unwrap().frame_id.clone();
    assert_ne!(child["frameId"], root_frame);
    let retained = cdp(&mut ctx, &session, "Network.getResponseBody", json!({"requestId": child["requestId"]})).await;
    assert_eq!(retained["body"], "export const value = 73;");
    evaluate(&mut ctx, &session, "new Promise(resolve => { const f = document.createElement('iframe'); f.setAttribute('sandbox', 'allow-scripts'); f.onload = () => resolve(true); f.src = '/opaque'; document.body.append(f); })").await;
    let opaque = response(&ctx, "/opaque")["frameId"].clone();
    let world = cdp(&mut ctx, &session, "Page.createIsolatedWorld", json!({"frameId": opaque, "worldName": "opaque-module"})).await;
    let imported = cdp(&mut ctx, &session, "Runtime.evaluate", json!({
        "expression": "import('/dep.js').then(m => m.value)", "contextId": world["executionContextId"],
        "returnByValue": true, "awaitPromise": true,
    })).await;
    assert!(imported.get("exceptionDetails").is_none(), "{imported}");
    assert_eq!(imported["result"]["value"].as_f64(), Some(42.0));
    let request = server.requests.lock().await.last().unwrap().clone();
    assert_eq!(header(&request, "origin").as_deref(), Some("null"));
    assert_eq!(header(&request, "cookie"), None);
}

#[tokio::test(flavor = "current_thread")]
async fn module_receipts_cover_static_dynamic_failed_and_frame_graphs() { module_receipts(false).await; }
#[cfg(feature = "stealth")]
#[tokio::test(flavor = "current_thread")]
async fn stealth_module_receipts_cover_static_dynamic_failed_and_frame_graphs() { module_receipts(true).await; }

async fn body_policy(stealth: bool) {
    let server = fixture(None).await;
    let (mut ctx, session) = context(stealth).await;
    cdp(&mut ctx, &session, "Page.navigate", json!({"url": server.base})).await;
    for (path, bytes, expected) in [
        ("gbk", vec![0xd6, 0xd0, 0xce, 0xc4], json!({"body": "中文", "base64Encoded": false})),
        ("binary", vec![0, 0xff, 0x80, 0x41], json!({"body": "AP+AQQ==", "base64Encoded": true})),
        ("invalid-json", vec![0xff], json!({"body": "/w==", "base64Encoded": true})),
        ("no-type", vec![0xe9], json!({"body": "é", "base64Encoded": false})),
        ("unknown-charset", vec![0xe9], json!({"body": "6Q==", "base64Encoded": true})),
    ] {
        assert_eq!(evaluate(&mut ctx, &session, &format!("fetch('/{path}').then(r => r.arrayBuffer()).then(b => Array.from(new Uint8Array(b)))")).await, json!(bytes));
        let id = response(&ctx, &format!("/{path}"))["requestId"].clone();
        assert_eq!(cdp(&mut ctx, &session, "Network.getResponseBody", json!({"requestId": id})).await, expected);
    }
    let navigated = cdp(&mut ctx, &session, "Page.navigate", json!({"url": format!("{}gbk", server.base)})).await;
    assert_eq!(cdp(&mut ctx, &session, "Network.getResponseBody", json!({"requestId": navigated["loaderId"]})).await,
        json!({"body": "中文", "base64Encoded": false}));
    assert_eq!(BASE64.decode("AP+AQQ==").unwrap(), vec![0, 0xff, 0x80, 0x41]);
}

#[tokio::test(flavor = "current_thread")]
async fn plain_response_body_charset_and_bytes_are_lossless() { body_policy(false).await; }
#[cfg(feature = "stealth")]
#[tokio::test(flavor = "current_thread")]
async fn stealth_response_body_charset_and_bytes_are_lossless() { body_policy(true).await; }

async fn headers_and_referrer(stealth: bool) {
    let destination = fixture(None).await;
    let server = fixture(Some(destination.base.clone())).await;
    let (mut ctx, session) = context(stealth).await;
    cdp(&mut ctx, &session, "Network.setExtraHTTPHeaders", json!({"headers": {
        "X-Page": "private", "Authorization": "secret", "Cookie": "manual=secret", "Proxy-Authorization": "proxy-secret"
    }})).await;
    cdp(&mut ctx, &session, "Page.navigate", json!({"url": format!("{}source?q=secret#fragment", server.base)})).await;
    evaluate(&mut ctx, &session, "fetch('/echo', {headers: {'x-page': 'request'}}).then(r => r.text())").await;
    evaluate(&mut ctx, &session, "fetch('/redirect', {mode: 'no-cors'}).then(r => r.arrayBuffer())").await;
    evaluate(&mut ctx, &session, "fetch('/echo', {referrerPolicy: 'no-referrer'}).then(r => r.text())").await;
    evaluate(&mut ctx, &session, "fetch('/policy-redirect').then(r => r.text())").await;
    let requests = server.requests.lock().await.clone();
    assert_eq!(header(&requests[1], "x-page").as_deref(), Some("request"));
    assert_eq!(header(&requests[1], "referer"), Some(format!("{}source?q=secret", server.base)));
    assert_eq!(header(&requests[3], "referer"), None);
    assert_eq!(header(requests.last().unwrap(), "referer"), None);
    let crossed = destination.requests.lock().await.clone();
    assert_eq!(header(&crossed[0], "referer"), Some(server.base.clone()));
    for name in ["authorization", "proxy-authorization", "cookie"] { assert_eq!(header(&crossed[0], name), None); }
    evaluate(&mut ctx, &session, "fetch(new Request('/echo', {referrer: '', referrerPolicy: 'unsafe-url'}).clone()).then(r => r.text())").await;
    assert_eq!(header(server.requests.lock().await.last().unwrap(), "referer"), None);
    evaluate(&mut ctx, &session, "fetch('/echo', {referrer: '/custom#hidden', referrerPolicy: 'origin'}).then(r => r.text())").await;
    assert_eq!(header(server.requests.lock().await.last().unwrap(), "referer"), Some(server.base.clone()));
    assert_eq!(evaluate(&mut ctx, &session, "fetch('/echo', {referrerPolicy: 'invalid'}).then(() => false, e => e instanceof TypeError)").await, true);
    evaluate(&mut ctx, &session, "new Promise(resolve => { const f = document.createElement('iframe'); f.onload = () => resolve(true); f.src = '/child'; document.body.append(f); })").await;
    assert_eq!(header(server.requests.lock().await.last().unwrap(), "x-page").as_deref(), Some("private"));
    evaluate(&mut ctx, &session, "document.querySelector('iframe').contentWindow.fetch('/echo').then(r => r.text())").await;
    let child = server.requests.lock().await.last().unwrap().clone();
    assert_eq!(header(&child, "x-page").as_deref(), Some("private"));
    assert_eq!(header(&child, "referer"), Some(format!("{}child", server.base)));
    evaluate(&mut ctx, &session, "window.networkPopup = window.open('', 'network-header-popup'); true").await;
    let targets = cdp(&mut ctx, "browser", "Target.getTargets", json!({})).await;
    let opener = ctx.get_session_page(&Some(session.clone())).unwrap().id.clone();
    let popup = targets["targetInfos"].as_array().unwrap().iter().find(|info| info["openerId"] == opener).unwrap()["targetId"].clone();
    let attached = cdp(&mut ctx, "browser", "Target.attachToTarget", json!({"targetId": popup, "flatten": true})).await;
    let popup_session = attached["sessionId"].as_str().unwrap();
    cdp(&mut ctx, popup_session, "Network.enable", json!({})).await;
    evaluate(&mut ctx, popup_session, "fetch('/echo').then(r => r.text())").await;
    assert_eq!(header(server.requests.lock().await.last().unwrap(), "x-page").as_deref(), Some("private"));
    cdp(&mut ctx, popup_session, "Network.setExtraHTTPHeaders", json!({"headers": {"X-Page": "popup"}})).await;
    evaluate(&mut ctx, &session, "fetch('/echo').then(r => r.text())").await;
    assert_eq!(header(server.requests.lock().await.last().unwrap(), "x-page").as_deref(), Some("private"));
    evaluate(&mut ctx, popup_session, "fetch('/echo').then(r => r.text())").await;
    assert_eq!(evaluate(&mut ctx, popup_session, "import('/dep.js').then(m => m.value)").await.as_f64(), Some(42.0));
    let popup_frame = ctx.get_session_page(&Some(popup_session.to_string())).unwrap().frame_id.clone();
    let popup_response = ctx.pending_events.iter().rev().find(|event| event.method == "Network.responseReceived"
        && event.session_id.as_deref() == Some(popup_session)
        && event.params["response"]["url"].as_str().is_some_and(|url| url.ends_with("/dep.js"))).unwrap();
    assert_eq!(popup_response.params["frameId"], popup_frame);
    assert_eq!(header(server.requests.lock().await.last().unwrap(), "x-page").as_deref(), Some("popup"));
    let second = ctx.create_page();
    ctx.sessions.insert("second".into(), second);
    cdp(&mut ctx, "second", "Page.navigate", json!({"url": format!("{}second", server.base)})).await;
    assert_eq!(header(server.requests.lock().await.last().unwrap(), "x-page"), None);
    cdp(&mut ctx, &session, "Page.navigate", json!({"url": format!("{}redirect", server.base)})).await;
    let native_redirect = destination.requests.lock().await.last().unwrap().clone();
    assert_eq!(header(&native_redirect, "x-page").as_deref(), Some("private"));
    for name in ["authorization", "proxy-authorization", "cookie"] { assert_eq!(header(&native_redirect, name), None); }
    cdp(&mut ctx, &session, "Network.setExtraHTTPHeaders", json!({"headers": {}})).await;
    cdp(&mut ctx, &session, "Page.navigate", json!({"url": format!("{}no-referrer.html", server.base)})).await;
    evaluate(&mut ctx, &session, "new Promise(resolve => { const x = new XMLHttpRequest(); x.open('GET', '/echo'); x.onload = () => resolve(x.responseText); x.send(); })").await;
    assert_eq!(header(server.requests.lock().await.last().unwrap(), "referer"), None);
    cdp(&mut ctx, &session, "Page.navigate", json!({"url": format!("{}header-policy.html", server.base)})).await;
    evaluate(&mut ctx, &session, "fetch('/echo').then(r => r.text())").await;
    assert_eq!(header(server.requests.lock().await.last().unwrap(), "referer"), None);
    evaluate(&mut ctx, &session, "new Promise(resolve => { const f = document.createElement('iframe'); f.onload = () => resolve(true); f.src = '/header-policy.html'; document.body.append(f); })").await;
    evaluate(&mut ctx, &session, "document.querySelector('iframe').contentWindow.fetch('/echo').then(r => r.text())").await;
    assert_eq!(header(server.requests.lock().await.last().unwrap(), "referer"), None);
    evaluate(&mut ctx, &session, "document.cookie = 'ambient=secret; path=/'; fetch('/echo?explicit-token', {credentials: 'omit', headers: {'Authorization': 'Bearer explicit'}}).then(r => r.text())").await;
    let explicit = server.requests.lock().await.last().unwrap().clone();
    assert_eq!(header(&explicit, "authorization").as_deref(), Some("Bearer explicit"));
    assert_eq!(header(&explicit, "cookie"), None);
    cdp(&mut ctx, &session, "Network.setExtraHTTPHeaders", json!({"headers": {
        "User-Agent": "header-override-agent", "Accept-Language": "fr"
    }})).await;
    cdp(&mut ctx, &session, "Page.navigate", json!({"url": format!("{}overridden-identity", server.base)})).await;
    let native = server.requests.lock().await.last().unwrap().clone();
    assert_eq!(header(&native, "user-agent").as_deref(), Some("header-override-agent"));
    assert_eq!(header(&native, "accept-language").as_deref(), Some("fr"));
    for name in ["user-agent", "accept-language"] {
        assert_eq!(native.lines().filter(|line| line.split_once(':')
            .is_some_and(|(key, _)| key.eq_ignore_ascii_case(name))).count(), 1);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn plain_page_headers_and_referrer_obey_ownership_and_redirect_policy() { headers_and_referrer(false).await; }
#[cfg(feature = "stealth")]
#[tokio::test(flavor = "current_thread")]
async fn stealth_page_headers_and_referrer_obey_ownership_and_redirect_policy() { headers_and_referrer(true).await; }

async fn identity_override(stealth: bool) {
    let server = fixture(None).await;
    let (mut ctx, session) = context(stealth).await;
    for method in ["Network.setUserAgentOverride", "Emulation.setUserAgentOverride"] {
        cdp(&mut ctx, &session, "Page.navigate", json!({"url": server.base})).await;
        let root = ctx.get_session_page(&Some(session.clone())).unwrap().frame_id.clone();
        let world = cdp(&mut ctx, &session, "Page.createIsolatedWorld", json!({"frameId": root, "worldName": "identity-test"})).await;
        let locale = evaluate(&mut ctx, &session, "Intl.DateTimeFormat().resolvedOptions().locale").await;
        evaluate(&mut ctx, &session, "new Promise(resolve => { const f = document.createElement('iframe'); f.onload = () => resolve(true); f.src = '/child'; document.body.append(f); })").await;
        cdp(&mut ctx, &session, method, json!({"userAgent": "Override-UA/7", "acceptLanguage": "fr-CA,fr;q=0.9,en;q=0.5"})).await;
        let expected = json!(["Override-UA/7", "fr-CA", ["fr-CA", "fr", "en"]]);
        assert_eq!(evaluate(&mut ctx, &session, "[navigator.userAgent, navigator.language, navigator.languages]").await, expected);
        let isolated = cdp(&mut ctx, &session, "Runtime.evaluate", json!({"expression": "[navigator.userAgent, navigator.language, navigator.languages]", "contextId": world["executionContextId"], "returnByValue": true})).await;
        assert_eq!(isolated["result"]["value"], expected);
        assert_eq!(evaluate(&mut ctx, &session, "document.querySelector('iframe').contentWindow.eval('[navigator.userAgent, navigator.language, navigator.languages]')").await, expected);
        assert_eq!(evaluate(&mut ctx, &session, "Intl.DateTimeFormat().resolvedOptions().locale").await, locale);
        evaluate(&mut ctx, &session, "const newChild = document.createElement('iframe'); document.body.append(newChild); true").await;
        assert_eq!(evaluate(&mut ctx, &session, "document.querySelectorAll('iframe')[1].contentWindow.eval('[navigator.userAgent, navigator.language, navigator.languages]')").await, expected);
        assert_eq!(evaluate(&mut ctx, &session, "const p = window.open('', '_blank'); [p.navigator.userAgent, p.navigator.language, p.navigator.languages]").await, expected);
        evaluate(&mut ctx, &session, "fetch('/echo').then(r => r.text())").await;
        let requests = server.requests.lock().await.clone();
        assert_eq!(header(requests.last().unwrap(), "user-agent").as_deref(), Some("Override-UA/7"));
        assert_eq!(header(requests.last().unwrap(), "accept-language").as_deref(), Some("fr-CA,fr;q=0.9,en;q=0.5"));
        cdp(&mut ctx, &session, "Page.navigate", json!({"url": format!("{}next", server.base)})).await;
        assert_eq!(evaluate(&mut ctx, &session, "[navigator.userAgent, navigator.language, navigator.languages]").await, expected);
        cdp(&mut ctx, &session, method, json!({"userAgent": "", "acceptLanguage": ""})).await;
        assert_eq!(evaluate(&mut ctx, &session, "navigator.language").await, "en-US");
        evaluate(&mut ctx, &session, "fetch('/echo').then(r => r.text())").await;
        let reset_request = server.requests.lock().await.last().unwrap().clone();
        assert_eq!(evaluate(&mut ctx, &session, "navigator.userAgent").await, json!(header(&reset_request, "user-agent").unwrap()));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn both_identity_override_spellings_reach_native_worlds_and_new_documents() { identity_override(false).await; }
#[cfg(feature = "stealth")]
#[tokio::test(flavor = "current_thread")]
async fn stealth_identity_override_spellings_reach_native_worlds_and_new_documents() { identity_override(true).await; }
