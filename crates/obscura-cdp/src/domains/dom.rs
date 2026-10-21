use std::collections::{HashMap, HashSet};

use obscura_browser::Page;
use obscura_dom::{DomTree, NodeData, NodeId};
use obscura_js::runtime::CdpExecutionWorld;
use serde_json::{json, Value};

use crate::dispatch::{CdpContext, NodeRecord};
use crate::types::CdpEvent;

pub(crate) struct DomDocument {
    pub page_id: String,
    document_generation: u64,
    children_sent: HashMap<u64, bool>,
    published_children: HashMap<u64, Vec<u64>>,
    retired_nodes: HashSet<u64>,
    topology_generation: u64,
}

impl DomDocument {
    fn remember(&mut self, root: &Value) {
        let mut pending = vec![root];
        while let Some(node) = pending.pop() {
            if let Some(id) = node["nodeId"].as_u64() {
                self.retired_nodes.remove(&id);
                self.children_sent.insert(id,
                    node["children"].is_array() || node["childNodeCount"] == 0);
                if let Some(children) = node["children"].as_array() {
                    self.published_children.insert(id, children.iter().filter_map(|child| child["nodeId"].as_u64()).collect());
                } else if node["childNodeCount"] == 0 {
                    self.published_children.insert(id, Vec::new());
                }
            }
            if let Some(children) = node["children"].as_array() {
                pending.extend(children);
            }
        }
    }
}

/// Compare only the client-published light tree after a native topology change.
/// No page-visible object map participates in node identity or mutation routing.
pub(crate) fn drain_mutations(ctx: &mut CdpContext) {
    let sessions = ctx.dom_documents.keys().cloned().collect::<Vec<_>>();
    for session in sessions {
        let Some(mut state) = ctx.dom_documents.remove(&session) else { continue };
        let current = ctx.get_session_page(&session).and_then(|page| page.js.as_ref())
            .and_then(|js| Some((js.cdp_document_generation(0).ok()?,
                js.with_dom_in_frame(0, DomTree::topology_generation).ok()?)));
        let Some((generation, topology)) = current else { continue };
        if generation != state.document_generation { continue; }
        if topology != state.topology_generation {
            let mut parents = state.published_children.keys().copied().collect::<Vec<_>>();
            parents.sort_unstable();
            for parent in parents {
                if !state.published_children.contains_key(&parent) { continue; }
                let Some(record) = ctx.backend_nodes.get(&parent).cloned() else { continue };
                let children = ctx.get_session_page(&session).and_then(|page| page.js.as_ref())
                    .and_then(|js| js.with_dom_in_frame(0, |dom| dom.children(NodeId::new(record.node_id))).ok())
                    .unwrap_or_default();
                let mut actual = Vec::with_capacity(children.len());
                for child in children {
                    if let Ok(id) = backend_node_id(ctx, &session, 0, child.index() as u32) { actual.push(id); }
                }
                let old = state.published_children.get(&parent).cloned().unwrap_or_default();
                if old == actual { continue; }
                let positions = old.iter().enumerate().map(|(index, id)| (*id, index)).collect::<HashMap<_, _>>();
                let mut kept = std::collections::HashSet::new();
                let mut next = 0;
                for id in &actual {
                    if let Some(&index) = positions.get(id) {
                        if index >= next { kept.insert(*id); next = index + 1; }
                    }
                }
                for id in old.iter().filter(|id| !kept.contains(id)) {
                    ctx.pending_events.push(CdpEvent { method: "DOM.childNodeRemoved".into(),
                        params: json!({"parentNodeId": parent, "nodeId": id}), session_id: session.clone() });
                    let mut removed = vec![*id];
                    while let Some(id) = removed.pop() {
                        state.retired_nodes.insert(id);
                        state.children_sent.remove(&id);
                        if let Some(children) = state.published_children.remove(&id) { removed.extend(children); }
                    }
                }
                let mut previous = 0;
                for id in &actual {
                    if !kept.contains(id) {
                        let Some(record) = ctx.backend_nodes.get(id).cloned() else { continue };
                        let node = ctx.get_session_page(&session).and_then(|page| page.js.as_ref())
                            .and_then(|js| js.with_dom_in_frame(0, |dom| serialize_node(dom, NodeId::new(record.node_id), 0, 0)).ok());
                        if let Some(mut node) = node {
                            if scope_node_tree(ctx, &session, 0, &mut node).is_ok() {
                                state.remember(&node);
                                ctx.pending_events.push(CdpEvent { method: "DOM.childNodeInserted".into(),
                                    params: json!({"parentNodeId": parent, "previousNodeId": previous, "node": node}),
                                    session_id: session.clone() });
                            }
                        }
                    }
                    previous = *id;
                }
                state.published_children.insert(parent, actual);
            }
            state.topology_generation = topology;
        }
        ctx.dom_documents.insert(session, state);
    }
}

fn node_id_param(params: &Value) -> Result<u64, String> {
    params["nodeId"].as_u64().ok_or_else(|| "valid nodeId required".into())
}

fn push_children(ctx: &mut CdpContext, state: &mut DomDocument, node: u64, depth: u32,
    session_id: &Option<String>, events: &mut Vec<CdpEvent>) -> Result<(), String> {
    let mut pending = vec![(node, depth.min(MAX_SERIALIZE_DEPTH))];
    while let Some((parent, depth)) = pending.pop() {
        let Some(sent) = state.children_sent.get(&parent).copied() else { continue };
        let record = ctx.backend_nodes.get(&parent).ok_or("Cannot find node in the current document")?;
        let (frame_id, native) = (record.frame_id, NodeId::new(record.node_id));
        let page = ctx.get_session_page(session_id).ok_or("No page")?;
        let js = page.js.as_ref().ok_or("No JavaScript runtime")?;
        if !sent {
            let mut nodes = js.with_dom_in_frame(frame_id, |dom| dom.children(native).into_iter()
                .map(|child| serialize_node(dom, child, depth - 1, 0)).collect::<Vec<_>>())?;
            for node in &mut nodes {
                scope_node_tree(ctx, session_id, frame_id, node)?;
                state.remember(node);
            }
            state.children_sent.insert(parent, true);
            state.published_children.insert(parent, nodes.iter().filter_map(|node| node["nodeId"].as_u64()).collect());
            events.push(CdpEvent {
                method: "DOM.setChildNodes".into(),
                params: json!({"parentId":parent, "nodes":nodes}),
                session_id: session_id.clone(),
            });
        } else if depth > 1 {
            let children = js.with_dom_in_frame(frame_id, |dom| dom.children(native))?;
            if children.is_empty() { continue; }
            let mut child_record = record.clone();
            for child in children.into_iter().rev() {
                child_record.node_id = child.index() as u32;
                if let Some(&id) = ctx.backend_node_ids.get(&child_record) {
                    pending.push((id, depth - 1));
                }
            }
        }
    }
    Ok(())
}

fn push_node(ctx: &mut CdpContext, session_id: &Option<String>, world: CdpExecutionWorld,
    node: NodeId) -> Result<u64, String> {
    drain_mutations(ctx);
    if world.frame_id != 0 {
        return Err("Node is outside the requested light-DOM document".into());
    }
    let page = ctx.get_session_page(session_id).ok_or("No page")?;
    let js = page.js.as_ref().ok_or("No JavaScript runtime")?;
    let generation = js.cdp_document_generation(world.frame_id)?;
    let path = js.with_dom_in_frame(world.frame_id, |dom| {
        if dom.get_node(node).is_none() { return Err("No node with given id found"); }
        let path = dom.ancestors(node);
        if node != dom.document() && path.last().copied() != Some(dom.document()) {
            return Err("Node is outside the requested light-DOM document");
        }
        Ok(path)
    })?.map_err(str::to_string)?;
    let Some(state) = ctx.dom_documents.get(session_id) else { return Ok(0) };
    if state.page_id != page.id || state.document_generation != generation {
        ctx.dom_documents.remove(session_id);
        return Ok(0);
    }
    let id = backend_node_id(ctx, session_id, world.frame_id, node.index() as u32)?;
    let mut state = ctx.dom_documents.remove(session_id).expect("document was just checked");
    let mut events = Vec::new();
    let result = (|| {
        if !state.children_sent.contains_key(&id) {
            for parent in path.into_iter().rev() {
                let parent_id = backend_node_id(ctx, session_id, world.frame_id, parent.index() as u32)?;
                push_children(ctx, &mut state, parent_id, 1, session_id, &mut events)?;
            }
            if !state.children_sent.contains_key(&id) {
                return Err("Node is outside the published light-DOM document".into());
            }
        }
        Ok(id)
    })();
    ctx.dom_documents.insert(session_id.clone(), state);
    ctx.pending_events.extend(events);
    result
}

fn node_for_location(page: &mut Page, x: i64, y: i64) -> Result<NodeId, String> {
    if x < 0 || y < 0 || x as f64 >= f64::from(page.viewport.0) || y as f64 >= f64::from(page.viewport.1) {
        return Err("No node found at given location".into());
    }
    #[cfg(feature = "render")]
    {
        page.js.as_mut().ok_or("No JavaScript runtime")?.native_hit_test(0, x as f64, y as f64)?
            .ok_or_else(|| "No node found at given location".into())
    }
    #[cfg(not(feature = "render"))]
    {
        Err("DOM hit testing requires rendering support".into())
    }
}

fn resolve_node_id(ctx: &mut CdpContext, session: &Option<String>, params: &Value)
    -> Result<(CdpExecutionWorld, u32), String>
{
    drain_mutations(ctx);
    if let Some(id) = params.get("nodeId").and_then(Value::as_u64) {
        if ctx.dom_documents.get(session).is_some_and(|document| document.retired_nodes.contains(&id)) {
            return Err("Cannot find node in the current document".to_string());
        }
    }
    if let Some(id) = params.get("nodeId").or_else(|| params.get("backendNodeId")).and_then(Value::as_u64) {
        let record = ctx.backend_nodes.get(&id).cloned().ok_or("Cannot find node in the current document")?;
        let page = ctx.get_session_page_mut(session).ok_or("No page")?;
        let js = page.js.as_mut().ok_or("No JavaScript runtime")?;
        if record.page_id != page.id || js.cdp_document_generation(record.frame_id).ok() != Some(record.document_generation) {
            return Err("Cannot find node in the current document".to_string());
        }
        if !js.with_dom_in_frame(record.frame_id, |dom| dom.get_node(NodeId::new(record.node_id)).is_some())? {
            return Err("Cannot find node in the current document".to_string());
        }
        return Ok((CdpExecutionWorld { frame_id: record.frame_id, world_id: None }, record.node_id));
    }
    if let Some(id) = params.get("objectId").and_then(Value::as_str) {
        let page = ctx.get_session_page_mut(session).ok_or("No page")?;
        return page.js.as_mut().ok_or("No JavaScript runtime")?.object_node_id(id);
    }
    Err("nodeId, backendNodeId, or objectId required".to_string())
}

pub(crate) fn backend_node_id(ctx: &mut CdpContext, session: &Option<String>, frame_id: u32, node_id: u32) -> Result<u64, String> {
    let page = ctx.get_session_page(session).ok_or("No page")?;
    let record = NodeRecord {
        page_id: page.id.clone(), frame_id, node_id,
        document_generation: page.js.as_ref().ok_or("No JavaScript runtime")?.cdp_document_generation(frame_id)?,
    };
    if let Some(id) = ctx.backend_node_ids.get(&record) { return Ok(*id); }
    let id = ctx.next_backend_node_id;
    ctx.next_backend_node_id = id.checked_add(1).ok_or("Backend node id space exhausted")?;
    ctx.backend_nodes.insert(id, record.clone());
    ctx.backend_node_ids.insert(record, id);
    Ok(id)
}

fn scope_node_tree(ctx: &mut CdpContext, session: &Option<String>, frame_id: u32, root: &mut Value) -> Result<(), String> {
    let mut pending = vec![root];
    while let Some(node) = pending.pop() {
        if let Some(native) = node["nodeId"].as_u64() {
            let id = backend_node_id(ctx, session, frame_id, u32::try_from(native).map_err(|_| "Invalid native node id")?)?;
            node["nodeId"] = json!(id);
            node["backendNodeId"] = json!(id);
        }
        if let Some(children) = node.get_mut("children").and_then(Value::as_array_mut) { pending.extend(children.iter_mut()); }
    }
    Ok(())
}

/// Standard base64 (with padding). Used to ferry file bytes to the JS layer for
/// DOM.setFileInputFiles without pulling in a dependency.
fn encode_base64(input: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((input.len() + 2) / 3 * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(T[((n >> 18) & 63) as usize] as char);
        out.push(T[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 { T[((n >> 6) & 63) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[(n & 63) as usize] as char } else { '=' });
    }
    out
}

/// Best-effort MIME type from a file extension, for the File objects created by
/// DOM.setFileInputFiles. Defaults to application/octet-stream.
fn guess_mime(path: &str) -> &'static str {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "pdf" => "application/pdf",
        "txt" => "text/plain",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" | "mjs" => "text/javascript",
        "json" => "application/json",
        "xml" => "application/xml",
        "csv" => "text/csv",
        "zip" => "application/zip",
        "gz" => "application/gzip",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        _ => "application/octet-stream",
    }
}

pub async fn handle(
    method: &str,
    params: &Value,
    ctx: &mut CdpContext,
    session_id: &Option<String>,
) -> Result<Value, String> {
    match method {
        "getFrameOwner" => {
            let page = ctx.get_session_page(session_id).ok_or("No page")?;
            let id = params.get("frameId").and_then(Value::as_str).ok_or("frameId required")?;
            let frame_id = super::page::native_frame_id(page, id)?;
            let (parent, owner) = page.live_frame_owner(frame_id).ok_or("No live owner for frame")?;
            let native_node = owner.index() as u32;
            let id = backend_node_id(ctx, session_id, parent, native_node)?;
            Ok(json!({"nodeId": id, "backendNodeId": id}))
        }
        "enable" => Ok(json!({})),
        "disable" => {
            ctx.dom_documents.remove(session_id);
            Ok(json!({}))
        }
        "getDocument" => {
            // Fresh about:blank targets retain a native DOM until first use.
            // Activate that document through the normal Page lifecycle before
            // requesting its generation and native node identities.
            let page = ctx.get_session_page_mut(session_id).ok_or("No page")?;
            let depth = params.get("depth").and_then(|v| v.as_i64()).unwrap_or(2);
            let mut root = page.with_dom(|dom| serialize_node(dom, dom.document(), depth as u32, 0))
                .ok_or("No DOM loaded")?;
            let mut state = DomDocument {
                page_id: page.id.clone(), children_sent: HashMap::new(),
                published_children: HashMap::new(),
                retired_nodes: HashSet::new(),
                topology_generation: page.with_dom(DomTree::topology_generation).ok_or("No DOM loaded")?,
                document_generation: page.js.as_ref().ok_or("No JavaScript runtime")?.cdp_document_generation(0)?,
            };
            scope_node_tree(ctx, session_id, 0, &mut root)?;
            state.remember(&root);
            ctx.dom_documents.insert(session_id.clone(), state);
            Ok(json!({"root":root}))
        }
        "getAttributes" => {
            node_id_param(params)?;
            let (world, node_id) = resolve_node_id(ctx, session_id, params)?;
            let page = ctx.get_session_page(session_id).ok_or("No page")?;
            let attributes = page.js.as_ref().ok_or("No JavaScript runtime")?.with_dom_in_frame(world.frame_id, |dom| {
                let node = dom.get_node(NodeId::new(node_id)).ok_or("No node with given id found")?;
                let attrs = node.attrs().ok_or("Node is not an Element")?;
                Ok::<_, &str>(attrs.iter().flat_map(|attr| [attr.qualified_name(), attr.value.clone()]).collect::<Vec<_>>())
            })?.map_err(str::to_string)?;
            Ok(json!({"attributes":attributes}))
        }
        "requestChildNodes" => {
            let node = node_id_param(params)?;
            let (world, native) = resolve_node_id(ctx, session_id, params)?;
            let depth = match params.get("depth") {
                None => 1,
                Some(value) => match value.as_i64() {
                    Some(-1) => u32::MAX,
                    Some(value) if value > 0 => u32::try_from(value).unwrap_or(u32::MAX),
                    _ => return Err("depth must be positive or -1".into()),
                },
            };
            if params["pierce"].as_bool() == Some(true) {
                let page = ctx.get_session_page(session_id).ok_or("No page")?;
                let needs_piercing = page.js.as_ref().ok_or("No JavaScript runtime")?.with_dom_in_frame(world.frame_id, |dom| {
                    let mut pending = vec![(NodeId::new(native), depth.min(MAX_SERIALIZE_DEPTH))];
                    while let Some((id, remaining)) = pending.pop() {
                        if dom.shadow_root(id).is_some() || dom.get_node(id).is_some_and(|node|
                            node.as_element().is_some_and(|name| &*name.ns == "http://www.w3.org/1999/xhtml"
                                && matches!(name.local.as_ref(), "iframe" | "frame"))) {
                            return true;
                        }
                        if remaining > 0 {
                            pending.extend(dom.children(id).into_iter().map(|child| (child, remaining - 1)));
                        }
                    }
                    false
                })?;
                if needs_piercing {
                    return Err("Piercing frame and shadow trees is not supported".into());
                }
            }
            if let Some(mut state) = ctx.dom_documents.remove(session_id) {
                let mut events = Vec::new();
                let result = push_children(ctx, &mut state, node, depth, session_id, &mut events);
                ctx.dom_documents.insert(session_id.clone(), state);
                ctx.pending_events.extend(events);
                result?;
            }
            Ok(json!({}))
        }
        "requestNode" => {
            let oid = params["objectId"].as_str().ok_or("objectId required")?;
            let page = ctx.get_session_page_mut(session_id).ok_or("No page")?;
            let (world, native) = page.js.as_mut().ok_or("No JavaScript runtime")?.object_node_id(oid)?;
            Ok(json!({"nodeId":push_node(ctx, session_id, world, NodeId::new(native))?}))
        }
        "getNodeForLocation" => {
            let x = params["x"].as_i64().ok_or("integer x required")?;
            let y = params["y"].as_i64().ok_or("integer y required")?;
            if params["ignorePointerEventsNone"].as_bool() == Some(true) {
                return Err("Ignoring pointer-events in hit testing is not supported".into());
            }
            let page = ctx.get_session_page_mut(session_id).ok_or("No page")?;
            let frame = page.frame_id.clone();
            let node = node_for_location(page, x, y)?;
            let id = backend_node_id(ctx, session_id, 0, node.index() as u32)?;
            let mut result = json!({"backendNodeId":id,"frameId":frame});
            if ctx.dom_documents.contains_key(session_id) {
                result["nodeId"] = json!(push_node(ctx, session_id, CdpExecutionWorld::default(), node)?);
            }
            Ok(result)
        }
        "querySelector" => {
            let selector = params.get("selector").and_then(Value::as_str).ok_or("selector required")?;
            let owner = if params.get("nodeId").is_some() { Some(resolve_node_id(ctx, session_id, params)?) } else { None };
            let frame_id = owner.map_or(0, |(world, _)| world.frame_id);
            let page = ctx.get_session_page(session_id).ok_or("No page")?;
            let result = page.js.as_ref().ok_or("No JavaScript runtime")?.with_dom_in_frame(frame_id, |dom| {
                dom.query_selector_from(owner.map_or(dom.document(), |(_, node)| NodeId::new(node)), selector)
            })??;
            let id = match result { Some(node) => backend_node_id(ctx, session_id, frame_id, node.index() as u32)?, None => 0 };
            Ok(json!({"nodeId": id}))
        }
        "querySelectorAll" => {
            let selector = params.get("selector").and_then(Value::as_str).ok_or("selector required")?;
            let owner = if params.get("nodeId").is_some() { Some(resolve_node_id(ctx, session_id, params)?) } else { None };
            let frame_id = owner.map_or(0, |(world, _)| world.frame_id);
            let page = ctx.get_session_page(session_id).ok_or("No page")?;
            let nodes = page.js.as_ref().ok_or("No JavaScript runtime")?.with_dom_in_frame(frame_id, |dom| {
                dom.query_selector_all_from(owner.map_or(dom.document(), |(_, node)| NodeId::new(node)), selector)
            })??;
            let ids = nodes.into_iter().map(|node| backend_node_id(ctx, session_id, frame_id, node.index() as u32))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(json!({ "nodeIds": ids }))
        }
        "getOuterHTML" => {
            // go-rod sends only objectId for an existing element handle.
            // Resolve all three identifiers in their owning frame document.
            let (world, node_id) = resolve_node_id(ctx, session_id, params)?;
            let page = ctx.get_session_page(session_id).ok_or("No page")?;
            let html = page.js.as_ref().ok_or("No JavaScript runtime")?.with_dom_in_frame(world.frame_id, |dom| {
                dom.outer_html(NodeId::new(node_id))
            })?;
            Ok(json!({ "outerHTML": html }))
        }
        "describeNode" => {
            let (world, node_id) = resolve_node_id(ctx, session_id, params)?;
            let depth = params.get("depth").and_then(Value::as_i64).unwrap_or(0);
            let page = ctx.get_session_page(session_id).ok_or("No page")?;
            let mut node = page.js.as_ref().ok_or("No JavaScript runtime")?.with_dom_in_frame(world.frame_id, |dom| {
                serialize_node(dom, NodeId::new(node_id), depth as u32, 0)
            })?;
            scope_node_tree(ctx, session_id, world.frame_id, &mut node)?;
            Ok(json!({"node":node}))
        }
        "resolveNode" => {
            let requested = super::runtime::validate_context(params, "executionContextId", ctx, session_id, "resolveNode")?;
            let (owner, node_id) = resolve_node_id(ctx, session_id, params)?;
            let world = requested.unwrap_or(CdpExecutionWorld { frame_id: owner.frame_id, world_id: None });
            if world.frame_id != owner.frame_id {
                return Err("Node belongs to a different frame document".to_string());
            }
            let page = ctx.get_session_page_mut(session_id).ok_or("No page")?;
            let info = page.js.as_mut().ok_or("No JavaScript runtime")?.store_node_for_cdp(world, node_id)?;
            let object_id = info.object_id.ok_or("Native node did not produce an object handle")?;
            Ok(json!({"object":{
                "type":"object", "subtype":"node", "className":info.class_name,
                "description":info.description, "objectId":object_id,
            }}))
        }
        "setAttributeValue" => Ok(json!({})),
        "removeNode" => Ok(json!({})),
        "focus" => {
            let (world, node_id) = resolve_node_id(ctx, session_id, params)?;
            let page = ctx.get_session_page_mut(session_id).ok_or("No page")?;
            page.dispatch_native_input_in_frame(world.frame_id, &json!({
                "kind": "focus", "nodeId": node_id,
            }))?;
            Ok(json!({}))
        }
        "scrollIntoViewIfNeeded" => {
            let (world, node_id) = resolve_node_id(ctx, session_id, params)?;
            let page = ctx.get_session_page_mut(session_id).ok_or("No page")?;
            // Share the DOM shim's nested-scrollport handling. Fully visible
            // boxes stay put; clipped boxes are centered only where needed.
            let code = format!(
                "(function() {{ var el = globalThis._wrap && globalThis._wrap({0}); \
                 if (!el || typeof el.scrollIntoViewIfNeeded !== 'function') return false; \
                 el.scrollIntoViewIfNeeded(); return true; }})()",
                node_id
            );
            let did_scroll = page.evaluate_in_execution_world(world, &code)?.as_bool().unwrap_or(false);
            if !did_scroll {
                return Err(format!(
                    "node {node_id} could not be resolved to a scrollable element"
                ));
            }
            Ok(json!({}))
        }
        "setFileInputFiles" => {
            // Puppeteer's ElementHandle.uploadFile / Playwright's setInputFiles
            // drive an <input type=file> through this CDP call (issue #359). Read
            // each local file, then hand its bytes (base64) to the JS layer, which
            // builds real File objects and fires input+change like a real
            // selection so page code can read/upload them.
            let (world, node_id) = resolve_node_id(ctx, session_id, params)?;
            let page = ctx.get_session_page_mut(session_id).ok_or("No page")?;
            // setFileInputFiles reads local files and hands their bytes to page
            // JS. Anyone who can reach the CDP port (default localhost, but
            // Docker images bind 0.0.0.0) could otherwise read any file the
            // process can read — the same threat as Page.navigate to file://, so
            // it honours the same opt-in and is off by default.
            if !page.context.allow_file_access {
                return Err("DOM.setFileInputFiles is disabled. Restart with `obscura serve --allow-file-access` to enable local file uploads.".to_string());
            }
            let paths: Vec<String> = params
                .get("files")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default();

            let mut specs = Vec::with_capacity(paths.len());
            for p in &paths {
                let bytes = std::fs::read(p)
                    .map_err(|e| format!("setFileInputFiles: cannot read '{}': {}", p, e))?;
                let name = std::path::Path::new(p)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("file")
                    .to_string();
                specs.push(json!({ "name": name, "type": guess_mime(p), "b64": encode_base64(&bytes) }));
            }

            page.dispatch_native_input_in_frame(world.frame_id, &json!({
                "kind": "setFiles", "nodeId": node_id, "files": specs,
            }))?;
            Ok(json!({}))
        }
        "getBoxModel" => {
            let (world, node_id) = resolve_node_id(ctx, session_id, params)?;
            let page = ctx.get_session_page_mut(session_id).ok_or("No page")?;
            let model = page.native_node_box_model(world.frame_id, NodeId::new(node_id))?
                .ok_or("Could not compute box model")?;
            Ok(json!({"model": {
                "content": quad_value(&model.content),
                "padding": quad_value(&model.padding),
                "border": quad_value(&model.border),
                "margin": quad_value(&model.margin),
                "width": coord_value(model.width), "height": coord_value(model.height),
            }}))
        }
        "getContentQuads" => {
            let (world, node_id) = resolve_node_id(ctx, session_id, params)?;
            let page = ctx.get_session_page_mut(session_id).ok_or("No page")?;
            let model = page.native_node_box_model(world.frame_id, NodeId::new(node_id))?;
            Ok(json!({"quads": model.map(|model| vec![quad_value(&model.content)]).unwrap_or_default()}))
        }
        _ => Err(format!("Unknown DOM method: {}", method)),
    }
}

fn quad_value(quad: &[(f64, f64); 4]) -> Vec<Value> {
    quad.iter().flat_map(|&(x, y)| [coord_value(x), coord_value(y)]).collect()
}

/// Serialize a CDP box-model coordinate the way Chrome does: an integral double
/// (e.g. `256.0`) becomes a JSON integer (`256`), while a genuinely fractional
/// value (`206.0390625`) stays a float. serde_json always writes an `f64` with a
/// decimal point, so `json!(256.0)` yields `256.0` — which strict CDP clients
/// (Hermes Agent) deserialize as `i64` and reject, breaking every click-by-
/// coordinate flow. Chrome only widens fractional coordinates, so mirroring that
/// keeps those clients working. (issue #576)
fn coord_value(n: f64) -> Value {
    // Collapse to an integer only when the value is exactly integral and fits an
    // i64 losslessly; NaN/inf and out-of-range doubles fall through unchanged.
    if n.is_finite() && n.fract() == 0.0 && n >= i64::MIN as f64 && n <= i64::MAX as f64 {
        json!(n as i64)
    } else {
        json!(n)
    }
}

// Hard cap on how deep a getDocument/describeNode response may nest, independent
// of the requested `depth`. DOM.getDocument{depth:-1} arrives here as u32::MAX,
// which on a pathologically deep DOM (trivially scriptable, unbounded by
// html5ever) produces a Value nested that far. Even built without recursion,
// serde_json's own serialization and the Value's Drop recurse over that nesting
// and overflow the stack — on tokio's ~2 MiB worker stacks especially. Bounding
// the depth keeps the response safe to serialize and drop. Real DOMs are shallow
// (deep React trees are a few hundred), so this only truncates pathological
// nesting, which beats crashing the worker (AGENTS.md: "one page must never
// crash a worker"). Mirrors DOMSnapshot's MAX_NODES guard (issue #341).
//
// The number is sized for the ~2 MiB stack of a tokio worker thread (where the
// CDP processor runs, and where `#[test]` threads also run): each DOM level
// becomes two nested JSON containers (object -> "children" array -> object ...),
// so serde_json's recursive serialization and the Value's recursive Drop descend
// ~2x this depth. 256 keeps that a few hundred frames deep with wide margin,
// while still far exceeding any real page. Clients needing a deeper subtree
// re-request it with DOM.requestChildNodes / describeNode on a specific node.
const MAX_SERIALIZE_DEPTH: u32 = 256;

/// Build the CDP Node object for a single node (without its `children` array),
/// returning it together with that node's child ids. `None` for a missing node.
fn node_value(dom: &DomTree, node_id: NodeId) -> Option<(Value, Vec<NodeId>)> {
    let node = dom.get_node(node_id)?;
    let children_ids = dom.children(node_id);
    let child_count = children_ids.len();
    let mut result = json!({ "nodeId": node_id.index(), "backendNodeId": node_id.index(), "childNodeCount": child_count });

    match &node.data {
        NodeData::Document => {
            result["nodeType"] = json!(9); result["nodeName"] = json!("#document");
            result["localName"] = json!(""); result["nodeValue"] = json!("");
            result["documentURL"] = json!(""); result["baseURL"] = json!(""); result["xmlVersion"] = json!("");
        }
        NodeData::Doctype { name, public_id, system_id } => {
            result["nodeType"] = json!(10); result["nodeName"] = json!(name);
            result["localName"] = json!(""); result["nodeValue"] = json!("");
            result["publicId"] = json!(public_id); result["systemId"] = json!(system_id);
        }
        NodeData::Element { name, attrs, .. } => {
            result["nodeType"] = json!(1);
            result["nodeName"] = json!(str::to_ascii_uppercase(&name.local));
            result["localName"] = json!(&*name.local);
            result["nodeValue"] = json!("");
            let cdp_attrs: Vec<String> = attrs.iter()
                .flat_map(|a| [a.qualified_name(), a.value.clone()]).collect();
            result["attributes"] = json!(cdp_attrs);
        }
        NodeData::Text { contents } => {
            result["nodeType"] = json!(3); result["nodeName"] = json!("#text");
            result["localName"] = json!(""); result["nodeValue"] = json!(contents);
        }
        NodeData::Comment { contents } => {
            result["nodeType"] = json!(8); result["nodeName"] = json!("#comment");
            result["localName"] = json!(""); result["nodeValue"] = json!(contents);
        }
        NodeData::ProcessingInstruction { target, data } => {
            result["nodeType"] = json!(7); result["nodeName"] = json!(target);
            result["localName"] = json!(""); result["nodeValue"] = json!(data);
        }
    }

    Some((result, children_ids))
}

/// Serialize a node and its descendants into the CDP Node tree, iteratively.
/// The requested `max_depth` is clamped to `current_depth + MAX_SERIALIZE_DEPTH`
/// so a `depth:-1` (u32::MAX) request on a very deep DOM cannot produce a Value
/// that overflows the stack when serde_json later serializes or drops it. An
/// explicit heap worklist keeps the builder itself off the call stack.
fn serialize_node(dom: &DomTree, node_id: NodeId, max_depth: u32, current_depth: u32) -> Value {
    let max_depth = max_depth.min(current_depth.saturating_add(MAX_SERIALIZE_DEPTH));

    struct Frame {
        value: Value,
        children: Vec<NodeId>,
        next: usize,
        built: Vec<Value>,
        depth: u32,
        expand: bool,
    }

    let (root_value, root_children) = match node_value(dom, node_id) {
        Some(v) => v,
        None => return json!(null),
    };
    let root_expand = current_depth < max_depth && !root_children.is_empty();
    let mut stack = vec![Frame {
        value: root_value,
        children: root_children,
        next: 0,
        built: Vec::new(),
        depth: current_depth,
        expand: root_expand,
    }];

    loop {
        // Decide the next step without holding a borrow across a push.
        let next_child = {
            let top = stack.last_mut().expect("stack is non-empty in loop");
            if top.expand && top.next < top.children.len() {
                let cid = top.children[top.next];
                top.next += 1;
                Some(cid)
            } else {
                None
            }
        };

        match next_child {
            Some(cid) => {
                let child_depth = stack.last().unwrap().depth + 1;
                match node_value(dom, cid) {
                    Some((cval, cchildren)) => {
                        let cexpand = child_depth < max_depth && !cchildren.is_empty();
                        stack.push(Frame {
                            value: cval,
                            children: cchildren,
                            next: 0,
                            built: Vec::new(),
                            depth: child_depth,
                            expand: cexpand,
                        });
                    }
                    // Missing child: match the old recursive behavior of emitting null.
                    None => stack.last_mut().unwrap().built.push(json!(null)),
                }
            }
            None => {
                // This node's children are all built; finalize and fold into parent.
                let mut frame = stack.pop().unwrap();
                if !frame.built.is_empty() {
                    frame.value["children"] = json!(frame.built);
                }
                match stack.last_mut() {
                    Some(parent) => parent.built.push(frame.value),
                    None => return frame.value,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::CdpContext;

    #[tokio::test(flavor = "current_thread")]
    async fn dom_node_tracking_is_released_on_navigation_detach_and_close() {
        let mut ctx = CdpContext::new();
        let page = ctx.create_page();
        let session = Some(format!("{page}-session"));
        ctx.sessions.insert(session.clone().unwrap(), page.clone());
        crate::domains::page::handle("navigate", &json!({"url":"data:text/html,<p>first</p>"}),
            &mut ctx, &session).await.unwrap();
        handle("getDocument", &json!({"depth":-1}), &mut ctx, &session).await.unwrap();
        ctx.pending_events.clear();
        crate::domains::page::handle("navigate", &json!({"url":"data:text/html,<p>second</p>"}),
            &mut ctx, &session).await.unwrap();
        assert!(ctx.dom_documents.is_empty(), "navigation must release the old node cache");
        assert!(ctx.pending_events.iter().any(|event| event.method == "DOM.documentUpdated"
            && event.session_id == session));
        handle("getDocument", &json!({}), &mut ctx, &session).await.unwrap();
        crate::domains::target::handle("detachFromTarget", &json!({"sessionId":session}),
            &mut ctx, &None).await.unwrap();
        assert!(ctx.dom_documents.is_empty(), "detach must release session-owned nodes");
        let closing_session = Some("closing".to_string());
        ctx.sessions.insert(closing_session.clone().unwrap(), page.clone());
        handle("getDocument", &json!({}), &mut ctx, &closing_session).await.unwrap();
        assert_eq!(ctx.dom_documents.len(), 1);
        ctx.remove_page(&page);
        assert!(ctx.dom_documents.is_empty(), "close must release the target's node cache");
    }

    // The escaping this file used to own now lives in `util::object_id_literal`,
    // which is where its tests went with it: the three lookup sites here embed
    // the id as a JSON literal rather than splicing it into a single-quoted
    // string, so there is no per-domain escaping left to assert on.

    // A stale/invalid objectId resolves to -1 in JS. describeNode/resolveNode
    // must surface an error, not cast -1 to nodeId 0 (the document root) and
    // return the wrong node. See #917.
    #[tokio::test]
    async fn describe_node_errors_on_unresolvable_object_id() {
        let mut ctx = CdpContext::new();
        let page_id = ctx.create_page();
        let session = Some(format!("{page_id}-session"));
        ctx.sessions.insert(session.clone().unwrap(), page_id.clone());

        crate::domains::page::handle(
            "navigate",
            &json!({ "url": "data:text/html,<p>hi</p>", "waitUntil": "load" }),
            &mut ctx,
            &session,
        )
        .await
        .expect("navigate should succeed");

        let res = handle(
            "describeNode",
            &json!({ "objectId": "no-such-object" }),
            &mut ctx,
            &session,
        )
        .await;
        assert!(
            res.is_err(),
            "describeNode must error on an unresolvable objectId, got: {res:?}"
        );
    }

    #[tokio::test]
    async fn resolve_node_errors_on_unresolvable_object_id() {
        let mut ctx = CdpContext::new();
        let page_id = ctx.create_page();
        let session = Some(format!("{page_id}-session"));
        ctx.sessions.insert(session.clone().unwrap(), page_id.clone());

        crate::domains::page::handle(
            "navigate",
            &json!({ "url": "data:text/html,<p>hi</p>", "waitUntil": "load" }),
            &mut ctx,
            &session,
        )
        .await
        .expect("navigate should succeed");

        let res = handle(
            "resolveNode",
            &json!({ "objectId": "no-such-object" }),
            &mut ctx,
            &session,
        )
        .await;
        assert!(
            res.is_err(),
            "resolveNode must error on an unresolvable objectId, got: {res:?}"
        );
    }

    #[tokio::test]
    async fn dom_focus_sets_active_element() {
        // CDP clients (browser-use) focus an input via DOM.focus before typing;
        // dispatchKeyEvent then targets document.activeElement. DOM.focus must
        // actually move focus or keystrokes land on nothing.
        let mut ctx = CdpContext::new();
        let page_id = ctx.create_page();
        let session = Some(format!("{page_id}-session"));
        ctx.sessions.insert(session.clone().unwrap(), page_id.clone());

        crate::domains::page::handle(
            "navigate",
            &json!({ "url": "data:text/html,<input id=q>", "waitUntil": "load" }),
            &mut ctx,
            &session,
        )
        .await
        .expect("navigate should succeed");

        let qs = handle("querySelector", &json!({ "selector": "input" }), &mut ctx, &session)
            .await
            .expect("querySelector should succeed");
        let nid = qs["nodeId"].as_u64().expect("input nodeId");
        assert!(nid > 0, "the input element should be found");

        handle("focus", &json!({ "nodeId": nid }), &mut ctx, &session)
            .await
            .expect("DOM.focus should succeed");

        let active = ctx
            .get_session_page_mut(&session)
            .unwrap()
            .evaluate("(function(){return document.activeElement?document.activeElement.tagName:'NONE';})()");
        assert_eq!(
            active,
            json!("INPUT"),
            "DOM.focus must set document.activeElement to the focused input"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn resolve_node_preserves_identity_and_specialized_wrappers() {
        let mut ctx = CdpContext::new();
        let page_id = ctx.create_page();
        let session = Some(format!("{page_id}-session"));
        ctx.sessions.insert(session.clone().unwrap(), page_id);
        crate::domains::page::handle("navigate", &json!({
            "url": "data:text/html,<html><body><textarea id=field></textarea><a id=link href=https://example.com>Next</a></body></html>"
        }), &mut ctx, &session).await.unwrap();
        for (id, constructor) in [
            ("field", "HTMLTextAreaElement"),
            ("link", "HTMLAnchorElement"),
        ] {
            let selector = format!("#{id}");
            let query = handle(
                "querySelector",
                &json!({"selector": selector}),
                &mut ctx,
                &session,
            )
            .await
            .unwrap();
            let resolved = handle(
                "resolveNode",
                &json!({"backendNodeId": query["nodeId"]}),
                &mut ctx,
                &session,
            )
            .await
            .unwrap();
            let object_id = resolved["object"]["objectId"].as_str().unwrap();
            let expression = format!(
                "(() => {{ const node = globalThis.__obscura_objects[{}]; return node === document.getElementById({}) && node instanceof {}; }})()",
                serde_json::to_string(object_id).unwrap(),
                serde_json::to_string(id).unwrap(),
                constructor
            );
            assert_eq!(
                ctx.get_session_page_mut(&session)
                    .unwrap()
                    .evaluate(&expression),
                json!(true)
            );
        }

        let document = handle("getDocument", &json!({}), &mut ctx, &session)
            .await
            .unwrap();
        let resolved = handle(
            "resolveNode",
            &json!({"nodeId": document["root"]["nodeId"]}),
            &mut ctx,
            &session,
        )
        .await
        .unwrap();
        let object_id = resolved["object"]["objectId"].as_str().unwrap();
        let expression = format!(
            "globalThis.__obscura_objects[{}] === document",
            serde_json::to_string(object_id).unwrap()
        );
        assert_eq!(
            ctx.get_session_page_mut(&session)
                .unwrap()
                .evaluate(&expression),
            json!(true)
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn scroll_into_view_if_needed_resolves_all_node_identifiers() {
        let mut ctx = CdpContext::new();
        let page_id = ctx.create_page();
        let session = Some(format!("{page_id}-session"));
        ctx.sessions.insert(session.clone().unwrap(), page_id);

        crate::domains::page::handle(
            "navigate",
            &json!({
                "url": "data:text/html,<main><button id=target>Go</button></main>",
                "waitUntil": "load"
            }),
            &mut ctx,
            &session,
        )
        .await
        .expect("navigate should succeed");

        let query = handle(
            "querySelector",
            &json!({ "selector": "#target" }),
            &mut ctx,
            &session,
        )
        .await
        .expect("querySelector should succeed");
        let node_id = query["nodeId"].as_u64().expect("button nodeId");

        let resolved = handle(
            "resolveNode",
            &json!({ "nodeId": node_id }),
            &mut ctx,
            &session,
        )
        .await
        .expect("resolveNode should succeed");
        let object_id = resolved["object"]["objectId"]
            .as_str()
            .expect("button objectId")
            .to_string();

        for params in [
            json!({ "nodeId": node_id }),
            json!({ "backendNodeId": node_id }),
            json!({ "objectId": object_id }),
        ] {
            ctx.get_session_page_mut(&session)
                .unwrap()
                .evaluate("globalThis.__obscura_click_target = null");

            handle("scrollIntoViewIfNeeded", &params, &mut ctx, &session)
                .await
                .expect("scrollIntoViewIfNeeded should succeed");

            let target_id = ctx
                .get_session_page_mut(&session)
                .unwrap()
                .evaluate("globalThis.__obscura_click_target && globalThis.__obscura_click_target.id");
            assert_eq!(target_id, json!("target"));
        }
    }

    #[tokio::test(flavor = "current_thread")]
    #[cfg(feature = "render")]
    async fn scroll_into_view_if_needed_preserves_visible_positions() {
        let mut ctx = CdpContext::new();
        let page_id = ctx.create_page();
        let session = Some(format!("{page_id}-session"));
        ctx.sessions.insert(session.clone().unwrap(), page_id);
        crate::domains::page::handle("navigate", &json!({
            "url": "data:text/html,<body style='margin:0;height:1800px'><div style='height:200px'></div><button id=target style='height:40px'>Go</button></body>",
            "waitUntil": "load"
        }), &mut ctx, &session).await.unwrap();
        let node = handle("querySelector", &json!({"selector":"#target"}), &mut ctx, &session).await.unwrap();
        handle("scrollIntoViewIfNeeded", &json!({"nodeId":node["nodeId"]}), &mut ctx, &session).await.unwrap();
        let position = ctx.get_session_page_mut(&session).unwrap()
            .evaluate("[scrollY, document.getElementById('target').getBoundingClientRect().top]");
        assert_eq!(position, json!([0, 200]));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn scroll_into_view_if_needed_requires_a_node_identifier() {
        let mut ctx = CdpContext::new();
        let page_id = ctx.create_page();
        let session = Some(format!("{page_id}-session"));
        ctx.sessions.insert(session.clone().unwrap(), page_id);

        let error = handle(
            "scrollIntoViewIfNeeded",
            &json!({}),
            &mut ctx,
            &session,
        )
        .await
        .expect_err("missing node identifier should fail");

        assert_eq!(error, "nodeId, backendNodeId, or objectId required");

        let error = handle(
            "scrollIntoViewIfNeeded",
            &json!({ "nodeId": 999_999 }),
            &mut ctx,
            &session,
        )
        .await
        .expect_err("an unknown node should fail");

        assert_eq!(error, "Cannot find node in the current document");
    }

    // A hostile or heavy page can nest nodes tens of thousands deep (trivially
    // scriptable, and html5ever puts no cap on generic nesting). DOM.getDocument
    // with the standard depth:-1 (which becomes u32::MAX here) must handle such a
    // tree without crashing the worker (AGENTS.md: "one page must never crash a
    // worker"). Two failure modes: (1) a recursive serialize_node overflows the
    // stack building the Value, and (2) even an iterative builder would emit a
    // Value so deeply nested that serde_json's own recursive serialize/Drop
    // overflow. The fix derecurses the builder AND bounds the depth, so the
    // response is always safe to serialize and drop. With the old recursive
    // serialize_node this test aborts with SIGABRT.
    #[test]
    fn get_document_deep_tree_does_not_overflow() {
        use obscura_dom::{DomTree, NodeData};

        // Build the deep chain directly (no parser) so setup is O(n) and fast.
        let dom = DomTree::new();
        let mut parent = dom.document();
        let depth = 50_000usize;
        for _ in 0..depth {
            let n = dom.new_node(NodeData::Text { contents: String::new() });
            dom.append_child(parent, n);
            parent = n;
        }

        // Mirror getDocument {"depth": -1}: as_i64() == -1, then `depth as u32`.
        let node = serialize_node(&dom, dom.document(), (-1i64) as u32, 0);

        // serde_json's own serialization recurses over the Value nesting; this
        // must not overflow either. That is why the fix bounds depth, not just
        // the builder.
        let s = serde_json::to_string(&node).expect("serialize");
        assert!(!s.is_empty());

        // Output nesting is bounded well below the tree's true depth (truncated,
        // not crashed) yet still serializes a meaningful prefix.
        let mut cur = &node;
        let mut levels = 0usize;
        while let Some(children) = cur.get("children").and_then(|c| c.as_array()) {
            let Some(first) = children.first() else { break };
            cur = first;
            levels += 1;
        }
        assert!(levels >= 100, "should serialize a deep prefix, got {levels}");
        assert!(
            levels < depth,
            "nesting must be bounded below the tree's true depth, got {levels}"
        );
    }

    /// SEC-003 / #579 — DOM.setFileInputFiles reads local files and hands
    /// their bytes to page JS. Without a gate, any CDP client (e.g. against a
    /// Docker image that binds the port to 0.0.0.0) gets an arbitrary
    /// file-read primitive. It must honour the same `allow_file_access` opt-in
    /// as Page.navigate to `file://`, which defaults to off.
    #[tokio::test(flavor = "current_thread")]
    async fn set_file_input_files_refuses_without_allow_file_access() {
        let mut ctx = CdpContext::new();
        let page_id = ctx.create_page();
        let session = Some(format!("{page_id}-session"));
        ctx.sessions.insert(session.clone().unwrap(), page_id);

        crate::domains::page::handle(
            "navigate",
            &json!({ "url": "data:text/html,<input type=file id=f>", "waitUntil": "load" }),
            &mut ctx,
            &session,
        )
        .await
        .expect("navigate should succeed");

        let qs = handle("querySelector", &json!({ "selector": "input" }), &mut ctx, &session)
            .await
            .expect("querySelector should succeed");
        let nid = qs["nodeId"].as_u64().expect("input nodeId");

        // A real, readable file. Without the gate the handler slurps it and
        // returns Ok; with the gate it must refuse before touching the disk.
        let existing = format!("{}/Cargo.toml", env!("CARGO_MANIFEST_DIR"));
        let err = handle(
            "setFileInputFiles",
            &json!({ "nodeId": nid, "files": [existing] }),
            &mut ctx,
            &session,
        )
        .await
        .expect_err("setFileInputFiles must be gated behind --allow-file-access");
        assert!(
            err.contains("allow-file-access"),
            "error must point at the allow-file-access flag: {err}"
        );
    }

    // issue #576: DOM.getBoxModel / getContentQuads build their quad from f64
    // pixel values, so serde_json emits integral coordinates as `256.0`. Strict
    // CDP clients (Hermes Agent) deserialize the quad as i64 and reject the
    // float. coord_value must serialize integral coordinates as integers, the way
    // Chrome does, while leaving genuinely fractional ones as floats.
    #[test]
    fn box_model_integral_coordinates_serialize_as_integers() {
        assert_eq!(serde_json::to_string(&coord_value(256.0)).unwrap(), "256");
        assert_eq!(serde_json::to_string(&coord_value(0.0)).unwrap(), "0");
        // fractional coordinates stay floats
        assert_eq!(serde_json::to_string(&coord_value(206.0390625)).unwrap(), "206.0390625");
        // a full quad mixes both, exactly as DOM.getBoxModel returns it
        let quad: Vec<Value> = [
            256.0, 206.0390625, 347.25, 206.0390625, 347.25, 225.0390625, 256.0, 225.0390625,
        ]
        .iter()
        .map(|n| coord_value(*n))
        .collect();
        assert_eq!(
            serde_json::to_string(&quad).unwrap(),
            "[256,206.0390625,347.25,206.0390625,347.25,225.0390625,256,225.0390625]"
        );
    }
}
