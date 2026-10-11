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
