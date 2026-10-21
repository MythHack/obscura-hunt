use std::sync::Arc;
use obscura_browser::BrowserContext;
use obscura_cdp::{dispatch::{dispatch, CdpContext}, types::CdpRequest};
use serde_json::{json, Value};

async fn call(ctx: &mut CdpContext, session: &str, method: &str, params: Value) -> Value {
    let response = dispatch(&CdpRequest { id: 1, method: method.into(), params,
        session_id: Some(session.into()) }, ctx).await;
    assert!(response.error.is_none(), "{method}: {:?}", response.error);
    response.result.unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn domains_and_retained_bodies_belong_to_the_observing_session() {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let fixture = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
        let mut request = Vec::new();
        let mut byte = [0];
        while !request.ends_with(b"\r\n\r\n") { socket.read_exact(&mut byte).unwrap(); request.push(byte[0]); }
        let body = "<!doctype html><title>observed body</title><p>one shared response</p>";
        write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
    });
    let context = BrowserContext::with_storage_and_network("sessions".into(), None, false, None, None, true)
        .expect("in-memory fixture context");
    let mut ctx = CdpContext::new_with_shared_context(Arc::new(context));
    let page = ctx.create_page();
    for session in ["a", "b", "silent"] { ctx.sessions.insert(session.into(), page.clone()); }
    call(&mut ctx, "a", "Page.enable", json!({})).await;
    call(&mut ctx, "b", "Runtime.enable", json!({})).await;
    for session in ["a", "b"] { call(&mut ctx, session, "Network.enable", json!({})).await; }
    ctx.pending_events.clear();
    call(&mut ctx, "a", "Page.navigate", json!({"url": format!("http://{address}/")})).await;
    fixture.join().unwrap();
    assert!(ctx.pending_events.iter().any(|event| event.method == "Page.frameNavigated" && event.session_id.as_deref() == Some("a")));
    assert!(!ctx.pending_events.iter().any(|event| event.method.starts_with("Page.") && event.session_id.as_deref() == Some("b")));
    assert!(!ctx.pending_events.iter().any(|event| event.method.starts_with("Network.") && event.session_id.as_deref() == Some("silent")));
    let response_id = ctx.pending_events.iter().find(|event| event.method == "Network.responseReceived"
        && event.session_id.as_deref() == Some("b")).unwrap().params["requestId"].as_str().unwrap().to_string();
    let expected = call(&mut ctx, "b", "Network.getResponseBody", json!({"requestId": response_id})).await;
    assert!(expected["body"].as_str().unwrap().contains("one shared response"));
    call(&mut ctx, "a", "Network.disable", json!({})).await;
    assert_eq!(call(&mut ctx, "b", "Network.getResponseBody", json!({"requestId": response_id})).await, expected);
    for session in ["a", "silent"] {
        let response = dispatch(&CdpRequest { id: 2, method: "Network.getResponseBody".into(),
            params: json!({"requestId": response_id}), session_id: Some(session.into()) }, &mut ctx).await;
        assert!(response.error.is_some(), "an unobserving session must not acquire another session's body");
    }
    ctx.pending_events.clear();
    call(&mut ctx, "a", "Runtime.evaluate", json!({"expression":"console.log('runtime subscriber only')"})).await;
    assert!(ctx.pending_events.iter().any(|event| event.method == "Runtime.consoleAPICalled" && event.session_id.as_deref() == Some("b")));
    assert!(!ctx.pending_events.iter().any(|event| event.method.starts_with("Runtime.") && event.session_id.as_deref() == Some("a")));
    call(&mut ctx, "b", "Network.disable", json!({})).await;
    call(&mut ctx, "b", "Network.enable", json!({})).await;
    let response = dispatch(&CdpRequest { id: 3, method: "Network.getResponseBody".into(),
        params: json!({"requestId": response_id}), session_id: Some("b".into()) }, &mut ctx).await;
    assert!(response.error.is_some(), "reenabling must not resurrect released observation receipts");
}
