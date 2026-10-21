use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicU64, Ordering};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use crate::server::{CdpMessage, ServerMessage};

/// Send-only control-plane directory. Native pages never leave their owner thread.
#[derive(Default)]
pub(crate) struct BrowserRegistry {
    state: Mutex<State>,
    serial: AtomicU64,
    network_tx: std::sync::OnceLock<mpsc::UnboundedSender<(String, obscura_js::ops::JsNetworkEvent)>>,
}
#[derive(Default)]
struct State {
    owners: HashMap<u64, mpsc::UnboundedSender<ServerMessage>>,
    targets: BTreeMap<String, (u64, Value)>,
    contexts: HashMap<String, u64>,
    clients: HashMap<u64, Client>,
    sessions: HashMap<String, Session>,
    pending: HashMap<u64, Pending>,
    network_sessions: HashSet<String>,
    local_requests: HashSet<(String, String)>,
    body_views: HashMap<String, HashSet<String>>,
    frame_routes: HashMap<String, (String, String, String)>,
    frame_owners: HashMap<String, FrameOwners>,
    page_sessions: HashSet<String>,
    announced_frames: HashSet<(String, String)>,
    live_requests: HashMap<String, LiveRequest>,
    live_order: std::collections::VecDeque<String>,
    redirects: HashMap<String, Value>,
    fetch_sessions: HashMap<String, String>,
    pauses: HashMap<String, (String, String, tokio::sync::oneshot::Sender<obscura_js::ops::InterceptResolution>)>,
    auto_attach: HashMap<(u64, Option<String>), crate::dispatch::AutoAttachSubscription>,
}
struct Client {
    home: u64,
    reply: ClientReply,
    implicit: Option<String>,
    discovery: HashSet<Option<String>>,
}

#[derive(Clone)]
struct ClientReply {
    sender: mpsc::Sender<String>,
    disconnect: Arc<tokio::sync::Notify>,
}
impl ClientReply {
    fn send(&self, text: String) -> Result<(), ()> {
        self.sender.try_send(text).map_err(|_| { self.disconnect.notify_one(); })
    }
}
struct Session { client: u64, owner: u64, target: String }
struct Pending { client: u64, owner: u64, id: u64, session: Option<String>, remaining: usize }
struct LiveRequest {
    target: String,
    wire_id: String,
    frame_id: String,
    loader_id: String,
    document_url: String,
    finished: bool,
}
#[derive(Default)]
struct FrameOwners {
    root_generation: u64,
    high_water: u32,
    live: HashMap<u32, u64>,
}
impl BrowserRegistry {
    fn send_session(state: &State, session: &str, mut event: Value) {
        let Some(route) = state.sessions.get(session) else { return };
        let Some(client) = state.clients.get(&route.client) else { return };
        if client.implicit.as_deref() != Some(session) { event["sessionId"] = json!(session); }
        let _ = client.reply.send(event.to_string());
    }

    pub(crate) fn subscriptions(&self, session: &str, network: bool) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if network { state.network_sessions.insert(session.into()); }
        else { state.network_sessions.remove(session); state.body_views.remove(session); }
    }

    pub(crate) fn body_visible(&self, session: &str, request: &str) -> bool {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.network_sessions.contains(session) && state.body_views.get(session).is_some_and(|view| view.contains(request))
    }
    pub(crate) fn native_request_id(&self, target: &str, wire: &str) -> Option<String> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.live_order.iter().rev().find(|id| state.live_requests.get(*id)
            .is_some_and(|receipt| receipt.target == target && receipt.wire_id == wire && receipt.finished)).cloned()
    }

    pub(crate) fn has_target(&self, target: &str) -> bool {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).targets.contains_key(target)
    }
    pub(crate) fn frame_owners(&self, target: &str, root: u64, frames: Vec<(u32, u32, u64)>) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let owners = state.frame_owners.entry(target.into()).or_default();
        if root > owners.root_generation { owners.high_water = 0; }
        owners.root_generation = root;
        owners.live.clear();
        owners.live.insert(0, root);
        for (frame, _, generation) in frames {
            owners.high_water = owners.high_water.max(frame);
            owners.live.insert(frame, generation);
        }
    }

    pub(crate) fn page_subscription(&self, session: &str, enabled: bool) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if enabled { state.page_sessions.insert(session.into()); }
        else if state.page_sessions.remove(session) { state.announced_frames.retain(|(owner, _)| owner != session); }
    }

    pub(crate) fn page_route(&self, target: &str, frame: &str, loader: &str, url: &str) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).frame_routes.insert(target.into(), (frame.into(), loader.into(), url.into()));
    }

    pub(crate) fn network_events(self: &Arc<Self>) -> mpsc::UnboundedSender<(String, obscura_js::ops::JsNetworkEvent)> {
        self.network_tx.get_or_init(|| {
            let (tx, mut rx) = mpsc::unbounded_channel::<(String, obscura_js::ops::JsNetworkEvent)>();
            let registry = Arc::downgrade(self);
            std::thread::Builder::new().name("obscura-cdp-network-events".into()).spawn(move || {
                while let Some((page, event)) = rx.blocking_recv() {
                    let Some(registry) = registry.upgrade() else { break };
                    registry.network_event(&page, event);
                }
            }).expect("CDP network event router thread");
            tx
        }).clone()
    }

    fn network_event(&self, page: &str, event: obscura_js::ops::JsNetworkEvent) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        // Fulfillment completes a real intercepted request without a transport
        // start. Its request lifecycle still precedes the response lifecycle.
        let starts_lifecycle = event.request_started
            || (event.intercepted && !state.live_requests.contains_key(&event.request_id));
        let mut receipt = if let Some(receipt) = state.live_requests.remove(&event.request_id) {
            if event.request_started || receipt.finished {
                state.live_requests.insert(event.request_id, receipt);
                return;
            }
            receipt
        } else {
            let Some((main, loader, document_url)) = state.frame_routes.get(page).cloned() else { return };
            let frame_id = if event.frame_id == 0 { main } else { crate::domains::page::child_frame_id(&main, event.frame_id) };
            let loader_id = if event.frame_id == 0 { loader } else { format!("{frame_id}-loader") };
            let wire_id = if event.resource_type == "Document" && event.frame_id == 0 { loader_id.clone() } else { event.request_id.clone() };
            state.live_order.push_back(event.request_id.clone());
            while state.live_order.len() > 4096 {
                if let Some(old) = state.live_order.pop_front() {
                    if let Some(receipt) = state.live_requests.remove(&old) {
                        for view in state.body_views.values_mut() { view.remove(&old); view.remove(&receipt.wire_id); }
                    }
                }
            }
            LiveRequest { target:page.into(), wire_id, frame_id, loader_id, document_url, finished:false }
        };
        receipt.finished = !event.request_started;
        let id = &receipt.wire_id;
        let frame = &receipt.frame_id;
        let loader = &receipt.loader_id;
        let document = &receipt.document_url;
        let sessions = state.network_sessions.iter().filter(|session|
            state.sessions.get(*session).is_some_and(|route| route.target == page)).cloned().collect::<Vec<_>>();
        let mut events = Vec::new();
        if starts_lifecycle {
            let mut request = json!({"url": event.url, "method": event.method, "headers": event.request_headers,
                "hasPostData": event.request_body.is_some()});
            if let Some(body) = &event.request_body {
                if let Ok(text) = std::str::from_utf8(body) { request["postData"] = json!(text); }
                use base64::Engine;
                request["postDataEntries"] = json!([{"bytes":base64::engine::general_purpose::STANDARD.encode(body)}]);
            }
            let redirect = state.redirects.remove(id);
            events.push(json!({"method":"Network.requestWillBeSent", "params":{
                "requestId":id,"loaderId":loader,"documentURL":document,"request":request,
                "timestamp":event.timestamp,"wallTime":event.timestamp,"initiator":{"type":"other"},
                "type":event.resource_type,"frameId":frame}}));
            if let Some(redirect) = redirect { events.last_mut().unwrap()["params"]["redirectResponse"] = redirect; }
            if event.request_started {
                events.push(json!({"method":"Network.requestWillBeSentExtraInfo", "params":{
                    "requestId":id,"associatedCookies":[],"headers":event.request_headers,
                    "connectTiming":{"requestTime":event.timestamp}}}));
            }
        }
        if !event.request_started {
            if let Some(error) = event.error_text {
                events.push(json!({"method":"Network.loadingFailed", "params":{
                    "requestId":id,"timestamp":event.timestamp,"type":event.resource_type,"errorText":error,
                    "canceled":error == "net::ERR_ABORTED"}}));
            } else if (300..400).contains(&event.status) && event.response_headers.contains_key("location") {
                state.redirects.insert(id.clone(), json!({"url":event.url,"status":event.status,"statusText":"",
                    "headers":event.response_headers,"mimeType":event.response_headers.get("content-type").cloned().unwrap_or_default()}));
                state.live_requests.insert(event.request_id, receipt);
                return;
            } else {
                events.push(json!({"method":"Network.responseReceived", "params":{
                    "requestId":id,"loaderId":loader,"timestamp":event.timestamp,"type":event.resource_type,
                    "response":{"url":event.url,"status":event.status,"statusText":"","headers":event.response_headers,
                        "mimeType":event.response_headers.get("content-type").cloned().unwrap_or_default()},"frameId":frame}}));
                events.push(json!({"method":"Network.loadingFinished", "params":{
                    "requestId":id,"timestamp":event.timestamp,"encodedDataLength":event.body_size}}));
            }
        }
        for session in sessions {
            if !event.request_started {
                state.body_views.entry(session.clone()).or_default().insert(id.clone());
            }
            for value in &events { Self::send_session(&state, &session, value.clone()); }
        }
        state.live_requests.insert(event.request_id, receipt);
    }

    pub(crate) fn intercept(self: &Arc<Self>, request: obscura_js::ops::InterceptedRequest) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let current = state.frame_owners.get_mut(&request.page_id).is_some_and(|owners| {
            match owners.live.get(&request.frame_id).copied() {
                Some(generation) if generation <= request.frame_generation => {
                    owners.live.insert(request.frame_id, request.frame_generation);
                    true
                }
                None if request.frame_id > owners.high_water && owners.live.contains_key(&request.parent_frame_id.unwrap_or(0)) => {
                    owners.high_water = request.frame_id;
                    owners.live.insert(request.frame_id, request.frame_generation);
                    true
                }
                _ => false,
            }
        });
        if !current {
            let _ = request.resolver.send(obscura_js::ops::InterceptResolution::Fail { reason:"Aborted".into() });
            return;
        }
        let Some(session) = state.fetch_sessions.get(&request.page_id).cloned() else {
            let _ = request.resolver.send(obscura_js::ops::InterceptResolution::Continue { url:None,method:None,headers:None,body:None });
            return;
        };
        let Some((main, _, _)) = state.frame_routes.get(&request.page_id).cloned() else { return };
        let frame = if request.frame_id == 0 { main.clone() } else { crate::domains::page::child_frame_id(&main, request.frame_id) };
        if state.pauses.len() >= 4096 {
            let _ = request.resolver.send(obscura_js::ops::InterceptResolution::Fail { reason:"Aborted".into() });
            return;
        }
        if request.frame_id != 0 {
            let parent = request.parent_frame_id.unwrap_or(0);
            let parent = if parent == 0 { main } else { crate::domains::page::child_frame_id(&main, parent) };
            let sessions = state.page_sessions.iter().filter(|session| state.sessions.get(*session)
                .is_some_and(|route| route.target == request.page_id)).cloned().collect::<Vec<_>>();
            for owner in sessions {
                if state.announced_frames.insert((owner.clone(), frame.clone())) {
                    Self::send_session(&state, &owner, json!({"method":"Page.frameAttached","params":{"frameId":frame,"parentFrameId":parent}}));
                }
            }
        }
        let mut payload = json!({"url":request.url,"method":request.method,"headers":request.headers,"hasPostData":request.body.is_some()});
        if let Some(body) = &request.body {
            use base64::Engine;
            payload["postDataEntries"] = json!([{"bytes":base64::engine::general_purpose::STANDARD.encode(body)}]);
            if let Ok(text) = std::str::from_utf8(body) { payload["postData"] = json!(text); }
        }
        let event = json!({"method":"Fetch.requestPaused", "params":{
            "requestId":request.request_id,"request":payload,
            "frameId":frame,"resourceType":request.resource_type,"networkId":request.request_id}});
        Self::send_session(&state, &session, event);
        state.pauses.insert(request.request_id, (session, request.page_id, request.resolver));
    }

    pub(crate) fn fetch_subscription(&self, target: &str, session: Option<&str>) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(session) = session { state.fetch_sessions.insert(target.into(), session.into()); }
        else {
            state.fetch_sessions.remove(target);
            let requests = state.pauses.iter().filter(|(_, (_, page, _))| page == target).map(|(id, _)| id.clone()).collect::<Vec<_>>();
            for id in requests {
                if let Some((_, _, resolver)) = state.pauses.remove(&id) {
                    let _ = resolver.send(obscura_js::ops::InterceptResolution::Continue { url:None,method:None,headers:None,body:None });
                }
            }
        }
    }

    fn resolve_fetch(&self, client: u64, request: &Value) -> bool {
        let method = request["method"].as_str().unwrap_or_default();
        if !matches!(method, "Fetch.continueRequest" | "Fetch.fulfillRequest" | "Fetch.failRequest") { return false; }
        let id = request["params"]["requestId"].as_str().unwrap_or_default();
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let actor = request["sessionId"].as_str().or_else(|| state.clients.get(&client).and_then(|client| client.implicit.as_deref()));
        let owned = state.pauses.get(id).is_some_and(|(session, _, _)| Some(session.as_str()) == actor
            && state.sessions.get(session).is_some_and(|route| route.client == client));
        let resolution = if owned { crate::server::decode_fetch_resolution(method, &request["params"]) }
            else { Err(format!("No paused request found for requestId {id}")) };
        let mut answer = match resolution {
            Ok(resolution) => {
                let (_, _, resolver) = state.pauses.remove(id).unwrap();
                let _ = resolver.send(resolution);
                json!({"id":request["id"],"result":{}})
            }
            Err(message) => json!({"id":request["id"],"error":{"code":-32602,"message":message}}),
        };
        if request["sessionId"].is_string() { answer["sessionId"] = request["sessionId"].clone(); }
        if let Some(client) = state.clients.get(&client) { let _ = client.reply.send(answer.to_string()); }
        true
    }
    pub(crate) fn next_id(&self) -> u64 { self.serial.fetch_add(1, Ordering::Relaxed) + 1 }
    pub(crate) fn add_owner(&self, id: u64, sender: mpsc::UnboundedSender<ServerMessage>) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).owners.insert(id, sender);
    }
    pub(crate) fn auto_attach_subscriptions(&self) -> Vec<crate::dispatch::AutoAttachSubscription> {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).auto_attach.values().cloned().collect()
    }
    pub(crate) fn detach_auto_attach_parent(&self, parent: &str) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let keys = state.auto_attach.keys().filter(|(_, session)| session.as_deref() == Some(parent))
            .cloned().collect::<Vec<_>>();
        for key in keys {
            if let Some(subscription) = state.auto_attach.remove(&key) {
                for sender in state.owners.values() {
                    let _ = sender.send(ServerMessage::SetAutoAttach {
                        subscription: subscription.clone(), enabled: false, acknowledgement: None,
                    });
                }
            }
        }
    }
    pub(crate) fn owner_count(&self) -> usize {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).owners.len()
    }
    pub(crate) fn remove_owner(&self, id: u64) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.owners.remove(&id);
        state.contexts.retain(|_, owner| *owner != id);
        state.targets.retain(|_, (owner, _)| *owner != id);
        state.sessions.retain(|_, route| route.owner != id);
    }
    pub(crate) fn add_client(&self, id: u64, home: u64, sender: mpsc::Sender<String>, disconnect: Arc<tokio::sync::Notify>) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).clients.insert(id,
            Client { home, reply: ClientReply { sender, disconnect }, implicit: None, discovery: HashSet::new() });
    }
    pub(crate) fn publish(&self, owner: u64, targets: Vec<Value>, contexts: Vec<String>) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        // Pages temporarily owned by a navigation task stay discoverable until
        // an explicit destruction receipt; absence from the local Vec is not close.
        for info in targets {
            let Some(id) = info["targetId"].as_str().map(str::to_string) else { continue };
            let method = match state.targets.get(&id) {
                None => Some("Target.targetCreated"),
                Some((_, previous)) if previous != &info => Some("Target.targetInfoChanged"),
                _ => None,
            };
            state.targets.insert(id, (owner, info.clone()));
            if let Some(method) = method { Self::discovery(&state, method, json!({"targetInfo": info})); }
        }
        state.contexts.retain(|_, existing| *existing != owner);
        for context in contexts { state.contexts.insert(context, owner); }
    }
    fn discovery(state: &State, method: &str, params: Value) {
        for client in state.clients.values() {
            for session in &client.discovery {
                let mut event = json!({"method": method, "params": params});
                if let Some(session) = session { event["sessionId"] = json!(session); }
                let _ = client.reply.send(event.to_string());
            }
        }
    }
    pub(crate) fn target_list(&self, authority: &str) -> Vec<Value> {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).targets.iter().map(|(id, (_, info))| json!({
            "description": "", "devtoolsFrontendUrl": "", "id": id, "title": info["title"],
            "type": "page", "url": info["url"],
            "webSocketDebuggerUrl": format!("ws://{authority}/devtools/page/{id}"),
        })).collect()
    }
    pub(crate) fn bind_page(&self, client: u64, target: &str) -> Result<(), String> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let owner = state.targets.get(target).map(|(owner, _)| *owner).ok_or("Target not found")?;
        let session = format!("endpoint-{}", self.next_id());
        state.clients.get_mut(&client).ok_or("Connection closed")?.implicit = Some(session.clone());
        state.sessions.insert(session.clone(), Session { client, owner, target: target.into() });
        state.owners.get(&owner).ok_or("Target owner closed")?.send(ServerMessage::BindSession {
            session, target: target.into(),
        }).map_err(|_| "Target owner closed".to_string())
    }
    pub(crate) fn session_client(&self, session: &str) -> Option<u64> {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).sessions.get(session).map(|s| s.client)
    }
    pub(crate) fn register_session(&self, session: &str, target: &str, owner: u64, client: u64) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.clients.contains_key(&client) {
            state.sessions.entry(session.into()).or_insert_with(|| Session { client, owner, target: target.into() });
        }
    }
    pub(crate) fn command_client(&self, id: u64) -> Option<u64> {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).pending.get(&id).map(|p| p.client)
    }
    pub(crate) fn route(&self, client_id: u64, text: &str, output: &mpsc::UnboundedSender<String>) {
        let Ok(mut req) = serde_json::from_str::<Value>(text) else { return };
        let Some(id) = req["id"].as_u64() else { return };
        let Some(method) = req["method"].as_str().map(str::to_string) else { return };
        if self.resolve_fetch(client_id, &req) { return; }
        let original_session = req["sessionId"].as_str().map(str::to_string);
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let Some(client) = state.clients.get(&client_id) else { return };
        let reply = client.reply.clone();
        let home = client.home;
        let implicit = client.implicit.clone();
        let answer = |result: Result<Value, String>| {
            let mut response = match result { Ok(result) => json!({"id": id, "result": result}),
                Err(message) => json!({"id": id, "error": {"code": -32000, "message": message}}) };
            if let Some(session) = &original_session { response["sessionId"] = json!(session); }
            let _ = reply.send(response.to_string());
        };
        match method.as_str() {
            "Target.setAutoAttach" => {
                let target = if let Some(session) = &original_session {
                    let Some(route) = state.sessions.get(session).filter(|route| route.client == client_id) else {
                        answer(Err("Target or session not found".into())); return;
                    };
                    (route.target != "browser").then(|| route.target.clone())
                } else { None };
                let subscription = crate::dispatch::AutoAttachSubscription {
                    client: client_id, parent: original_session.clone(), target,
                    wait_for_debugger: req["params"]["waitForDebuggerOnStart"].as_bool().unwrap_or(false),
                };
                let key = (client_id, original_session.clone());
                let enabled = req["params"]["autoAttach"].as_bool().unwrap_or(false);
                if enabled { state.auto_attach.insert(key, subscription.clone()); }
                else { state.auto_attach.remove(&key); }
                let routed = self.next_id();
                let remaining = state.owners.values().filter(|sender| sender.send(ServerMessage::SetAutoAttach {
                    subscription: subscription.clone(), enabled, acknowledgement: Some(routed),
                }).is_ok()).count();
                if remaining == 0 { answer(Err("Target owner closed".into())); }
                else { state.pending.insert(routed, Pending { client: client_id, owner: home, id,
                    session: original_session, remaining }); }
                return;
            }
            "Target.getTargets" => { answer(Ok(json!({"targetInfos": state.targets.values().map(|(_, info)| info).collect::<Vec<_>>()}))); return; }
            "Target.getBrowserContexts" => { answer(Ok(json!({"browserContextIds": state.contexts.keys().collect::<Vec<_>>()}))); return; }
            "Target.getTargetInfo" if req["params"]["targetId"].is_string() => {
                let result = state.targets.get(req["params"]["targetId"].as_str().unwrap())
                    .map(|(_, info)| json!({"targetInfo": info})).ok_or_else(|| "Target not found".into());
                answer(result); return;
            }
            "Target.setDiscoverTargets" => {
                if req["params"]["discover"].as_bool() == Some(true) {
                    state.clients.get_mut(&client_id).unwrap().discovery.insert(original_session.clone());
                    for (_, info) in state.targets.values() {
                        let mut event = json!({"method":"Target.targetCreated", "params":{"targetInfo":info}});
                        if let Some(session) = &original_session { event["sessionId"] = json!(session); }
                        let _ = reply.send(event.to_string());
                    }
                } else { state.clients.get_mut(&client_id).unwrap().discovery.remove(&original_session); }
                answer(Ok(json!({}))); return;
            }
            _ => {}
        }
        let session = original_session.clone().or_else(|| {
            (!method.starts_with("Target.") && !method.starts_with("Browser.")).then_some(implicit).flatten()
        });
        let target = req["params"]["targetId"].as_str();
        let context = req["params"]["browserContextId"].as_str();
        let owner = if let Some(target) = target {
            state.targets.get(target).map(|(owner, _)| *owner)
        } else if let Some(context) = context {
            state.contexts.get(context).copied()
        } else if let Some(session) = session.as_ref() {
            state.sessions.get(session).filter(|s| s.client == client_id).map(|s| s.owner)
        } else if let Some(session) = req["params"]["sessionId"].as_str() {
            state.sessions.get(session).filter(|s| s.client == client_id).map(|s| s.owner)
        } else { Some(home) };
        let Some(owner) = owner else { answer(Err("Target or session not found".into())); return };
        let Some(sender) = state.owners.get(&owner).cloned() else { answer(Err("Target owner closed".into())); return };
        if state.pending.values().filter(|p| p.client == client_id || p.owner == owner).count() >= 256 {
            answer(Err("Too many pending commands".into())); return;
        }
        if let Some(session) = session { req["sessionId"] = json!(session); }
        let routed = self.next_id();
        req["id"] = json!(routed);
        if sender.send(ServerMessage::Cdp(CdpMessage { text: req.to_string(), reply_tx: output.clone() })).is_err() {
            answer(Err("Target owner closed".into()));
            return;
        }
        state.pending.insert(routed, Pending { client: client_id, owner, id, session: original_session, remaining: 1 });
    }
    pub(crate) fn forward(&self, text: String) {
        let Ok(mut value) = serde_json::from_str::<Value>(&text) else { return };
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(id) = value["id"].as_u64() {
            if let Some(pending) = state.pending.get_mut(&id) {
                if pending.remaining > 1 { pending.remaining -= 1; return; }
            }
            let Some(pending) = state.pending.remove(&id) else { return };
            value["id"] = json!(pending.id);
            value.as_object_mut().unwrap().remove("sessionId");
            if let Some(session) = pending.session { value["sessionId"] = json!(session); }
            if let Some(client) = state.clients.get(&pending.client) { let _ = client.reply.send(value.to_string()); }
            return;
        }
        let method = value["method"].as_str().unwrap_or_default();
        if matches!(method, "Page.frameAttached" | "Page.frameDetached") {
            if let (Some(session), Some(frame)) = (value["sessionId"].as_str(), value["params"]["frameId"].as_str()) {
                let key = (session.to_string(), frame.to_string());
                if method == "Page.frameAttached" {
                    if !state.announced_frames.insert(key) { return; }
                } else { state.announced_frames.remove(&key); }
            }
        }
        if method == "Target.targetDestroyed" {
            if let Some(target) = value["params"]["targetId"].as_str() {
                state.frame_owners.remove(target);
                if state.targets.remove(target).is_some() { Self::discovery(&state, method, value["params"].clone()); }
                state.frame_routes.remove(target);
                state.fetch_sessions.remove(target);
                let requests = state.live_requests.iter().filter(|(_, receipt)| receipt.target == target)
                    .map(|(id, _)| id.clone()).collect::<Vec<_>>();
                for request in requests {
                    if let Some(receipt) = state.live_requests.remove(&request) { state.redirects.remove(&receipt.wire_id); }
                }
                let sessions = state.sessions.iter().filter(|(_, route)| route.target == target)
                    .map(|(session, _)| session.clone()).collect::<Vec<_>>();
                for session in sessions {
                    let keys = state.auto_attach.keys().filter(|(_, parent)| parent.as_ref() == Some(&session))
                        .cloned().collect::<Vec<_>>();
                    for key in keys {
                        if let Some(subscription) = state.auto_attach.remove(&key) {
                            for sender in state.owners.values() {
                                let _ = sender.send(ServerMessage::SetAutoAttach {
                                    subscription: subscription.clone(), enabled: false, acknowledgement: None,
                                });
                            }
                        }
                    }
                    state.network_sessions.remove(&session); state.body_views.remove(&session);
                    state.page_sessions.remove(&session);
                    state.announced_frames.retain(|(owner, _)| owner != &session);
                    state.local_requests.retain(|(owner, _)| owner != &session);
                }
                let paused = state.pauses.iter().filter(|(_, (_, page, _))| page == target).map(|(id, _)| id.clone()).collect::<Vec<_>>();
                for request in paused {
                    if let Some((_, _, resolver)) = state.pauses.remove(&request) {
                        let _ = resolver.send(obscura_js::ops::InterceptResolution::Fail { reason:"Aborted".into() });
                    }
                }
                state.sessions.retain(|_, session| session.target != target);
            }
            return;
        }
        if matches!(method, "Target.targetCreated" | "Target.targetInfoChanged") { return; }
        if method.starts_with("Network.") {
            let id = value["params"]["requestId"].as_str().unwrap_or_default();
            let session = value["sessionId"].as_str().unwrap_or_default();
            let key = (session.to_string(), id.to_string());
            if method == "Network.requestWillBeSent" {
                let url = value["params"]["request"]["url"].as_str().unwrap_or_default();
                // HTTP receipts come exclusively from the send-stage producer.
                // Snapshot completion ordering must never fabricate a sent request.
                if url.starts_with("http://") || url.starts_with("https://") { return; }
                state.local_requests.insert(key);
            } else if matches!(method, "Network.responseReceived" | "Network.loadingFinished" | "Network.loadingFailed" | "Network.requestWillBeSentExtraInfo") {
                if !state.local_requests.contains(&key) { return; }
                if matches!(method, "Network.loadingFinished" | "Network.loadingFailed") { state.local_requests.remove(&key); }
            }
        }
        let session = value["sessionId"].as_str().or_else(|| value["params"]["sessionId"].as_str());
        let Some(session) = session else { return };
        let Some(route) = state.sessions.get(session) else { return };
        let Some(client) = state.clients.get(&route.client) else { return };
        if value["sessionId"].as_str() == client.implicit.as_deref() { value.as_object_mut().unwrap().remove("sessionId"); }
        let _ = client.reply.send(value.to_string());
        if value["method"] == "Target.detachedFromTarget" {
            if let Some(session) = value["params"]["sessionId"].as_str() { state.sessions.remove(session); }
        }
    }
    pub(crate) fn disconnect(&self, id: u64) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(client) = state.clients.remove(&id) {
            if let Some(sender) = state.owners.get(&client.home) { let _ = sender.send(ServerMessage::ClientDisconnected); }
        }
        state.pending.retain(|_, pending| pending.client != id);
        let keys = state.auto_attach.keys().filter(|(client, _)| *client == id).cloned().collect::<Vec<_>>();
        for key in keys {
            if let Some(subscription) = state.auto_attach.remove(&key) {
                for sender in state.owners.values() {
                    let _ = sender.send(ServerMessage::SetAutoAttach {
                        subscription: subscription.clone(), enabled: false, acknowledgement: None,
                    });
                }
            }
        }
        let sessions = state.sessions.iter().filter(|(_, route)| route.client == id)
            .map(|(session, route)| (session.clone(), route.owner)).collect::<Vec<_>>();
        for (session, owner) in sessions {
            if let Some(sender) = state.owners.get(&owner) { let _ = sender.send(ServerMessage::DetachSession(session.clone())); }
            state.sessions.remove(&session);
            state.network_sessions.remove(&session);
            state.body_views.remove(&session);
            state.local_requests.retain(|(owner, _)| owner != &session);
            state.page_sessions.remove(&session);
            state.announced_frames.retain(|(owner, _)| owner != &session);
            state.fetch_sessions.retain(|_, owner| owner != &session);
            let paused = state.pauses.iter().filter(|(_, (owner, _, _))| owner == &session)
                .map(|(id, _)| id.clone()).collect::<Vec<_>>();
            for id in paused {
                if let Some((_, _, resolver)) = state.pauses.remove(&id) {
                    let _ = resolver.send(obscura_js::ops::InterceptResolution::Fail { reason:"Aborted".into() });
                }
            }
        }
    }
    pub(crate) fn output(self: &Arc<Self>) -> mpsc::UnboundedSender<String> {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let registry = self.clone();
        std::thread::Builder::new().name("obscura-cdp-router".into()).spawn(move || {
            while let Some(text) = rx.blocking_recv() { registry.forward(text); }
        }).expect("CDP output router thread");
        tx
    }
}
