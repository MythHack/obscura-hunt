use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use deno_core::{op2, v8, OpDecl, OpState};
use deno_core::v8::{ValueDeserializerHelper, ValueSerializerHelper};
use deno_error::JsErrorBox;
use crate::ops::{ObscuraState, RealmStates, SharedState};
use crate::module_loader::ObscuraModuleLoader;
use crate::storage::{SharedStorage, StorageNamespace};

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
    pub context: v8::Global<v8::Context>,
    pub module_realm: deno_core::BrowserRealm,
    pub session_storage: SharedStorage,
    pub loader: Rc<ObscuraModuleLoader>,
    pub main_realm: bool,
    pub navigation_generation: u64,
}

pub(crate) struct WindowMessage {
    pub target: WindowId,
    pub source_realm: u32,
    pub source_window: WindowId,
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
    pub(crate) root_realms: HashMap<u32, WindowId>,
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
            popup_debugger_wait: false, state, context, module_realm,
            session_storage: empty_storage(), loader, main_realm: true,
            navigation_generation: 0 });
        Self { windows, events: VecDeque::new(), created: VecDeque::new(),
            navigations: VecDeque::new(), messages: VecDeque::new(), message_bytes: 0,
            engine_id: id, local_storage: empty_storage(), callbacks: HashMap::new(),
            root_realms: HashMap::from([(0, id)]) }
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
            let Some((url, method, body)) = document.pending_navigation.take() else { continue };
            let source_url = document.url.clone();
            drop(document);
            window.navigation_generation = window.navigation_generation.wrapping_add(1);
            let id = window.id;
            self.navigations.retain(|job| job.id != id);
            self.navigations.push_back(WindowNavigation { id, url, method, body,
                source_url, noreferrer: false, generation: window.navigation_generation });
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

pub type SharedWindows = Rc<RefCell<WindowRegistry>>;

#[derive(Clone)]
pub(crate) struct WindowCallbacks {
    pub context: v8::Global<v8::Context>,
    pub initialize: v8::Global<v8::Function>,
    pub proxy: v8::Global<v8::Function>,
    pub deliver: v8::Global<v8::Function>,
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
        realm = realms.live_frame_owner(realm)?.0;
    }
    None
}

pub(crate) fn context_document(state: &OpState, context: &v8::Global<v8::Context>)
    -> Result<(SharedWindows, WindowId, SharedState), JsErrorBox>
{
    let shared = state.borrow::<SharedWindows>().clone();
    let windows = shared.try_borrow().map_err(|_| JsErrorBox::generic("Window registry is busy"))?;
    if let Some(window) = windows.windows.values().find(|window| !window.closed && &window.context == context) {
        return Ok((shared.clone(), window.id, window.state.clone()));
    }
    let document = state.borrow::<Rc<RefCell<RealmStates>>>().borrow().by_context(context)
        .ok_or_else(security_error)?;
    let root = root_for_document(state, &windows, &document).ok_or_else(security_error)?;
    Ok((shared.clone(), root, document))
}

pub(crate) fn caller_window(scope: &mut v8::PinScope, state: &OpState)
    -> Result<(SharedWindows, WindowId, SharedState), JsErrorBox>
{
    // Shared ops' creation and entered host contexts are not the incumbent
    // principal, including when popup.eval runs inside an opener task.
    // UnsafeRawIsolatePtr is transparent over the C++ isolate pointer, unlike
    // a pointer to Rust's Isolate wrapper, which addresses a pointer holder.
    let raw = unsafe { obscura_v8_incumbent_context(scope.as_raw_isolate_ptr()) };
    let context = std::ptr::NonNull::new(raw.cast_mut()).ok_or_else(security_error)?;
    // SAFETY: rusty_v8 150.4.0's Local is repr(C), containing NonNull<T>
    // followed only by PhantomData. The shim returns a V8 local in this
    // scope, without opening a nested HandleScope. Its lifetime cannot escape
    // this conversion to a persistent handle. A missing context is rejected.
    let context: v8::Local<v8::Context> = unsafe { std::mem::transmute(context) };
    let context = v8::Global::new(scope, context);
    context_document(state, &context)
}

pub(crate) fn close_window(scope: &mut v8::PinScope, state: &OpState, id: WindowId) {
    let shared = state.borrow::<SharedWindows>();
    let realms = state.borrow::<Rc<RefCell<RealmStates>>>();
    let (context, module_realm) = {
        let mut windows = shared.borrow_mut();
        let Some(window) = windows.windows.get(&id).filter(|window| !window.closed) else { return };
        let realm_id = window.realm_id;
        let context = window.context.clone();
        let module_realm = (!window.main_realm).then(|| window.module_realm.clone());
        let registry = realms.borrow();
        let max_frames = crate::ops::max_live_frames();
        windows.callbacks.retain(|realm, _| {
            let mut realm = *realm;
            for _ in 0..=max_frames {
                if realm == realm_id { return false; }
                let Some((parent, _)) = registry.live_frame_owner(realm) else { break };
                realm = parent;
            }
            true
        });
        drop(registry);
        windows.close(id);
        (context, module_realm)
    };
    realms.borrow_mut().forget(&context);
    // The engine's base module map also serves live sibling windows. Only
    // attached document maps can be retired by a window close.
    if let Some(module_realm) = module_realm { module_realm.retire(scope); }
    state.waker.wake();
}

fn document_access(windows: &WindowRegistry, source_root: WindowId, source: &SharedState,
    target_root: WindowId, target: &SharedState) -> bool
{
    let Some(a) = windows.windows.get(&source_root) else { return false };
    let Some(b) = windows.windows.get(&target_root) else { return false };
    if a.closed || b.closed || a.group != b.group { return false; }
    if Rc::ptr_eq(source, target) { return true; }
    let a = origin(&source.borrow());
    a != "null" && a == origin(&target.borrow())
}

pub(crate) fn storage_window(scope: &mut v8::PinScope, state: &OpState,
    receiver: v8::Local<v8::Object>) -> Result<(SharedWindows, WindowId, SharedState), JsErrorBox>
{
    let (shared, source_root, source) = caller_window(scope, state)?;
    let context = receiver.get_creation_context(scope).ok_or_else(security_error)?;
    let context = v8::Global::new(scope, context);
    let (_, root, document) = context_document(state, &context)?;
    if !document_access(&shared.borrow(), source_root, &source, root, &document) {
        return Err(security_error());
    }
    Ok((shared, root, document))
}

#[op2(fast)]
fn op_window_current(scope: &mut v8::PinScope, state: &OpState) -> Result<u32, JsErrorBox> {
    caller_window(scope, state).map(|(_, id, _)| id)
}

#[op2(nofast, reentrant)]
fn op_window_register(scope: &mut v8::PinScope, state: &OpState,
    initialize: v8::Local<v8::Function>, proxy: v8::Local<v8::Function>,
    deliver: v8::Local<v8::Function>) -> Result<bool, JsErrorBox>
{
    let context = initialize.get_creation_context(scope).ok_or_else(security_error)?;
    if proxy.get_creation_context(scope) != Some(context) || deliver.get_creation_context(scope) != Some(context) {
        return Err(security_error());
    }
    let context = v8::Global::new(scope, context);
    let (shared, _, document) = context_document(state, &context)?;
    let realm = document.borrow().frame_id;
    let mut windows = shared.borrow_mut();
    if windows.callbacks.contains_key(&realm) { return Err(JsErrorBox::type_error("Window realm is already registered")); }
    let main_realm = windows.windows.values().any(|window|
        !window.closed && window.main_realm && window.context == context);
    windows.callbacks.insert(realm, WindowCallbacks { context,
        initialize: v8::Global::new(scope, initialize), proxy: v8::Global::new(scope, proxy),
        deliver: v8::Global::new(scope, deliver) });
    Ok(main_realm)
}

fn resolve_url(document: &ObscuraState, value: &str) -> Result<String, JsErrorBox> {
    if value.is_empty() { return Ok("about:blank".to_string()); }
    url::Url::parse(value).or_else(|_| {
        url::Url::parse(crate::ops::document_base_url(document).as_deref().unwrap_or(&document.url))
            .and_then(|base| base.join(value))
    }).map(|url| url.to_string()).map_err(|_| JsErrorBox::type_error("Invalid window URL"))
}

fn queue_navigation(windows: &mut WindowRegistry, id: WindowId, url: String,
    source_url: String, noreferrer: bool) -> bool
{
    let Some(window) = windows.windows.get_mut(&id) else { return false };
    if window.closed { return false; }
    // A newer native navigation supersedes a still-undrained Location setter.
    window.state.borrow_mut().pending_navigation = None;
    window.navigation_generation = window.navigation_generation.wrapping_add(1);
    let generation = window.navigation_generation;
    windows.navigations.retain(|job| job.id != id);
    windows.navigations.push_back(WindowNavigation { id, url, method: "GET".to_string(),
        body: String::new(), source_url, noreferrer, generation });
    true
}

#[op2(nofast, reentrant)]
fn op_window_open(scope: &mut v8::PinScope, state: &OpState,
    #[string] url: &str, #[string] target: &str, #[string] features: &str) -> Result<u32, JsErrorBox>
{
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let (shared, source, source_state) = caller_window(scope, state)?;
        let source_realm = source_state.borrow().frame_id;
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
                if !url.is_empty() { queue_navigation(&mut windows, id, resolved, source_state.borrow().url.clone(), noreferrer); }
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
        let context = v8::Context::from_snapshot(scope, 1, Default::default())
            .or_else(|| v8::Context::from_snapshot(scope, 0, Default::default()))
            .ok_or_else(|| JsErrorBox::generic("Window context creation failed"))?;
        let undefined = v8::undefined(scope);
        crate::ops::initialize_document_realm_slots(context, undefined.into());
        let mut child = ObscuraState::new();
        child.dom = Some(obscura_dom::parse_html("<html><head></head><body></body></html>"));
        child.frame_id = realm_id;
        {
            let parent = source_state.borrow();
            child.inherit_resources(&parent);
            child.intercept_tx = parent.intercept_tx.clone();
            child.intercept_page_id = format!("window-{id}");
            child.page_in_flight = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
            child.about_base_url = crate::ops::document_base_url(&parent);
            child.inherited_origin = Some(if noopener { "null".to_string() } else { origin(&parent) });
            child.referrer = if noreferrer { String::new() } else { parent.url.clone() };
            #[cfg(feature = "render")]
            child.set_viewport(parent.viewport.0 as f64, parent.viewport.1 as f64);
        }
        let child = Rc::new(RefCell::new(child));
        let loader = Rc::new(ObscuraModuleLoader::with_page_state("about:blank", proxy_url,
            &child, child.borrow().import_map.clone()));
        let module_realm = deno_core::BrowserRealm::attach(scope, context, loader.clone());
        let parent_context = v8::Local::new(scope, &callbacks.context);
        context.set_security_token(parent_context.get_security_token(scope));
        let handle = v8::Global::new(scope, context);
        state.borrow::<Rc<RefCell<RealmStates>>>().borrow_mut().register(handle.clone(), realm_id, child.clone());
        shared.borrow_mut().windows.insert(id, WindowRecord { id, realm_id, target_id: format!("window-{id}"),
            group: if noopener { id } else { group }, opener: (!noopener).then_some(source),
            opener_frame: (!noopener).then_some(source_realm), name: if name.eq_ignore_ascii_case("_blank") { String::new() } else { name.to_string() },
            created_by_script: true, closed: false, debugger_wait, popup_debugger_wait: debugger_wait,
            state: child, context: handle.clone(), module_realm, session_storage, loader,
            main_realm: false, navigation_generation: 0 });
        shared.borrow_mut().root_realms.insert(realm_id, id);
        let callback = v8::Local::new(scope, callbacks.initialize);
        let window = context.global(scope);
        let window_id = v8::Integer::new_from_unsigned(scope, id);
        let realm = v8::Integer::new_from_unsigned(scope, realm_id);
        let receiver = v8::undefined(scope);
        if callback.call(scope, receiver.into(), &[window.into(), window_id.into(), realm.into()]).is_none() {
            close_window(scope, state, id);
            return Err(JsErrorBox::generic("Window initialization failed"));
        }
        if noopener { context.use_default_security_token(); }
        let mut windows = shared.borrow_mut();
        if let Some(info) = windows.info(id) { windows.events.push_back(WindowEvent::Created(info)); }
        windows.created.push_back(id);
        if !url.is_empty() && resolved != "about:blank" {
            queue_navigation(&mut windows, id, resolved, source_state.borrow().url.clone(), noreferrer);
        }
        Ok(if noopener { 0 } else { id })
    })).unwrap_or_else(|_| Err(JsErrorBox::generic("Window creation failed")))
}

#[op2(fast)]
fn op_window_same_origin(scope: &mut v8::PinScope, state: &OpState, id: u32) -> bool {
    let Ok((shared, source, document)) = caller_window(scope, state) else { return false };
    let windows = shared.borrow();
    windows.windows.get(&id).is_some_and(|target| document_access(&windows, source, &document, id, &target.state))
}

#[op2]
fn op_window_target<'s, 'i>(scope: &mut v8::PinScope<'s, 'i>, state: &OpState, id: u32) -> v8::Local<'s, v8::Value> {
    let target = caller_window(scope, state).ok().and_then(|(shared, source, document)| {
        let windows = shared.borrow();
        let target = windows.windows.get(&id)?;
        document_access(&windows, source, &document, id, &target.state).then(|| target.context.clone())
    });
    match target {
        Some(context) => v8::Local::new(scope, context).global(scope).into(),
        None => v8::undefined(scope).into(),
    }
}

#[op2(fast)]
fn op_window_closed(state: &OpState, id: u32) -> bool {
    state.borrow::<SharedWindows>().borrow().windows.get(&id).is_none_or(|window| window.closed)
}

#[op2]
#[string]
fn op_window_relation(scope: &mut v8::PinScope, state: &OpState, id: u32, #[string] name: &str) -> String {
    let Ok((shared, source, _)) = caller_window(scope, state) else { return "{}".to_string() };
    let windows = shared.borrow();
    let Some(window) = windows.windows.get(&id) else { return "{}".to_string() };
    if windows.windows.get(&source).is_none_or(|source| source.group != window.group) { return "{}".to_string(); }
    match name {
        "self" => serde_json::json!({"windowId": id, "realmId": window.realm_id, "name": window.name}).to_string(),
        "parent" | "top" => serde_json::json!({"windowId": id, "realmId": window.realm_id}).to_string(),
        "opener" => window.opener.and_then(|opener| windows.windows.get(&opener)
            .filter(|opener| !opener.closed && opener.group == window.group)
            .map(|opener| serde_json::json!({"windowId": opener.id, "realmId": window.opener_frame.unwrap_or(opener.realm_id)})))
            .unwrap_or_else(|| serde_json::json!({"windowId": 0, "realmId": 0})).to_string(),
        "length" => {
            let count = state.borrow::<Rc<RefCell<RealmStates>>>().borrow().live_frame_owners().iter()
                .filter(|(_, parent, _)| *parent == window.realm_id).count();
            serde_json::json!({"length": count}).to_string()
        }
        _ => "{}".to_string(),
    }
}

#[op2(fast)]
fn op_window_source(state: &OpState, realm: u32) -> u32 {
    state.borrow::<SharedWindows>().borrow().root_realms.get(&realm).copied().unwrap_or(0)
}

#[op2(fast)]
fn op_window_navigate(scope: &mut v8::PinScope, state: &OpState, id: u32,
    #[string] url: &str, _replace: bool) -> Result<bool, JsErrorBox>
{
    let (shared, source, document) = caller_window(scope, state)?;
    let resolved = resolve_url(&document.borrow(), url)?;
    let mut windows = shared.borrow_mut();
    if windows.windows.get(&id).is_none_or(|target| target.closed || windows.windows.get(&source)
        .is_none_or(|source| source.group != target.group)) { return Err(security_error()); }
    let source_url = document.borrow().url.clone();
    Ok(queue_navigation(&mut windows, id, resolved, source_url, false))
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
    let (shared, source, document) = caller_window(scope, state)?;
    let mut windows = shared.borrow_mut();
    let target = windows.windows.get(&id).ok_or_else(security_error)?;
    if !document_access(&windows, source, &document, id, &target.state) { return Err(security_error()); }
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
    let source_realm = document.borrow().frame_id;
    let origin = origin(&document.borrow());
    let target_origin = match target_origin {
        "*" => "*".to_string(),
        "/" | "" => origin.clone(),
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
        origin, target_origin, bytes, transfers });
    drop(windows);
    state.waker.wake();
    Ok(true)
}

#[op2(fast)]
fn op_window_realm_same_origin(scope: &mut v8::PinScope, state: &OpState, realm: u32) -> bool {
    let Ok((shared, source, document)) = caller_window(scope, state) else { return false };
    let windows = shared.borrow();
    if let Some(root) = windows.root_for_realm(realm) {
        return windows.windows.get(&root).is_some_and(|target|
            document_access(&windows, source, &document, root, &target.state));
    }
    let realms = state.borrow::<Rc<RefCell<RealmStates>>>().borrow();
    let Some(target) = realms.by_frame_id(realm).filter(|_| realms.context(realm).is_some()) else { return false };
    drop(realms);
    let Some(root) = root_for_document(state, &windows, &target) else { return false };
    document_access(&windows, source, &document, root, &target)
}

#[op2(reentrant)]
fn op_window_realm_proxy<'s, 'i>(scope: &mut v8::PinScope<'s, 'i>, state: &OpState,
    id: u32, realm: u32) -> v8::Local<'s, v8::Value>
{
    let callbacks = (|| {
        let (shared, source, _) = caller_window(scope, state).ok()?;
        let windows = shared.borrow();
        let target = windows.windows.get(&id)?;
        if target.closed || windows.windows.get(&source)?.group != target.group { return None; }
        let realms = state.borrow::<Rc<RefCell<RealmStates>>>().borrow();
        let document = realms.by_frame_id(realm)?;
        realms.context(realm)?;
        drop(realms);
        if root_for_document(state, &windows, &document)? != id { return None; }
        windows.callbacks.get(&target.realm_id).cloned()
    })();
    let Some(callbacks) = callbacks else { return v8::undefined(scope).into() };
    let callback = v8::Local::new(scope, callbacks.proxy);
    let window = v8::Integer::new_from_unsigned(scope, id);
    let realm = v8::Integer::new_from_unsigned(scope, realm);
    let receiver = v8::undefined(scope);
    callback.call(scope, receiver.into(), &[window.into(), realm.into()])
        .unwrap_or_else(|| v8::undefined(scope).into())
}

pub(crate) fn deliver_messages(scope: &mut v8::PinScope, shared: &SharedWindows) -> Result<(), String> {
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
                    let current_origin = origin(&window.state.borrow());
                    if message.target_origin != "*" && (current_origin == "null" || current_origin != message.target_origin) { return None; }
                    windows.callbacks.get(&window.realm_id).cloned()
                })
        };
        let Some(target) = target else { continue };
        let context = v8::Local::new(scope, target.context);
        let scope = &mut v8::ContextScope::new(scope, context);
        v8::tc_scope!(let scope, scope);
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
        let callback = v8::Local::new(scope, target.deliver);
        let receiver = v8::undefined(scope);
        if callback.call(scope, receiver.into(), &[data, origin.into(), source.into(), source_realm.into()]).is_none() {
            tracing::warn!("page message handler threw: {}", scope.exception()
                .map(|error| error.to_rust_string_lossy(scope)).unwrap_or_default());
        }
        scope.perform_microtask_checkpoint();
    }
    Ok(())
}

pub(crate) fn ops() -> Vec<OpDecl> {
    vec![op_window_current(), op_window_register(), op_window_open(), op_window_target(),
        op_window_same_origin(), op_window_closed(), op_window_relation(), op_window_source(),
        op_window_navigate(), op_window_close(), op_window_command(), op_window_post_message(),
        op_window_realm_proxy(), op_window_realm_same_origin()]
}
