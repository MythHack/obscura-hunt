//! `DOM.getOuterHTML` is the one DOM call a CDP client makes while holding a
//! JavaScript element handle rather than a node id: go-rod's `element.HTML()`
//! sends `DOM.getOuterHTML {objectId}` (rod/element.go), because it never asked
//! for a node id in the first place.
//!
//! Chromium accepts any of `nodeId`, `backendNodeId` or `objectId` here.
//! obscura accepted only the first two, so every rod `page.HTML()` - the call
//! behind the crawler's static HTML collection - came back with
//! `{-32601 nodeId required}` and the page was recorded as a failure even
//! though it had loaded fine.
//!
//! The objectId lookup is shared with `DOM.describeNode` / `DOM.resolveNode`
//! via `resolve_node_id`, so a handle that cannot be mapped to a node fails the
//! same way in all three rather than silently returning node 0.

use obscura_cdp::dispatch::{dispatch, CdpContext};
use obscura_cdp::types::CdpRequest;
use serde_json::{json, Value};

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

#[tokio::test(flavor = "current_thread")]
async fn outer_html_is_available_for_a_node_only_object_id() {
    let mut ctx = CdpContext::new();
    let page_id = ctx.create_page();
    let session_id = "session-1";
    ctx.sessions.insert(session_id.to_string(), page_id.clone());

    cdp(
        &mut ctx,
        1,
        "Page.navigate",
        json!({"url": "data:text/html,<div id=a><b>hi</b></div>", "waitUntil": "load"}),
        session_id,
    )
    .await;

    let handle = cdp(
        &mut ctx,
        2,
        "Runtime.evaluate",
        json!({"expression": "document.getElementById('a')"}),
        session_id,
    )
    .await;
    let object_id = handle
        .pointer("/result/objectId")
        .and_then(|v| v.as_str())
        .expect("an element handle must come back as an objectId")
        .to_string();

    // The regression itself: this used to answer `nodeId required`.
    let by_object_id = cdp(
        &mut ctx,
        3,
        "DOM.getOuterHTML",
        json!({"objectId": object_id}),
        session_id,
    )
    .await;

    assert_eq!(
        by_object_id
            .get("outerHTML")
            .and_then(|v| v.as_str()),
        Some("<div id=\"a\"><b>hi</b></div>"),
        "the HTML must come from the element the handle points at; got {by_object_id:#?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn outer_html_still_accepts_a_plain_node_id() {
    let mut ctx = CdpContext::new();
    let page_id = ctx.create_page();
    let session_id = "session-1";
    ctx.sessions.insert(session_id.to_string(), page_id.clone());

    cdp(
        &mut ctx,
        1,
        "Page.navigate",
        json!({"url": "data:text/html,<p id=a>text</p>", "waitUntil": "load"}),
        session_id,
    )
    .await;

    let document = cdp(&mut ctx, 0, "DOM.getDocument", json!({}), session_id).await;
    let document_id = document["root"]["nodeId"].as_u64().expect("document nodeId");

    let found = cdp(
        &mut ctx,
        2,
        "DOM.querySelector",
        json!({"nodeId": document_id, "selector": "#a"}),
        session_id,
    )
    .await;
    let node_id = found
        .get("nodeId")
        .and_then(|v| v.as_u64())
        .expect("querySelector must hand back a nodeId");

    let html = cdp(
        &mut ctx,
        3,
        "DOM.getOuterHTML",
        json!({"nodeId": node_id}),
        session_id,
    )
    .await;

    assert_eq!(
        html.get("outerHTML").and_then(|v| v.as_str()),
        Some("<p id=\"a\">text</p>"),
        "the nodeId path must keep working; got {html:#?}"
    );

    let by_backend_id = cdp(
        &mut ctx,
        4,
        "DOM.getOuterHTML",
        json!({"backendNodeId": node_id}),
        session_id,
    )
    .await;
    assert_eq!(by_backend_id, html, "backendNodeId must resolve the same document node");
}

#[tokio::test(flavor = "current_thread")]
async fn outer_html_uses_the_object_handles_child_document() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let fixture = tokio::spawn(async move {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buffer = [0; 2048];
            let read = stream.read(&mut buffer).await.unwrap();
            let request = String::from_utf8_lossy(&buffer[..read]);
            let body = if request.starts_with("GET /child.html ") {
                "<!doctype html><div id=a><b>child</b></div>"
            } else {
                "<!doctype html><div id=a>parent</div><iframe src='/child.html'></iframe>"
            };
            stream.write_all(format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len(),
            ).as_bytes()).await.unwrap();
        }
    });
    let context = obscura_browser::context::BrowserContext::with_storage_and_network(
        "child-html-test".into(), None, false, None, None, true,
    );
    let mut ctx = CdpContext::new_with_shared_context(std::sync::Arc::new(context));
    let page_id = ctx.create_page();
    let session = "child-html";
    ctx.sessions.insert(session.to_string(), page_id);
    cdp(&mut ctx, 1, "Page.navigate", json!({
        "url": format!("http://{address}/"),
        "waitUntil": "load",
    }), session).await;
    let tree = cdp(&mut ctx, 2, "Page.getFrameTree", json!({}), session).await;
    let frame = tree["frameTree"]["childFrames"][0]["frame"]["id"].as_str().expect("live child frame");
    fixture.await.expect("parent and child document responses");
    let world = cdp(&mut ctx, 3, "Page.createIsolatedWorld", json!({
        "frameId": frame, "worldName": "child-html-utility",
    }), session).await;
    let handle = cdp(&mut ctx, 4, "Runtime.evaluate", json!({
        "expression": "document.getElementById('a')", "contextId": world["executionContextId"],
    }), session).await;
    let object_id = handle["result"]["objectId"].as_str().expect("child element handle");
    let described = cdp(&mut ctx, 5, "DOM.describeNode", json!({"objectId": object_id}), session).await;
    for params in [
        json!({"objectId": object_id}),
        json!({"nodeId": described["node"]["nodeId"]}),
        json!({"backendNodeId": described["node"]["backendNodeId"]}),
    ] {
        let html = cdp(&mut ctx, 6, "DOM.getOuterHTML", params, session).await;
        assert_eq!(html["outerHTML"], "<div id=\"a\"><b>child</b></div>");
    }
    cdp(&mut ctx, 7, "Runtime.evaluate", json!({
        "expression": "document.querySelector('iframe').remove()",
    }), session).await;
    let retired = dispatch(&CdpRequest {
        id: 8, method: "DOM.getOuterHTML".to_string(), params: json!({"objectId": object_id}),
        session_id: Some(session.to_string()),
    }, &mut ctx).await;
    assert!(retired.error.is_some(), "a detached frame handle must not resolve against the parent document");
}
