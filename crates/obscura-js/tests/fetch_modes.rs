use obscura_js::ops::InterceptResolution;
use obscura_js::runtime::{CdpExecutionWorld, ObscuraJsRuntime};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

struct LocalServers {
    origin: String,
    foreign: String,
    requests: [Arc<Mutex<Vec<String>>>; 2],
    tasks: [tokio::task::JoinHandle<()>; 2],
}

impl LocalServers {
    async fn new() -> Self {
        let a = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let b = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", a.local_addr().unwrap());
        let foreign = format!("http://{}", b.local_addr().unwrap());
        let requests = [Arc::new(Mutex::new(Vec::new())), Arc::new(Mutex::new(Vec::new()))];
        let tasks = [(a, origin.clone(), foreign.clone(), requests[0].clone()),
            (b, foreign.clone(), origin.clone(), requests[1].clone())]
            .map(|(listener, own, other, capture)| {
                tokio::spawn(async move {
                    loop {
                        let (mut stream, _) = listener.accept().await.unwrap();
                        let mut bytes = Vec::new();
                        while !bytes.windows(4).any(|part| part == b"\r\n\r\n") {
                            let mut chunk = [0u8; 1024];
                            let read = stream.read(&mut chunk).await.unwrap();
                            if read == 0 { break; }
                            bytes.extend_from_slice(&chunk[..read]);
                            assert!(bytes.len() < 16 * 1024, "bounded fixture request");
                        }
                        let request = String::from_utf8_lossy(&bytes);
                        let path = request.split_whitespace().nth(1).unwrap().to_string();
                        capture.lock().await.push(path.clone());
                        if path == "/disconnect" { continue; }
                        if path == "/stall" {
                            let _ = tokio::time::timeout(std::time::Duration::from_secs(3), stream.read(&mut [0u8; 1])).await;
                            continue;
                        }
                        if path == "/byte-string" {
                            stream.write_all(b"HTTP/1.1 200 OK\r\nX-Obs-Text: \xE9\r\nX-Utf8-Bytes: \xC3\xA9\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
                            continue;
                        }
                        let location = match path.as_str() {
                            "/redirect" => Some(format!("{other}/forbidden")),
                            "/redirect-back" => Some(format!("{other}/back")),
                            "/back" => Some(format!("{other}/ok")),
                            "/local-redirect" => Some(format!("{own}/ok")),
                            _ => None,
                        };
                        let body = if path.ends_with(".vtt") {
                            "WEBVTT\n\n00:00:00.000 --> 00:00:02.000\nNative protected track"
                        } else { "actual network body" };
                        let status = if location.is_some() { "302 Found" } else { "200 OK" };
                        let content_type = if path.ends_with(".vtt") { "text/vtt" } else { "text/plain" };
                        let location = location.map(|url| format!("Location: {url}\r\n")).unwrap_or_default();
                        let cors = if path == "/cors-denied" { "" } else { "Access-Control-Allow-Origin: *\r\n" };
                        stream.write_all(format!("HTTP/1.1 {status}\r\n{location}{cors}Content-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
                    }
                })
            });
        Self { origin, foreign, requests, tasks }
    }

    fn page(&self, stealth: bool, document_url: &str) -> ObscuraJsRuntime {
        let mut runtime = ObscuraJsRuntime::with_base_url(document_url);
        runtime.set_dom(obscura_dom::parse_html("<html><body><video id='media'><track id='track'></video></body></html>"));
        runtime.set_url(document_url);
        let jar = Arc::new(obscura_net::CookieJar::new());
        runtime.set_http_client(Arc::new(obscura_net::ObscuraHttpClient::with_full_options(
            jar.clone(), None, true,
        )));
        #[cfg(feature = "stealth")]
        if stealth {
            runtime.set_stealth_client(Arc::new(obscura_net::StealthHttpClient::with_proxy(jar, None, true)));
        }
        #[cfg(not(feature = "stealth"))]
        assert!(!stealth);
        runtime.run_page_init();
        runtime
    }
}

impl Drop for LocalServers {
    fn drop(&mut self) {
        for task in &self.tasks { task.abort(); }
    }
}

async fn assert_native_same_origin_transport(stealth: bool) {
    let servers = LocalServers::new().await;
    let mut runtime = servers.page(stealth, &format!("{}/document", servers.origin));
    let result = runtime.call_function_on_for_cdp(r#"async function(foreign) {
        const reject = async url => {
            try { await fetch(url, {mode:'same-origin'}); return 'resolved'; }
            catch (error) { return [error.name, error instanceof TypeError]; }
        };
        const local = await fetch('/ok', {mode:'same-origin'});
        const redirected = await fetch('/local-redirect', {mode:'same-origin'});
        return [local.status, await local.text(), redirected.redirected, await redirected.text(),
            await reject(foreign+'/initial'), await reject('/redirect'), await reject('/redirect-back')];
    }"#, None, &[json!({"value":servers.foreign})], true, true).await.unwrap();
    assert!(!result.thrown, "authenticated same-origin fetch rejected: {result:?}");
    assert_eq!(result.value, Some(json!([200,"actual network body",true,"actual network body",
        ["TypeError",true],["TypeError",true],["TypeError",true]])));
    assert!(servers.requests[1].lock().await.is_empty(), "no forbidden initial or redirect request may leave");
    for expected in ["/redirect", "/redirect-back"] {
        assert!(servers.requests[0].lock().await.iter().any(|path| path == expected),
            "same-origin redirect source must actually be requested");
    }

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    runtime.set_intercept_tx(tx);
    runtime.set_intercept_enabled(true);
    let fetch = runtime.call_function_on_for_cdp(r#"async function() {
        try { await fetch('/rewrite', {mode:'same-origin'}); return 'resolved'; }
        catch (error) { return [error.name, error instanceof TypeError]; }
    }"#, None, &[], true, true);
    let resolve = async {
        let request = rx.recv().await.unwrap();
        request.resolver.send(InterceptResolution::Continue {
            url: Some(format!("{}/rewritten", servers.foreign)), method: None, headers: None, body: None,
        }).unwrap();
    };
    let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        tokio::join!(fetch, resolve)
    }).await.expect("intercepted request must settle");
    assert_eq!(result.unwrap().value, Some(json!(["TypeError",true])));
    assert!(servers.requests[1].lock().await.is_empty(), "no rewritten cross-origin request may leave");
    assert!(!servers.requests[0].lock().await.iter().any(|path| path == "/rewrite"));
    runtime.set_intercept_enabled(false);

    let utility = CdpExecutionWorld {
        frame_id: 0,
        world_id: Some(runtime.ensure_isolated_world(0, "universal-fetch", true).unwrap()),
    };
    let source = format!(r#"(async () => {{
        try {{ await fetch({:?}, {{mode:'same-origin'}}); return 'resolved'; }}
        catch (error) {{ return [error.name, error instanceof TypeError]; }}
    }})()"#, format!("{}/universal", servers.foreign));
    let result = runtime.evaluate_for_cdp_in_frame(utility, &source, true, true, 2_000).await.unwrap();
    assert_eq!(result.value, Some(json!(["TypeError",true])));
    assert!(servers.requests[1].lock().await.is_empty(), "universal DOM access does not bypass the native fetch mode gate");

    runtime.evaluate("document.getElementById('track').src='/redirect'; document.getElementById('track').track.mode='hidden'").unwrap();
    runtime.run_event_loop_until_quiescent(2_000, 20).await.unwrap();
    assert_eq!(runtime.evaluate("[document.getElementById('track').readyState,document.getElementById('track').track.cues.length]").unwrap(), json!([3,0]));
    assert!(servers.requests[1].lock().await.is_empty(), "external tracks require native protection before redirects are sent");

    let result = runtime.call_function_on_for_cdp(r#"async function(foreign) {
        const cors = await fetch(foreign+'/cors');
        let denied;
        try { await fetch(foreign+'/cors-denied'); denied='resolved'; }
        catch (error) { denied=[error.name,error instanceof TypeError]; }
        const opaque = await fetch(foreign+'/opaque', {mode:'no-cors'});
        const byteString = await fetch('/byte-string');
        const byteHeaders = [byteString.headers.get('x-obs-text'),byteString.headers.get('x-utf8-bytes')];
        let failed;
        try { await fetch('/disconnect'); failed='resolved'; }
        catch (error) { failed=[error.name,error instanceof TypeError]; }
        const controller = new AbortController(); controller.abort();
        let aborted;
        try { await fetch('/aborted', {signal:controller.signal}); aborted='resolved'; }
        catch (error) { aborted=[error.name,error === controller.signal.reason]; }
        return [cors.status,await cors.text(),denied,byteHeaders,
            [opaque.type,opaque.status,opaque.url,await opaque.text(),Array.from(opaque.headers).length],failed,aborted];
    }"#, None, &[json!({"value":servers.foreign})], true, true).await.unwrap();
    assert!(!result.thrown, "fetch response consumption rejected: {result:?}");
    assert_eq!(result.value, Some(json!([200,"actual network body",["TypeError",true],["é","Ã©"],
        ["opaque",0,"","",0],["TypeError",true],["AbortError",true]])));
    assert!(!servers.requests[0].lock().await.iter().any(|path| path == "/aborted"));

    runtime.execute_script("start-live-abort", r#"
        globalThis.liveAbort = new AbortController(); globalThis.liveOutcome = null;
        fetch('/stall', {signal:liveAbort.signal}).then(() => liveOutcome='resolved',
            error => liveOutcome=[error.name,error === liveAbort.signal.reason]);
    "#).unwrap();
    assert!(runtime.resolve_promises_until(|_| servers.requests[0].try_lock()
        .is_ok_and(|requests| requests.iter().any(|path| path == "/stall")), 2_000).await,
        "abort fixture must admit a real network request before cancellation");
    runtime.execute_script("abort-live-fetch", "liveAbort.abort()").unwrap();
    assert!(runtime.resolve_promises_until(|runtime|
        runtime.evaluate("liveOutcome !== null").unwrap() == json!(true), 2_000).await);
    assert_eq!(runtime.evaluate("liveOutcome").unwrap(), json!(["AbortError",true]));

    // Universal DOM authority must not silently grant private network access.
    let previous_private = std::env::var_os("OBSCURA_ALLOW_PRIVATE_NETWORK");
    std::env::remove_var("OBSCURA_ALLOW_PRIVATE_NETWORK");
    runtime.set_http_client(Arc::new(obscura_net::ObscuraHttpClient::with_full_options(
        Arc::new(obscura_net::CookieJar::new()), None, false,
    )));
    #[cfg(feature = "stealth")]
    if stealth {
        runtime.set_stealth_client(Arc::new(obscura_net::StealthHttpClient::with_proxy(
            Arc::new(obscura_net::CookieJar::new()), None, false,
        )));
    }
    let source = format!(r#"(async () => {{
        try {{ await fetch({:?}); return 'resolved'; }}
        catch (error) {{ return [error.name,error instanceof TypeError]; }}
    }})()"#, format!("{}/private-universal",servers.origin));
    let result = runtime.evaluate_for_cdp_in_frame(utility, &source, true, true, 2_000).await;
    if let Some(previous) = previous_private {
        std::env::set_var("OBSCURA_ALLOW_PRIVATE_NETWORK", previous);
    }
    assert_eq!(result.unwrap().value, Some(json!(["TypeError",true])));
    assert!(!servers.requests[0].lock().await.iter().any(|path| path == "/private-universal"));
}

#[tokio::test(flavor = "current_thread")]
async fn rustls_fetch_enforces_authenticated_same_origin_before_every_send() {
    assert_native_same_origin_transport(false).await;
}

#[cfg(feature = "stealth")]
#[tokio::test(flavor = "current_thread")]
async fn stealth_fetch_enforces_authenticated_same_origin_before_every_send() {
    assert_native_same_origin_transport(true).await;
}

#[tokio::test(flavor = "current_thread")]
async fn data_fetch_decodes_real_bytes_metadata_and_fragments_without_network() {
    let servers = LocalServers::new().await;
    let mut runtime = servers.page(false, &format!("{}/document", servers.origin));
    let result = runtime.call_function_on_for_cdp(r#"async function() {
        const inspect = async (url,init) => {
            const response=await fetch(url,init);
            return [response.status,response.statusText,response.type,response.url,response.redirected,
                response.headers.get('content-type'),Array.from(new Uint8Array(await response.arrayBuffer()))];
        };
        const reject = async url => {
            try { await fetch(url); return 'resolved'; }
            catch (error) { return [error.name,error instanceof TypeError]; }
        };
        return [await inspect('data:text/plain,hello#not-body',{mode:'same-origin'}),
            await inspect('data:,a%20b%23c'),
            await inspect('data:application/octet-stream;base64,AP%2F%2BgA%3D%3D#ignored',{mode:'no-cors'}),
            await inspect('data:;charset=UTF-8,%E4%BD%A0'),
            await inspect('data:text/plain;base64,aGVsbG8'),
            await inspect('data:broken,ok'), await inspect('data:,bad%escape'),
            await reject('data:text/plain;base64,%%%'), await reject('data:missing-comma'),
            await reject('http://['), await reject('file:///forbidden'), await reject('custom:unsupported')];
    }"#, None, &[], true, true).await.unwrap();
    assert_eq!(result.value, Some(json!([
        [200,"OK","basic","data:text/plain,hello",false,"text/plain",[104,101,108,108,111]],
        [200,"OK","basic","data:,a%20b%23c",false,"text/plain;charset=US-ASCII",[97,32,98,35,99]],
        [200,"OK","basic","data:application/octet-stream;base64,AP%2F%2BgA%3D%3D",false,"application/octet-stream",[0,255,254,128]],
        [200,"OK","basic","data:;charset=UTF-8,%E4%BD%A0",false,"text/plain;charset=UTF-8",[228,189,160]],
        [200,"OK","basic","data:text/plain;base64,aGVsbG8",false,"text/plain",[104,101,108,108,111]],
        [200,"OK","basic","data:broken,ok",false,"text/plain;charset=US-ASCII",[111,107]],
        [200,"OK","basic","data:,bad%escape",false,"text/plain;charset=US-ASCII",[98,97,100,37,101,115,99,97,112,101]],
        ["TypeError",true],["TypeError",true],["TypeError",true],["TypeError",true],["TypeError",true]
    ])));
    for requests in &servers.requests { assert!(requests.lock().await.is_empty()); }
}

#[tokio::test(flavor = "current_thread")]
async fn opaque_document_cannot_claim_network_same_origin_but_can_fetch_data() {
    let servers = LocalServers::new().await;
    let mut runtime = servers.page(false, "data:text/html,opaque");
    let result = runtime.call_function_on_for_cdp(r#"async function(target) {
        let network;
        try { await fetch(target,{mode:'same-origin'}); network='resolved'; }
        catch (error) { network=[error.name,error instanceof TypeError]; }
        const data=await fetch('data:text/plain,opaque-data',{mode:'same-origin'});
        return [network,data.status,await data.text()];
    }"#, None, &[json!({"value":format!("{}/opaque-document",servers.origin)})], true, true).await.unwrap();
    assert_eq!(result.value, Some(json!([["TypeError",true],200,"opaque-data"])));
    for requests in &servers.requests { assert!(requests.lock().await.is_empty()); }
}

#[tokio::test(flavor = "current_thread")]
async fn data_fetch_obeys_the_existing_decoded_body_limit() {
    std::env::set_var("OBSCURA_FETCH_MAX_BODY_BYTES", "4");
    let mut runtime = ObscuraJsRuntime::with_base_url("https://limits.example/document");
    runtime.set_dom(obscura_dom::parse_html("<body></body>"));
    runtime.run_page_init();
    let result = runtime.call_function_on_for_cdp(r#"async function() {
        const first=await fetch('data:,1234');
        const reject=async url => {
            try { await fetch(url); return 'resolved'; }
            catch(error) { return [error.name,error instanceof TypeError]; }
        };
        return [await first.text(),await reject('data:,%31%32%33%34%35'),
            await reject('data:;base64,MTIzNDU=')];
    }"#, None, &[], true, true).await.unwrap();
    assert_eq!(result.value, Some(json!(["1234",["TypeError",true],["TypeError",true]])));
}
