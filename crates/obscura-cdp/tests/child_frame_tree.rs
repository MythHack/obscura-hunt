//! Regression test for the protocol half of issue #600: `Page.getFrameTree`
//! reported `childFrames: []` however many frames a page had built, and no
//! `Page.frameAttached` was ever emitted, so a Playwright or Puppeteer client
//! saw a single-frame page and could never address the child.

use obscura_cdp::dispatch::{dispatch, CdpContext};
use obscura_cdp::types::CdpRequest;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// `/` embeds `/child.html`, which itself embeds `/grandchild.html`, so the
/// tree is deep enough to show nesting rather than a flat list.
async fn serve() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 2048];
                let read = socket.read(&mut buf).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..read]).to_string();
                let body = if request.starts_with("GET /denied-host.html ") {
                    "<html><body><iframe src=\"/denied.html\"></iframe></body></html>"
                } else if request.starts_with("GET /denied.html ") {
                    "<html><body><script>window.secret = 'must-not-run';</script></body></html>"
                } else if request.starts_with("GET /child.html ") {
                    r#"<html><head><style>html,body{margin:0}iframe{position:absolute;left:30px;top:20px;width:100px;height:80px;border:0;padding:0}</style></head><body><iframe src="/grandchild.html"></iframe></body></html>"#
                } else if request.starts_with("GET /grandchild.html ") {
                    r#"<html><head><style>html,body{margin:0}#target{position:absolute;left:7px;top:9px;box-sizing:content-box;width:40px;height:30px;border:2px solid black;padding:3px;margin:0}</style></head><body><button id="target">deep</button></body></html>"#
                } else {
                    r#"<html><head><style>html,body{margin:0}iframe{position:absolute;left:100px;top:50px;width:200px;height:150px;border:0;padding:0}#cover{display:none;position:absolute;left:100px;top:50px;width:200px;height:150px}</style></head><body><iframe src="/child.html"></iframe><div id="cover"></div></body></html>"#
                };
                let policy = if request.starts_with("GET /denied.html ") {
                    "Content-Security-Policy: frame-ancestors 'none'\r\nContent-Security-Policy: img-src 'self'\r\n"
                } else { "" };
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n{policy}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(resp.as_bytes()).await;
            });
        }
    });
    format!("http://{addr}/")
}

async fn cdp(ctx: &mut CdpContext, id: u64, method: &str, params: Value, session: &str) -> Value {
    let resp = dispatch(
        &CdpRequest {
            id,
            method: method.to_string(),
            params,
            session_id: Some(session.to_string()),
        },
        ctx,
    )
    .await;
    assert!(resp.error.is_none(), "CDP {method} failed: {:?}", resp.error);
    resp.result.unwrap_or_else(|| json!({}))
}

/// Gets a page and a session the way a real client does, rather than by
/// inserting a session for a hand-made page: `Target.createTarget` then
/// `Target.attachToTarget`, which is the only route Puppeteer and Playwright
/// can take and therefore the only one worth asserting against.
async fn attached_session(ctx: &mut CdpContext) -> String {
    let created = dispatch(
        &CdpRequest {
            id: 900,
            method: "Target.createTarget".to_string(),
            params: json!({"url": "about:blank"}),
            session_id: None,
        },
        ctx,
    )
    .await
    .result
    .expect("Target.createTarget produced no result");
    let target_id = created["targetId"].as_str().expect("no targetId").to_string();

    let attached = dispatch(
        &CdpRequest {
            id: 901,
            method: "Target.attachToTarget".to_string(),
            params: json!({"targetId": target_id, "flatten": true}),
            session_id: None,
        },
        ctx,
    )
    .await
    .result
    .expect("Target.attachToTarget produced no result");
    attached["sessionId"]
        .as_str()
        .expect("no sessionId")
        .to_string()
}

#[tokio::test(flavor = "current_thread")]
async fn get_frame_tree_reports_nested_child_frames() {
    std::env::set_var("OBSCURA_ALLOW_PRIVATE_NETWORK", "1");
    let url = serve().await;
    let mut ctx = CdpContext::new();
    let session = &attached_session(&mut ctx).await;

    // Deliberately no `waitUntil`: that is what Puppeteer and Playwright send,
    // and it resolves to DomContentLoaded rather than to load. Passing
    // "load" here hid the frame build behind a readiness level no real client
    // asks for, so the tree came back empty for every one of them.
    cdp(&mut ctx, 1, "Page.navigate", json!({"url": url}), session).await;
    // Frames are built when the page settles, which is not part of the
    // navigation itself.
    cdp(&mut ctx, 2, "Runtime.evaluate", json!({"expression": "1"}), session).await;

    let tree = cdp(&mut ctx, 3, "Page.getFrameTree", json!({}), session).await;
    let root = &tree["frameTree"];
    let child = &root["childFrames"][0];
    assert!(
        child["frame"]["url"]
            .as_str()
            .unwrap_or_default()
            .ends_with("/child.html"),
        "no child frame in the tree: {tree}"
    );
    assert_eq!(
        child["frame"]["parentId"], root["frame"]["id"],
        "the child does not point back at the main frame"
    );

    let grandchild = &child["childFrames"][0];
    assert!(
        grandchild["frame"]["url"]
            .as_str()
            .unwrap_or_default()
            .ends_with("/grandchild.html"),
        "a frame inside a frame is missing: {tree}"
    );
    assert_eq!(grandchild["frame"]["parentId"], child["frame"]["id"]);

    // A client builds its frame list from the events, so the tree alone is not
    // enough. They have to carry the session the client attached with, because
    // a client discards anything addressed to a session it does not hold, and
    // Target.createTarget leaves a second session on the same page.
    let child_id = child["frame"]["id"].as_str().unwrap().to_string();
    let page_id = ctx.sessions.get(session.as_str()).cloned().unwrap();
    // Announcing to one arbitrary session of the page is what the ordering of a
    // HashMap decides, so require every session on the page to be told. That
    // makes the client's own session covered whichever one it is.
    for (candidate, owner) in &ctx.sessions {
        if owner != &page_id {
            continue;
        }
        assert!(
            ctx.pending_events.iter().any(|e| {
                e.method == "Page.frameAttached"
                    && e.params["frameId"] == child_id
                    && e.session_id.as_deref() == Some(candidate.as_str())
            }),
            "session {candidate} on the page was never told the child frame attached"
        );
    }

    let mine = |e: &obscura_cdp::types::CdpEvent| e.session_id.as_deref() == Some(session.as_str());
    let attached = ctx
        .pending_events
        .iter()
        .position(|e| {
            e.method == "Page.frameAttached" && e.params["frameId"] == child_id && mine(e)
        })
        .expect("no Page.frameAttached for the child frame on the client's own session");
    let navigated = ctx
        .pending_events
        .iter()
        .position(|e| {
            e.method == "Page.frameNavigated" && e.params["frame"]["id"] == child_id && mine(e)
        })
        .expect("no Page.frameNavigated for the child frame on the client's own session");
    assert!(
        attached < navigated,
        "frameAttached must come before frameNavigated"
    );

    // Each frame is announced once, however many commands the client sends.
    let before = ctx.pending_events.len();
    cdp(&mut ctx, 4, "Page.getFrameTree", json!({}), session).await;
    let repeats = ctx.pending_events[before..]
        .iter()
        .filter(|e| e.method == "Page.frameAttached")
        .count();
    assert_eq!(repeats, 0, "the same frame was announced twice");
}

#[tokio::test(flavor = "current_thread")]
async fn child_default_contexts_are_owned_by_their_exact_frame() {
    std::env::set_var("OBSCURA_ALLOW_PRIVATE_NETWORK", "1");
    let url = serve().await;
    let mut ctx = CdpContext::new();
    let session = attached_session(&mut ctx).await;

    cdp(&mut ctx, 1, "Page.navigate", json!({"url": url}), &session).await;
    cdp(&mut ctx, 2, "Runtime.evaluate", json!({"expression": "1"}), &session).await;
    let target = ctx.sessions[&session].clone();
    let attached = dispatch(
        &CdpRequest {
            id: 3,
            method: "Target.attachToTarget".to_string(),
            params: json!({"targetId": target, "flatten": true}),
            session_id: None,
        },
        &mut ctx,
    ).await.result.unwrap();
    let second_session = attached["sessionId"].as_str().unwrap().to_string();
    for runtime_session in [&session, &second_session] {
        cdp(&mut ctx, 3, "Runtime.enable", json!({}), runtime_session).await;
    }
    let tree = cdp(&mut ctx, 4, "Page.getFrameTree", json!({}), &session).await;
    let main_id = tree["frameTree"]["frame"]["id"].as_str().unwrap().to_string();
    let child_id = tree["frameTree"]["childFrames"][0]["frame"]["id"]
        .as_str().unwrap().to_string();
    let grandchild_id = tree["frameTree"]["childFrames"][0]["childFrames"][0]["frame"]["id"]
        .as_str().unwrap().to_string();

    let main_world = cdp(
        &mut ctx, 5, "Page.createIsolatedWorld",
        json!({"frameId": main_id.clone(), "worldName": "utility"}), &session,
    ).await["executionContextId"].as_i64().unwrap();
    let default_for = |frame_id: &str| ctx.pending_events.iter().find(|event| {
        event.method == "Runtime.executionContextCreated"
            && event.session_id.as_deref() == Some(session.as_str())
            && event.params["context"]["auxData"]["frameId"] == frame_id
            && event.params["context"]["auxData"]["isDefault"] == true
    }).unwrap().params["context"]["id"].as_i64().unwrap();
    let child_world = default_for(&child_id);
    let grandchild_world = default_for(&grandchild_id);
    assert_ne!(main_world, child_world);
    assert_ne!(child_world, grandchild_world);
    let child_utility = cdp(&mut ctx, 6, "Page.createIsolatedWorld",
        json!({"frameId":child_id,"worldName":"utility"}), &session).await["executionContextId"].as_i64().unwrap();
    let grandchild_utility = cdp(&mut ctx, 6, "Page.createIsolatedWorld",
        json!({"frameId":grandchild_id,"worldName":"utility"}), &session).await["executionContextId"].as_i64().unwrap();
    assert_ne!(child_utility, child_world);
    assert_ne!(grandchild_utility, grandchild_world);
    cdp(&mut ctx, 7, "Runtime.evaluate", json!({
        "expression": "globalThis.marker = 'host'", "returnByValue": true,
    }), &session).await;
    let child_marker = cdp(&mut ctx, 7, "Runtime.evaluate", json!({
        "expression": "globalThis.marker = 'child'; [marker, location.pathname]",
        "contextId": child_world, "returnByValue": true,
    }), &session).await;
    assert_eq!(child_marker["result"]["value"], json!(["child", "/child.html"]));
    let host_marker = cdp(&mut ctx, 7, "Runtime.evaluate", json!({
        "expression": "marker", "returnByValue": true,
    }), &session).await;
    assert_eq!(host_marker["result"]["value"], "host");
    let utility_marker = cdp(&mut ctx, 7, "Runtime.evaluate", json!({
        "expression":"[typeof marker,location.pathname,document.body.setAttribute('data-world','utility')]",
        "contextId":child_utility,"returnByValue":true,
    }), &session).await;
    assert_eq!(utility_marker["result"]["value"][0], "undefined");
    assert_eq!(utility_marker["result"]["value"][1], "/child.html");
    let shared = cdp(&mut ctx, 7, "Runtime.evaluate", json!({
        "expression":"document.body.getAttribute('data-world')","contextId":child_world,"returnByValue":true,
    }), &session).await;
    assert_eq!(shared["result"]["value"], "utility");

    let unknown = dispatch(
        &CdpRequest {
            id: 8,
            method: "Page.createIsolatedWorld".to_string(),
            params: json!({"frameId": "missing-frame", "worldName": "utility"}),
            session_id: Some(session.clone()),
        },
        &mut ctx,
    ).await;
    assert!(unknown.error.is_some());

    ctx.pending_events.clear();
    cdp(
        &mut ctx,
        9,
        "Runtime.evaluate",
        json!({"expression": "document.querySelector('iframe').remove()"}),
        &session,
    ).await;
    for _ in 0..3 {
        ctx.get_session_page_mut(&Some(session.clone()))
            .unwrap()
            .run_autonomous_event_loop_turn()
            .await
            .unwrap();
    }
    cdp(&mut ctx, 10, "Runtime.evaluate", json!({"expression": "1"}), &session).await;

    for (frame_id, context_id) in [
        (&child_id, child_world),
        (&grandchild_id, grandchild_world),
        (&child_id, child_utility),
        (&grandchild_id, grandchild_utility),
    ] {
        for runtime_session in [&session, &second_session] {
            let destroyed = ctx.pending_events.iter().position(|event| {
                event.method == "Runtime.executionContextDestroyed"
                    && event.session_id.as_deref() == Some(runtime_session.as_str())
                    && event.params["executionContextId"] == context_id
            }).unwrap_or_else(|| panic!(
                "missing executionContextDestroyed for {frame_id}/{context_id}: {:?}",
                ctx.pending_events.iter().map(|event| (
                    event.method.as_str(),
                    event.session_id.as_deref(),
                    event.params.clone(),
                )).collect::<Vec<_>>(),
            ));
            let detached = ctx.pending_events.iter().position(|event| {
                event.method == "Page.frameDetached"
                    && event.session_id.as_deref() == Some(runtime_session.as_str())
                    && event.params["frameId"] == frame_id.as_str()
            }).expect("missing frameDetached");
            assert!(destroyed < detached, "context must be destroyed before its frame detaches");
        }
    }

    let main_still_routes = dispatch(
        &CdpRequest {
            id: 11,
            method: "Runtime.evaluate".to_string(),
            params: json!({"expression": "1", "contextId": main_world}),
            session_id: Some(session.clone()),
        },
        &mut ctx,
    ).await;
    assert!(main_still_routes.error.is_none());
    for (id, stale_context) in [(12, child_world), (13, grandchild_world), (14, child_utility), (15, grandchild_utility)] {
        let stale = dispatch(
            &CdpRequest {
                id,
                method: "Runtime.evaluate".to_string(),
                params: json!({"expression": "1", "contextId": stale_context}),
                session_id: Some(session.clone()),
            },
            &mut ctx,
        ).await;
        assert!(stale.error.is_some());
    }

    ctx.pending_events.clear();
    cdp(
        &mut ctx, 14, "Page.navigate",
        json!({"url": "data:text/html,<p>replacement</p>", "waitUntil": "load"}),
        &session,
    ).await;
    let recreated = ctx.pending_events.iter().filter(|event| {
        event.method == "Runtime.executionContextCreated"
            && event.session_id.as_deref() == Some(session.as_str())
            && event.params["context"]["name"] == "utility"
    }).collect::<Vec<_>>();
    assert_eq!(recreated.len(), 1, "only the main-frame world should persist");
    assert_eq!(recreated[0].params["context"]["auxData"]["frameId"], main_id);

}

#[cfg(feature = "render")]
#[tokio::test(flavor = "current_thread")]
async fn isolated_grandchild_quads_use_retained_owner_projection_and_gesture_gates() {
    std::env::set_var("OBSCURA_ALLOW_PRIVATE_NETWORK", "1");
    let mut ctx = CdpContext::new();
    let session = attached_session(&mut ctx).await;
    cdp(&mut ctx, 1, "Page.navigate", json!({"url":serve().await,"waitUntil":"load"}), &session).await;
    cdp(&mut ctx, 2, "Runtime.enable", json!({}), &session).await;
    let tree = cdp(&mut ctx, 3, "Page.getFrameTree", json!({}), &session).await;
    let child = tree["frameTree"]["childFrames"][0]["frame"]["id"].as_str().unwrap();
    let grandchild = tree["frameTree"]["childFrames"][0]["childFrames"][0]["frame"]["id"].as_str().unwrap();
    let world = cdp(&mut ctx, 4, "Page.createIsolatedWorld", json!({"frameId":grandchild,"worldName":"quad-utility"}), &session)
        .await["executionContextId"].as_i64().unwrap();
    let object = cdp(&mut ctx, 5, "Runtime.evaluate", json!({
        "contextId":world,"expression":"(()=>{const node=document.getElementById('target');node.getBoundingClientRect=()=>{throw new Error('author geometry must not run')};globalThis.clicks=0;node.addEventListener('click',()=>clicks++);return node})()",
    }), &session).await["result"]["objectId"].as_str().unwrap().to_string();
    let described = cdp(&mut ctx, 6, "DOM.describeNode", json!({"objectId":object}), &session).await;
    let backend = described["node"]["backendNodeId"].clone();
    let model = cdp(&mut ctx, 7, "DOM.getBoxModel", json!({"backendNodeId":backend}), &session).await["model"].clone();
    let quads = cdp(&mut ctx, 8, "DOM.getContentQuads", json!({"backendNodeId":backend}), &session).await["quads"].clone();
    assert_eq!(quads, json!([model["content"].clone()]));
    let content = model["content"].as_array().unwrap();
    let border = model["border"].as_array().unwrap();
    assert_eq!(content[0], json!(142));
    assert_eq!(content[1], json!(84));
    assert_eq!(border[0], json!(137));
    assert_eq!(border[1], json!(79));
    assert_eq!(model["width"], json!(50));
    assert_eq!(model["height"], json!(40));
    for event in ["mousePressed", "mouseReleased"] {
        cdp(&mut ctx, 9, "Input.dispatchMouseEvent", json!({"type":event,"x":145,"y":90,"button":"left"}), &session).await;
    }
    assert_eq!(cdp(&mut ctx, 10, "Runtime.evaluate", json!({"contextId":world,"expression":"clicks === 1","returnByValue":true}), &session).await["result"]["value"], true);
    cdp(&mut ctx, 11, "Input.dispatchMouseEvent", json!({"type":"mousePressed","x":145,"y":90,"button":"left"}), &session).await;
    cdp(&mut ctx, 12, "Runtime.evaluate", json!({"expression":"document.getElementById('cover').style.display='block'"}), &session).await;
    let obscured = dispatch(&CdpRequest { id: 13, method:"Input.dispatchMouseEvent".to_string(), params:json!({"type":"mouseReleased","x":145,"y":90,"button":"left"}), session_id:Some(session.clone()) }, &mut ctx).await;
    assert!(obscured.error.is_some(), "an occluded pressed descendant must not receive release/click");
    cdp(&mut ctx, 14, "Runtime.evaluate", json!({"expression":"document.getElementById('cover').style.display='none'"}), &session).await;
    for event in ["mousePressed", "mouseReleased"] {
        cdp(&mut ctx, 15, "Input.dispatchMouseEvent", json!({"type":event,"x":145,"y":90,"button":"left"}), &session).await;
    }
    assert_eq!(cdp(&mut ctx, 16, "Runtime.evaluate", json!({"contextId":world,"expression":"clicks === 2","returnByValue":true}), &session).await["result"]["value"], true);
    assert!(!child.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn protected_response_does_not_create_an_executable_frame() {
    std::env::set_var("OBSCURA_ALLOW_PRIVATE_NETWORK", "1");
    let mut ctx = CdpContext::new();
    let session = attached_session(&mut ctx).await;
    let base = serve().await;
    cdp(&mut ctx, 1, "Network.enable", json!({}), &session).await;
    cdp(&mut ctx, 2, "Page.navigate",
        json!({"url": format!("{base}denied-host.html"), "waitUntil": "load"}),
        &session).await;
    assert!(ctx.pending_events.iter().any(|event|
        event.method == "Network.responseReceived"
            && event.params["response"]["url"] == format!("{base}denied.html")
            && event.params["response"]["status"] == 200),
        "the protected response must actually reach the browser");
    let tree = cdp(&mut ctx, 3, "Page.getFrameTree", json!({}), &session).await;
    assert_eq!(tree["frameTree"]["childFrames"], json!([]),
        "every enforced CSP field must prevent a live child frame");
}
