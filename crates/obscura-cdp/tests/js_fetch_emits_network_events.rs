// Regression for issue #406: requests initiated by page JS (fetch/XHR/dynamic
// resource) must emit Network.requestWillBeSent / responseReceived so
// Puppeteer/Playwright `page.on('request'|'response')` observe them. On main
// only the static navigation subresources surfaced; a `fetch()` fired from the
// page produced no CDP Network event, so clients captured zero XHR/JSON
// responses (this is also the root cause of the Aviasales half of #394).

use obscura_cdp::dispatch::{dispatch, CdpContext};
use obscura_cdp::types::CdpRequest;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

// Serves an HTML page that fetches /api/start.json, which redirects to the
// JSON response at /api/data.json.
async fn serve() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        for _ in 0..12 {
            let (mut socket, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let mut buf = [0u8; 2048];
                let _ = socket.read(&mut buf).await.unwrap();
                let req = String::from_utf8_lossy(&buf[..]);
                if req.starts_with("GET /api/start.json") {
                    let resp = "HTTP/1.1 302 Found\r\nLocation: /api/data.json\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                    let _ = socket.write_all(resp.as_bytes()).await;
                    return;
                }
                if req.starts_with("GET /api/missing.json") {
                    let body = r#"{"error":"missing"}"#;
                    let response = format!("HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                    let _ = socket.write_all(response.as_bytes()).await;
                    return;
                }
                let (ct, body) = if req.starts_with("GET /api/data.json") {
                    ("application/json", "{\"value\":42}")
                } else {
                    (
                        "text/html",
                        r#"<html><head></head><body>
<div id="r">stage1</div>
<script>
window.__done = new Promise(function (resolve) {
  fetch("/api/start.json")
    .then(function (r) { return r.json(); })
    .then(function (d) { document.getElementById("r").textContent = "got:" + d.value; resolve("ok"); })
    .catch(function (e) { resolve("err:" + e); });
});
</script>
</body></html>"#,
                    )
                };
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {ct}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(resp.as_bytes()).await;
            });
        }
    });
    format!("http://{addr}/")
}

async fn cdp(ctx: &mut CdpContext, id: u64, method: &str, params: Value, session_id: &str) -> Value {
    let resp = dispatch(
        &CdpRequest {
            id,
            method: method.to_string(),
            params,
            session_id: Some(session_id.to_string()),
        },
        ctx,
    )
    .await;
    assert!(resp.error.is_none(), "CDP {method} failed: {:?}", resp.error);
    resp.result.unwrap_or_else(|| json!({}))
}

// Collect the request URLs from every Network.requestWillBeSent currently
// queued in ctx.pending_events, then clear the queue.
fn drain_request_urls(ctx: &mut CdpContext) -> Vec<String> {
    let urls = ctx
        .pending_events
        .iter()
        .filter(|e| e.method == "Network.requestWillBeSent")
        .filter_map(|e| e.params.get("request").and_then(|r| r.get("url")).and_then(|u| u.as_str()).map(str::to_string))
        .collect();
    ctx.pending_events.clear();
    urls
}

// The requestId that Network.responseReceived reported for the given URL.
fn response_request_id(ctx: &CdpContext, url_needle: &str) -> Option<String> {
    ctx.pending_events
        .iter()
        .find(|e| {
            e.method == "Network.responseReceived"
                && e.params
                    .get("response")
                    .and_then(|r| r.get("url"))
                    .and_then(|u| u.as_str())
                    .map(|u| u.contains(url_needle))
                    .unwrap_or(false)
        })
        .and_then(|e| e.params.get("requestId").and_then(|v| v.as_str()).map(str::to_string))
}

#[tokio::test(flavor = "current_thread")]
async fn js_fetch_emits_network_request_and_response() {
    std::env::set_var("OBSCURA_ALLOW_PRIVATE_NETWORK", "1");
    let base = serve().await;
    let mut ctx = CdpContext::new();
    let page_id = ctx.create_page();
    let session_id = "session-1";
    ctx.sessions.insert(session_id.to_string(), page_id.clone());
    cdp(&mut ctx, 900, "Network.enable", json!({}), session_id).await;

    // An ordinary fetch() is not load-delaying in Chromium: `load` may fire
    // while its response is still pending. Ask for networkidle0 explicitly so
    // this output-level assertion observes the completed request without
    // turning every load navigation into an implicit global settle.
    cdp(
        &mut ctx,
        1,
        "Page.navigate",
        json!({"url": base, "waitUntil": "networkidle0"}),
        session_id,
    )
    .await;

    // The final fetched JSON URL must appear as a requestWillBeSent event.
    let request_urls = ctx
        .pending_events
        .iter()
        .filter(|e| e.method == "Network.requestWillBeSent")
        .filter_map(|e| e.params.get("request").and_then(|r| r.get("url")).and_then(|u| u.as_str()).map(str::to_string))
        .collect::<Vec<_>>();
    assert!(
        request_urls.iter().any(|u| u.contains("/api/data.json")),
        "script-initiated fetch must emit Network.requestWillBeSent; saw {request_urls:?}"
    );
    // And its response body must be resolvable via the same requestId, so a
    // client can read the captured JSON.
    let request_id = response_request_id(&ctx, "/api/data.json")
        .expect("fetch must emit Network.responseReceived with a requestId");
    let body = cdp(
        &mut ctx,
        2,
        "Network.getResponseBody",
        json!({"requestId": request_id}),
        session_id,
    )
    .await;
    assert_eq!(
        body.get("body").and_then(|b| b.as_str()),
        Some("{\"value\":42}"),
        "Network.getResponseBody must return the script-fetched JSON"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn navigation_without_script_fetch_is_unaffected() {
    // A page that issues no script fetch must still emit exactly its document
    // request, proving the #406 change adds nothing spurious.
    std::env::set_var("OBSCURA_ALLOW_PRIVATE_NETWORK", "1");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 1024];
        let _ = socket.read(&mut buf).await.unwrap();
        let body = "<html><body>plain</body></html>";
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = socket.write_all(resp.as_bytes()).await;
    });
    let base = format!("http://{addr}/");

    let mut ctx = CdpContext::new();
    let page_id = ctx.create_page();
    let session_id = "session-1";
    ctx.sessions.insert(session_id.to_string(), page_id.clone());
    cdp(&mut ctx, 900, "Network.enable", json!({}), session_id).await;

    cdp(&mut ctx, 1, "Page.navigate", json!({"url": base, "waitUntil": "load"}), session_id).await;

    let urls = drain_request_urls(&mut ctx);
    assert!(
        urls.iter().any(|u| u == &base || u.starts_with(&base)),
        "the document request must still be emitted; saw {urls:?}"
    );
    assert!(
        !urls.iter().any(|u| u.contains("/api/")),
        "no spurious script-fetch events for a page that makes none; saw {urls:?}"
    );
}

#[cfg(feature = "stealth")]
async fn stealth_context() -> (CdpContext, String, String) {
    let mut ctx = CdpContext::new();
    ctx.default_context = std::sync::Arc::new(obscura_browser::BrowserContext::with_storage_and_network("stealth-events".into(), None, true, None, None, true,).unwrap());
    let page = ctx.create_page();
    let session = "stealth-session".to_string();
    ctx.sessions.insert(session.clone(), page);
    cdp(&mut ctx, 900, "Network.enable", json!({}), &session).await;
    let base = serve().await;
    cdp(&mut ctx, 1, "Page.navigate", json!({"url":base,"waitUntil":"networkidle0"}), &session).await;
    (ctx, session, base)
}

#[cfg(feature = "stealth")]
fn recorded_live_request_id(ctx: &mut CdpContext, session: &str) -> String {
    // dispatch() does not run the WebSocket server's post-command drain.
    let page = ctx.get_session_page_mut(&Some(session.to_string())).unwrap();
    page.sync_js_network_events();
    let event = page.network_events.pop().expect("scripted request recorded by the transport");
    assert!(event.url.ends_with("/api/data.json"));
    assert_eq!(event.status, 200);
    assert_eq!(event.method, "GET");
    assert_eq!(event.body_size, 12);
    event.request_id
}

#[cfg(feature = "stealth")]
#[tokio::test(flavor = "current_thread")]
async fn stealth_fetch_records_redirect_identity_and_readable_response_body() {
    let (mut ctx, session, _base) = stealth_context().await;
    let request_id = response_request_id(&ctx, "/api/data.json").expect("stealth response event");
    let events = ctx.pending_events.iter().filter(|event|
        event.method.starts_with("Network.") && event.params["requestId"] == request_id
    ).collect::<Vec<_>>();
    assert_eq!(events.iter().map(|event| event.method.as_str()).collect::<Vec<_>>(),
        ["Network.requestWillBeSent", "Network.responseReceived", "Network.loadingFinished"]);
    assert_eq!(events[1].params["response"]["status"], 200);
    assert_eq!(events[2].params["encodedDataLength"], 12);
    let body = cdp(&mut ctx, 2, "Network.getResponseBody", json!({"requestId":request_id}), &session).await;
    assert_eq!(body["body"], r#"{"value":42}"#);
    assert_eq!(body["base64Encoded"], false);
}

#[cfg(feature = "stealth")]
#[tokio::test(flavor = "current_thread")]
async fn stealth_xhr_records_events_after_navigation() {
    let (mut ctx, session, _base) = stealth_context().await;
    ctx.pending_events.clear();
    let response = cdp(&mut ctx, 2, "Runtime.evaluate", json!({
        "expression":"new Promise((resolve,reject)=>{let xhr=new XMLHttpRequest();xhr.open('GET','/api/data.json');xhr.onload=()=>resolve(xhr.responseText);xhr.onerror=reject;xhr.send()})",
        "awaitPromise":true,"returnByValue":true
    }), &session).await;
    assert_eq!(response["result"]["value"], r#"{"value":42}"#);
    let request_id = recorded_live_request_id(&mut ctx, &session);
    let body = cdp(&mut ctx, 3, "Network.getResponseBody", json!({"requestId":request_id}), &session).await;
    assert_eq!(body["body"], r#"{"value":42}"#);
}

#[cfg(feature = "stealth")]
#[tokio::test(flavor = "current_thread")]
async fn stealth_events_survive_disabled_response_body_retention() {
    std::env::set_var("OBSCURA_NETWORK_BODY_BUFFER_ENTRIES", "0");
    let (mut ctx, session, _base) = stealth_context().await;
    let request_id = response_request_id(&ctx, "/api/data.json").expect("retention opt-out must not disable observation");
    let response = dispatch(&CdpRequest { id: 2, method: "Network.getResponseBody".into(),
        params: json!({"requestId":request_id}), session_id: Some(session) }, &mut ctx).await;
    assert!(response.error.is_some(), "opt-out must not retain the body");
}

#[cfg(feature = "stealth")]
#[tokio::test(flavor = "current_thread")]
async fn stealth_response_body_buffer_evicts_old_requests() {
    std::env::set_var("OBSCURA_NETWORK_BODY_BUFFER_ENTRIES", "2");
    let (mut ctx, session, _base) = stealth_context().await;
    let oldest = response_request_id(&ctx, "/api/data.json").expect("first response");
    let mut latest = String::new();
    for id in 2..6 {
        ctx.pending_events.clear();
        let response = cdp(&mut ctx, id, "Runtime.evaluate", json!({"expression":"fetch('/api/data.json').then(r=>r.text())", "awaitPromise":true, "returnByValue":true}), &session).await;
        assert_eq!(response["result"]["value"], r#"{"value":42}"#);
        latest = recorded_live_request_id(&mut ctx, &session);
    }
    let evicted = dispatch(&CdpRequest { id: 6, method: "Network.getResponseBody".into(),
        params: json!({"requestId":oldest}), session_id: Some(session.clone()) }, &mut ctx).await;
    assert!(evicted.error.is_some(), "bounded cache must evict old responses");
    assert_eq!(cdp(&mut ctx, 7, "Network.getResponseBody", json!({"requestId":latest}), &session).await["body"], r#"{"value":42}"#);
}

#[cfg(feature = "stealth")]
#[tokio::test(flavor = "current_thread")]
async fn stealth_http_error_response_keeps_status_and_body() {
    let (mut ctx, session, _base) = stealth_context().await;
    let response = cdp(&mut ctx, 2, "Runtime.evaluate", json!({"expression":"fetch('/api/missing.json').then(async r=>{await r.text();return r.status})","awaitPromise":true,"returnByValue":true}), &session).await;
    assert_eq!(response["result"]["value"].as_f64(), Some(404.0));
    let page = ctx.get_session_page_mut(&Some(session.clone())).unwrap();
    page.sync_js_network_events();
    let event = page.network_events.pop().expect("HTTP error response must be recorded");
    assert_eq!(event.status, 404);
    assert!(event.url.ends_with("/api/missing.json"));
    assert_eq!(cdp(&mut ctx, 3, "Network.getResponseBody", json!({"requestId":event.request_id}), &session).await["body"], r#"{"error":"missing"}"#);
}

// The interception channel is shared with JS fetch, but a Document-only route
// must stop the native navigation before transport and leave later fetches alone.
#[tokio::test(flavor = "current_thread")]
async fn native_document_continue_keeps_owner_identity_and_request_headers() {
    use obscura_js::ops::InterceptResolution;
    use futures_util::FutureExt as _;
    use std::collections::HashMap;
    use std::time::Duration;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let mut ctx = CdpContext::new();
    ctx.default_context = std::sync::Arc::new(obscura_browser::BrowserContext::with_storage_and_network(
        "document-interception".into(), None, false, None, None, true,
    ).unwrap());
    let page_id = ctx.create_page();
    let sid = format!("{page_id}-session");
    ctx.sessions.insert(sid.clone(), page_id.clone());
    let frame_id = ctx.get_page(&page_id).unwrap().frame_id.clone();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    ctx.intercept_tx = Some(tx);
    cdp(&mut ctx, 1, "Fetch.enable", json!({"patterns": [{"resourceType": "Document"}]}), &sid).await;
    let page = ctx.get_page_mut(&page_id).unwrap();
    let (network_tx, mut network_rx) = tokio::sync::mpsc::unbounded_channel();
    page.set_network_event_tx(network_tx);
    page.set_extra_headers(HashMap::from([("X-Existing".into(), "preserved".into())])).await;
    page.context.cookie_jar.set_cookie("session=logged-in; Path=/", &url::Url::parse(&origin).unwrap());
    let original = format!("{origin}/original");
    let rewritten = format!("{origin}/continued");
    let navigate = page.navigate_with_wait_post(&original, obscura_browser::WaitUntil::Load, "POST", "a=1");
    let consume = async {
        let request = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.unwrap().unwrap();
        assert_eq!(request.page_id, page_id);
        assert_eq!(request.frame_id, 0);
        assert_eq!(request.parent_frame_id, None);
        assert_eq!(request.resource_type, "Document");
        assert_eq!(request.url, original);
        assert_eq!(request.method, "POST");
        assert_eq!(request.headers.get("x-existing").map(String::as_str), Some("preserved"));
        assert!(listener.accept().now_or_never().is_none(), "document was sent before Continue");
        let mut headers = request.headers;
        headers.insert("x-request-proof".into(), "one-navigation".into());
        request.resolver.send(InterceptResolution::Continue {
            url: Some(rewritten.clone()), method: None, headers: Some(headers), body: None,
        }).unwrap();
        for index in 0..2 {
            let (mut socket, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept()).await.unwrap().unwrap();
            let mut wire = Vec::new();
            loop {
                let mut buffer = [0; 4096];
                let count = socket.read(&mut buffer).await.unwrap();
                assert_ne!(count, 0);
                wire.extend_from_slice(&buffer[..count]);
                if let Some(end) = wire.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&wire[..end]).to_ascii_lowercase();
                    let length = head.lines().find_map(|line| line.strip_prefix("content-length:").map(str::trim))
                        .unwrap_or("0").parse::<usize>().unwrap();
                    if wire.len() >= end + 4 + length { break; }
                }
            }
            let wire = String::from_utf8(wire).unwrap().to_ascii_lowercase();
            if index == 0 {
                assert!(wire.starts_with("post /continued http/1.1\r\n"), "{wire}");
                assert!(wire.contains("x-existing: preserved\r\n"), "{wire}");
                assert!(wire.contains("x-request-proof: one-navigation\r\n"), "{wire}");
                assert!(wire.contains("cookie: session=logged-in\r\n"), "{wire}");
                assert!(wire.ends_with("\r\n\r\na=1"), "{wire}");
            } else {
                assert!(wire.starts_with("get /api/session http/1.1\r\n"), "{wire}");
                assert!(!wire.contains("x-request-proof:"), "navigation override leaked into JS fetch");
            }
            let body = if index == 0 { "<script>fetch('/api/session')</script>" } else { "ok" };
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
        }
        request.request_id
    };
    let (result, request_id) = tokio::time::timeout(Duration::from_secs(10), async { tokio::join!(navigate, consume) }).await.unwrap();
    result.unwrap();
    let page = ctx.get_page_mut(&page_id).unwrap();
    assert_eq!(page.frame_id, frame_id);
    assert_eq!(page.url_string(), rewritten);
    let receipts = std::iter::from_fn(|| network_rx.try_recv().ok()).collect::<Vec<_>>();
    let (owner, document) = receipts.iter().find(|(_, event)|
        event.resource_type == "Document" && !event.request_started).unwrap();
    assert_eq!(owner, &page_id);
    assert_eq!(document.request_id, request_id);
    assert_eq!(document.method, "POST");
    assert!(page.get_response_body(&request_id).is_some());
    assert!(rx.try_recv().is_err(), "Document-only pattern paused a JS fetch");
}

#[tokio::test(flavor = "current_thread")]
async fn native_document_fail_never_sends_request() {
    use obscura_js::ops::InterceptResolution;
    use futures_util::FutureExt as _;
    use std::time::Duration;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target = format!("http://{}/blocked", listener.local_addr().unwrap());
    let context = std::sync::Arc::new(obscura_browser::BrowserContext::with_storage_and_network(
        "document-fail".into(), None, false, None, None, true,
    ).unwrap());
    let mut page = obscura_browser::Page::new("document-fail".into(), context);
    let mut rx = page.enable_interception();
    let consume = async {
        let request = rx.recv().await.unwrap();
        assert_eq!(request.page_id, "document-fail");
        assert_eq!(request.resource_type, "Document");
        request.resolver.send(InterceptResolution::Fail { reason: "BlockedByClient".into() }).unwrap();
    };
    let (result, ()) = tokio::time::timeout(Duration::from_secs(5), async { tokio::join!(page.navigate(&target), consume) }).await.unwrap();
    assert!(result.unwrap_err().to_string().contains("BlockedByClient"));
    assert!(listener.accept().now_or_never().is_none(), "failed document reached transport");
}
