use obscura_js::frame::FrameRealm;
use obscura_js::runtime::{CdpExecutionWorld, ObscuraJsRuntime};
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

/// Loads `url` into `#child` the way the embedding consumer does: the fetch
/// queues the document, the host adopts it as a native realm, runs its
/// document, and then dispatches the element's single `load`. A bare runtime
/// has no Page, so the test performs that handoff explicitly. The returned
/// realm must stay alive for the frame's browsing context to remain live.
async fn load_frame(runtime: &mut ObscuraJsRuntime, url: &str) -> FrameRealm {
    runtime.execute_script("load-frame", &format!(r#"
        globalThis.iframe = document.getElementById('child');
        globalThis.loaded = false;
        iframe.addEventListener('load', () => {{ loaded = true; }}, {{once: true}});
        iframe.src = {};
    "#, serde_json::to_string(url).unwrap())).unwrap();
    runtime.run_event_loop_until_quiescent(2_000, 30).await.unwrap();
    let pending = runtime.take_pending_frames().pop()
        .expect("the fetched frame document was not queued");
    let mut realm = FrameRealm::new(runtime, pending.frame_id, pending.parent_frame_id,
        &pending.url, &pending.html).expect("frame realm");
    if pending.replace_document {
        realm.replace_document(runtime, &pending.url, "GET", "", &pending.html).unwrap();
    }
    realm.dispatch_load_events(runtime).unwrap();
    runtime.execute_script("frame-load", &format!(
        "globalThis.__obscura_frameElements[{}].dispatchEvent(new Event('load'));",
        pending.frame_id,
    )).unwrap();
    assert_eq!(runtime.evaluate("loaded").unwrap(), json!(true));
    realm
}

#[tokio::test(flavor = "current_thread")]
async fn iframe_cross_origin_facade_blocks_dom_storage_and_reflection_escapes() {
    let url = frame_fixture();
    let mut parent = page("https://parent.example/page");
    let _frame = load_frame(&mut parent, &url).await;
    assert_eq!(parent.evaluate(r#"(() => {
        const child = iframe.contentWindow;
        const denied = fn => { try { fn(); return 'allowed'; } catch (e) { return e.name; } };
        const keys = Reflect.ownKeys(child);
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
            keys.includes('window') && keys.includes('then') && keys.includes(Symbol.toStringTag) && !keys.includes('document'),
            Object.getOwnPropertyDescriptor(child, 'window').get.call(child) === child,
            Object.getOwnPropertyDescriptor(child, 'postMessage').value === child.postMessage,
            iframe._iframeDoc === undefined,
            denied(() => Object.freeze(child)),
        ];
    })()"#).unwrap(), json!([
        true, true, true, true, "function",
        "SecurityError", "SecurityError", "SecurityError", "SecurityError", "SecurityError",
        true, "SecurityError", "SecurityError", "SecurityError", "SecurityError", "SecurityError",
        "function", "SecurityError", "SecurityError", "SecurityError", "SecurityError", true, true, true,
        true, "SecurityError",
    ]));
}

/// A document commit inside one browsing context must keep the same native
/// WindowProxy, including its cross-origin surface, while the frame starts
/// answering for the new document.
#[tokio::test(flavor = "current_thread")]
async fn iframe_navigation_reuses_the_native_window_proxy() {
    let url = frame_fixture();
    let mut parent = page("https://parent.example/page");
    let mut frame = load_frame(&mut parent, &url).await;
    assert_eq!(parent.evaluate(r#"(() => {
        globalThis.retainedWindow = iframe.contentWindow;
        const denied = fn => { try { fn(); return 'allowed'; } catch (e) { return e.name; } };
        return [retainedWindow === iframe.contentWindow,
                retainedWindow.window === retainedWindow,
                retainedWindow.parent === window,
                denied(() => retainedWindow.document)];
    })()"#).unwrap(), json!([true, true, true, "SecurityError"]));
    frame.replace_document(&mut parent, "http://127.0.0.1:9/next", "GET", "",
        "<!doctype html><html><head><title>next</title></head><body><p>next</p></body></html>")
        .expect("replace document");
    assert_eq!(frame.evaluate(&mut parent, "document.title").unwrap(), json!("next"));
    assert_eq!(parent.evaluate(r#"(() => {
        const now = iframe.contentWindow;
        const denied = fn => { try { fn(); return 'allowed'; } catch (e) { return e.name; } };
        return [now === retainedWindow, now === globalThis.retainedWindow,
                now.window === now, now.frames === now,
                now.parent === window && now.top === window,
                typeof now.postMessage,
                denied(() => now.document), denied(() => now.location.href),
                denied(() => now._url)];
    })()"#).unwrap(), json!([true, true, true, true, true, "function",
        "SecurityError", "SecurityError", "SecurityError"]));
}

/// An isolated world has its own security token and author-global boundary.
/// It must see its own global for its own document, and only the native cross-origin
/// surface for another origin's window; the author world's global must never
/// be handed to it as a shortcut.
#[tokio::test(flavor = "current_thread")]
async fn isolated_world_keeps_its_own_global_and_native_cross_origin_surface() {
    let url = frame_fixture();
    let mut parent = page("https://parent.example/page");
    let frame = load_frame(&mut parent, &url).await;
    parent.execute_script("author-probe", "globalThis.__authorProbe = 'author';").unwrap();
    let frame_id = frame.frame_id();
    assert_ne!(frame_id, 0);
    let world = parent.ensure_isolated_world(frame_id, "probe", false).unwrap();
    assert_eq!(parent.evaluate_in_cdp_frame(
        CdpExecutionWorld { frame_id, world_id: Some(world) },
        r#"(() => {
            const denied = fn => { try { fn(); return 'allowed'; } catch (e) { return e.name; } };
            return [window.top !== window,
                    window.parent === window.top,
                    typeof window.top.postMessage,
                    window.top.window === window.top,
                    denied(() => window.top.document),
                    denied(() => window.top.location.href)];
        })()"#).unwrap(),
        json!([true, true, "function", true, "SecurityError", "SecurityError"]));
    let root_world = parent.ensure_isolated_world(0, "root-probe", false).unwrap();
    assert_eq!(parent.evaluate_in_cdp_frame(
        CdpExecutionWorld { frame_id: 0, world_id: Some(root_world) },
        r#"(() => [window.top === window, window.parent === window,
                    window.top.__authorProbe === undefined])()"#).unwrap(),
        json!([true, true, true]));
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
    let frame = load_frame(&mut parent, &url).await;
    assert_eq!(parent.evaluate(r#"(() => {
        const child = iframe.contentWindow;
        child.document.body.textContent = 'updated';
        child.sentinel = 42;
        child.localStorage.setItem('cross-window', 'stored-by-child');
        const parentSeesChild = localStorage.getItem('cross-window');
        localStorage.setItem('parent-only', 'stored-by-parent');
        const childSeesParent = child.localStorage.getItem('parent-only');
        child.sessionStorage.setItem('session-cross', 'session-value');
        const parentSeesSession = sessionStorage.getItem('session-cross');
        return [child.document === iframe.contentDocument, child.document.body.textContent,
            child.sentinel, child.localStorage === localStorage,
            child.sessionStorage === sessionStorage, typeof child.Array,
            Object.getOwnPropertyDescriptor(child, 'document').get.call(child) === child.document,
            parentSeesChild, childSeesParent, parentSeesSession];
    })()"#).unwrap(), json!([true, "updated", 42, false, false, "function", true,
        "stored-by-child", "stored-by-parent", "session-value"]));

    assert_eq!(frame.url(), url, "src navigation must queue the fetched document, not initial about:blank");
    assert_eq!(parent.live_frame_owner(frame.frame_id()).unwrap().0, frame.parent_frame_id());
    assert_eq!(frame.evaluate(&mut parent, "[document.URL, document.body.textContent, sentinel]").unwrap(),
        json!([url, "updated", 42]));
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
    let frame = load_frame(&mut parent, &url).await;
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
    parent.run_event_loop_until_quiescent(2_000, 30).await.unwrap();
    assert_eq!(parent.evaluate("[received, wrapped, syntheticTrust]").unwrap(),
        json!([[["forged-message", "https://trusted.example", true, false],
            ["child-message", frame.origin(), true, true]], 0, false]));
}

#[tokio::test(flavor = "current_thread")]
async fn iframe_published_native_objects_stay_private_and_follow_the_live_owner() {
    let url = frame_fixture();
    let parent_url = url.replace("/frame", "/parent");
    let mut parent = page(&parent_url);
    let frame = load_frame(&mut parent, &url).await;
    frame.execute_script(&mut parent, "globalThis.nativeSecret = 'native';").unwrap();
    assert_eq!(parent.evaluate(r#"(() => {
        globalThis.savedChild = iframe.contentWindow;
        return [savedChild.document.body.textContent, savedChild.nativeSecret,
            typeof globalThis.__obscura_frameObjects, typeof globalThis.__obscura_frameWindows];
    })()"#).unwrap(), json!(["child secret", "native", "undefined", "undefined"]));
    // A same-origin published Window is a real native object, not a facade.
    // There is deliberately no in-place URL rewrite here: the browser consumer
    // commits an origin transition by replacing the document realm
    // (`Page::init_js` -> `refresh_window_realm`), exactly like Chrome, so no
    // old reference survives to be checked. Rewriting `set_url` on a live
    // realm has no browser analogue and would only test a host-only state.
    assert_eq!(parent.evaluate("iframe.contentWindow === savedChild").unwrap(), json!(true));
    parent.forget_frame_state(frame.frame_id());
    assert_eq!(parent.evaluate(r#"(() => {
        const denied = fn => { try { fn(); return 'allowed'; } catch (error) { return error.name; } };
        return [iframe.contentDocument === null, iframe.contentWindow === savedChild,
            savedChild.document.body.textContent, savedChild.closed,
            denied(() => iframe.contentWindow.document),
            typeof globalThis.__obscura_frameObjects, typeof globalThis.__obscura_frameWindows];
    })()"#).unwrap(), json!([true, false, "child secret", true, "SecurityError",
        "undefined", "undefined"]));
}

#[tokio::test(flavor = "current_thread")]
async fn iframe_post_message_defaults_options_and_native_sender_do_not_overdeliver() {
    let url = frame_fixture();
    let mut parent = page("https://parent.example/page");
    let frame = load_frame(&mut parent, &url).await;
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

    parent.execute_script("capture-reply", "addEventListener('message', e => { globalThis.reply = [e.data, e.origin, e.source === iframe.contentWindow]; });").unwrap();
    frame.execute_script(&mut parent, "parent.postMessage('reply', {targetOrigin: '*'});").unwrap();
    parent.run_event_loop_until_quiescent(2_000, 30).await.unwrap();
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
    let frame = load_frame(&mut parent, &url).await;
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
async fn iframe_retired_facade_keeps_its_document_but_not_a_new_owner() {
    let url = frame_fixture();
    let mut parent = page(&url.replace("/frame", "/parent"));
    let _frame = load_frame(&mut parent, &url).await;
    assert_eq!(parent.evaluate(r#"(() => {
        const old = iframe.contentWindow;
        const savedDocument = old.document;
        const before = savedDocument.body.textContent;
        iframe.remove();
        // Chrome keeps a removed frame's already-committed same-origin
        // document alive on an object the page still holds. Retirement alone
        // is not a cross-origin boundary.
        const retained = [old.document === savedDocument,
            old.document.body.textContent, old.closed];
        // A new browsing context is a different owner: it gets its own opaque
        // document, and the retained facade must not be retargeted to it.
        const fresh = document.createElement('iframe');
        fresh.setAttribute('sandbox', 'allow-scripts');
        document.body.appendChild(fresh);
        const next = fresh.contentWindow;
        let denied;
        try { next.document; denied = 'allowed'; } catch (error) { denied = error.name; }
        return [...retained, next !== old, fresh.contentDocument === null, denied,
            old.document === savedDocument];
    })()"#).unwrap(), json!([
        true, "child secret", true, true, true, "SecurityError", true,
    ]));
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

/// Serves the child's dynamically created external script.
fn dynamic_script_fixture() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        use std::io::{Read as _, Write as _};

        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0u8; 4096];
        let read = stream.read(&mut request).unwrap_or(0);
        let head = String::from_utf8_lossy(&request[..read]).to_string();
        // Anything but the child's own script must be observable in the
        // outcome, not as a detached-thread panic.
        let body = if head.starts_with("GET /child-script.js ") {
            "globalThis.externalChildScript = 'external-ran';"
        } else {
            "globalThis.externalChildScript = 'wrong-request';"
        };
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/javascript\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len(),
        );
        let _ = stream.write_all(response.as_bytes());
    });
    format!("http://{address}/parent")
}

/// A lazily created about:blank child that appends its own scripts must
/// prepare them against its own document. The node ids of a child DomTree
/// collide with the page's, so an op that reads the root state either drops
/// the script silently (no request, no execution) or claims the page's node.
/// The frame id comes from the child realm's internal bootstrap closure.
#[tokio::test(flavor = "current_thread")]
async fn lazily_created_child_dynamic_scripts_start_in_their_own_document() {
    let url = dynamic_script_fixture();
    let mut parent = page(&url);
    // Same-origin `eval` runs with the page as the entered host context, so
    // the child must not depend on the running context to name its document.
    parent.execute_script("child-dynamic-scripts", r#"
        globalThis.child = document.getElementById('child').contentWindow;
        child.eval(`
            globalThis.inlineChildScript = 'pending';
            const inline = document.createElement('script');
            inline.textContent = "globalThis.inlineChildScript = 'inline-ran';";
            document.head.appendChild(inline);
            const external = document.createElement('script');
            external.src = '/child-script.js';
            document.head.appendChild(external);
        `);
    "#).unwrap();
    parent.run_event_loop_until_quiescent(2_000, 30).await.unwrap();
    assert_eq!(parent.evaluate("child.inlineChildScript").unwrap(), json!("inline-ran"));
    assert_eq!(parent.evaluate("child.externalChildScript").unwrap(), json!("external-ran"));
}

/// The sender is the script that called, even when the code runs through a
/// same-origin `eval` in another realm. A parent calling
/// `child.eval('postMessage(...)')` must deliver with the child as `source`
/// and the child's inherited origin, not the parent.
#[tokio::test(flavor = "current_thread")]
async fn same_origin_child_eval_self_post_keeps_the_child_as_sender() {
    let url = frame_fixture();
    let origin = url.strip_suffix("/frame").unwrap().to_string();
    let mut parent = page(&url.replace("/frame", "/parent"));
    let frame = load_frame(&mut parent, &url).await;
    parent.execute_script("child-eval-self-post", r#"
        globalThis.child = document.getElementById('child').contentWindow;
        child.eval(`
            globalThis.selfEvents = [];
            addEventListener('message', event => selfEvents.push([
                event.data, event.origin, event.source === window, event.source === parent]));
            postMessage('child-self', '*');
        `);
    "#).unwrap();
    for message in parent.take_pending_frame_messages() {
        frame.deliver_message(&mut parent, &message.data_json, &message.origin,
            message.source_frame_id, &message.target_origin).unwrap();
    }
    parent.run_event_loop_bounded(100).await.unwrap();
    assert_eq!(frame.evaluate(&mut parent, "selfEvents").unwrap(),
        json!([["child-self", origin, true, false]]));
}

/// One native WindowProxy per browsing context: a retained reference survives a
/// cross-origin to same-origin commit, and nested frame/function/defaultView
/// identities remain the same objects inside that document.
#[tokio::test(flavor = "current_thread")]
async fn iframe_window_identity_survives_cross_to_same_navigation_and_nests() {
    let url = frame_fixture();
    let mut parent = page("https://parent.example/page");
    let mut frame = load_frame(&mut parent, &url).await;
    assert_eq!(parent.evaluate(
        "globalThis.retainedWindow = iframe.contentWindow, iframe.contentWindow === retainedWindow")
        .unwrap(), json!(true));
    frame.replace_document(&mut parent, "https://parent.example/back", "GET", "",
        "<!doctype html><html><body><p id='back'>back</p></body></html>")
        .expect("replace document");
    assert_eq!(parent.evaluate(r#"(() => {
        const child = iframe.contentWindow;
        child.document.body.insertAdjacentHTML('beforeend', '<iframe id="grand"></iframe>');
        const html = child.document.getElementById('grand');
        const first = html.contentWindow;
        return [child === retainedWindow, child === iframe.contentWindow,
            child.document.body.textContent, child.document.defaultView === child,
            child.window === child, child.focus === child.focus,
            first.parent === child, first === child.frames[0], child.length,
            new Map([[child, 'child'], [first, 'grand']]).get(child.frames[0])];
    })()"#).unwrap(), json!([true, true, "back", true, true, true,
        true, true, 1, "grand"]));
}

/// A CDP universal-access grant permits DOM access to a cross-origin frame's
/// document through the isolated-world adapter, while the target's raw global,
/// its eval, and its author globals stay unreachable.
#[tokio::test(flavor = "current_thread")]
async fn isolated_world_grant_reaches_foreign_dom_but_not_foreign_globals() {
    let url = frame_fixture();
    let mut parent = page("https://parent.example/page");
    let frame = load_frame(&mut parent, &url).await;
    parent.execute_script("author-state", r#"
        document.body.setAttribute('data-secret', 'parent-secret');
        globalThis.__authorGlobal = 'author';
    "#).unwrap();
    let frame_id = frame.frame_id();
    let world = parent.ensure_isolated_world(frame_id, "granted", true).unwrap();
    assert_eq!(parent.evaluate_in_cdp_frame(
        CdpExecutionWorld { frame_id, world_id: Some(world) },
        r#"(() => {
            const denied = fn => { try { fn(); return 'allowed'; } catch (e) { return e.name; } };
            const top = window.top;
            const doc = top.document;
            return [top !== window, doc.body.getAttribute('data-secret'),
                doc.defaultView === top, top.window === top,
                denied(() => top.eval('1')),
                denied(() => top.__authorGlobal),
                denied(() => { top.__authorGlobal = 'mutated'; })];
        })()"#).unwrap(),
        json!([true, "parent-secret", true, true, "SecurityError", "SecurityError", "SecurityError"]));
}
