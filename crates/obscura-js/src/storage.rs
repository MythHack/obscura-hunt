//! Native Web Storage namespaces. Browser contexts own local storage; each
//! top-level native window owns a session namespace that survives navigation.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use deno_core::{op2, v8, OpDecl, OpState};
use deno_error::JsErrorBox;

/// Origin-partitioned storage shared by a browser context or one native window.
/// Disk persistence remains governed by the BrowserContext's existing policy.
#[derive(Clone, Default)]
pub struct StorageNamespace {
    areas: HashMap<String, StorageArea>,
    bytes: usize,
}

pub type SharedStorage = Arc<Mutex<StorageNamespace>>;

// Storage is held outside V8's heap limit. Bound both per-origin quota and the
// complete namespace, including empty values and distinct origin/key metadata.
const AREA_BYTES: usize = 5 * 1024 * 1024;
const NAMESPACE_BYTES: usize = 64 * 1024 * 1024;
const AREA_ENTRIES: usize = 16_384;
const NAMESPACE_AREAS: usize = 256;

#[derive(Clone, Default)]
struct StorageArea {
    entries: Vec<(String, String)>,
    bytes: usize,
}

fn string_bytes(value: &str) -> usize {
    value.encode_utf16().count().saturating_mul(2)
}

fn entry_bytes(key: &str, value: &str) -> usize {
    string_bytes(key).saturating_add(string_bytes(value)).saturating_add(32)
}

fn security_error() -> JsErrorBox {
    JsErrorBox::new("DOMExceptionSecurityError", "Storage is unavailable for this document")
}

fn quota_error() -> JsErrorBox {
    JsErrorBox::new("DOMExceptionQuotaExceededError", "Storage quota exceeded")
}

impl StorageNamespace {
    fn set(&mut self, origin: &str, key: &str, value: &str) -> Result<(), JsErrorBox> {
        let area = self.areas.get(origin);
        let previous = area.and_then(|area| area.entries.iter().position(|(known, _)| known == key));
        if previous.is_some_and(|index| area.unwrap().entries[index].1 == value) { return Ok(()); }
        let old_bytes = previous.map_or(0, |index| {
            let (key, value) = &area.unwrap().entries[index];
            entry_bytes(key, value)
        });
        let new_bytes = entry_bytes(key, value);
        let origin_bytes = if area.is_none() { string_bytes(origin).saturating_add(64) } else { 0 };
        let new_area = area.is_none();
        let area_bytes = area.map_or(0, |area| area.bytes).saturating_sub(old_bytes)
            .saturating_add(new_bytes);
        let namespace_bytes = self.bytes.saturating_sub(old_bytes)
            .saturating_add(new_bytes).saturating_add(origin_bytes);
        if area_bytes > AREA_BYTES || namespace_bytes > NAMESPACE_BYTES
            || (previous.is_none() && area.is_some_and(|area| area.entries.len() >= AREA_ENTRIES))
            || (area.is_none() && self.areas.len() >= NAMESPACE_AREAS)
        {
            return Err(quota_error());
        }
        if new_area { self.areas.insert(origin.to_string(), StorageArea::default()); }
        let area = self.areas.get_mut(origin).unwrap();
        if let Some(index) = previous {
            let stored = &mut area.entries[index].1;
            stored.clear();
            stored.push_str(value);
        } else {
            area.entries.push((key.to_string(), value.to_string()));
        }
        area.bytes = area_bytes;
        self.bytes = namespace_bytes;
        Ok(())
    }

    fn remove(&mut self, origin: &str, key: &str) {
        let Some(area) = self.areas.get_mut(origin) else { return };
        let Some(index) = area.entries.iter().position(|(known, _)| known == key) else { return };
        let (key, value) = area.entries.remove(index);
        let bytes = entry_bytes(&key, &value);
        area.bytes -= bytes;
        self.bytes -= bytes;
        if area.entries.is_empty() { self.clear(origin); }
    }

    fn clear(&mut self, origin: &str) {
        if let Some(area) = self.areas.remove(origin) {
            self.bytes -= area.bytes + string_bytes(origin) + 64;
        }
    }
}

#[op2]
#[serde]
fn op_storage(
    scope: &mut v8::PinScope,
    state: &OpState,
    receiver: v8::Local<v8::Object>,
    session: bool,
    #[string] action: &str,
    #[string] key: &str,
    #[string] value: &str,
    index: u32,
) -> Result<serde_json::Value, JsErrorBox> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let (registry, root, document) = crate::window::storage_window(scope, state, receiver)?;
        let origin = crate::window::origin(&*document.try_borrow()
            .map_err(|_| JsErrorBox::generic("Storage document is unavailable"))?);
        if origin == "null" { return Err(security_error()); }
        let storage = {
            let registry = registry.try_borrow()
                .map_err(|_| JsErrorBox::generic("Storage window registry is unavailable"))?;
            if session {
                registry.windows.get(&root).ok_or_else(security_error)?.session_storage.clone()
            } else {
                registry.local_storage.clone()
            }
        };
        let mut storage = storage.lock()
            .map_err(|_| JsErrorBox::generic("Storage namespace lock is poisoned"))?;
        match action {
            "check" => Ok(serde_json::Value::Null),
            "get" => Ok(storage.areas.get(&origin)
                .and_then(|area| area.entries.iter().find(|(known, _)| known == key))
                .map_or(serde_json::Value::Null, |(_, value)| serde_json::Value::String(value.clone()))),
            "length" => Ok(serde_json::Value::from(storage.areas.get(&origin)
                .map_or(0, |area| area.entries.len()))),
            "key" => Ok(storage.areas.get(&origin)
                .and_then(|area| area.entries.get(index as usize))
                .map_or(serde_json::Value::Null, |(key, _)| serde_json::Value::String(key.clone()))),
            "keys" => Ok(serde_json::Value::Array(storage.areas.get(&origin)
                .map(|area| area.entries.iter().map(|(key, _)| serde_json::Value::String(key.clone()))
                    .collect()).unwrap_or_default())),
            "set" => {
                storage.set(&origin, key, value)?;
                Ok(serde_json::Value::Null)
            }
            "remove" => {
                storage.remove(&origin, key);
                Ok(serde_json::Value::Null)
            }
            "clear" => {
                storage.clear(&origin);
                Ok(serde_json::Value::Null)
            }
            _ => Err(JsErrorBox::type_error("Unknown storage operation")),
        }
    })).unwrap_or_else(|_| Err(JsErrorBox::generic("Storage operation failed")))
}

pub(crate) fn ops() -> Vec<OpDecl> {
    vec![op_storage()]
}
