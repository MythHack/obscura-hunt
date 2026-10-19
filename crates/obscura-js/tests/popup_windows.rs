use obscura_js::runtime::ObscuraJsRuntime;
use serde_json::json;

fn document(url: &str) -> ObscuraJsRuntime {
    let mut runtime = ObscuraJsRuntime::with_base_url(url);
    runtime.set_dom(obscura_dom::parse_html("<html><body><p id='creator'>creator</p></body></html>"));
    runtime.set_url(url);
    runtime.run_page_init();
    runtime
}


#[tokio::test(flavor = "current_thread")]
async fn popup_initial_blank_is_synchronous_and_shares_real_same_origin_objects() {
    let mut runtime = document("https://creator.example/path/page");
    assert_eq!(runtime.evaluate(r#"(() => {
        const popup = open('', 'child');
        popup.document.body.innerHTML = '<p id="result">initial</p>';
        popup.callback = value => { document.getElementById('creator').textContent = value; };
        popup.eval('callback("called from popup"); globalThis.popupOnly = 73');
        return [popup !== null, popup === open('', 'child'), popup.document.URL,
            popup.document.baseURI, popup.document.getElementById('result').textContent,
            popup.popupOnly, typeof popup.Array, popup.Array !== Array,
            popup.opener === window, popup.parent === popup, popup.top === popup,
            popup.frameElement === null, document.getElementById('creator').textContent,
            globalThis.popupOnly === undefined];
    })()"#).unwrap(), json!([true, true, "about:blank", "https://creator.example/path/page",
        "initial", 73, "function", true, true, true, true, true, "called from popup", true]));
}

/// `window.open` must hand back the browsing context's native WindowProxy, the
/// same object the popup sees as `window`, not a per-caller wrapper.
#[tokio::test(flavor = "current_thread")]
async fn popup_open_returns_the_native_window_proxy_on_both_sides() {
    let mut runtime = document("https://creator.example/path/page");
    assert_eq!(runtime.evaluate(r#"(() => {
        const first = open('', 'stable');
        first.eval('globalThis.side = (window === opener.open("", "stable"))');
        return [first === open('', 'stable'),
                first.window === first,
                first.eval('window') === first,
                first.document.defaultView === first,
                first.side];
    })()"#).unwrap(), json!([true, true, true, true, true]));
}

#[tokio::test(flavor = "current_thread")]
async fn popup_navigation_is_not_consumed_by_the_opener_frame_queue() {
    let mut runtime = document("https://creator.example/page");
    runtime.execute_script("open-popup", "globalThis.popup = open('', 'navigation-owner')").unwrap();
    let popup_id = runtime.take_created_windows().into_iter().next().unwrap();
    runtime.execute_script("navigate-popup",
        "popup.location.href = 'https://destination.example/next'").unwrap();
    let child_requests: Vec<_> = runtime.take_pending_frame_navigations().into_iter()
        .map(|(frame, request)| (frame, request.url)).collect();
    assert_eq!(child_requests, Vec::<(u32, String)>::new(),
        "a top-level popup is not an iframe of the opener");
    let window_requests: Vec<_> = runtime.take_window_navigations().into_iter()
        .map(|request| (request.id, request.url)).collect();
    assert_eq!(window_requests, vec![(popup_id, "https://destination.example/next".to_string())]);
}

/// Closing retires the browsing context but the retained WindowProxy keeps its
/// browser surface: `closed` becomes true and `window`/`self`/`frames` still
/// identify the same object. A later `open` with the same name is a new
/// browsing context, not the closed one.
#[tokio::test(flavor = "current_thread")]
async fn popup_close_preserves_the_proxy_surface_and_reopens_a_new_context() {
    let mut runtime = document("https://creator.example/path/page");
    assert_eq!(runtime.evaluate(r#"(() => {
        const popup = open('', 'closer');
        const retained = popup;
        popup.close();
        return [retained.closed, retained.window === retained,
                retained.self === retained, retained.frames === retained,
                open('', 'closer') !== retained];
    })()"#).unwrap(), json!([true, true, true, true, true]));
}

/// Chromium oracle: closing retires the browsing context, not the committed
/// same-origin document an author reference still holds. Identity, document
/// state, navigation surface and no-op operations survive a GC pass.
#[tokio::test(flavor = "current_thread")]
async fn popup_closed_reference_keeps_committed_document_across_gc() {
    let mut runtime = document("https://creator.example/path/page");
    assert_eq!(runtime.evaluate(r#"(() => {
        const popup = open('', 'closed-document');
        const doc = popup.document;
        doc.body.textContent = 'original';
        globalThis.retainedPopup = popup;
        globalThis.retainedDocument = doc;
        const before = [popup.document === doc, doc.body.textContent,
            popup.document.defaultView === popup,
            new Map([[popup, doc], [popup.window, popup]]).get(popup.window) === popup];
        popup.close();
        return [...before, popup.closed, popup.window === popup,
            popup.document === doc, doc.body.textContent, popup.length,
            popup.location.href, popup.postMessage('dropped', '*') === undefined,
            popup.focus() === undefined];
    })()"#).unwrap(), json!([true, "original", true, true,
        true, true, true, "original", 0, "about:blank", true, true]));
    runtime.collect_garbage();
    assert_eq!(runtime.evaluate(r#"(() => {
        const popup = retainedPopup;
        return [popup.closed, popup.window === popup, popup.self === popup,
            popup.frames === popup, popup.document === retainedDocument,
            retainedDocument.body.textContent,
            new Map([[popup, 1]]).get(popup.window) === 1,
            open('', 'closed-document') !== popup];
    })()"#).unwrap(), json!([true, true, true, true, true, "original", true, true]));
}

#[tokio::test(flavor = "current_thread")]
async fn popup_post_message_preserves_cycles_and_typed_arrays_without_sharing_mutations() {
    let mut runtime = document("https://creator.example/path/page");
    runtime.execute_script("popup-structured-message", r#"
        globalThis.popup = open('', 'messenger');
        globalThis.received = [];
        addEventListener('message', event => received.push([
            event.data.value, event.data.self === event.data, Array.from(event.data.bytes),
            event.origin, event.source === popup, event.isTrusted,
        ]));
        popup.eval(`
            const payload = { value: 'snapshot', bytes: new Uint8Array([1, 2, 3]) };
            payload.self = payload;
            opener.postMessage(payload, { targetOrigin: 'https://creator.example' });
            payload.value = 'mutated';
            payload.bytes[0] = 9;
        `);
    "#).unwrap();
    assert_eq!(runtime.evaluate("received").unwrap(), json!([]));
    runtime.run_event_loop_until_quiescent(2_000, 30).await.unwrap();
    assert_eq!(runtime.evaluate("received").unwrap(),
        json!([["snapshot", true, [1, 2, 3], "https://creator.example", true, true]]));
}

/// A popup posting to its own window through `eval` must report itself as the
/// message source, with the origin it inherited from its creator.
#[tokio::test(flavor = "current_thread")]
async fn popup_eval_self_post_keeps_the_popup_as_sender() {
    let mut runtime = document("https://creator.example/path/page");
    runtime.execute_script("popup-self-message", r#"
        globalThis.popup = open('', 'self-poster');
        popup.eval(`
            globalThis.selfEvents = [];
            addEventListener('message', event => selfEvents.push([
                event.data, event.origin, event.source === window, event.source === opener]));
            postMessage('popup-self', '*');
        `);
    "#).unwrap();
    runtime.run_event_loop_until_quiescent(2_000, 30).await.unwrap();
    assert_eq!(runtime.evaluate("popup.selfEvents").unwrap(),
        json!([["popup-self", "https://creator.example", true, false]]));
}
