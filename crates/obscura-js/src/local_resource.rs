use std::collections::HashMap;
use std::cell::RefCell;
use std::fmt::Write;
use std::rc::Rc;
use std::sync::{Arc, Mutex, Weak};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use deno_core::{op2, v8, OpDecl, OpState};
use deno_error::JsErrorBox;
use crate::ops::ObscuraState;

#[derive(Clone)]
pub(crate) struct LocalResource { pub bytes: Arc<[u8]>, pub mime: String }

static NEXT_PRINCIPAL: AtomicU64 = AtomicU64::new(1);
thread_local! {
    static OPAQUE_PRINCIPALS: RefCell<HashMap<usize, (std::rc::Weak<()>, u64)>> = RefCell::new(HashMap::new());
}
fn opaque_principal(origin: &Rc<()>) -> u64 {
    OPAQUE_PRINCIPALS.with_borrow_mut(|principals| {
        let address = Rc::as_ptr(origin) as usize;
        if let Some((owner, id)) = principals.get(&address) {
            if owner.upgrade().is_some_and(|owner| Rc::ptr_eq(&owner, origin)) { return *id; }
        }
        principals.retain(|_, (owner, _)| owner.strong_count() != 0);
        let id = NEXT_PRINCIPAL.try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .expect("native opaque principal identity exhausted");
        principals.insert(address, (Rc::downgrade(origin), id));
        id
    })
}

/// A native principal snapshot, never deserialized from author input. The
/// opaque identity is derived while its owning native realm is authenticated.
#[derive(Clone)]
pub(crate) struct LocalResourcePrincipal { origin: String, opaque_origin: Option<u64> }
impl LocalResourcePrincipal {
    pub(crate) fn for_document(owner: &ObscuraState) -> Self {
        owner.local_resource_principal.clone().unwrap_or_else(|| Self {
            origin: crate::window::origin(owner),
            opaque_origin: owner.opaque_origin.as_ref().map(opaque_principal),
        })
    }
}

pub(crate) struct LocalResourceOwner {
    active: Arc<AtomicBool>,
    registry: Mutex<Option<Weak<Mutex<HashMap<String, ObjectUrl>>>>>,
}
impl Default for LocalResourceOwner {
    fn default() -> Self { Self { active: Arc::new(AtomicBool::new(true)), registry: Mutex::new(None) } }
}
impl LocalResourceOwner {
    pub(crate) fn retire(&self) {
        self.active.store(false, Ordering::Release);
        if let Some(registry) = self.registry.lock().unwrap_or_else(|error| error.into_inner()).take().and_then(|registry| registry.upgrade()) {
            let owner = Arc::downgrade(&self.active);
            registry.lock().unwrap_or_else(|error| error.into_inner()).retain(|_, entry| !entry.owner.ptr_eq(&owner));
        }
    }
}
impl Drop for LocalResourceOwner { fn drop(&mut self) { self.retire(); } }

struct ObjectUrl { resource: LocalResource, principal: LocalResourcePrincipal, owner: Weak<AtomicBool> }
#[derive(Clone, Default)]
pub(crate) struct LocalResources(Arc<Mutex<HashMap<String, ObjectUrl>>>);
impl LocalResources {
    fn discard_retired(&self) {
        self.0.lock().unwrap_or_else(|error| error.into_inner()).retain(|_, entry|
            entry.owner.upgrade().is_some_and(|active| active.load(Ordering::Acquire)));
    }
    pub(crate) fn load(&self, url: &url::Url, principal: &LocalResourcePrincipal) -> Result<Option<LocalResource>, JsErrorBox> {
        match url.scheme() {
            "data" => decode_data(url).map(Some),
            "blob" => {
                self.discard_retired();
                let mut url = url.clone(); url.set_fragment(None);
                let store = self.0.lock().unwrap_or_else(|error| error.into_inner());
                let entry = store.get(url.as_str()).ok_or_else(|| JsErrorBox::type_error("Object URL is revoked or unavailable"))?;
                if entry.principal.origin != principal.origin || (principal.origin == "null"
                    && (principal.opaque_origin.is_none() || entry.principal.opaque_origin != principal.opaque_origin)) {
                    return Err(JsErrorBox::new("DOMExceptionSecurityError", "Object URL belongs to another origin"));
                }
                Ok(Some(entry.resource.clone()))
            }
            _ => Ok(None),
        }
    }
}

fn decode_data(url: &url::Url) -> Result<LocalResource, JsErrorBox> {
    let mut url = url.clone(); url.set_fragment(None);
    let data = data_url::DataUrl::process(url.as_str()).map_err(|error| JsErrorBox::type_error(format!("Invalid data URL: {error}")))?;
    let limit = crate::ops::fetch_max_body_bytes();
    let mut bytes = Vec::with_capacity(url.as_str().len().min(limit));
    data.decode(|chunk| {
        if chunk.len() > limit.saturating_sub(bytes.len()) { return Err("Data URL exceeds response body byte limit"); }
        bytes.extend_from_slice(chunk); Ok(())
    }).map_err(|error| JsErrorBox::type_error(format!("Invalid data URL: {error}")))?;
    Ok(LocalResource { bytes: bytes.into(), mime: data.mime_type().to_string() })
}

pub(crate) fn is_javascript_mime(mime: &str) -> bool {
    matches!(mime.split(';').next().unwrap_or("").trim().to_ascii_lowercase().as_str(),
        "application/ecmascript" | "application/javascript" | "application/x-ecmascript" |
        "application/x-javascript" | "text/ecmascript" | "text/javascript" |
        "text/javascript1.0" | "text/javascript1.1" | "text/javascript1.2" |
        "text/javascript1.3" | "text/javascript1.4" | "text/javascript1.5" |
        "text/jscript" | "text/livescript" | "text/x-ecmascript" | "text/x-javascript")
}

pub(crate) fn load_local_resource(owner: &ObscuraState, url: &url::Url) -> Result<Option<LocalResource>, JsErrorBox> {
    if !owner.document_active.get() { return Err(JsErrorBox::type_error("Resource document is inactive")); }
    owner.local_resources.load(url, &LocalResourcePrincipal::for_document(owner))
}

#[op2]
#[string]
fn op_object_url_create(scope: &mut v8::PinScope, state: &OpState,
    #[buffer] bytes: &[u8], #[string] mime: String, receiver: v8::Local<v8::Object>) -> Result<String, JsErrorBox> {
    let owner = if let Some((owner, _, _)) = crate::worker::receiver_state(scope, state, receiver) { owner }
        else { crate::window::storage_window(scope, state, receiver)?.2 };
    let mut owner = owner.borrow_mut();
    if !owner.document_active.get() { return Err(JsErrorBox::type_error("Object URL document is inactive")); }
    if crate::window::origin(&owner) == "null" && owner.opaque_origin.is_none() && owner.local_resource_principal.is_none() {
        owner.opaque_origin = Some(Rc::new(()));
    }
    let principal = LocalResourcePrincipal::for_document(&owner);
    let mut uuid = [0u8; 16];
    getrandom::fill(&mut uuid).map_err(|error| JsErrorBox::generic(error.to_string()))?;
    uuid[6] = (uuid[6] & 15) | 0x40; uuid[8] = (uuid[8] & 63) | 0x80;
    let mut hex = String::with_capacity(32);
    for byte in uuid { write!(&mut hex, "{byte:02x}").expect("writing UUID to String"); }
    let url = format!("blob:{}/{}-{}-{}-{}-{}", principal.origin, &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..]);
    owner.local_resources.discard_retired();
    *owner.local_resource_owner.registry.lock().unwrap_or_else(|error| error.into_inner()) = Some(Arc::downgrade(&owner.local_resources.0));
    owner.local_resources.0.lock().unwrap_or_else(|error| error.into_inner()).insert(url.clone(), ObjectUrl {
        resource: LocalResource { bytes: Arc::from(bytes), mime }, principal,
        owner: Arc::downgrade(&owner.local_resource_owner.active),
    });
    Ok(url)
}

#[op2(nofast)]
fn op_object_url_revoke(scope: &mut v8::PinScope, state: &OpState,
    #[string] url: String, receiver: v8::Local<v8::Object>) -> Result<(), JsErrorBox> {
    let owner = if let Some((owner, _, _)) = crate::worker::receiver_state(scope, state, receiver) { owner }
        else { crate::window::storage_window(scope, state, receiver)?.2 };
    let owner = owner.borrow();
    if let Ok(mut url) = url::Url::parse(&url) {
        if load_local_resource(&owner, &url).is_ok() {
            url.set_fragment(None);
            owner.local_resources.0.lock().unwrap_or_else(|error| error.into_inner()).remove(url.as_str());
        }
    }
    Ok(())
}

pub(crate) fn ops() -> Vec<OpDecl> { vec![op_object_url_create(), op_object_url_revoke()] }
