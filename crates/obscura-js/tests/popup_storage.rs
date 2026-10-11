use obscura_js::runtime::ObscuraJsRuntime;
use serde_json::json;

fn document(url: &str) -> ObscuraJsRuntime {
    let mut runtime = ObscuraJsRuntime::with_base_url(url);
    runtime.set_dom(obscura_dom::parse_html("<html><body></body></html>"));
    runtime.set_url(url);
    runtime.run_page_init();
    runtime
}

#[tokio::test(flavor = "current_thread")]
async fn related_windows_share_live_local_storage_and_clone_session_once() {
    let mut runtime = document("https://storage.example/start");
    assert_eq!(runtime.evaluate(r#"(() => {
        localStorage.clear();
        sessionStorage.clear();
        localStorage.setItem('shared', 'before');
        sessionStorage.setItem('session', 'creator');
        const child = open('', 'storage-child');
        const initial = [child.localStorage.getItem('shared'), child.sessionStorage.getItem('session')];
        localStorage.shared = 'after';
        child.localStorage.childValue = 42;
        child.sessionStorage.session = 'child';
        sessionStorage.later = 'creator-only';
        const reused = open('', 'storage-child');
        return [initial, child.localStorage.shared, localStorage.childValue,
            sessionStorage.session, child.sessionStorage.session,
            child.sessionStorage.getItem('later'), reused === child,
            reused.sessionStorage.session, child.localStorage !== localStorage,
            child.sessionStorage !== sessionStorage];
    })()"#).unwrap(), json!([["before", "creator"], "after", "42", "creator", "child",
        null, true, "child", true, true]));
}

#[tokio::test(flavor = "current_thread")]
async fn storage_mutations_and_named_properties_observe_the_same_live_area() {
    let mut runtime = document("https://storage.example/start");
    assert_eq!(runtime.evaluate(r#"(() => {
        const child = open('', 'storage-mutations');
        localStorage.clear();
        localStorage.setItem('first', 1);
        child.localStorage.second = null;
        Object.defineProperty(child.localStorage, 'third', { value: true,
            writable: true, enumerable: true, configurable: true });
        localStorage.setItem('first', 2);
        const before = [localStorage.length, localStorage.key(0), localStorage.key(1),
            localStorage.key(2), localStorage.key(3), localStorage.first,
            Object.keys(localStorage), 'second' in localStorage,
            Object.getOwnPropertyDescriptor(localStorage, 'third').value];
        delete child.localStorage.second;
        localStorage.removeItem('missing');
        const removed = [child.localStorage.length, child.localStorage.getItem('second'),
            localStorage.second === undefined, 'second' in localStorage];
        child.localStorage.clear();
        return [before, removed, localStorage.length, localStorage.key(0),
            Object.keys(child.localStorage), Object.prototype.toString.call(localStorage)];
    })()"#).unwrap(), json!([[3, "first", "second", "third", null, "2",
        ["first", "second", "third"], true, "true"], [2, null, true, false],
        0, null, [], "[object Storage]"]));
}

#[tokio::test(flavor = "current_thread")]
async fn noopener_initial_blank_has_no_opener_and_no_tuple_storage_origin() {
    let mut runtime = document("https://storage.example/start");
    assert_eq!(runtime.evaluate(r#"(() => {
        sessionStorage.setItem('private', 'creator');
        return open('', '_blank', 'noopener');
    })()"#).unwrap(), json!(null));
    let created = runtime.take_created_windows();
    assert_eq!(created.len(), 1);
    let mut child = runtime.for_window(created[0]).unwrap();
    assert_eq!(child.evaluate(r#"(() => {
        try { sessionStorage.getItem('private'); return 'allowed'; }
        catch (error) { return [opener, error.name, error instanceof DOMException]; }
    })()"#).unwrap(), json!([null, "SecurityError", true]));
}

#[tokio::test(flavor = "current_thread")]
async fn opaque_documents_cannot_read_or_mutate_storage() {
    let mut runtime = document("data:text/html,<html></html>");
    assert_eq!(runtime.evaluate(r#"(() => {
        const denied = action => { try { action(); return 'allowed'; }
            catch (error) { return [error.name, error instanceof DOMException]; } };
        return [denied(() => localStorage), denied(() => sessionStorage)];
    })()"#).unwrap(), json!([["SecurityError", true], ["SecurityError", true]]));
}

#[tokio::test(flavor = "current_thread")]
async fn rejected_storage_write_leaves_the_shared_area_unchanged() {
    let mut runtime = document("https://storage.example/start");
    assert_eq!(runtime.evaluate(r#"(() => {
        localStorage.setItem('stable', 'kept');
        const child = open('', 'storage-quota');
        let failure;
        try { child.localStorage.setItem('stable', 'x'.repeat(5 * 1024 * 1024)); }
        catch (error) { failure = [error.name, error instanceof child.DOMException, error instanceof DOMException]; }
        return [failure, localStorage.getItem('stable'), child.localStorage.getItem('stable'),
            localStorage.length, child.localStorage.length];
    })()"#).unwrap(), json!([["QuotaExceededError", true, false], "kept", "kept", 1, 1]));
}
