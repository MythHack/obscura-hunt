#![cfg(feature = "render")]

use base64::{Engine as _, engine::general_purpose::STANDARD};
use obscura_js::runtime::ObscuraJsRuntime;
use serde_json::json;

const FONT: &[u8] = include_bytes!("../../obscura-render/assets/liberation-serif.ttf");

fn document() -> ObscuraJsRuntime {
    let mut runtime = ObscuraJsRuntime::with_base_url("https://font.example/page/index.html");
    runtime.set_dom(obscura_dom::parse_html("<html><body></body></html>"));
    runtime.set_url("https://font.example/page/index.html");
    runtime.run_page_init();
    runtime
}

#[tokio::test(flavor = "current_thread")]
async fn invalid_url_and_binary_fonts_reject_without_claiming_loaded() {
    let mut runtime = document();
    runtime.execute_script("invalid-fonts", r#"
        globalThis.result = [];
        const url = new FontFace('InvalidUrl', 'url(data:font/ttf;base64,AA==)');
        const binary = new FontFace('InvalidBinary', new Uint8Array([0, 1, 0, 0]));
        Promise.all([url, binary].map(face => face.load().then(
            () => ['unexpected success', face.status],
            error => [face.status, error.name, error instanceof DOMException,
                face.loaded === face.load()]
        ))).then(value => result = value);
    "#).unwrap();
    runtime.run_event_loop_bounded(100).await.unwrap();
    assert_eq!(runtime.evaluate("result").unwrap(), json!([
        ["error", "NetworkError", true, true], ["error", "SyntaxError", true, true]
    ]));
}

#[tokio::test(flavor = "current_thread")]
async fn bundled_binary_and_unattached_url_fonts_decode_and_shape_after_add() {
    let mut runtime = document();
    runtime.seed_render_resource("https://font.example/fonts/fixture.ttf".into(), Some(FONT.to_vec()));
    runtime.execute_script("valid-fonts", &format!(r#"
        globalThis.result = 'pending';
        const bytes = Uint8Array.from(atob('{}'), c => c.charCodeAt(0));
        const binary = new FontFace('BinaryFixture', bytes);
        const url = new FontFace('UrlFixture', "url('../fonts/fixture.ttf')");
        const identity = binary.loaded === binary.load();
        Promise.all([binary.loaded, url.load()]).then(faces => {{
            const sample = document.createElement('span');
            sample.style.cssText = 'display:inline-block;width:max-content;font-size:40px;white-space:nowrap';
            sample.textContent = 'WWWWiiii';
            document.body.append(sample);
            const fallback = sample.getBoundingClientRect().width;
            document.fonts.add(binary);
            document.fonts.add(url);
            sample.style.fontFamily = 'BinaryFixture';
            const binaryWidth = sample.getBoundingClientRect().width;
            sample.style.fontFamily = 'UrlFixture';
            const urlWidth = sample.getBoundingClientRect().width;
            result = [faces[0] === binary, faces[1] === url, binary.status, url.status,
                identity, binaryWidth !== fallback, binaryWidth === urlWidth,
                document.fonts.check('40px UrlFixture')];
        }});
    "#, STANDARD.encode(FONT))).unwrap();
    runtime.run_event_loop_bounded(100).await.unwrap();
    assert_eq!(runtime.evaluate("result").unwrap(), json!([
        true, true, "loaded", "loaded", true, true, true, true
    ]));
}

#[tokio::test(flavor = "current_thread")]
async fn font_face_load_updates_status_set_readiness_and_matching() {
    let mut runtime = document();
    runtime.seed_render_resource("https://font.example/fonts/good.ttf".into(), Some(FONT.to_vec()));
    runtime.execute_script("font-set-outcomes", r#"
        globalThis.events = [];
        globalThis.result = 'pending';
        const good = new FontFace('Lifecycle', "url('../fonts/good.ttf')", {weight:'700'});
        const bad = new FontFace('Broken', 'url(data:font/ttf;base64,AA==)');
        for (const name of ['loading', 'loadingdone', 'loadingerror']) {
            document.fonts.addEventListener(name, event => events.push(
                [name, event.fontfaces.map(face => face.family)]));
        }
        document.fonts.add(good).add(bad);
        const before = [good.status, document.fonts.status,
            document.fonts.check('700 16px Lifecycle')];
        const goodLoad = document.fonts.load('700 16px Lifecycle');
        const badLoad = document.fonts.load('16px Broken').catch(error => error.name);
        const ready = document.fonts.ready;
        Promise.all([goodLoad, badLoad, ready]).then(([faces, error, set]) => {
            result = [before, faces.length, faces[0] === good, good.status, bad.status,
                error, set === document.fonts, document.fonts.status,
                document.fonts.check('700 16px Lifecycle'), document.fonts.check('16px Broken')];
        });
    "#).unwrap();
    runtime.run_event_loop_bounded(100).await.unwrap();
    assert_eq!(runtime.evaluate("result").unwrap(), json!([
        ["unloaded", "loaded", false], 1, true, "loaded", "error", "NetworkError",
        true, "loaded", true, false
    ]));
    assert_eq!(runtime.evaluate("events").unwrap(), json!([
        ["loading", []], ["loadingdone", ["Lifecycle"]], ["loadingerror", ["Broken"]]
    ]));
}

#[tokio::test(flavor = "current_thread")]
async fn malformed_preferred_source_tries_real_font_and_unicode_matching_skips_unused_faces() {
    let mut runtime = document();
    runtime.seed_render_resource("https://font.example/fonts/good.ttf".into(), Some(FONT.to_vec()));
    runtime.execute_script("font-fallback", r#"
        globalThis.result = 'pending';
        const used = new FontFace('Subset', "url(data:font/ttf;base64,AA==), url('../fonts/good.ttf')",
            {unicodeRange:'U+0400-04FF'});
        const unused = new FontFace('Subset', 'url(data:font/ttf;base64,AA==)', {unicodeRange:'U+20-7E'});
        document.fonts.add(used).add(unused);
        document.fonts.load('16px Subset', 'Ж').then(faces => {
            result = [faces.length, faces[0] === used, used.status, unused.status,
                document.fonts.check('16px Subset', 'Ж'), document.fonts.check('16px Subset', 'A')];
        });
    "#).unwrap();
    runtime.run_event_loop_bounded(100).await.unwrap();
    assert_eq!(runtime.evaluate("result").unwrap(), json!([1, true, "loaded", "unloaded", true, false]));
}

#[tokio::test(flavor = "current_thread")]
async fn unattached_font_from_replaced_document_rejects_with_security_error() {
    let mut runtime = document();
    runtime.seed_render_resource("https://font.example/fonts/good.ttf".into(), Some(FONT.to_vec()));
    runtime.execute_script("old-font-owner", r#"
        globalThis.oldFace = new FontFace('OldDocument', "url('../fonts/good.ttf')");
        const observer = open('', 'font-observer');
        observer.oldFace = oldFace;
        observer.FontError = DOMException;
    "#).unwrap();
    let observer_id = runtime.take_created_windows().pop().unwrap();
    let mut observer = runtime.for_window(observer_id).unwrap();
    runtime.set_dom(obscura_dom::parse_html("<html><body>replacement</body></html>"));
    // Production navigation replaces the document realm (Page::init_js calls
    // refresh_window_realm) before running page init. set_dom alone retains the
    // previous JS realm, so model the same public contract here.
    runtime.set_url("https://font.example/page/index.html");
    runtime.seed_render_resource("https://font.example/fonts/good.ttf".into(), Some(FONT.to_vec()));
    runtime.refresh_window_realm().unwrap();
    runtime.run_page_init();
    assert_eq!(runtime.evaluate_with_timeout("typeof oldFace", std::time::Duration::from_secs(1)).unwrap(),
        json!("undefined"));
    // The replacement document is a live, current world of its own: a fresh
    // FontFace from that document still reaches the transport and loads.
    runtime.execute_script("replacement-font-owner", r#"
        globalThis.freshFontResult = 'pending';
        const fresh = new FontFace('ReplacementDocument', "url('../fonts/good.ttf')");
        fresh.load().then(face => {
            document.fonts.add(face);
            freshFontResult = [face === fresh, face.status,
                document.fonts.check('16px ReplacementDocument')];
        }, error => freshFontResult = error.name);
    "#).unwrap();
    runtime.run_event_loop_bounded(100).await.unwrap();
    assert_eq!(
        runtime.evaluate("freshFontResult").unwrap(),
        json!([true, "loaded", true])
    );
    observer.execute_script("retired-font-owner", r#"
        globalThis.result = 'pending';
        oldFace.load().then(() => result = 'unexpected success', error => {
            result = [oldFace.status, error.name, error instanceof FontError,
                oldFace.loaded === oldFace.load()];
        });
    "#).unwrap();
    observer.run_event_loop_bounded(100).await.unwrap();
    assert_eq!(observer.evaluate("result").unwrap(), json!([
        "error", "SecurityError", true, true
    ]));
}

#[tokio::test(flavor = "current_thread")]
async fn disposing_font_execution_world_cancels_unattached_request_without_retiring_document() {
    use obscura_js::runtime::CdpExecutionWorld;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (closed_tx, closed_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        while !request.windows(4).any(|part| part == b"\r\n\r\n") {
            let mut chunk = [0u8; 1024];
            let count = stream.read(&mut chunk).await.unwrap();
            assert_ne!(count, 0, "font request must reach the local transport");
            request.extend_from_slice(&chunk[..count]);
            assert!(request.len() < 16 * 1024, "bounded fixture request");
        }
        assert!(String::from_utf8_lossy(&request).starts_with("GET /pending.ttf "));
        started_tx.send(()).unwrap();
        // No response is sent until the original request has been cancelled.
        // Waiting for transport EOF distinguishes per-world cancellation from
        // merely rejecting a stale JS result after a response has arrived.
        let count = stream.read(&mut [0u8; 1]).await.unwrap();
        closed_tx.send(count).unwrap();
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0u8; 2048];
        assert_ne!(stream.read(&mut request).await.unwrap(), 0);
        stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: font/ttf\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", FONT.len()).as_bytes()).await.unwrap();
        stream.write_all(FONT).await.unwrap();
    });

    let mut runtime = ObscuraJsRuntime::with_base_url(&format!("{origin}/page"));
    runtime.set_dom(obscura_dom::parse_html("<html><body></body></html>"));
    runtime.set_url(&format!("{origin}/page"));
    runtime.set_http_client(Arc::new(obscura_net::ObscuraHttpClient::with_full_options(
        Arc::new(obscura_net::CookieJar::new()), None, true,
    )));
    runtime.run_page_init();
    let utility = CdpExecutionWorld { frame_id: 0,
        world_id: Some(runtime.ensure_isolated_world(0, "font-loader", false).unwrap()) };
    runtime.evaluate_in_cdp_frame(utility, r#"
        globalThis.pendingFace = new FontFace('UtilityFont', 'url(/pending.ttf)');
        pendingFace.load().catch(() => {});
    "#).unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        tokio::select! {
            started = started_rx => started.unwrap(),
            event_loop = runtime.run_event_loop_bounded(2_000) => {
                event_loop.unwrap();
                panic!("font transport did not start before the event-loop deadline");
            }
        }
    }).await.unwrap();
    assert_eq!(runtime.evaluate_in_cdp_frame(utility, "[pendingFace.status, document.fonts.size]").unwrap(),
        json!(["loading", 0]));
    let generation = runtime.window_document_generation();
    runtime.dispose_isolated_world(utility.world_id.unwrap()).unwrap();
    runtime.run_event_loop_bounded(100).await.unwrap();
    assert_eq!(tokio::time::timeout(Duration::from_secs(2), closed_rx).await.unwrap().unwrap(), 0);
    assert_eq!(runtime.window_document_generation(), generation);
    runtime.execute_script("canonical-font-after-world-disposal", r#"
        globalThis.result = 'pending';
        const face = new FontFace('CanonicalFont', 'url(/good.ttf)');
        face.load().then(value => result = [value === face, face.status, document.fonts.size],
            error => result = error.name);
    "#).unwrap();
    runtime.run_event_loop_until_quiescent(2_000, 30).await.unwrap();
    assert_eq!(runtime.evaluate("result").unwrap(), json!([true, "loaded", 0]));
    server.await.unwrap();
}
