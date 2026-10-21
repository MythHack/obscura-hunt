use obscura_cdp::dispatch::{dispatch, CdpContext};
use obscura_cdp::types::{CdpRequest, CdpResponse};
use serde_json::{json, Value};

async fn call(ctx: &mut CdpContext, session: &str, method: &str, params: Value) -> CdpResponse {
    dispatch(&CdpRequest {
        id: 1, method: format!("DOM.{method}"), params, session_id: Some(session.into()),
    }, ctx).await
}

async fn ok(ctx: &mut CdpContext, session: &str, method: &str, params: Value) -> Value {
    let response = call(ctx, session, method, params).await;
    assert!(response.error.is_none(), "{method}: {:?}", response.error);
    response.result.unwrap()
}

async fn fixture() -> (CdpContext, String, String) {
    let mut ctx = CdpContext::new();
    let page = ctx.create_page();
    let session = format!("{page}-session");
    ctx.sessions.insert(session.clone(), page.clone());
    obscura_cdp::domains::page::handle("navigate", &json!({"url":
        "data:text/html,<main><label id=lab for=c style='position:absolute;left:32px;top:48px;width:120px;height:32px'>agree</label><input id=c></main>"
    }), &mut ctx, &Some(session.clone())).await.unwrap();
    ctx.pending_events.clear();
    (ctx, session, page)
}

fn object(ctx: &mut CdpContext, session: &str, expression: &str) -> String {
    ctx.get_session_page_mut(&Some(session.into())).unwrap().js.as_mut().unwrap()
        .store_object_with_meta(expression).unwrap().object_id.unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn dom_attributes_preserve_qualified_names_and_reject_invalid_nodes() {
    let (mut ctx, session, _) = fixture().await;
    let node = ok(&mut ctx, &session, "querySelector", json!({"selector":"#lab"})).await["nodeId"].clone();
    ctx.get_session_page_mut(&Some(session.clone())).unwrap().evaluate(
        "document.querySelector('#lab').setAttributeNS('urn:test','x:flag','yes')"
    );
    let attributes = ok(&mut ctx, &session, "getAttributes", json!({"nodeId":node})).await;
    let pairs: std::collections::BTreeMap<_, _> = attributes["attributes"].as_array().unwrap()
        .chunks_exact(2).map(|pair| (pair[0].as_str().unwrap(), pair[1].as_str().unwrap())).collect();
    assert_eq!(pairs["id"], "lab");
    assert_eq!(pairs["for"], "c");
    assert_eq!(pairs["x:flag"], "yes");
    for invalid in [json!({}), json!({"nodeId":999999}), json!({"nodeId":u64::MAX})] {
        assert!(call(&mut ctx, &session, "getAttributes", invalid).await.error.is_some());
    }
}

#[tokio::test(flavor = "current_thread")]
async fn dom_request_node_pushes_ancestors_once_to_the_requesting_session() {
    let (mut ctx, session, page) = fixture().await;
    let oid = object(&mut ctx, &session, "document.querySelector('#lab')");
    assert_eq!(ok(&mut ctx, &session, "requestNode", json!({"objectId":oid})).await["nodeId"], 0);
    ok(&mut ctx, &session, "getDocument", json!({"depth":0})).await;
    ctx.pending_events.clear();
    let node = ok(&mut ctx, &session, "requestNode", json!({"objectId":oid})).await["nodeId"].clone();
    assert_ne!(node, 0);
    let pushes: Vec<_> = ctx.pending_events.iter().filter(|event| event.method == "DOM.setChildNodes").collect();
    assert!(!pushes.is_empty());
    assert!(pushes.iter().all(|event| event.session_id.as_deref() == Some(session.as_str())));
    assert!(pushes.iter().any(|event| event.params["nodes"].as_array().unwrap()
        .iter().any(|child| child["nodeId"] == node && child["nodeName"] == "LABEL")));
    let queried = ok(&mut ctx, &session, "querySelector", json!({"selector":"#lab"})).await;
    assert_eq!(node, queried["nodeId"]);
    let described = ok(&mut ctx, &session, "describeNode", json!({"nodeId":node})).await;
    assert_eq!(described["node"]["nodeName"], "LABEL");
    assert_eq!(described["node"]["backendNodeId"], node);
    ctx.pending_events.clear();
    ok(&mut ctx, &session, "requestNode", json!({"objectId":oid})).await;
    assert!(ctx.pending_events.is_empty(), "known nodes must not be pushed again");
    ctx.sessions.insert("other".into(), page);
    let other_oid = object(&mut ctx, "other", "document.querySelector('#lab')");
    assert_eq!(ok(&mut ctx, "other", "requestNode", json!({"objectId":other_oid})).await["nodeId"], 0);
    ok(&mut ctx, "other", "getDocument", json!({"depth":0})).await;
    assert_ne!(ok(&mut ctx, "other", "requestNode", json!({"objectId":other_oid})).await["nodeId"], 0);
    assert!(ctx.pending_events.iter().any(|event| event.method == "DOM.setChildNodes"
        && event.session_id.as_deref() == Some("other")));
    for expression in ["({})", "({_nid:1})"] {
        let invalid = object(&mut ctx, &session, expression);
        assert!(call(&mut ctx, &session, "requestNode", json!({"objectId":invalid})).await.error.is_some());
    }
    assert!(call(&mut ctx, &session, "requestNode", json!({"objectId":"missing"})).await.error.is_some());
    ok(&mut ctx, &session, "getDocument", json!({"depth":-1})).await;
    let inserted = object(&mut ctx, &session,
        "document.querySelector('main').appendChild(document.createElement('button'))");
    let inserted_id = ok(&mut ctx, &session, "requestNode", json!({"objectId":inserted})).await["nodeId"].clone();
    let event = ctx.pending_events.iter().find(|event| event.method == "DOM.childNodeInserted"
        && event.params["node"]["nodeId"] == inserted_id).expect("insertions in a published parent must be published live");
    assert_eq!(event.params["node"]["nodeName"], "BUTTON");
    assert_eq!(ok(&mut ctx, &session, "getAttributes", json!({"nodeId":inserted_id})).await["attributes"], json!([]));
}

#[tokio::test(flavor = "current_thread")]
async fn dom_child_requests_expand_depth_without_repeating_known_children() {
    let (mut ctx, session, _) = fixture().await;
    let root = ok(&mut ctx, &session, "getDocument", json!({"depth":0})).await["root"]["nodeId"].clone();
    ctx.pending_events.clear();
    assert_eq!(ok(&mut ctx, &session, "requestChildNodes", json!({"nodeId":root,"depth":2})).await, json!({}));
    let event = ctx.pending_events.iter().find(|event| event.method == "DOM.setChildNodes").unwrap();
    assert_eq!(event.params["parentId"], root);
    let html = &event.params["nodes"][0];
    assert_eq!(html["nodeName"], "HTML");
    assert_eq!(html["children"][1]["nodeName"], "BODY");
    let html_id = html["nodeId"].clone();
    let body_id = html["children"][1]["nodeId"].clone();
    assert_eq!(html["backendNodeId"], html_id);
    assert_eq!(html["children"][1]["backendNodeId"], body_id);
    assert_eq!(ok(&mut ctx, &session, "querySelector", json!({"selector":"html"})).await["nodeId"], html_id);
    assert_eq!(ok(&mut ctx, &session, "querySelector", json!({"selector":"body"})).await["nodeId"], body_id);
    let described = ok(&mut ctx, &session, "describeNode", json!({"backendNodeId":body_id})).await;
    assert_eq!(described["node"]["nodeName"], "BODY");
    ctx.pending_events.clear();
    ok(&mut ctx, &session, "requestChildNodes", json!({"nodeId":root,"depth":2})).await;
    assert!(ctx.pending_events.is_empty());
    ok(&mut ctx, &session, "requestChildNodes", json!({"nodeId":root,"depth":-1})).await;
    assert!(ctx.pending_events.iter().any(|event| event.method == "DOM.setChildNodes"));
    for depth in [json!(0), json!(-2), json!("bad")] {
        assert!(call(&mut ctx, &session, "requestChildNodes", json!({"nodeId":root,"depth":depth})).await.error.is_some());
    }
}

#[tokio::test(flavor = "current_thread")]
async fn dom_child_requests_allow_pierce_on_plain_trees_but_reject_unserved_roots() {
    let (mut ctx, session, _) = fixture().await;
    let root = ok(&mut ctx, &session, "getDocument", json!({"depth":0})).await["root"]["nodeId"].clone();
    ok(&mut ctx, &session, "requestChildNodes", json!({"nodeId":root,"depth":-1,"pierce":true})).await;
    assert!(ctx.pending_events.iter().any(|event| event.method == "DOM.setChildNodes"));
    ctx.get_session_page_mut(&Some(session.clone())).unwrap().evaluate(
        "document.querySelector('main').attachShadow({mode:'open'}).innerHTML='<button>shadow</button>'"
    );
    assert!(call(&mut ctx, &session, "requestChildNodes",
        json!({"nodeId":root,"depth":-1,"pierce":true})).await.error.is_some(),
        "a requested shadow tree must not be silently omitted");
}

#[tokio::test(flavor = "current_thread")]
async fn dom_request_node_accepts_native_isolated_handles_and_rejects_disconnected_nodes() {
    let (mut ctx, session, _) = fixture().await;
    ok(&mut ctx, &session, "getDocument", json!({"depth":0})).await;
    let world = obscura_cdp::domains::page::handle("createIsolatedWorld",
        &json!({"worldName":"node-query-utility"}), &mut ctx, &Some(session.clone())).await.unwrap();
    let evaluated = dispatch(&CdpRequest {
        id: 2, method: "Runtime.evaluate".into(), session_id: Some(session.clone()),
        params: json!({"expression":"document.querySelector('#lab')","contextId":world["executionContextId"]}),
    }, &mut ctx).await;
    assert!(evaluated.error.is_none(), "{:?}", evaluated.error);
    let oid = evaluated.result.unwrap()["result"]["objectId"].as_str().unwrap().to_string();
    let node = ok(&mut ctx, &session, "requestNode", json!({"objectId":oid})).await["nodeId"].clone();
    let queried = ok(&mut ctx, &session, "querySelector", json!({"selector":"#lab"})).await;
    assert_eq!(node, queried["nodeId"], "worlds must share the same document-scoped node identity");
    let resolved = ok(&mut ctx, &session, "resolveNode",
        json!({"nodeId":node,"executionContextId":world["executionContextId"]})).await;
    let identity = dispatch(&CdpRequest {
        id: 3, method: "Runtime.callFunctionOn".into(), session_id: Some(session.clone()),
        params: json!({"objectId":oid,"arguments":[{"objectId":resolved["object"]["objectId"]}],
            "functionDeclaration":"function(other){return this===other}","returnByValue":true}),
    }, &mut ctx).await;
    assert!(identity.error.is_none(), "{:?}", identity.error);
    assert_eq!(identity.result.unwrap()["result"]["value"], true);
    for expression in [
        "document.createElement('aside')",
        "(document.querySelector('#lab').remove(), document.createElement('aside'))",
        "Object.create(Node.prototype)",
    ] {
        let invalid = object(&mut ctx, &session, expression);
        assert!(call(&mut ctx, &session, "requestNode", json!({"objectId":invalid})).await.error.is_some());
    }
    assert!(call(&mut ctx, &session, "requestNode", json!({"objectId":oid})).await.error.is_some(),
        "a previously published node must be rejected after it is disconnected");
}

#[tokio::test(flavor = "current_thread")]
async fn dom_node_queries_reject_foreign_and_stale_scoped_ids() {
    let (mut ctx, session, _) = fixture().await;
    let root = ok(&mut ctx, &session, "getDocument", json!({"depth":0})).await["root"]["nodeId"].clone();
    let node = ok(&mut ctx, &session, "querySelector", json!({"selector":"#lab"})).await["nodeId"].clone();
    let oid = object(&mut ctx, &session, "document.querySelector('#lab')");
    let foreign_page = ctx.create_page();
    let foreign = format!("{foreign_page}-session");
    ctx.sessions.insert(foreign.clone(), foreign_page);
    obscura_cdp::domains::page::handle("navigate", &json!({"url":"data:text/html,<label id=lab>foreign</label>"}),
        &mut ctx, &Some(foreign.clone())).await.unwrap();
    ok(&mut ctx, &foreign, "getDocument", json!({"depth":0})).await;
    for (method, params) in [
        ("getAttributes", json!({"nodeId":node})),
        ("requestChildNodes", json!({"nodeId":root})),
        ("requestNode", json!({"objectId":oid})),
    ] {
        assert!(call(&mut ctx, &foreign, method, params).await.error.is_some(), "{method} accepted a foreign node");
    }
    obscura_cdp::domains::page::handle("navigate", &json!({"url":"data:text/html,<label id=lab>replacement</label>"}),
        &mut ctx, &Some(session.clone())).await.unwrap();
    let replacement = ok(&mut ctx, &session, "getDocument", json!({"depth":0})).await["root"]["nodeId"].clone();
    assert_ne!(root, replacement, "navigation must not reuse the old frontend identity");
    ctx.pending_events.clear();
    for (method, params) in [
        ("getAttributes", json!({"nodeId":node})),
        ("requestChildNodes", json!({"nodeId":root})),
        ("requestNode", json!({"objectId":oid})),
    ] {
        assert!(call(&mut ctx, &session, method, params).await.error.is_some(), "{method} accepted a stale node");
    }
    assert!(!ctx.pending_events.iter().any(|event| event.method == "DOM.setChildNodes"));
}

#[tokio::test(flavor = "current_thread")]
async fn dom_node_queries_keep_child_frame_identity_outside_the_main_light_dom() {
    let (mut ctx, session, _) = fixture().await;
    obscura_cdp::domains::page::handle("navigate", &json!({"url":
        "data:text/html,<label id=lab>parent</label><iframe srcdoc='<label id=lab data-owner=child>child</label>'></iframe>"
    }), &mut ctx, &Some(session.clone())).await.unwrap();
    let tree = obscura_cdp::domains::page::handle("getFrameTree", &json!({}),
        &mut ctx, &Some(session.clone())).await.unwrap();
    let frame = tree["frameTree"]["childFrames"][0]["frame"]["id"].as_str().unwrap();
    let world = obscura_cdp::domains::page::handle("createIsolatedWorld",
        &json!({"frameId":frame,"worldName":"child-node-query"}), &mut ctx, &Some(session.clone())).await.unwrap();
    let evaluated = dispatch(&CdpRequest {
        id: 2, method: "Runtime.evaluate".into(), session_id: Some(session.clone()),
        params: json!({"expression":"document.querySelector('#lab')","contextId":world["executionContextId"]}),
    }, &mut ctx).await;
    assert!(evaluated.error.is_none(), "{:?}", evaluated.error);
    let oid = evaluated.result.unwrap()["result"]["objectId"].as_str().unwrap().to_string();
    let child = ok(&mut ctx, &session, "describeNode", json!({"objectId":oid})).await["node"]["nodeId"].clone();
    let parent = ok(&mut ctx, &session, "querySelector", json!({"selector":"#lab"})).await["nodeId"].clone();
    assert_ne!(child, parent);
    let attributes = ok(&mut ctx, &session, "getAttributes", json!({"nodeId":child})).await;
    assert!(attributes["attributes"].as_array().unwrap().chunks_exact(2)
        .any(|pair| pair[0] == "data-owner" && pair[1] == "child"));
    ok(&mut ctx, &session, "getDocument", json!({"depth":-1})).await;
    ctx.pending_events.clear();
    assert!(call(&mut ctx, &session, "requestNode", json!({"objectId":oid})).await.error.is_some(),
        "child-frame handles must not alias a main-document native node index");
    ok(&mut ctx, &session, "requestChildNodes", json!({"nodeId":child})).await;
    assert!(!ctx.pending_events.iter().any(|event| event.method == "DOM.setChildNodes"),
        "an unserved child frame must not expand a main-document node with the same native index");
}

#[cfg(feature = "render")]
#[tokio::test(flavor = "current_thread")]
async fn dom_hit_lookup_reports_frame_and_only_publishes_requested_node_ids() {
    let (mut ctx, session, page) = fixture().await;
    let params = json!({"x":64,"y":64});
    let before = ok(&mut ctx, &session, "getNodeForLocation", params.clone()).await;
    assert!(before.get("nodeId").is_none());
    assert!(before["backendNodeId"].is_number());
    assert_eq!(before["frameId"], ctx.get_page(&page).unwrap().frame_id);
    ok(&mut ctx, &session, "getDocument", json!({"depth":0})).await;
    let after = ok(&mut ctx, &session, "getNodeForLocation", params).await;
    let description = ok(&mut ctx, &session, "describeNode", json!({"backendNodeId":after["backendNodeId"]})).await;
    assert_eq!(description["node"]["nodeName"], "LABEL");
    assert_eq!(after["nodeId"], description["node"]["nodeId"]);
    assert_eq!(before["backendNodeId"], after["backendNodeId"]);
    assert_eq!(ok(&mut ctx, &session, "querySelector", json!({"selector":"#lab"})).await["nodeId"], after["nodeId"]);
    assert!(call(&mut ctx, &session, "getNodeForLocation",
        json!({"x":64,"y":64,"ignorePointerEventsNone":true})).await.error.is_some());
    assert!(call(&mut ctx, &session, "getNodeForLocation", json!({"x":-1,"y":64})).await.error.is_some());
    ok(&mut ctx, &session, "disable", json!({})).await;
    assert!(ok(&mut ctx, &session, "getNodeForLocation", json!({"x":64,"y":64})).await.get("nodeId").is_none());
}
