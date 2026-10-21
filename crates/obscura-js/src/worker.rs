//! Dedicated Workers own an isolate and OS thread. Messaging carries only
//! serialized bytes, backing stores and native endpoint ownership, never V8
//! handles or page Rc state across threads.
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use deno_core::{op2, v8, CancelHandle, OpDecl, OpState};
use deno_core::v8::{ValueDeserializerHelper, ValueSerializerHelper};
use deno_error::JsErrorBox;
use crate::ops::{ObscuraState, SharedState};

#[path = "worker_thread.rs"]
mod thread;
use thread::{Incoming, RemoteWorker, ThreadLink, PortRouter};
#[path = "worker_module.rs"]
mod modules;
#[path = "worker_clone.rs"]
mod clone;

const BOOTSTRAP: &str = include_str!("../js/worker.js");
const MAX_MESSAGE_BYTES: usize = 64 * 1024 * 1024;
const MAX_TASKS: usize = 1024;
type SharedWorkers = Rc<RefCell<Workers>>;

#[derive(Default)]
pub(crate) struct WorkerCancellation {
    stopped: std::sync::atomic::AtomicBool,
    notify: tokio::sync::Notify,
}
impl WorkerCancellation {
    pub(crate) fn is_cancelled(&self) -> bool { self.stopped.load(std::sync::atomic::Ordering::Acquire) }
    pub(crate) fn cancel(&self) { self.stopped.store(true, std::sync::atomic::Ordering::Release); self.notify.notify_waiters(); }
    pub(crate) async fn cancelled(&self) {
        loop {
            let notified = self.notify.notified(); tokio::pin!(notified); notified.as_mut().enable();
            if self.is_cancelled() { return; }
            notified.await;
        }
    }
}
pub(crate) fn request_cancellation(scope: &mut v8::PinScope, state: &OpState,
    receiver: v8::Local<v8::Object>) -> Option<std::sync::Arc<WorkerCancellation>> {
    let context = context_of(scope, receiver).ok()?;
    let workers = state.try_borrow::<SharedWorkers>()?.borrow();
    workers.worker_id(&context)?;
    Some(workers.thread_link.as_ref()?.control.cancellation.clone())
}

struct Binding {
    context: v8::Global<v8::Context>,
    owner: SharedState,
    generation: u64,
    callbacks: v8::Global<v8::Object>,
}
struct Worker {
    creator_state: SharedState,
    owner: SharedState,
    root: SharedState,
    context: v8::Global<v8::Context>,
    cancel: Rc<CancelHandle>,
    ready: bool,
    pending: VecDeque<Envelope>,
    entry: Option<Result<crate::local_resource::LocalResource, String>>,
}
struct Port {
    context: Option<v8::Global<v8::Context>>,
    wrapper: Option<v8::Global<v8::Object>>,
    generation: u64,
    enabled: bool,
    closed: bool,
}
struct Envelope {
    bytes: Vec<u8>,
    buffers: Vec<v8::SharedRef<v8::BackingStore>>,
    ports: std::sync::Arc<[u32]>,
    keys: Vec<std::sync::Arc<crate::webcrypto::KeyData>>,
    platform: Vec<clone::PlatformValue>,
    _quota: thread::MessageLease,
}
enum Task {
    Start(u32),
    Message(u32, bool, Envelope),
    Error(u32, thread::WorkerError),
    Port(u32),
}
struct Timer {
    worker: u32,
    deadline: Instant,
    repeat: Option<Duration>,
    callback: v8::Global<v8::Function>,
}
struct Workers {
    next: u32,
    workers: HashMap<u32, Worker>,
    remotes: HashMap<u32, RemoteWorker>,
    thread_link: Option<ThreadLink>,
    input: tokio::sync::mpsc::UnboundedReceiver<Incoming>,
    sender: tokio::sync::mpsc::UnboundedSender<Incoming>,
    router: std::sync::Arc<std::sync::Mutex<PortRouter>>,
    bindings: Vec<Binding>,
    ports: HashMap<u32, Port>,
    tasks: VecDeque<Task>,
    timers: HashMap<u32, Timer>,
    sleep: Option<Pin<Box<tokio::time::Sleep>>>,
}
impl Default for Workers {
    fn default() -> Self {
        let (sender, input) = tokio::sync::mpsc::unbounded_channel();
        Self { next: 0, workers: HashMap::new(), remotes: HashMap::new(), thread_link: None,
            input, sender, router: Default::default(), bindings: Vec::new(), ports: HashMap::new(),
            tasks: VecDeque::new(), timers: HashMap::new(), sleep: None }
    }
}
impl Drop for Workers {
    fn drop(&mut self) {
        for worker in self.remotes.values() { worker.control.terminate(); }
        let mut router = self.router.lock().unwrap_or_else(|error| error.into_inner());
        for id in self.ports.keys() { router.close(*id); }
    }
}
impl Workers {
    fn id(&mut self) -> Result<u32, JsErrorBox> {
        self.next = self.next.checked_add(1).ok_or_else(|| JsErrorBox::range_error("Worker identifier space exhausted"))?;
        Ok(self.next)
    }
    fn worker_id(&self, context: &v8::Global<v8::Context>) -> Option<u32> {
        self.workers.iter().find(|(_, worker)| &worker.context == context).map(|(id, _)| *id)
    }
    fn binding(&self, context: &v8::Global<v8::Context>) -> Option<&Binding> {
        self.bindings.iter().find(|binding| &binding.context == context && { let owner = binding.owner.borrow(); owner.document_active.get() && owner.document_generation == binding.generation })
    }
    fn budget(&self) -> Result<(), JsErrorBox> {
        let pending = self.workers.values().map(|worker| worker.pending.len()).sum::<usize>();
        if self.tasks.len() + pending >= MAX_TASKS { return Err(JsErrorBox::range_error("Worker message task budget exceeded")); }
        Ok(())
    }
}
fn registry(state: &mut OpState) -> SharedWorkers {
    if let Some(value) = state.try_borrow::<SharedWorkers>() { return value.clone(); }
    let value = SharedWorkers::default(); state.put(value.clone()); value
}
fn clone_error(message: &str) -> JsErrorBox { JsErrorBox::new("DOMExceptionDataCloneError", message.to_string()) }
fn context_of(scope: &mut v8::PinScope, receiver: v8::Local<v8::Object>) -> Result<v8::Global<v8::Context>, JsErrorBox> {
    let context = receiver.get_creation_context(scope).ok_or_else(|| JsErrorBox::type_error("Execution context unavailable"))?;
    if !receiver.strict_equals(context.global(scope).into()) { return Err(JsErrorBox::type_error("Invalid Worker global receiver")); }
    Ok(v8::Global::new(scope, context))
}

/// Native authority capture used by the shared network send-stage.
pub(crate) fn receiver_state(scope: &mut v8::PinScope, state: &OpState,
    receiver: v8::Local<v8::Object>) -> Option<(SharedState, SharedState, Rc<CancelHandle>)> {
    let context = context_of(scope, receiver).ok()?;
    let workers = state.try_borrow::<SharedWorkers>()?.borrow();
    let worker = workers.workers.values().find(|worker| worker.context == context)?;
    if worker.cancel.is_canceled() || !worker.owner.borrow().document_active.get()
        || !worker.creator_state.borrow().document_active.get() { return None; }
    Some((worker.owner.clone(), worker.root.clone(), worker.cancel.clone()))
}
fn callback<'s>(scope: &mut v8::PinScope<'s, '_>, shared: &SharedWorkers,
    context: &v8::Global<v8::Context>, name: &str) -> Option<v8::Local<'s, v8::Function>> {
    let callbacks = shared.borrow().binding(context)?.callbacks.clone();
    let callbacks = v8::Local::new(scope, callbacks);
    let key = v8::String::new(scope, name)?;
    callbacks.get(scope, key.into())?.try_cast().ok()
}
fn port_id(scope: &mut v8::PinScope, port: v8::Local<v8::Object>) -> Option<u32> {
    if port.internal_field_count() != 1 { return None; }
    port.get_internal_field(scope, 0)?.try_cast::<v8::Value>().ok()?.uint32_value(scope)
}
fn owned_port(scope: &mut v8::PinScope, shared: &SharedWorkers, object: v8::Local<v8::Object>, context: &v8::Global<v8::Context>) -> Option<u32> {
    let id = port_id(scope, object)?;
    let workers = shared.borrow(); let port = workers.ports.get(&id)?;
    if workers.binding(context)?.generation != port.generation { return None; }
    if port.closed || port.context.as_ref() != Some(context) { return None; }
    let wrapper = v8::Local::new(scope, port.wrapper.as_ref()?);
    wrapper.strict_equals(object.into()).then_some(id)
}
fn new_port<'s>(scope: &mut v8::PinScope<'s, '_>, id: u32) -> Option<v8::Local<'s, v8::Object>> {
    let template = v8::ObjectTemplate::new(scope); template.set_internal_field_count(1);
    let object = template.new_instance(scope)?;
    let value = v8::Integer::new_from_unsigned(scope, id); object.set_internal_field(0, value.into());
    Some(object)
}
struct SerializePorts {
    ports: Vec<(v8::Global<v8::Object>, u32)>,
    keys: Rc<RefCell<Vec<std::sync::Arc<crate::webcrypto::KeyData>>>>,
    platform: Rc<RefCell<Vec<clone::PlatformValue>>>,
    platform_kind: v8::Global<v8::Function>,
    platform_read: v8::Global<v8::Function>,
}
impl v8::ValueSerializerImpl for SerializePorts {
    fn throw_data_clone_error<'s>(&self, scope: &mut v8::PinScope<'s, '_>, message: v8::Local<'s, v8::String>) {
        let error = v8::Exception::error(scope, message);
        let name = v8::String::new(scope, "name").unwrap(); let value = v8::String::new(scope, "DataCloneError").unwrap();
        let object = error.to_object(scope).unwrap(); object.create_data_property(scope, name.into(), value.into()); scope.throw_exception(error);
    }
    fn has_custom_host_object(&self, _isolate: &v8::Isolate) -> bool { true }
    fn is_host_object<'s>(&self, scope: &mut v8::PinScope<'s, '_>, object: v8::Local<'s, v8::Object>) -> Option<bool> {
        if object.internal_field_count() != 0 || crate::webcrypto::clone_native_key(scope, object.into()).is_some() { return Some(true); }
        let callback = v8::Local::new(scope,&self.platform_kind); let receiver = v8::undefined(scope);
        callback.call(scope,receiver.into(),&[object.into()]).map(|value| value.is_true())
    }
    fn write_host_object<'s>(&self, scope: &mut v8::PinScope<'s, '_>, object: v8::Local<'s, v8::Object>, serializer: &dyn ValueSerializerHelper) -> Option<bool> {
        let index = self.ports.iter().position(|(wrapper, _)| v8::Local::new(scope, wrapper).strict_equals(object.into()));
        if let Some(index) = index { serializer.write_uint32(0); serializer.write_uint32(index as u32); Some(true) }
        else if let Some(key) = crate::webcrypto::clone_native_key(scope, object.into()) {
            let mut keys = self.keys.borrow_mut(); let index = keys.len(); keys.push(key);
            serializer.write_uint32(1); serializer.write_uint32(index as u32); Some(true)
        } else if let Some(value) = clone::PlatformValue::read(scope,&self.platform_read,object) {
            let mut platform = self.platform.borrow_mut(); let index = platform.len(); platform.push(value);
            serializer.write_uint32(2); serializer.write_uint32(index as u32); Some(true)
        } else { let message = v8::String::new(scope, "Value is not a cloneable platform object").unwrap(); self.throw_data_clone_error(scope, message); None }
    }
}
struct DeserializePorts {
    ports: Vec<v8::Global<v8::Object>>,
    keys: Vec<std::sync::Arc<crate::webcrypto::KeyData>>,
    wrap_key: v8::Global<v8::Function>,
    platform: RefCell<Vec<Option<clone::PlatformValue>>>,
    wrap_platform: v8::Global<v8::Function>,
}
impl v8::ValueDeserializerImpl for DeserializePorts {
    fn read_host_object<'s>(&self, scope: &mut v8::PinScope<'s, '_>, reader: &dyn ValueDeserializerHelper) -> Option<v8::Local<'s, v8::Object>> {
        let mut tag = 0; let mut index = 0;
        if !reader.read_uint32(&mut tag) || !reader.read_uint32(&mut index) { return None; }
        // V8's ReadObject forbids JS execution. Native key and platform-object
        // restoration must re-enter the receiving realm's registered wrappers.
        let scope = std::pin::pin!(v8::AllowJavascriptExecutionScope::new(scope));
        let scope = &mut scope.init();
        match tag {
            0 => self.ports.get(index as usize).map(|port| v8::Local::new(scope, port)),
            1 => {
                let key = crate::webcrypto::wrap_native_key(scope, self.keys.get(index as usize)?.clone());
                let callback = v8::Local::new(scope, &self.wrap_key); let receiver = v8::undefined(scope);
                callback.call(scope, receiver.into(), &[key.into()])?.try_cast().ok()
            }
            2 => self.platform.borrow_mut().get_mut(index as usize)?.take()?.write(scope,&self.wrap_platform),
            _ => None,
        }
    }
}
fn serialize(scope: &mut v8::PinScope, shared: &SharedWorkers, context: &v8::Global<v8::Context>,
    data: v8::Local<v8::Value>, transfer: v8::Local<v8::Array>) -> Result<Envelope, JsErrorBox> {
    shared.borrow().budget()?;
    let mut buffers = Vec::new(); let mut ports = Vec::new();
    for index in 0..transfer.length() {
        let value = transfer.get_index(scope, index).ok_or_else(|| clone_error("Invalid transfer list"))?;
        if let Ok(buffer) = value.try_cast::<v8::ArrayBuffer>() {
            if buffer.was_detached() || !buffer.is_detachable() || buffers.contains(&buffer) { return Err(clone_error("Invalid or duplicate ArrayBuffer")); }
            buffers.push(buffer);
        } else if let Ok(port) = value.try_cast::<v8::Object>() {
            let id = owned_port(scope, shared, port, context).ok_or_else(|| clone_error("Object is not a transferable owned MessagePort"))?;
            if ports.iter().any(|(_, previous)| *previous == id) { return Err(clone_error("Duplicate MessagePort")); }
            ports.push((v8::Global::new(scope, port), id));
        } else { return Err(clone_error("Object is not transferable")); }
    }
    let keys = Rc::new(RefCell::new(Vec::new()));
    let platform = Rc::new(RefCell::new(Vec::new()));
    let platform_kind = callback(scope,shared,context,"platformKind").ok_or_else(|| clone_error("Clone realm is unavailable"))?;
    let platform_kind = v8::Global::new(scope,platform_kind);
    let platform_read = callback(scope,shared,context,"platformRead").ok_or_else(|| clone_error("Clone realm is unavailable"))?;
    let platform_read = v8::Global::new(scope,platform_read);
    let bytes = {
        v8::tc_scope!(let scope, scope);
        let serializer = v8::ValueSerializer::new(scope, Box::new(SerializePorts { ports: ports.clone(), keys: keys.clone(), platform: platform.clone(), platform_kind, platform_read }));
        serializer.write_header();
        for (index, buffer) in buffers.iter().enumerate() { serializer.transfer_array_buffer(index as u32, *buffer); }
        if serializer.write_value(scope.get_current_context(), data) != Some(true) { return Err(clone_error("Value could not be cloned")); }
        serializer.release()
    };
    // Getters invoked during serialization may themselves transfer an object.
    // Revalidate the entire transaction before any detachment or ownership move.
    for buffer in &buffers { if buffer.was_detached() || !buffer.is_detachable() { return Err(clone_error("ArrayBuffer was detached during serialization")); } }
    for (wrapper, id) in &ports { let wrapper = v8::Local::new(scope, wrapper); if owned_port(scope, shared, wrapper, context) != Some(*id) { return Err(clone_error("MessagePort ownership changed during serialization")); } }
    let stores = buffers.iter().map(|buffer| buffer.get_backing_store()).collect::<Vec<_>>();
    let size = bytes.len().saturating_add(stores.iter().map(|store| store.byte_length()).sum::<usize>()).saturating_add(platform.borrow().iter().map(clone::PlatformValue::len).sum::<usize>());
    let transferred_ports: std::sync::Arc<[u32]> = ports.iter().map(|(_,id)| *id).collect::<Vec<_>>().into();
    let quota = { let workers = shared.borrow(); workers.budget()?;
        let mut router = workers.router.lock().unwrap_or_else(|error| error.into_inner()); router.reserve(size,transferred_ports.clone())? };
    let mut envelope = Envelope { bytes, buffers: stores, ports: transferred_ports, keys: std::mem::take(&mut *keys.borrow_mut()), platform: std::mem::take(&mut *platform.borrow_mut()), _quota: quota };
    for buffer in buffers { if buffer.detach(None) != Some(true) { return Err(clone_error("ArrayBuffer transfer failed")); } }
    let mut workers = shared.borrow_mut();
    for (_, id) in ports {
        workers.ports.remove(&id);
        workers.router.lock().unwrap_or_else(|error| error.into_inner()).transfer(id);
    }
    envelope._quota.armed = true;
    Ok(envelope)
}
fn deserialize<'s>(scope: &mut v8::PinScope<'s, '_>, shared: &SharedWorkers, context: &v8::Global<v8::Context>,
    message: Envelope) -> Option<(v8::Local<'s, v8::Value>, v8::Local<'s, v8::Array>)> {
    let Envelope { bytes, buffers, ports: transferred_ports, keys, platform, mut _quota } = message;
    let factory = callback(scope, shared, context, "port")?;
    let mut ports = Vec::new();
    for id in transferred_ports.iter() {
        let port = new_port(scope, *id)?;
        let receiver = v8::undefined(scope);
        let wrapped = factory.call(scope, receiver.into(), &[port.into()])?.try_cast::<v8::Object>().ok()?;
        let handle = v8::Global::new(scope, wrapped);
        let mut workers = shared.borrow_mut();
        let generation = workers.binding(context)?.generation;
        let port = workers.ports.entry(*id).or_insert_with(|| Port { context: None, wrapper: None, generation, enabled: false, closed: false });
        port.context = Some(context.clone()); port.wrapper = Some(handle.clone());
        workers.router.lock().unwrap_or_else(|error| error.into_inner()).claim(*id, &workers.sender);
        ports.push(handle);
    }
    let wrap_key = callback(scope, shared, context, "key")?;
    let wrap_key = v8::Global::new(scope, wrap_key);
    let wrap_platform = callback(scope,shared,context,"platformWrite")?;
    let wrap_platform = v8::Global::new(scope,wrap_platform);
    let deserializer = v8::ValueDeserializer::new(scope, Box::new(DeserializePorts { ports: ports.clone(), keys, wrap_key, platform: RefCell::new(platform.into_iter().map(Some).collect()), wrap_platform }), &bytes);
    for (index, store) in buffers.iter().enumerate() { let buffer = v8::ArrayBuffer::with_backing_store(scope, store); deserializer.transfer_array_buffer(index as u32, buffer); }
    let local_context = v8::Local::new(scope, context);
    if deserializer.read_header(local_context) != Some(true) { return None; }
    let data = deserializer.read_value(local_context)?;
    let values = ports.iter().map(|port| v8::Local::new(scope, port).into()).collect::<Vec<v8::Local<v8::Value>>>();
    _quota.armed = false;
    Some((data, v8::Array::new_with_elements(scope, &values)))
}

#[op2(fast, reentrant)]
fn op_worker_bind(scope: &mut v8::PinScope, state: Rc<RefCell<OpState>>, receiver: v8::Local<v8::Object>, callbacks: v8::Local<v8::Object>) -> Result<(), JsErrorBox> {
    let context = context_of(scope, receiver)?;
    let shared = registry(&mut state.borrow_mut());
    let owner = if let Some((owner, _, _)) = receiver_state(scope, &state.borrow(), receiver) { owner }
        else { crate::window::context_document(&state.borrow(), &context)?.2 };
    let callbacks = v8::Global::new(scope, callbacks);
    let generation = owner.borrow().document_generation;
    let mut workers = shared.borrow_mut();
    if let Some(binding) = workers.bindings.iter_mut().find(|binding| binding.context == context) { binding.callbacks = callbacks; binding.owner = owner; binding.generation = generation; }
    else { workers.bindings.push(Binding { context, owner, callbacks, generation }); }
    Ok(())
}
fn creator_identity(scope: &mut v8::PinScope, receiver: v8::Local<v8::Object>) -> serde_json::Value {
    let mut identity = serde_json::Map::new();
    for name in ["__obscura_ua", "__obscura_platform", "__obscura_ua_platform", "__obscura_ua_platform_version", "__obscura_language", "__obscura_languages", "__obscura_stealth"] {
        let key = v8::String::new(scope,name).unwrap();
        let Some(descriptor) = receiver.get_own_property_descriptor(scope,key.into()) else { continue; };
        let Ok(descriptor) = descriptor.try_cast::<v8::Object>() else { continue; };
        let value_key = v8::String::new(scope,"value").unwrap();
        let Some(value) = descriptor.get(scope,value_key.into()) else { continue; };
        if value.is_string() {
            let value = value.to_rust_string_lossy(scope);
            if value.len() <= 4096 { identity.insert(name.to_string(),value.into()); }
        } else if value.is_boolean() { identity.insert(name.to_string(),value.is_true().into()); }
        else if name == "__obscura_languages" {
            if let Ok(values) = value.try_cast::<v8::Array>() {
                let mut languages = Vec::new();
                for index in 0..values.length().min(32) {
                    if let Some(value) = values.get_index(scope,index).filter(|value| value.is_string()) {
                        let value = value.to_rust_string_lossy(scope); if value.len() <= 128 { languages.push(value); }
                    }
                }
                identity.insert(name.to_string(),serde_json::json!(languages));
            }
        }
    }
    identity.into()
}
#[op2(fast, reentrant)]
fn op_worker_create(scope: &mut v8::PinScope, state: Rc<RefCell<OpState>>, #[string] value: String, #[string] name: String,
    #[string] kind: String, receiver: v8::Local<v8::Object>) -> Result<u32, JsErrorBox> {
    if kind != "classic" && kind != "module" { return Err(JsErrorBox::type_error("Invalid Worker type")); }
    let creator = context_of(scope, receiver)?;
    let (creator_state, root) = if let Some((owner, root, _)) = receiver_state(scope, &state.borrow(), receiver) { (owner, root) }
        else { let (windows, root, owner) = crate::window::context_document(&state.borrow(), &creator)?;
            let page = windows.borrow().windows.get(&root).ok_or_else(|| JsErrorBox::type_error("Worker creator is closed"))?.state.clone(); (owner, page) };
    let url = {
        let parent = creator_state.borrow();
        if !parent.document_active.get() || !parent.scripting_enabled { return Err(JsErrorBox::new("DOMExceptionSecurityError", "Worker creator cannot execute scripts")); }
        let base = crate::ops::document_base_url(&parent).unwrap_or_else(|| parent.url.clone());
        let url = url::Url::parse(&base).and_then(|base| base.join(&value)).or_else(|_| url::Url::parse(&value))
            .map_err(|_| JsErrorBox::new("DOMExceptionSyntaxError", "Invalid Worker URL"))?;
        if !matches!(url.scheme(), "http" | "https" | "blob" | "data" | "file") { return Err(JsErrorBox::new("DOMExceptionSecurityError", "Unsupported Worker scheme")); }
        let origin = crate::window::origin(&parent);
        if matches!(url.scheme(), "http" | "https") && url.origin().ascii_serialization() != origin
            || url.scheme() == "file" && !parent.url.starts_with("file:") { return Err(JsErrorBox::new("DOMExceptionSecurityError", "Worker entry must be same-origin")); }
        policy_check(&creator_policies(&parent), &crate::window::origin(&parent), &url, true)?;
        url
    };
    let inherited_identity = state.borrow().try_borrow::<thread::WorkerIdentity>().map(|identity| identity.0.clone());
    let identity = inherited_identity.unwrap_or_else(|| {
        // Shared ops are bound in the engine context, not the authenticated
        // creator's Window. Inspect its globals inside its own access domain.
        let context = v8::Local::new(scope, &creator);
        let scope = &mut v8::ContextScope::new(scope, context);
        creator_identity(scope, receiver)
    });
    {
        let mut parent = creator_state.borrow_mut();
        if crate::window::origin(&parent) == "null" && parent.opaque_origin.is_none() && parent.local_resource_principal.is_none() {
            parent.opaque_origin = Some(Rc::new(()));
        }
    }
    let snapshot = thread::WorkerNetworkSnapshot::capture(&creator_state.borrow(), &root.borrow(), url.as_str(),identity);
    let shared = registry(&mut state.borrow_mut());
    let (id, sender, router) = {
        let mut workers = shared.borrow_mut(); workers.budget()?;
        if workers.remotes.len() >= 128 { return Err(JsErrorBox::range_error("Worker count limit exceeded")); }
        (workers.id()?, workers.sender.clone(), workers.router.clone())
    };
    let remote = thread::spawn(id, creator, creator_state, root, name, kind, snapshot, sender, router)?;
    shared.borrow_mut().remotes.insert(id, remote);
    state.borrow().waker.wake();
    Ok(id)
}
fn creator_policies(owner: &ObscuraState) -> Vec<String> {
    let mut policies = owner.content_security_policy.clone();
    if let Some(dom) = &owner.dom { for node in dom.query_selector_all("meta").unwrap_or_default() {
        if let Some(policy) = dom.with_node(node, |node| if node.get_attribute("http-equiv").is_some_and(|value| value.eq_ignore_ascii_case("content-security-policy")) { node.get_attribute("content").map(str::to_string) } else { None }).flatten() { policies.push(policy); }
    } }
    policies
}
fn policy_check(policies: &[String], source: &str, target: &url::Url, entry: bool) -> Result<(), JsErrorBox> {
    let origin = url::Url::parse(source).map(|url| url.origin().ascii_serialization()).unwrap_or_default();
    let directives: &[&str] = if entry { &["worker-src", "child-src", "script-src", "default-src"] } else { &["script-src", "default-src"] };
    if policies.iter().all(|policy| crate::embedding_policy::allows_source(policy, &origin, target, directives)) { Ok(()) }
    else { Err(JsErrorBox::new(if entry { "DOMExceptionSecurityError" } else { "DOMExceptionNetworkError" }, "Worker script blocked by Content Security Policy")) }
}
fn authorized_worker(shared: &SharedWorkers, context: &v8::Global<v8::Context>, id: u32) -> Result<u32, JsErrorBox> {
    let workers = shared.borrow();
    if id == 0 { return workers.worker_id(context).ok_or_else(|| JsErrorBox::type_error("Not a Worker global")); }
    if workers.remotes.get(&id).is_some_and(|worker| &worker.creator == context && {
        let owner = worker.creator_state.borrow(); owner.document_active.get() && owner.document_generation == worker.generation
    }) { Ok(id) }
    else { Err(JsErrorBox::new("DOMExceptionSecurityError", "Worker belongs to another execution context")) }
}
#[op2(fast, reentrant)]
fn op_worker_send(scope: &mut v8::PinScope, state: Rc<RefCell<OpState>>, id: u32, data: v8::Local<v8::Value>,
    transfer: v8::Local<v8::Array>, receiver: v8::Local<v8::Object>) -> Result<(), JsErrorBox> {
    let context = context_of(scope, receiver)?; let shared = registry(&mut state.borrow_mut());
    // A terminated Worker's public object silently discards later sends.
    if id != 0 && !shared.borrow().remotes.contains_key(&id) { return Ok(()); }
    let worker_id = authorized_worker(&shared, &context, id)?;
    let message = serialize(scope, &shared, &context, data, transfer)?;
    let workers = shared.borrow();
    if id == 0 {
        if let Some(link) = &workers.thread_link { let _ = link.parent.send(Incoming::Message(link.id, true, message)); }
    } else if let Some(worker) = workers.remotes.get(&worker_id) { let _ = worker.sender.send(Incoming::Message(0, false, message)); }
    state.borrow().waker.wake(); Ok(())
}
fn terminate(_scope: &mut v8::PinScope, shared: &SharedWorkers, id: u32) {
    let mut workers = shared.borrow_mut();
    if id == 0 {
        if let Some(link) = &workers.thread_link { link.control.cancellation.cancel(); }
        if let Some(worker) = workers.workers.get(&0) { worker.owner.borrow().document_active.set(false); worker.owner.borrow().resource_load_cancel.cancel(); worker.cancel.cancel(); }
        workers.timers.clear();
        for remote in workers.remotes.values() { remote.control.terminate(); }
        workers.remotes.clear();
    } else { workers.remotes.remove(&id); }
    workers.tasks.retain(|task| !matches!(task, Task::Message(target, _, _) | Task::Start(target) | Task::Error(target, _) if *target == id));
}
#[op2(fast, reentrant)]
fn op_worker_terminate(scope: &mut v8::PinScope, state: Rc<RefCell<OpState>>, id: u32, receiver: v8::Local<v8::Object>) -> Result<(), JsErrorBox> {
    let context = context_of(scope, receiver)?; let shared = registry(&mut state.borrow_mut());
    if id != 0 && !shared.borrow().remotes.contains_key(&id) { return Ok(()); }
    let id = authorized_worker(&shared, &context, id)?; terminate(scope, &shared, id); Ok(())
}
#[op2(fast, reentrant)]
fn op_worker_ready(scope: &mut v8::PinScope, state: Rc<RefCell<OpState>>, receiver: v8::Local<v8::Object>) -> Result<(), JsErrorBox> {
    let context = context_of(scope, receiver)?; let shared = registry(&mut state.borrow_mut());
    let id = authorized_worker(&shared, &context, 0)?;
    let mut workers = shared.borrow_mut(); let worker = workers.workers.get_mut(&id).unwrap(); worker.ready = true;
    let pending = std::mem::take(&mut worker.pending);
    workers.tasks.extend(pending.into_iter().map(|message| Task::Message(id, false, message))); state.borrow().waker.wake(); Ok(())
}
fn worker_error(scope: &mut v8::PinScope, error: v8::Local<v8::Value>) -> thread::WorkerError {
    let message = if let Ok(object) = error.try_cast::<v8::Object>() {
        let key = v8::String::new(scope,"message").unwrap();
        object.get(scope,key.into()).filter(|value| !value.is_undefined()).map(|value| value.to_rust_string_lossy(scope))
    } else { None }.unwrap_or_else(|| error.to_rust_string_lossy(scope));
    let location = v8::Exception::create_message(scope,error);
    let filename = location.get_script_resource_name(scope).filter(|value| value.is_string()).map(|value| value.to_rust_string_lossy(scope)).unwrap_or_default();
    thread::WorkerError { message, filename, line: location.get_line_number(scope).unwrap_or(0) as u32, column: location.get_start_column() as u32 + 1 }
}
fn dispatch_error(scope: &mut v8::PinScope, shared: &SharedWorkers, target: &v8::Global<v8::Context>, id: u32, error: &thread::WorkerError) -> bool {
    let local = v8::Local::new(scope,target); let scope = &mut v8::ContextScope::new(scope,local);
    let Some(function) = callback(scope,shared,target,"error") else { return false; };
    let id = v8::Integer::new_from_unsigned(scope,id); let message = v8::String::new(scope,&error.message).unwrap(); let filename = v8::String::new(scope,&error.filename).unwrap();
    let line = v8::Integer::new_from_unsigned(scope,error.line); let column = v8::Integer::new_from_unsigned(scope,error.column); let receiver = v8::undefined(scope);
    function.call(scope,receiver.into(),&[id.into(),message.into(),filename.into(),line.into(),column.into()]).is_some_and(|value| value.is_true())
}
#[op2(fast, reentrant)]
fn op_worker_failed(scope: &mut v8::PinScope, state: Rc<RefCell<OpState>>, error: v8::Local<v8::Value>, receiver: v8::Local<v8::Object>) -> Result<(), JsErrorBox> {
    let message = worker_error(scope,error);
    let context = context_of(scope, receiver)?; let shared = registry(&mut state.borrow_mut());
    authorized_worker(&shared, &context, 0)?;
    let workers = shared.borrow();
    if let Some(link) = &workers.thread_link { let _ = link.parent.send(Incoming::Error(link.id, message)); link.control.cancellation.cancel(); }
    state.borrow().waker.wake(); Ok(())
}
#[op2(fast, reentrant)]
fn op_worker_report_error(scope: &mut v8::PinScope, state: &OpState, error: v8::Local<v8::Value>, receiver: v8::Local<v8::Object>) -> Result<(), JsErrorBox> {
    let message = worker_error(scope,error);
    let context = context_of(scope, receiver)?;
    let shared = state.borrow::<SharedWorkers>(); authorized_worker(shared, &context, 0)?;
    let workers = shared.borrow();
    if let Some(link) = &workers.thread_link { let _ = link.parent.send(Incoming::Error(link.id, message)); }
    Ok(())
}
#[op2(reentrant)]
#[string]
fn op_worker_location(scope: &mut v8::PinScope, state: &OpState, receiver: v8::Local<v8::Object>) -> Result<String,JsErrorBox> {
    let context = context_of(scope,receiver)?;
    let shared = state.borrow::<SharedWorkers>(); let workers = shared.borrow();
    let worker = workers.workers.values().find(|worker| worker.context == context).ok_or_else(|| JsErrorBox::type_error("Not a Worker global"))?;
    let url = worker.owner.borrow().url.clone(); Ok(url)
}
#[op2(nofast, reentrant)]
fn op_worker_eval(scope: &mut v8::PinScope, state: Rc<RefCell<OpState>>, #[string] source: String, #[string] name: String, receiver: v8::Local<v8::Object>) -> Result<(), JsErrorBox> {
    if receiver_state(scope, &state.borrow(), receiver).is_none() { return Err(JsErrorBox::type_error("Worker execution realm is retired")); }
    let context = receiver.get_creation_context(scope).unwrap(); let scope = &mut v8::ContextScope::new(scope, context);
    v8::tc_scope!(let scope,scope);
    let source = v8::String::new(scope, &source).ok_or_else(|| JsErrorBox::range_error("Worker source is too large"))?;
    let name = v8::String::new(scope, &name).unwrap();
    let origin = v8::ScriptOrigin::new(scope, name.into(), 0, 0, false, 0, None, false, false, false, None);
    if v8::Script::compile(scope, source, Some(&origin)).and_then(|script| script.run(scope)).is_none() {
        scope.rethrow();
        return Ok(());
    }
    Ok(())
}
#[op2(reentrant)]
#[string]
fn op_worker_local_script(scope: &mut v8::PinScope, state: &OpState, #[string] value: String, receiver: v8::Local<v8::Object>) -> Result<Option<String>, JsErrorBox> {
    let (owner, _, _) = receiver_state(scope, state, receiver).ok_or_else(|| JsErrorBox::type_error("Worker execution realm is retired"))?;
    let url = url::Url::parse(&value).map_err(|_| JsErrorBox::type_error("Invalid Worker script URL"))?;
    let context = context_of(scope, receiver)?;
    let shared = state.borrow::<SharedWorkers>();
    let workers = shared.borrow(); let worker = workers.workers.values().find(|worker| worker.context == context).unwrap();
    { let owner = owner.borrow();
        if !owner.worker_entry_pending || value != owner.url { policy_check(&owner.content_security_policy,&crate::window::origin(&owner),&url,false)?; }
    }
    let resource = if value == owner.borrow().url {
        match &worker.entry {
            Some(Ok(resource)) => Some(resource.clone()),
            Some(Err(error)) => return Err(JsErrorBox::new("DOMExceptionNetworkError", error.clone())),
            None => crate::local_resource::load_local_resource(&owner.borrow(), &url)?,
        }
    } else { crate::local_resource::load_local_resource(&owner.borrow(), &url)? };
    drop(workers);
    if resource.is_some() && value == owner.borrow().url { owner.borrow_mut().worker_entry_pending = false; }
    resource.map(|resource| {
        if !crate::local_resource::is_javascript_mime(&resource.mime) { return Err(JsErrorBox::new("DOMExceptionNetworkError", "Worker script has an unsupported MIME type")); }
        Ok(obscura_net::encoding::decode_non_html(&resource.bytes, Some(&resource.mime)))
    }).transpose()
}
#[op2(fast, reentrant)]
fn op_worker_script_policy(scope: &mut v8::PinScope, state: &OpState, #[string] value: String, receiver: v8::Local<v8::Object>) -> Result<(), JsErrorBox> {
    let (owner,_,_) = receiver_state(scope,state,receiver).ok_or_else(|| JsErrorBox::type_error("Worker execution realm is retired"))?;
    let url = url::Url::parse(&value).map_err(|_| JsErrorBox::type_error("Invalid Worker script URL"))?;
    let owner = owner.borrow();
    policy_check(&owner.content_security_policy,&crate::window::origin(&owner),&url,false)
}
#[op2(reentrant)]
fn op_worker_port_pair<'s>(scope: &mut v8::PinScope<'s, '_>, state: Rc<RefCell<OpState>>, receiver: v8::Local<v8::Object>) -> Result<v8::Local<'s, v8::Array>, JsErrorBox> {
    let context = context_of(scope, receiver)?; let shared = registry(&mut state.borrow_mut());
    let (a, b) = {
        let workers = shared.borrow();
        let mut router = workers.router.lock().unwrap_or_else(|error| error.into_inner());
        router.pair(&workers.sender)?
    };
    let first = new_port(scope, a).ok_or_else(|| JsErrorBox::generic("MessagePort allocation failed"))?;
    let second = new_port(scope, b).ok_or_else(|| JsErrorBox::generic("MessagePort allocation failed"))?;
    let mut workers = shared.borrow_mut();
    let generation = workers.binding(&context).ok_or_else(|| JsErrorBox::type_error("MessagePort realm is retired"))?.generation;
    for (id, object) in [(a, first), (b, second)] { workers.ports.insert(id, Port { context: Some(context.clone()), wrapper: Some(v8::Global::new(scope, object)), generation, enabled: false, closed: false }); }
    Ok(v8::Array::new_with_elements(scope, &[first.into(), second.into()]))
}
#[op2(fast, reentrant)]
fn op_worker_port_send(scope: &mut v8::PinScope, state: Rc<RefCell<OpState>>, port: v8::Local<v8::Object>, data: v8::Local<v8::Value>, transfer: v8::Local<v8::Array>, receiver: v8::Local<v8::Object>) -> Result<(), JsErrorBox> {
    let context = context_of(scope, receiver)?; let shared = registry(&mut state.borrow_mut());
    let id = owned_port(scope, &shared, port, &context);
    for index in 0..transfer.length() {
        let value = transfer.get_index(scope,index).ok_or_else(|| clone_error("Invalid transfer list"))?;
        if let Ok(value) = value.try_cast::<v8::Object>() {
            if id.is_some() && owned_port(scope,&shared,value,&context) == id { return Err(clone_error("A MessagePort cannot transfer itself through its own endpoint")); }
        }
    }
    let message = serialize(scope, &shared, &context, data, transfer)?;
    if let Some(id) = id { shared.borrow().router.lock().unwrap_or_else(|error| error.into_inner()).send(id, message); }
    state.borrow().waker.wake();
    Ok(())
}
#[op2(fast, reentrant)]
fn op_worker_port_control(scope: &mut v8::PinScope, state: Rc<RefCell<OpState>>, port: v8::Local<v8::Object>, close: bool, receiver: v8::Local<v8::Object>) -> Result<(), JsErrorBox> {
    let context = context_of(scope, receiver)?; let shared = registry(&mut state.borrow_mut());
    let Some(id) = owned_port(scope, &shared, port, &context) else { return Ok(()); };
    let mut workers = shared.borrow_mut();
    if close { workers.ports.remove(&id); workers.router.lock().unwrap_or_else(|error| error.into_inner()).close(id); }
    else if let Some(port) = workers.ports.get_mut(&id) { if !port.enabled { port.enabled = true; workers.tasks.push_back(Task::Port(id)); } }
    drop(workers); state.borrow().waker.wake(); Ok(())
}
#[op2(reentrant)]
fn op_worker_clone<'s>(scope: &mut v8::PinScope<'s, '_>, state: Rc<RefCell<OpState>>, data: v8::Local<'s, v8::Value>, transfer: v8::Local<v8::Array>, receiver: v8::Local<v8::Object>) -> Result<v8::Local<'s, v8::Value>, JsErrorBox> {
    let context = context_of(scope, receiver)?; let shared = registry(&mut state.borrow_mut());
    let message = serialize(scope, &shared, &context, data, transfer)?;
    let local = v8::Local::new(scope, &context); let scope = &mut v8::ContextScope::new(scope, local);
    deserialize(scope, &shared, &context, message).map(|(value, _)| value).ok_or_else(|| clone_error("Value could not be deserialized"))
}
#[op2(fast, reentrant)]
fn op_worker_timer(scope: &mut v8::PinScope, state: Rc<RefCell<OpState>>, callback: v8::Local<v8::Function>, millis: f64, repeat: bool, receiver: v8::Local<v8::Object>) -> Result<u32, JsErrorBox> {
    let context = context_of(scope, receiver)?; let shared = registry(&mut state.borrow_mut()); let worker = authorized_worker(&shared, &context, 0)?;
    let mut workers = shared.borrow_mut(); if workers.timers.len() >= MAX_TASKS { return Err(JsErrorBox::range_error("Worker timer task budget exceeded")); }
    let id = workers.id()?; let delay = Duration::from_millis((millis.max(0.0).min(2_147_483_647.0)) as u64);
    workers.timers.insert(id, Timer { worker, deadline: Instant::now() + delay, repeat: repeat.then_some(delay.max(Duration::from_millis(1))), callback: v8::Global::new(scope, callback) });
    workers.sleep = None; drop(workers); state.borrow().waker.wake(); Ok(id)
}
#[op2(fast, reentrant)]
fn op_worker_clear_timer(scope: &mut v8::PinScope, state: &OpState, id: u32, receiver: v8::Local<v8::Object>) -> Result<(), JsErrorBox> {
    let context = context_of(scope, receiver)?; let shared = state.borrow::<SharedWorkers>(); let worker = authorized_worker(shared, &context, 0)?;
    let mut workers = shared.borrow_mut(); if workers.timers.get(&id).is_some_and(|timer| timer.worker == worker) { workers.timers.remove(&id); workers.sleep = None; } Ok(())
}
#[op2(fast)]
fn op_worker_now() -> f64 { static START: std::sync::LazyLock<Instant> = std::sync::LazyLock::new(Instant::now); START.elapsed().as_secs_f64() * 1000.0 }
#[op2(fast)]
fn op_worker_random(#[buffer] output: &mut [u8]) -> Result<(), JsErrorBox> {
    if output.len() > 65536 { return Err(JsErrorBox::range_error("Random byte quota exceeded")); }
    getrandom::fill(output).map_err(|error| JsErrorBox::generic(error.to_string()))
}
#[op2(fast)]
fn op_worker_console(#[string] level: &str, #[string] message: &str) { if level == "error" || level == "warn" { tracing::warn!("Worker: {message}"); } else { tracing::debug!("Worker: {message}"); } }

/// Run one native Worker task per browser turn, with a microtask checkpoint.
/// Returning true keeps the owning event loop awake only while real work exists.
pub(crate) fn poll(scope: &mut v8::PinScope, state: &Rc<RefCell<OpState>>, cx: &mut Context<'_>) -> bool {
    let Some(shared) = state.borrow().try_borrow::<SharedWorkers>().cloned() else { return false; };
    let retired = shared.borrow().remotes.iter().filter(|(_, worker)| !worker.creator_state.borrow().document_active.get()
        || worker.creator_state.borrow().document_generation != worker.generation).map(|(id, _)| *id).collect::<Vec<_>>();
    for id in retired { terminate(scope, &shared, id); }
    let stale_ports = {
        let workers = shared.borrow();
        workers.ports.iter().filter(|(_,port)| port.context.as_ref().is_some_and(|context| workers.binding(context).is_none_or(|binding| binding.generation != port.generation)))
            .map(|(id,_)| *id).collect::<Vec<_>>()
    };
    for id in stale_ports { let mut workers = shared.borrow_mut(); workers.ports.remove(&id); workers.router.lock().unwrap_or_else(|error| error.into_inner()).close(id); }
    shared.borrow().router.lock().unwrap_or_else(|error| error.into_inner()).collect();
    // Register the channel waker even at idle; replies do not depend on a
    // later evaluation or an unrelated page timer.
    for _ in 0..MAX_TASKS {
        let incoming = shared.borrow_mut().input.poll_recv(cx);
        let Poll::Ready(Some(incoming)) = incoming else { break; };
        let mut workers = shared.borrow_mut();
        match incoming {
            Incoming::Message(id, parent, message) => {
                if parent {
                    if workers.remotes.contains_key(&id) { workers.tasks.push_back(Task::Message(id, true, message)); }
                } else if workers.workers.get(&0).is_some_and(|worker| worker.ready) { workers.tasks.push_back(Task::Message(0, false, message)); }
                else if let Some(worker) = workers.workers.get_mut(&0) { worker.pending.push_back(message); }
            }
            Incoming::Error(id, message) => if workers.remotes.contains_key(&id) { workers.tasks.push_back(Task::Error(id, message)); },
            Incoming::Port(id) => workers.tasks.push_back(Task::Port(id)),
            Incoming::Receipts(id, receipts) => if let Some(worker) = workers.remotes.get(&id) {
                if worker.creator_state.borrow().document_active.get() && worker.creator_state.borrow().document_generation == worker.generation { receipts.merge(&worker.root); }
            },
            Incoming::Stop => if let Some(link) = &workers.thread_link { link.control.cancellation.cancel(); },
        }
    }
    let task = shared.borrow_mut().tasks.pop_front();
    let mut timer_task = None;
    if task.is_none() {
        let mut workers = shared.borrow_mut();
        if let Some(id) = workers.timers.iter().filter(|(_, timer)| timer.deadline <= Instant::now()).min_by_key(|(id, timer)| (timer.deadline, **id)).map(|(id, _)| *id) {
            let mut timer = workers.timers.remove(&id).unwrap(); timer_task = Some((timer.worker, timer.callback.clone()));
            if let Some(repeat) = timer.repeat { timer.deadline = Instant::now() + repeat; workers.timers.insert(id, timer); }
            workers.sleep = None;
        }
    }
    let callback_context = match &task {
        Some(Task::Start(id)) => shared.borrow().workers.get(id).map(|worker| worker.context.clone()),
        None => timer_task.as_ref().and_then(|(id, _)| shared.borrow().workers.get(id).map(|worker| worker.context.clone())),
        _ => None,
    };
    let local = callback_context.as_ref().map(|context| v8::Local::new(scope, context)).unwrap_or_else(|| scope.get_current_context());
    let scope = &mut v8::ContextScope::new(scope, local);
    v8::tc_scope!(let scope, scope);
    match task {
        Some(Task::Start(_)) => {
            if let Some(target) = callback_context.as_ref() {
                if let Some(start) = callback(scope, &shared, target, "start") { let receiver = v8::undefined(scope); start.call(scope, receiver.into(), &[]); }
            }
        }
        Some(Task::Message(id, parent, message)) => {
            let target = if parent { shared.borrow().remotes.get(&id).map(|worker| worker.creator.clone()) }
                else { shared.borrow().workers.get(&id).map(|worker| worker.context.clone()) };
            if let Some(target) = target { deliver(scope, &shared, &target, if parent { "parent" } else { "worker" }, id, None, message); }
        }
        Some(Task::Error(id, mut error)) => {
            let target = shared.borrow().remotes.get(&id).map(|worker| (worker.creator.clone(), worker.url.clone()));
            if let Some((target,filename)) = target { if error.filename.is_empty() { error.filename = filename; } dispatch_error(scope,&shared,&target,id,&error); }
        }
        Some(Task::Port(id)) => {
            let delivery = {
                let workers = shared.borrow();
                workers.ports.get(&id).and_then(|port| {
                    if port.closed || !port.enabled { return None; }
                    let message = workers.router.lock().unwrap_or_else(|error| error.into_inner()).take(id)?;
                    Some((port.context.clone()?, port.wrapper.clone()?, message))
                })
            };
            if let Some((target, wrapper, message)) = delivery {
                deliver(scope, &shared, &target, "port", id, Some(&wrapper), message);
                let pending = shared.borrow().router.lock().unwrap_or_else(|error| error.into_inner()).pending(id);
                if pending { shared.borrow_mut().tasks.push_back(Task::Port(id)); }
            }
        }
        None => if let Some((_, callback)) = timer_task {
            if callback_context.is_some() { let callback = v8::Local::new(scope, callback); let receiver = local.global(scope); callback.call(scope, receiver.into(), &[]); }
        }
    }
    if let Some(exception) = scope.exception() {
        let mut error = worker_error(scope,exception);
        scope.reset();
        let own = shared.borrow().workers.get(&0).map(|worker| (worker.context.clone(), worker.owner.borrow().url.clone()));
        if let Some((context,filename)) = own {
            if error.filename.is_empty() { error.filename = filename; }
            if !dispatch_error(scope,&shared,&context,0,&error) {
                let workers = shared.borrow(); if let Some(link) = &workers.thread_link { let _ = link.parent.send(Incoming::Error(link.id,error)); }
            }
        } else { tracing::warn!("Worker message listener threw: {}",error.message); }
    }
    scope.perform_microtask_checkpoint();
    let mut workers = shared.borrow_mut();
    if !workers.tasks.is_empty() || !workers.input.is_empty() { cx.waker().wake_by_ref(); return true; }
    let deadline = workers.timers.values().map(|timer| timer.deadline).min();
    if let Some(deadline) = deadline {
        if workers.sleep.is_none() { workers.sleep = Some(Box::pin(tokio::time::sleep_until(deadline.into()))); }
        if let Some(sleep) = &mut workers.sleep { if matches!(sleep.as_mut().poll(cx), Poll::Ready(())) { workers.sleep = None; cx.waker().wake_by_ref(); } }
        true
    } else { workers.sleep = None; !workers.remotes.is_empty() }
}

pub(crate) fn has_pending_work(state: &OpState) -> bool {
    let Some(shared) = state.try_borrow::<SharedWorkers>() else { return false; };
    let workers = shared.borrow();
    !workers.tasks.is_empty() || !workers.input.is_empty()
        || !workers.remotes.is_empty() || !workers.timers.is_empty()
}
fn deliver(scope: &mut v8::PinScope, shared: &SharedWorkers, target: &v8::Global<v8::Context>, kind: &str,
    id: u32, port: Option<&v8::Global<v8::Object>>, message: Envelope) {
    let local = v8::Local::new(scope, target); let scope = &mut v8::ContextScope::new(scope, local);
    let Some((data, ports)) = deserialize(scope, shared, target, message) else { return; };
    let Some(function) = callback(scope, shared, target, "message") else { return; };
    let kind = v8::String::new(scope, kind).unwrap();
    let id = if let Some(port) = port { v8::Local::new(scope, port).into() } else { v8::Integer::new_from_unsigned(scope, id).into() };
    let receiver = v8::undefined(scope); function.call(scope, receiver.into(), &[kind.into(), id, data, ports.into()]);
}

pub(crate) fn ops() -> Vec<OpDecl> { vec![op_worker_bind(), op_worker_create(), op_worker_send(), op_worker_terminate(),
    op_worker_ready(), op_worker_failed(), op_worker_report_error(), op_worker_location(), op_worker_eval(), op_worker_local_script(), op_worker_script_policy(),
    op_worker_port_pair(), op_worker_port_send(), op_worker_port_control(), op_worker_clone(), op_worker_timer(),
    op_worker_clear_timer(), op_worker_now(), op_worker_random(), op_worker_console()] }
