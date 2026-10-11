#![cfg(feature = "render")]

use obscura_cdp::dispatch::{dispatch, CdpContext};
use obscura_cdp::types::CdpRequest;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

async fn setup() -> (CdpContext, String) {
    std::env::set_var("OBSCURA_ALLOW_PRIVATE_NETWORK", "1");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 2048];
        let _ = socket.read(&mut buf).await.unwrap();
        let body = r#"<!doctype html><style>
          body { margin: 0 }
          #background { position: absolute; left: 10px; top: 10px; width: 80px; height: 30px }
          dialog { position: absolute; left: 200px; top: 100px; width: 300px }
          dialog input, dialog button { display: block; width: 80px; height: 30px }
        </style><input id="background" value="background"><dialog id="outer">
          <input id="first"><input id="auto" autofocus>
          <button id="last" type="button">Last</button>
          <div inert><input id="inert-field" value="inert"></div>
          <input id="hidden-field" hidden><input id="disabled-field" disabled>
          <div id="shadow-host"></div>
        </dialog><dialog id="inner"><input id="inner-field"></dialog>"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = socket.write_all(response.as_bytes()).await;
    });
    let mut ctx = CdpContext::new();
    let page = ctx.create_page();
    let sid = "modal-input-session".to_string();
    ctx.sessions.insert(sid.clone(), page);
    cdp(&mut ctx, "Page.navigate", json!({"url":format!("http://{addr}/"),"waitUntil":"load"}), &sid).await;
    (ctx, sid)
}

async fn cdp(ctx: &mut CdpContext, method: &str, params: Value, sid: &str) -> Value {
    let response = dispatch(&CdpRequest {
        id: 1,
        method: method.to_string(),
        params,
        session_id: Some(sid.to_string()),
    }, ctx).await;
    assert!(response.error.is_none(), "CDP {method} failed: {:?}", response.error);
    response.result.unwrap_or_else(|| json!({}))
}

async fn evaluate_world(ctx: &mut CdpContext, sid: &str, world: Option<i64>, expression: &str) -> Value {
    let mut params = json!({"expression":expression,"returnByValue":true,"awaitPromise":true});
    if let Some(world) = world { params["contextId"] = json!(world); }
    let result = cdp(ctx, "Runtime.evaluate", params, sid).await;
    assert!(result.get("exceptionDetails").is_none(), "{result}");
    result["result"]["value"].clone()
}

async fn evaluate(ctx: &mut CdpContext, sid: &str, expression: &str) -> Value {
    evaluate_world(ctx, sid, None, expression).await
}

async fn key(ctx: &mut CdpContext, sid: &str, key: &str, shift: bool) {
    let virtual_key = if key == "Tab" { 9 } else { 27 };
    for kind in ["rawKeyDown", "keyUp"] {
        cdp(ctx, "Input.dispatchKeyEvent", json!({
            "type":kind,"key":key,"code":key,"windowsVirtualKeyCode":virtual_key,
            "modifiers":if shift { 8 } else { 0 },
        }), sid).await;
    }
}

async fn click(ctx: &mut CdpContext, sid: &str, expression: &str) {
    let point = evaluate(ctx, sid, &format!(
        "(()=>{{const r=({expression}).getBoundingClientRect();return [r.left+r.width/2,r.top+r.height/2]}})()"
    )).await;
    for kind in ["mousePressed", "mouseReleased"] {
        cdp(ctx, "Input.dispatchMouseEvent", json!({
            "type":kind,"x":point[0],"y":point[1],"button":"left","clickCount":1,
        }), sid).await;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn native_tab_stays_in_modal_and_focus_events_are_trusted() {
    let (mut ctx, sid) = setup().await;
    evaluate(&mut ctx, &sid, r#"(()=>{
        globalThis.focusEvents=[];
        for(const id of ['background','first','auto','last']) {
            document.getElementById(id).addEventListener('focus',e=>focusEvents.push([id,e.isTrusted]));
        }
        document.getElementById('background').focus();
        document.getElementById('outer').showModal();
    })()"#).await;
    assert_eq!(evaluate(&mut ctx, &sid, "document.activeElement.id").await, "auto");
    for (shift, expected) in [(false, "last"), (false, "first"), (true, "last")] {
        key(&mut ctx, &sid, "Tab", shift).await;
        assert_eq!(evaluate(&mut ctx, &sid, "document.activeElement.id").await, expected);
    }
    assert_eq!(evaluate(&mut ctx, &sid, "focusEvents").await,
        json!([["background",true],["auto",true],["last",true],["first",true],["last",true]]));
    evaluate(&mut ctx, &sid, "document.getElementById('outer').close();document.getElementById('auto').removeAttribute('autofocus');document.getElementById('outer').showModal()").await;
    assert_eq!(evaluate(&mut ctx, &sid, "document.activeElement.id").await, "first");
    evaluate(&mut ctx, &sid, "document.getElementById('outer').showModal()").await;
    assert_eq!(evaluate(&mut ctx, &sid, "document.activeElement.id").await, "first");
}

#[tokio::test(flavor = "current_thread")]
async fn native_escape_honors_cancel_and_restores_nested_focus() {
    let (mut ctx, sid) = setup().await;
    evaluate(&mut ctx, &sid, r#"(()=>{
        globalThis.cancelClose=true;globalThis.cancelEvents=[];
        const outer=document.getElementById('outer'),inner=document.getElementById('inner');
        inner.addEventListener('cancel',e=>{cancelEvents.push([e.isTrusted,e.cancelable]);if(cancelClose)e.preventDefault()});
        document.getElementById('background').focus();outer.showModal();inner.showModal();
    })()"#).await;
    key(&mut ctx, &sid, "Escape", false).await;
    assert_eq!(evaluate(&mut ctx, &sid, "[document.getElementById('inner').open,document.activeElement.id,cancelEvents]").await,
        json!([true,"inner-field",[[true,true]]]));
    evaluate(&mut ctx, &sid, "cancelClose=false").await;
    key(&mut ctx, &sid, "Escape", false).await;
    assert_eq!(evaluate(&mut ctx, &sid, "[document.getElementById('inner').open,document.getElementById('outer').open,document.activeElement.id]").await,
        json!([false,true,"auto"]));
    evaluate(&mut ctx, &sid, "document.getElementById('outer').closedBy='none'").await;
    key(&mut ctx, &sid, "Escape", false).await;
    assert_eq!(evaluate(&mut ctx, &sid, "document.getElementById('outer').open").await, true);
    evaluate(&mut ctx, &sid, "document.getElementById('outer').close('accepted')").await;
    assert_eq!(evaluate(&mut ctx, &sid, "[document.activeElement.id,document.getElementById('outer').returnValue]").await,
        json!(["background","accepted"]));
}

#[tokio::test(flavor = "current_thread")]
async fn background_and_inert_native_input_ignore_author_global_overrides() {
    let (mut ctx, sid) = setup().await;
    evaluate(&mut ctx, &sid, r#"(()=>{
        globalThis.blockedEvents=[];
        for(const id of ['background','inert-field'])for(const kind of ['pointerdown','mousedown','click','beforeinput','input']){
            document.getElementById(id).addEventListener(kind,()=>blockedEvents.push([id,kind]));
        }
        globalThis.__obscura_inputAllowed=()=>true;
        globalThis.__obscura_keyDefault=()=>document.getElementById('background').focus();
        document.getElementById('background').focus();document.getElementById('outer').showModal();
        document.getElementById('background').focus();document.getElementById('inert-field').focus();
        globalThis.__obscura_focused=document.getElementById('background');
        Element.prototype.focus=function(){throw new Error('author focus must not run')};
    })()"#).await;
    click(&mut ctx, &sid, "document.getElementById('background')").await;
    click(&mut ctx, &sid, "document.getElementById('inert-field')").await;
    cdp(&mut ctx, "Input.insertText", json!({"text":"X"}), &sid).await;
    assert_eq!(evaluate(&mut ctx, &sid,
        "[document.activeElement.id,document.getElementById('background').value,document.getElementById('inert-field').value,document.getElementById('auto').value,blockedEvents]").await,
        json!(["auto","background","inert","X",[]]));
    key(&mut ctx, &sid, "Tab", false).await;
    assert_eq!(evaluate(&mut ctx, &sid, "document.activeElement.id").await, "last");
    evaluate(&mut ctx, &sid, "document.getElementById('outer').close()").await;
    cdp(&mut ctx, "Input.insertText", json!({"text":"Y"}), &sid).await;
    assert_eq!(evaluate(&mut ctx, &sid, "[document.activeElement.id,document.getElementById('inert-field').value]").await,
        json!(["background","inert"]));
    assert_eq!(evaluate(&mut ctx, &sid, "document.getElementById('background').value").await, "backgroundY");
}

#[tokio::test(flavor = "current_thread")]
async fn isolated_world_cancellation_preserves_modal_input_default_actions() {
    let (mut ctx, sid) = setup().await;
    let world = cdp(&mut ctx, "Page.createIsolatedWorld", json!({"worldName":"modal-utility"}), &sid)
        .await["executionContextId"].as_i64().unwrap();
    evaluate(&mut ctx, &sid, "document.getElementById('background').focus();document.getElementById('outer').showModal();globalThis.mainInput=[];document.getElementById('auto').addEventListener('input',e=>mainInput.push([e.data,e.isTrusted]))").await;
    evaluate_world(&mut ctx, &sid, Some(world), r#"(()=>{
        globalThis.cancelEdit=true;globalThis.cancelTab=true;globalThis.cancelClose=true;globalThis.utilityEvents=[];
        const field=document.getElementById('auto');
        field.addEventListener('beforeinput',e=>{utilityEvents.push([e.type,e.isTrusted]);if(cancelEdit)e.preventDefault()});
        field.addEventListener('keydown',e=>{if(e.key==='Tab'&&cancelTab)e.preventDefault()});
        document.getElementById('outer').addEventListener('cancel',e=>{if(cancelClose)e.preventDefault()});
        document.getElementById('background').focus();
    })()"#).await;
    cdp(&mut ctx, "Input.insertText", json!({"text":"blocked"}), &sid).await;
    key(&mut ctx, &sid, "Tab", false).await;
    key(&mut ctx, &sid, "Escape", false).await;
    assert_eq!(evaluate(&mut ctx, &sid, "[document.activeElement.id,document.getElementById('auto').value,document.getElementById('outer').open,mainInput]").await,
        json!(["auto","",true,[]]));
    assert_eq!(evaluate_world(&mut ctx, &sid, Some(world), "utilityEvents").await, json!([["beforeinput",true]]));
    evaluate_world(&mut ctx, &sid, Some(world), "cancelEdit=false;cancelTab=false;cancelClose=false").await;
    cdp(&mut ctx, "Input.insertText", json!({"text":"accepted"}), &sid).await;
    assert_eq!(evaluate(&mut ctx, &sid, "[document.getElementById('auto').value,mainInput]").await,
        json!(["accepted",[["accepted",true]]]));
    key(&mut ctx, &sid, "Tab", false).await;
    assert_eq!(evaluate(&mut ctx, &sid, "document.activeElement.id").await, "last");
    key(&mut ctx, &sid, "Escape", false).await;
    assert_eq!(evaluate_world(&mut ctx, &sid, Some(world), "[document.getElementById('outer').open,document.activeElement.id]").await,
        json!([false,"background"]));
    evaluate_world(&mut ctx, &sid, Some(world), "document.getElementById('outer').showModal()").await;
    key(&mut ctx, &sid, "Tab", false).await;
    assert_eq!(evaluate(&mut ctx, &sid, "document.activeElement.id").await, "last");
}

#[tokio::test(flavor = "current_thread")]
async fn modal_input_crosses_closed_shadow_and_slot_inert_ownership() {
    let (mut ctx, sid) = setup().await;
    evaluate(&mut ctx, &sid, r#"(()=>{
        const host=document.getElementById('shadow-host');
        const root=host.attachShadow({mode:'closed'});
        root.innerHTML='<input id="shadow-field"><div inert><slot name="blocked"></slot></div>';
        globalThis.shadowField=root.querySelector('input');
        const slotted=document.createElement('input');slotted.id='slotted-field';slotted.slot='blocked';host.appendChild(slotted);
        document.getElementById('outer').showModal();shadowField.focus();
    })()"#).await;
    cdp(&mut ctx, "Input.insertText", json!({"text":"shadow"}), &sid).await;
    assert_eq!(evaluate(&mut ctx, &sid, "[document.activeElement===shadowField,shadowField.value]").await, json!([true,"shadow"]));
    evaluate(&mut ctx, &sid, "document.getElementById('slotted-field').focus()").await;
    cdp(&mut ctx, "Input.insertText", json!({"text":"!"}), &sid).await;
    assert_eq!(evaluate(&mut ctx, &sid, "[document.activeElement===shadowField,shadowField.value,document.getElementById('slotted-field').value]").await,
        json!([true,"shadow!",""]));
    evaluate(&mut ctx, &sid, "document.getElementById('shadow-host').inert=true").await;
    cdp(&mut ctx, "Input.insertText", json!({"text":"blocked"}), &sid).await;
    assert_eq!(evaluate(&mut ctx, &sid, "shadowField.value").await, "shadow!");
}

#[tokio::test(flavor = "current_thread")]
async fn modal_stacks_are_document_local_and_close_does_not_steal_newer_focus() {
    let (mut ctx, sid) = setup().await;
    evaluate(&mut ctx, &sid, r#"(()=>{
        const frame=document.createElement('iframe');frame.id='child';document.body.appendChild(frame);
        const child=frame.contentDocument;child.body.innerHTML='<input id="child-background"><dialog id="child-modal"><input id="child-field"></dialog>';
        document.getElementById('background').focus();document.getElementById('outer').showModal();
        child.getElementById('child-background').focus();child.getElementById('child-modal').showModal();
    })()"#).await;
    assert_eq!(evaluate(&mut ctx, &sid, "[document.activeElement.id,document.getElementById('child').contentDocument.activeElement.id]").await,
        json!(["auto","child-field"]));
    evaluate(&mut ctx, &sid, "document.getElementById('inner').showModal();document.getElementById('outer').close()").await;
    assert_eq!(evaluate(&mut ctx, &sid, "document.activeElement.id").await, "inner-field");
    evaluate(&mut ctx, &sid, "document.getElementById('inner').close()").await;
    assert_eq!(evaluate(&mut ctx, &sid, "[document.activeElement.id,document.getElementById('child').contentDocument.activeElement.id]").await,
        json!(["background","child-field"]));
}

#[tokio::test(flavor = "current_thread")]
async fn native_tab_orders_controls_and_script_close_requests_restore_focus() {
    let (mut ctx, sid) = setup().await;
    evaluate(&mut ctx, &sid, r#"(()=>{
        const outer=document.getElementById('outer');
        document.getElementById('background').focus();
        document.getElementById('first').tabIndex=2;document.getElementById('last').tabIndex=1;
        document.getElementById('auto').removeAttribute('autofocus');outer.showModal();
        globalThis.cancelScript=true;globalThis.scriptCancel=[];
        outer.addEventListener('cancel',e=>{scriptCancel.push(e.isTrusted);outer.requestClose();if(cancelScript)e.preventDefault()});
    })()"#).await;
    assert_eq!(evaluate(&mut ctx, &sid, "document.activeElement.id").await, "last");
    key(&mut ctx, &sid, "Tab", false).await;
    assert_eq!(evaluate(&mut ctx, &sid, "document.activeElement.id").await, "first");
    evaluate(&mut ctx, &sid, "document.activeElement.dispatchEvent(new KeyboardEvent('keydown',{key:'Escape',bubbles:true,cancelable:true}));document.getElementById('outer').requestClose('blocked')").await;
    assert_eq!(evaluate(&mut ctx, &sid, "[document.getElementById('outer').open,document.activeElement.id,scriptCancel]").await,
        json!([true,"first",[false]]));
    evaluate(&mut ctx, &sid, "cancelScript=false;document.getElementById('outer').requestClose('accepted')").await;
    assert_eq!(evaluate(&mut ctx, &sid, "[document.getElementById('outer').open,document.getElementById('outer').returnValue,document.activeElement.id,scriptCancel]").await,
        json!([false,"accepted","background",[false,false]]));
    evaluate(&mut ctx, &sid, "document.getElementById('outer').innerHTML='<input hidden><input disabled>';document.getElementById('outer').showModal()").await;
    key(&mut ctx, &sid, "Tab", true).await;
    assert_eq!(evaluate(&mut ctx, &sid, "document.activeElement.id").await, "outer");
}
