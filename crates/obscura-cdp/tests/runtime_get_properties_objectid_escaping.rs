//! Regression coverage for property handles and DOM handles (issue #709).
//! The former implementation embedded property names in child object ids and
//! generated JS lookups. Control characters could turn those lookups into syntax
//! errors, hiding nested properties or resolving the wrong DOM node.
//!
//! Native descriptor reads now mint ordinary execution-world handles in the
//! existing object store. Property names remain data, accessors remain uncalled,
//! and DOM resolution uses the captured private node brand rather than `_nid`.

use obscura_cdp::dispatch::{dispatch, CdpContext};
use obscura_cdp::types::CdpRequest;
use serde_json::{json, Value};

async fn cdp(ctx: &mut CdpContext, id: u64, method: &str, params: Value, session_id: &str) -> Value {
    let resp = dispatch(
        &CdpRequest {
            id,
            method: method.to_string(),
            params,
            session_id: Some(session_id.to_string()),
        },
        ctx,
    )
    .await;
    assert!(resp.error.is_none(), "CDP {method} failed: {:?}", resp.error);
    resp.result.unwrap_or_else(|| json!({}))
}

/// The `objectId` of the property descriptor named `name`.
fn child_object_id(props: &Value, name: &str) -> Option<String> {
    props
        .get("result")?
        .as_array()?
        .iter()
        .find(|d| d.get("name").and_then(|v| v.as_str()) == Some(name))
        .and_then(|d| d.pointer("/value/objectId"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// The string value of the property descriptor named `name`.
fn string_property(props: &Value, name: &str) -> Option<String> {
    props
        .get("result")?
        .as_array()?
        .iter()
        .find(|d| d.get("name").and_then(|v| v.as_str()) == Some(name))
        .and_then(|d| d.pointer("/value/value"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

// The keys are assembled with String.fromCharCode so this expression carries no
// backslash of its own: the characters under test are exactly the four below
// and not an artefact of how the literal was written. A page is free to define
// every one of them; walking their handles must preserve the underlying value.
const SETUP: &str = r#"(function () {
    var LF = String.fromCharCode(10);
    var CR = String.fromCharCode(13);
    var NUL = String.fromCharCode(0);
    var BACKSLASH = String.fromCharCode(92);
    var o = {};
    o["lf" + LF + "x"] = { mark: "lf" };
    o["cr" + CR + "x"] = { mark: "cr" };
    o["nul" + NUL + "x"] = { mark: "nul" };
    o["quote'" + BACKSLASH + "x"] = { mark: "quote" };
    globalThis.__t = o;
    return o;
})()"#;

#[tokio::test(flavor = "current_thread")]
async fn get_properties_walks_into_a_child_whose_key_holds_a_control_character() {
    let mut ctx = CdpContext::new();
    let page_id = ctx.create_page();
    let session_id = "session-1";
    ctx.sessions.insert(session_id.to_string(), page_id.clone());

    cdp(
        &mut ctx,
        1,
        "Page.navigate",
        json!({"url": "about:blank", "waitUntil": "load"}),
        session_id,
    )
    .await;

    // returnByValue omitted, so the result is a handle rather than a copy.
    let root = cdp(
        &mut ctx,
        2,
        "Runtime.evaluate",
        json!({"expression": SETUP}),
        session_id,
    )
    .await;
    let root_oid = root
        .pointer("/result/objectId")
        .and_then(|v| v.as_str())
        .expect("Runtime.evaluate must hand back an objectId for an object result")
        .to_string();

    let top = cdp(
        &mut ctx,
        3,
        "Runtime.getProperties",
        json!({"objectId": root_oid}),
        session_id,
    )
    .await;

    // The last pair is the case the hand-rolled escaping already covered. It is
    // here so a regression in either direction shows up in one run: a fix that
    // traded the quote handling for control-character handling would pass the
    // first three and fail this one.
    let cases = [
        ("lf\nx", "lf"),
        ("cr\rx", "cr"),
        ("nul\0x", "nul"),
        ("quote'\\x", "quote"),
    ];

    for (id, (key, mark)) in cases.iter().enumerate() {
        let child_oid = child_object_id(&top, key).unwrap_or_else(|| {
            panic!("no child objectId for key {key:?}; descriptors were {top:#?}")
        });

        let child = cdp(
            &mut ctx,
            10 + id as u64,
            "Runtime.getProperties",
            json!({"objectId": child_oid}),
            session_id,
        )
        .await;

        assert_eq!(
            string_property(&child, "mark").as_deref(),
            Some(*mark),
            "walking into the child keyed {key:?} must reach its properties, \
             not fall through to an empty list; got {child:#?}"
        );
    }
}

// Puppeteer's `$$` flow is evaluate -> getProperties -> asElement. The property
// handles then reach DOM.describeNode / DOM.resolveNode, so the same unusual
// names must preserve the native node's identity across both protocol domains.
const SETUP_NODES: &str = r#"(function () {
    var LF = String.fromCharCode(10);
    var o = {};
    o["plain"] = document.getElementById("a");
    o["lf" + LF + "x"] = document.getElementById("b");
    globalThis.__n = o;
    return o;
})()"#;

#[tokio::test(flavor = "current_thread")]
async fn describe_node_resolves_a_child_handle_whose_key_holds_a_control_character() {
    let mut ctx = CdpContext::new();
    let page_id = ctx.create_page();
    let session_id = "session-1";
    ctx.sessions.insert(session_id.to_string(), page_id.clone());

    cdp(
        &mut ctx,
        1,
        "Page.navigate",
        json!({"url": "data:text/html,<div id=a></div><p id=b></p>", "waitUntil": "load"}),
        session_id,
    )
    .await;

    let root = cdp(
        &mut ctx,
        2,
        "Runtime.evaluate",
        json!({"expression": SETUP_NODES}),
        session_id,
    )
    .await;
    let root_oid = root
        .pointer("/result/objectId")
        .and_then(|v| v.as_str())
        .expect("Runtime.evaluate must hand back an objectId for an object result")
        .to_string();

    let top = cdp(
        &mut ctx,
        3,
        "Runtime.getProperties",
        json!({"objectId": root_oid}),
        session_id,
    )
    .await;

    for (id, (key, node_name)) in [("plain", "DIV"), ("lf\nx", "P")].iter().enumerate() {
        let child_oid = child_object_id(&top, key).unwrap_or_else(|| {
            panic!("no child objectId for key {key:?}; descriptors were {top:#?}")
        });

        let described = cdp(
            &mut ctx,
            20 + id as u64,
            "DOM.describeNode",
            json!({"objectId": child_oid}),
            session_id,
        )
        .await;

        assert_eq!(
            described.pointer("/node/nodeName").and_then(|v| v.as_str()),
            Some(*node_name),
            "the handle keyed {key:?} must describe its own element; got {described:#?}"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn get_properties_preserves_own_descriptors_without_running_author_getters() {
    let mut ctx = CdpContext::new();
    let page_id = ctx.create_page();
    let session = "own-descriptors";
    ctx.sessions.insert(session.to_string(), page_id);
    // The first command materializes this blank document. Its returned context
    // must remain live for the next command, without a Runtime.enable warmup.
    let world = cdp(&mut ctx, 1, "Page.createIsolatedWorld", json!({"worldName":"utility"}), session)
        .await["executionContextId"].as_i64().unwrap();
    let root = cdp(&mut ctx, 2, "Runtime.evaluate", json!({
        "contextId": world,
        "expression": r#"(() => {
            globalThis.getterCalls = 0;
            const child = {mark:'child'};
            for (const key of ['constructor', 'toJSON', 'toString']) {
                Object.defineProperty(child, key, {get() { getterCalls++; throw new Error(key); }});
            }
            const object = Object.create({inherited:1});
            Object.defineProperties(object, {
                hidden: {value:child, writable:false, enumerable:false, configurable:true},
                accessor: {get() { getterCalls++; throw new Error('accessor'); }, set(value) {}, enumerable:true},
                undef: {value:undefined},
                negativeZero: {value:-0},
                nan: {value:NaN},
                bigint: {value:9007199254740993n}
            });
            object[Symbol('token')] = 'symbol value';
            Object.getOwnPropertyNames = Object.getOwnPropertyDescriptor = () => { throw new Error('author intrinsic'); };
            return object;
        })()"#,
    }), session).await;
    let oid = root["result"]["objectId"].as_str().unwrap();
    let props = cdp(&mut ctx, 3, "Runtime.getProperties", json!({"objectId":oid,"ownProperties":true}), session).await;
    let descriptors = props["result"].as_array().unwrap();
    let property = |name: &str| descriptors.iter().find(|descriptor| descriptor["name"] == name).unwrap();
    let hidden = property("hidden");
    assert_eq!(hidden["writable"], false);
    assert_eq!(hidden["enumerable"], false);
    assert_eq!(hidden["configurable"], true);
    assert_eq!(hidden["isOwn"], true);
    let accessor = property("accessor");
    assert!(accessor.get("value").is_none());
    assert!(accessor.get("writable").is_none());
    assert_eq!(accessor["get"]["type"], "function");
    assert_eq!(accessor["set"]["type"], "function");
    assert!(accessor["get"]["objectId"].is_string());
    assert_eq!(property("undef")["value"]["type"], "undefined");
    assert!(property("undef")["value"].get("value").is_none());
    assert_eq!(property("negativeZero")["value"]["unserializableValue"], "-0");
    assert_eq!(property("nan")["value"]["unserializableValue"], "NaN");
    assert_eq!(property("bigint")["value"]["unserializableValue"], "9007199254740993n");
    assert!(!descriptors.iter().any(|descriptor| descriptor["name"] == "inherited"));
    let symbol = descriptors.iter().find(|descriptor| descriptor.get("symbol").is_some()).unwrap();
    assert_eq!(symbol["name"], "Symbol(token)");
    assert_eq!(symbol["symbol"]["type"], "symbol");
    assert_eq!(symbol["value"]["value"], "symbol value");
    let child = cdp(&mut ctx, 4, "Runtime.getProperties", json!({
        "objectId": hidden["value"]["objectId"], "ownProperties":true,
    }), session).await;
    assert_eq!(string_property(&child, "mark").as_deref(), Some("child"));
    let identity = cdp(&mut ctx, 5, "Runtime.callFunctionOn", json!({
        "objectId": hidden["value"]["objectId"], "executionContextId":world,
        "functionDeclaration":"function(){return this.mark}", "returnByValue":true,
    }), session).await;
    assert_eq!(identity["result"]["value"], "child");
    let calls = cdp(&mut ctx, 6, "Runtime.evaluate", json!({
        "expression":"getterCalls===0", "contextId":world, "returnByValue":true,
    }), session).await;
    assert_eq!(calls["result"]["value"], true);
}

#[tokio::test(flavor = "current_thread")]
async fn dom_object_handles_use_private_node_identity_not_author_fields() {
    let mut ctx = CdpContext::new();
    let page_id = ctx.create_page();
    let session = "node-identity";
    ctx.sessions.insert(session.to_string(), page_id);
    cdp(&mut ctx, 1, "Page.navigate", json!({
        "url":"data:text/html,<p id=real>native node</p>", "waitUntil":"load",
    }), session).await;
    let root = cdp(&mut ctx, 2, "Runtime.evaluate", json!({
        "expression":r#"(() => {
            globalThis.getterCalls = 0;
            const real = document.getElementById('real'), nid = real._nid;
            const forged = {}, constructed = new Node(nid);
            Object.defineProperty(forged, '_nid', {get() { getterCalls++; return nid; }});
            for (const key of ['_nid', 'constructor', 'nodeType', 'tagName', 'nodeName']) {
                Object.defineProperty(real, key, {get() { getterCalls++; throw new Error(key); }});
            }
            return {real, forged, constructed};
        })()"#,
    }), session).await;
    let props = cdp(&mut ctx, 3, "Runtime.getProperties", json!({
        "objectId":root["result"]["objectId"],
    }), session).await;
    let real = child_object_id(&props, "real").unwrap();
    let described = cdp(&mut ctx, 4, "DOM.describeNode", json!({"objectId":real}), session).await;
    assert_eq!(described["node"]["nodeName"], "P");
    let resolved = cdp(&mut ctx, 5, "DOM.resolveNode", json!({
        "backendNodeId":described["node"]["backendNodeId"],
    }), session).await;
    let identity = cdp(&mut ctx, 6, "Runtime.callFunctionOn", json!({
        "objectId":resolved["object"]["objectId"],
        "functionDeclaration":"function(){return this===document.getElementById('real')}",
        "returnByValue":true,
    }), session).await;
    assert_eq!(identity["result"]["value"], true);
    for name in ["forged", "constructed"] {
        let response = dispatch(&CdpRequest {
            id:7, method:"DOM.describeNode".to_string(),
            params:json!({"objectId":child_object_id(&props, name).unwrap()}),
            session_id:Some(session.to_string()),
        }, &mut ctx).await;
        assert!(response.error.is_some(), "accepted unbranded {name}");
    }
    let calls = cdp(&mut ctx, 8, "Runtime.evaluate", json!({
        "expression":"getterCalls", "returnByValue":true,
    }), session).await;
    assert_eq!(calls["result"]["value"], json!(0.0));
}
