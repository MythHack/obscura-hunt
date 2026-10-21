use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use obscura_js::storage::{SharedStorage, StorageNamespace};
use obscura_net::{CookieJar, ObscuraHttpClient, RobotsCache};

pub struct BrowserContext {
    pub id: String,
    pub cookie_jar: Arc<CookieJar>,
    /// Live origin-partitioned localStorage for every page in this context.
    /// Owned by the context rather than by a V8 realm, so an entry written in
    /// one document is still there in the next (issue #678), and shared with
    /// every native window the context opens.
    pub local_storage: SharedStorage,
    pub indexed_db: Arc<obscura_js::indexed_db::IndexedDbStorage>,
    pub http_client: Arc<ObscuraHttpClient>,
    /// Resolution errors remain observable before any page I/O or script.
    pub regional: Result<Arc<crate::regional::ResolvedRegion>, String>,
    pub geolocation_permissions: Arc<obscura_js::geolocation::Permissions>,
    pub user_agent: String,
    pub platform: String,
    pub ua_platform: String,
    pub ua_platform_version: String,
    pub proxy_url: Option<String>,
    pub robots_cache: Arc<RobotsCache>,
    pub obey_robots: bool,
    pub stealth: bool,
    /// When true, CDP-driven navigation to file:// URLs is permitted.
    /// Default is false: a remote CDP client cannot point the browser
    /// at /etc/shadow even if Obscura is running as a privileged user.
    /// Flip on via `obscura serve --allow-file-access` for legitimate
    /// local-HTML testing workflows. Enforced by `Page` navigation itself,
    /// so every CDP and MCP route is covered; the CLI's own `obscura fetch
    /// file://...` opts its local context in. A page can never drive
    /// itself from a web origin into file:// regardless of this flag.
    pub allow_file_access: bool,
    pub storage_dir: Option<PathBuf>,
    /// When true, the http client allows fetching localhost / RFC1918 /
    /// link-local addresses. Set via `--allow-private-network` (issue #33).
    /// Independent of `allow_file_access` because they cover different threat
    /// models: file:// is a local file-system read, while private-network is
    /// the broader SSRF gate from issue #4.
    pub allow_private_network: bool,
}

/// One origin's persisted `localStorage`. The origin is inside the file, so the
/// file name only has to be unique and stable, not reversible.
#[derive(serde::Serialize, serde::Deserialize)]
struct StoredOriginStorage {
    origin: String,
    /// `[key, value]` pairs in insertion order, the order `Storage.key(i)` uses.
    entries: Vec<(String, String)>,
}

/// Filename-safe stem for an origin: a sanitized prefix for readability plus the
/// full FNV-1a digest of the origin, so `https://a-b.example` and
/// `https://a_b.example` cannot share a file.
fn storage_file_stem(origin: &str) -> String {
    let prefix: String = origin
        .chars()
        .take(48)
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' { c } else { '_' })
        .collect();
    let mut digest: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in origin.as_bytes() {
        digest ^= u64::from(*byte);
        digest = digest.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{prefix}-{digest:016x}")
}

fn profile_error(action: &str, path: &std::path::Path, error: std::io::Error) -> std::io::Error {
    std::io::Error::new(error.kind(), format!("Profile {action} failed for {}: {error}", path.display()))
}

/// Restore origin-partitioned storage before any document can execute.
fn load_web_storage(storage: &SharedStorage, dir: &PathBuf) -> std::io::Result<()> {
    use std::io::Read;
    let root = dir.join("localStorage");
    obscura_net::profile::private_directory(&root)
        .map_err(|error| profile_error("directory permissions", &root, error))?;
    let read_dir = std::fs::read_dir(&root)
        .map_err(|error| profile_error("load directory", &root, error))?;
    let mut storage = storage.lock().expect("local storage namespace lock is poisoned");
    let mut origins = std::collections::HashSet::new();
    for entry in read_dir {
        let entry = entry.map_err(|error| profile_error("load directory entry", &root, error))?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") { continue; }
        let kind = entry.file_type().map_err(|error| profile_error("load metadata", &path, error))?;
        // Reject special entries without opening them (a FIFO must not block startup).
        if !kind.is_file() {
            return Err(profile_error("load localStorage", &path,
                std::io::Error::new(std::io::ErrorKind::InvalidData, "origin storage entry is not a regular file")));
        }
        obscura_net::profile::private_file(&path)
            .map_err(|error| profile_error("file permissions", &path, error))?;
        // A 5 MiB store can expand sixfold when encoded as JSON escapes.
        let mut contents = String::new();
        let limit = 32 * 1024 * 1024;
        std::fs::File::open(&path).and_then(|file| file.take(limit + 1).read_to_string(&mut contents))
            .map_err(|error| profile_error("load localStorage", &path, error))?;
        if contents.len() as u64 > limit {
            return Err(profile_error("load localStorage", &path,
                std::io::Error::new(std::io::ErrorKind::InvalidData, "encoded origin exceeds storage limit")));
        }
        // Do not include serde's input-dependent error text: malformed profile
        // data can contain session secrets in field names or unexpected values.
        let stored: StoredOriginStorage = serde_json::from_str(&contents).map_err(|_| {
            profile_error("load localStorage", &path,
                std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid origin storage JSON"))
        })?;
        let valid_origin = stored.origin == "file://" || url::Url::parse(&stored.origin)
            .is_ok_and(|url| url.origin().ascii_serialization() == stored.origin && stored.origin != "null");
        if !valid_origin || !origins.insert(stored.origin.clone()) || !storage.load(&stored.origin, stored.entries) {
            return Err(profile_error("load localStorage", &path,
                std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid, duplicate or over-quota origin storage")));
        }
    }
    Ok(())
}

impl BrowserContext {
    pub fn new(id: String) -> Self {
        Self::_new_inner(id, None, false, None, None, false).expect("memory-only context has no profile I/O")
    }

    /// Create a BrowserContext with an optional storage directory.
    /// When `storage_dir` is set, cookies are automatically loaded from
    /// `{storage_dir}/cookies.json` on creation.
    pub fn with_storage(
        id: String,
        storage_dir: Option<PathBuf>,
    ) -> std::io::Result<Self> {
        Self::_new_inner(id, None, false, None, storage_dir.or_else(|| std::env::var_os("OBSCURA_STORAGE_DIR").map(PathBuf::from)), false)
    }

    /// Create a BrowserContext with full options including storage_dir.
    pub fn with_storage_full(
        id: String,
        proxy_url: Option<String>,
        stealth: bool,
        user_agent: Option<String>,
        storage_dir: Option<PathBuf>,
    ) -> std::io::Result<Self> {
        Self::_new_inner(id, proxy_url, stealth, user_agent, storage_dir.or_else(|| std::env::var_os("OBSCURA_STORAGE_DIR").map(PathBuf::from)), false)
    }

    /// Variant that also accepts the `allow_private_network` opt-in. All
    /// pre-existing constructors default it to `false`; callers that want the
    /// CLI's `--allow-private-network` (issue #33) behaviour go through here.
    pub fn with_storage_and_network(
        id: String,
        proxy_url: Option<String>,
        stealth: bool,
        user_agent: Option<String>,
        storage_dir: Option<PathBuf>,
        allow_private_network: bool,
    ) -> std::io::Result<Self> {
        Self::_new_inner(id, proxy_url, stealth, user_agent, storage_dir.or_else(|| std::env::var_os("OBSCURA_STORAGE_DIR").map(PathBuf::from)), allow_private_network)
    }

    fn _new_inner(
        id: String,
        proxy_url: Option<String>,
        stealth: bool,
        user_agent: Option<String>,
        storage_dir: Option<PathBuf>,
        allow_private_network: bool,
    ) -> std::io::Result<Self> {
        let cookie_jar = Arc::new(CookieJar::new());

        if let Some(dir) = &storage_dir {
            obscura_net::profile::private_directory(dir)
                .map_err(|error| profile_error("directory permissions", dir, error))?;
            let cookie_path = dir.join("cookies.json");
            cookie_jar.load_from_file(&cookie_path)
                .map_err(|error| profile_error("load cookies", &cookie_path, error))?;
        }

        let mut client = ObscuraHttpClient::with_full_options(
            cookie_jar.clone(),
            proxy_url.as_deref(),
            allow_private_network,
        );
        if stealth {
            client.block_trackers = true;
        }
        let profile = crate::profiles::select_profile();
        let explicit_user_agent = user_agent.is_some();
        let resolved_ua = user_agent.unwrap_or_else(|| profile.user_agent.to_string());
        let (platform, ua_platform, ua_platform_version) = explicit_user_agent
            .then(|| crate::profiles::platform_from_user_agent(&resolved_ua)).flatten()
            .unwrap_or((profile.platform, profile.ua_platform, profile.ua_platform_version));
        let (platform, ua_platform, ua_platform_version) = (platform.to_string(), ua_platform.to_string(), ua_platform_version.to_string());
        // Sync the http client's UA at construction so navigation requests pick it
        // up before any async setup runs. The lock has no other holders here, so
        // try_write always succeeds; we fall back silently if it ever fails.
        if let Ok(mut guard) = client.user_agent.try_write() {
            *guard = resolved_ua.clone();
        }
        let regional = crate::regional::context_region(proxy_url.clone(), stealth, allow_private_network);
        if let Ok(region) = &regional {
            *client.accept_language.get_mut() = region.accept_language();
        }
        *client.platform.get_mut() = ua_platform.clone();
        let http_client = Arc::new(client);
        let local_storage: SharedStorage = Arc::new(Mutex::new(StorageNamespace::default()));
        if let Some(dir) = &storage_dir {
            load_web_storage(&local_storage, dir)?;
        }

        Ok(BrowserContext {
            id,
            cookie_jar,
            local_storage,
            indexed_db: Arc::new(obscura_js::indexed_db::IndexedDbStorage::default()),
            http_client,
            regional,
            geolocation_permissions: Arc::new(obscura_js::geolocation::Permissions::default()),
            user_agent: resolved_ua,
            platform,
            ua_platform,
            ua_platform_version,
            proxy_url,
            robots_cache: Arc::new(RobotsCache::new()),
            obey_robots: false,
            stealth,
            allow_file_access: false,
            storage_dir,
            allow_private_network,
        })
    }

    pub fn with_options(id: String, proxy_url: Option<String>, stealth: bool) -> Self {
        Self::with_full_options(id, proxy_url, stealth, None)
    }

    pub fn with_full_options(
        id: String,
        proxy_url: Option<String>,
        stealth: bool,
        user_agent: Option<String>,
    ) -> Self {
        Self::_new_inner(id, proxy_url, stealth, user_agent, None, false).expect("memory-only context has no profile I/O")
    }

    pub fn with_proxy(id: String, proxy_url: Option<String>) -> Self {
        Self::with_options(id, proxy_url, false)
    }

    /// Create an explicitly independent context with its own network state,
    /// Web Storage view and IndexedDB store. A persistent copy starts from the
    /// current storage snapshot and retains the profile directory; an incognito
    /// copy starts empty and never writes to the template's profile.
    pub fn isolated_copy(&self, id: String, persistent: bool) -> Self {
        let cookie_jar = Arc::new(CookieJar::new());
        if persistent {
            cookie_jar.copy_from(&self.cookie_jar);
        }
        let mut client = ObscuraHttpClient::with_full_options(
            cookie_jar.clone(),
            self.proxy_url.as_deref(),
            self.allow_private_network,
        );
        if self.stealth {
            client.block_trackers = true;
        }
        if let Ok(mut guard) = client.user_agent.try_write() {
            *guard = self.user_agent.clone();
        }
        *client.platform.get_mut() = self.ua_platform.clone();
        client.allow_file_access = self.allow_file_access;

        let geolocation_permissions = if id == self.id { self.geolocation_permissions.clone() }
            else { Arc::new(obscura_js::geolocation::Permissions::default()) };
        let regional = if id == self.id { self.regional.clone() }
            else { crate::regional::context_region(self.proxy_url.clone(), self.stealth, self.allow_private_network) };
        if let Ok(region) = &regional { *client.accept_language.get_mut() = region.accept_language(); }
        BrowserContext {
            id,
            cookie_jar,
            // Network state is owned independently by every copy, and so is the
            // Web Storage view: the CDP server builds one connection context per
            // client from the immutable startup snapshot (server.rs connection
            // setup), so a persistent copy starts from the template's entries and
            // then stands alone instead of letting one client see another
            // client's writes. The dedicated persistence handle merges deltas.
            local_storage: if persistent {
                Arc::new(Mutex::new(self.local_storage.lock()
                    .expect("local storage namespace lock is poisoned")
                    .clone()))
            } else {
                Arc::new(Mutex::new(StorageNamespace::default()))
            },
            // IndexedDB is never written to the storage directory, so sharing it
            // across copies would let one identity read another identity's data
            // for free; every copy gets its own store instead, shared only by
            // this context's pages and native child windows.
            indexed_db: Arc::new(obscura_js::indexed_db::IndexedDbStorage::default()),
            http_client: Arc::new(client),
            regional,
            geolocation_permissions,
            user_agent: self.user_agent.clone(),
            platform: self.platform.clone(),
            ua_platform: self.ua_platform.clone(),
            ua_platform_version: self.ua_platform_version.clone(),
            proxy_url: self.proxy_url.clone(),
            robots_cache: Arc::new(RobotsCache::new()),
            obey_robots: self.obey_robots,
            stealth: self.stealth,
            allow_file_access: self.allow_file_access,
            storage_dir: persistent.then(|| self.storage_dir.clone()).flatten(),
            allow_private_network: self.allow_private_network,
        }
    }

    pub fn set_allow_file_access(&mut self, allow: bool) {
        self.allow_file_access = allow;
        let mut client = self.http_client.for_page();
        client.allow_file_access = allow;
        self.http_client = Arc::new(client);
    }

    /// Flush `localStorage` to `{storage_dir}/localStorage`. Only the origins
    /// that changed since the last successful write are touched, and an origin
    /// that lost its last entry has its file removed so a restart cannot
    /// resurrect it. The store is only marked clean once every touched origin
    /// reached the disk, so a failed write is retried instead of lost.
    pub fn save_local_storage(&self) -> std::io::Result<()> {
        let Some(dir) = &self.storage_dir else { return Ok(()) };
        let mut storage = self.local_storage.lock().expect("local storage namespace lock is poisoned");
        if !storage.is_dirty() { return Ok(()) }
        let root = dir.join("localStorage");
        obscura_net::profile::private_directory(dir)
            .and_then(|()| obscura_net::profile::private_directory(&root))
            .map_err(|error| profile_error("directory permissions", &root, error))?;
        let mut first_error = None;
        storage.flush_changed(|origin, entries| {
            let path = root.join(format!("{}.json", storage_file_stem(origin)));
            let result = if entries.is_empty() {
                obscura_net::profile::remove_file(&path)
            } else {
                let stored = StoredOriginStorage { origin: origin.to_string(), entries: entries.to_vec() };
                serde_json::to_vec(&stored).map_err(std::io::Error::other)
                    .and_then(|bytes| obscura_net::profile::write_atomically(&path, &bytes))
            };
            match result {
                Ok(()) => true,
                Err(error) => {
                    if first_error.is_none() { first_error = Some(profile_error("commit localStorage", &path, error)); }
                    false
                }
            }
        });
        first_error.map_or(Ok(()), Err)
    }

    /// Memory changes have already occurred when a commit fails. Successful
    /// units remain committed; failed origin units stay dirty for the next save.
    pub fn save_storage(&self) -> std::io::Result<()> {
        let local = self.save_local_storage();
        let cookies = self.save_cookies();
        local.and(cookies).map_err(|error| std::io::Error::new(error.kind(),
            format!("Browser memory was updated, but profile state was not fully saved: {error}")))
    }

    pub fn save_cookies(&self) -> std::io::Result<()> {
        let Some(dir) = &self.storage_dir else { return Ok(()) };
        obscura_net::profile::private_directory(dir)
            .map_err(|error| profile_error("directory permissions", dir, error))?;
        let path = dir.join("cookies.json");
        self.cookie_jar.save_to_file(&path)
            .map_err(|error| profile_error("commit cookies", &path, error))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn with_full_options_propagates_user_agent_to_http_client() {
        let ctx = BrowserContext::with_full_options(
            "test".to_string(),
            None,
            false,
            Some("Custom-UA/1.0".to_string()),
        );
        assert_eq!(ctx.user_agent, "Custom-UA/1.0");
        let client_ua = ctx.http_client.user_agent.read().await.clone();
        assert_eq!(client_ua, "Custom-UA/1.0");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn with_full_options_falls_back_to_chrome_default() {
        let ctx = BrowserContext::with_full_options(
            "test".to_string(),
            None,
            false,
            None,
        );
        assert!(ctx.user_agent.contains("Chrome"));
        let client_ua = ctx.http_client.user_agent.read().await.clone();
        assert!(client_ua.contains("Chrome"));
        assert_eq!(ctx.user_agent, client_ua);
    }

    /// Lock the context's Web Storage namespace for a test.
    fn ns(context: &BrowserContext) -> std::sync::MutexGuard<'_, StorageNamespace> {
        context.local_storage.lock().expect("local storage namespace lock is poisoned")
    }

    #[tokio::test(flavor = "current_thread")]
    async fn with_options_keeps_default_user_agent() {
        let ctx = BrowserContext::with_options("test".to_string(), None, false);
        assert!(ctx.user_agent.contains("Chrome"));
    }

    fn temp_storage_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "obscura-storage-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// --storage-dir has to survive a restart, which is the whole point of a
    /// profile: a second context over the same directory reads back what the
    /// first one wrote.
    #[test]
    fn local_storage_retries_failed_writes_without_rewriting_successes() {
        let dir = temp_storage_dir("write-retry");
        let context = BrowserContext::with_storage("write-retry".into(), Some(dir.clone())).unwrap();
        let root = dir.join("localStorage");
        std::fs::create_dir_all(&root).unwrap();
        let failed = root.join(format!("{}.json", storage_file_stem("https://failed.test")));
        let blocked = failed.clone();
        std::fs::create_dir(&blocked).unwrap();
        assert!(ns(&context).set("https://failed.test", "token", "kept").is_ok());
        assert!(ns(&context).set("https://saved.test", "token", "saved").is_ok());
        assert!(context.save_local_storage().is_err());
        assert!(ns(&context).is_dirty(), "failed writes must remain retryable");
        let saved = root.join(format!("{}.json", storage_file_stem("https://saved.test")));
        let saved_before = std::fs::metadata(&saved).unwrap().modified().unwrap();
        std::fs::remove_dir(&blocked).unwrap();
        context.save_local_storage().unwrap();
        assert!(!ns(&context).is_dirty());
        // Only the origin whose write failed is retried.
        assert_eq!(
            std::fs::metadata(&saved).unwrap().modified().unwrap(),
            saved_before,
            "a flushed origin must not be rewritten by a retry"
        );
        let reloaded = BrowserContext::with_storage("reloaded".into(), Some(dir.clone())).unwrap();
        assert_eq!(ns(&reloaded).get("https://failed.test", "token").as_deref(), Some("kept"));
        assert_eq!(ns(&reloaded).get("https://saved.test", "token").as_deref(), Some("saved"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn local_storage_rejects_non_regular_profile_files_without_blocking() {
        let dir = temp_storage_dir("non-regular");
        let root = dir.join("localStorage");
        std::fs::create_dir_all(&root).unwrap();
        let pipe = root.join("blocked.json");
        assert!(std::process::Command::new("mkfifo").arg(&pipe).status().unwrap().success());
        let (tx, rx) = std::sync::mpsc::channel();
        let profile = dir.clone();
        let reader = std::thread::spawn(move || {
            let result = BrowserContext::with_storage("reader".into(), Some(profile));
            tx.send(result.is_err()).unwrap();
        });
        let completed = rx.recv_timeout(std::time::Duration::from_secs(2));
        if completed.is_err() {
            // Release the broken reader before reporting the failure.
            std::fs::write(&pipe, b"{}").unwrap();
        }
        reader.join().unwrap();
        std::fs::remove_dir_all(dir).unwrap();
        assert_eq!(completed.ok(), Some(true), "invalid profile input must error without opening a FIFO");
    }

    #[test]
    fn local_storage_retries_failed_deletions() {
        let dir = temp_storage_dir("delete-retry");
        let context = BrowserContext::with_storage("delete-retry".into(), Some(dir.clone())).unwrap();
        assert!(ns(&context).set("https://app.test", "token", "old").is_ok());
        context.save_local_storage().unwrap();
        let path = dir.join("localStorage").join(format!("{}.json", storage_file_stem("https://app.test")));
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        ns(&context).clear("https://app.test");
        assert!(context.save_local_storage().is_err());
        assert!(ns(&context).is_dirty(), "a failed deletion must not be acknowledged");
        std::fs::remove_dir(&path).unwrap();
        context.save_local_storage().unwrap();
        assert!(!ns(&context).is_dirty());
        assert!(!path.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }


    #[test]
    fn local_storage_round_trips_through_the_storage_dir() {
        let dir = temp_storage_dir("roundtrip");
        let first = BrowserContext::with_storage("profile-a".into(), Some(dir.clone())).unwrap();
        assert!(ns(&first).set("https://app.example", "token", "abc").is_ok());
        assert!(ns(&first).set("https://app.example", "theme", "dark").is_ok());
        assert!(ns(&first).set("https://other.example", "k", "v").is_ok());
        first.save_local_storage().unwrap();

        let files: Vec<String> = std::fs::read_dir(dir.join("localStorage"))
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(files.len(), 2, "one file per origin: {files:?}");

        let second = BrowserContext::with_storage("profile-b".into(), Some(dir.clone())).unwrap();
        assert_eq!(
            ns(&second).get("https://app.example", "token"),
            Some("abc".to_string())
        );
        assert_eq!(
            ns(&second).get("https://app.example", "theme"),
            Some("dark".to_string())
        );
        assert_eq!(
            ns(&second).get("https://other.example", "k"),
            Some("v".to_string())
        );
        // Insertion order is part of the format: Storage.key(i) depends on it.
        let keys: Vec<String> = ns(&second).snapshot("https://app.example")
            .into_iter()
            .map(|(key, _)| key)
            .collect();
        assert_eq!(keys, vec!["token".to_string(), "theme".to_string()]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Without --storage-dir nothing is written and nothing is loaded, and the
    /// incognito copy of a persistent context stays empty.
    #[test]
    fn local_storage_stays_in_memory_without_a_storage_dir() {
        let persistent = BrowserContext::with_storage("no-dir".into(), None).unwrap();
        assert!(persistent.storage_dir.is_none());
        assert!(ns(&persistent).set("https://app.example", "k", "v").is_ok());
        persistent.save_local_storage().unwrap();
        // No directory is configured, so the write is a no-op and the entry
        // stays in this process only.
        assert!(ns(&persistent).is_dirty());
        assert_eq!(
            ns(&persistent).get("https://app.example", "k"),
            Some("v".to_string())
        );

        let incognito = persistent.isolated_copy("incognito".into(), false);
        assert_eq!(ns(&incognito).get("https://app.example", "k"), None);
        // The template keeps its own entry.
        assert_eq!(
            ns(&persistent).get("https://app.example", "k"),
            Some("v".to_string())
        );
    }

    /// An unchanged store must not touch the disk, or every navigation on a
    /// large profile would rewrite every origin file.
    #[test]
    fn save_local_storage_skips_untouched_origins() {
        let dir = temp_storage_dir("dirty");
        let context = BrowserContext::with_storage("dirty-check".into(), Some(dir.clone())).unwrap();
        assert!(ns(&context).set("https://app.example", "k", "v").is_ok());
        context.save_local_storage().unwrap();
        let first_write = std::fs::metadata(
            dir.join("localStorage").join(format!("{}.json", storage_file_stem("https://app.example"))),
        )
        .and_then(|meta| meta.modified())
        .expect("the first write must land");
        std::thread::sleep(std::time::Duration::from_millis(1100));
        context.save_local_storage().unwrap();
        let second_write = std::fs::metadata(
            dir.join("localStorage").join(format!("{}.json", storage_file_stem("https://app.example"))),
        )
        .and_then(|meta| meta.modified())
        .expect("the file must still exist");
        assert_eq!(
            first_write, second_write,
            "an unchanged store must not be rewritten"
        );

        // A write to one origin must leave another origin's file alone.
        assert!(ns(&context).set("https://other.example", "x", "y").is_ok());
        context.save_local_storage().unwrap();
        let other_path = dir.join("localStorage").join(format!(
            "{}.json",
            storage_file_stem("https://other.example")
        ));
        let other_first = std::fs::metadata(&other_path).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        assert!(ns(&context).set("https://app.example", "k2", "v2").is_ok());
        context.save_local_storage().unwrap();
        let other_second = std::fs::metadata(&other_path).unwrap().modified().unwrap();
        assert_eq!(other_first, other_second, "an untouched origin must not be rewritten");

        // Clearing an origin removes its file instead of leaving it behind for
        // the next load to resurrect.
        ns(&context).clear("https://other.example");
        context.save_local_storage().unwrap();
        assert!(!other_path.exists(), "a cleared origin must not keep its file");
        let after_clear = BrowserContext::with_storage("dirty-check-3".into(), Some(dir.clone())).unwrap();
        assert_eq!(ns(&after_clear).get("https://other.example", "x"), None);

        let reloaded = BrowserContext::with_storage("dirty-check-2".into(), Some(dir.clone())).unwrap();
        assert_eq!(
            ns(&reloaded).get("https://app.example", "k2"),
            Some("v2".to_string())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_profile_units_report_load_errors_without_overwriting() {
        let dir = temp_storage_dir("corrupt");
        let cookies = dir.join("cookies.json");
        std::fs::write(&cookies, b"not cookie JSON").unwrap();
        assert!(BrowserContext::with_storage("bad-cookies".into(), Some(dir.clone())).is_err());
        assert_eq!(std::fs::read(&cookies).unwrap(), b"not cookie JSON");
        std::fs::remove_file(cookies).unwrap();
        let root = dir.join("localStorage");
        std::fs::create_dir_all(&root).unwrap();
        let origin = root.join("corrupt.json");
        std::fs::write(&origin, b"{\"origin\":\"null\",\"entries\":[]}").unwrap();
        assert!(BrowserContext::with_storage("bad-origin".into(), Some(dir.clone())).is_err());
        assert_eq!(std::fs::read(&origin).unwrap(), b"{\"origin\":\"null\",\"entries\":[]}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn isolated_copy_does_not_share_mutable_network_state() {
        let source = BrowserContext::with_full_options(
            "source".to_string(),
            None,
            false,
            Some("Template-UA/1.0".to_string()),
        );
        source.cookie_jar.set_cookie("sid=source", &url::Url::parse("https://example.com").unwrap());

        let persistent = source.isolated_copy("persistent".to_string(), true);
        let incognito = source.isolated_copy("incognito".to_string(), false);

        assert_eq!(persistent.cookie_jar.get_all_cookies().len(), 1);
        assert!(incognito.cookie_jar.get_all_cookies().is_empty());
        assert!(persistent
            .cookie_jar
            .get_cookie_header(&url::Url::parse("https://sub.example.com").unwrap())
            .is_empty());
        persistent.cookie_jar.clear();
        persistent.http_client.set_user_agent("Changed-UA/2.0").await;

        assert_eq!(source.cookie_jar.get_all_cookies().len(), 1);
        assert_eq!(source.http_client.user_agent.read().await.as_str(), "Template-UA/1.0");
    }
}
