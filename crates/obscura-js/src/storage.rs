//! Native Web Storage namespaces. Browser contexts own local storage; each
//! top-level native window owns a session namespace that survives navigation.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use deno_core::{op2, v8, OpDecl, OpState};
use deno_error::JsErrorBox;

/// Origin-partitioned storage shared by a browser context or one native window.
/// Disk persistence remains governed by the BrowserContext's existing policy.
#[derive(Clone, Default)]
pub struct StorageNamespace {
    areas: HashMap<String, StorageArea>,
    bytes: usize,
    changed: HashSet<String>,
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
    pub fn set(&mut self, origin: &str, key: &str, value: &str) -> Result<(), JsErrorBox> {
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
        self.mark_changed(origin);
        Ok(())
    }

    pub fn remove(&mut self, origin: &str, key: &str) {
        let Some(area) = self.areas.get_mut(origin) else { return };
        let Some(index) = area.entries.iter().position(|(known, _)| known == key) else { return };
        let (key, value) = area.entries.remove(index);
        let bytes = entry_bytes(&key, &value);
        area.bytes -= bytes;
        self.bytes -= bytes;
        let empty = area.entries.is_empty();
        self.mark_changed(origin);
        if empty { self.clear(origin); }
    }

    pub fn clear(&mut self, origin: &str) {
        if let Some(area) = self.areas.remove(origin) {
            self.bytes -= area.bytes + string_bytes(origin) + 64;
            self.mark_changed(origin);
        }
    }

    fn mark_changed(&mut self, origin: &str) {
        if !self.changed.contains(origin) { self.changed.insert(origin.to_string()); }
    }

    pub fn get(&self, origin: &str, key: &str) -> Option<String> {
        self.areas.get(origin)
            .and_then(|area| area.entries.iter().find(|(known, _)| known == key))
            .map(|(_, value)| value.clone())
    }

    pub fn len(&self, origin: &str) -> usize {
        self.areas.get(origin).map_or(0, |area| area.entries.len())
    }

    pub fn key(&self, origin: &str, index: usize) -> Option<String> {
        self.areas.get(origin).and_then(|area| area.entries.get(index))
            .map(|(key, _)| key.clone())
    }

    pub fn snapshot(&self, origin: &str) -> Vec<(String, String)> {
        self.areas.get(origin).map_or_else(Vec::new, |area| area.entries.clone())
    }

    pub fn entries(&self) -> Vec<(String, Vec<(String, String)>)> {
        let mut entries: Vec<_> = self.areas.iter()
            .map(|(origin, area)| (origin.clone(), area.entries.clone())).collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        entries
    }

    /// Restore an origin atomically within the native quotas, without marking it dirty.
    pub fn load(&mut self, origin: &str, entries: Vec<(String, String)>) -> bool {
        if entries.len() > AREA_ENTRIES { return false; }
        let mut keys = HashSet::with_capacity(entries.len());
        let mut bytes = 0usize;
        for (key, value) in &entries {
            bytes = bytes.saturating_add(entry_bytes(key, value));
            if bytes > AREA_BYTES || !keys.insert(key.as_str()) { return false; }
        }
        drop(keys);
        let previous = self.areas.get(origin);
        if !entries.is_empty() && previous.is_none() && self.areas.len() >= NAMESPACE_AREAS {
            return false;
        }
        let origin_bytes = string_bytes(origin).saturating_add(64);
        let old_bytes = previous.map_or(0, |area| area.bytes.saturating_add(origin_bytes));
        let new_bytes = if entries.is_empty() { 0 } else { bytes.saturating_add(origin_bytes) };
        let namespace_bytes = self.bytes.saturating_sub(old_bytes).saturating_add(new_bytes);
        if namespace_bytes > NAMESPACE_BYTES { return false; }
        if entries.is_empty() {
            self.areas.remove(origin);
        } else {
            self.areas.insert(origin.to_string(), StorageArea { entries, bytes });
        }
        self.bytes = namespace_bytes;
        self.changed.remove(origin);
        true
    }

    /// Merge only a connection's edits, preserving changes from other isolated clients.
    ///
    /// The two snapshots carry the intent: keys the connection left in place
    /// keep the target's current value (another client may have rewritten
    /// them), keys it deleted disappear, and keys it (re)inserted move behind
    /// the target's retained prefix in the connection's own order. Enumeration
    /// order is observable through `Storage.key(i)` and the stored profile, so a
    /// remove-and-reinsert with an unchanged value must reorder the key rather
    /// than collapse into "no change". A changed value is an explicit write and
    /// reapplies even when a concurrent client deleted the key.
    pub fn merge_delta(&mut self, initial: &Self, current: &Self) -> Result<(), JsErrorBox> {
        let origins = initial.areas.keys().chain(current.areas.keys()
            .filter(|origin| !initial.areas.contains_key(*origin)));
        for origin in origins {
            let before = initial.areas.get(origin).map_or(&[][..], |area| area.entries.as_slice());
            let after = current.areas.get(origin).map_or(&[][..], |area| area.entries.as_slice());
            if before == after { continue; }
            let before_map: HashMap<_, _> = before.iter().enumerate()
                .map(|(index, (key, value))| (key.as_str(), (index, value.as_str()))).collect();
            let after_map: HashMap<_, _> = after.iter()
                .map(|(key, value)| (key.as_str(), value.as_str())).collect();
            // The retained prefix is the longest run of current keys that already
            // existed, in the same relative order, in the snapshot. The first new
            // or out-of-order key begins the reinserted suffix, which the merged
            // origin must end with in this connection's order.
            let mut prefix = 0;
            let mut previous = None;
            while let Some((key, _)) = after.get(prefix) {
                let Some(&(index, _)) = before_map.get(key.as_str()) else { break };
                if previous.is_some_and(|previous| index <= previous) { break; }
                previous = Some(index);
                prefix += 1;
            }
            let suffix: HashSet<&str> = after.iter().skip(prefix)
                .map(|(key, _)| key.as_str()).collect();
            let saved = self.areas.get(origin).map_or(&[][..], |area| area.entries.as_slice());
            let saved_keys: HashSet<&str> = saved.iter().map(|(key, _)| key.as_str()).collect();
            let mut merged = Vec::with_capacity(saved.len() + after.len());
            for (key, value) in saved {
                let named = key.as_str();
                if before_map.contains_key(named) && !after_map.contains_key(named) { continue; }
                if suffix.contains(named) { continue; }
                let value = after_map.get(named).copied()
                    .filter(|value| before_map.get(named).map(|(_, value)| *value) != Some(*value))
                    .unwrap_or(value.as_str());
                merged.push((key.clone(), value.to_string()));
            }
            for (key, value) in after.iter().take(prefix) {
                if !saved_keys.contains(key.as_str())
                    && before_map.get(key.as_str()).map(|(_, value)| *value) != Some(value.as_str()) {
                    merged.push((key.clone(), value.clone()));
                }
            }
            for (key, value) in after.iter().skip(prefix) {
                merged.push((key.clone(), value.clone()));
            }
            if merged.as_slice() == saved { continue; }
            if !self.load(origin, merged) { return Err(quota_error()); }
            self.mark_changed(origin);
        }
        Ok(())
    }

    pub fn is_dirty(&self) -> bool { !self.changed.is_empty() }

    /// The caller holds the namespace lock through each successful write acknowledgement.
    pub fn flush_changed(&mut self, mut write: impl FnMut(&str, &[(String, String)]) -> bool) {
        let areas = &self.areas;
        self.changed.retain(|origin| {
            !write(origin, areas.get(origin).map_or(&[], |area| area.entries.as_slice()))
        });
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
            "get" => Ok(storage.get(&origin, key)
                .map_or(serde_json::Value::Null, serde_json::Value::String)),
            "length" => Ok(serde_json::Value::from(storage.len(&origin))),
            "key" => Ok(storage.key(&origin, index as usize)
                .map_or(serde_json::Value::Null, serde_json::Value::String)),
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

#[cfg(test)]
mod tests {
    use super::{AREA_BYTES, AREA_ENTRIES, NAMESPACE_AREAS, NAMESPACE_BYTES, StorageNamespace};

    // #678: Web Storage lives in Rust, keyed by origin, so these pin the
    // behaviour a realm-local object could not provide. Quota accounting is in
    // UTF-16 units plus per-entry metadata, so every size below is expressed
    // against `AREA_BYTES` rather than `str::len`.
    #[test]
    fn deletion_reinsertion_and_profile_replacement_keep_keys_and_quota_consistent() {
        let mut store = StorageNamespace::default();
        let origin = "https://app.test";
        assert!(store.load(origin, vec![
            ("a".into(), "one".into()),
            ("b".into(), "two".into()),
            ("c".into(), "three".into()),
        ]));
        store.remove(origin, "b");
        assert_eq!(store.get(origin, "c").as_deref(), Some("three"));
        // Replacing a live key keeps its position; a re-inserted key moves to
        // the end of the enumeration order.
        assert!(store.set(origin, "c", "replaced").is_ok());
        assert!(store.set(origin, "b", "last").is_ok());
        assert_eq!(store.snapshot(origin), vec![
            ("a".into(), "one".into()),
            ("c".into(), "replaced".into()),
            ("b".into(), "last".into()),
        ]);
        store.remove(origin, "a");
        assert_eq!(store.key(origin, 0).as_deref(), Some("c"));
        assert_eq!(store.get(origin, "b").as_deref(), Some("last"));

        // A profile load replaces the whole origin, dropping the entries the
        // stored profile does not name.
        assert!(store.load(origin, vec![("z".into(), "x".repeat(AREA_BYTES / 2 - 32))]));
        assert_eq!(store.get(origin, "c"), None);
        assert!(store.set(origin, "extra", "").is_err());
        assert_eq!(store.len(origin), 1, "a rejected write must not resize the origin");
        assert!(store.set(origin, "z", "short").is_ok());
        assert!(store.set(origin, "extra", &"y".repeat(AREA_BYTES / 2 - 50)).is_ok());
        assert!(store.set(origin, "third", "").is_err());
        store.remove(origin, "z");
        assert!(store.set(origin, "third", "").is_ok());
        assert_eq!(store.key(origin, 0).as_deref(), Some("extra"));
        store.clear(origin);
        assert_eq!(store.len(origin), 0);
        assert!(store.set(origin, "fresh", "").is_ok());
        assert_eq!(store.key(origin, 0).as_deref(), Some("fresh"));
    }

    #[test]
    fn duplicate_profile_keys_are_rejected_without_replacing_valid_data() {
        let mut store = StorageNamespace::default();
        assert!(store.set("https://app.test", "token", "original").is_ok());
        assert!(!store.load("https://app.test", vec![
            ("key".into(), "one".into()),
            ("key".into(), "two".into()),
        ]));
        assert_eq!(store.get("https://app.test", "token").as_deref(), Some("original"));
        assert_eq!(store.len("https://app.test"), 1, "a rejected profile must not partially apply");
    }

    #[test]
    fn over_budget_profiles_are_rejected_without_replacing_valid_data() {
        let mut store = StorageNamespace::default();
        assert!(store.set("https://app.test", "token", "original").is_ok());
        assert!(!store.load("https://app.test", vec![("key".into(), "x".repeat(AREA_BYTES))]));
        assert_eq!(store.get("https://app.test", "token").as_deref(), Some("original"));

        let mut count_limited = StorageNamespace::default();
        for index in 0..NAMESPACE_AREAS {
            assert!(count_limited.set(&format!("https://origin-{index}.test"), "key", "value").is_ok());
        }
        assert!(!count_limited.load("https://extra.test", vec![("key".into(), "value".into())]));
        assert_eq!(count_limited.get("https://extra.test", "key"), None);

        let mut total_limited = StorageNamespace::default();
        let value = "x".repeat(AREA_BYTES / 2 - 64);
        let mut accepted = 0;
        for index in 0..NAMESPACE_AREAS {
            if total_limited.set(&format!("https://large-{index}.test"), "key", &value).is_err() { break; }
            accepted += 1;
        }
        assert!(accepted > 1, "a per-area-sized entry must fit several times");
        assert!(accepted < NAMESPACE_AREAS, "the namespace budget must stop writes before the origin count does");
        assert!(!total_limited.load("https://extra.test", vec![("key".into(), value)]));
        assert_eq!(total_limited.get("https://extra.test", "key"), None);
    }

    #[test]
    fn restored_profiles_do_not_mark_the_namespace_dirty() {
        let mut store = StorageNamespace::default();
        assert!(store.load("https://app.test", vec![("token".into(), "restored".into())]));
        assert_eq!(store.get("https://app.test", "token").as_deref(), Some("restored"));
        assert!(!store.is_dirty(), "restoring a saved profile must not schedule a rewrite");
    }

    #[test]
    fn origins_stay_apart_and_insertion_order_survives_deletion() {
        let mut store = StorageNamespace::default();
        assert!(store.set("https://a.example", "k1", "one").is_ok());
        assert!(store.set("https://a.example", "k2", "two").is_ok());
        assert!(store.set("https://b.example", "k1", "other").is_ok());

        // Storage.key() is defined in insertion order, not sorted order.
        let order: Vec<String> = (0..store.len("https://a.example"))
            .filter_map(|index| store.key("https://a.example", index))
            .collect();
        assert_eq!(order, vec!["k1".to_string(), "k2".to_string()]);

        // Deleting and re-inserting a key moves it behind the survivors.
        store.remove("https://a.example", "k1");
        assert_eq!(store.key("https://a.example", 0).as_deref(), Some("k2"));
        assert!(store.set("https://a.example", "k1", "one").is_ok());
        assert_eq!(store.key("https://a.example", 1).as_deref(), Some("k1"));

        assert_eq!(store.get("https://a.example", "k1").as_deref(), Some("one"));
        assert_eq!(store.get("https://b.example", "k1").as_deref(), Some("other"));
        assert_eq!(store.get("https://c.example", "k1"), None);
    }

    #[test]
    fn replacing_a_value_stays_in_place_without_growing_the_quota() {
        let mut store = StorageNamespace::default();
        let origin = "https://a.example";
        let big = AREA_BYTES / 2 - 128;
        assert!(store.set(origin, "k", &"x".repeat(big)).is_ok());
        // Overwriting an existing key must not count the old bytes twice: the
        // replacement fits even though a second key of the same size would not,
        // and the entry count must not change.
        assert!(store.set(origin, "k", &"y".repeat(big)).is_ok());
        assert_eq!(store.len(origin), 1);
        assert_eq!(store.get(origin, "k").map(|value| value.len()), Some(big));
        assert!(store.set(origin, "second", &"z".repeat(big)).is_err());

        // A single value over the per-origin budget is rejected, and the
        // rejected write leaves the store untouched.
        assert!(store.set(origin, "huge", &"z".repeat(AREA_BYTES)).is_err());
        assert_eq!(store.len(origin), 1);
        assert_eq!(store.get(origin, "k").map(|value| value.len()), Some(big));
    }

    #[test]
    fn remove_and_clear_return_the_quota() {
        let mut store = StorageNamespace::default();
        let origin = "https://a.example";
        let value = "x".repeat(AREA_BYTES / 4 - 32);
        assert!(store.set(origin, "k1", &value).is_ok());
        assert!(store.set(origin, "k2", &value).is_ok());
        // The two entries saturate the origin budget: one more of the same size
        // cannot fit.
        assert!(store.set(origin, "k3", &value).is_err());

        store.remove(origin, "k1");
        assert_eq!(store.get(origin, "k1"), None);
        // The freed bytes are usable again, which is the part a naive counter
        // gets wrong.
        assert!(store.set(origin, "k3", &value).is_ok());
        assert_eq!(store.len(origin), 2);

        store.clear(origin);
        assert!(store.snapshot(origin).is_empty());
        assert!(store.set(origin, "after-clear", &value).is_ok());
    }

    #[test]
    fn namespace_budget_limits_total_bytes_and_origin_count() {
        // Distinct origins share one namespace budget.
        let mut store = StorageNamespace::default();
        let value = "x".repeat(AREA_BYTES / 2 - 64);
        let mut rejected = 0;
        for index in 0..NAMESPACE_AREAS {
            if store.set(&format!("https://o{index}.example"), "k", &value).is_err() { rejected += 1; }
        }
        assert!(
            rejected > 0,
            "the namespace budget must stop writes once {NAMESPACE_BYTES} bytes are used"
        );
        assert!(store.get("https://o0.example", "k").is_some(), "accepted origins keep their data");

        // The origin count is capped even when each origin is tiny.
        let mut counted = StorageNamespace::default();
        for index in 0..NAMESPACE_AREAS {
            assert!(counted.set(&format!("https://o{index}.example"), "k", "v").is_ok());
        }
        assert!(
            counted.set("https://one-too-many.example", "k", "v").is_err(),
            "an origin past the count limit must be rejected"
        );
        assert_eq!(counted.get("https://o0.example", "k").as_deref(), Some("v"));

        // A single origin also stops taking new keys at the entry cap, while
        // replacements keep working.
        let mut entry_limited = StorageNamespace::default();
        let origin = "https://many.example";
        for index in 0..AREA_ENTRIES {
            assert!(entry_limited.set(origin, &format!("k{index}"), "v").is_ok());
        }
        assert!(entry_limited.set(origin, "one-too-many", "v").is_err());
        assert_eq!(entry_limited.len(origin), AREA_ENTRIES);
        assert!(entry_limited.set(origin, "k0", "w").is_ok());
    }

    #[test]
    fn mutations_flush_only_on_acknowledgement_and_clear_marks_the_origin() {
        let mut store = StorageNamespace::default();
        let origin = "https://a.example";
        assert!(!store.is_dirty());
        assert!(store.set(origin, "k", "v").is_ok());
        assert!(store.is_dirty(), "a write must schedule a rewrite");

        let mut attempts = 0;
        store.flush_changed(|flushed, entries| {
            attempts += 1;
            assert_eq!(flushed, origin);
            assert_eq!(entries, [("k".to_string(), "v".to_string())].as_slice());
            false
        });
        assert!(store.is_dirty(), "an unacknowledged write must stay dirty for a retry");
        store.flush_changed(|flushed, entries| {
            attempts += 1;
            assert_eq!(flushed, origin);
            assert_eq!(entries, [("k".to_string(), "v".to_string())].as_slice());
            true
        });
        assert_eq!(attempts, 2, "the retry must be offered the same origin again");
        assert!(!store.is_dirty());

        let mut clean_calls = 0;
        store.flush_changed(|_, _| { clean_calls += 1; true });
        assert_eq!(clean_calls, 0, "a clean namespace must not rewrite anything");

        store.clear(origin);
        assert!(store.is_dirty(), "a clear must mark the store dirty");
        store.flush_changed(|flushed, entries| {
            assert_eq!(flushed, origin);
            assert!(entries.is_empty(), "a cleared origin flushes empty so its file is removed");
            true
        });
        assert!(!store.is_dirty());

        // Removing the last key is a deletion too.
        assert!(store.set(origin, "only", "value").is_ok());
        store.flush_changed(|_, _| true);
        store.remove(origin, "only");
        assert!(store.is_dirty(), "removing the last key must schedule the deletion");
        store.flush_changed(|flushed, entries| {
            assert_eq!(flushed, origin);
            assert!(entries.is_empty());
            true
        });
        assert!(!store.is_dirty());
    }

    /// A namespace holding one origin exactly as it was loaded from disk.
    fn loaded(origin: &str, entries: &[(&str, &str)]) -> StorageNamespace {
        let mut store = StorageNamespace::default();
        assert!(store.load(origin, entries.iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string())).collect()));
        assert!(!store.is_dirty(), "loading a snapshot must not mark it dirty");
        store
    }

    // A CDP connection works on its own copy of the namespace while attached,
    // so its delta merges back key by key: edits made by other connections on
    // keys this connection never touched must survive the merge.
    #[test]
    fn merged_deltas_keep_unrelated_keys_and_apply_only_the_clients_edits() {
        let origin = "https://app.test";
        let initial = loaded(origin, &[("a", "1"), ("gone", "bye")]);
        let mut current = initial.clone();
        assert!(current.set(origin, "a", "2").is_ok());
        assert!(current.set(origin, "added", "new").is_ok());
        current.remove(origin, "gone");

        let mut shared = loaded(origin, &[("a", "1"), ("gone", "bye")]);
        assert!(shared.set(origin, "a", "5").is_ok());
        assert!(shared.set(origin, "other-client", "kept").is_ok());
        assert!(shared.set("https://other.test", "k", "v").is_ok());
        shared.flush_changed(|_, _| true);

        assert!(shared.merge_delta(&initial, &current).is_ok());
        assert_eq!(shared.get(origin, "a").as_deref(), Some("2"), "this client's edit wins for its own key");
        assert_eq!(shared.get(origin, "added").as_deref(), Some("new"));
        assert_eq!(shared.get(origin, "gone"), None, "this client's deletion applies");
        assert_eq!(shared.get(origin, "other-client").as_deref(), Some("kept"), "unrelated keys survive");
        assert_eq!(shared.get("https://other.test", "k").as_deref(), Some("v"));

        assert!(shared.is_dirty());
        let mut flushed = Vec::new();
        shared.flush_changed(|flushed_origin, entries| {
            flushed.push((flushed_origin.to_string(), entries.to_vec()));
            true
        });
        assert_eq!(flushed, vec![(origin.to_string(), vec![
            ("a".into(), "2".into()),
            ("other-client".into(), "kept".into()),
            ("added".into(), "new".into()),
        ])], "only the touched origin is offered for rewrite");
        assert!(!shared.is_dirty());
    }

    #[test]
    fn merging_the_last_key_deletion_keeps_the_origin_dirty_for_file_removal() {
        let origin = "https://app.test";
        let initial = loaded(origin, &[("only", "value")]);
        let mut current = initial.clone();
        current.remove(origin, "only");

        let mut shared = loaded(origin, &[("only", "value")]);
        assert!(shared.merge_delta(&initial, &current).is_ok());
        assert!(shared.snapshot(origin).is_empty(), "the emptied origin must not survive the merge");
        assert!(shared.is_dirty(), "removing the origin on disk needs a flush");
        let mut flushed = Vec::new();
        shared.flush_changed(|flushed_origin, entries| {
            flushed.push((flushed_origin.to_string(), entries.to_vec()));
            true
        });
        assert_eq!(flushed, vec![(origin.to_string(), Vec::<(String, String)>::new())],
            "the emptied origin must flush empty so its file is removed");
        assert!(!shared.is_dirty());
    }

    #[test]
    fn merge_delta_rejects_an_over_budget_origin_without_touching_saved_data() {
        let origin = "https://app.test";
        let filler = AREA_BYTES / 2 - 128;
        let initial = loaded(origin, &[("keys", "initial")]);
        let mut current = loaded(origin, &[("keys", "initial")]);
        assert!(current.load(origin, vec![("big".into(), "x".repeat(filler))]));

        let mut shared = loaded(origin, &[("keep", "y".repeat(filler).as_str())]);
        assert!(shared.set(origin, "other-client", "v").is_ok());
        shared.flush_changed(|_, _| true);
        let saved = shared.snapshot(origin);

        let mut probe = StorageNamespace::default();
        let quota = probe.set(origin, "k", &"x".repeat(AREA_BYTES)).unwrap_err();
        let error = shared.merge_delta(&initial, &current).unwrap_err();
        assert_eq!(format!("{error}"), format!("{quota}"), "a rejected merge reports the native quota error");
        assert_eq!(shared.snapshot(origin), saved, "a rejected merge must leave the origin untouched");
        assert!(!shared.is_dirty(), "a rejected merge must not schedule a rewrite");
    }

    // A remove-and-reinsert is an order change even when the value comes back
    // unchanged, and `Storage.key(i)` plus the stored profile record that order.
    #[test]
    fn merged_reinsertion_keeps_the_clients_key_order_and_unrelated_keys() {
        let origin = "https://app.test";
        let initial = loaded(origin, &[("a", "1"), ("b", "2"), ("c", "3")]);
        let mut current = initial.clone();
        current.remove(origin, "a");
        assert!(current.set(origin, "a", "1").is_ok());

        let mut shared = loaded(origin, &[("a", "1"), ("b", "2"), ("c", "3")]);
        assert!(shared.set(origin, "unrelated", "x").is_ok());
        shared.flush_changed(|_, _| true);

        assert!(shared.merge_delta(&initial, &current).is_ok());
        assert_eq!(shared.snapshot(origin), vec![
            ("b".into(), "2".into()),
            ("c".into(), "3".into()),
            ("unrelated".into(), "x".into()),
            ("a".into(), "1".into()),
        ], "a re-inserted key moves behind the retained prefix in the client's order");
        assert!(shared.is_dirty());
    }

    #[test]
    fn merged_prefix_keys_keep_the_target_and_do_not_resurrect_concurrent_deletes() {
        let origin = "https://app.test";
        let initial = loaded(origin, &[("kept", "1"), ("edited", "2"), ("gone-elsewhere", "3")]);
        let mut current = initial.clone();
        assert!(current.set(origin, "edited", "2-updated").is_ok());

        let mut shared = loaded(origin, &[("kept", "1"), ("edited", "2"), ("gone-elsewhere", "3")]);
        shared.remove(origin, "gone-elsewhere");
        assert!(shared.set(origin, "kept", "concurrent").is_ok());
        assert!(shared.set(origin, "other", "kept").is_ok());
        shared.flush_changed(|_, _| true);

        assert!(shared.merge_delta(&initial, &current).is_ok());
        assert_eq!(shared.get(origin, "kept").as_deref(), Some("concurrent"),
            "an untouched key keeps the value another client wrote");
        assert_eq!(shared.get(origin, "edited").as_deref(), Some("2-updated"));
        assert_eq!(shared.get(origin, "gone-elsewhere"), None,
            "an untouched key must not resurrect another client's deletion");
        assert_eq!(shared.get(origin, "other").as_deref(), Some("kept"));
        assert_eq!(shared.snapshot(origin), vec![
            ("kept".into(), "concurrent".into()),
            ("edited".into(), "2-updated".into()),
            ("other".into(), "kept".into()),
        ]);
        assert!(shared.is_dirty());
    }

    #[test]
    fn merged_changed_value_reapplies_after_a_concurrent_delete() {
        let origin = "https://app.test";
        let initial = loaded(origin, &[("edited", "1")]);
        let mut current = initial.clone();
        assert!(current.set(origin, "edited", "2").is_ok());

        let mut shared = loaded(origin, &[("edited", "1")]);
        shared.remove(origin, "edited");
        shared.flush_changed(|_, _| true);

        assert!(shared.merge_delta(&initial, &current).is_ok());
        assert_eq!(shared.get(origin, "edited").as_deref(), Some("2"),
            "an explicit write must survive another client's concurrent delete");
        assert!(shared.is_dirty());
    }
}
