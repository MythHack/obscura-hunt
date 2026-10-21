use obscura_dom::parse_html;
use obscura_js::frame::FrameRealm;
use obscura_js::runtime::{CdpExecutionWorld, ObscuraJsRuntime};
use serde_json::json;

fn page() -> ObscuraJsRuntime {
    let mut runtime = ObscuraJsRuntime::with_base_url("https://world.example/page");
    runtime.set_dom(parse_html("<html><body><p id='p'>original</p><input id='field'><input id='check' type='checkbox'></body></html>"));
    runtime.run_page_init();
    runtime
}

fn world(runtime: &mut ObscuraJsRuntime, frame_id: u32, name: &str) -> CdpExecutionWorld {
    CdpExecutionWorld { frame_id, world_id: Some(runtime.ensure_isolated_world(frame_id, name, false).unwrap()) }
}

#[test]
fn isolated_globals_prototypes_and_expandos_share_only_the_native_document() {
    let mut runtime = page();
    assert_eq!(runtime.evaluate(r#"
        globalThis.author = 1; Element.prototype.authorPrototype = 2;
        document.getElementById('p').authorExpando = 3;
        var authorVariable = 11; let authorLexical = 12;
        authorVariable + authorLexical;
        //# sourceURL=author-world-script.js
    "#).unwrap().as_f64(), Some(23.0));
    let utility = world(&mut runtime, 0, "utility");
    assert_eq!(runtime.evaluate_in_cdp_frame(utility, "[typeof author, typeof authorVariable, typeof authorLexical, typeof Element.prototype.authorPrototype, typeof document.getElementById('p').authorExpando]").unwrap(), json!(["undefined", "undefined", "undefined", "undefined", "undefined"]));
    runtime.evaluate_in_cdp_frame(utility, "globalThis.utility = 4; Element.prototype.utilityPrototype = 5; document.getElementById('p').utilityExpando = 6; document.getElementById('p').textContent = 'shared'; document.getElementById('field').value = 'utility'; document.getElementById('check').checked = true; document.getElementById('field').focus()").unwrap();
    assert_eq!(runtime.evaluate("[typeof utility, typeof Element.prototype.utilityPrototype, typeof document.getElementById('p').utilityExpando, document.getElementById('p').textContent, document.getElementById('field').value, document.getElementById('check').checked, document.activeElement.id]").unwrap(), json!(["undefined", "undefined", "undefined", "shared", "utility", true, "field"]));
    runtime.evaluate("document.getElementById('field').value = 'author'; document.getElementById('check').checked = false").unwrap();
    assert_eq!(runtime.evaluate_in_cdp_frame(utility, "[document.getElementById('field').value, document.getElementById('check').checked]").unwrap(), json!(["author", false]));
    assert_eq!(runtime.evaluate("[authorVariable, authorLexical]").unwrap(), json!([11, 12]));
    assert!(runtime.evaluate("throw new Error('script failure')").is_err());
    assert!(runtime.evaluate("let = ;").is_err());
    assert_eq!(world(&mut runtime, 0, "utility"), utility);
    assert_ne!(world(&mut runtime, 0, "another"), utility);
}

#[tokio::test(flavor = "current_thread")]
async fn isolated_await_timers_and_objects_are_owned_by_the_real_world() {
    let mut runtime = page();
    let utility = world(&mut runtime, 0, "utility");
    let result = runtime.evaluate_for_cdp_in_frame(utility, "new Promise(resolve => setTimeout(() => { globalThis.timerMarker = 7; document.getElementById('p').textContent = 'timer'; resolve(timerMarker); }, 0))", true, true, 2_000).await.unwrap();
    assert!(!result.thrown, "isolated timer rejected: {result:?}");
    assert_eq!(result.value.as_ref().and_then(serde_json::Value::as_f64), Some(7.0));
    assert_eq!(runtime.evaluate("[typeof timerMarker, document.getElementById('p').textContent]").unwrap(), json!(["undefined", "timer"]));
    let object = runtime.evaluate_for_cdp_in_frame(utility, "({utility: 9})", false, false, 2_000).await.unwrap().object_id.unwrap();
    assert_eq!(runtime.object_world(&object).unwrap(), utility);
    let foreign = runtime.call_function_on_for_cdp_in_frame(CdpExecutionWorld::default(), "function(value) { return value.utility; }", None, &[json!({"objectId": object})], true, false, 2_000).await;
    assert!(foreign.is_err(), "object arguments must not cross execution worlds");
    runtime.set_dom(parse_html("<html><body>replacement</body></html>"));
    runtime.refresh_window_realm().unwrap();
    runtime.run_page_init();
    assert!(runtime.validate_cdp_world(utility).is_err());
    assert!(runtime.object_world(&object).is_err());
    assert_ne!(world(&mut runtime, 0, "utility"), utility);
}

#[test]
fn all_worlds_participate_in_one_cancelable_activation() {
    let mut runtime = page();
    let utility = world(&mut runtime, 0, "utility");
    runtime.evaluate("globalThis.authorClicks = 0; document.getElementById('check').addEventListener('click', () => authorClicks++)").unwrap();
    runtime.evaluate_in_cdp_frame(utility, "globalThis.utilityClicks = 0; document.getElementById('check').addEventListener('click', event => { utilityClicks++; event.preventDefault(); }, {once:true})").unwrap();
    runtime.evaluate("document.getElementById('check').click()").unwrap();
    assert_eq!(runtime.evaluate("[authorClicks, document.getElementById('check').checked]").unwrap(), json!([1, false]));
    assert_eq!(runtime.evaluate_in_cdp_frame(utility, "utilityClicks").unwrap().as_f64(), Some(1.0));
    runtime.evaluate("document.getElementById('check').click()").unwrap();
    assert_eq!(runtime.evaluate("[authorClicks, document.getElementById('check').checked]").unwrap(), json!([2, true]));
    assert_eq!(runtime.evaluate_in_cdp_frame(utility, "utilityClicks").unwrap().as_f64(), Some(1.0));
}

#[test]
fn child_world_disposal_does_not_dispose_its_canonical_document() {
    let mut runtime = page();
    let child = FrameRealm::new(&mut runtime, 41, 0, "https://child.example/document", "<html><body><p id='child'>child</p></body></html>").unwrap();
    let utility = world(&mut runtime, 41, "utility");
    runtime.evaluate_in_cdp_frame(utility, "globalThis.marker = 'utility'; document.getElementById('child').textContent = 'shared child'").unwrap();
    assert_eq!(child.evaluate(&mut runtime, "[typeof marker, document.getElementById('child').textContent, location.origin]").unwrap(), json!(["undefined", "shared child", "https://child.example"]));
    runtime.dispose_isolated_world(utility.world_id.unwrap()).unwrap();
    assert!(runtime.validate_cdp_world(utility).is_err());
    assert_eq!(child.evaluate(&mut runtime, "document.getElementById('child').textContent").unwrap(), json!("shared child"));
    let fresh = world(&mut runtime, 41, "utility");
    drop(child);
    assert!(runtime.validate_cdp_world(fresh).is_err());
}

#[test]
fn explicit_isolated_grant_upgrades_only_live_related_dom_access() {
    let mut runtime = page();
    let child = FrameRealm::new(&mut runtime, 42, 0, "https://child.example/document", "<html><body>child</body></html>").unwrap();
    let inspect = "(() => { try { return parent.document.getElementById('p').textContent; } catch (error) { return error.name; } })()";
    let utility = world(&mut runtime, 42, "dom-access");
    let ordinary = world(&mut runtime, 42, "ordinary");
    assert_eq!(child.evaluate(&mut runtime, inspect).unwrap(), json!("SecurityError"));
    runtime.evaluate_in_cdp_frame(utility, "globalThis.grantUniveralAccess = true; globalThis.__obscura_frameId = 0").unwrap();
    assert_eq!(runtime.evaluate_in_cdp_frame(utility, inspect).unwrap(), json!("SecurityError"));
    assert_eq!(runtime.ensure_isolated_world(42, "dom-access", true).unwrap(), utility.world_id.unwrap());
    assert_eq!(runtime.evaluate_in_cdp_frame(utility, inspect).unwrap(), json!("original"));
    runtime.evaluate("globalThis.authorSecret = 'author'").unwrap();
    assert_eq!(runtime.evaluate_in_cdp_frame(utility, r#"[
        parent.document.defaultView === parent,
        ...[
            () => parent.authorSecret,
            () => parent.eval,
            () => parent.Function,
            () => parent.document.defaultView.authorSecret,
            () => { parent.authorSecret = () => 'overwritten'; },
        ].map(probe => { try { probe(); return 'allowed'; } catch (error) { return error.name; } }),
    ]"#).unwrap(), json!([true, "SecurityError", "SecurityError", "SecurityError", "SecurityError", "SecurityError"]));
    assert_eq!(runtime.evaluate("authorSecret").unwrap(), json!("author"));
    assert_eq!(child.evaluate(&mut runtime, inspect).unwrap(), json!("SecurityError"));
    assert_eq!(runtime.evaluate_in_cdp_frame(ordinary, inspect).unwrap(), json!("SecurityError"));
    assert_eq!(runtime.evaluate("open('', 'unrelated', 'noopener')").unwrap(), json!(null));
    let unrelated_id = runtime.take_created_windows().pop().unwrap();
    let mut unrelated = runtime.for_window(unrelated_id).unwrap();
    unrelated.evaluate("document.body.textContent = 'unrelated secret'").unwrap();
    // A granted world still cannot resolve a named window in another group.
    assert_eq!(runtime.evaluate_in_cdp_frame(utility, "open('', 'unrelated').document.body.textContent").unwrap(), json!(""));
    let related_id = runtime.take_created_windows().pop().unwrap();
    assert_ne!(related_id, unrelated_id);
    assert_eq!(unrelated.evaluate("document.body.textContent").unwrap(), json!("unrelated secret"));
    runtime.evaluate_in_cdp_frame(utility, "parent.savedInspector = () => { try { return parent.document.getElementById('p').textContent; } catch (error) { return error.name; } }").unwrap();
    assert_eq!(runtime.evaluate("savedInspector()").unwrap(), json!("original"));
    runtime.dispose_isolated_world(utility.world_id.unwrap()).unwrap();
    assert_eq!(runtime.evaluate("savedInspector()").unwrap(), json!("SecurityError"));
    assert!(runtime.validate_cdp_world(utility).is_err());
    let fresh = world(&mut runtime, 42, "dom-access");
    assert_eq!(runtime.evaluate_in_cdp_frame(fresh, inspect).unwrap(), json!("SecurityError"));
}

#[tokio::test(flavor = "current_thread")]
async fn geolocation_permission_and_position_watches_follow_the_native_document() {
    use obscura_js::geolocation::{Decision, Permissions, Position};
    use std::sync::Arc;
    let mut runtime = page();
    runtime.set_url("https://world.example/page");
    let permissions = Arc::new(Permissions::default());
    runtime.set_geolocation_context(permissions.clone(),Some(Position::new(58.4167,15.6167,76_000.0).unwrap()));
    let world = CdpExecutionWorld::default();
    let prompt = runtime.evaluate_for_cdp_in_frame(world,
        "navigator.permissions.query({name:'geolocation'}).then(status => {globalThis.locationPermission=status;return status.state})", true,true,2_000).await.unwrap();
    assert_eq!(prompt.value,Some(json!("prompt")));
    let denied = runtime.evaluate_for_cdp_in_frame(world,
        "new Promise(resolve => {let synchronous=true;navigator.geolocation.getCurrentPosition(() => resolve('leaked'),error => resolve([error.code,synchronous]));synchronous=false})", true,true,2_000).await.unwrap();
    assert_eq!(denied.value,Some(json!([1,false])));
    runtime.execute_script("observe-permission","globalThis.permissionChange=new Promise(resolve=>locationPermission.addEventListener('change',()=>resolve(locationPermission.state),{once:true}))").unwrap();
    permissions.set(Some("https://world.example"),Decision::Granted).unwrap();
    let changed = runtime.evaluate_for_cdp_in_frame(world,"permissionChange",true,true,2_000).await.unwrap();
    assert_eq!(changed.value,Some(json!("granted")));
    let first = runtime.evaluate_for_cdp_in_frame(world,
        "new Promise(resolve=>{globalThis.locationCalls=0;globalThis.locationWatch=navigator.geolocation.watchPosition(position=>{locationCalls++;if(locationCalls===1)resolve(position.toJSON());else if(globalThis.nextPosition)nextPosition(position.coords.latitude)},error=>resolve({error:error.code}))})",true,true,2_000).await.unwrap();
    assert_eq!(first.value.as_ref().unwrap()["coords"]["accuracy"].as_f64(),Some(76_000.0));
    assert_eq!(first.value.as_ref().unwrap()["coords"]["latitude"].as_f64(),Some(58.4167));
    runtime.execute_script("next-position","globalThis.positionChanged=new Promise(resolve=>globalThis.nextPosition=resolve)").unwrap();
    runtime.set_geolocation(Some(Position::new(59.0,16.0,80_000.0).unwrap())).unwrap();
    let updated = runtime.evaluate_for_cdp_in_frame(world,"positionChanged",true,true,2_000).await.unwrap();
    assert_eq!(updated.value.and_then(|value| value.as_f64()),Some(59.0));
    runtime.evaluate("navigator.geolocation.clearWatch(locationWatch)").unwrap();
    runtime.set_geolocation(Some(Position::new(60.0,17.0,80_000.0).unwrap())).unwrap();
    let cleared = runtime.evaluate_for_cdp_in_frame(world,"new Promise(resolve=>setTimeout(()=>resolve(locationCalls),0))",true,true,2_000).await.unwrap();
    assert_eq!(cleared.value.and_then(|value| value.as_f64()),Some(2.0));
    permissions.reset().unwrap();
    let reset = runtime.evaluate_for_cdp_in_frame(world,"new Promise(resolve=>navigator.geolocation.getCurrentPosition(()=>resolve('leaked'),error=>resolve([locationPermission.state,error.code])))",true,true,2_000).await.unwrap();
    assert_eq!(reset.value,Some(json!(["prompt",1])));
}
