use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::future::Future;
use std::rc::{Rc, Weak};
use std::sync::{Arc, atomic::{AtomicU32, AtomicUsize, Ordering}};
use std::time::Duration;

use deno_core::{op2, v8, OpDecl, OpState, ToJsBuffer};
use deno_error::JsErrorBox;
use obscura_net::websocket::{CloseCode, CloseFrame, Message, WebSocketConnection, WebSocketError, WebSocketRequest};
use serde::Serialize;
use tokio::sync::{mpsc, Mutex};
use url::Url;

use crate::ops::SharedState;

const MAX_BUFFERED: usize = 64 * 1024 * 1024;
const CLOSE_TIMEOUT: Duration = Duration::from_secs(5);
static NEXT_SOCKET: AtomicU32 = AtomicU32::new(1);

#[derive(Default)]
pub(crate) struct SocketRegistry(RefCell<HashMap<u32, Rc<SocketResource>>>);

impl SocketRegistry {
    pub(crate) fn cancel_all(&self) {
        for (_, socket) in self.0.borrow_mut().drain() { socket.cancel(); }
    }
}

impl Drop for SocketRegistry {
    fn drop(&mut self) { self.cancel_all(); }
}

struct SocketResource {
    owner: Weak<RefCell<crate::ops::ObscuraState>>,
    generation: u64,
    commands: mpsc::Sender<Command>,
    events: Mutex<mpsc::Receiver<(SocketEvent, usize)>>,
    buffered: Arc<AtomicUsize>,
    received: Arc<AtomicUsize>,
    task: tokio::task::AbortHandle,
    cancelled: Cell<bool>,
}

impl SocketResource {
    fn cancel(&self) {
        self.cancelled.set(true);
        self.task.abort();
    }
}


impl Drop for SocketResource {
    fn drop(&mut self) { self.task.abort(); }
}

enum Command { Send(Message, usize), Close(Option<CloseFrame>) }

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum SocketEvent {
    Open { protocol: String },
    Text { text: String },
    Binary { data: ToJsBuffer },
    Closing,
    Error,
    Close { code: u16, reason: String, clean: bool },
    Cancelled,
    Aborted,
}

fn socket_for(owner: &SharedState, rid: u32) -> Result<Rc<SocketResource>, JsErrorBox> {
    let socket = owner.borrow().websockets.0.borrow().get(&rid).cloned()
        .ok_or_else(|| JsErrorBox::type_error("WebSocket is closed"))?;
    if !socket.owner.upgrade().is_some_and(|document| Rc::ptr_eq(&document, owner))
        || owner.borrow().document_generation != socket.generation
        || !owner.borrow().document_active.get() {
        return Err(JsErrorBox::new("DOMExceptionSecurityError", "WebSocket document is retired or foreign"));
    }
    Ok(socket)
}

fn document(scope: &mut v8::PinScope, state: &OpState, receiver: v8::Local<v8::Object>)
    -> Result<SharedState, JsErrorBox>
{
    let context = receiver.get_creation_context(scope)
        .ok_or_else(|| JsErrorBox::type_error("WebSocket execution world is retired"))?;
    if !receiver.strict_equals(context.global(scope).into()) {
        return Err(JsErrorBox::type_error("Invalid WebSocket execution world"));
    }
    let context = v8::Global::new(scope, context);
    let (_, _, owner) = crate::window::context_document(state, &context)?;
    if !owner.borrow().document_active.get() {
        return Err(JsErrorBox::type_error("WebSocket document is retired"));
    }
    Ok(owner)
}

#[op2]
#[serde]
fn op_websocket_create(scope: &mut v8::PinScope, state: &mut OpState,
    #[string] input: &str, #[serde] protocols: Vec<String>, receiver: v8::Local<v8::Object>)
    -> Result<(u32, String), JsErrorBox>
{
    let owner = document(scope, state, receiver)?;
    let mut document = owner.borrow_mut();
    let base = crate::ops::document_base_url(&document).and_then(|value| Url::parse(&value).ok());
    let mut url = base.as_ref().map_or_else(|| Url::parse(input), |base| base.join(input))
        .map_err(|_| JsErrorBox::new("DOMExceptionSyntaxError", "Invalid WebSocket URL"))?;
    if matches!(url.scheme(), "http" | "https") {
        let scheme = if url.scheme() == "http" { "ws" } else { "wss" };
        url.set_scheme(scheme).map_err(|_| JsErrorBox::new("DOMExceptionSyntaxError", "Invalid WebSocket URL"))?;
    }
    if !matches!(url.scheme(), "ws" | "wss") || url.host().is_none() || url.fragment().is_some() {
        return Err(JsErrorBox::new("DOMExceptionSyntaxError", "Invalid WebSocket URL"));
    }
    let mut seen = std::collections::HashSet::with_capacity(protocols.len());
    for protocol in &protocols {
        if protocol.is_empty() || !protocol.bytes().all(|byte| byte.is_ascii_alphanumeric()
            || matches!(byte, b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'*' | b'+' | b'-' | b'.' | b'^' | b'_' | b'`' | b'|' | b'~'))
            || !seen.insert(protocol) {
            return Err(JsErrorBox::new("DOMExceptionSyntaxError", "Invalid or duplicate WebSocket protocol"));
        }
    }
    let origin = crate::window::origin(&document);
    let mixed = Url::parse(&origin).ok().is_some_and(|origin| origin.scheme() == "https") && url.scheme() == "ws";
    if mixed {
        return Err(JsErrorBox::new("DOMExceptionSecurityError", "Insecure WebSocket from a secure document"));
    }
    capture_meta_policy(&mut document);
    let permitted = !document.blocked_urls.iter().any(|pattern| pattern == "*"
        || url.as_str().contains(pattern) || crate::ops::glob_match(pattern, url.as_str()))
        && document.content_security_policy.iter().all(|policy| crate::embedding_policy::allows_source(
            policy, &origin, &url, &["connect-src", "default-src"]));
    // Standalone runtimes also retain one native context client, never a direct
    // or verification-disabled fallback. Pages install their selected client.
    #[cfg(feature = "stealth")]
    let stealth = document.stealth_client.clone();
    let needs_default = document.http_client.is_none();
    #[cfg(feature = "stealth")]
    let needs_default = needs_default && stealth.is_none();
    if needs_default {
        let cookies = document.cookie_jar.get_or_insert_with(|| Arc::new(obscura_net::CookieJar::new())).clone();
        document.http_client = Some(Arc::new(obscura_net::ObscuraHttpClient::with_cookie_jar(cookies)));
    }
    let client = document.http_client.clone();
    let generation = document.document_generation;
    let normalized_url = url.to_string();
    let request = WebSocketRequest { url, origin, protocols };
    let (commands, commands_rx) = mpsc::channel(1024);
    // Bound application backlog without suspending native ping/close processing.
    let (events_tx, events) = mpsc::channel(128);
    let received = Arc::new(AtomicUsize::new(0));
    let incoming = received.clone();
    let buffered = Arc::new(AtomicUsize::new(0));
    let queued = buffered.clone();
    let rid = NEXT_SOCKET.try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
        .map_err(|_| JsErrorBox::generic("WebSocket identity space exhausted"))?;
    let _tokio = crate::runtime::enter_tokio_context();
    let task = tokio::spawn(async move {
        let connection = async {
            if !permitted { return Err(obscura_net::ObscuraNetError::Network("WebSocket blocked by document policy".into())); }
            #[cfg(feature = "stealth")]
            if let Some(stealth) = stealth { return stealth.websocket(&request).await; }
            client.ok_or_else(|| obscura_net::ObscuraNetError::Network("Document transport is unavailable".into()))?
                .websocket(&request).await
        };
        let outcome = tokio::time::timeout(Duration::from_secs(30), connection).await;
        match outcome {
            Ok(Ok(connection)) => run_socket(connection, commands_rx, &events_tx, queued, incoming).await,
            _ => failure(&events_tx).await,
        }
    });
    let resource = Rc::new(SocketResource { owner: Rc::downgrade(&owner), generation, commands,
        events: Mutex::new(events), buffered, received, task: task.abort_handle(), cancelled: Cell::new(false) });
    document.websockets.0.borrow_mut().insert(rid, resource);
    Ok((rid, normalized_url))
}

pub(crate) fn capture_meta_policy(document: &mut crate::ops::ObscuraState) {
    let Some(dom) = &document.dom else { return; };
    let Some(head) = dom.descendants_iter(dom.document()).find(|node| dom.with_node(*node, |node|
        node.as_element().is_some_and(|name| &*name.local == "head"
            && &*name.ns == "http://www.w3.org/1999/xhtml")).unwrap_or(false)) else { return; };
    for node in dom.descendants_iter(head) {
        if let Some(policy) = dom.with_node(node, |node| {
            if node.as_element().is_none_or(|name| &*name.local != "meta"
                || &*name.ns != "http://www.w3.org/1999/xhtml") || !node.get_attribute("http-equiv")
                .is_some_and(|value| value.eq_ignore_ascii_case("content-security-policy")) { return None; }
            node.get_attribute("content").map(str::to_owned)
        }).flatten() {
            if !document.content_security_policy.contains(&policy) { document.content_security_policy.push(policy); }
        }
    }
}

#[op2]
#[serde]
fn op_websocket_next(scope: &mut v8::PinScope, state: Rc<RefCell<OpState>>, rid: u32,
    receiver: v8::Local<v8::Object>) -> impl Future<Output = Result<SocketEvent, JsErrorBox>> + use<>
{
    let captured = document(scope, &state.borrow(), receiver)
        .and_then(|owner| socket_for(&owner, rid).map(|socket| (owner, socket)));
    async move {
        let Ok((owner, socket)) = captured else { return Ok(SocketEvent::Cancelled); };
        let (event, bytes) = socket.events.lock().await.recv().await.unwrap_or((SocketEvent::Aborted, 0));
        socket.received.fetch_sub(bytes, Ordering::AcqRel);
        if socket.cancelled.get() || owner.borrow().document_generation != socket.generation
            || !owner.borrow().document_active.get() { return Ok(SocketEvent::Cancelled); }
        Ok(event)
    }
}

#[op2(nofast)]
fn op_websocket_send(scope: &mut v8::PinScope, state: &OpState, rid: u32, text: bool,
    #[buffer] bytes: &[u8], receiver: v8::Local<v8::Object>) -> Result<bool, JsErrorBox>
{
    let owner = document(scope, state, receiver)?;
    let socket = socket_for(&owner, rid)?;
    let length = bytes.len();
    if socket.buffered.try_update(Ordering::AcqRel, Ordering::Acquire,
        |queued| queued.checked_add(length).filter(|total| *total <= MAX_BUFFERED)).is_err() {
        socket.task.abort();
        return Ok(false);
    }
    let message = if text {
        Message::Text(std::str::from_utf8(bytes).map_err(|_| JsErrorBox::type_error("Invalid WebSocket text"))?.into())
    } else { Message::Binary(bytes.to_vec().into()) };
    if socket.commands.try_send(Command::Send(message, length)).is_err() {
        socket.buffered.fetch_sub(length, Ordering::AcqRel);
        socket.task.abort();
        return Ok(false);
    }
    Ok(true)
}

#[op2(nofast)]
fn op_websocket_buffered(scope: &mut v8::PinScope, state: &OpState, rid: u32,
    receiver: v8::Local<v8::Object>) -> Result<u32, JsErrorBox>
{
    let owner = document(scope, state, receiver)?;
    Ok(socket_for(&owner, rid)?.buffered.load(Ordering::Acquire) as u32)
}

#[op2(nofast)]
fn op_websocket_close(scope: &mut v8::PinScope, state: &OpState, rid: u32, code: u32,
    #[string] reason: String, connecting: bool, receiver: v8::Local<v8::Object>) -> Result<(), JsErrorBox>
{
    if code != 0 && code != 1000 && !(3000..=4999).contains(&code) {
        return Err(JsErrorBox::new("DOMExceptionInvalidAccessError", "Invalid WebSocket close code"));
    }
    if reason.len() > 123 { return Err(JsErrorBox::new("DOMExceptionSyntaxError", "WebSocket close reason exceeds 123 bytes")); }
    let owner = document(scope, state, receiver)?;
    let socket = socket_for(&owner, rid)?;
    if connecting {
        // Abort an in-flight DNS/TLS/Upgrade immediately. No close handshake exists.
        socket.task.abort();
    } else {
        let frame = (code != 0 || !reason.is_empty()).then(|| CloseFrame {
            code: CloseCode::from(if code == 0 { 1000 } else { code as u16 }), reason: reason.into(),
        });
        if socket.commands.try_send(Command::Close(frame)).is_err() { socket.task.abort(); }
    }
    Ok(())
}

#[op2(nofast)]
fn op_websocket_release(scope: &mut v8::PinScope, state: &mut OpState, rid: u32,
    receiver: v8::Local<v8::Object>) -> Result<(), JsErrorBox>
{
    let owner = document(scope, state, receiver)?;
    let socket = socket_for(&owner, rid)?;
    owner.borrow().websockets.0.borrow_mut().remove(&rid);
    socket.cancel();
    Ok(())
}

async fn failure(events: &mpsc::Sender<(SocketEvent, usize)>) {
    let _ = events.send((SocketEvent::Error, 0)).await;
    let _ = events.send((SocketEvent::Close { code: 1006, reason: String::new(), clean: false }, 0)).await;
}

fn queue_message(events: &mpsc::Sender<(SocketEvent, usize)>, incoming: &AtomicUsize,
    event: SocketEvent, bytes: usize) -> bool
{
    if incoming.try_update(Ordering::AcqRel, Ordering::Acquire,
        |queued| queued.checked_add(bytes).filter(|total| *total <= MAX_BUFFERED)).is_err() { return false; }
    if events.try_send((event, bytes)).is_err() {
        incoming.fetch_sub(bytes, Ordering::AcqRel);
        return false;
    }
    true
}

async fn run_socket(mut socket: WebSocketConnection, mut commands: mpsc::Receiver<Command>,
    events: &mpsc::Sender<(SocketEvent, usize)>, buffered: Arc<AtomicUsize>, incoming: Arc<AtomicUsize>)
{
    if events.send((SocketEvent::Open { protocol: std::mem::take(&mut socket.protocol) }, 0)).await.is_err() { return; }
    let mut closing = false;
    let mut close_received = None;
    let mut close_deadline = None;
    loop {
        tokio::select! {
            _ = async { if let Some(deadline) = close_deadline { tokio::time::sleep_until(deadline).await; } },
                if close_deadline.is_some() => { failure(events).await; return; }
            command = commands.recv() => {
                let Some(command) = command else { return; };
                let result = match command {
                    Command::Send(message, length) if !closing => {
                        let result = tokio::time::timeout(Duration::from_secs(30), socket.send(message)).await;
                        if matches!(result, Ok(Ok(()))) { buffered.fetch_sub(length, Ordering::AcqRel); }
                        result
                    }
                    Command::Send(_, _) => continue,
                    Command::Close(frame) if !closing => {
                        closing = true;
                        close_deadline = Some(tokio::time::Instant::now() + CLOSE_TIMEOUT);
                        tokio::time::timeout(CLOSE_TIMEOUT, socket.send(Message::Close(frame))).await
                    }
                    Command::Close(_) => continue,
                };
                if !matches!(result, Ok(Ok(()))) { failure(events).await; return; }
            }
            message = socket.receive() => {
                match message {
                    Some(Ok(Message::Text(text))) if !closing => {
                        let length = text.len();
                        if !queue_message(events, &incoming, SocketEvent::Text { text: text.to_string() }, length) {
                            let _ = tokio::time::timeout(CLOSE_TIMEOUT, socket.send(Message::Close(Some(CloseFrame { code: CloseCode::Size, reason: "".into() })))).await;
                            failure(events).await; return;
                        }
                    }
                    Some(Ok(Message::Binary(data))) if !closing => {
                        let length = data.len();
                        if !queue_message(events, &incoming, SocketEvent::Binary { data: Vec::<u8>::from(data).into() }, length) {
                            let _ = tokio::time::timeout(CLOSE_TIMEOUT, socket.send(Message::Close(Some(CloseFrame { code: CloseCode::Size, reason: "".into() })))).await;
                            failure(events).await; return;
                        }
                    }
                    Some(Ok(Message::Ping(_))) => {
                        if !matches!(tokio::time::timeout(CLOSE_TIMEOUT, socket.flush()).await, Ok(Ok(()))) { failure(events).await; return; }
                    }
                    Some(Ok(Message::Close(frame))) => {
                        closing = true;
                        close_deadline = Some(tokio::time::Instant::now() + CLOSE_TIMEOUT);
                        close_received = Some(frame.map(|frame| (frame.code.into(), frame.reason.to_string()))
                            .unwrap_or_else(|| (1005, String::new())));
                        if events.send((SocketEvent::Closing, 0)).await.is_err() { return; }
                        if !matches!(tokio::time::timeout(CLOSE_TIMEOUT, socket.flush()).await, Ok(Ok(()))) { failure(events).await; return; }
                    }
                    None | Some(Err(WebSocketError::ConnectionClosed)) => {
                        if let Some((code, reason)) = close_received {
                            let _ = events.send((SocketEvent::Close { code, reason, clean: true }, 0)).await;
                        } else { failure(events).await; }
                        return;
                    }
                    Some(Err(error)) => {
                        let code = if matches!(error, WebSocketError::Utf8(_)) { CloseCode::Invalid } else { CloseCode::Protocol };
                        let _ = tokio::time::timeout(CLOSE_TIMEOUT, socket.send(Message::Close(Some(CloseFrame { code, reason: "".into() })))).await;
                        failure(events).await;
                        return;
                    }
                    _ => {},
                }
            }
        }
    }
}


pub(crate) fn ops() -> Vec<OpDecl> {
    vec![op_websocket_create(), op_websocket_next(), op_websocket_send(), op_websocket_buffered(),
        op_websocket_close(), op_websocket_release()]
}
