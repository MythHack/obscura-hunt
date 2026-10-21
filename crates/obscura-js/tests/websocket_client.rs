//! Page-facing WebSocket regressions. Every success crosses an actual TCP peer.
use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};
use std::time::Duration;
use futures_util::{SinkExt, StreamExt};
use obscura_dom::{DomTree, parse_html};
use obscura_js::runtime::{CdpExecutionWorld, ObscuraJsRuntime};
use obscura_net::{CookieJar, ObscuraHttpClient};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::{Message, protocol::{CloseFrame, frame::{Frame, coding::{CloseCode, Data, OpCode}}}};

fn runtime(origin: &str, allow: bool, stealth: bool) -> ObscuraJsRuntime {
    let jar = Arc::new(CookieJar::new());
    jar.set_cookie("socket_cookie=owned; SameSite=Strict; Path=/", &origin.parse().unwrap());
    let client = Arc::new(ObscuraHttpClient::with_full_options(jar.clone(), None, allow));
    let mut runtime = ObscuraJsRuntime::with_base_url(origin);
    runtime.set_dom(parse_html("<!doctype html><html><head></head><body></body></html>"));
    runtime.set_url(origin);
    runtime.set_cookie_jar(jar.clone());
    runtime.set_http_client(client);
    #[cfg(feature = "stealth")]
    if stealth { runtime.set_stealth_client(Arc::new(obscura_net::StealthHttpClient::with_proxy(jar, None, allow))); }
    #[cfg(not(feature = "stealth"))]
    assert!(!stealth);
    runtime.run_page_init();
    runtime
}

async fn evaluate(runtime: &mut ObscuraJsRuntime, source: &str) -> Value {
    let result = runtime.evaluate_for_cdp_in_frame(CdpExecutionWorld::default(), source, true, true, 5_000).await.unwrap();
    assert!(!result.thrown, "page WebSocket consumer threw: {result:?}");
    result.value.unwrap()
}

#[tokio::test]
async fn page_socket_echoes_real_unicode_and_binary_payloads_on_each_selected_transport() {
    for stealth in [false, true].into_iter().filter(|stealth| !*stealth || cfg!(feature = "stealth")) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_hdr_async(stream, |request: &tokio_tungstenite::tungstenite::handshake::server::Request,
                mut response: tokio_tungstenite::tungstenite::handshake::server::Response| {
                assert_eq!(request.headers()["origin"], format!("http://{address}"));
                assert!(request.headers()["cookie"].to_str().unwrap().contains("socket_cookie=owned"));
                assert_eq!(request.headers()["sec-fetch-mode"], "websocket");
                assert_eq!(request.headers()["sec-fetch-site"], "same-origin");
                assert_eq!(request.headers()["sec-websocket-protocol"], "echo.v1");
                response.headers_mut().insert("sec-websocket-protocol", "echo.v1".parse().unwrap());
                Ok(response)
            }).await.unwrap();
            let mut received = Vec::new();
            while let Some(message) = socket.next().await {
                match message.unwrap() {
                    message @ (Message::Text(_) | Message::Binary(_)) => {
                        received.push(message.clone());
                        socket.send(message).await.unwrap();
                    }
                    Message::Close(_) => { let _ = socket.flush().await; break; }
                    _ => {},
                }
            }
            received
        });
        let mut runtime = runtime(&format!("http://{address}/document"), true, stealth);
        let result = evaluate(&mut runtime, r#"new Promise((resolve, reject) => {
            const socket = new WebSocket('/echo', ['echo.v1']);
            const seen = []; let buffered = false; let opens = 0; let firstBinary;
            socket.onerror = () => reject(new Error('real echo failed'));
            socket.onopen = event => {
                opens++; if (!event.isTrusted || socket.readyState !== WebSocket.OPEN) reject('untrusted open');
                let invalid;
                try { socket.close(1001); } catch (error) { invalid = error.name; }
                if (invalid !== 'InvalidAccessError') reject('invalid close accepted');
                socket.send('hello 世界');
                socket.send(new Blob([new Uint8Array([0,255,128])]));
                const bytes = new Uint8Array([9,0,254,8]);
                socket.send(bytes.buffer);
                socket.send(bytes.subarray(1,3));
                socket.send(new DataView(bytes.buffer,1,2));
                buffered = socket.bufferedAmount > 0;
            };
            socket.onmessage = event => {
                if (typeof event.data === 'string') seen.push(Promise.resolve(['text', event.data]));
                else if (event.data instanceof Blob) {
                    firstBinary = 'blob'; socket.binaryType = 'arraybuffer';
                    seen.push(event.data.arrayBuffer().then(bytes => ['binary', Array.from(new Uint8Array(bytes))]));
                } else seen.push(Promise.resolve(['binary', Array.from(new Uint8Array(event.data))]));
                if (seen.length === 5) socket.close(4000, '再见');
            };
            socket.onclose = async event => {
                socket.send('é');
                resolve({opens, buffered, firstBinary, protocol: socket.protocol, extensions: socket.extensions,
                    state: socket.readyState, close: [event.code,event.reason,event.wasClean,event.isTrusted],
                    afterClose: socket.bufferedAmount, messages: await Promise.all(seen)});
            };
        })"#).await;
        assert_eq!(result, json!({"opens":1,"buffered":true,"firstBinary":"blob","protocol":"echo.v1","extensions":"",
            "state":3,"close":[4000,"再见",true,true],"afterClose":2,
            "messages":[["text","hello 世界"],["binary",[0,255,128]],["binary",[9,0,254,8]],["binary",[0,254]],["binary",[0,254]]]}));
        let received = server.await.unwrap();
        assert_eq!(received.len(), 5, "send must put every payload on the wire, not echo locally");
        assert_eq!(received[0], Message::text("hello 世界"));
        assert_eq!(received[3], Message::binary(vec![0,254]));
    }
}

#[tokio::test]
async fn fragmented_messages_ping_pong_and_server_close_are_browser_events() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        socket.send(Message::Frame(Frame::message(vec![b's',b'p',b'l',b'i',b't',b' ',0xe7], OpCode::Data(Data::Text), false))).await.unwrap();
        socket.send(Message::Ping(vec![0,255,19].into())).await.unwrap();
        socket.send(Message::Frame(Frame::message(vec![0x95,0x8c], OpCode::Data(Data::Continue), true))).await.unwrap();
        socket.send(Message::Frame(Frame::message(vec![0,255], OpCode::Data(Data::Binary), false))).await.unwrap();
        socket.send(Message::Frame(Frame::message(vec![128], OpCode::Data(Data::Continue), true))).await.unwrap();
        loop {
            if let Message::Pong(payload) = socket.next().await.unwrap().unwrap() {
                assert_eq!(payload.as_ref(), &[0,255,19]); break;
            }
        }
        socket.send(Message::Close(Some(CloseFrame { code: CloseCode::Library(4001), reason: "peer 完成".into() }))).await.unwrap();
        assert!(matches!(socket.next().await.unwrap().unwrap(), Message::Close(_)));
    });
    let mut runtime = runtime(&format!("http://{address}/"), true, false);
    let result = evaluate(&mut runtime, r#"new Promise((resolve,reject) => {
        const socket = new WebSocket('/fragments'); socket.binaryType='arraybuffer'; const seen=[];
        socket.onmessage = event => seen.push(typeof event.data === 'string' ? event.data : Array.from(new Uint8Array(event.data)));
        socket.onerror = () => reject('fragment/pong failed');
        socket.onclose = event => resolve([seen,event.code,event.reason,event.wasClean]);
    })"#).await;
    assert_eq!(result, json!([["split 界",[0,255,128]],4001,"peer 完成",true]));
    server.await.unwrap();
}

#[tokio::test]
async fn policy_denial_and_failed_upgrade_never_report_open() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(AtomicUsize::new(0));
    let count = requests.clone();
    let server = tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            count.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let mut bytes=[0;4096]; let _=stream.read(&mut bytes).await;
                stream.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
            });
        }
    });
    let consumer = r#"new Promise(resolve => {
        let opens=0,errors=0; const socket=new WebSocket('/denied');
        socket.onopen=()=>opens++; socket.onerror=()=>errors++;
        socket.onclose=event=>resolve([opens,errors,event.code,event.wasClean,socket.readyState]);
    })"#;
    let mut guarded = runtime(&format!("http://{address}/"), false, false);
    assert_eq!(evaluate(&mut guarded, consumer).await, json!([0,1,1006,false,3]));
    drop(guarded);
    assert_eq!(requests.load(Ordering::SeqCst),0,"private destination must be rejected before I/O");
    let mut csp = runtime(&format!("http://{address}/"), true, false);
    csp.set_document_response_policy(&std::collections::HashMap::from([("content-security-policy".into(), "default-src *; connect-src 'none'".into())]));
    assert_eq!(evaluate(&mut csp, consumer).await, json!([0,1,1006,false,3]));
    assert_eq!(requests.load(Ordering::SeqCst),0,"CSP must reject before Upgrade");
    drop(csp);
    let mut allowed = runtime(&format!("http://{address}/"), true, false);
    assert_eq!(evaluate(&mut allowed, consumer).await, json!([0,1,1006,false,3]));
    assert_eq!(requests.load(Ordering::SeqCst),1,"failed Upgrade must be a real failed server response");
    let errors = allowed.evaluate(r#"[
        (()=>{try{new WebSocket('ws://localhost/#fragment')}catch(e){return e.name}})(),
        (()=>{try{new WebSocket('ws://localhost/',['x','x'])}catch(e){return e.name}})(),
        (()=>{try{new WebSocket('file:///socket')}catch(e){return e.name}})()
    ]"#).unwrap();
    assert_eq!(errors,json!(["SyntaxError","SyntaxError","SyntaxError"]));
    server.abort();
}

#[tokio::test]
async fn invalid_utf8_fails_instead_of_replacing_message_data() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        socket.get_mut().write_all(&[0x81,0x01,0xff]).await.unwrap();
        assert!(matches!(socket.next().await.unwrap().unwrap(), Message::Close(Some(frame)) if frame.code==CloseCode::Invalid));
    });
    let mut runtime = runtime(&format!("http://{address}/"), true, false);
    let result = evaluate(&mut runtime, r#"new Promise(resolve => {
        const socket=new WebSocket('/invalid'); let opens=0,errors=0,messages=0;
        socket.onopen=()=>opens++; socket.onmessage=()=>messages++; socket.onerror=()=>errors++;
        socket.onclose=event=>resolve([opens,errors,messages,event.code,event.wasClean]);
    })"#).await;
    assert_eq!(result,json!([1,1,0,1006,false]));
    server.await.unwrap();
}

#[tokio::test]
async fn retiring_document_aborts_inflight_upgrade_without_retired_callbacks() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (requested_tx, requested_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request=Vec::new(); let mut byte=[0];
        while !request.ends_with(b"\r\n\r\n") { stream.read_exact(&mut byte).await.unwrap(); request.push(byte[0]); }
        requested_tx.send(()).unwrap();
        let mut byte=[0];
        tokio::time::timeout(Duration::from_secs(2),stream.read(&mut byte)).await.unwrap().unwrap()
    });
    let mut runtime = runtime(&format!("http://{address}/"), true, false);
    runtime.evaluate("globalThis.events=[]; globalThis.socket=new WebSocket('/stall'); socket.onopen=()=>events.push('open'); socket.onerror=()=>events.push('error'); socket.onclose=()=>events.push('close')").unwrap();
    tokio::time::timeout(Duration::from_secs(2),requested_rx).await.unwrap().unwrap();
    runtime.set_dom(DomTree::new());
    runtime.run_event_loop_bounded(100).await.unwrap();
    assert_eq!(server.await.unwrap(),0,"navigation must drop the in-flight native transport");
    assert_eq!(runtime.evaluate("events").unwrap(),json!([]),"retired document callbacks must not escape");
}
