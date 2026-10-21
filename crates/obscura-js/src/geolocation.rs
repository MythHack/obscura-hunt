//! Origin/context geolocation decisions, independent of regional coordinates.
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::future::Future;
use std::rc::Rc;
use std::sync::{Arc, RwLock};
use deno_core::{op2, v8, CancelFuture, OpState};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use crate::ops::{ObscuraState, RealmStates};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct Position { pub latitude: f64, pub longitude: f64, pub accuracy: f64 }
impl Position {
    pub fn new(latitude: f64, longitude: f64, accuracy: f64) -> Result<Self, String> {
        let position = Self { latitude, longitude, accuracy }; position.validate()?; Ok(position)
    }
    pub fn validate(self) -> Result<(), String> {
        if !self.latitude.is_finite() || !self.longitude.is_finite() || !self.accuracy.is_finite()
            || !(-90.0..=90.0).contains(&self.latitude) || !(-180.0..=180.0).contains(&self.longitude) || self.accuracy < 0.0 {
            return Err("Geolocation requires finite latitude (-90..90), longitude (-180..180), and nonnegative accuracy in meters".into());
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Decision { #[default] Prompt, Granted, Denied }
#[derive(Default)]
struct Decisions { origins: BTreeMap<String, Decision>, default: Decision }
pub struct Permissions { decisions: RwLock<Decisions>, changes: watch::Sender<u64> }
impl Default for Permissions {
    fn default() -> Self { Self { decisions: RwLock::new(Decisions::default()), changes: watch::channel(0).0 } }
}
impl Permissions {
    pub fn set(&self, origin: Option<&str>, decision: Decision) -> Result<(), String> {
        let origin = origin.map(canonical_origin).transpose()?;
        let mut decisions = self.decisions.write().map_err(|_| "Geolocation permission lock is poisoned")?;
        if let Some(origin) = origin { decisions.origins.insert(origin, decision); } else { decisions.default = decision; }
        self.changes.send_modify(|revision| *revision = revision.wrapping_add(1));
        Ok(())
    }
    pub fn reset(&self) -> Result<(), String> {
        *self.decisions.write().map_err(|_| "Geolocation permission lock is poisoned")? = Decisions::default();
        self.changes.send_modify(|revision| *revision = revision.wrapping_add(1)); Ok(())
    }
    fn decision(&self, origin: &str) -> Decision {
        self.decisions.read().map(|decisions| decisions.origins.get(origin).copied().unwrap_or(decisions.default)).unwrap_or(Decision::Denied)
    }
    pub(crate) fn policy_changed(&self) {
        self.changes.send_modify(|revision| *revision = revision.wrapping_add(1));
    }
}
pub fn canonical_origin(value: &str) -> Result<String, String> {
    let url = url::Url::parse(value).map_err(|_| "Permission origin must be an absolute HTTP(S) origin")?;
    if !matches!(url.scheme(), "http" | "https") || !url.username().is_empty() || url.password().is_some() { return Err("Permission origin must be an HTTP(S) tuple origin".into()); }
    Ok(url.origin().ascii_serialization())
}
pub struct Geolocation {
    pub permissions: Arc<Permissions>,
    position: watch::Sender<(u64, Option<Position>)>,
}
impl Default for Geolocation {
    fn default() -> Self { Self::new(Arc::new(Permissions::default()), None) }
}
impl Geolocation {
    pub fn new(permissions: Arc<Permissions>, position: Option<Position>) -> Self { Self { permissions, position: watch::channel((0, position)).0 } }
    pub fn set_position(&self, position: Option<Position>) -> Result<(), String> {
        if let Some(position) = position { position.validate()?; }
        self.position.send_modify(|(revision, current)| { *revision = revision.wrapping_add(1); *current = position; }); Ok(())
    }
    fn snapshot(&self, origin: &str, allowed: bool) -> serde_json::Value {
        let decision = if allowed { self.permissions.decision(origin) } else { Decision::Denied };
        let (position_revision, position) = *self.position.borrow();
        serde_json::json!({"state":decision,"position":if decision == Decision::Granted { position } else { None },
            "revision":format!("{}:{position_revision}", *self.permissions.changes.borrow()),
            "timestamp":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64})
    }
}
fn secure(origin: &str) -> bool {
    url::Url::parse(origin).is_ok_and(|url| url.scheme() == "https" || (url.scheme() == "http" && url.host_str().is_some_and(|host|
        host == "localhost" || host.ends_with(".localhost") || host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback()))))
}
/// Parse the geolocation directive only; unrelated device permissions do not
/// acquire a pretend successful implementation through this module.
fn header_allows(header: &str, document_origin: &str, target_origin: &str) -> bool {
    for directive in header.split(',') {
        let Some((name, values)) = directive.trim().split_once('=') else { continue };
        if name.trim() != "geolocation" { continue; }
        if values.trim() == "*" { return true; }
        let Some(values) = values.trim().strip_prefix('(').and_then(|value| value.strip_suffix(')')) else { return false };
        return values.split_ascii_whitespace().any(|token| token == "*" || (token == "self" && document_origin == target_origin)
            || token.strip_prefix('"').and_then(|value| value.strip_suffix('"')).is_some_and(|origin| origin == target_origin));
    }
    true
}
fn iframe_allows(allow: Option<&str>, parent: &str, child: &str, source: &str) -> bool {
    let Some(allow) = allow else { return parent == child; };
    for directive in allow.split(';') {
        let mut tokens = directive.split_ascii_whitespace();
        if tokens.next() != Some("geolocation") { continue; }
        let values = tokens.collect::<Vec<_>>();
        return if values.is_empty() { child == source } else { values.iter().any(|token| *token == "*" || (*token == "'src'" && child == source) || (*token == "'self'" && parent == child) || *token == child) };
    }
    parent == child
}
fn allowed(owner: &Rc<RefCell<ObscuraState>>, registry: &RealmStates) -> bool {
    let origin = crate::window::origin(&owner.borrow());
    if origin == "null" || !secure(&origin) { return false; }
    let mut current = owner.clone();
    loop {
        let document = current.borrow();
        let document_origin = crate::window::origin(&document);
        if !document.document_active.get() || document.sandbox_opaque_origin || !secure(&document_origin)
            || !header_allows(&document.geolocation_policy, &document_origin, &origin) { return false; }
        let id = document.frame_id;
        drop(document);
        let Some((parent_id, node)) = registry.live_frame_owner(id) else { return registry.geolocation_is_top_level(id); };
        let Some(parent) = registry.by_frame_id(parent_id) else { return false; };
        {
            let parent = parent.borrow();
            let parent_origin = crate::window::origin(&parent);
            let (allow, source) = parent.dom.as_ref().and_then(|dom| dom.with_node(node, |node| {
                let source = if node.get_attribute("srcdoc").is_some() { parent_origin.clone() }
                    else { node.get_attribute("src").and_then(|source| url::Url::parse(&parent.url).ok()?.join(source).ok())
                        .map(|url| if url.scheme() == "about" { parent_origin.clone() } else { url.origin().ascii_serialization() }).unwrap_or_else(|| parent_origin.clone()) };
                (node.get_attribute("allow").map(str::to_owned), source)
            })).unwrap_or((None, parent_origin.clone()));
            if !iframe_allows(allow.as_deref(), &parent_origin, &document_origin, &source) { return false; }
        }
        current = parent;
    }
}
fn capture(scope: &mut v8::PinScope, state: &OpState, receiver: v8::Local<v8::Object>)
    -> Result<(Rc<RefCell<ObscuraState>>, Rc<RefCell<RealmStates>>, Rc<deno_core::CancelHandle>), deno_error::JsErrorBox>
{
    let context = receiver.get_creation_context(scope).ok_or_else(|| deno_error::JsErrorBox::type_error("Retired geolocation document"))?;
    if !receiver.strict_equals(context.global(scope).into()) { return Err(deno_error::JsErrorBox::type_error("Invalid geolocation receiver")); }
    let context = v8::Global::new(scope, context);
    let (_, _, owner) = crate::window::context_document(state, &context)?;
    let registry = state.borrow::<Rc<RefCell<RealmStates>>>().clone();
    let cancel = registry.borrow().context_cancel(&context).ok_or_else(|| deno_error::JsErrorBox::type_error("Retired geolocation realm"))?;
    Ok((owner, registry, cancel))
}
#[op2]
#[string]
fn op_geolocation(scope: &mut v8::PinScope, state: &OpState, receiver: v8::Local<v8::Object>) -> Result<String, deno_error::JsErrorBox> {
    let (owner, registry, _) = capture(scope, state, receiver)?;
    let permitted = allowed(&owner, &registry.borrow());
    let owner = owner.borrow();
    Ok(owner.geolocation.snapshot(&crate::window::origin(&owner), permitted).to_string())
}
#[op2]
#[string]
fn op_geolocation_wait(scope: &mut v8::PinScope, state: &OpState, #[string] revision: String, receiver: v8::Local<v8::Object>)
    -> impl Future<Output = Result<String, deno_error::JsErrorBox>> + use<>
{
    let captured = capture(scope, state, receiver);
    async move {
        let (owner, registry, cancel) = captured?;
        let geo = owner.borrow().geolocation.clone();
        let mut permissions = geo.permissions.changes.subscribe();
        let mut position = geo.position.subscribe();
        loop {
            let snapshot = geo.snapshot(&crate::window::origin(&owner.borrow()), allowed(&owner, &registry.borrow()));
            if snapshot["revision"].as_str() != Some(&revision) { return Ok(snapshot.to_string()); }
            let change = async { tokio::select! { _ = permissions.changed() => {}, _ = position.changed() => {} } };
            if change.or_cancel(cancel.clone()).await.is_err() { return Ok("null".into()); }
        }
    }
}
pub(crate) fn ops() -> Vec<deno_core::OpDecl> { vec![op_geolocation(), op_geolocation_wait()] }
