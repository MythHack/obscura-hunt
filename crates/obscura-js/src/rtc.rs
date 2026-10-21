//! Document-owned, data-only WebRTC. Protocol state and packets are owned by
//! webrtc-rs; JavaScript only implements WebIDL wrappers and event delivery.
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::net::IpAddr;
use std::rc::Rc;
use std::sync::{Arc, Mutex, Weak};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
use deno_core::{CancelFuture, JsBuffer, OpState, op2, v8};
use deno_error::JsErrorBox;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::{mpsc, Mutex as AsyncMutex};
use webrtc::data_channel::{DataChannel, DataChannelEvent, RTCDataChannelInit};
use webrtc::peer_connection::{PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler,
    RTCConfiguration, RTCConfigurationBuilder, RTCIceCandidateInit, RTCIceCandidateType,
    RTCIceConnectionState, RTCIceGatheringState, RTCIceServer, RTCIceTransportPolicy,
    RTCPeerConnectionIceErrorEvent, RTCPeerConnectionIceEvent, RTCPeerConnectionState,
    RTCSessionDescription, RTCSignalingState, RTCStatsReportEntry, StatsSelector};
use crate::ops::RealmStates;
use crate::rtc_transport::{Route, ServerTransport};

fn operation(error: impl std::fmt::Display) -> JsErrorBox { JsErrorBox::new("DOMExceptionOperationError", error.to_string()) }
fn security(message: &'static str) -> JsErrorBox { JsErrorBox::new("DOMExceptionSecurityError", message) }
fn invalid_state() -> JsErrorBox { JsErrorBox::new("DOMExceptionInvalidStateError", "The RTC connection's document has been retired") }
#[derive(Default)]
struct Registry { next: u32, sessions: BTreeMap<u32, Entry> }
struct Entry { session: Arc<Session>, cancel: Rc<deno_core::CancelHandle> }
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Configuration {
    #[serde(default)] ice_servers: Vec<IceServer>,
    #[serde(default = "all")] ice_transport_policy: String,
}
fn all() -> String { "all".into() }
#[derive(Clone, Deserialize)]
struct IceServer {
    urls: Value,
    #[serde(default)] username: String,
    #[serde(default)] credential: String,
    #[serde(default, rename = "credentialType")] credential_type: Option<String>,
}
#[derive(Default)]
pub(crate) struct Connections(Mutex<Vec<Weak<Session>>>);
impl Connections {
    fn register(&self, session: &Arc<Session>) {
        let mut sessions = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        sessions.retain(|session| session.strong_count() != 0); sessions.push(Arc::downgrade(session));
    }
    pub(crate) fn cancel_all(&self) { self.cancel(None); }
    pub(crate) fn cancel_realm(&self, cancel: &Rc<deno_core::CancelHandle>) { self.cancel(Some(Rc::as_ptr(cancel) as usize)); }
    fn cancel(&self, realm: Option<usize>) {
        let mut retained = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        retained.retain(|weak| {
            let Some(session) = weak.upgrade() else { return false; };
            if realm.is_some_and(|realm| realm != session.realm) { return true; }
            session.closed.store(true, Ordering::Release); session.shutdown.send_replace(true);
            let reactor = session.route.reactor.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
            if let Some(reactor) = reactor { reactor.spawn(async move { session.close().await; }); }
            false
        });
    }
}
struct Session {
    route: Route, config: AsyncMutex<Configuration>, peer: AsyncMutex<Option<Arc<dyn PeerConnection>>>,
    channels: Mutex<HashMap<usize, Arc<dyn DataChannel>>>, states: Mutex<Value>,
    urls: Mutex<HashMap<String, String>>,
    sender: mpsc::Sender<Value>, events: AsyncMutex<mpsc::Receiver<Value>>,
    shutdown: tokio::sync::watch::Sender<bool>, relay: AtomicBool,
    closed: AtomicBool,
    realm: usize,
}
impl Session {
    async fn emit(&self, event: Value) { let _ = self.sender.send(event).await; }
    async fn state(&self, name: &str, value: String, event: &str) {
        if let Ok(mut states) = self.states.lock() { states[name] = json!(value); }
        self.emit(json!({"type":event,"state":value})).await;
    }
    fn relay_only(&self) -> bool { self.relay.load(Ordering::Acquire) }
    async fn build_configuration(&self, config: &Configuration) -> Result<RTCConfiguration, JsErrorBox> {
        if !matches!(config.ice_transport_policy.as_str(), "all" | "relay") { return Err(JsErrorBox::type_error("Invalid iceTransportPolicy")); }
        let relay_only = self.route.proxy.is_some() || config.ice_transport_policy == "relay";
        let mut servers = Vec::new();
        let mut routes = HashMap::new(); let mut urls = HashMap::new();
        for server in &config.ice_servers {
            if server.credential_type.as_deref().is_some_and(|value| value != "password") { return Err(JsErrorBox::new("DOMExceptionNotSupportedError", "Only password TURN credentials are supported")); }
            let values: Vec<&str> = match &server.urls {
                Value::String(value) => vec![value],
                Value::Array(values) => values.iter().map(|value| value.as_str().ok_or_else(|| JsErrorBox::type_error("ICE URLs must be strings"))).collect::<Result<_, _>>()?,
                _ => return Err(JsErrorBox::type_error("ICE server urls must be a string or sequence")),
            };
            let mut parsed_urls = Vec::new();
            for value in values {
                let (scheme, rest) = value.split_once(':').ok_or_else(|| JsErrorBox::new("DOMExceptionSyntaxError", "Invalid ICE server URL"))?;
                if !matches!(scheme, "stun" | "turn" | "stuns" | "turns") { return Err(JsErrorBox::new("DOMExceptionSyntaxError", "Invalid ICE server scheme")); }
                let endpoint = url::Url::parse(&format!("http://{rest}")).map_err(operation)?;
                if !endpoint.username().is_empty() || endpoint.password().is_some() || endpoint.fragment().is_some() || endpoint.path() != "/" { return Err(JsErrorBox::new("DOMExceptionSyntaxError", "Invalid ICE server endpoint")); }
                let host = endpoint.host_str().ok_or_else(|| JsErrorBox::new("DOMExceptionSyntaxError", "Missing ICE server host"))?;
                let secure = matches!(scheme, "turns" | "stuns");
                let base_scheme = if matches!(scheme, "turn" | "turns") { "turn" } else { "stun" };
                let port = endpoint.port().unwrap_or(if secure { 5349 } else { 3478 });
                let transport = match endpoint.query() {
                    None | Some("transport=tcp") if secure => ServerTransport::Tls(host.trim_matches(['[', ']']).to_owned()),
                    None | Some("transport=udp") if !secure => ServerTransport::Udp,
                    Some("transport=tcp") if base_scheme == "turn" => ServerTransport::Tcp,
                    _ => return Err(JsErrorBox::new("DOMExceptionSyntaxError", "Invalid ICE server transport")),
                };
                if relay_only && base_scheme == "stun" { continue; }
                if self.route.proxy.as_ref().is_some_and(|proxy| proxy.scheme() == "http") && transport == ServerTransport::Udp {
                    return Err(JsErrorBox::new("DOMExceptionNotSupportedError", "An HTTP CONNECT RTC route requires a TURN URL with transport=tcp"));
                }
                if base_scheme == "turn" && (server.username.is_empty() || server.credential.is_empty()) { return Err(JsErrorBox::new("DOMExceptionInvalidAccessError", "TURN requires username and credential")); }
                let addresses = self.route.resolve(host.trim_matches(['[', ']']).into(), port).await.map_err(operation)?;
                if addresses.is_empty() { return Err(operation("ICE server resolution returned no allowed endpoints")); }
                for address in addresses {
                    self.route.check(address).map_err(operation)?;
                    // Pin DNS at the policy seam. The transport adapter selects the
                    // actual UDP/TCP connection; the sans-I/O TURN core consumes packets.
                    let internal = format!("{base_scheme}:{address}{}", if base_scheme == "turn" { "?transport=udp" } else { "" });
                    if routes.insert(address, transport.clone()).is_some_and(|old| old != transport) { return Err(operation("The same RTC server endpoint cannot use multiple transports in one connection")); }
                    urls.insert(internal.clone(), value.to_owned()); parsed_urls.push(internal);
                }
            }
            if !parsed_urls.is_empty() { servers.push(RTCIceServer { urls: parsed_urls, username: server.username.clone(), credential: server.credential.clone(), ..Default::default() }); }
        }
        if relay_only && !servers.iter().any(|server| server.urls.iter().any(|url| url.starts_with("turn:"))) { return Err(security("A relay-only RTC route requires a reachable TURN relay")); }
        *self.route.servers.lock().map_err(operation)? = routes;
        *self.urls.lock().map_err(operation)? = urls;
        Ok(RTCConfigurationBuilder::new().with_ice_servers(servers)
            .with_ice_transport_policy(if relay_only { RTCIceTransportPolicy::Relay } else { RTCIceTransportPolicy::All }).build())
    }
    async fn peer(self: &Arc<Self>) -> Result<Arc<dyn PeerConnection>, JsErrorBox> {
        if self.closed.load(Ordering::Acquire) { return Err(invalid_state()); }
        tokio::runtime::Handle::try_current().map_err(|_| operation("RTC operations require the browser's asynchronous runtime"))?;
        let mut peer = self.peer.lock().await;
        if let Some(peer) = &*peer { return Ok(peer.clone()); }
        *self.route.reactor.lock().map_err(operation)? = Some(tokio::runtime::Handle::current());
        let config = self.config.lock().await.clone();
        let configuration = self.build_configuration(&config).await?;
        let mut addresses = vec!["0.0.0.0:0".to_owned(), "[::]:0".to_owned()];
        if self.route.allowed(IpAddr::from([127, 0, 0, 1])) { addresses.push("127.0.0.1:0".into()); }
        if self.route.allowed(IpAddr::from([0u16, 0, 0, 0, 0, 0, 0, 1])) { addresses.push("[::1]:0".into()); }
        let value: Arc<dyn PeerConnection> = Arc::new(PeerConnectionBuilder::new().with_configuration(configuration)
            .with_runtime(Arc::new(self.route.clone())).with_handler(Arc::new(Handler(Arc::downgrade(self))))
            .with_data_channel_send_buffer_limit(16 * 1024 * 1024).with_udp_addrs(addresses).build().await.map_err(operation)?);
        if self.closed.load(Ordering::Acquire) { let _ = value.close().await; return Err(invalid_state()); }
        *peer = Some(value.clone()); Ok(value)
    }
    async fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.shutdown.send_replace(true);
        { let mut events = self.events.lock().await; events.close(); while events.try_recv().is_ok() {} }
        let peer = self.peer.lock().await.take();
        if let Some(peer) = peer { let _ = peer.close().await; }
        if let Ok(mut channels) = self.channels.lock() { channels.clear(); }
    }
    fn original_url(&self, url: &str) -> String { self.urls.lock().ok().and_then(|urls| urls.get(url).cloned()).unwrap_or_else(|| url.to_owned()) }
    fn public_candidate(&self, candidate: &str) -> Result<Option<String>, JsErrorBox> {
        if candidate.is_empty() { return Ok(Some(String::new())); }
        let tokens: Vec<&str> = candidate.split_ascii_whitespace().collect();
        if tokens.len() < 8 || tokens[6] != "typ" { return Err(operation("Malformed ICE candidate")); }
        let address = tokens[4].parse::<IpAddr>().map_err(|_| security("RTC candidates require a policy-checked IP address"))?;
        if !self.route.allowed(address) || (self.relay_only() && tokens[7] != "relay") { return Ok(None); }
        let related_allowed = tokens.windows(2).find(|pair| pair[0] == "raddr").is_none_or(|pair|
            pair[1].parse::<IpAddr>().is_ok_and(|address| self.route.allowed(address)));
        let mut output = tokens[..8].join(" ");
        let mut i = 8;
        while i < tokens.len() {
            if i + 1 == tokens.len() { return Err(operation("Malformed ICE candidate extension")); }
            let hide_related = (self.relay_only() || !related_allowed) && matches!(tokens[i], "raddr" | "rport");
            if !hide_related { output.push(' '); output.push_str(tokens[i]); output.push(' '); output.push_str(tokens[i + 1]); }
            i += 2;
        }
        Ok(Some(output))
    }
    fn description(&self, description: Option<RTCSessionDescription>) -> Result<Value, JsErrorBox> {
        let Some(mut description) = description else { return Ok(Value::Null); };
        let mut sdp = String::with_capacity(description.sdp.len());
        for line in description.sdp.lines() {
            if let Some(candidate) = line.strip_prefix("a=candidate:") {
                if let Some(candidate) = self.public_candidate(&format!("candidate:{candidate}"))? { sdp.push_str("a="); sdp.push_str(&candidate); sdp.push_str("\r\n"); }
            } else { sdp.push_str(line); sdp.push_str("\r\n"); }
        }
        description.sdp = sdp;
        serde_json::to_value(description).map_err(operation)
    }
    async fn snapshot(&self, peer: &Arc<dyn PeerConnection>) -> Result<Value, JsErrorBox> {
        let states = self.states.lock().map_err(operation)?.clone();
        Ok(json!({"states":states,
            "localDescription":self.description(peer.local_description().await)?,
            "currentLocalDescription":self.description(peer.current_local_description().await)?,
            "pendingLocalDescription":self.description(peer.pending_local_description().await)?,
            "remoteDescription":self.description(peer.remote_description().await)?,
            "currentRemoteDescription":self.description(peer.current_remote_description().await)?,
            "pendingRemoteDescription":self.description(peer.pending_remote_description().await)?,
            "canTrickleIceCandidates":peer.can_trickle_ice_candidates().await}))
    }
    async fn channel_info(&self, channel: &Arc<dyn DataChannel>) -> Result<Value, JsErrorBox> {
        let mut wire_id = Value::Null;
        if let Some(peer) = self.peer.lock().await.as_ref().cloned() {
            let report = peer.get_stats(Instant::now(), StatsSelector::None).await;
            // The stack's stats id names the stable handle, while the reported
            // identifier is the real SCTP stream number. Do not expose the handle as id.
            if let Some(RTCStatsReportEntry::DataChannel(stats)) = report.get(&format!("RTCDataChannel_{}", channel.id())) {
                if stats.state.to_string() != "connecting" { wire_id = json!(stats.data_channel_identifier); }
            }
        }
        Ok(json!({"handle":channel.id(),"id":wire_id,"label":channel.label().await.map_err(operation)?,
            "ordered":channel.ordered().await.map_err(operation)?,"protocol":channel.protocol().await.map_err(operation)?,
            "maxPacketLifeTime":channel.max_packet_life_time().await.map_err(operation)?,"maxRetransmits":channel.max_retransmits().await.map_err(operation)?,
            "negotiated":channel.negotiated().await.map_err(operation)?,"readyState":channel.ready_state().await.map_err(operation)?.to_string(),
            "bufferedAmount":channel.outstanding_bytes().await.map_err(operation)?}))
    }
    async fn register_channel(self: &Arc<Self>, channel: Arc<dyn DataChannel>, incoming: bool) -> Result<Value, JsErrorBox> {
        let handle = channel.id(); let info = self.channel_info(&channel).await?;
        self.channels.lock().map_err(operation)?.insert(handle, channel.clone());
        if incoming { self.emit(json!({"type":"datachannel","channel":info})).await; }
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            while let Some(event) = channel.poll().await {
                let Some(session) = weak.upgrade() else { break; };
                if session.closed.load(Ordering::Acquire) { break; }
                let terminal = matches!(event, DataChannelEvent::OnClose);
                let event = match event {
                    DataChannelEvent::OnOpen => json!({"type":"channel","event":"open","handle":handle,"channel":session.channel_info(&channel).await.ok()}),
                    DataChannelEvent::OnClosing => json!({"type":"channel","event":"closing","handle":handle}),
                    DataChannelEvent::OnClose => json!({"type":"channel","event":"close","handle":handle}),
                    DataChannelEvent::OnError => json!({"type":"channel","event":"error","handle":handle}),
                    DataChannelEvent::OnBufferedAmountLow => json!({"type":"channel","event":"bufferedamountlow","handle":handle,"bufferedAmount":channel.outstanding_bytes().await.unwrap_or(0)}),
                    DataChannelEvent::OnMessage(message) => json!({"type":"channel","event":"message","handle":handle,"text":message.is_string,"data":message.data.as_ref()}),
                    _ => continue,
                };
                session.emit(event).await;
                if terminal { if let Ok(mut channels) = session.channels.lock() { channels.remove(&handle); } break; }
            }
        });
        Ok(info)
    }
}
struct Handler(Weak<Session>);
#[async_trait::async_trait]
impl PeerConnectionEventHandler for Handler {
    async fn on_negotiation_needed(&self) { if let Some(session) = self.0.upgrade() { session.emit(json!({"type":"negotiationneeded"})).await; } }
    async fn on_ice_candidate(&self, event: RTCPeerConnectionIceEvent) {
        let Some(session) = self.0.upgrade() else { return; };
        if session.relay_only() && event.candidate.typ != RTCIceCandidateType::Relay { return; }
        let local = { let peer = session.peer.lock().await.clone(); if let Some(peer) = peer { peer.local_description().await } else { None } };
        let mid = local.as_ref().and_then(|description| description.sdp.lines().find_map(|line| line.strip_prefix("a=mid:").map(str::to_owned)));
        let ufrag = local.as_ref().and_then(|description| description.sdp.lines().find_map(|line| line.strip_prefix("a=ice-ufrag:").map(str::to_owned)));
        match event.candidate.to_json().map_err(operation).and_then(|mut candidate| {
            let Some(value) = session.public_candidate(&candidate.candidate)? else { return Ok(None); };
            candidate.candidate = value;
            candidate.sdp_mid = mid; candidate.username_fragment = ufrag;
            if !event.url.is_empty() { candidate.url = Some(session.original_url(&event.url)); }
            Ok(Some(candidate))
        }) { Ok(Some(candidate)) => session.emit(json!({"type":"icecandidate","candidate":candidate})).await, Ok(None) => {}, Err(error) => session.emit(json!({"type":"icecandidateerror","errorText":error.to_string(),"errorCode":701})).await }
    }
    async fn on_ice_candidate_error(&self, event: RTCPeerConnectionIceErrorEvent) {
        if let Some(session) = self.0.upgrade() {
            let expose = !session.relay_only() && event.address.parse().is_ok_and(|address| session.route.allowed(address));
            session.emit(json!({"type":"icecandidateerror","address":if expose { &event.address } else { "" },"port":if expose { Some(event.port) } else { None },"url":session.original_url(&event.url),"errorCode":event.error_code,"errorText":event.error_text})).await;
        }
    }
    async fn on_signaling_state_change(&self, state: RTCSignalingState) { if let Some(s) = self.0.upgrade() { s.state("signalingState", state.to_string(), "signalingstatechange").await; } }
    async fn on_ice_connection_state_change(&self, state: RTCIceConnectionState) { if let Some(s) = self.0.upgrade() { s.state("iceConnectionState", state.to_string(), "iceconnectionstatechange").await; } }
    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        if let Some(s) = self.0.upgrade() { s.state("iceGatheringState", state.to_string(), "icegatheringstatechange").await; if state == RTCIceGatheringState::Complete { s.emit(json!({"type":"icecandidate","candidate":null})).await; } }
    }
    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) { if let Some(s) = self.0.upgrade() { s.state("connectionState", state.to_string(), "connectionstatechange").await; } }
    async fn on_data_channel(&self, channel: Arc<dyn DataChannel>) { if let Some(s) = self.0.upgrade() { if let Err(error) = s.register_channel(channel, true).await { s.emit(json!({"type":"error","message":error.to_string()})).await; } } }
}

fn document(scope: &mut v8::PinScope, state: &OpState, receiver: v8::Local<v8::Object>) -> Result<(Rc<RefCell<crate::ops::ObscuraState>>, Rc<deno_core::CancelHandle>), JsErrorBox> {
    let context = receiver.get_creation_context(scope).ok_or_else(invalid_state)?;
    if !receiver.strict_equals(context.global(scope).into()) { return Err(JsErrorBox::type_error("Invalid RTC realm receiver")); }
    let context = v8::Global::new(scope, context);
    let (_, _, owner) = crate::window::context_document(state, &context)?;
    let registry = state.borrow::<Rc<RefCell<RealmStates>>>();
    let cancel = registry.borrow().context_cancel(&context).ok_or_else(invalid_state)?;
    Ok((owner, cancel))
}
fn capture(scope: &mut v8::PinScope, state: &OpState, receiver: v8::Local<v8::Object>) -> Result<(Route, Rc<deno_core::CancelHandle>, Rc<RefCell<crate::ops::ObscuraState>>), JsErrorBox> {
    let (document, cancel) = document(scope, state, receiver)?;
    let owner = document.borrow();
    let http = owner.http_client.clone().unwrap_or_else(|| Arc::new(obscura_net::ObscuraHttpClient::new()));
    #[cfg(feature = "stealth")]
    let proxy = owner.stealth_client.as_ref().map_or_else(|| http.proxy_url(), |client| client.proxy_url());
    #[cfg(not(feature = "stealth"))]
    let proxy = http.proxy_url();
    let proxy = proxy.map(url::Url::parse).transpose().map_err(|_| security("Invalid RTC proxy route"))?;
    if proxy.as_ref().is_some_and(|proxy| !matches!(proxy.scheme(), "socks5" | "socks5h" | "http")) { return Err(JsErrorBox::new("DOMExceptionNotSupportedError", "RTC requires SOCKS5 or HTTP CONNECT with TURN/TCP")); }
    Ok((Route { http, #[cfg(feature = "stealth")] stealth: owner.stealth_client.clone(), proxy, servers: Arc::new(Mutex::new(HashMap::new())), events: None,
        reactor: Arc::new(Mutex::new(None)) }, cancel, document.clone()))
}
fn lookup(scope: &mut v8::PinScope, state: &OpState, receiver: v8::Local<v8::Object>, id: u32) -> Result<(Arc<Session>, Rc<deno_core::CancelHandle>), JsErrorBox> {
    let (_, cancel) = document(scope, state, receiver)?;
    let entry = state.try_borrow::<Registry>().and_then(|registry| registry.sessions.get(&id)).ok_or_else(invalid_state)?;
    if !Rc::ptr_eq(&entry.cancel, &cancel) || cancel.is_canceled() { return Err(invalid_state()); }
    Ok((entry.session.clone(), cancel))
}
#[op2(fast)]
fn op_rtc_create(scope: &mut v8::PinScope, state: &mut OpState, #[string] config: String, receiver: v8::Local<v8::Object>) -> Result<u32, JsErrorBox> {
    let (mut route, cancel, document) = capture(scope, state, receiver)?;
    let config: Configuration = serde_json::from_str(&config).map_err(|_| JsErrorBox::type_error("Invalid RTCConfiguration"))?;
    if !matches!(config.ice_transport_policy.as_str(), "all" | "relay") { return Err(JsErrorBox::type_error("Invalid iceTransportPolicy")); }
    if state.try_borrow::<Registry>().is_none() { state.put(Registry::default()); }
    let registry = state.borrow_mut::<Registry>();
    registry.sessions.retain(|_, entry| !entry.session.closed.load(Ordering::Acquire));
    registry.next = registry.next.checked_add(1).ok_or_else(|| operation("RTC connection identifiers exhausted"))?;
    let (sender, events) = mpsc::channel(64);
    route.events = Some(sender.clone());
    let relay = AtomicBool::new(route.proxy.is_some() || config.ice_transport_policy == "relay");
    let session = Arc::new(Session { route, config: AsyncMutex::new(config), peer: AsyncMutex::new(None), channels: Mutex::new(HashMap::new()),
        states: Mutex::new(json!({"signalingState":"stable","iceGatheringState":"new","iceConnectionState":"new","connectionState":"new"})),
        urls: Mutex::new(HashMap::new()), sender, events: AsyncMutex::new(events), closed: AtomicBool::new(false),
        shutdown: tokio::sync::watch::channel(false).0, relay, realm: Rc::as_ptr(&cancel) as usize });
    document.borrow().rtc_connections.register(&session);
    let id = registry.next; registry.sessions.insert(id, Entry { session, cancel }); Ok(id)
}
#[op2]
#[string]
fn op_rtc_next(scope: &mut v8::PinScope, state: Rc<RefCell<OpState>>, id: u32, receiver: v8::Local<v8::Object>) -> impl Future<Output = Result<String, JsErrorBox>> + use<> {
    let captured = lookup(scope, &state.borrow(), receiver, id);
    async move {
        let (session, cancel) = captured?;
        let mut shutdown = session.shutdown.subscribe();
        let event = async {
            if *shutdown.borrow() { return None; }
            let mut events = session.events.lock().await;
            tokio::select! { event = events.recv() => event, _ = shutdown.changed() => None }
        }.or_cancel(cancel).await;
        match event {
            Ok(Some(event)) => Ok(event.to_string()),
            _ => { session.close().await; if let Some(registry) = state.borrow_mut().try_borrow_mut::<Registry>() { registry.sessions.remove(&id); } Ok("null".into()) },
        }
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChannelOptions { #[serde(default = "yes")] ordered: bool, max_packet_life_time: Option<u16>, max_retransmits: Option<u16>, #[serde(default)] protocol: String, #[serde(default)] negotiated: bool, id: Option<u16> }
fn yes() -> bool { true }
#[op2]
#[string]
fn op_rtc_request(scope: &mut v8::PinScope, state: &OpState, id: u32, #[string] command: String, #[string] input: String, receiver: v8::Local<v8::Object>) -> impl Future<Output = Result<String, JsErrorBox>> + use<> {
    let captured = lookup(scope, state, receiver, id);
    async move {
        let (session, cancel) = captured?;
        let action = async {
            if command == "close" { session.close().await; return Ok(json!({"value":null})); }
            let peer = session.peer().await?;
            let input: Value = serde_json::from_str(&input).map_err(operation)?;
            let value = match command.as_str() {
                "offer" => {
                    if input["iceRestart"].as_bool() == Some(true) { peer.restart_ice().await.map_err(operation)?; }
                    session.description(Some(peer.create_offer(None).await.map_err(operation)?))?
                }
                "answer" => session.description(Some(peer.create_answer(None).await.map_err(operation)?))?,
                "local" | "remote" => {
                    let description: RTCSessionDescription = serde_json::from_value(input).map_err(operation)?;
                    // Validate every supplied endpoint before the core can start ICE checks.
                    for line in description.sdp.lines() {
                        if let Some(candidate) = line.strip_prefix("a=candidate:") { if session.public_candidate(&format!("candidate:{candidate}"))?.is_none() { return Err(security("SDP contains an endpoint forbidden by RTC network policy")); } }
                    }
                    if command == "local" { peer.set_local_description(description).await.map_err(operation)?; }
                    else { peer.set_remote_description(description).await.map_err(operation)?; } Value::Null
                }
                "candidate" => {
                    let candidate: RTCIceCandidateInit = serde_json::from_value(input).map_err(operation)?;
                    if session.public_candidate(&candidate.candidate)?.is_none() { return Err(security("ICE candidate is forbidden by RTC network policy")); }
                    peer.add_ice_candidate(candidate).await.map_err(operation)?; Value::Null
                }
                "restart" => { peer.restart_ice().await.map_err(operation)?; Value::Null }
                "snapshot" => Value::Null,
                "configure" => {
                    let config: Configuration = serde_json::from_value(input).map_err(operation)?;
                    let configuration = session.build_configuration(&config).await?; peer.set_configuration(configuration).await.map_err(operation)?;
                    session.relay.store(session.route.proxy.is_some() || config.ice_transport_policy == "relay", Ordering::Release);
                    *session.config.lock().await = config; Value::Null
                }
                "channel" => {
                    let label = input["label"].as_str().ok_or_else(|| JsErrorBox::type_error("Missing data channel label"))?;
                    let options: ChannelOptions = serde_json::from_value(input["options"].clone()).map_err(operation)?;
                    if options.max_packet_life_time.is_some() && options.max_retransmits.is_some() { return Err(JsErrorBox::type_error("maxPacketLifeTime and maxRetransmits are mutually exclusive")); }
                    if options.negotiated && options.id.is_none() { return Err(JsErrorBox::type_error("A negotiated data channel requires an id")); }
                    let channel = peer.create_data_channel(label, Some(RTCDataChannelInit { ordered:options.ordered, max_packet_life_time:options.max_packet_life_time,
                        max_retransmits:options.max_retransmits, protocol:options.protocol, negotiated:if options.negotiated { options.id } else { None } })).await.map_err(operation)?;
                    session.register_channel(channel, false).await?
                }
                "channelClose" | "threshold" => {
                    let handle = input["handle"].as_u64().ok_or_else(|| JsErrorBox::type_error("Missing data channel handle"))? as usize;
                    let channel = session.channels.lock().map_err(operation)?.get(&handle).cloned().ok_or_else(invalid_state)?;
                    if command == "channelClose" { channel.close().await.map_err(operation)?; }
                    else { channel.set_buffered_amount_low_threshold(input["threshold"].as_u64().unwrap_or(0).min(u32::MAX as u64) as u32).await.map_err(operation)?; } Value::Null
                }
                "stats" => {
                    let report = peer.get_stats(Instant::now(), StatsSelector::None).await;
                    let mut entries = Vec::with_capacity(report.len());
                    for entry in report.iter() {
                        let mut value = match entry {
                            RTCStatsReportEntry::PeerConnection(v) => serde_json::to_value(v), RTCStatsReportEntry::Transport(v) => serde_json::to_value(v),
                            RTCStatsReportEntry::IceCandidatePair(v) => serde_json::to_value(v), RTCStatsReportEntry::LocalCandidate(v) | RTCStatsReportEntry::RemoteCandidate(v) => serde_json::to_value(v),
                            RTCStatsReportEntry::Certificate(v) => serde_json::to_value(v), RTCStatsReportEntry::DataChannel(v) => serde_json::to_value(v), _ => continue,
                        }.map_err(operation)?;
                        if matches!(entry, RTCStatsReportEntry::LocalCandidate(_) | RTCStatsReportEntry::RemoteCandidate(_)) {
                            if value["address"].as_str().is_some_and(|address| address.parse().is_ok_and(|ip| !session.route.allowed(ip))) {
                                if let Some(object) = value.as_object_mut() { object.remove("address"); }
                            }
                            if session.relay_only() {
                                if value["candidateType"].as_str() != Some("relay") { continue; }
                            }
                            let related_allowed = value["relatedAddress"].as_str().is_none_or(|address| address.is_empty() || address.parse().is_ok_and(|ip| session.route.allowed(ip)));
                            if session.relay_only() || !related_allowed {
                                if let Some(object) = value.as_object_mut() { object.remove("relatedAddress"); object.remove("relatedPort"); }
                            }
                            if let Some(url) = value["url"].as_str() {
                                let url = session.original_url(url);
                                if url.starts_with("turns:") { value["relayProtocol"] = json!("tls"); }
                                else if url.contains("transport=tcp") { value["relayProtocol"] = json!("tcp"); }
                                value["url"] = json!(url);
                            }
                        }
                        entries.push(value);
                    }
                    json!(entries)
                }
                _ => return Err(JsErrorBox::type_error("Unknown RTC operation")),
            };
            Ok(json!({"value":value,"snapshot":session.snapshot(&peer).await?}))
        };
        match action.or_cancel(cancel).await { Ok(result) => result.map(|value| value.to_string()), Err(_) => { session.close().await; Err(invalid_state()) } }
    }
}
#[op2]
fn op_rtc_send(scope: &mut v8::PinScope, state: &OpState, id: u32, handle: u32, #[buffer] data: JsBuffer, text: bool, receiver: v8::Local<v8::Object>) -> impl Future<Output = Result<(), JsErrorBox>> + use<> {
    let captured = lookup(scope, state, receiver, id);
    async move {
        let (session, cancel) = captured?;
        let channel = session.channels.lock().map_err(operation)?.get(&(handle as usize)).cloned().ok_or_else(invalid_state)?;
        let send = async {
            if text { channel.try_send_text(std::str::from_utf8(&data).map_err(operation)?).await.map_err(operation) }
            else { channel.try_send(bytes::BytesMut::from(data.as_ref())).await.map_err(operation) }
        };
        send.or_cancel(cancel).await.map_err(|_| invalid_state())?
    }
}
pub(crate) fn ops() -> Vec<deno_core::OpDecl> { vec![op_rtc_create(), op_rtc_request(), op_rtc_next(), op_rtc_send()] }
