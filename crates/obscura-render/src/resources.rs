//! One bounded page byte cache for native font metadata and optional paint.
//! Raster state is present only with paint; metadata consumers retain the same
//! owned font bytes, decoding entries, cancellation, and FIFO accounting.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
#[cfg(feature = "paint")]
use crate::paint::{CachedRasterPaint, RememberedContentImageIntrinsic, HttpResourceLoader, DEFAULT_CONTENT_IMAGE_INTRINSIC_ENTRIES};
const DEFAULT_RESOURCE_CACHE_ENTRIES: usize = 512;
pub(crate) const DEFAULT_RESOURCE_CACHE_BYTES: usize = 64 * 1024 * 1024;
pub(crate) const MISSING_RESOURCE_RETRY_AFTER: std::time::Duration = std::time::Duration::from_secs(2);
#[cfg(feature = "paint")]
/// Synchronous byte loader used by [`RenderResourceCache`]. The default
/// implementation uses Obscura's pooled image agent; tests and embedding
/// callers can provide a local loader without changing preparation or paint.
pub trait RenderResourceLoader {
    fn load(&mut self, url: &str) -> Option<Vec<u8>>;
}
#[cfg(feature = "paint")]
impl<F> RenderResourceLoader for F
where
    F: FnMut(&str) -> Option<Vec<u8>>,
{
    fn load(&mut self, url: &str) -> Option<Vec<u8>> {
        self(url)
    }
}
pub(crate) enum CachedResource {
    Bytes {
        bytes: Arc<[u8]>,
        #[cfg(feature = "paint")]
        intrinsic: std::sync::OnceLock<Option<crate::ReplacedIntrinsic>>,
        #[cfg(feature = "paint")]
        static_raster: std::sync::OnceLock<bool>,
        decoded_font: Option<Arc<Vec<u8>>>,
    },
    Missing(std::time::Instant),
}
/// Fetch credentials/CORS identity for an HTML image request. No-CORS uses
/// the ordinary URL cache key so CSS images can share the same response;
/// CORS variants use private keys and can never contaminate one another.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ImageRequestProfile {
    NoCorsInclude,
    CorsSameOrigin,
    CorsInclude,
}
pub(crate) fn image_resource_key(url: &str, profile: ImageRequestProfile) -> String {
    let url = network_resource_url(url);
    match profile {
        ImageRequestProfile::NoCorsInclude => url,
        ImageRequestProfile::CorsSameOrigin => format!("\0obscura-img-cors\0{url}"),
        ImageRequestProfile::CorsInclude => format!("\0obscura-img-credentials\0{url}"),
    }
}

pub(crate) fn network_resource_url(url: &str) -> String {
    if url.starts_with('\0') || url.starts_with("data:") {
        return url.to_string();
    }
    let Ok(mut parsed) = url::Url::parse(url) else {
        return url.to_string();
    };
    parsed.set_fragment(None);
    parsed.to_string()
}
/// Page-scoped raw resource bytes shared by layout preparation and repeated
/// paints. Entries are FIFO-bounded by both count and retained byte size.
/// Successful bytes use `Arc` so consumers never clone an image/font body.
pub struct RenderResourceCache {
    pub(crate) entries: HashMap<String, CachedResource>,
    // Cancellation belongs to the document, not the retryable byte cache.
    pub(crate) stopped: HashSet<String>,
    order: VecDeque<String>,
    retained_bytes: usize,
    #[cfg(feature = "paint")]
    pub(crate) raster_paints: VecDeque<CachedRasterPaint>,
    #[cfg(feature = "paint")]
    pub(crate) raster_paint_bytes: usize,
    max_entries: usize,
    max_bytes: usize,
    #[cfg(feature = "paint")]
    pub(crate) content_image_intrinsics: HashMap<obscura_dom::tree::NodeId, RememberedContentImageIntrinsic>,
    #[cfg(feature = "paint")]
    pub(crate) content_image_intrinsic_order: VecDeque<obscura_dom::tree::NodeId>,
    #[cfg(feature = "paint")]
    pub(crate) max_content_image_intrinsics: usize,
    #[cfg(all(test, feature = "paint"))]
    pub(crate) content_image_layout_retries: usize,
    sync_loading_enabled: bool,
    sync_misses: Vec<(String, Option<ImageRequestProfile>, bool)>,
    sync_miss_keys: HashSet<String>,
    #[cfg(feature = "paint")]
    loader: Option<Box<dyn RenderResourceLoader>>,
}

impl Default for RenderResourceCache {
    fn default() -> Self {
        #[cfg(feature = "paint")]
        {
            Self::with_loader_and_limits(HttpResourceLoader, DEFAULT_RESOURCE_CACHE_ENTRIES, DEFAULT_RESOURCE_CACHE_BYTES)
        }
        #[cfg(not(feature = "paint"))]
        {
            Self::with_limits(DEFAULT_RESOURCE_CACHE_ENTRIES, DEFAULT_RESOURCE_CACHE_BYTES)
        }
    }
}

impl RenderResourceCache {
    #[cfg(feature = "paint")]
    pub fn with_loader(loader: impl RenderResourceLoader + 'static) -> Self {
        Self::with_loader_and_limits(
            loader,
            DEFAULT_RESOURCE_CACHE_ENTRIES,
            DEFAULT_RESOURCE_CACHE_BYTES,
        )
    }

    #[cfg(feature = "paint")]
    pub fn with_loader_and_limits(
        loader: impl RenderResourceLoader + 'static,
        max_entries: usize,
        max_bytes: usize,
    ) -> Self {
        let mut cache = Self::with_limits(max_entries, max_bytes);
        cache.loader = Some(Box::new(loader));
        cache.sync_loading_enabled = true;
        cache
    }

    fn with_limits(max_entries: usize, max_bytes: usize) -> Self {
        Self {
            entries: HashMap::new(),
            stopped: HashSet::new(),
            order: VecDeque::new(),
            retained_bytes: 0,
            #[cfg(feature = "paint")]
            raster_paints: VecDeque::new(),
            #[cfg(feature = "paint")]
            raster_paint_bytes: 0,
            max_entries,
            max_bytes,
            #[cfg(feature = "paint")]
            content_image_intrinsics: HashMap::new(),
            #[cfg(feature = "paint")]
            content_image_intrinsic_order: VecDeque::new(),
            #[cfg(feature = "paint")]
            max_content_image_intrinsics: max_entries.min(DEFAULT_CONTENT_IMAGE_INTRINSIC_ENTRIES),
            #[cfg(all(test, feature = "paint"))]
            content_image_layout_retries: 0,
            sync_loading_enabled: false,
            sync_misses: Vec::new(),
            sync_miss_keys: HashSet::new(),
            #[cfg(feature = "paint")]
            loader: None,
        }
    }

    /// Temporarily control the compatibility loader used by synchronous
    /// layout and paint. Capture callers disable it so a screenshot observes
    /// only bytes already prepared by the page transport. Unknown URLs remain
    /// unknown and can still be fetched by a later navigation/settle warmup.
    pub fn set_sync_loading_enabled(&mut self, enabled: bool) -> bool {
        std::mem::replace(&mut self.sync_loading_enabled, enabled)
    }

    /// Whether layout and paint may still open synchronous compatibility
    /// requests. Page-owned caches disable this permanently and are fed by
    /// the page transport instead.
    pub fn sync_loading_enabled(&self) -> bool {
        self.sync_loading_enabled
    }

    /// Take the resources that cache-only layout or paint asked for and did
    /// not have, exactly as they were resolved (network URL plus the image
    /// request profile, or `None` for CSS images and fonts). The owning page
    /// loads them through its asynchronous transport; nothing is
    /// reconstructed from the DOM, so script-registered fonts, shadow roots
    /// and every other renderer-only source are covered.
    pub fn take_sync_misses(&mut self, limit: usize) -> Vec<(String, Option<ImageRequestProfile>, bool)> {
        if limit >= self.sync_misses.len() {
            self.sync_miss_keys.clear();
            return std::mem::take(&mut self.sync_misses);
        }
        let misses: Vec<_> = self.sync_misses.drain(..limit).collect();
        for (url, profile, _) in &misses {
            let key = profile.map_or_else(|| url.clone(), |profile| image_resource_key(url, profile));
            self.sync_miss_keys.remove(&key);
        }
        misses
    }

    /// Whether any cache-only miss is waiting to be taken.
    pub fn has_sync_misses(&self) -> bool {
        !self.sync_misses.is_empty()
    }

    #[cfg(feature = "paint")]
    fn record_sync_miss(
        &mut self,
        key: String,
        url: String,
        profile: Option<ImageRequestProfile>,
        is_font: bool,
    ) {
        // Bound a hostile document's per-layout queue to the number of
        // entries this cache could retain. A later layout reports skipped
        // misses again after the current batch has been drained.
        if self.sync_misses.len() >= self.max_entries {
            return;
        }
        if self.sync_miss_keys.insert(key) {
            self.sync_misses.push((url, profile, is_font));
        }
    }

    pub fn retained_entry_count(&self) -> usize {
        self.entries.len()
    }

    pub fn retained_byte_len(&self) -> usize {
        self.retained_bytes
    }

    pub fn has_live_outcome(&self, url: &str) -> bool {
        let key = network_resource_url(url);
        if self.stopped.contains(&key) { return true; }
        match self.entries.get(&key) {
            Some(CachedResource::Bytes { .. }) => true,
            Some(CachedResource::Missing(at)) => at.elapsed() < MISSING_RESOURCE_RETRY_AFTER,
            None => false,
        }
    }

    /// Whether this URL currently retains usable bytes (as opposed to a
    /// short-lived negative-cache entry). Profile-specific HTML image loads
    /// use this to avoid replacing paint bytes from another credential mode.
    pub fn has_cached_bytes(&self, url: &str) -> bool {
        matches!(
            self.entries.get(&network_resource_url(url)),
            Some(CachedResource::Bytes { .. })
        )
    }

    pub fn has_live_image_outcome(&self, url: &str, profile: ImageRequestProfile) -> bool {
        self.has_live_outcome(&image_resource_key(url, profile))
    }

    pub fn seed_image(&mut self, url: String, profile: ImageRequestProfile, bytes: Vec<u8>) {
        self.seed_shared(image_resource_key(&url, profile), Arc::from(bytes));
    }

    pub fn seed_image_shared(
        &mut self,
        url: String,
        profile: ImageRequestProfile,
        bytes: Arc<[u8]>,
    ) {
        self.seed_shared(image_resource_key(&url, profile), bytes);
    }

    pub fn seed_image_missing(&mut self, url: String, profile: ImageRequestProfile) {
        self.seed_missing(image_resource_key(&url, profile));
    }

    /// Seed bytes fetched by the owning page's asynchronous browser transport.
    ///
    /// Layout and paint are synchronous, so letting them open their own HTTP
    /// requests serializes every image/font behind the capture call.  The page
    /// layer uses this entry point to fetch a bounded resource batch through
    /// its cookie/proxy/CORS-aware connection pool before entering layout.
    pub fn seed(&mut self, url: String, bytes: Vec<u8>) {
        self.seed_shared(url, Arc::from(bytes));
    }

    /// Seed already-shared bytes without copying a complete response body.
    pub fn seed_shared(&mut self, url: String, bytes: Arc<[u8]>) {
        let url = network_resource_url(&url);
        self.stopped.remove(&url);
        self.remove(&url);
        self.insert_bytes(url, bytes);
    }

    /// Keep a canceled request stopped until this document's cache is replaced.
    pub fn stop_resource(&mut self, url: &str, profile: Option<ImageRequestProfile>) {
        let key = match profile {
            Some(profile) => image_resource_key(url, profile),
            None => network_resource_url(url),
        };
        if self.has_cached_bytes(&key) { return; }
        self.remove(&key);
        self.stopped.insert(key);
    }

    /// Retain a page-transport failure so capture does not immediately repeat
    /// the same slow request through the renderer's compatibility loader.
    pub fn seed_missing(&mut self, url: String) {
        let url = network_resource_url(&url);
        self.remove(&url);
        self.insert_missing(url);
    }

    #[cfg(feature = "paint")]
    pub(crate) fn get_or_load(&mut self, url: &str, is_font: bool) -> Option<Arc<[u8]>> {
        let url = network_resource_url(url);
        if self.stopped.contains(&url) { return None; }
        if let Some(entry) = self.entries.get(&url) {
            match entry {
                CachedResource::Bytes { bytes, .. } => return Some(Arc::clone(bytes)),
                CachedResource::Missing(at) if at.elapsed() < MISSING_RESOURCE_RETRY_AFTER => {
                    return None;
                }
                CachedResource::Missing(_) => {}
            }
        }
        if !self.sync_loading_enabled {
            self.record_sync_miss(url.clone(), url, None, is_font);
            return None;
        }
        self.remove(&url);

        let loaded = self.loader.as_mut()?.load(&url).map(Arc::<[u8]>::from);
        match loaded {
            Some(bytes) => {
                self.insert_bytes(url, Arc::clone(&bytes));
                Some(bytes)
            }
            None => {
                self.insert_missing(url);
                None
            }
        }
    }

    #[cfg(feature = "paint")]
    pub(crate) fn get_or_load_image(&mut self, url: &str, profile: ImageRequestProfile) -> Option<Arc<[u8]>> {
        if profile == ImageRequestProfile::NoCorsInclude {
            return self.get_or_load(url, false);
        }
        let key = image_resource_key(url, profile);
        if self.stopped.contains(&key) { return None; }
        if let Some(entry) = self.entries.get(&key) {
            match entry {
                CachedResource::Bytes { bytes, .. } => return Some(Arc::clone(bytes)),
                CachedResource::Missing(at) if at.elapsed() < MISSING_RESOURCE_RETRY_AFTER => {
                    return None;
                }
                CachedResource::Missing(_) => {}
            }
        }
        if !self.sync_loading_enabled {
            self.record_sync_miss(key, network_resource_url(url), Some(profile), false);
            return None;
        }
        self.remove(&key);
        // The private profile key is cache identity only and must never escape
        // into a network loader.
        let loaded = self
            .loader.as_mut()?
            .load(&network_resource_url(url))
            .map(Arc::<[u8]>::from);
        match loaded {
            Some(bytes) => {
                self.insert_bytes(key, Arc::clone(&bytes));
                Some(bytes)
            }
            None => {
                self.insert_missing(key);
                None
            }
        }
    }

    fn insert_bytes(&mut self, url: String, bytes: Arc<[u8]>) {
        if self.max_entries == 0 || bytes.len() > self.max_bytes {
            return;
        }
        while self.entries.len() >= self.max_entries
            || self.retained_bytes.saturating_add(bytes.len()) > self.max_bytes
        {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            self.remove_entry(&oldest);
        }
        self.retained_bytes = self.retained_bytes.saturating_add(bytes.len());
        self.order.push_back(url.clone());
        self.entries.insert(url, CachedResource::Bytes {
            bytes,
            #[cfg(feature = "paint")]
            intrinsic: std::sync::OnceLock::new(),
            #[cfg(feature = "paint")]
            static_raster: std::sync::OnceLock::new(),
            decoded_font: None,
        });
    }

    fn insert_missing(&mut self, url: String) {
        if self.max_entries == 0 {
            return;
        }
        while self.entries.len() >= self.max_entries {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            self.remove_entry(&oldest);
        }
        self.order.push_back(url.clone());
        self.entries
            .insert(url, CachedResource::Missing(std::time::Instant::now()));
    }

    fn remove(&mut self, url: &str) {
        if self.entries.contains_key(url) {
            self.order.retain(|key| key != url);
            self.remove_entry(url);
        }
    }

    fn remove_entry(&mut self, url: &str) {
        if let Some(CachedResource::Bytes { bytes, decoded_font, .. }) = self.entries.remove(url) {
            self.retained_bytes = self.retained_bytes.saturating_sub(bytes.len()
                + decoded_font.as_ref().map_or(0, |font| font.len()));
        }
        #[cfg(feature = "paint")]
        self.raster_paints.retain(|entry| {
            if entry.resource_key == url {
                self.raster_paint_bytes -= entry.raster.data().len();
                false
            } else {
                true
            }
        });
    }
}

impl RenderResourceCache {
    /// Lifecycle consumers only inspect retained page-transport bytes here;
    /// this lookup never opens the renderer's compatibility HTTP loader.
    pub fn cached_font_bytes(&self, url: &str) -> Option<Arc<[u8]>> {
        match self.entries.get(&network_resource_url(url))? {
            CachedResource::Bytes { bytes, .. } => Some(Arc::clone(bytes)),
            CachedResource::Missing(_) => None,
        }
    }

    pub fn cached_decoded_font(&self, url: &str) -> Option<Arc<Vec<u8>>> {
        match self.entries.get(&network_resource_url(url))? {
            CachedResource::Bytes { decoded_font, .. } => decoded_font.clone(),
            CachedResource::Missing(_) => None,
        }
    }

    /// Decoded font bodies count against the existing page cache byte budget.
    /// The caller still owns the font if this cache cannot retain both forms.
    pub fn cache_decoded_font(&mut self, url: &str, font: Arc<Vec<u8>>) {
        let key = network_resource_url(url);
        let Some(CachedResource::Bytes { bytes, decoded_font, .. }) = self.entries.get(&key) else { return };
        if decoded_font.is_some() || bytes.len().saturating_add(font.len()) > self.max_bytes { return; }
        while self.retained_bytes.saturating_add(font.len()) > self.max_bytes {
            let Some(index) = self.order.iter().position(|entry| entry != &key) else { return };
            let Some(oldest) = self.order.remove(index) else { return };
            self.remove_entry(&oldest);
        }
        if let Some(CachedResource::Bytes { decoded_font, .. }) = self.entries.get_mut(&key) {
            self.retained_bytes += font.len();
            *decoded_font = Some(font);
        }
    }
}
