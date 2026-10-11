use obscura_js::frame::FrameRealm;
use obscura_js::runtime::ObscuraJsRuntime;
use serde_json::json;

fn frame_fixture() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        use std::io::{Read as _, Write as _};

        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0u8; 2048];
        let _ = stream.read(&mut request);
        let body = "<!doctype html><html><body><p id='secret'>child secret</p></body></html>";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len(),
        );
        stream.write_all(response.as_bytes()).unwrap();
    });
    format!("http://{address}/frame")
}

fn page(url: &str) -> ObscuraJsRuntime {
    let mut runtime = ObscuraJsRuntime::new();
    runtime.set_dom(obscura_dom::parse_html("<html><body><iframe id='child'></iframe></body></html>"));
    runtime.set_url(url);
    runtime.set_http_client(std::sync::Arc::new(
        obscura_net::ObscuraHttpClient::with_full_options(
            std::sync::Arc::new(obscura_net::CookieJar::new()), None, true,
        ),
    ));
    runtime.run_page_init();
    runtime
}

async fn load_frame(runtime: &mut ObscuraJsRuntime, url: &str) {
    runtime.execute_script("load-frame", &format!(r#"
        globalThis.iframe = document.getElementById('child');
        globalThis.loaded = false;
        iframe.addEventListener('load', () => {{ loaded = true; }}, {{once: true}});
        iframe.src = {};
    "#, serde_json::to_string(url).unwrap())).unwrap();
    runtime.run_event_loop_until_quiescent(2_000, 30).await.unwrap();
    assert_eq!(runtime.evaluate("loaded").unwrap(), json!(true));
}

#[tokio::test(flavor = "current_thread")]
async fn iframe_cross_origin_facade_blocks_dom_storage_and_reflection_escapes() {
    let url = frame_fixture();
    let mut parent = page("https://parent.example/page");
    load_frame(&mut parent, &url).await;
    assert_eq!(parent.evaluate(r#"(() => {
        const child = iframe.contentWindow;
        const denied = fn => { try { fn(); return 'allowed'; } catch (e) { return e.name; } };
        return [
            iframe.contentDocument === null,
            child === iframe.contentWindow,
            child.window === child && child.self === child && child.frames === child,
            child.parent === window && child.top === window,
            typeof child.postMessage,
            denied(() => child.document.body.textContent),
            denied(() => child.localStorage),
            denied(() => child.sessionStorage),
            denied(() => Object.getOwnPropertyDescriptor(child, 'document').value),
            denied(() => Object.getOwnPropertyDescriptor(child, 'localStorage').value),
            Object.getPrototypeOf(child) === null,
            denied(() => child.constructor),
            denied(() => child.__proto__),
            denied(() => child._url),
            denied(() => child._frameId),
            denied(() => child.location.href),
            typeof child.location.replace,
            denied(() => { child.document = document; }),
            denied(() => Object.defineProperty(child, 'document', {value: document})),
            denied(() => { delete child.document; }),
            denied(() => Object.setPrototypeOf(child, {document})),
            Reflect.ownKeys(child).includes('document'),
            Object.getOwnPropertyDescriptor(child, 'window').value === child,
            Object.getOwnPropertyDescriptor(child, 'postMessage').value === child.postMessage,
            iframe._iframeDoc === undefined,
            denied(() => Object.freeze(child)),
        ];
    })()"#).unwrap(), json!([
        true, true, true, true, "function",
        "SecurityError", "SecurityError", "SecurityError", "SecurityError", "SecurityError",
        true, "SecurityError", "SecurityError", "SecurityError", "SecurityError", "SecurityError",
        "function", "SecurityError", "SecurityError", "SecurityError", "SecurityError", false, true, true,
        true, "TypeError",
    ]));
}

#[tokio::test(flavor = "current_thread")]
async fn iframe_same_origin_facade_and_native_realm_remain_accessible() {
    let url = frame_fixture();
    let mut parent = page(&url.replace("/frame", "/parent"));
    assert_eq!(parent.evaluate(r#"(() => {
        const iframe = document.getElementById('child');
        const initial = iframe.contentWindow;
        return [initial.document.URL, initial.document === iframe.contentDocument,
            initial.Array !== Array, initial.parent === window];
    })()"#).unwrap(), json!(["about:blank", true, true, true]));
    load_frame(&mut parent, &url).await;
    assert_eq!(parent.evaluate(r#"(() => {
        const child = iframe.contentWindow;
        child.document.body.textContent = 'updated';
        child.sentinel = 42;
        return [child.document === iframe.contentDocument, child.document.body.textContent,
            child.sentinel, child.localStorage === localStorage,
            child.sessionStorage === sessionStorage, typeof child.Array,
            Object.getOwnPropertyDescriptor(child, 'document').value === child.document];
    })()"#).unwrap(), json!([true, "updated", 42, true, true, "function", true]));

    let pending = parent.take_pending_frames().pop().expect("fetched frame queued");
    let frame = FrameRealm::new(&mut parent, pending.frame_id, pending.parent_frame_id,
        &pending.url, &pending.html).expect("same-origin frame realm");
    assert_eq!(pending.url, url, "src navigation must queue the fetched document, not initial about:blank");
    assert_eq!(parent.live_frame_owner(pending.frame_id).unwrap().0, pending.parent_frame_id);
    assert_eq!(frame.evaluate(&mut parent, "[document.URL, document.getElementById('secret').textContent]").unwrap(),
        json!([url, "child secret"]));
    frame.execute_script(&mut parent, "document.body.textContent = 'native'; globalThis.sentinel = 73;").unwrap();
    assert_eq!(parent.evaluate(r#"(() => {
        const child = iframe.contentWindow;
        return [child === iframe.contentWindow, child.document === iframe.contentDocument,
            child.document.body.textContent, child.sentinel, child.Array !== Array,
            child.window === child && child.self === child && child.frames === child];
    })()"#).unwrap(), json!([true, true, "native", 73, true, true]));
}

#[tokio::test(flavor = "current_thread")]
async fn iframe_native_delivery_cannot_be_forged_or_wrapped_by_author_script() {
    let url = frame_fixture();
    let mut parent = page("https://parent.example/page");
    load_frame(&mut parent, &url).await;
    let pending = parent.take_pending_frames().pop().unwrap();
    let frame = FrameRealm::new(&mut parent, pending.frame_id, pending.parent_frame_id,
        &pending.url, &pending.html).unwrap();
    let attack = r#"
        globalThis.received = [];
        globalThis.wrapped = 0;
        addEventListener('message', event => received.push([
            event.data, event.origin, event.source === (window === parent ? iframe.contentWindow : parent),
            event.isTrusted,
        ]));
        globalThis.hiddenFrameInternals = [
            typeof globalThis.__obscura_deliverMessage,
            typeof globalThis.__obscura_frameObjects,
            typeof globalThis.__obscura_frameWindows,
        ];
        const deliver = globalThis.__obscura_deliverMessage;
        if (typeof deliver === 'function') deliver('{"v":"forged"}', 'https://trusted.example', 0, '*');
        globalThis.__obscura_deliverMessage = (...args) => {
            wrapped++;
            if (deliver) deliver(args[0], 'https://trusted.example', 0, '*');
        };
        globalThis.__obscura_frameObjects = {0: {window: {forged: true}, document}};
        globalThis.__obscura_frameWindows = {0: {forged: true}};
        const dispatch = globalThis.dispatchEvent;
        globalThis.dispatchEvent = event => { wrapped++; return dispatch(event); };
        const Message = globalThis.MessageEvent;
        const synthetic = new Message('message', {
            data: 'forged-message', origin: 'https://attacker.example', source: null,
        });
        synthetic.origin = 'https://trusted.example';
        synthetic.source = window === parent ? iframe.contentWindow : parent;
        synthetic._isTrusted = true;
        synthetic._trusted = true;
        globalThis.syntheticTrust = synthetic.isTrusted;
        dispatch(synthetic);
        globalThis.MessageEvent = class extends Message {
            constructor(...args) { super(...args); wrapped++; this.origin = 'https://trusted.example'; }
        };
    "#;
    parent.execute_script("attack-page-delivery", attack).unwrap();
    frame.execute_script(&mut parent, attack).unwrap();
    assert_eq!(parent.evaluate("hiddenFrameInternals").unwrap(), json!(["undefined", "undefined", "undefined"]));
    assert_eq!(frame.evaluate(&mut parent, "hiddenFrameInternals").unwrap(), json!(["undefined", "undefined", "undefined"]));
    parent.execute_script("send-authenticated", "iframe.contentWindow.postMessage('parent-message', '*');").unwrap();
    let message = parent.take_pending_frame_messages().pop().unwrap();
    frame.deliver_message(&mut parent, &message.data_json, &message.origin,
        message.source_frame_id, &message.target_origin).unwrap();
    assert_eq!(frame.evaluate(&mut parent, "[received, wrapped, syntheticTrust]").unwrap(),
        json!([[["forged-message", "https://trusted.example", true, false],
            ["parent-message", "https://parent.example", true, true]], 0, false]));
    frame.execute_script(&mut parent, "parent.postMessage('child-message', '*');").unwrap();
    let message = parent.take_pending_frame_messages().pop().unwrap();
    parent.deliver_frame_message(0, &message.data_json, &message.origin,
        message.source_frame_id, &message.target_origin).unwrap();
    assert_eq!(parent.evaluate("[received, wrapped, syntheticTrust]").unwrap(),
        json!([[["forged-message", "https://trusted.example", true, false],
            ["child-message", frame.origin(), true, true]], 0, false]));
}

#[tokio::test(flavor = "current_thread")]
async fn iframe_published_native_objects_recheck_origin_and_live_owner() {
    let url = frame_fixture();
    let parent_url = url.replace("/frame", "/parent");
    let mut parent = page(&parent_url);
    load_frame(&mut parent, &url).await;
    let pending = parent.take_pending_frames().pop().unwrap();
    let frame = FrameRealm::new(&mut parent, pending.frame_id, pending.parent_frame_id,
        &pending.url, &pending.html).unwrap();
    frame.execute_script(&mut parent, "globalThis.nativeSecret = 'native';").unwrap();
    assert_eq!(parent.evaluate(r#"(() => {
        globalThis.savedChild = iframe.contentWindow;
        return [savedChild.document.body.textContent, savedChild.nativeSecret,
            typeof globalThis.__obscura_frameObjects, typeof globalThis.__obscura_frameWindows];
    })()"#).unwrap(), json!(["child secret", "native", "undefined", "undefined"]));
    parent.set_url("https://other.example/page");
    assert_eq!(parent.evaluate(r#"(() => {
        const denied = fn => { try { fn(); return 'allowed'; } catch (error) { return error.name; } };
        return [iframe.contentDocument === null, iframe.contentWindow === savedChild,
            denied(() => savedChild.document.body.textContent),
            denied(() => savedChild.nativeSecret),
            denied(() => Object.getOwnPropertyDescriptor(savedChild, 'document').value),
            Object.getPrototypeOf(savedChild) === null, typeof savedChild.postMessage,
            typeof globalThis.__obscura_frameObjects, typeof globalThis.__obscura_frameWindows];
    })()"#).unwrap(), json!([true, true, "SecurityError", "SecurityError", "SecurityError", true,
        "function", "undefined", "undefined"]));
    parent.set_url(&parent_url);
    assert_eq!(parent.evaluate("iframe.contentDocument.body.textContent").unwrap(), json!("child secret"));
    parent.forget_frame_state(pending.frame_id);
    assert_eq!(parent.evaluate(r#"(() => {
        let denied;
        try { savedChild.document; denied = 'allowed'; } catch (error) { denied = error.name; }
        return [iframe.contentDocument === null, denied, savedChild.closed];
    })()"#).unwrap(), json!([true, "SecurityError", true]));
}

#[tokio::test(flavor = "current_thread")]
async fn iframe_post_message_defaults_options_and_native_sender_do_not_overdeliver() {
    let url = frame_fixture();
    let mut parent = page("https://parent.example/page");
    load_frame(&mut parent, &url).await;
    let pending = parent.take_pending_frames().pop().expect("fetched frame queued");
    let frame = FrameRealm::new(&mut parent, pending.frame_id, pending.parent_frame_id,
        &pending.url, &pending.html).expect("cross-origin frame realm");
    frame.execute_script(&mut parent, r#"
        globalThis.received = [];
        addEventListener('message', event => received.push([
            event.data, event.origin, event.source === parent, event.isTrusted,
        ]));
    "#).unwrap();
    parent.execute_script("send-frame-messages", &format!(r#"
        const child = iframe.contentWindow;
        globalThis.__obscura_frameId = 999;
        child.postMessage('omitted');
        child.postMessage('undefined', undefined);
        child.postMessage('null-options', null);
        child.postMessage('default', '/');
        child.postMessage('empty-options', {{}});
        child.postMessage('options-default', {{targetOrigin: undefined}});
        child.postMessage('wildcard', '*');
        child.postMessage('options-wildcard', {{targetOrigin: '*'}});
        child.postMessage('explicit', {});
        child.postMessage('options-explicit', {{targetOrigin: {}}});
    "#, serde_json::to_string(&url).unwrap(), serde_json::to_string(&url).unwrap())).unwrap();
    let messages = parent.take_pending_frame_messages();
    assert_eq!(messages.len(), 10);
    for message in &messages {
        assert_eq!(message.source_frame_id, 0);
        assert_eq!(message.origin, "https://parent.example");
        frame.deliver_message(&mut parent, &message.data_json, &message.origin,
            message.source_frame_id, &message.target_origin).unwrap();
    }
    assert_eq!(frame.evaluate(&mut parent, "received").unwrap(), json!([
        ["wildcard", "https://parent.example", true, true],
        ["options-wildcard", "https://parent.example", true, true],
        ["explicit", "https://parent.example", true, true],
        ["options-explicit", "https://parent.example", true, true],
    ]));
    assert_eq!(parent.evaluate(r#"(() => {
        const child = iframe.contentWindow;
        return ['', 'null', 'not an origin', 'https://[invalid', {targetOrigin: ''},
            {targetOrigin: null}, {targetOrigin: 'not an origin'}].map(origin => {
                try { child.postMessage('secret', origin); return 'allowed'; }
                catch (error) { return error.name; }
            });
    })()"#).unwrap(), json!([
        "SyntaxError", "SyntaxError", "SyntaxError", "SyntaxError", "SyntaxError", "SyntaxError", "SyntaxError",
    ]));
    assert!(parent.take_pending_frame_messages().is_empty());

    frame.execute_script(&mut parent, "parent.postMessage('reply', {targetOrigin: '*'});").unwrap();
    let reply = parent.take_pending_frame_messages().pop().unwrap();
    parent.execute_script("capture-reply", "addEventListener('message', e => { globalThis.reply = [e.data, e.origin, e.source === iframe.contentWindow]; });").unwrap();
    parent.deliver_frame_message(0, &reply.data_json, &reply.origin,
        reply.source_frame_id, &reply.target_origin).unwrap();
    assert_eq!(parent.evaluate("reply").unwrap(), json!(["reply", frame.origin(), true]));
}

#[tokio::test(flavor = "current_thread")]
async fn iframe_initial_blank_inherits_origin_and_sandbox_stays_opaque() {
    let mut parent = page("https://parent.example/page");
    assert_eq!(parent.evaluate(r#"(() => {
        const blank = document.getElementById('child');
        const child = blank.contentWindow;
        child.document.body.textContent = 'inherited';
        const opaque = document.createElement('iframe');
        opaque.setAttribute('sandbox', 'allow-scripts');
        document.body.appendChild(opaque);
        const hidden = opaque.contentWindow;
        let blocked;
        try { hidden.document; blocked = 'allowed'; } catch (error) { blocked = error.name; }
        return [child === blank.contentWindow, child.document === blank.contentDocument,
            child.eval('location.href'), child.document.body.textContent,
            opaque.contentDocument === null, hidden !== null, blocked,
            Object.getPrototypeOf(hidden) === null];
    })()"#).unwrap(), json!([true, true, "about:blank", "inherited", true, true, "SecurityError", true]));
    let pending = parent.take_pending_frames().pop().unwrap();
    let frame = FrameRealm::new(&mut parent, pending.frame_id, pending.parent_frame_id,
        &pending.url, &pending.html).unwrap();
    frame.execute_script(&mut parent, "globalThis.received = []; addEventListener('message', e => received.push([e.data, e.origin, e.source === parent]));").unwrap();
    parent.execute_script("send-default", "document.getElementById('child').contentWindow.postMessage('same-origin');").unwrap();
    let message = parent.take_pending_frame_messages().pop().unwrap();
    frame.deliver_message(&mut parent, &message.data_json, &message.origin,
        message.source_frame_id, &message.target_origin).unwrap();
    assert_eq!(frame.evaluate(&mut parent, "received").unwrap(), json!([["same-origin", "https://parent.example", true]]));
}

#[tokio::test(flavor = "current_thread")]
async fn iframe_fetched_sandbox_cannot_be_deopaqued_by_removing_attribute() {
    let url = frame_fixture();
    let mut parent = page(&url.replace("/frame", "/parent"));
    parent.execute_script("sandbox-frame", "document.getElementById('child').setAttribute('sandbox', 'allow-scripts');").unwrap();
    load_frame(&mut parent, &url).await;
    let pending = parent.take_pending_frames().pop().expect("sandbox frame queued");
    let frame = FrameRealm::new(&mut parent, pending.frame_id, pending.parent_frame_id,
        &pending.url, &pending.html).expect("sandbox frame realm");
    assert_eq!(frame.origin(), "null");
    assert_eq!(parent.evaluate(r#"(() => {
        iframe.removeAttribute('sandbox');
        const child = iframe.contentWindow;
        const denied = fn => { try { fn(); return 'allowed'; } catch (e) { return e.name; } };
        return [iframe.contentDocument === null, denied(() => child.document),
            denied(() => child.localStorage), Object.getPrototypeOf(child) === null];
    })()"#).unwrap(), json!([true, "SecurityError", "SecurityError", true]));
}

#[tokio::test(flavor = "current_thread")]
async fn iframe_retired_facade_cannot_reveal_its_previous_document() {
    let url = frame_fixture();
    let mut parent = page(&url.replace("/frame", "/parent"));
    load_frame(&mut parent, &url).await;
    assert_eq!(parent.evaluate(r#"(() => {
        const old = iframe.contentWindow;
        const before = old.document.body.textContent;
        iframe.remove();
        let after;
        try { old.document; after = 'allowed'; } catch (error) { after = error.name; }
        return [before, after, old.closed, Object.getPrototypeOf(old) === null];
    })()"#).unwrap(), json!(["child secret", "SecurityError", true, true]));
}

#[tokio::test(flavor = "current_thread")]
async fn post_message_self_checks_current_origin_at_delivery() {
    let mut parent = page("https://parent.example/page");
    parent.execute_script("send-self", r#"
        globalThis.received = [];
        addEventListener('message', e => received.push([e.data, e.origin, e.source === window]));
        postMessage('omitted');
        postMessage('explicit', 'https://parent.example');
        postMessage('wildcard', {targetOrigin: '*'});
    "#).unwrap();
    parent.set_url("https://other.example/page");
    parent.run_event_loop_bounded(30).await.unwrap();
    assert_eq!(parent.evaluate("received").unwrap(), json!([["wildcard", "https://parent.example", true]]));
}

#[tokio::test(flavor = "current_thread")]
async fn post_message_distinct_opaque_realms_do_not_share_default_target_origin() {
    let mut parent = page("data:text/html,parent");
    let frame = FrameRealm::new(&mut parent, 1, 0, "data:text/html,child",
        "<html><body></body></html>").unwrap();
    parent.execute_script("capture-opaque", "globalThis.received = []; addEventListener('message', e => received.push([e.data, e.origin]));").unwrap();
    frame.execute_script(&mut parent, r#"
        parent.postMessage('omitted');
        parent.postMessage('default', '/');
        parent.postMessage('explicit-opaque', 'about:blank');
        parent.postMessage('wildcard', '*');
    "#).unwrap();
    for message in parent.take_pending_frame_messages() {
        parent.deliver_frame_message(0, &message.data_json, &message.origin,
            message.source_frame_id, &message.target_origin).unwrap();
    }
    parent.execute_script("opaque-self", "postMessage('self');").unwrap();
    parent.run_event_loop_bounded(30).await.unwrap();
    assert_eq!(parent.evaluate("received").unwrap(), json!([["wildcard", "null"], ["self", "null"]]));
}
