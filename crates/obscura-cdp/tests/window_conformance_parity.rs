// Conformance parity: a handful of standard Web APIs that real Chrome
// exposes but bootstrap.js left undefined, so vendor JS that feature-detects
// them dies with "X is not a function". Each case fails on main, passes
// after the stubs land. These are spec surface gaps, not anti-bot work.

use futures_util::{SinkExt, StreamExt};
use obscura_cdp::dispatch::{dispatch, CdpContext};
use obscura_cdp::types::CdpRequest;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_tungstenite::{connect_async, tungstenite::Message};

async fn serve_once() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        tokio::spawn(async move {
            let mut buf = [0u8; 2048];
            let _ = socket.read(&mut buf).await.unwrap();
            let body = "<html><body><div id=a></div></body></html>";
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = socket.write_all(resp.as_bytes()).await;
        });
    });
    format!("http://{addr}/")
}

async fn cdp(
    ctx: &mut CdpContext,
    id: u64,
    method: &str,
    params: Value,
    session_id: &str,
) -> Value {
    let resp = dispatch(
        &CdpRequest {
            id,
            method: method.to_string(),
            params,
            session_id: (session_id != "browser").then(|| session_id.to_string()),
        },
        ctx,
    )
    .await;
    assert!(resp.error.is_none(), "CDP {method} failed: {:?}", resp.error);
    resp.result.unwrap_or_else(|| json!({}))
}

async fn eval(ctx: &mut CdpContext, id: u64, expr: &str, session_id: &str) -> Value {
    cdp(
        ctx,
        id,
        "Runtime.evaluate",
        json!({"expression": expr, "returnByValue": true, "awaitPromise": true}),
        session_id,
    )
    .await
}

async fn setup() -> (CdpContext, String) {
    std::env::set_var("OBSCURA_ALLOW_PRIVATE_NETWORK", "1");
    let url = serve_once().await;
    let mut ctx = CdpContext::new();
    let page_id = ctx.create_page();
    let session_id = "session-1";
    ctx.sessions.insert(session_id.to_string(), page_id.clone());
    cdp(
        &mut ctx,
        1,
        "Page.navigate",
        json!({"url": url, "waitUntil": "load"}),
        session_id,
    )
    .await;
    (ctx, session_id.to_string())
}

#[tokio::test(flavor = "current_thread")]
async fn element_toggle_attribute_is_callable_and_toggles() {
    let (mut ctx, sid) = setup().await;
    // Element.prototype.toggleAttribute is on every real Chrome. Bootstrap
    // leaves it undefined, so any framework that calls el.toggleAttribute()
    // (Lit, Stencil, several ad SDKs) throws.
    let v = eval(
        &mut ctx,
        2,
        r#"(function () {
            const el = document.getElementById('a');
            const t = typeof el.toggleAttribute;
            const first = el.toggleAttribute('hidden');
            const afterFirst = el.hasAttribute('hidden');
            const second = el.toggleAttribute('hidden');
            const afterSecond = el.hasAttribute('hidden');
            return JSON.stringify({ type: t, first, afterFirst, second, afterSecond });
        })()"#,
        &sid,
    )
    .await;
    let val = serde_json::from_str::<Value>(v["result"]["value"].as_str().unwrap()).unwrap();
    assert_eq!(val["type"], "function");
    assert_eq!(val["first"], true, "first toggle should add the attribute");
    assert_eq!(val["afterFirst"], true);
    assert_eq!(val["second"], false, "second toggle should remove it");
    assert_eq!(val["afterSecond"], false);
}

#[tokio::test(flavor = "current_thread")]
async fn document_adopt_node_moves_node() {
    let (mut ctx, sid) = setup().await;
    // document.adoptNode is standard DOM; bootstrap leaves it undefined.
    // Frameworks that move nodes between documents (iframes, portals) call it.
    let v = eval(
        &mut ctx,
        2,
        r#"(function () {
            const t = typeof document.adoptNode;
            const span = document.createElement('span');
            span.id = 'moved';
            const adopted = document.adoptNode(span);
            return JSON.stringify({
                type: t,
                sameNode: adopted === span,
                ownerDoc: adopted.ownerDocument === document,
            });
        })()"#,
        &sid,
    )
    .await;
    let val = serde_json::from_str::<Value>(v["result"]["value"].as_str().unwrap()).unwrap();
    assert_eq!(val["type"], "function");
    assert_eq!(val["sameNode"], true);
    assert_eq!(val["ownerDoc"], true);
}

#[tokio::test(flavor = "current_thread")]
async fn assignment_shadows_window_named_element_property() {
    let (mut ctx, sid) = setup().await;
    let v = eval(
        &mut ctx,
        2,
        r#"(function () {
            'use strict';
            const named = window.a;
            window.a = 42;
            const descriptor = Object.getOwnPropertyDescriptor(window, 'a');
            return JSON.stringify({
                startedAsElement: named === document.getElementById('a'),
                value: window.a,
                writable: descriptor.writable,
                hasGetter: typeof descriptor.get === 'function',
            });
        })()"#,
        &sid,
    )
    .await;
    let val = serde_json::from_str::<Value>(v["result"]["value"].as_str().unwrap()).unwrap();
    assert_eq!(val["startedAsElement"], true);
    assert_eq!(val["value"], 42);
    assert_eq!(val["writable"], true);
    assert_eq!(val["hasGetter"], false);
}

#[cfg(feature = "render")]
#[tokio::test(flavor = "current_thread")]
async fn root_document_timer_and_intersection_observer_survive_second_navigation() {
    let mut ctx = CdpContext::new();
    let target = cdp(&mut ctx, 1, "Target.createTarget", json!({"url": "about:blank"}), "browser").await;
    let target_id = target["targetId"].as_str().unwrap();
    let session = format!("{target_id}-session");
    let first_url = "data:text/html,<html><head><title>First document</title></head><body><button id='first'>First</button></body></html>";
    let navigation = cdp(&mut ctx, 2, "Page.navigate", json!({"url": first_url, "waitUntil": "load"}), &session).await;
    assert!(navigation.get("errorText").is_none(), "first navigation failed: {navigation}");
    let first = cdp(&mut ctx, 3, "Runtime.evaluate", json!({
        "expression": "new Promise(resolve => setTimeout(() => resolve({title: document.title, connected: document.getElementById('first').isConnected}), 5))",
        "returnByValue": true, "awaitPromise": true, "timeout": 3000,
    }), &session).await;
    assert_eq!(first["result"]["value"], json!({"title": "First document", "connected": true}));
    let second_url = "data:text/html,<html><head><title>Second document</title></head><body><button id='second' style='position:absolute;left:16px;top:16px;width:120px;height:40px;box-sizing:border-box;'>Second</button></body></html>";
    let navigation = cdp(&mut ctx, 4, "Page.navigate", json!({"url": second_url, "waitUntil": "load"}), &session).await;
    assert!(navigation.get("errorText").is_none(), "second navigation failed: {navigation}");
    let second = cdp(&mut ctx, 5, "Runtime.evaluate", json!({
        "expression": r#"new Promise(resolve => setTimeout(() => {
            const button = document.getElementById('second');
            const observer = new IntersectionObserver(entries => {
                const entry = entries.find(entry => entry.target === button);
                if (!entry) return;
                observer.disconnect();
                resolve({title: document.title, connected: button.isConnected,
                    visible: entry.isIntersecting, ratio: entry.intersectionRatio,
                    x: entry.boundingClientRect.x, y: entry.boundingClientRect.y,
                    width: entry.boundingClientRect.width, height: entry.boundingClientRect.height,
                    intersectionWidth: entry.intersectionRect.width,
                    intersectionHeight: entry.intersectionRect.height});
            });
            observer.observe(button);
        }, 5))"#,
        "returnByValue": true, "awaitPromise": true, "timeout": 3000,
    }), &session).await;
    let visible = &second["result"]["value"];
    assert_eq!(visible["title"], "Second document");
    assert_eq!(visible["connected"], true);
    assert_eq!(visible["visible"], true);
    assert_eq!(visible["ratio"].as_f64(), Some(1.0));
    assert_eq!(visible["x"].as_f64(), Some(16.0));
    assert_eq!(visible["y"].as_f64(), Some(16.0));
    assert_eq!(visible["width"].as_f64(), Some(120.0));
    assert_eq!(visible["height"].as_f64(), Some(40.0));
    assert_eq!(visible["intersectionWidth"].as_f64(), Some(120.0));
    assert_eq!(visible["intersectionHeight"].as_f64(), Some(40.0));
}

async fn window_fixture() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else { break };
            tokio::spawn(async move {
                let mut request = [0u8; 4096];
                let size = socket.read(&mut request).await.unwrap();
                let request = String::from_utf8_lossy(&request[..size]);
                let popup = request.starts_with("GET /popup ");
                let body = if popup {
                    r#"<html><head><title>Native popup</title></head><body><script>
                    window.addEventListener('load', () => {
                        if (window.opener) window.opener.postMessage('popup-ready', location.origin);
                    });
                    </script></body></html>"#
                } else {
                    "<html><head><title>Native opener</title></head><body></body></html>"
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            });
        }
    });
    format!("http://{addr}/")
}

async fn window_setup() -> (CdpContext, String, String, String) {
    std::env::set_var("OBSCURA_ALLOW_PRIVATE_NETWORK", "1");
    let url = window_fixture().await;
    let mut ctx = CdpContext::new();
    let target = cdp(&mut ctx, 1, "Target.createTarget", json!({"url": url}), "browser").await;
    let target_id = target["targetId"].as_str().unwrap().to_string();
    let session = format!("{target_id}-session");
    cdp(&mut ctx, 2, "Target.setDiscoverTargets", json!({"discover": true}), "browser").await;
    ctx.pending_events.clear();
    (ctx, target_id, session, url)
}

#[tokio::test(flavor = "current_thread")]
async fn native_popup_target_reports_opener_url_and_title() {
    let (mut ctx, opener, session, url) = window_setup().await;
    let result = cdp(&mut ctx, 3, "Runtime.evaluate", json!({
        "expression": "new Promise(resolve => { window.addEventListener('message', e => { if (e.data === 'popup-ready') resolve(e.source === popup); }, {once: true}); window.popup = window.open('/popup', 'native-popup'); })",
        "returnByValue": true, "awaitPromise": true, "timeout": 3000,
    }), &session).await;
    assert_eq!(result["result"]["value"], true, "message source must be the stable native popup proxy");
    let targets = cdp(&mut ctx, 4, "Target.getTargets", json!({}), "browser").await;
    let popup = targets["targetInfos"].as_array().unwrap().iter()
        .find(|info| info["targetId"] != opener).expect("script-created popup target");
    assert_eq!(popup["openerId"], opener);
    assert_eq!(popup["url"], format!("{url}popup"));
    assert_eq!(popup["title"], "Native popup");
    assert_eq!(popup["canAccessOpener"], true);
    let created = ctx.pending_events.iter().find(|event| event.method == "Target.targetCreated"
        && event.params["targetInfo"]["targetId"] == popup["targetId"]).unwrap();
    assert_eq!(created.params["targetInfo"]["openerId"], opener);
    assert!(created.params["targetInfo"]["canAccessOpener"].is_boolean());
}

#[tokio::test(flavor = "current_thread")]
async fn native_popup_message_progresses_during_awaited_function() {
    let (mut ctx, _, session, _) = window_setup().await;
    let result = cdp(&mut ctx, 3, "Runtime.callFunctionOn", json!({
        "functionDeclaration": "function () { return new Promise(resolve => { window.addEventListener('message', e => { if (e.data === 'popup-ready') resolve({data: e.data, source: e.source === popup, origin: e.origin === location.origin}); }, {once: true}); window.popup = window.open('/popup', 'awaited-popup'); }); }",
        "returnByValue": true, "awaitPromise": true, "timeout": 3000,
    }), &session).await;
    assert_eq!(result["result"]["value"], json!({"data": "popup-ready", "source": true, "origin": true}));
}

#[tokio::test(flavor = "current_thread")]
async fn native_popup_close_updates_proxy_and_destroys_target_once() {
    let (mut ctx, opener, session, _) = window_setup().await;
    eval(&mut ctx, 3, "(function () { window.popup = window.open('', 'close-popup'); window.savedPopup = popup; return popup.closed; })()", &session).await;
    let targets = cdp(&mut ctx, 4, "Target.getTargets", json!({}), "browser").await;
    let popup_id = targets["targetInfos"].as_array().unwrap().iter()
        .find(|info| info["targetId"] != opener).unwrap()["targetId"].as_str().unwrap().to_string();
    let attached = cdp(&mut ctx, 5, "Target.attachToTarget", json!({"targetId": popup_id, "flatten": true}), "browser").await;
    ctx.pending_events.clear();
    let closed = cdp(&mut ctx, 6, "Target.closeTarget", json!({"targetId": popup_id}), "browser").await;
    assert_eq!(closed["success"], true);
    let destroyed = ctx.pending_events.iter().filter(|event| event.method == "Target.targetDestroyed"
        && event.params["targetId"] == popup_id).count();
    assert_eq!(destroyed, 1);
    assert!(ctx.pending_events.iter().any(|event| event.method == "Target.detachedFromTarget"
        && event.params["sessionId"] == attached["sessionId"]));
    let state = eval(&mut ctx, 7, "({closed: popup.closed, openerClosed: window.closed, same: savedPopup === popup})", &session).await;
    assert_eq!(state["result"]["value"], json!({"closed": true, "openerClosed": false, "same": true}));
    let targets = cdp(&mut ctx, 8, "Target.getTargets", json!({}), "browser").await;
    assert_eq!(targets["targetInfos"].as_array().unwrap().len(), 1, "a later command must not resurrect a closed popup");
    assert_eq!(targets["targetInfos"][0]["targetId"], opener);
}

#[tokio::test(flavor = "current_thread")]
async fn native_noopener_popup_has_no_false_opener_target() {
    let (mut ctx, opener, session, _) = window_setup().await;
    let opened = eval(&mut ctx, 3, "window.open('', 'detached-popup', 'noopener') === null", &session).await;
    assert_eq!(opened["result"]["value"], true);
    let targets = cdp(&mut ctx, 4, "Target.getTargets", json!({}), "browser").await;
    let popup = targets["targetInfos"].as_array().unwrap().iter()
        .find(|info| info["targetId"] != opener).unwrap();
    assert!(popup.get("openerId").is_none());
    assert!(popup.get("openerFrameId").is_none());
    assert_eq!(popup["canAccessOpener"], false);
    let attached = cdp(&mut ctx, 5, "Target.attachToTarget", json!({"targetId": popup["targetId"], "flatten": true}), "browser").await;
    let popup_session = attached["sessionId"].as_str().unwrap();
    let state = eval(&mut ctx, 6, "window.opener === null", popup_session).await;
    assert_eq!(state["result"]["value"], true);
}

#[tokio::test(flavor = "current_thread")]
async fn native_popup_script_close_detaches_actual_target() {
    let (mut ctx, opener, session, _) = window_setup().await;
    eval(&mut ctx, 3, "(window.popup = window.open('', 'script-close'), popup.closed)", &session).await;
    let targets = cdp(&mut ctx, 4, "Target.getTargets", json!({}), "browser").await;
    let popup_id = targets["targetInfos"].as_array().unwrap().iter()
        .find(|info| info["targetId"] != opener).unwrap()["targetId"].as_str().unwrap().to_string();
    let attached = cdp(&mut ctx, 5, "Target.attachToTarget", json!({"targetId": popup_id, "flatten": true}), "browser").await;
    ctx.pending_events.clear();
    let state = eval(&mut ctx, 6, "(popup.close(), {popupClosed: popup.closed, openerClosed: closed})", &session).await;
    assert_eq!(state["result"]["value"], json!({"popupClosed": true, "openerClosed": false}));
    assert!(ctx.pending_events.iter().any(|event| event.method == "Target.targetDestroyed"
        && event.params["targetId"] == popup_id));
    assert!(ctx.pending_events.iter().any(|event| event.method == "Target.detachedFromTarget"
        && event.params["sessionId"] == attached["sessionId"]));
    let targets = cdp(&mut ctx, 7, "Target.getTargets", json!({}), "browser").await;
    assert_eq!(targets["targetInfos"].as_array().unwrap().len(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn native_popup_open_and_close_in_one_script_reports_lifecycle_without_page() {
    let (mut ctx, opener, session, _) = window_setup().await;
    let browser = cdp(&mut ctx, 3, "Target.attachToBrowserTarget", json!({}), "browser").await;
    let browser_session = browser["sessionId"].as_str().unwrap();
    cdp(&mut ctx, 4, "Target.setDiscoverTargets", json!({"discover": true}), browser_session).await;
    cdp(&mut ctx, 5, "Target.setAutoAttach", json!({"autoAttach": true, "flatten": true, "waitForDebuggerOnStart": false}), browser_session).await;
    let opener_page = ctx.get_page(&opener).unwrap();
    let opener_frame = opener_page.frame_id.clone();
    let browser_context = opener_page.context.id.clone();
    let session_count = ctx.sessions.len();
    ctx.pending_events.clear();
    let state = eval(&mut ctx, 6, "(function () { const popup = window.open('', 'transient-popup'); popup.close(); return {popupClosed: popup.closed, openerClosed: closed}; })()", &session).await;
    assert_eq!(state["result"]["value"], json!({"popupClosed": true, "openerClosed": false}));
    let created = ctx.pending_events.iter().filter(|event| event.method == "Target.targetCreated"
        && event.params["targetInfo"]["targetId"] != opener).collect::<Vec<_>>();
    assert_eq!(created.len(), 2, "root and browser discovery must each receive one creation receipt");
    let popup_id = created[0].params["targetInfo"]["targetId"].as_str().unwrap().to_string();
    for subscription in [None, Some(browser_session)] {
        let receipt = created.iter().find(|event| event.session_id.as_deref() == subscription).unwrap();
        let info = &receipt.params["targetInfo"];
        assert_eq!(info["targetId"], popup_id);
        assert_eq!(info["openerId"], opener);
        assert_eq!(info["openerFrameId"], opener_frame);
        assert_eq!(info["canAccessOpener"], true);
        assert_eq!(info["browserContextId"], browser_context);
        assert_eq!(info["attached"], false);
        let destroyed = ctx.pending_events.iter().filter(|event| event.method == "Target.targetDestroyed"
            && event.params["targetId"] == popup_id && event.session_id.as_deref() == subscription).count();
        assert_eq!(destroyed, 1, "each discovery route must receive exactly one destruction receipt");
        let created_index = ctx.pending_events.iter().position(|event| event.method == "Target.targetCreated"
            && event.params["targetInfo"]["targetId"] == popup_id && event.session_id.as_deref() == subscription).unwrap();
        let destroyed_index = ctx.pending_events.iter().position(|event| event.method == "Target.targetDestroyed"
            && event.params["targetId"] == popup_id && event.session_id.as_deref() == subscription).unwrap();
        assert!(created_index < destroyed_index);
    }
    assert!(ctx.get_page(&popup_id).is_none(), "a closed native window must not gain a synthetic Page");
    assert_eq!(ctx.sessions.len(), session_count, "a closed native window must not gain an attachment");
    assert!(!ctx.pending_events.iter().any(|event| event.method == "Target.attachedToTarget"
        && event.params["targetInfo"]["targetId"] == popup_id));
    ctx.pending_events.clear();
    let targets = cdp(&mut ctx, 7, "Target.getTargets", json!({}), "browser").await;
    assert_eq!(targets["targetInfos"].as_array().unwrap().len(), 1);
    assert_eq!(targets["targetInfos"][0]["targetId"], opener);
    assert!(!ctx.pending_events.iter().any(|event| event.method.starts_with("Target.")
        && (event.params["targetId"] == popup_id || event.params["targetInfo"]["targetId"] == popup_id)),
        "later snapshots must not duplicate or resurrect the transient target");
}

#[tokio::test(flavor = "current_thread")]
async fn native_popup_auto_attachment_and_discovery_use_browser_session() {
    let (mut ctx, opener, session, _) = window_setup().await;
    let browser = cdp(&mut ctx, 3, "Target.attachToBrowserTarget", json!({}), "browser").await;
    let browser_session = browser["sessionId"].as_str().unwrap();
    cdp(&mut ctx, 4, "Target.setDiscoverTargets", json!({"discover": false}), "browser").await;
    cdp(&mut ctx, 5, "Target.setDiscoverTargets", json!({"discover": true}), browser_session).await;
    cdp(&mut ctx, 6, "Target.setAutoAttach", json!({"autoAttach": true, "flatten": true, "waitForDebuggerOnStart": false}), browser_session).await;
    ctx.pending_events.clear();
    eval(&mut ctx, 7, "(window.popup = window.open('', 'attached-popup'), popup.closed)", &session).await;
    let created = ctx.pending_events.iter().find(|event| event.method == "Target.targetCreated"
        && event.params["targetInfo"]["targetId"] != opener).unwrap();
    assert_eq!(created.session_id.as_deref(), Some(browser_session));
    let popup_id = created.params["targetInfo"]["targetId"].as_str().unwrap().to_string();
    let attached = ctx.pending_events.iter().find(|event| event.method == "Target.attachedToTarget"
        && event.params["targetInfo"]["targetId"] == popup_id).unwrap();
    assert_eq!(attached.session_id.as_deref(), Some(browser_session));
    assert_eq!(attached.params["waitingForDebugger"], false);
    let popup_session = attached.params["sessionId"].as_str().unwrap().to_string();
    let state = eval(&mut ctx, 8, "opener !== null && !closed", &popup_session).await;
    assert_eq!(state["result"]["value"], true);
    ctx.pending_events.clear();
    cdp(&mut ctx, 9, "Target.closeTarget", json!({"targetId": popup_id}), browser_session).await;
    let detached = ctx.pending_events.iter().find(|event| event.method == "Target.detachedFromTarget"
        && event.params["sessionId"] == popup_session).unwrap();
    assert_eq!(detached.session_id.as_deref(), Some(browser_session));
}

#[tokio::test(flavor = "current_thread")]
async fn native_popup_debugger_start_pause_is_not_falsely_advertised() {
    let mut ctx = CdpContext::new();
    let response = dispatch(&CdpRequest {
        id: 1, method: "Target.setAutoAttach".to_string(),
        params: json!({"autoAttach": true, "waitForDebuggerOnStart": true, "flatten": true}),
        session_id: None,
    }, &mut ctx).await;
    assert!(response.error.is_some(), "unsupported admission must not claim debugger pause semantics");
    assert!(ctx.pending_events.is_empty());
}

type WindowSocket = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn window_socket_command(ws: &mut WindowSocket, id: u64, method: &str, params: Value, session: Option<&str>) -> Value {
    let mut command = json!({"id": id, "method": method, "params": params});
    if let Some(session) = session { command["sessionId"] = json!(session); }
    ws.send(Message::Text(command.to_string().into())).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let message = ws.next().await.unwrap().unwrap();
            let Message::Text(text) = message else { continue };
            let value: Value = serde_json::from_str(&text).unwrap();
            if value["id"].as_u64() == Some(id) {
                assert!(value.get("error").is_none(), "CDP {method}: {value}");
                return value["result"].clone();
            }
        }
    }).await.expect("CDP command timed out")
}

#[tokio::test(flavor = "current_thread")]
async fn native_popup_load_and_message_advance_while_client_is_silent() {
    let fixture = window_fixture().await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let local = tokio::task::LocalSet::new();
    local.run_until(async move {
        let server = tokio::task::spawn_local(async move {
            obscura_cdp::start_with_serve_options_and_limit(
                port, "127.0.0.1", None, false, None, false, None, true, 1,
            ).await.unwrap();
        });
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let (mut ws, _) = connect_async(format!("ws://127.0.0.1:{port}/devtools/browser")).await.unwrap();
        let target = window_socket_command(&mut ws, 1, "Target.createTarget", json!({"url": fixture}), None).await;
        let opener = target["targetId"].as_str().unwrap().to_string();
        let session = format!("{opener}-session");
        window_socket_command(&mut ws, 2, "Target.setDiscoverTargets", json!({"discover": true}), None).await;
        window_socket_command(&mut ws, 3, "Runtime.enable", json!({}), Some(&session)).await;
        window_socket_command(&mut ws, 4, "Runtime.evaluate", json!({
            "expression": "window.addEventListener('message', e => { if (e.data === 'popup-ready') console.log('native-popup-ready', e.source === popup, e.origin === location.origin); }); setTimeout(() => { window.popup = window.open('/popup', 'silent-popup'); }, 50); true",
            "returnByValue": true, "awaitPromise": false,
        }), Some(&session)).await;
        // No subsequent command is allowed to drive this load or posted task.
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let mut popup = None;
            let mut received_message = false;
            while popup.is_none() || !received_message {
                let Message::Text(text) = ws.next().await.unwrap().unwrap() else { continue };
                let event: Value = serde_json::from_str(&text).unwrap();
                if event["method"] == "Target.targetCreated" && event["params"]["targetInfo"]["targetId"] != opener {
                    let info = &event["params"]["targetInfo"];
                    assert_eq!(info["openerId"], opener);
                    assert_eq!(info["canAccessOpener"], true);
                    popup = info["targetId"].as_str().map(str::to_string);
                }
                if event["method"] == "Runtime.consoleAPICalled" && event["params"]["args"][0]["value"] == "native-popup-ready" {
                    assert_eq!(event["sessionId"], session);
                    assert_eq!(event["params"]["args"][1]["value"], true);
                    assert_eq!(event["params"]["args"][2]["value"], true);
                    received_message = true;
                }
            }
        }).await.expect("silent-client native popup lifecycle stopped progressing");
        ws.close(None).await.unwrap();
        server.abort();
    }).await;
}
