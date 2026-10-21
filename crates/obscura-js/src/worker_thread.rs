use super::*;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicU32, Ordering};
use tokio::sync::mpsc::{UnboundedSender, UnboundedReceiver, unbounded_channel};
pub(super) struct WorkerIdentity(pub serde_json::Value);
pub(super) struct WorkerError {
    pub message: String,
    pub filename: String,
    pub line: u32,
    pub column: u32,
}
impl WorkerError {
    fn plain(message: String) -> Self { Self { message, filename: String::new(), line: 0, column: 0 } }
}

pub(super) enum Incoming {
    Message(u32, bool, Envelope),
    Error(u32, WorkerError),
    Port(u32),
    Receipts(u32, Receipts),
    Stop,
}
pub(super) struct ThreadLink {
    pub parent: UnboundedSender<Incoming>,
    pub id: u32,
    pub control: Arc<Control>,
}
pub(super) struct RemoteWorker {
    pub creator: v8::Global<v8::Context>,
    pub creator_state: SharedState,
    pub root: SharedState,
    pub generation: u64,
    pub url: String,
    pub sender: UnboundedSender<Incoming>,
    pub control: Arc<Control>,
}
pub(super) struct Control {
    pub cancellation: Arc<WorkerCancellation>,
    isolate: Mutex<Option<v8::IsolateHandle>>,
    sender: UnboundedSender<Incoming>,
}
impl Control {
    pub fn terminate(&self) {
        self.cancellation.cancel();
        if let Some(handle) = self.isolate.lock().unwrap_or_else(|error| error.into_inner()).as_ref() { handle.terminate_execution(); }
        let _ = self.sender.send(Incoming::Stop);
    }
}
impl Drop for RemoteWorker { fn drop(&mut self) { self.control.terminate(); } }

/// Entanglement belongs to the browser's native messaging agent, not to a JS
/// wrapper. A transferred endpoint stays queued here while neither realm owns
/// a wrapper; installing the receiving wrapper atomically takes ownership.
#[derive(Default)]
pub(super) struct PortRouter {
    endpoints: HashMap<u32, Endpoint>,
    quota: Arc<Mutex<(usize,usize)>>,
    abandoned: Arc<Mutex<Vec<u32>>>,
}
struct Endpoint {
    peer: Option<u32>,
    sender: Option<UnboundedSender<Incoming>>,
    messages: VecDeque<Envelope>,
    closed: bool,
}
pub(super) struct MessageLease {
    quota: Arc<Mutex<(usize,usize)>>,
    size: usize,
    ports: Arc<[u32]>,
    abandoned: Arc<Mutex<Vec<u32>>>,
    pub armed: bool,
}
impl Drop for MessageLease {
    fn drop(&mut self) {
        if self.armed && !self.ports.is_empty() { self.abandoned.lock().unwrap_or_else(|error| error.into_inner()).extend_from_slice(&self.ports); }
        let mut quota = self.quota.lock().unwrap_or_else(|error| error.into_inner());
        quota.0 = quota.0.saturating_sub(self.size); quota.1 = quota.1.saturating_sub(1);
    }
}
impl PortRouter {
    pub fn collect(&mut self) {
        loop {
            let ids = std::mem::take(&mut *self.abandoned.lock().unwrap_or_else(|error| error.into_inner()));
            if ids.is_empty() { break; }
            for id in ids { self.close(id); }
        }
    }
    pub fn reserve(&mut self, size: usize, ports: Arc<[u32]>) -> Result<MessageLease, JsErrorBox> {
        self.collect();
        let mut quota = self.quota.lock().unwrap_or_else(|error| error.into_inner());
        if size > MAX_MESSAGE_BYTES.saturating_sub(quota.0) || quota.1 >= MAX_TASKS { return Err(clone_error("Worker message budget exceeded")); }
        quota.0 += size; quota.1 += 1;
        Ok(MessageLease { quota: self.quota.clone(), size, ports, abandoned: self.abandoned.clone(), armed: false })
    }
    pub fn pair(&mut self, sender: &UnboundedSender<Incoming>) -> Result<(u32,u32),JsErrorBox> {
        self.collect();
        static NEXT: AtomicU32 = AtomicU32::new(1);
        if self.endpoints.len() >= 4096 { return Err(JsErrorBox::range_error("MessagePort count limit exceeded")); }
        let a = NEXT.fetch_add(2, Ordering::Relaxed); let b = a.checked_add(1).ok_or_else(|| JsErrorBox::range_error("MessagePort identifier space exhausted"))?;
        for (id,peer) in [(a,b),(b,a)] { self.endpoints.insert(id, Endpoint { peer: Some(peer), sender: Some(sender.clone()), messages: VecDeque::new(), closed: false }); }
        Ok((a,b))
    }
    pub fn transfer(&mut self, id: u32) { if let Some(endpoint) = self.endpoints.get_mut(&id) { endpoint.sender = None; } }
    pub fn claim(&mut self, id: u32, sender: &UnboundedSender<Incoming>) {
        if let Some(endpoint) = self.endpoints.get_mut(&id) { endpoint.sender = Some(sender.clone()); if !endpoint.messages.is_empty() { let _ = sender.send(Incoming::Port(id)); } }
    }
    pub fn send(&mut self, id: u32, message: Envelope) {
        let peer = self.endpoints.get(&id).filter(|endpoint| !endpoint.closed).and_then(|endpoint| endpoint.peer);
        if let Some(endpoint) = peer.and_then(|id| self.endpoints.get_mut(&id).map(|endpoint| (id,endpoint))).filter(|(_,endpoint)| !endpoint.closed) {
            let (id,endpoint) = endpoint; endpoint.messages.push_back(message);
            if let Some(sender) = &endpoint.sender { let _ = sender.send(Incoming::Port(id)); }
        }
    }
    pub fn take(&mut self, id: u32) -> Option<Envelope> { self.endpoints.get_mut(&id).filter(|endpoint| !endpoint.closed)?.messages.pop_front() }
    pub fn pending(&self, id: u32) -> bool { self.endpoints.get(&id).is_some_and(|endpoint| !endpoint.closed && !endpoint.messages.is_empty()) }
    pub fn close(&mut self, id: u32) {
        let peer = self.endpoints.remove(&id).and_then(|mut endpoint| endpoint.peer.take());
        if let Some(peer) = peer.and_then(|id| self.endpoints.get_mut(&id)) { peer.peer = None; }
    }
}

/// Captured only from the native authenticated creator before spawning. The
/// selected clients own TLS/proxy/CIDR/file policy and cookie configuration.
pub(super) struct WorkerNetworkSnapshot {
    url: String,
    origin: String,
    referrer: String,
    referrer_policy: String,
    policies: Vec<String>,
    local_resources: crate::local_resource::LocalResources,
    principal: crate::local_resource::LocalResourcePrincipal,
    cookie_jar: Option<Arc<obscura_net::CookieJar>>,
    client: Option<Arc<obscura_net::ObscuraHttpClient>>,
    callbacks: Option<Arc<obscura_net::CallbackRegistry>>,
    #[cfg(feature = "stealth")]
    stealth: Option<Arc<obscura_net::StealthHttpClient>>,
    intercept_tx: Option<UnboundedSender<crate::ops::InterceptedRequest>>,
    intercept_enabled: bool,
    intercept_patterns: Vec<obscura_net::InterceptPattern>,
    page_id: String,
    blocked: Vec<String>,
    in_flight: Arc<AtomicU32>,
    frame_id: u32,
    generation: u64,
    entry: Option<Result<crate::local_resource::LocalResource,String>>,
    identity: serde_json::Value,
}
impl WorkerNetworkSnapshot {
    pub fn capture(creator: &ObscuraState, root: &ObscuraState, url: &str, identity: serde_json::Value) -> Self {
        let entry = url::Url::parse(url).ok().and_then(|url| match crate::local_resource::load_local_resource(creator, &url) {
            Ok(Some(resource)) => Some(Ok(resource)), Ok(None) => None, Err(error) => Some(Err(error.to_string())),
        });
        Self { url: url.to_string(), origin: crate::window::origin(creator), referrer: creator.url.clone(),
            referrer_policy: crate::ops::document_referrer_policy(creator), policies: creator_policies(creator),
            local_resources: creator.local_resources.clone(), principal: crate::local_resource::LocalResourcePrincipal::for_document(creator),
            cookie_jar: root.cookie_jar.clone(), client: root.http_client.clone(), callbacks: root.callbacks.clone(),
            #[cfg(feature = "stealth")]
            stealth: root.stealth_client.clone(),
            intercept_tx: root.intercept_tx.clone(), intercept_enabled: root.intercept_enabled,
            intercept_patterns: root.intercept_url_patterns.clone(), page_id: root.intercept_page_id.clone(), blocked: root.blocked_urls.clone(),
            in_flight: root.page_in_flight.clone(), frame_id: creator.frame_id, generation: creator.document_generation, entry, identity }
    }
    fn install(self) -> SharedState {
        let mut owner = ObscuraState::new();
        owner.url = self.url; owner.inherited_origin = Some(self.origin); owner.referrer = self.referrer;
        owner.referrer_policy = self.referrer_policy; owner.content_security_policy = self.policies;
        owner.local_resources = self.local_resources; owner.local_resource_principal = Some(self.principal);
        owner.cookie_jar = self.cookie_jar; owner.http_client = self.client; owner.callbacks = self.callbacks;
        #[cfg(feature = "stealth")]
        { owner.stealth_client = self.stealth; }
        owner.intercept_tx = self.intercept_tx; owner.intercept_enabled = self.intercept_enabled;
        owner.intercept_url_patterns = self.intercept_patterns; owner.intercept_page_id = self.page_id;
        owner.blocked_urls = self.blocked; owner.page_in_flight = self.in_flight;
        owner.frame_id = self.frame_id; owner.document_generation = self.generation; owner.is_worker = true;
        owner.worker_entry_pending = true;
        Rc::new(RefCell::new(owner))
    }
}
pub(super) struct Receipts {
    urls: Vec<String>,
    events: Vec<crate::ops::JsNetworkEvent>,
    bodies: HashMap<String,crate::ops::StoredNetworkResponseBody>,
}
impl Receipts {
    fn drain(owner: &SharedState) -> Option<Self> {
        let mut owner = owner.borrow_mut();
        if owner.fetched_urls.is_empty() && owner.js_network_events.is_empty() && owner.network_response_bodies.is_empty() { return None; }
        owner.network_response_body_order.clear();
        Some(Self { urls: std::mem::take(&mut owner.fetched_urls), events: std::mem::take(&mut owner.js_network_events),
            bodies: std::mem::take(&mut owner.network_response_bodies) })
    }
    pub fn merge(self, root: &SharedState) {
        let mut root = root.borrow_mut();
        for url in self.urls { root.fetched_urls.push(url); }
        if root.fetched_urls.len() > 16384 { let overflow = root.fetched_urls.len() - 16384; root.fetched_urls.drain(..overflow); }
        let mut bodies = self.bodies;
        for event in self.events {
            if let Some(body) = bodies.remove(&event.request_id) { crate::ops::record_network_completion(&mut root,event, |_| body); }
            else { crate::ops::push_js_network_event(&mut root,event); }
        }
    }
}
pub(super) fn spawn(id: u32, creator: v8::Global<v8::Context>, creator_state: SharedState, root: SharedState,
    name: String, kind: String, snapshot: WorkerNetworkSnapshot, parent: UnboundedSender<Incoming>, router: Arc<Mutex<PortRouter>>) -> Result<RemoteWorker, JsErrorBox> {
    let (sender,input) = unbounded_channel();
    let url = snapshot.url.clone();
    let control = Arc::new(Control { cancellation: Arc::new(WorkerCancellation::default()), isolate: Mutex::new(None), sender: sender.clone() });
    let thread_control = control.clone();
    std::thread::Builder::new().name(format!("obscura-worker-{id}")).stack_size(8 * 1024 * 1024).spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let executor = tokio::runtime::Builder::new_current_thread().enable_all().build()
                .map_err(|error| error.to_string())?;
            executor.block_on(run(id, name, kind, snapshot, parent.clone(), input, router, thread_control.clone()))
        }));
        let error = match result { Ok(Ok(())) => None, Ok(Err(error)) => Some(error), Err(_) => Some("Worker owner thread panicked".to_string()) };
        if let Some(error) = error { if !thread_control.cancellation.is_cancelled() { let _ = parent.send(Incoming::Error(id,WorkerError::plain(error))); } }
        thread_control.cancellation.cancel();
        thread_control.isolate.lock().unwrap_or_else(|error| error.into_inner()).take();
    }).map_err(|error| JsErrorBox::generic(format!("Worker thread creation failed: {error}")))?;
    let generation = creator_state.borrow().document_generation;
    Ok(RemoteWorker { creator, creator_state, root, generation, url, sender, control })
}
async fn run(id: u32, name: String, kind: String, snapshot: WorkerNetworkSnapshot, parent: UnboundedSender<Incoming>,
    input: UnboundedReceiver<Incoming>, router: Arc<Mutex<PortRouter>>, control: Arc<Control>) -> Result<(),String> {
    let entry = snapshot.entry.clone();
    let identity = snapshot.identity.clone();
    let owner = snapshot.install();
    let loader = Rc::new(modules::WorkerModuleLoader {
        inner: crate::module_loader::ObscuraModuleLoader::with_page_state(&owner.borrow().url, None, &owner, Rc::new(RefCell::new(crate::import_map::ImportMap::default()))),
        owner: owner.clone(), entry: entry.clone(), cancellation: control.cancellation.clone(),
    });
    // Construction shares only the existing init lock, never an execution lock.
    let mut runtime = {
        let _lock = crate::runtime::ISOLATE_CREATE_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        deno_core::JsRuntime::new(deno_core::RuntimeOptions {
            extensions: vec![crate::ops::build_extension()],
            module_loader: Some(loader),
            create_params: Some(v8::Isolate::create_params().heap_limits(0, 256 * 1024 * 1024)),
            ..Default::default()
        })
    };
    let handle = runtime.v8_isolate().thread_safe_handle();
    *control.isolate.lock().unwrap_or_else(|error| error.into_inner()) = Some(handle.clone());
    if control.cancellation.is_cancelled() { return Ok(()); }
    let heap_control = control.clone();
    runtime.add_near_heap_limit_callback(move |limit,_| { heap_control.terminate(); limit.saturating_add(16*1024*1024) });
    let context = runtime.main_context();
    let shared = SharedWorkers::default();
    let cancel = CancelHandle::new_rc();
    { let mut workers = shared.borrow_mut(); workers.input = input; workers.sender = control.sender.clone(); workers.router = router;
        workers.thread_link = Some(ThreadLink { parent: parent.clone(), id, control: control.clone() });
        workers.workers.insert(0,Worker { creator_state: owner.clone(), owner: owner.clone(), root: owner.clone(), context: context.clone(), cancel: cancel.clone(), ready: false, pending: VecDeque::new(), entry });
    }
    let state = runtime.op_state();
    { let mut state = state.borrow_mut(); state.put(shared.clone()); state.put(owner.clone()); state.put(WorkerIdentity(identity.clone()));
        state.put(Rc::new(RefCell::new(crate::ops::RealmStates::default()))); }
    let source = format!("(function() {{ const __obscuraCore = Deno.core; globalThis.__obscura_clone_hooks = {{}}; ({})(__obscuraCore.ops, {}); ({})(globalThis, __obscuraCore.ops); ({})(globalThis); {}; delete globalThis.Deno; }})();", BOOTSTRAP,
        serde_json::json!({"name":name,"url":owner.borrow().url,"type":kind,"identity":identity}), include_str!("../js/text_decoder.js"), include_str!("../js/streams.js"), include_str!("../js/webcrypto.js"));
    runtime.execute_script("<worker:bootstrap>", source).map_err(|error| error.to_string())?;
    shared.borrow_mut().tasks.push_back(Task::Start(0));
    loop {
        if control.cancellation.is_cancelled() { break; }
        let result = std::future::poll_fn(|cx| {
            if control.cancellation.is_cancelled() { return Poll::Ready(Ok(())); }
            let watchdog = crate::cdp_watchdog::arm(handle.clone(),Duration::from_secs(30));
            let work = {
                v8::scope_with_context!(scope,runtime.v8_isolate(),&context);
                super::poll(scope,&state,cx)
            };
            let engine = runtime.poll_event_loop(cx,deno_core::PollEventLoopOptions::default());
            let timed_out = crate::cdp_watchdog::disarm(watchdog);
            if timed_out { return Poll::Ready(Err("Worker execution deadline exceeded".to_string())); }
            if let Some(receipts) = Receipts::drain(&owner) { let _ = parent.send(Incoming::Receipts(id,receipts)); }
            if control.cancellation.is_cancelled() { return Poll::Ready(Ok(())); }
            match engine { Poll::Ready(Err(error)) => Poll::Ready(Err(error.to_string())), Poll::Ready(Ok(())) if !work => Poll::Pending, _ => Poll::Pending }
        }).await;
        if result.is_err() { return result; }
        break;
    }
    owner.borrow().document_active.set(false); owner.borrow().resource_load_cancel.cancel(); cancel.cancel();
    // Drop pending network futures/timers on their owner before the executor.
    shared.borrow_mut().timers.clear();
    Ok(())
}
