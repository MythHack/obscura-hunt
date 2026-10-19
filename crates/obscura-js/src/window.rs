use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use deno_core::{op2, v8, OpDecl, OpState};
use deno_core::v8::{ValueDeserializerHelper, ValueSerializerHelper};
use deno_error::JsErrorBox;
use crate::ops::{ObscuraState, RealmStates, SharedState};
use crate::module_loader::ObscuraModuleLoader;
use crate::storage::{SharedStorage, StorageNamespace};

#[path = "native_window.rs"]
pub(crate) mod native_window;

/// Creates (or restores) the V8 context for one browsing context and world.
///
/// The first creation captures the context's native global proxy.  Later
/// navigations detach that proxy from the retired document and reuse it with
/// the same snapshot index, so retained Window references keep their identity
/// across same-origin and cross-origin navigations.  Isolated worlds pass a
/// distinct `world_id`, so they never share a proxy with the author world.
pub(crate) fn create_realm_context<'s>(
    scope: &mut v8::PinScope<'s, '_, ()>,
    realms: &Rc<RefCell<RealmStates>>,
    frame_id: u32,
    world_id: Option<u64>,
    previous: Option<&v8::Global<v8::Context>>,
) -> Option<v8::Global<v8::Context>> {
    let existing = realms.borrow().proxy_record(frame_id, world_id);
    let (index, proxy) = match existing {
        Some((proxy, index)) => {
            if let Some(previous) = previous {
                let context = v8::Local::new(&*scope, previous);
                let pointer = &*context as *const v8::Context as *mut std::ffi::c_void;
                obscura_v8_native::detach_global(pointer);
            }
            (index, Some(proxy))
        }
        None => (native_window::SNAPSHOT_INDEX, None),
    };
    let options = v8::ContextOptions {
        global_object: proxy.as_ref().map(|proxy| v8::Local::new(&*scope, proxy).into()),
        ..Default::default()
    };
    let context = v8::Context::from_snapshot(&*scope, index, options)?;
    if proxy.is_none() {
        let global = context.global(scope);
        realms.borrow_mut().remember_proxy(frame_id, world_id,
            v8::Global::new(scope, global), index);
    }
    Some(v8::Global::new(scope, context))
}

unsafe extern "C" {
    fn obscura_v8_incumbent_context(isolate: v8::UnsafeRawIsolatePtr) -> *const v8::Context;
}

pub type WindowId = u32;

#[derive(Clone, Debug)]
pub struct WindowInfo {
    pub id: WindowId,
    pub target_id: String,
    pub frame_id: String,
    pub opener_id: Option<WindowId>,
    pub opener_frame_id: Option<u32>,
    pub name: String,
    pub url: String,
    pub title: String,
    pub closed: bool,
    pub can_access_opener: bool,
    pub created_by_script: bool,
}

#[derive(Clone, Debug)]
pub enum WindowEvent {
    Open { source: WindowId, source_frame_id: u32, url: String, name: String,
        features: Vec<String>, user_gesture: bool },
    Created(WindowInfo),
    Navigated(WindowInfo),
    Closed(WindowInfo),
}

#[derive(Clone)]
pub struct WindowNavigation {
    pub id: WindowId,
    pub url: String,
    pub method: String,
    pub body: String,
    /// Native frame id of the document that queued the navigation (0 root).
    pub origin_frame_id: u32,
    pub source_url: String,
    pub noreferrer: bool,
    pub generation: u64,
}

pub(crate) struct WindowRecord {
    pub id: WindowId,
    pub realm_id: u32,
    pub target_id: String,
    pub group: WindowId,
    pub opener: Option<WindowId>,
    pub opener_frame: Option<u32>,
    pub name: String,
    pub created_by_script: bool,
    pub closed: bool,
    pub debugger_wait: bool,
    pub popup_debugger_wait: bool,
    pub state: SharedState,
    /// Strong handle to the current document's context while the browsing
    /// context is live; taken on close so a retired context is collected once
    /// author JavaScript stops holding its global proxy. Operations issued by
    /// a retained closed proxy read their document from the context-owned
    /// `DocumentStateSlot`, never from this registry.
    pub context: Option<v8::Global<v8::Context>>,
    pub module_realm: deno_core::BrowserRealm,
    pub session_storage: SharedStorage,
    pub loader: Rc<ObscuraModuleLoader>,
    pub navigation_generation: u64,
}

pub(crate) struct WindowMessage {
    pub target: WindowId,
    pub source_realm: u32,
    pub source_window: WindowId,
    pub source_generation: u64,
    pub origin: String,
    pub target_origin: String,
    pub bytes: Vec<u8>,
    pub transfers: Vec<v8::SharedRef<v8::BackingStore>>,
}

pub struct WindowRegistry {
    pub(crate) windows: HashMap<WindowId, WindowRecord>,
    pub(crate) events: VecDeque<WindowEvent>,
    pub(crate) created: VecDeque<WindowId>,
    pub(crate) navigations: VecDeque<WindowNavigation>,
    pub(crate) messages: VecDeque<WindowMessage>,
    pub(crate) message_bytes: usize,
    pub(crate) engine_id: WindowId,
    pub(crate) local_storage: SharedStorage,
    pub(crate) callbacks: HashMap<u32, WindowCallbacks>,
}

pub(crate) fn allocate_navigable_id() -> Option<u32> {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);
    NEXT.try_update(std::sync::atomic::Ordering::Relaxed,
        std::sync::atomic::Ordering::Relaxed, |id| id.checked_add(1)).ok()
}

impl WindowRegistry {
    pub(crate) fn new(id: WindowId, context: v8::Global<v8::Context>,
        state: SharedState, module_realm: deno_core::BrowserRealm,
        loader: Rc<ObscuraModuleLoader>) -> Self {
        let mut windows = HashMap::new();
        windows.insert(id, WindowRecord { id, realm_id: 0, target_id: format!("window-{id}"),
            group: id, opener: None, opener_frame: None, name: String::new(),
            created_by_script: false, closed: false, debugger_wait: false,
            popup_debugger_wait: false, state, context: Some(context), module_realm,
            session_storage: empty_storage(), loader,
            navigation_generation: 0 });
        Self { windows, events: VecDeque::new(), created: VecDeque::new(),
            navigations: VecDeque::new(), messages: VecDeque::new(), message_bytes: 0,
            engine_id: id, local_storage: empty_storage(), callbacks: HashMap::new() }
    }

    pub fn info(&self, id: WindowId) -> Option<WindowInfo> {
        let window = self.windows.get(&id)?;
        let state = window.state.borrow();
        let can_access_opener = window.opener.and_then(|id| self.windows.get(&id))
            .is_some_and(|opener| !opener.closed && opener.group == window.group);
        Some(WindowInfo { id, target_id: window.target_id.clone(),
            frame_id: window.target_id.clone(), opener_id: window.opener,
            opener_frame_id: window.opener_frame, name: window.name.clone(),
            url: state.url.clone(), title: state.title.clone(), closed: window.closed,
            can_access_opener, created_by_script: window.created_by_script })
    }

    pub(crate) fn root_for_realm(&self, realm: u32) -> Option<WindowId> {
        self.windows.values().find(|window| window.realm_id == realm && !window.closed)
            .map(|window| window.id)
    }

    pub fn take_events(&mut self) -> Vec<WindowEvent> { self.events.drain(..).collect() }

    pub fn take_created(&mut self) -> Vec<WindowId> { self.created.drain(..).collect() }

    pub fn take_navigations(&mut self) -> Vec<WindowNavigation> {
        // Page may already have consumed its own tuple. Take each live root
        // exactly once; iframe-owned pending navigations stay on their states.
        for window in self.windows.values_mut().filter(|window| !window.closed) {
            let mut document = window.state.borrow_mut();
            let Some(navigation) = document.pending_navigation.take() else { continue };
            let source_url = document.url.clone();
            drop(document);
            window.navigation_generation = window.navigation_generation.wrapping_add(1);
            let id = window.id;
            self.navigations.retain(|job| job.id != id);
            self.navigations.push_back(WindowNavigation { id, url: navigation.url,
                method: navigation.method, body: navigation.body,
                origin_frame_id: navigation.origin_frame_id, source_url,
                noreferrer: false, generation: window.navigation_generation });
        }
        let mut ready = Vec::new();
        let count = self.navigations.len();
        for _ in 0..count {
            let navigation = self.navigations.pop_front().unwrap();
            match self.windows.get(&navigation.id) {
                Some(window) if !window.closed && window.debugger_wait => {
                    self.navigations.push_back(navigation);
                }
                Some(window) if !window.closed => ready.push(navigation),
                _ => {}
            }
        }
        ready
    }

    pub fn has_work(&self) -> bool {
        !self.created.is_empty() || !self.messages.is_empty()
            || self.windows.values().any(|window| !window.closed && !window.debugger_wait
                && window.state.borrow().pending_navigation.is_some())
            || self.navigations.iter().any(|job| self.windows.get(&job.id)
                .is_some_and(|window| !window.closed && !window.debugger_wait))
    }

    pub fn close(&mut self, id: WindowId) {
        let Some(window) = self.windows.get_mut(&id) else { return };
        if window.closed { return; }
        window.closed = true;
        let mut state = window.state.borrow_mut();
        state.document_generation = state.document_generation.wrapping_add(1);
        state.pending_navigation = None;
        drop(state);
        self.callbacks.remove(&window.realm_id);
        self.created.retain(|window| *window != id);
        self.navigations.retain(|job| job.id != id);
        self.messages.retain(|message| message.target != id);
        self.message_bytes = self.messages.iter().map(|message|
            message.bytes.len() + message.transfers.iter().map(|store| store.byte_length()).sum::<usize>()).sum();
        if let Some(info) = self.info(id) { self.events.push_back(WindowEvent::Closed(info)); }
    }

    pub fn set_target_id(&mut self, id: WindowId, target_id: &str) {
        if let Some(window) = self.windows.get_mut(&id) { window.target_id = target_id.to_string(); }
    }

    pub fn set_debugger_wait(&mut self, id: WindowId, waiting: bool) {
        if let Some(window) = self.windows.get_mut(&id) { window.debugger_wait = waiting; }
    }

    pub fn set_popup_debugger_wait(&mut self, id: WindowId, waiting: bool) {
        if let Some(window) = self.windows.get_mut(&id) { window.popup_debugger_wait = waiting; }
    }

    pub fn target_id(&self, id: WindowId) -> Option<String> {
        self.windows.get(&id).map(|window| window.target_id.clone())
    }

    pub fn engine_id(&self) -> WindowId { self.engine_id }
}

pub(crate) fn origin(state: &ObscuraState) -> String {
    state.inherited_origin.clone().unwrap_or_else(|| url::Url::parse(&state.url)
        .map(|url| url.origin().ascii_serialization()).unwrap_or_else(|_| "null".to_string()))
}

/// Opaque principal a document can hand to about:blank/srcdoc descendants.
///
/// A creator whose URL serializes to a null origin (data:, file:, empty URL)
/// owns an opaque principal even when navigation never minted one. Mint it on
/// first inheritance so children share that exact principal instead of both
/// sides having no principal (which would make them cross-origin to each
/// other).
pub(crate) fn inherited_opaque_principal(state: &SharedState) -> Option<Rc<()>> {
    let opaque = state.borrow().opaque_origin.clone();
    if opaque.is_some() {
        return opaque;
    }
    if origin(&state.borrow()) != "null" {
        return None;
    }
    state.borrow_mut().opaque_origin.get_or_insert_with(|| Rc::new(())).clone().into()
}

pub type SharedWindows = Rc<RefCell<WindowRegistry>>;

#[derive(Clone)]
pub(crate) struct WindowCallbacks {
    pub context: v8::Global<v8::Context>,
    pub initialize: v8::Global<v8::Function>,
}

fn empty_storage() -> SharedStorage {
    std::sync::Arc::new(std::sync::Mutex::new(StorageNamespace::default()))
}

pub fn allocate_window_id() -> WindowId {
    allocate_navigable_id().expect("window id space exhausted")
}

fn security_error() -> JsErrorBox {
    JsErrorBox::new("DOMExceptionSecurityError", "Blocked access to a foreign or retired document")
}

fn clone_error(message: &str) -> JsErrorBox {
    JsErrorBox::new("DOMExceptionDataCloneError", message.to_string())
}

fn root_for_document(state: &OpState, windows: &WindowRegistry, document: &SharedState)
    -> Option<WindowId>
{
    let mut realm = document.borrow().frame_id;
    let realms = state.borrow::<Rc<RefCell<RealmStates>>>().borrow();
    for _ in 0..=crate::ops::max_live_frames() {
        if let Some(root) = windows.root_for_realm(realm) { return Some(root); }
        realm = realms.frame_parent(realm)?;
    }
    None
}

pub(crate) fn context_document(state: &OpState, context: &v8::Global<v8::Context>)
    -> Result<(SharedWindows, WindowId, SharedState), JsErrorBox>
{
    let shared = state.borrow::<SharedWindows>().clone();
    let windows = shared.try_borrow().map_err(|_| JsErrorBox::generic("Window registry is busy"))?;
    let document = state.borrow::<Rc<RefCell<RealmStates>>>().borrow().by_context(context)
        .ok_or_else(security_error)?;
    let root = root_for_document(state, &windows, &document).ok_or_else(security_error)?;
    Ok((shared.clone(), root, document))
}

/// The isolate's incumbent context as a local in `scope`.
///
/// V8's incumbent is the realm of the topmost JavaScript frame: the code that
/// called into the host, not the realm a shared op function was bound to and
/// not a context an outer host entry left on the entered-context stack.
fn incumbent_context_local<'s>(scope: &v8::PinScope<'s, '_>)
    -> Option<v8::Local<'s, v8::Context>>
{
    // Shared ops' creation and entered host contexts are not the incumbent
    // principal, including when popup.eval runs inside an opener task.
    // UnsafeRawIsolatePtr is transparent over the C++ isolate pointer, unlike
    // a pointer to Rust's Isolate wrapper, which addresses a pointer holder.
    let raw = unsafe { obscura_v8_incumbent_context(scope.as_raw_isolate_ptr()) };
    let context = std::ptr::NonNull::new(raw.cast_mut())?;
    // SAFETY: rusty_v8 152.2.0's Local is repr(C), containing NonNull<T>
    // followed only by PhantomData. The shim returns a V8 local in this
    // scope, without opening a nested HandleScope. Its lifetime cannot escape
    // this scope. A missing context is rejected by the caller.
    let context: v8::Local<'s, v8::Context> = unsafe { std::mem::transmute(context) };
    Some(context)
}

pub(crate) fn caller_context(scope: &mut v8::PinScope)
    -> Result<v8::Global<v8::Context>, JsErrorBox>
{
    let context = incumbent_context_local(scope).ok_or_else(security_error)?;
    Ok(v8::Global::new(scope, context))
}

pub(crate) fn caller_window(scope: &mut v8::PinScope, state: &OpState)
    -> Result<(SharedWindows, WindowId, SharedState), JsErrorBox>
{
    let context = caller_context(scope)?;
    context_document(state, &context)
}

pub(crate) fn caller_dom_window(scope: &mut v8::PinScope, state: &OpState)
    -> Result<(SharedWindows, WindowId, SharedState, bool), JsErrorBox>
{
    let context = caller_context(scope)?;
    let (windows, root, document) = context_document(state, &context)?;
    let universal_access = state.borrow::<Rc<RefCell<RealmStates>>>().borrow()
        .context_universal_access(&context);
    Ok((windows, root, document, universal_access))
}

pub(crate) fn close_window(scope: &mut v8::PinScope, state: &OpState, id: WindowId) {
    let shared = state.borrow::<SharedWindows>();
    let realms = state.borrow::<Rc<RefCell<RealmStates>>>();
    let (context, module_realm, realm_id) = {
        let mut windows = shared.borrow_mut();
        let Some(window) = windows.windows.get_mut(&id).filter(|window| !window.closed) else { return };
        let realm_id = window.realm_id;
        // Move the only registry-owned context handle out. Author references
        // to the global proxy keep the context alive for as long as script can
        // reach it; an unreferenced retired context is left to V8's collector.
        let context = window.context.take();
        let module_realm = window.module_realm.clone();
        let registry = realms.borrow();
        let max_frames = crate::ops::max_live_frames();
        windows.callbacks.retain(|realm, _| {
            let mut realm = *realm;
            for _ in 0..=max_frames {
                if realm == realm_id { return false; }
                let Some(parent) = registry.frame_parent(realm) else { break };
                realm = parent;
            }
            true
        });
        drop(registry);
        windows.close(id);
        (context, module_realm, realm_id)
    };
    if let Some(context) = context {
        realms.borrow_mut().forget(&context);
    }
    realms.borrow_mut().forget_proxies(realm_id);
    // Each window owns an attached document map; the engine control context
    // remains alive for asynchronous work in sibling windows.
    module_realm.retire(scope);
    realms.borrow_mut().retire_modules(scope);
    state.waker.wake();
}

pub(crate) fn document_access(windows: &WindowRegistry, source_root: WindowId, source: &SharedState,
    target_root: WindowId, target: &SharedState, universal_access: bool) -> bool
{
    let Some(a) = windows.windows.get(&source_root) else { return false };
    let Some(b) = windows.windows.get(&target_root) else { return false };
    if a.closed || b.closed || a.group != b.group { return false; }
    // An isolated grant bypasses only the origin comparison, never reachability.
    if Rc::ptr_eq(source, target) || universal_access { return true; }
    same_principal(&source.borrow(), &target.borrow())
}

/// Same-origin check that keeps opaque origins unique.
///
/// Serialized origins cannot compare `"null" == "null"`: a data: document and
/// a sandboxed frame must stay isolated from each other and from their parent.
/// A child that inherited an opaque origin (initial about:blank, srcdoc) shares
/// its creator's token instead of minting a new one.
pub(crate) fn same_principal(source: &ObscuraState, target: &ObscuraState) -> bool {
    let source_origin = origin(source);
    if source_origin != "null" { return source_origin == origin(target); }
    match (&source.opaque_origin, &target.opaque_origin) {
        (Some(source), Some(target)) => Rc::ptr_eq(source, target),
        _ => false,
    }
}

/// Fresh opaque principal for a document URL, or `None` for tuple origins.
/// `file:` is treated as a storage origin, not as an access principal.
pub(crate) fn opaque_origin_for_url(url: &str) -> Option<Rc<()>> {
    match url::Url::parse(url) {
        Ok(url) if url.scheme() == "file" || url.origin().is_tuple() => None,
        _ => Some(Rc::new(())),
    }
}

pub(crate) fn storage_window(scope: &mut v8::PinScope, state: &OpState,
    receiver: v8::Local<v8::Object>) -> Result<(SharedWindows, WindowId, SharedState), JsErrorBox>
{
    let (shared, source_root, source, universal_access) = caller_dom_window(scope, state)?;
    let context = receiver.get_creation_context(scope).ok_or_else(security_error)?;
    let context = v8::Global::new(scope, context);
    let (_, root, document) = context_document(state, &context)?;
    if !document_access(&shared.borrow(), source_root, &source, root, &document, universal_access) {
        return Err(security_error());
    }
    Ok((shared, root, document))
}

// These are traced V8 fields, not persistent handles rooted by their own
// context. Retiring one execution world therefore releases only its callbacks.
const WORLD_NODE_ID_SLOT: i32 = crate::ops::FRAME_MESSAGE_SLOT + 1;
const WORLD_WRAP_NODE_SLOT: i32 = WORLD_NODE_ID_SLOT + 1;
const WORLD_EVENT_CREATE_SLOT: i32 = WORLD_WRAP_NODE_SLOT + 1;
const WORLD_EVENT_PHASE_SLOT: i32 = WORLD_EVENT_CREATE_SLOT + 1;
const WORLD_EVENT_FINISH_SLOT: i32 = WORLD_EVENT_PHASE_SLOT + 1;
const WORLD_NATIVE_INPUT_SLOT: i32 = WORLD_EVENT_FINISH_SLOT + 1;
pub(crate) const WINDOW_MESSAGE_SLOT: i32 = WORLD_NATIVE_INPUT_SLOT + 1;
const WINDOW_EVENT_NODE: u32 = u32::MAX;

pub(crate) fn initialize_world_callback_slots(context: v8::Local<v8::Context>,
    undefined: v8::Local<v8::Value>) {
    for slot in WORLD_NODE_ID_SLOT..=WINDOW_MESSAGE_SLOT {
        context.set_embedder_data(slot, undefined);
    }
}


fn world_receiver(scope: &mut v8::PinScope, state: &OpState,
    receiver: v8::Local<v8::Object>) -> Result<(v8::Global<v8::Context>, SharedState), JsErrorBox> {
    let context = receiver.get_creation_context(scope).ok_or_else(security_error)?;
    if !receiver.strict_equals(context.global(scope).into()) { return Err(security_error()); }
    let (_, _, document) = storage_window(scope, state, receiver)?;
    Ok((v8::Global::new(scope, context), document))
}

#[op2(nofast)]
fn op_world_register(scope: &mut v8::PinScope, state: &OpState,
    receiver: v8::Local<v8::Object>, node_id: v8::Local<v8::Function>,
    wrap_node: v8::Local<v8::Function>, event_create: v8::Local<v8::Function>,
    event_phase: v8::Local<v8::Function>, event_finish: v8::Local<v8::Function>,
    native_input: v8::Local<v8::Function>) -> Result<(), JsErrorBox> {
    let (handle, _) = world_receiver(scope, state, receiver)?;
    let context = v8::Local::new(scope, handle);
    let functions = [node_id, wrap_node, event_create, event_phase, event_finish, native_input];
    if functions.iter().any(|function| function.get_creation_context(scope) != Some(context)) {
        return Err(security_error());
    }
    if context.get_embedder_data(scope, WORLD_NODE_ID_SLOT).is_some_and(|value| value.is_function()) {
        return Err(JsErrorBox::type_error("Execution world callbacks are already registered"));
    }
    for (offset, function) in functions.into_iter().enumerate() {
        context.set_embedder_data(WORLD_NODE_ID_SLOT + offset as i32, function.into());
    }
    Ok(())
}


#[op2(nofast)]
fn op_world_dom_generation(scope: &mut v8::PinScope, state: &OpState,
    receiver: v8::Local<v8::Object>, tree: bool) -> Result<f64, JsErrorBox> {
    let (_, document) = world_receiver(scope, state, receiver)?;
    let owner = document.borrow();
    let dom = owner.dom.as_ref().ok_or_else(security_error)?;
    let generation = dom.topology_generation();
    Ok(if tree { generation } else {
        generation.wrapping_add(owner.activity_generation)
            .wrapping_add(dom.external_stylesheet_generation())
    } as f64)
}

fn world_function<'s, 'i>(scope: &mut v8::PinScope<'s, 'i>,
    context: &v8::Global<v8::Context>, slot: i32) -> Option<v8::Local<'s, v8::Function>> {
    let context = v8::Local::new(scope, context);
    v8::Local::<v8::Function>::try_from(context.get_embedder_data(scope, slot)?).ok()
}

// Branding-only helpers. Their native callers authenticate the creation
// context through RealmStates before exposing or using the returned node.
pub(crate) fn world_node_id(scope: &mut v8::PinScope, context: &v8::Global<v8::Context>,
    value: v8::Local<v8::Value>) -> Option<u32> {
    let callback = world_function(scope, context, WORLD_NODE_ID_SLOT)?;
    let receiver = v8::undefined(scope);
    let value = callback.call(scope, receiver.into(), &[value])?;
    value.is_uint32().then(|| value.uint32_value(scope)).flatten()
}

pub(crate) fn world_wrap_node<'s, 'i>(scope: &mut v8::PinScope<'s, 'i>,
    context: &v8::Global<v8::Context>, node: u32) -> Option<v8::Local<'s, v8::Value>> {
    let callback = world_function(scope, context, WORLD_WRAP_NODE_SLOT)?;
    let receiver = v8::undefined(scope);
    let node = v8::Integer::new_from_unsigned(scope, node);
    callback.call(scope, receiver.into(), &[node.into()])
}

// Only the authenticated native runtime can reach this captured callback.
// It mints individual input events without ambient authority: author getters,
// constructors and listeners invoked during input always remain untrusted.
pub(crate) fn world_native_input<'s, 'i>(scope: &mut v8::PinScope<'s, 'i>,
    context: &v8::Global<v8::Context>, payload: v8::Local<'s, v8::Value>)
    -> Option<v8::Local<'s, v8::Value>>
{
    let callback = world_function(scope, context, WORLD_NATIVE_INPUT_SLOT)?;
    let receiver = v8::undefined(scope);
    callback.call(scope, receiver.into(), &[payload])
}

#[op2(reentrant)]
fn op_world_node_id<'s, 'i>(scope: &mut v8::PinScope<'s, 'i>, state: &OpState,
    receiver: v8::Local<'s, v8::Object>, value: v8::Local<'s, v8::Value>) -> v8::Local<'s, v8::Value> {
    let node = (|| {
        let (_, document) = world_receiver(scope, state, receiver).ok()?;
        let object = v8::Local::<v8::Object>::try_from(value).ok()?;
        let context = object.get_creation_context(scope)?;
        let context = v8::Global::new(scope, context);
        let (_, _, owner) = context_document(state, &context).ok()?;
        if !Rc::ptr_eq(&document, &owner) { return None; }
        world_node_id(scope, &context, value)
    })();
    node.map(|node| v8::Integer::new_from_unsigned(scope, node).into())
        .unwrap_or_else(|| v8::null(scope).into())
}

#[op2(nofast, reentrant)]
fn op_world_focus(scope: &mut v8::PinScope, state: &OpState,
    receiver: v8::Local<v8::Object>, target: v8::Local<v8::Value>, update: bool) -> Result<u32, JsErrorBox> {
    let (context, document) = world_receiver(scope, state, receiver)?;
    let target = if update && !target.is_null() {
        Some(obscura_dom::NodeId::new(world_node_id(scope, &context, target)
            .filter(|node| *node != WINDOW_EVENT_NODE).ok_or_else(|| JsErrorBox::type_error("Illegal invocation"))?))
    } else { None };
    let mut owner = document.borrow_mut();
    if update {
        if target.is_some_and(|node| owner.dom.as_ref().is_none_or(|dom| !dom.is_connected(node))) {
            return Ok(WINDOW_EVENT_NODE);
        }
        owner.focused_node = target;
    }
    if owner.focused_node.is_some_and(|node| owner.dom.as_ref().is_none_or(|dom| !dom.is_connected(node))) {
        owner.focused_node = None;
    }
    Ok(owner.focused_node.map(|node| node.index() as u32).unwrap_or(WINDOW_EVENT_NODE))
}

struct WorldEventCallbacks {
    context: v8::Global<v8::Context>,
    create: v8::Global<v8::Function>,
    phase: v8::Global<v8::Function>,
    finish: v8::Global<v8::Function>,
    event: Option<v8::Global<v8::Value>>,
}

fn world_is_live(state: &OpState, context: &v8::Global<v8::Context>, document: &SharedState) -> bool {
    state.borrow::<Rc<RefCell<RealmStates>>>().borrow().by_context(context)
        .is_some_and(|owner| Rc::ptr_eq(&owner, document))
}

fn event_phase(scope: &mut v8::PinScope, callbacks: &WorldEventCallbacks,
    current: u32, capture: bool, phase: u32, status: u32) -> Option<u32> {
    let context = v8::Local::new(scope, &callbacks.context);
    let scope = &mut v8::ContextScope::new(scope, context);
    let event = v8::Local::new(scope, callbacks.event.as_ref()?);
    let callback = v8::Local::new(scope, &callbacks.phase);
    let current = v8::Integer::new_from_unsigned(scope, current);
    let capture = v8::Boolean::new(scope, capture);
    let phase = v8::Integer::new_from_unsigned(scope, phase);
    let status = v8::Integer::new_from_unsigned(scope, status);
    let receiver = v8::undefined(scope);
    // Author listeners may recursively dispatch, mutate or retire worlds.
    // No document/registry borrow, or native-input authority, survives this call.
    callback.call(scope, receiver.into(), &[event, current.into(), capture.into(), phase.into(), status.into()])?
        .uint32_value(scope)
}

#[op2(nofast, reentrant)]
fn op_world_event_dispatch(scope: &mut v8::PinScope, state: &OpState,
    receiver: v8::Local<v8::Object>, target: v8::Local<v8::Value>,
    event: v8::Local<v8::Value>) -> Result<bool, JsErrorBox> {
    let (source, document) = world_receiver(scope, state, receiver)?;
    let event_object = v8::Local::<v8::Object>::try_from(event)
        .map_err(|_| JsErrorBox::type_error("Invalid event"))?;
    let event_context = event_object.get_creation_context(scope)
        .ok_or_else(|| JsErrorBox::type_error("Invalid event"))?;
    storage_window(scope, state, event_object)?;
    let event_context = v8::Global::new(scope, event_context);
    let target = world_node_id(scope, &source, target).ok_or_else(|| JsErrorBox::type_error("Illegal invocation"))?;
    let contexts = state.borrow::<Rc<RefCell<RealmStates>>>().borrow().document_contexts(&document);
    let mut callbacks = Vec::with_capacity(contexts.len());
    for context in contexts {
        let Some(create) = world_function(scope, &context, WORLD_EVENT_CREATE_SLOT) else { continue };
        let Some(phase) = world_function(scope, &context, WORLD_EVENT_PHASE_SLOT) else { continue };
        let Some(finish) = world_function(scope, &context, WORLD_EVENT_FINISH_SLOT) else { continue };
        callbacks.push(WorldEventCallbacks { context, create: v8::Global::new(scope, create),
            phase: v8::Global::new(scope, phase), finish: v8::Global::new(scope, finish), event: None });
    }
    let source_index = callbacks.iter().position(|callbacks| callbacks.context == source).ok_or_else(security_error)?;
    let callback = v8::Local::new(scope, &callbacks[source_index].create);
    let node = v8::Integer::new_from_unsigned(scope, target);
    let original = v8::Boolean::new(scope, event_context == source);
    // Native input provenance is consumed by the originating world's private
    // callback; an author dispatch never receives ambient native authority.
    let native_input = v8::Boolean::new(scope, false);
    // One private cancellation/propagation cell is captured by every world's
    // Event state, never exposed as an Event expando or a copied global.
    let shared_flags = v8::Array::new(scope, 1);
    let zero = v8::Integer::new(scope, 0);
    shared_flags.set_index(scope, 0, zero.into());
    let undefined = v8::undefined(scope);
    let borrowed_state = if event_context == source { undefined.into() } else {
        let create = world_function(scope, &event_context, WORLD_EVENT_CREATE_SLOT)
            .ok_or_else(|| JsErrorBox::type_error("Invalid event"))?;
        let context = v8::Local::new(scope, &event_context);
        let scope = &mut v8::ContextScope::new(scope, context);
        let original = v8::Boolean::new(scope, true);
        // Probe the creating world's private brand/state before admitting a
        // foreign Event. No author property supplies this dispatch authority.
        create.call(scope, undefined.into(), &[event, undefined.into(), original.into(), native_input.into(), undefined.into()])
            .ok_or_else(|| JsErrorBox::type_error("Invalid event"))?
    };
    let original_event = {
        let context = v8::Local::new(scope, &source);
        let scope = &mut v8::ContextScope::new(scope, context);
        callback.call(scope, undefined.into(), &[event, node.into(), original.into(), native_input.into(), shared_flags.into(), borrowed_state])
            .ok_or_else(|| JsErrorBox::type_error("Invalid event"))?
    };
    callbacks[source_index].event = Some(v8::Global::new(scope, original_event));
    let metadata = event_phase(scope, &callbacks[source_index], target, false, 0, 0)
        .ok_or_else(|| JsErrorBox::type_error("Invalid event"))?;
    let mut status = metadata & 1;
    let trusted = metadata & 8 != 0;
    for index in 0..callbacks.len() {
        if index == source_index || !world_is_live(state, &callbacks[index].context, &document) { continue; }
        let callback = v8::Local::new(scope, &callbacks[index].create);
        let trusted = v8::Boolean::new(scope, trusted);
        let original = v8::Boolean::new(scope, false);
        let undefined = v8::undefined(scope);
        let context = v8::Local::new(scope, &callbacks[index].context);
        let scope = &mut v8::ContextScope::new(scope, context);
        if let Some(wrapper) = callback.call(scope, undefined.into(), &[event, node.into(), original.into(), trusted.into(), shared_flags.into()]) {
            callbacks[index].event = Some(v8::Global::new(scope, wrapper));
        }
    }
    let path = {
        let owner = document.borrow();
        let dom = owner.dom.as_ref().ok_or_else(security_error)?;
        let mut path = vec![target];
        if target != WINDOW_EVENT_NODE {
            let mut node = obscura_dom::NodeId::new(target);
            if dom.with_node(node, |_| ()).is_none() { return Err(security_error()); }
            while let Some(parent) = dom.with_node(node, |node| node.parent).flatten() {
                path.push(parent.index() as u32);
                node = parent;
            }
            if node == dom.document() && metadata & 32 == 0 { path.push(WINDOW_EVENT_NODE); }
        }
        path
    };
    // A phase callback returns None when it could not run to completion. The
    // JS dispatcher catches author listener errors per listener, so None here
    // means the invocation itself failed - notably a watchdog termination,
    // which must not be reported to the caller as a completed dispatch.
    let mut interrupted = false;
    let mut invoke = |scope: &mut v8::PinScope, current: u32, capture: bool, phase: u32, status: &mut u32| {
        for callbacks in &callbacks {
            if *status & 4 != 0 { break; }
            if callbacks.event.is_none() || !world_is_live(state, &callbacks.context, &document) { continue; }
            match event_phase(scope, callbacks, current, capture, phase, *status) {
                Some(flags) => *status |= flags & 7,
                None => {
                    interrupted = true;
                    break;
                }
            }
        }
    };
    for current in path.iter().skip(1).rev() {
        invoke(scope, *current, true, 1, &mut status);
        if status & 2 != 0 { break; }
    }
    if status & 2 == 0 {
        invoke(scope, target, true, 2, &mut status);
        if status & 4 == 0 { invoke(scope, target, false, 2, &mut status); }
    }
    if metadata & 16 != 0 && status & 2 == 0 {
        for current in path.iter().skip(1) {
            invoke(scope, *current, false, 3, &mut status);
            if status & 2 != 0 { break; }
        }
    }
    if interrupted {
        return Err(JsErrorBox::generic("Event dispatch was interrupted"));
    }
    for callbacks in &callbacks {
        if callbacks.event.is_none() || !world_is_live(state, &callbacks.context, &document) { continue; }
        let callback = v8::Local::new(scope, &callbacks.finish);
        let event = v8::Local::new(scope, callbacks.event.as_ref().unwrap());
        let flags = v8::Integer::new_from_unsigned(scope, status);
        let undefined = v8::undefined(scope);
        let context = v8::Local::new(scope, &callbacks.context);
        let scope = &mut v8::ContextScope::new(scope, context);
        callback.call(scope, undefined.into(), &[event, flags.into()]);
    }
    Ok(status & 1 == 0)
}

#[op2(fast)]
fn op_window_current(scope: &mut v8::PinScope, state: &OpState) -> Result<u32, JsErrorBox> {
    caller_window(scope, state).map(|(_, id, _)| id)
}

#[op2(nofast, reentrant)]
fn op_window_register(scope: &mut v8::PinScope, state: &OpState,
    initialize: v8::Local<v8::Function>,
    deliver: v8::Local<v8::Function>) -> Result<(), JsErrorBox>
{
    let context = initialize.get_creation_context(scope).ok_or_else(security_error)?;
    if deliver.get_creation_context(scope) != Some(context) {
        return Err(security_error());
    }
    let context = v8::Global::new(scope, context);
    let (shared, _, document) = context_document(state, &context)?;
    let realm = document.borrow().frame_id;
    let mut windows = shared.borrow_mut();
    if windows.callbacks.contains_key(&realm) { return Err(JsErrorBox::type_error("Window realm is already registered")); }
    let registry = state.borrow::<Rc<RefCell<RealmStates>>>().clone();
    if registry.borrow().proxy_record(realm, None).is_none() {
        let local = v8::Local::new(scope, &context);
        let global = local.global(scope);
        registry.borrow_mut().remember_proxy(realm, None,
            v8::Global::new(scope, global), native_window::SNAPSHOT_INDEX);
    }
    windows.callbacks.insert(realm, WindowCallbacks { context,
        initialize: v8::Global::new(scope, initialize) });
    Ok(())
}

fn resolve_url(document: &ObscuraState, value: &str) -> Result<String, JsErrorBox> {
    if value.is_empty() { return Ok("about:blank".to_string()); }
    url::Url::parse(value).or_else(|_| {
        url::Url::parse(crate::ops::document_base_url(document).as_deref().unwrap_or(&document.url))
            .and_then(|base| base.join(value))
    }).map(|url| url.to_string()).map_err(|_| JsErrorBox::type_error("Invalid window URL"))
}

fn queue_navigation(windows: &mut WindowRegistry, id: WindowId, url: String,
    source_url: String, noreferrer: bool, method: &str, body: &str, origin_frame_id: u32) -> bool
{
    let Some(window) = windows.windows.get_mut(&id) else { return false };
    if window.closed { return false; }
    // A newer native navigation supersedes a still-undrained Location setter.
    window.state.borrow_mut().pending_navigation = None;
    window.navigation_generation = window.navigation_generation.wrapping_add(1);
    let generation = window.navigation_generation;
    windows.navigations.retain(|job| job.id != id);
    windows.navigations.push_back(WindowNavigation { id, url, method: method.to_string(),
        body: body.to_string(), origin_frame_id, source_url, noreferrer, generation });
    true
}

#[op2(nofast, reentrant)]
fn op_window_open(scope: &mut v8::PinScope, state: &OpState,
    #[string] url: &str, #[string] target: &str, #[string] features: &str,
    #[string] method: &str, #[string] body: &str) -> Result<u32, JsErrorBox>
{
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let (shared, source, source_state) = caller_window(scope, state)?;
        let source_realm = source_state.borrow().frame_id;
        let origin_frame_id = {
            let windows = shared.borrow();
            if windows.windows.get(&source).is_some_and(|window| Rc::ptr_eq(&window.state, &source_state)) {
                0
            } else {
                source_state.borrow().frame_id
            }
        };
        let resolved = resolve_url(&source_state.borrow(), url)?;
        let feature_list: Vec<String> = features.split(',').map(str::trim)
            .filter(|value| !value.is_empty()).map(str::to_string).collect();
        let enabled = |name: &str| feature_list.iter().any(|feature| {
            let (key, value) = feature.split_once('=').unwrap_or((feature, "yes"));
            key.trim().eq_ignore_ascii_case(name) && !matches!(value.trim().to_ascii_lowercase().as_str(), "0" | "no" | "false")
        });
        let noreferrer = enabled("noreferrer");
        let noopener = noreferrer || enabled("noopener");
        let name = if target.is_empty() { "_blank" } else { target };
        let (callbacks, group, debugger_wait, session_storage, proxy_url) = {
            let mut windows = shared.borrow_mut();
            windows.events.push_back(WindowEvent::Open { source, source_frame_id: source_realm,
                url: resolved.clone(), name: name.to_string(), features: feature_list.clone(), user_gesture: false });
            let owner = windows.windows.get(&source).ok_or_else(security_error)?;
            let existing = if name.eq_ignore_ascii_case("_self") || name.eq_ignore_ascii_case("_parent") || name.eq_ignore_ascii_case("_top") {
                Some(source)
            } else if !noopener && !name.eq_ignore_ascii_case("_blank") {
                windows.windows.values().find(|window| !window.closed && window.group == owner.group && window.name == name).map(|window| window.id)
            } else { None };
            if let Some(id) = existing {
                if !url.is_empty() {
                    queue_navigation(&mut windows, id, resolved, source_state.borrow().url.clone(),
                        noreferrer, method, body, origin_frame_id);
                }
                return Ok(if noopener { 0 } else { id });
            }
            if windows.windows.values().filter(|window| !window.closed).count() >= 64 { return Ok(0); }
            let callbacks = windows.callbacks.get(&source_realm).cloned().ok_or_else(security_error)?;
            let storage = if noopener { empty_storage() } else {
                std::sync::Arc::new(std::sync::Mutex::new(owner.session_storage.lock()
                    .map_err(|_| JsErrorBox::generic("Storage namespace lock is poisoned"))?.clone()))
            };
            (callbacks, owner.group, owner.popup_debugger_wait, storage, owner.loader.proxy_url.clone())
        };
        let id = allocate_navigable_id().ok_or_else(|| JsErrorBox::generic("Window id space exhausted"))?;
        let realm_id = allocate_navigable_id().ok_or_else(|| JsErrorBox::generic("Realm id space exhausted"))?;
        let realms = state.borrow::<Rc<RefCell<RealmStates>>>().clone();
        let context = create_realm_context(scope, &realms, realm_id, None, None)
            .ok_or_else(|| JsErrorBox::generic("Window context creation failed"))?;
        // A context handle cannot call Local-only APIs; keep the live handle
        // for token/global/attach work and re-root it only when storing.
        let context = v8::Local::new(scope, &context);
        let undefined = v8::undefined(scope);
        crate::ops::initialize_document_realm_slots(context, undefined.into());
        let parent_opaque = if noopener { None } else {
            crate::window::inherited_opaque_principal(&source_state)
        };
        let mut child = ObscuraState::new();
        child.dom = Some(obscura_dom::parse_html("<html><head></head><body></body></html>"));
        child.frame_id = realm_id;
        {
            let parent = source_state.borrow();
            child.inherit_resources(&parent);
            // Popups inherit a profile snapshot; iframes share their page client.
            child.http_client = parent.http_client.as_ref().map(|client| std::sync::Arc::new(client.for_page()));
            #[cfg(feature = "stealth")]
            { child.stealth_client = parent.stealth_client.as_ref().map(|client| std::sync::Arc::new(client.for_page())); }
            child.intercept_tx = parent.intercept_tx.clone();
            child.intercept_page_id = format!("window-{id}");
            child.page_in_flight = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
            child.about_base_url = crate::ops::document_base_url(&parent);
            child.inherited_origin = Some(if noopener { "null".to_string() } else { origin(&parent) });
            child.opaque_origin = if noopener { Some(Rc::new(())) } else { parent_opaque.clone() };
            child.referrer = if noreferrer { String::new() } else { parent.url.clone() };
            #[cfg(feature = "render")]
            child.set_viewport(parent.viewport.0 as f64, parent.viewport.1 as f64);
        }
        let child = Rc::new(RefCell::new(child));
        let (base, imports) = {
            let child = child.borrow();
            (crate::ops::document_base_url(&child).unwrap_or_else(|| child.url.clone()), child.import_map.clone())
        };
        let loader = Rc::new(ObscuraModuleLoader::with_page_state(&base, proxy_url, &child, imports));
        let module_realm = deno_core::BrowserRealm::attach(scope, context, loader.clone());
        let parent_context = v8::Local::new(scope, &callbacks.context);
        context.set_security_token(parent_context.get_security_token(scope));
        let handle = v8::Global::new(scope, context);
        state.borrow::<Rc<RefCell<RealmStates>>>().borrow_mut().register(handle.clone(), realm_id, child.clone());
        shared.borrow_mut().windows.insert(id, WindowRecord { id, realm_id, target_id: format!("window-{id}"),
            group: if noopener { id } else { group }, opener: (!noopener).then_some(source),
            opener_frame: (!noopener).then_some(source_realm), name: if name.eq_ignore_ascii_case("_blank") { String::new() } else { name.to_string() },
            created_by_script: true, closed: false, debugger_wait, popup_debugger_wait: debugger_wait,
            state: child, context: Some(handle.clone()), module_realm, session_storage, loader,
            navigation_generation: 0 });
        let callback = v8::Local::new(scope, callbacks.initialize);
        let window = context.global(scope);
        let window_id = v8::Integer::new_from_unsigned(scope, id);
        let realm = v8::Integer::new_from_unsigned(scope, realm_id);
        let receiver = v8::undefined(scope);
        let initialized = {
            let scope = &mut v8::ContextScope::new(scope, parent_context);
            callback.call(scope, receiver.into(), &[window.into(), window_id.into(), realm.into()]).is_some()
        };
        if !initialized {
            close_window(scope, state, id);
            return Err(JsErrorBox::generic("Window initialization failed"));
        }
        if noopener { context.use_default_security_token(); }
        let mut windows = shared.borrow_mut();
        if let Some(info) = windows.info(id) { windows.events.push_back(WindowEvent::Created(info)); }
        windows.created.push_back(id);
        if !url.is_empty() && resolved != "about:blank" {
            queue_navigation(&mut windows, id, resolved, source_state.borrow().url.clone(),
                noreferrer, method, body, origin_frame_id);
        }
        Ok(if noopener { 0 } else { id })
    })).unwrap_or_else(|_| Err(JsErrorBox::generic("Window creation failed")))
}

#[op2(fast)]
fn op_window_same_origin(scope: &mut v8::PinScope, state: &OpState, id: u32) -> bool {
    let Ok((shared, source, document, universal_access)) = caller_dom_window(scope, state) else { return false };
    let windows = shared.borrow();
    windows.windows.get(&id).is_some_and(|target| document_access(&windows, source, &document, id, &target.state, universal_access))
}

#[op2(reentrant)]
fn op_window_target<'s, 'i>(scope: &mut v8::PinScope<'s, 'i>, state: &OpState,
    id: u32, realm: u32) -> v8::Local<'s, v8::Value> {
    let caller = caller_context(scope).ok();
    let target = caller.as_ref().and_then(|caller| {
        let (shared, source, _document, _universal_access) = caller_dom_window(scope, state).ok()?;
        let windows = shared.borrow();
        let target_window = windows.windows.get(&id).filter(|target| !target.closed)?;
        let source_window = windows.windows.get(&source)?;
        if source_window.closed || source_window.group != target_window.group { return None; }
        let registry = state.borrow::<Rc<RefCell<RealmStates>>>().borrow();
        let target = registry.by_frame_id(realm)?;
        let context = registry.context(realm)?;
        let caller_world = registry.world_id_of_context(caller)?;
        let target_world = registry.world_id_of_context(&context)?;
        drop(registry);
        if root_for_document(state, &windows, &target) != Some(id) { return None; }
        // A raw native proxy is only handed to the same browsing-context world.
        // Isolated worlds have their own proxy and must use the facade path;
        // V8's token check would otherwise let a same-token isolated world
        // read the author global.
        (caller_world == target_world).then_some(context)
    });
    let Some(context) = target else { return v8::undefined(scope).into() };
    let context = v8::Local::new(scope, context);
    context.global(scope).into()
}

#[op2(fast)]
fn op_window_closed(state: &OpState, id: u32) -> bool {
    state.borrow::<SharedWindows>().borrow().windows.get(&id).is_none_or(|window| window.closed)
}

#[op2]
#[string]
fn op_window_relation(scope: &mut v8::PinScope, state: &OpState, id: u32,
    #[string] name: &str, realm: v8::Local<v8::Value>) -> String {
    let Ok((shared, source, document)) = caller_window(scope, state) else { return "{}".to_string() };
    let document_realm = document.borrow().frame_id;
    let windows = shared.borrow();
    let Some(window) = windows.windows.get(&id) else { return "{}".to_string() };
    if windows.windows.get(&source).is_none_or(|source| source.group != window.group) { return "{}".to_string(); }
    let target_realm = if realm.is_undefined() {
        if id == source { document_realm } else { window.realm_id }
    } else {
        let Some(realm) = realm.is_uint32().then(|| realm.uint32_value(scope)).flatten() else {
            return "{}".to_string();
        };
        let registry = state.borrow::<Rc<RefCell<RealmStates>>>().borrow();
        let Some(target) = registry.by_frame_id(realm).filter(|_| registry.context(realm).is_some()) else {
            return "{}".to_string();
        };
        drop(registry);
        if root_for_document(state, &windows, &target) != Some(id) { return "{}".to_string(); }
        realm
    };
    let relation_realm = |top| {
        let realms = state.borrow::<Rc<RefCell<RealmStates>>>().borrow();
        let mut realm = target_realm;
        if top {
            for _ in 0..=crate::ops::max_live_frames() {
                let Some(parent) = realms.frame_parent(realm) else { break };
                realm = parent;
            }
        } else if let Some(parent) = realms.frame_parent(realm) {
            realm = parent;
        }
        realm
    };
    match name {
        "self" => serde_json::json!({"windowId": id,
            "realmId": if realm.is_undefined() { window.realm_id } else { target_realm },
            "name": window.name}).to_string(),
        "parent" => serde_json::json!({"windowId": id, "realmId": relation_realm(false)}).to_string(),
        "top" => serde_json::json!({"windowId": id, "realmId": relation_realm(true)}).to_string(),
        "opener" => window.opener.and_then(|opener| windows.windows.get(&opener)
            .filter(|opener| !opener.closed && opener.group == window.group)
            .map(|opener| serde_json::json!({"windowId": opener.id, "realmId": window.opener_frame.unwrap_or(opener.realm_id)})))
            .unwrap_or_else(|| serde_json::json!({"windowId": 0, "realmId": 0})).to_string(),
        "length" => {
            let count = state.borrow::<Rc<RefCell<RealmStates>>>().borrow()
                .document_iframe_count(target_realm);
            serde_json::json!({"length": count}).to_string()
        }
        _ => "{}".to_string(),
    }
}

#[op2(fast)]
fn op_window_source(scope: &mut v8::PinScope, state: &OpState, realm: u32) -> u32 {
    (|| {
        let (shared, source, _) = caller_window(scope, state).ok()?;
        let windows = shared.borrow();
        let registry = state.borrow::<Rc<RefCell<RealmStates>>>().borrow();
        let document = registry.by_frame_id(realm)?;
        registry.context(realm)?;
        drop(registry);
        let root = root_for_document(state, &windows, &document)?;
        let target = windows.windows.get(&root)?;
        let source_window = windows.windows.get(&source)?;
        (!target.closed && !source_window.closed
            && source_window.group == target.group).then_some(root)
    })().unwrap_or(0)
}

#[op2(fast)]
fn op_window_navigate(scope: &mut v8::PinScope, state: &OpState, id: u32,
    #[string] url: &str, _replace: bool, realm: u32,
    #[string] method: &str, #[string] body: &str) -> Result<bool, JsErrorBox>
{
    let (shared, source, document) = caller_window(scope, state)?;
    let origin_frame_id = {
        let windows = shared.borrow();
        if windows.windows.get(&source).is_some_and(|window| Rc::ptr_eq(&window.state, &document)) {
            0
        } else {
            document.borrow().frame_id
        }
    };
    let resolved = resolve_url(&document.borrow(), url)?;
    let mut windows = shared.borrow_mut();
    if windows.windows.get(&id).is_none_or(|target| target.closed || windows.windows.get(&source)
        .is_none_or(|source| source.group != target.group)) { return Err(security_error()); }
    let source_url = document.borrow().url.clone();
    if windows.windows.get(&id).is_some_and(|window| window.realm_id == realm) {
        return Ok(queue_navigation(&mut windows, id, resolved, source_url, false, method, body, origin_frame_id));
    }
    let registry = state.borrow::<Rc<RefCell<RealmStates>>>().borrow();
    let target = registry.by_frame_id(realm).filter(|_| registry.context(realm).is_some())
        .ok_or_else(security_error)?;
    drop(registry);
    if root_for_document(state, &windows, &target) != Some(id) { return Err(security_error()); }
    target.borrow_mut().pending_navigation = Some(crate::ops::PendingNavigation {
        url: resolved, method: method.to_string(), body: body.to_string(), origin_frame_id,
    });
    state.waker.wake();
    Ok(true)
}

#[op2(fast)]
fn op_window_close(scope: &mut v8::PinScope, state: &OpState, id: u32) -> Result<bool, JsErrorBox> {
    let (shared, source, _) = caller_window(scope, state)?;
    let windows = shared.borrow();
    let Some(target) = windows.windows.get(&id) else { return Ok(false) };
    if windows.windows.get(&source).is_none_or(|source| source.group != target.group) { return Err(security_error()); }
    if !target.created_by_script { return Ok(false); }
    drop(windows);
    close_window(scope, state, id);
    Ok(true)
}

#[op2(fast)]
fn op_window_command(scope: &mut v8::PinScope, state: &OpState, id: u32,
    #[string] command: &str, #[string] value: &str) -> Result<bool, JsErrorBox>
{
    let (shared, source, document, universal_access) = caller_dom_window(scope, state)?;
    let mut windows = shared.borrow_mut();
    let target = windows.windows.get(&id).ok_or_else(security_error)?;
    if !document_access(&windows, source, &document, id, &target.state, universal_access) { return Err(security_error()); }
    let target = windows.windows.get_mut(&id).unwrap();
    match command {
        "name" => target.name = value.to_string(),
        "opener-null" => { target.opener = None; target.opener_frame = None; },
        _ => return Err(JsErrorBox::type_error("Unknown window operation")),
    }
    Ok(true)
}

struct WindowClone;

impl v8::ValueSerializerImpl for WindowClone {
    fn throw_data_clone_error<'s>(&self, scope: &mut v8::PinScope<'s, '_>, message: v8::Local<'s, v8::String>) {
        let error = v8::Exception::error(scope, message);
        scope.throw_exception(error);
    }
}

impl v8::ValueDeserializerImpl for WindowClone {}

#[op2(nofast, reentrant)]
fn op_window_post_message(scope: &mut v8::PinScope, state: &OpState, id: u32,
    #[string] target_origin: &str, data: v8::Local<v8::Value>, transfer: v8::Local<v8::Value>) -> Result<bool, JsErrorBox>
{
    let (shared, source, document) = caller_window(scope, state)?;
    let (source_realm, source_generation, origin) = {
        let source = document.borrow();
        (source.frame_id, source.document_generation, origin(&source))
    };
    let target_origin = match target_origin {
        "*" => "*".to_string(),
        "/" | "" => "/".to_string(),
        value => url::Url::parse(value).map(|url| url.origin().ascii_serialization())
            .map_err(|_| JsErrorBox::new("DOMExceptionSyntaxError", "Invalid postMessage target origin"))?,
    };
    {
        let windows = shared.borrow();
        if windows.windows.get(&id).is_none_or(|target| target.closed || windows.windows.get(&source)
            .is_none_or(|source| source.group != target.group)) { return Ok(false); }
    }
    let mut transfers = Vec::new();
    let mut buffers = Vec::new();
    if !transfer.is_undefined() && !transfer.is_null() {
        let array = v8::Local::<v8::Array>::try_from(transfer).map_err(|_| clone_error("Transfer list must be an array"))?;
        for index in 0..array.length() {
            let value = array.get_index(scope, index).ok_or_else(|| clone_error("Invalid transfer list"))?;
            let buffer = v8::Local::<v8::ArrayBuffer>::try_from(value).map_err(|_| clone_error("Object is not transferable"))?;
            if buffer.was_detached() || !buffer.is_detachable() || buffers.contains(&buffer) { return Err(clone_error("Invalid or duplicate transferred ArrayBuffer")); }
            transfers.push(buffer.get_backing_store());
            buffers.push(buffer);
        }
    }
    let bytes = {
        v8::tc_scope!(let scope, scope);
        let serializer = v8::ValueSerializer::new(scope, Box::new(WindowClone));
        serializer.write_header();
        for (index, buffer) in buffers.iter().enumerate() { serializer.transfer_array_buffer(index as u32, *buffer); }
        if serializer.write_value(scope.get_current_context(), data) != Some(true) {
            return Err(clone_error("Value could not be cloned"));
        }
        serializer.release()
    };
    let transfer_bytes = transfers.iter().map(|store| store.byte_length()).sum::<usize>();
    let size = bytes.len().saturating_add(transfer_bytes);
    let mut windows = shared.borrow_mut();
    if windows.messages.len() >= 1024 || windows.message_bytes.saturating_add(size) > 64 * 1024 * 1024 {
        return Err(JsErrorBox::generic("Posted message queue exceeded its task budget"));
    }
    for buffer in buffers { if buffer.detach(None) != Some(true) { return Err(clone_error("ArrayBuffer transfer failed")); } }
    windows.message_bytes += size;
    windows.messages.push_back(WindowMessage { target: id, source_realm, source_window: source,
        source_generation, origin, target_origin, bytes, transfers });
    drop(windows);
    state.waker.wake();
    Ok(true)
}

#[op2(fast)]
fn op_window_realm_same_origin(scope: &mut v8::PinScope, state: &OpState, realm: u32) -> bool {
    let Ok((shared, source, document, universal_access)) = caller_dom_window(scope, state) else { return false };
    let windows = shared.borrow();
    if let Some(root) = windows.root_for_realm(realm) {
        return windows.windows.get(&root).is_some_and(|target|
            document_access(&windows, source, &document, root, &target.state, universal_access));
    }
    let realms = state.borrow::<Rc<RefCell<RealmStates>>>().borrow();
    let Some(target) = realms.by_frame_id(realm).filter(|_| realms.context(realm).is_some()) else { return false };
    drop(realms);
    let Some(root) = root_for_document(state, &windows, &target) else { return false };
    document_access(&windows, source, &document, root, &target, universal_access)
}

#[op2(reentrant)]
fn op_window_realm_proxy<'s, 'i>(scope: &mut v8::PinScope<'s, 'i>, state: &OpState,
    id: u32, realm: u32) -> v8::Local<'s, v8::Value>
{
    let resolved = (|| {
        let source_context = caller_context(scope).ok()?;
        let (shared, source, source_document) = context_document(state, &source_context).ok()?;
        let windows = shared.borrow();
        let target = windows.windows.get(&id)?;
        let source_window = windows.windows.get(&source)?;
        // Both ends of a relationship must still be live browsing contexts.
        // A closed window's retained proxy and a realm whose document was
        // replaced must never be handed out through the native fast path.
        if source_window.closed || target.closed
            || source_window.group != target.group { return None; }
        let realms = state.borrow::<Rc<RefCell<RealmStates>>>().borrow();
        let document = realms.by_frame_id(realm)?;
        let target_context = realms.context(realm)?;
        drop(realms);
        if root_for_document(state, &windows, &document)? != id { return None; }
        // A relationship back to the calling document retains that world's
        // identity. Other documents always expose their stable native proxy;
        // the native access handlers enforce world separation and DOM grants.
        Some(if Rc::ptr_eq(&source_document, &document) {
            source_context
        } else {
            target_context
        })
    })();
    let Some(context) = resolved else { return v8::undefined(scope).into() };
    let context = v8::Local::new(scope, context);
    context.global(scope).into()
}

pub(crate) fn deliver_messages(scope: &mut v8::PinScope, shared: &SharedWindows,
    realms: &Rc<RefCell<RealmStates>>) -> Result<(), String> {
    // One task boundary drains only the messages already queued. Messages
    // posted by handlers belong to the next turn, not a recursive dispatch.
    let messages = {
        let mut windows = shared.borrow_mut();
        windows.message_bytes = 0;
        std::mem::take(&mut windows.messages)
    };
    for message in messages {
        let target = {
            let windows = shared.borrow();
            windows.windows.get(&message.target).filter(|window| !window.closed)
                .and_then(|window| {
                    let document = window.state.borrow();
                    let current_origin = origin(&document);
                    if message.target_origin == "/" {
                        if current_origin != message.origin || (current_origin == "null"
                            && (window.id != message.source_window || window.realm_id != message.source_realm
                                || document.document_generation != message.source_generation)) { return None; }
                    } else if message.target_origin != "*"
                        && (current_origin == "null" || current_origin != message.target_origin) { return None; }
                    Some(window.state.clone())
                })
        };
        let Some(target) = target else { continue };
        let contexts = realms.borrow().document_contexts(&target);
        for context in contexts {
            // Only the same current native document's execution worlds receive
            // this delivery; handlers can retire worlds between callbacks.
            if !realms.borrow().document_contexts(&target).contains(&context) { continue; }
            let context = v8::Local::new(scope, context);
            let scope = &mut v8::ContextScope::new(scope, context);
            v8::tc_scope!(let scope, scope);
            let callback = context.get_embedder_data(scope, WINDOW_MESSAGE_SLOT)
                .and_then(|value| v8::Local::<v8::Function>::try_from(value).ok())
                .ok_or("Window message delivery is unavailable")?;
            let deserializer = v8::ValueDeserializer::new(scope, Box::new(WindowClone), &message.bytes);
            for (index, store) in message.transfers.iter().enumerate() {
                let buffer = v8::ArrayBuffer::with_backing_store(scope, store);
                deserializer.transfer_array_buffer(index as u32, buffer);
            }
            if deserializer.read_header(context) != Some(true) { return Err("Posted message clone header is invalid".to_string()); }
            let data = deserializer.read_value(context).ok_or("Posted message could not be deserialized")?;
            let origin = v8::String::new(scope, &message.origin).ok_or("Message origin is too large")?;
            let source = v8::Integer::new_from_unsigned(scope, message.source_window);
            let source_realm = v8::Integer::new_from_unsigned(scope, message.source_realm);
            let receiver = v8::undefined(scope);
            if callback.call(scope, receiver.into(), &[data, origin.into(), source.into(), source_realm.into()]).is_none() {
                tracing::warn!("page message handler threw: {}", scope.exception()
                    .map(|error| error.to_rust_string_lossy(scope)).unwrap_or_default());
            }
            scope.perform_microtask_checkpoint();
        }
    }
    Ok(())
}

// Values shared only with the native shim's WindowAccess enum. Keeping this
// callback in a private internal field avoids snapshotting a runtime callback
// or exposing a script-callable authority query.
fn native_window_access(scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments, mut result: v8::ReturnValue) {
    let access = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let source = args.get(0).to_object(scope)?.get_creation_context(scope)?;
        let target = args.get(1).to_object(scope)?.get_creation_context(scope)?;
        let source = v8::Global::new(scope, source);
        let target = v8::Global::new(scope, target);
        let op_state = deno_core::JsRuntime::op_state_from(scope);
        let state = op_state.try_borrow().ok()?;
        let windows = state.try_borrow::<SharedWindows>()?.try_borrow().ok()?;
        let registry = state.try_borrow::<Rc<RefCell<RealmStates>>>()?.try_borrow().ok()?;
        // Every query requires entry_active: a retained function from a
        // disposed world cannot recover its former document's grant.
        let source_world = registry.world_id_of_context(&source)?;
        let target_world = registry.world_id_of_context(&target)?;
        let universal_access = registry.context_universal_access(&source);
        let source = registry.by_context(&source)?;
        let target = registry.by_context(&target)?;
        drop(registry);
        let source_window = root_for_document(&state, &windows, &source)?;
        let target_window = root_for_document(&state, &windows, &target)?;
        if source_world == target_world && document_access(&windows, source_window,
            &source, target_window, &target, false) {
            Some(1)
        } else if source_world.is_some() && target_world.is_none() && universal_access
            && document_access(&windows, source_window, &source, target_window, &target, true) {
            Some(2)
        } else {
            None
        }
    })).ok().flatten().unwrap_or(0);
    result.set_int32(access);
}

/// Installs the target world's cross-origin Window bundle and its live native
/// authorization callback in private global internal fields. Both are bound
/// after snapshot restoration, never serialized as runtime external references.
#[op2(nofast, reentrant)]
fn op_install_window_bundle(scope: &mut v8::PinScope,
    bundle: v8::Local<v8::Object>) -> Result<(), JsErrorBox> {
    let Some(context) = bundle.get_creation_context(scope) else {
        return Err(security_error());
    };
    // The bundle is created by the realm running __obscura_init. A shared op
    // executes with the op function's binding realm as V8's current context,
    // and an outer host entry may have the creator's realm entered; only the
    // incumbent names the JavaScript frame that actually built this bundle.
    let incumbent = incumbent_context_local(scope).ok_or_else(security_error)?;
    if context != incumbent { return Err(security_error()); }
    let global = context.global(scope);
    let scope = &mut v8::ContextScope::new(scope, context);
    let access = v8::Function::new(scope, native_window_access).ok_or_else(security_error)?;
    if !global.set_internal_field(0, bundle.into())
        || !global.set_internal_field(1, access.into()) { return Err(security_error()); }
    Ok(())
}

/// Returns a member function from a foreign world's document bundle without
/// exposing that world's raw global. Used by isolated-world bootstrap for
/// native state that must stay shared with the canonical document.
#[op2(reentrant)]
fn op_window_realm_member<'s>(scope: &mut v8::PinScope<'s, '_>, state: &OpState,
    id: u32, realm: u32, #[string] name: &str) -> v8::Local<'s, v8::Value> {
    let resolved = (|| {
        let source_context = caller_context(scope).ok()?;
        let (shared, source, _document) = context_document(state, &source_context).ok()?;
        let windows = shared.borrow();
        let target_window = windows.windows.get(&id).filter(|target| !target.closed)?;
        if windows.windows.get(&source)?.group != target_window.group { return None; }
        let registry = state.borrow::<Rc<RefCell<RealmStates>>>().borrow();
        let target_document = registry.by_frame_id(realm)?;
        let context = registry.context(realm)?;
        drop(registry);
        if root_for_document(state, &windows, &target_document)? != id { return None; }
        Some(context)
    })();
    let Some(context) = resolved else { return v8::undefined(scope).into() };
    let context = v8::Local::new(scope, context);
    let scope = &mut v8::ContextScope::new(scope, context);
    let Some(members) = context.get_embedder_data(scope, crate::ops::DOCUMENT_MEMBERS_SLOT)
        .and_then(|value| value.to_object(scope)) else {
        return v8::undefined(scope).into();
    };
    let Some(key) = v8::String::new(scope, name) else { return v8::undefined(scope).into() };
    members.get(scope, key.into()).unwrap_or_else(|| v8::undefined(scope).into())
}


pub(crate) fn ops() -> Vec<OpDecl> {
    vec![op_window_current(), op_window_register(), op_window_open(), op_window_target(),
        op_window_same_origin(), op_window_closed(), op_window_relation(), op_window_source(),
        op_window_navigate(), op_window_close(), op_window_command(), op_window_post_message(),
        op_window_realm_proxy(), op_window_realm_same_origin(), op_install_window_bundle(), op_window_realm_member(), op_world_register(),
        op_world_dom_generation(), op_world_node_id(), op_world_focus(), op_world_event_dispatch()]
}
