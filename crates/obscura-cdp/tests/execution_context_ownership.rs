use obscura_cdp::dispatch::{dispatch, CdpContext};
use obscura_cdp::types::CdpRequest;
use serde_json::{json, Value};

async fn cdp(
    ctx: &mut CdpContext,
    id: u64,
    method: &str,
    params: Value,
    session_id: Option<&str>,
) -> obscura_cdp::types::CdpResponse {
    dispatch(&CdpRequest {
        id,
        method: method.to_string(),
        params,
        session_id: session_id.map(str::to_string),
    }, ctx).await
}

async fn create_and_attach(ctx: &mut CdpContext, url: &str, id: u64) -> (String, String) {
    let created = cdp(ctx, id, "Target.createTarget", json!({"url": url}), None).await;
    assert!(created.error.is_none(), "createTarget failed: {:?}", created.error);
    let target = created.result.unwrap()["targetId"].as_str().unwrap().to_string();
    let attached = cdp(
        ctx, id + 1, "Target.attachToTarget",
        json!({"targetId": target, "flatten": true}), None,
    ).await;
    let session = attached.result.unwrap()["sessionId"].as_str().unwrap().to_string();
    (target, session)
}

fn latest_default_context(ctx: &CdpContext, session: &str) -> (i64, String, String) {
    let context = &ctx.pending_events.iter().rev().find(|event| {
        event.method == "Runtime.executionContextCreated"
            && event.session_id.as_deref() == Some(session)
            && event.params["context"]["auxData"]["isDefault"] == true
    }).expect("missing default context").params["context"];
    (
        context["id"].as_i64().unwrap(),
        context["uniqueId"].as_str().unwrap().to_string(),
        context["origin"].as_str().unwrap().to_string(),
    )
}

#[tokio::test(flavor = "current_thread")]
async fn nonblank_create_target_exposes_only_the_committed_document_context() {
    let mut ctx = CdpContext::new();
    let (_, session) = create_and_attach(
        &mut ctx, "data:text/html,<title>committed</title>", 1,
    ).await;
    ctx.pending_events.clear();
    cdp(&mut ctx, 3, "Runtime.enable", json!({}), Some(&session)).await;

    let (id, _, origin) = latest_default_context(&ctx, &session);
    assert_eq!(ctx.pending_events.iter().filter(|event| {
        event.method == "Runtime.executionContextCreated"
            && event.session_id.as_deref() == Some(session.as_str())
            && event.params["context"]["auxData"]["isDefault"] == true
    }).count(), 1);
    assert_eq!(world_value(&mut ctx, &session, Some(id), "document.title").await, "committed");
    assert!(origin.starts_with("data:text/html,"));
    assert!(!ctx.pending_events.iter().any(|event| {
        event.method == "Runtime.executionContextCreated"
            && event.params["context"]["origin"] == "about:blank"
    }));
}

#[tokio::test(flavor = "current_thread")]
async fn default_context_identity_is_page_owned_for_id_and_unique_id() {
    let mut ctx = CdpContext::new();
    let (_, first) = create_and_attach(&mut ctx, "about:blank", 1).await;
    let (_, second) = create_and_attach(&mut ctx, "about:blank", 10).await;
    cdp(&mut ctx, 20, "Runtime.enable", json!({}), Some(&first)).await;
    let (id, unique_id, _) = latest_default_context(&ctx, &first);

    let foreign_id = cdp(
        &mut ctx, 21, "Runtime.evaluate",
        json!({"expression": "1", "contextId": id}), Some(&second),
    ).await;
    assert!(foreign_id.error.unwrap().message.contains("Cannot find context"));
    let foreign_unique = cdp(
        &mut ctx, 22, "Runtime.evaluate",
        json!({"expression": "1", "uniqueContextId": unique_id.clone()}), Some(&second),
    ).await;
    assert!(foreign_unique.error.unwrap().message.contains("Cannot find context"));

    let foreign_call = cdp(
        &mut ctx, 23, "Runtime.callFunctionOn",
        json!({
            "functionDeclaration": "() => 1",
            "executionContextId": id,
            "returnByValue": true,
        }),
        Some(&second),
    ).await;
    assert!(foreign_call.error.unwrap().message.contains("Cannot find context"));

    cdp(
        &mut ctx, 24, "Page.navigate",
        json!({"url": "data:text/html,<p>replacement</p>", "waitUntil": "load"}),
        Some(&first),
    ).await;
    for (request_id, params) in [
        (25, json!({"expression": "1", "uniqueContextId": unique_id})),
        (26, json!({
            "functionDeclaration": "() => 1",
            "executionContextId": id,
            "returnByValue": true,
        })),
    ] {
        let method = if request_id == 25 { "Runtime.evaluate" } else { "Runtime.callFunctionOn" };
        let stale = cdp(&mut ctx, request_id, method, params, Some(&first)).await;
        assert!(stale.error.unwrap().message.contains("Cannot find context"));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn navigating_one_page_preserves_the_other_pages_isolated_context() {
    let mut ctx = CdpContext::new();
    let (_, first) = create_and_attach(&mut ctx, "about:blank", 1).await;
    let (_, second) = create_and_attach(&mut ctx, "about:blank", 10).await;
    for (id, session) in [(20, &first), (21, &second)] {
        cdp(&mut ctx, id, "Runtime.enable", json!({}), Some(session)).await;
    }
    let first_isolated = cdp(
        &mut ctx, 30, "Page.createIsolatedWorld",
        json!({"worldName": "first-utility"}), Some(&first),
    ).await.result.unwrap()["executionContextId"].as_i64().unwrap();
    let second_isolated = cdp(
        &mut ctx, 31, "Page.createIsolatedWorld",
        json!({"worldName": "second-utility"}), Some(&second),
    ).await.result.unwrap()["executionContextId"].as_i64().unwrap();

    cdp(
        &mut ctx, 32, "Page.navigate",
        json!({"url": "data:text/html,<p>first replacement</p>", "waitUntil": "load"}),
        Some(&first),
    ).await;

    let second_still_routes = cdp(
        &mut ctx, 33, "Runtime.evaluate",
        json!({"expression": "1", "contextId": second_isolated}), Some(&second),
    ).await;
    assert!(second_still_routes.error.is_none());
    let first_is_stale = cdp(
        &mut ctx, 34, "Runtime.evaluate",
        json!({"expression": "1", "contextId": first_isolated}), Some(&first),
    ).await;
    assert!(first_is_stale.error.unwrap().message.contains("Cannot find context"));
}

#[tokio::test(flavor = "current_thread")]
async fn attached_isolated_contexts_have_private_globals_and_shared_dom() {
    let mut ctx = CdpContext::new();
    let (_, session) = create_and_attach(&mut ctx, "data:text/html,<style>#probe{color:rgb(1,2,3)}</style><p id=probe>default</p><input id=focus value=preset>", 1).await;
    cdp(&mut ctx, 3, "Runtime.enable", json!({}), Some(&session)).await;
    cdp(&mut ctx, 4, "Runtime.evaluate", json!({
        "expression": "globalThis.realmProbe='default';document.getElementById('probe').expando=1;Element.prototype.authorOnly=1;let input=document.getElementById('focus');input.value='preserved';input.focus()",
    }), Some(&session)).await;
    let isolated = cdp(&mut ctx, 5, "Page.createIsolatedWorld", json!({"worldName":"utility"}), Some(&session))
        .await.result.unwrap()["executionContextId"].as_i64().unwrap();
    let private = cdp(&mut ctx, 6, "Runtime.evaluate", json!({
        "expression": "(()=>{let p=document.getElementById('probe'),input=document.getElementById('focus');let before=[typeof realmProbe,typeof p.expando,typeof Element.prototype.authorOnly];globalThis.realmProbe='utility';p.textContent='shared';p.expando=2;return [before,input.value,document.activeElement===input,p.parentNode===document.body,p.isConnected]})()",
        "contextId": isolated, "returnByValue": true,
    }), Some(&session)).await;
    assert!(private.error.is_none(), "{:?}", private.error);
    assert_eq!(private.result.unwrap()["result"]["value"], json!([["undefined","undefined","undefined"],"preserved",true,true,true]));
    #[cfg(feature = "render")]
    assert_eq!(world_value(&mut ctx, &session, Some(isolated), "getComputedStyle(document.getElementById('probe')).color").await, "rgb(1, 2, 3)");
    let default = cdp(&mut ctx, 7, "Runtime.evaluate", json!({
        "expression":"[realmProbe,document.getElementById('probe').textContent,document.getElementById('probe').expando]","returnByValue":true,
    }), Some(&session)).await.result.unwrap();
    assert_eq!(default["result"]["value"], json!(["default","shared",1]));
    let repeat = cdp(&mut ctx, 8, "Page.createIsolatedWorld", json!({"worldName":"utility"}), Some(&session))
        .await.result.unwrap()["executionContextId"].as_i64().unwrap();
    assert_eq!(repeat, isolated);
    let other = cdp(&mut ctx, 9, "Page.createIsolatedWorld", json!({"worldName":"other"}), Some(&session))
        .await.result.unwrap()["executionContextId"].as_i64().unwrap();
    let value = cdp(&mut ctx, 10, "Runtime.evaluate", json!({
        "expression":"typeof realmProbe","contextId":other,"returnByValue":true,
    }), Some(&session)).await.result.unwrap();
    assert_eq!(value["result"]["value"], "undefined");
}

#[tokio::test(flavor = "current_thread")]
async fn navigation_context_events_preserve_order_for_every_runtime_attachment() {
    let mut ctx = CdpContext::new();
    let (target, first) = create_and_attach(&mut ctx, "about:blank", 1).await;
    let attached = cdp(
        &mut ctx, 3, "Target.attachToTarget",
        json!({"targetId": target, "flatten": true}), None,
    ).await;
    let second = attached.result.unwrap()["sessionId"].as_str().unwrap().to_string();
    for (id, session) in [(10, &first), (11, &second)] {
        cdp(&mut ctx, id, "Runtime.enable", json!({}), Some(session)).await;
        cdp(&mut ctx, id + 100, "Page.enable", json!({}), Some(session)).await;
        cdp(&mut ctx, id + 200, "Page.setLifecycleEventsEnabled", json!({"enabled":true}), Some(session)).await;
    }
    cdp(
        &mut ctx, 12, "Page.createIsolatedWorld",
        json!({"worldName": "utility"}), Some(&first),
    ).await;
    ctx.pending_events.clear();

    cdp(
        &mut ctx, 13, "Page.navigate",
        json!({"url": "data:text/html,<p>replacement</p>", "waitUntil": "load"}),
        Some(&first),
    ).await;

    let first_methods = ctx.pending_events.iter().filter(|event| {
        event.session_id.as_deref() == Some(first.as_str())
            && (event.method == "Page.lifecycleEvent"
                || event.method == "Page.frameNavigated"
                || event.method.starts_with("Runtime.executionContext"))
    }).map(|event| {
        if event.method == "Page.lifecycleEvent" {
            format!("{}:{}", event.method, event.params["name"].as_str().unwrap_or(""))
        } else {
            event.method.clone()
        }
    }).collect::<Vec<_>>();
    assert_eq!(&first_methods[..4], [
        "Page.lifecycleEvent:init",
        "Runtime.executionContextsCleared",
        "Page.frameNavigated",
        "Runtime.executionContextCreated",
    ]);
    assert_eq!(first_methods.iter().filter(|method| {
        method.as_str() == "Runtime.executionContextCreated"
    }).count(), 2);

    let second_methods = ctx.pending_events.iter().filter(|event| {
        event.session_id.as_deref() == Some(second.as_str())
            && event.method.starts_with("Runtime.executionContext")
    }).map(|event| event.method.as_str()).collect::<Vec<_>>();
    assert_eq!(second_methods, [
        "Runtime.executionContextsCleared",
        "Runtime.executionContextCreated",
        "Runtime.executionContextCreated",
    ]);
}

#[tokio::test(flavor = "current_thread")]
async fn runtime_and_binding_events_use_the_owning_pages_default_context() {
    let mut ctx = CdpContext::new();
    let (_, first) = create_and_attach(&mut ctx, "about:blank", 1).await;
    let (_, second) = create_and_attach(&mut ctx, "about:blank", 10).await;
    for (id, session) in [(20, &first), (21, &second)] {
        cdp(&mut ctx, id, "Runtime.enable", json!({}), Some(session)).await;
    }
    let (first_context, _, _) = latest_default_context(&ctx, &first);
    cdp(
        &mut ctx,
        22,
        "Runtime.addBinding",
        json!({"name": "probe"}),
        Some(&second),
    ).await;
    ctx.pending_events.clear();

    cdp(
        &mut ctx,
        23,
        "Page.navigate",
        json!({
            "url": "data:text/html,<script>console.log('owned');probe('bound')</script>",
            "waitUntil": "load",
        }),
        Some(&second),
    ).await;
    let (second_context, _, _) = latest_default_context(&ctx, &second);
    assert_ne!(first_context, second_context);

    let console = ctx.pending_events.iter().find(|event| {
        event.method == "Runtime.consoleAPICalled"
            && event.session_id.as_deref() == Some(second.as_str())
    }).expect("missing console event");
    assert_eq!(console.params["executionContextId"], second_context);
    let binding = ctx.pending_events.iter().find(|event| {
        event.method == "Runtime.bindingCalled"
            && event.session_id.as_deref() == Some(second.as_str())
    }).expect("missing binding event");
    assert_eq!(binding.params["executionContextId"], second_context);
}

async fn world_value(ctx: &mut CdpContext, session: &str, world: Option<i64>, expression: &str) -> Value {
    let mut params = json!({"expression": expression, "returnByValue": true});
    if let Some(world) = world { params["contextId"] = json!(world); }
    let response = cdp(ctx, 100, "Runtime.evaluate", params, Some(session)).await;
    assert!(response.error.is_none(), "{:?}", response.error);
    let result = response.result.unwrap();
    assert!(result.get("exceptionDetails").is_none(), "{result}");
    result["result"]["value"].clone()
}

#[tokio::test(flavor = "current_thread")]
async fn isolated_input_listeners_cancel_one_shared_form_activation() {
    let mut ctx = CdpContext::new();
    let (_, session) = create_and_attach(&mut ctx, "data:text/html,<style>body{margin:0}input{position:absolute;left:20px;top:20px;width:40px;height:40px;margin:0;appearance:none}</style><form id=f><input id=check type=checkbox><textarea id=text></textarea></form>", 1).await;
    let world = cdp(&mut ctx, 3, "Page.createIsolatedWorld", json!({"worldName":"utility"}), Some(&session))
        .await.result.unwrap()["executionContextId"].as_i64().unwrap();
    assert_eq!(world_value(&mut ctx, &session, None, "(()=>{globalThis.clicks=0;globalThis.changes=0;globalThis.saved=null;let c=document.getElementById('check');c.addEventListener('click',e=>{clicks++;saved=e});c.addEventListener('change',()=>changes++);return true})()").await, true);
    assert_eq!(world_value(&mut ctx, &session, Some(world), "(()=>{globalThis.clicks=0;globalThis.cancel=true;let c=document.getElementById('check');c.addEventListener('click',e=>{clicks++;if(cancel)e.preventDefault()});document.getElementById('text').value='shared value';return [c.form===document.getElementById('f'),typeof changes]})()").await, json!([true,"undefined"]));
    // This fixture's stylesheet needs rendering for coordinate hit testing.
    #[cfg(feature = "render")]
    for event in ["mousePressed", "mouseReleased"] {
        let response = cdp(&mut ctx, 4, "Input.dispatchMouseEvent", json!({"type":event,"x":30,"y":30,"button":"left","clickCount":1}), Some(&session)).await;
        assert!(response.error.is_none(), "{:?}", response.error);
    }
    #[cfg(not(feature = "render"))]
    world_value(&mut ctx, &session, None, "document.getElementById('check').click()").await;
    assert_eq!(world_value(&mut ctx, &session, None, "[clicks,changes,document.getElementById('check').checked,document.getElementById('text').value,saved && saved.defaultPrevented]").await, json!([1,0,false,"shared value",true]));
    assert_eq!(world_value(&mut ctx, &session, Some(world), "[clicks,document.getElementById('check').checked]").await, json!([1,false]));
    world_value(&mut ctx, &session, Some(world), "cancel=false").await;
    #[cfg(feature = "render")]
    for event in ["mousePressed", "mouseReleased"] {
        let response = cdp(&mut ctx, 5, "Input.dispatchMouseEvent", json!({"type":event,"x":30,"y":30,"button":"left","clickCount":1}), Some(&session)).await;
        assert!(response.error.is_none(), "{:?}", response.error);
    }
    #[cfg(not(feature = "render"))]
    world_value(&mut ctx, &session, None, "document.getElementById('check').click()").await;
    assert_eq!(world_value(&mut ctx, &session, None, "[clicks,changes,document.getElementById('check').checked,saved && saved.defaultPrevented]").await, json!([2,1,true,false]));
    assert_eq!(world_value(&mut ctx, &session, Some(world), "[clicks,document.getElementById('check').checked]").await, json!([2,true]));
}

#[tokio::test(flavor = "current_thread")]
async fn object_arguments_and_backend_nodes_reject_foreign_worlds_pages_and_documents() {
    let mut ctx = CdpContext::new();
    let (_, session) = create_and_attach(&mut ctx, "data:text/html,<input id=field>", 1).await;
    let (_, foreign) = create_and_attach(&mut ctx, "data:text/html,<input id=field>", 10).await;
    let world = cdp(&mut ctx, 3, "Page.createIsolatedWorld", json!({"worldName":"utility"}), Some(&session))
        .await.result.unwrap()["executionContextId"].as_i64().unwrap();
    let main = cdp(&mut ctx, 4, "Runtime.evaluate", json!({"expression":"document.getElementById('field')"}), Some(&session))
        .await.result.unwrap()["result"]["objectId"].as_str().unwrap().to_string();
    let isolated = cdp(&mut ctx, 5, "Runtime.evaluate", json!({"expression":"document.getElementById('field')","contextId":world}), Some(&session))
        .await.result.unwrap()["result"]["objectId"].as_str().unwrap().to_string();
    let node = cdp(&mut ctx, 6, "DOM.describeNode", json!({"objectId":isolated}), Some(&session))
        .await.result.unwrap()["node"]["backendNodeId"].clone();
    let resolved = cdp(&mut ctx, 7, "DOM.resolveNode", json!({"backendNodeId":node,"executionContextId":world}), Some(&session))
        .await.result.unwrap()["object"]["objectId"].as_str().unwrap().to_string();
    let valid = cdp(&mut ctx, 8, "Runtime.callFunctionOn", json!({
        "objectId":isolated,"arguments":[{"objectId":resolved}],"functionDeclaration":"function(other){return this===other && this instanceof HTMLInputElement}","returnByValue":true,
    }), Some(&session)).await;
    assert_eq!(valid.result.unwrap()["result"]["value"], true);
    for params in [
        json!({"objectId":main,"executionContextId":world}),
        json!({"objectId":isolated,"arguments":[{"objectId":main}]}),
        json!({"executionContextId":world,"arguments":[{"objectId":main}]}),
    ] {
        let mut params = params;
        params["functionDeclaration"] = json!("function(){throw new Error('must not execute')}");
        let rejected = cdp(&mut ctx, 9, "Runtime.callFunctionOn", params, Some(&session)).await;
        assert!(rejected.error.unwrap().message.contains("different execution context"));
    }
    for (method, params) in [
        ("Runtime.getProperties",json!({"objectId":isolated})),
        ("DOM.describeNode",json!({"objectId":isolated})),
        ("DOM.resolveNode",json!({"backendNodeId":node})),
    ] {
        assert!(cdp(&mut ctx, 10, method, params, Some(&foreign)).await.error.is_some(), "{method} accepted a foreign handle");
    }
    let replacement = cdp(&mut ctx, 11, "Page.navigate", json!({"url":"data:text/html,<input id=field>"}), Some(&session)).await;
    assert!(replacement.error.is_none());
    for (method, params) in [
        ("Runtime.getProperties",json!({"objectId":isolated})),
        ("Runtime.callFunctionOn",json!({"objectId":main,"functionDeclaration":"function(){return this.id}"})),
        ("DOM.describeNode",json!({"backendNodeId":node})),
    ] {
        assert!(cdp(&mut ctx, 12, method, params, Some(&session)).await.error.is_some(), "{method} accepted a retired handle");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn named_preloads_remain_private_on_immediate_blank_and_created_documents() {
    let mut ctx = CdpContext::new();
    let (target, session) = create_and_attach(&mut ctx, "about:blank", 1).await;
    cdp(&mut ctx, 3, "Runtime.enable", json!({}), Some(&session)).await;
    let script = cdp(&mut ctx, 4, "Page.addScriptToEvaluateOnNewDocument", json!({
        "worldName":"utility","runImmediately":true,"source":"globalThis.privatePreload=(globalThis.privatePreload||0)+1;globalThis.preloadURL=location.href",
    }), Some(&session)).await.result.unwrap();
    let world = ctx.pending_events.iter().rev().find(|event| event.method=="Runtime.executionContextCreated"
        && event.session_id.as_deref()==Some(session.as_str()) && event.params["context"]["name"]=="utility")
        .expect("immediate named preload context was not advertised").params["context"]["id"].as_i64().unwrap();
    assert_eq!(world_value(&mut ctx, &session, Some(world), "privatePreload===1 && preloadURL===location.href").await, true);
    world_value(&mut ctx, &session, Some(world), "document.body.setAttribute('data-preload',privatePreload)").await;
    assert_eq!(world_value(&mut ctx, &session, None, "[typeof privatePreload,document.body.getAttribute('data-preload')]").await, json!(["undefined","1"]));

    let (_, created) = create_and_attach(&mut ctx, "about:blank", 10).await;
    cdp(&mut ctx, 12, "Runtime.enable", json!({}), Some(&created)).await;
    assert!(!ctx.pending_events.iter().any(|event| event.method=="Runtime.executionContextCreated"
        && event.session_id.as_deref()==Some(created.as_str()) && event.params["context"]["name"]=="utility"));
    let created_world = cdp(&mut ctx, 13, "Page.createIsolatedWorld", json!({"worldName":"utility"}), Some(&created))
        .await.result.unwrap()["executionContextId"].as_i64().unwrap();
    assert_eq!(world_value(&mut ctx, &created, Some(created_world), "typeof privatePreload").await, "undefined");
    assert_eq!(world_value(&mut ctx, &created, None, "typeof privatePreload").await, "undefined");
    assert!(cdp(&mut ctx, 14, "Page.removeScriptToEvaluateOnNewDocument", script.clone(), Some(&created)).await.error.is_some());
    let attached = cdp(&mut ctx, 15, "Target.attachToTarget", json!({"targetId":target,"flatten":true}), None).await;
    let other_session = attached.result.unwrap()["sessionId"].as_str().unwrap().to_string();
    cdp(&mut ctx, 16, "Runtime.enable", json!({}), Some(&other_session)).await;
    assert_eq!(world_value(&mut ctx, &other_session, Some(world), "privatePreload===1").await, true);
    assert!(cdp(&mut ctx, 17, "Page.removeScriptToEvaluateOnNewDocument", script.clone(), Some(&other_session)).await.error.is_some());

    for (id, url) in [(20, "about:blank"), (30, "data:text/html,<title>preloaded</title>")] {
        assert!(cdp(&mut ctx, id, "Page.navigate", json!({"url":url}), Some(&session)).await.error.is_none());
        let next_world = ctx.pending_events.iter().rev().find(|event| event.method=="Runtime.executionContextCreated"
            && event.session_id.as_deref()==Some(session.as_str()) && event.params["context"]["name"]=="utility")
            .expect("new-document named preload context was not advertised").params["context"]["id"].as_i64().unwrap();
        assert_eq!(world_value(&mut ctx, &session, Some(next_world), "privatePreload===1 && preloadURL===location.href").await, true);
        assert_eq!(world_value(&mut ctx, &session, None, "typeof privatePreload").await, "undefined");
        assert_eq!(world_value(&mut ctx, &other_session, Some(next_world), "privatePreload===1").await, true);
    }
    assert!(cdp(&mut ctx, 40, "Page.removeScriptToEvaluateOnNewDocument", script.clone(), Some(&session)).await.error.is_none());
    assert!(cdp(&mut ctx, 41, "Page.removeScriptToEvaluateOnNewDocument", script, Some(&session)).await.error.is_some());
    assert!(cdp(&mut ctx, 42, "Page.navigate", json!({"url":"about:blank"}), Some(&session)).await.error.is_none());
    let next_world = cdp(&mut ctx, 43, "Page.createIsolatedWorld", json!({"worldName":"utility"}), Some(&session))
        .await.result.unwrap()["executionContextId"].as_i64().unwrap();
    assert_eq!(world_value(&mut ctx, &session, Some(next_world), "typeof privatePreload").await, "undefined");

    cdp(&mut ctx, 44, "Page.addScriptToEvaluateOnNewDocument", json!({
        "worldName":"utility","source":"globalThis.privatePreload=1",
    }), Some(&session)).await;
    assert!(cdp(&mut ctx, 45, "Target.detachFromTarget", json!({"sessionId":session}), None).await.error.is_none());
    assert_eq!(world_value(&mut ctx, &other_session, Some(next_world), "typeof privatePreload").await, "undefined");
    assert!(cdp(&mut ctx, 46, "Page.navigate", json!({"url":"about:blank"}), Some(&other_session)).await.error.is_none());
    let remaining_world = cdp(&mut ctx, 47, "Page.createIsolatedWorld", json!({"worldName":"utility"}), Some(&other_session))
        .await.result.unwrap()["executionContextId"].as_i64().unwrap();
    assert_eq!(world_value(&mut ctx, &other_session, Some(remaining_world), "typeof privatePreload").await, "undefined");
}
