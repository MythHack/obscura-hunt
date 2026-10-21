#[cfg(feature = "stealth")]
use std::collections::HashMap;
#[cfg(feature = "stealth")]
use std::error::Error;
#[cfg(feature = "stealth")]
use std::sync::Arc;
#[cfg(feature = "stealth")]
use std::time::Duration;

#[cfg(feature = "stealth")]
use futures_util::StreamExt;
#[cfg(feature = "stealth")]
use tokio::sync::RwLock;
#[cfg(feature = "stealth")]
use url::Url;

#[cfg(feature = "stealth")]
use crate::client::{
    cors_required, env_allows_private_network, error_chain, fetch_file_url,
    redirect_taints_origin, request_referrer, response_too_large,
    same_site_context, serialized_request_origin, validate_cors_response, validate_request_mode,
    CallbackRegistry, InFlightGuard, ObscuraNetError, RequestInfo, RequestMode,
    ResourceRequest, Response, SsrfGuardResolver,
};
#[cfg(feature = "stealth")]
use crate::cookies::{CookieJar, SameSiteContext};

/// The wreq half of [`SsrfGuardResolver`]. `validate_url` only inspects the
/// host *string*, so on its own it lets a public name that resolves inward
/// straight through — the reqwest client closes that with a `dns_resolver`, and
/// until this impl existed the stealth client did not, which made `--stealth` a
/// downgrade in protection rather than only a change of fingerprint. The
/// deny-set is shared, so the two transports cannot drift apart.
#[cfg(feature = "stealth")]
impl wreq::dns::Resolve for SsrfGuardResolver {
    fn resolve(&self, name: wreq::dns::Name) -> wreq::dns::Resolving {
        let allow = self.allow_private || env_allows_private_network();
        let networks = self.allow_network.clone();
        let host = name.as_str().to_string();
        Box::pin(async move {
            let addrs: Vec<std::net::SocketAddr> = tokio::net::lookup_host((host.as_str(), 0))
                .await
                .map_err(|e| -> Box<dyn Error + Send + Sync> { Box::new(e) })?
                .collect();
            if !allow {
                if let Some(bad) = addrs.iter().find(|sa| !obscura_ssrf::address_allowed(sa.ip(), allow, &networks)) {
                    return Err(format!(
                        "SSRF blocked: '{}' resolves to forbidden address {}",
                        host,
                        bad.ip()
                    )
                    .into());
                }
            }
            let iter: wreq::dns::Addrs = Box::new(addrs.into_iter());
            Ok(iter)
        })
    }
}

#[cfg(feature = "stealth")]
pub const STEALTH_USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/145.0.0.0 Safari/537.36";

// The wreq emulation (Profile::Chrome145, Platform::Windows) sends this exact
// UA and sec-ch-ua-platform "Windows" on the wire. navigator has to report the
// same identity, otherwise the TLS/HTTP layer and the JS layer disagree and a
// site cross-checks the mismatch as a bot signal.
#[cfg(feature = "stealth")]
pub const STEALTH_NAVIGATOR_PLATFORM: &str = "Win32";
#[cfg(feature = "stealth")]
pub const STEALTH_UA_PLATFORM: &str = "Windows";
#[cfg(feature = "stealth")]
pub const STEALTH_UA_PLATFORM_VERSION: &str = "15.0.0";

#[cfg(feature = "stealth")]
fn tracker_blocking_enabled(value: Option<&str>) -> bool {
    !matches!(
        value.map(str::trim),
        Some(value) if matches!(value.to_ascii_lowercase().as_str(), "0" | "false" | "no" | "off")
    )
}

#[cfg(feature = "stealth")]
fn is_tracker_blocked(url: &Url, block_trackers: bool) -> bool {
    block_trackers && url.host_str().is_some_and(crate::blocklist::is_blocked)
}

#[cfg(feature = "stealth")]
fn wreq_response_header_value<'a>(
    headers: &'a wreq::header::HeaderMap,
    name: &'static str,
    url: &Url,
) -> Result<Option<&'a str>, ObscuraNetError> {
    let mut values = headers.get_all(name).iter();
    let Some(first) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err(ObscuraNetError::Cors(format!(
            "{} returned multiple {} headers",
            url, name
        )));
    }
    first.to_str().map(Some).map_err(|_| {
        ObscuraNetError::Cors(format!("{} returned an invalid {} header", url, name))
    })
}

#[cfg(feature = "stealth")]
fn validate_wreq_cors_response(
    request: &ResourceRequest,
    target: &Url,
    serialized_origin: &str,
    headers: &wreq::header::HeaderMap,
) -> Result<(), ObscuraNetError> {
    if !cors_required(request, target) {
        return Ok(());
    }
    let allow_origin =
        wreq_response_header_value(headers, "access-control-allow-origin", target)?;
    let allow_credentials =
        wreq_response_header_value(headers, "access-control-allow-credentials", target)?;
    validate_cors_response(
        request,
        target,
        serialized_origin,
        allow_origin,
        allow_credentials,
    )
}

#[cfg(feature = "stealth")]
async fn read_wreq_body_limited(
    response: wreq::Response,
    url: &Url,
    limit: usize,
) -> Result<Vec<u8>, ObscuraNetError> {
    if response
        .headers()
        .get("content-length")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .is_some_and(|length| length > limit as u64)
    {
        return Err(response_too_large(url, limit));
    }

    let capacity = response
        .content_length()
        .and_then(|length| usize::try_from(length).ok())
        .unwrap_or(0)
        .min(limit);
    let stream = response.bytes_stream();
    futures_util::pin_mut!(stream);
    let mut body = Vec::with_capacity(capacity);
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| {
            ObscuraNetError::Network(format!("Failed to read body: {}", error_chain(&error)))
        })?;
        if chunk.len() > limit.saturating_sub(body.len()) {
            return Err(response_too_large(url, limit));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}


#[cfg(feature = "stealth")]
pub struct StealthHttpClient {
    client: wreq::Client,
    pub allow_private_network: bool,
    proxy_url: Option<String>,
    pub allow_network: Vec<crate::client::NetworkRange>,
    pub allow_file_access: bool,
    pub request_interception: std::sync::RwLock<crate::RequestInterception>,
    pub block_trackers: bool,
    pub cookie_jar: Arc<CookieJar>,
    pub user_agent: RwLock<String>,
    pub accept_language: RwLock<String>,
    pub platform: RwLock<String>,
    pub extra_headers: RwLock<HashMap<String, String>>,
    pub in_flight: Arc<std::sync::atomic::AtomicU32>,
}

/// Scripted fetch exposes these headers before consuming the bounded body.
#[cfg(feature = "stealth")]
pub struct StealthResponseHeaders {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: futures_util::future::BoxFuture<'static, Result<Vec<u8>, ObscuraNetError>>,
}

#[cfg(feature = "stealth")]
impl StealthHttpClient {
    pub fn new(cookie_jar: Arc<CookieJar>) -> Self {
        Self::with_proxy(cookie_jar, None, false)
    }

    pub fn with_proxy(
        cookie_jar: Arc<CookieJar>,
        proxy_url: Option<&str>,
        allow_private_network: bool,
    ) -> Self {
        Self::with_network_policy(cookie_jar, proxy_url, allow_private_network,
            obscura_ssrf::env_allowed_networks().expect("Invalid OBSCURA_ALLOW_NETWORK"), false)
    }

    pub fn with_network_policy(cookie_jar: Arc<CookieJar>, proxy_url: Option<&str>,
        allow_private_network: bool, allow_network: Vec<crate::client::NetworkRange>, allow_file_access: bool) -> Self {
        let _network_runtime = crate::network_io_handle().enter();
        let emulation_opts = wreq_util::Emulation::builder()
            .profile(wreq_util::Profile::Chrome145)
            .platform(wreq_util::Platform::Windows)
            .build();

        let mut builder = wreq::Client::builder()
            .emulation(emulation_opts)
            // Read a folded header line (obs-fold) as Chrome does instead of
            // failing the response. Set after `emulation()`, which overwrites
            // `http1_options`; the Chrome profile leaves them unset.
            .http1_options(
                wreq::http1::Http1Options::builder()
                    .allow_obsolete_multiline_headers_in_responses(true)
                    .build(),
            )
            .timeout(Duration::from_secs(30))
            // SSRF guard: reject hostnames that resolve to a private/loopback
            // IP. Use the same opt-in as the `validate_url` calls below so
            // `--allow-private-network` reaches this transport (#793); the
            // resolver still honours OBSCURA_ALLOW_PRIVATE_NETWORK on its own.
            .dns_resolver(Arc::new(SsrfGuardResolver::with_networks(allow_private_network, allow_network.clone())))
            .redirect(wreq::redirect::Policy::none());
        builder = builder.no_proxy();

        builder = crate::aia::configure_wreq(builder, crate::aia::AiaPolicy {
            proxy_url: proxy_url.map(str::to_owned), allow_private_network,
            allow_network: allow_network.clone(),
        }).expect("failed to configure certificate verifier");
        if crate::client::custom_cert_store_requested(std::env::var_os("SSL_CERT_FILE").as_deref(), std::env::var_os("SSL_CERT_DIR").as_deref()) {
            builder = builder.tls_cert_store(wreq::tls::trust::CertStore::builder().set_default_paths().build()
                .expect("failed to build configured certificate store"));
        }

        if let Some(proxy) = proxy_url {
            if let Ok(p) = wreq::Proxy::all(proxy) {
                builder = builder.proxy(p);
            }
        }

        let client = builder.build().expect("failed to build wreq stealth client");

        StealthHttpClient {
            client,
            proxy_url: proxy_url.map(str::to_owned),
            allow_private_network,
            allow_network, allow_file_access,
            request_interception: std::sync::RwLock::new(crate::RequestInterception::default()),
            platform: RwLock::new(String::new()),
            block_trackers: tracker_blocking_enabled(
                std::env::var("OBSCURA_BLOCK_TRACKERS").ok().as_deref(),
            ),
            cookie_jar,
            user_agent: RwLock::new(STEALTH_USER_AGENT.to_string()),
            accept_language: RwLock::new("en-US,en;q=0.9".to_string()),
            extra_headers: RwLock::new(HashMap::new()),
            in_flight: Arc::new(std::sync::atomic::AtomicU32::new(0)),
        }
    }

    pub fn for_page(&self) -> Self {
        Self {
            client: self.client.clone(), allow_private_network: self.allow_private_network,
            proxy_url: self.proxy_url.clone(),
            block_trackers: self.block_trackers, cookie_jar: self.cookie_jar.clone(),
            user_agent: RwLock::new(self.user_agent.try_read().expect("identity is being updated").clone()),
            accept_language: RwLock::new(self.accept_language.try_read().expect("identity is being updated").clone()),
            extra_headers: RwLock::new(self.extra_headers.try_read().expect("headers are being updated").clone()),
            in_flight: self.in_flight.clone(),
            allow_network: self.allow_network.clone(), allow_file_access: self.allow_file_access,
            request_interception: std::sync::RwLock::new(self.request_interception.read().unwrap().clone()),
            platform: RwLock::new(self.platform.try_read().expect("identity is being updated").clone()),
        }
    }

    /// Keep page sockets on the selected BoringSSL/proxy/DNS transport.
    pub async fn websocket(&self, request: &crate::websocket::WebSocketRequest)
        -> Result<crate::websocket::WebSocketConnection, ObscuraNetError>
    {
        let url = request.http_url()?;
        self.validate_destination(&url).await?;
        if is_tracker_blocked(&url, self.block_trackers) {
            return Err(crate::websocket::network_error("WebSocket target is blocked"));
        }
        let extra = self.extra_headers.read().await;
        let (key, headers) = request.headers(&extra, &url);
        let mut outgoing = self.client.get(url.as_str()).version(wreq::Version::HTTP_11);
        if !extra.keys().any(|name| name.eq_ignore_ascii_case("user-agent")) {
            outgoing = outgoing.header("user-agent", self.user_agent.read().await.as_str());
        }
        if !extra.keys().any(|name| name.eq_ignore_ascii_case("accept-language")) {
            outgoing = outgoing.header("accept-language", self.accept_language.read().await.as_str());
        }
        drop(extra);
        for (name, value) in headers { outgoing = outgoing.header(name, value); }
        let cookies = self.cookie_jar.get_cookie_header_in_context(&url, request.cookie_context(&url));
        if !cookies.is_empty() { outgoing = outgoing.header("cookie", cookies); }
        let response = outgoing.send().await.map_err(|error| crate::websocket::network_error(error_chain(&error)))?;
        let protocol = request.validate_response(response.status().as_u16(), &key,
            response.headers().iter().map(|(name, value)| (name.as_str(), value.as_bytes())))?;
        for value in response.headers().get_all("set-cookie") {
            if let Ok(value) = value.to_str() { self.cookie_jar.set_cookie(value, &url); }
        }
        let upgraded = response.upgrade().await.map_err(|error| crate::websocket::network_error(error_chain(&error)))?;
        Ok(crate::websocket::WebSocketConnection::upgraded(upgraded, protocol).await)
    }

    pub async fn fetch(&self, url: &Url) -> Result<Response, ObscuraNetError> {
        self.fetch_with_callbacks(url, None).await
    }

    pub async fn fetch_with_callbacks(
        &self,
        url: &Url,
        callbacks: Option<&CallbackRegistry>,
    ) -> Result<Response, ObscuraNetError> {
        self.fetch_with_profile(url, ResourceRequest::navigation(), callbacks)
            .await
    }

    pub async fn fetch_resource_with_callbacks(
        &self,
        url: &Url,
        request: ResourceRequest,
        callbacks: Option<&CallbackRegistry>,
    ) -> Result<Response, ObscuraNetError> {
        self.fetch_with_profile(url, request, callbacks).await
    }

    async fn fetch_with_profile(&self, url: &Url, request: ResourceRequest,
        callbacks: Option<&CallbackRegistry>) -> Result<Response, ObscuraNetError> {
        self.fetch_with_method("GET", url, None, callbacks, request).await
    }

    pub fn validate_url(&self, url: &Url) -> Result<(), ObscuraNetError> {
        crate::client::validate_url_with_networks(url, self.allow_private_network, &self.allow_network)
    }

    pub fn proxy_url(&self) -> Option<&str> { self.proxy_url.as_deref() }

    pub async fn validate_destination(&self, url: &Url) -> Result<(), ObscuraNetError> {
        self.validate_url(url)?;
        if self.proxy_url.is_some() { crate::client::validate_proxy_destination(url, self.allow_private_network, &self.allow_network).await?; }
        Ok(())
    }

    pub async fn fetch_with_method(&self, method: &str, url: &Url, mut body: Option<Vec<u8>>,
        callbacks: Option<&CallbackRegistry>, mut request: ResourceRequest) -> Result<Response, ObscuraNetError> {
        self.validate_url(url)?;
        validate_request_mode(&request, url)?;
        crate::client::validate_file_request(url, &request, self.allow_file_access)?;
        let mut current_url = url.clone();
        let mut method = method.to_string();
        let mut redirects = Vec::new();
        let mut tainted = false;
        let mut strip_credentials = false;
        for _ in 0..=20 {
            self.validate_url(&current_url)?;
            validate_request_mode(&request, &current_url)?;
            if is_tracker_blocked(&current_url, self.block_trackers) {
                return Err(ObscuraNetError::Blocked(current_url.to_string()));
            }
            let mut headers: HashMap<String, String> = self.extra_headers.read().await.iter()
                .map(|(name, value)| (name.to_ascii_lowercase(), value.clone())).collect();
            headers.retain(|name, _| !name.eq_ignore_ascii_case("referer")
                && !name.eq_ignore_ascii_case("origin")
                && !(strip_credentials && crate::client::is_credential_header(name)));
            let ua = self.user_agent.read().await.clone();
            headers.entry("user-agent".into()).or_insert(ua.clone());
            headers.entry("accept-language".into()).or_insert(self.accept_language.read().await.clone());
            let (brands, derived_platform) = crate::client::chrome_client_hints(&ua);
            let platform = self.platform.read().await.clone();
            headers.insert("sec-ch-ua".into(), brands);
            headers.insert("sec-ch-ua-platform".into(), if platform.is_empty() { derived_platform } else { format!("{:?}", platform) });
            headers.insert("sec-ch-ua-mobile".into(), "?0".into());
            headers.insert("accept".into(), request.accept().into());
            headers.insert("sec-fetch-mode".into(), request.mode.header_value().into());
            headers.insert("sec-fetch-site".into(), crate::client::request_fetch_site_with_redirects(&request, &current_url, &redirects).into());
            headers.insert("sec-fetch-dest".into(), request.destination().into());
            if request.mode == RequestMode::Navigate { headers.insert("upgrade-insecure-requests".into(), "1".into()); }
            let origin = serialized_request_origin(&request, tainted);
            if cors_required(&request, &current_url) || !matches!(method.as_str(), "GET" | "HEAD") {
                headers.insert("origin".into(), origin.clone());
            }
            if let Some(referer) = request_referrer(&request, &current_url) { headers.insert("referer".into(), referer); }
            if request.sends_credentials_to(&current_url) {
                let cookie = self.cookie_jar.get_cookie_header_in_context(&current_url,
                    same_site_context(&request, &current_url, matches!(method.as_str(), "GET" | "HEAD")));
                if !cookie.is_empty() { headers.insert("cookie".into(), cookie); }
            } else { headers.retain(|name, _| !crate::client::is_credential_header(name)); }
            if method == "POST" && body.is_some() {
                headers.entry("content-type".into()).or_insert("application/x-www-form-urlencoded".into());
            }
            let mut info = RequestInfo::new(current_url.clone(), method.clone(), headers.clone(), request.resource_type);
            let interception = self.request_interception.read().unwrap().clone();
            let old_url = current_url.clone();
            info.body = body.clone(); info.page_id = interception.page_id.clone();
            info.frame_id = request.frame_id; info.parent_frame_id = request.parent_frame_id; info.frame_generation = request.frame_generation;
            if let Some(response) = interception.resolve(&info.request_id, &mut current_url, &mut method,
                &mut headers, &mut body, &request).await? {
                validate_cors_response(&request, &current_url, &origin,
                    response.header("access-control-allow-origin"), response.header("access-control-allow-credentials"))?;
                if let Some(callbacks) = callbacks { callbacks.fire_response(&info, &response).await; }
                return Ok(response);
            }
            self.validate_url(&current_url)?;
            validate_request_mode(&request, &current_url)?;
            if old_url.scheme() != "file" && current_url.scheme() == "file" {
                return Err(ObscuraNetError::Network("Network interception cannot access file URLs".into()));
            }
            if old_url.origin() != current_url.origin() { headers.retain(|name, _| !crate::client::is_credential_header(name)); }
            if old_url != current_url {
                crate::client::apply_request_metadata(&mut headers, &request, &current_url, &redirects, &ua, &platform);
                headers.remove("referer");
                if let Some(referer) = request_referrer(&request, &current_url) { headers.insert("referer".into(), referer); }
            }
            info.url = current_url.clone(); info.method = method.clone(); info.headers = headers.clone();
            info.body = body.clone(); info.page_id = interception.page_id;
            info.frame_id = request.frame_id; info.parent_frame_id = request.parent_frame_id; info.frame_generation = request.frame_generation;
            if let Some(callbacks) = callbacks { callbacks.fire_request(&info).await; }
            if current_url.scheme() == "file" {
                crate::client::validate_file_request(&current_url, &request, self.allow_file_access)?;
                let response = fetch_file_url(&current_url, request.max_response_bytes, self.allow_file_access).await?;
                if let Some(callbacks) = callbacks { callbacks.fire_response(&info, &response).await; }
                return Ok(response);
            }
            self.validate_destination(&current_url).await?;
            let req_method = method.parse::<wreq::Method>()
                .map_err(|error| ObscuraNetError::Network(format!("invalid method '{}': {}", method, error)))?;
            let mut outgoing = self.client.request(req_method, current_url.as_str());
            for (name, value) in &headers {
                outgoing = outgoing.header(name.as_str(), value.as_str());
            }
            if let Some(bytes) = body.as_ref().filter(|bytes| !bytes.is_empty()) {
                outgoing = outgoing.body(bytes.clone());
            }
            let in_flight = InFlightGuard::new(&self.in_flight);
            let raw_response = crate::on_network_io(async move { outgoing.send().await }).await
                .map_err(ObscuraNetError::Network)?
                .map_err(|error| ObscuraNetError::Network(error_chain(&error)))?;
            // Check the raw header list before duplicate fields are normalized,
            // cookies are stored, or the response body is consumed.
            validate_wreq_cors_response(&request, &current_url, &origin, raw_response.headers())?;
            let status = raw_response.status().as_u16();
            if request.sends_credentials_to(&current_url) {
                for value in raw_response.headers().get_all("set-cookie") {
                    if let Ok(value) = value.to_str() {
                        self.cookie_jar.set_cookie(value, &current_url);
                    }
                }
            }
            let mut response_headers: HashMap<String, String> = HashMap::new();
            for (name, value) in raw_response.headers().iter() {
                let name = name.as_str();
                let value = crate::client::decode_response_header_value(value.as_bytes());
                if matches!(name, "content-security-policy" | "x-frame-options") {
                    response_headers.entry(name.to_string())
                        .and_modify(|existing| {
                            existing.push_str(", ");
                            existing.push_str(&value);
                        })
                        .or_insert(value);
                } else {
                    response_headers.insert(name.to_string(), value);
                }
            }
            let body_url = current_url.clone();
            let response_body = crate::on_network_io(async move {
                let _in_flight = in_flight;
                read_wreq_body_limited(raw_response, &body_url, 64 * 1024 * 1024).await
            }).await.map_err(ObscuraNetError::Network)??;
            let response = Response {
                url: current_url.clone(), status, headers: response_headers,
                body: response_body, redirected_from: Vec::new(),
            };
            if let Some(callbacks) = callbacks { callbacks.fire_response(&info, &response).await; }
            if matches!(response.status, 301 | 302 | 303 | 307 | 308) {
                if let Some(location) = response.header("location") {
                    let next = current_url.join(location).map_err(|error| ObscuraNetError::Network(error.to_string()))?;
                    self.validate_url(&next)?;
                    validate_request_mode(&request, &next)?;
                    if next.scheme() == "file" { return Err(ObscuraNetError::Network("Network redirects cannot access file URLs".into())); }
                    tainted |= redirect_taints_origin(&request, &current_url, &next);
                    strip_credentials |= current_url.origin() != next.origin();
                    if let Some(policy) = response.header("referrer-policy").and_then(crate::client::parse_referrer_policy) { request.referrer_policy = policy.into(); }
                    if ((response.status == 301 || response.status == 302) && method == "POST")
                        || (response.status == 303 && method != "HEAD") { method = "GET".into(); body = None; }
                    redirects.push(current_url); current_url = next;
                    continue;
                }
            }
            return Ok(Response { redirected_from: redirects, ..response });
        }
        Err(ObscuraNetError::TooManyRedirects(current_url.to_string()))
    }

    /// One request with no redirect following, for scripted fetch()/XHR. The
    /// caller supplies the Fetch credentials decision for this redirect hop,
    /// while this method preserves the Chrome transport fingerprint.
    pub async fn send_single(
        &self,
        method: &str,
        url: &Url,
        headers: &HashMap<String, String>,
        body: &[u8],
        send_cookies: bool,
        store_cookies: bool,
    ) -> Result<Response, ObscuraNetError> {
        self.send_single_with_context(
            method,
            url,
            headers,
            body,
            send_cookies.then_some(SameSiteContext::SameSite),
            store_cookies,
        )
        .await
    }

    /// One request with an explicit cookie site context. Scripted fetch/XHR
    /// uses this so Strict and Lax cookies cannot leak cross-site.
    pub async fn send_single_with_context(
        &self,
        method: &str,
        url: &Url,
        headers: &HashMap<String, String>,
        body: &[u8],
        cookie_context: Option<SameSiteContext>,
        store_cookies: bool,
    ) -> Result<Response, ObscuraNetError> {
        let response = self.send_single_headers_with_context(
            method, url, headers, body, cookie_context, store_cookies, 64 * 1024 * 1024,
        ).await?;
        Ok(Response {
            url: url.clone(), status: response.status, headers: response.headers,
            body: response.body.await?, redirected_from: Vec::new(),
        })
    }

    /// Preserve request policy and cookie handling while deferring body reads.
    pub async fn send_single_headers_with_context(
        &self,
        method: &str,
        url: &Url,
        headers: &HashMap<String, String>,
        body: &[u8],
        cookie_context: Option<SameSiteContext>,
        store_cookies: bool,
        max_body_bytes: usize,
    ) -> Result<StealthResponseHeaders, ObscuraNetError> {
        self.validate_destination(url).await?;
        if is_tracker_blocked(url, self.block_trackers) {
            tracing::debug!("Blocked tracker: {}", url);
            return Ok(StealthResponseHeaders {
                status: 0,
                headers: HashMap::new(),
                body: Box::pin(async { Ok(Vec::new()) }),
            });
        }

        let req_method = method
            .parse::<wreq::Method>()
            .map_err(|e| ObscuraNetError::Network(format!("invalid method '{}': {}", method, e)))?;
        let mut req = self.client.request(req_method, url.as_str());
        if !headers.keys().any(|name| name.eq_ignore_ascii_case("user-agent")) {
            req = req.header("user-agent", self.user_agent.read().await.as_str());
        }
        if !headers.keys().any(|name| name.eq_ignore_ascii_case("accept-language")) {
            req = req.header("accept-language", self.accept_language.read().await.as_str());
        }

        if let Some(context) = cookie_context {
            let cookie_header = self.cookie_jar.get_cookie_header_in_context(url, context);
            if !cookie_header.is_empty() {
                req = req.header("cookie", &cookie_header);
            }
        }
        for (k, v) in headers.iter() {
            req = req.header(k.as_str(), v.as_str());
        }
        if !body.is_empty() {
            req = req.body(body.to_vec());
        }

        let in_flight = InFlightGuard::new(&self.in_flight);
        let resp = crate::on_network_io(async move { req.send().await }).await
            .map_err(ObscuraNetError::Network)?
            .map_err(|error| ObscuraNetError::Network(error_chain(&error)))?;

        let status = resp.status();
        if store_cookies {
            for val in resp.headers().get_all("set-cookie") {
                if let Ok(s) = val.to_str() {
                    self.cookie_jar.set_cookie(s, url);
                }
            }
        }
        let mut response_headers: HashMap<String, String> = HashMap::new();
        for (name, value) in resp.headers().iter() {
            let name = name.as_str();
            let value = crate::client::decode_response_header_value(value.as_bytes());
            if matches!(name, "content-security-policy" | "x-frame-options") {
                response_headers.entry(name.to_string())
                    .and_modify(|existing| {
                        existing.push_str(", ");
                        existing.push_str(&value);
                    })
                    .or_insert(value);
            } else {
                response_headers.insert(name.to_string(), value);
            }
        }
        let body_url = url.clone();
        Ok(StealthResponseHeaders {
            status: status.as_u16(),
            headers: response_headers,
            body: Box::pin(async move {
                let _in_flight = in_flight;
                crate::on_network_io(async move { read_wreq_body_limited(resp, &body_url, max_body_bytes).await })
                    .await.map_err(ObscuraNetError::Network)?
            }),
        })
    }

    pub async fn set_extra_headers(&self, headers: HashMap<String, String>) {
        *self.extra_headers.write().await = headers;
    }

    pub async fn set_user_agent(&self, user_agent: &str) { *self.user_agent.write().await = user_agent.to_string(); }

    pub async fn set_accept_language(&self, language: &str) { *self.accept_language.write().await = language.to_string(); }

    pub fn active_requests(&self) -> u32 {
        self.in_flight.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn is_network_idle(&self) -> bool {
        self.active_requests() == 0
    }
}

#[cfg(all(test, feature = "stealth"))]
mod tests {
    use std::io::Read;
    use std::sync::Arc;
    use std::time::Duration;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use url::Url;

    use super::{
        StealthHttpClient, is_tracker_blocked,
        tracker_blocking_enabled,
    };
    use crate::client::{ObscuraNetError, SsrfGuardResolver};
    use crate::cookies::CookieJar;
    use wreq::dns::{Name, Resolve};

    // Mirrors client::ssrf_tests::resolver_blocks_hostname_that_resolves_to_loopback.
    // Both transports must agree: a host-string check alone cannot see that a
    // public name points inward, so the stealth client needs the same resolver.
    #[tokio::test]
    async fn stealth_resolver_blocks_hostname_that_resolves_to_loopback() {
        let resolver = SsrfGuardResolver::new(false);
        let res = resolver.resolve(Name::from("localtest.me")).await;
        assert!(res.is_err(), "localtest.me -> 127.0.0.1 must be blocked");
    }

    #[tokio::test]
    async fn stealth_resolver_does_not_block_public_host() {
        // Tolerate a no-network sandbox: only an actual SSRF rejection fails.
        let resolver = SsrfGuardResolver::new(false);
        match resolver.resolve(Name::from("example.com")).await {
            Ok(_) => {}
            Err(e) => assert!(
                !e.to_string().contains("SSRF blocked"),
                "example.com wrongly SSRF-blocked: {e}"
            ),
        }
    }

    const PLAIN_BODY: &str = "<!DOCTYPE html><html><body><p id=\"mark\">gzip ok</p></body></html>";

    #[test]
    fn tracker_blocking_environment_defaults_to_enabled() {
        for value in [
            None,
            Some(""),
            Some("1"),
            Some("true"),
            Some("yes"),
            Some("on"),
            Some("invalid"),
        ] {
            assert!(
                tracker_blocking_enabled(value),
                "{value:?} must leave tracker blocking enabled"
            );
        }
        for value in [
            Some("0"), Some("false"), Some(" NO "), Some("off"), Some(" FALSE "),
        ] {
            assert!(
                !tracker_blocking_enabled(value),
                "{value:?} must disable tracker blocking"
            );
        }
    }

    #[test]
    fn tracker_blocking_respects_host_and_setting() {
        let url = Url::parse("https://www.google-analytics.com/collect").unwrap();

        assert!(is_tracker_blocked(&url, true));
        assert!(!is_tracker_blocked(&url, false));
        assert!(!is_tracker_blocked(
            &Url::parse("https://example.com/").unwrap(), true,
        ));
        assert!(!is_tracker_blocked(
            &Url::parse("file:///tmp/page.html").unwrap(), true,
        ));
    }

    #[test]
    fn tracker_blocking_constructor_reads_environment() {
        let client = StealthHttpClient::new(Arc::new(CookieJar::new()));
        let expected = tracker_blocking_enabled(
            std::env::var("OBSCURA_BLOCK_TRACKERS").ok().as_deref(),
        );
        assert_eq!(client.block_trackers, expected);
    }

    async fn assert_tracker_blocking_request(scripted: bool) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy = format!("http://{}", listener.local_addr().unwrap());
        let mut client = StealthHttpClient::with_proxy(
            Arc::new(CookieJar::new()),
            Some(&proxy),
            true,
        );
        let url = Url::parse("http://www.google-analytics.com/collect").unwrap();
        let headers = std::collections::HashMap::new();

        client.block_trackers = true;
        let blocked = if scripted {
            client.send_single("GET", &url, &headers, &[], false, false)
                .await
        } else {
            client.fetch(&url).await
        }.expect("blocked tracker returns an empty response");
        assert_eq!(blocked.status, 0);
        assert!(blocked.body.is_empty());
        assert!(tokio::time::timeout(Duration::from_millis(50), listener.accept())
            .await.is_err());

        let server = tokio::spawn(async move {
            let (mut stream, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
                .await.expect("request reaches local proxy").unwrap();
            let mut request = Vec::new();
            while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                let mut buffer = [0u8; 1024];
                let count = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buffer))
                    .await.expect("request headers arrive").unwrap();
                assert_ne!(count, 0);
                request.extend_from_slice(&buffer[..count]);
            }
            assert!(String::from_utf8(request).unwrap().starts_with(
                "GET http://www.google-analytics.com/collect HTTP/1.1\r\n",
            ));
            stream.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok")
                .await.unwrap();
        });

        client.block_trackers = false;
        let allowed = if scripted {
            client.send_single("GET", &url, &headers, &[], false, false)
                .await
        } else {
            client.fetch(&url).await
        }.expect("disabled blocking allows the request");
        assert_eq!(allowed.status, 200);
        assert_eq!(allowed.body, b"ok");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn tracker_blocking_navigation_request() {
        assert_tracker_blocking_request(false).await;
    }

    #[tokio::test]
    async fn tracker_blocking_scripted_request() {
        assert_tracker_blocking_request(true).await;
    }

    // gzip (level 9) of PLAIN_BODY, hardcoded so the fixture needs no
    // compression dependency. A wrong byte fails the assert below.
    const GZIP_BODY: &[u8] = &[
        0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x03, 0xb3, 0x51,
        0x74, 0xf1, 0x77, 0x0e, 0x89, 0x0c, 0x70, 0x55, 0xc8, 0x28, 0xc9, 0xcd,
        0xb1, 0xb3, 0x81, 0x90, 0x49, 0xf9, 0x29, 0x95, 0x76, 0x36, 0x05, 0x0a,
        0x99, 0x29, 0xb6, 0x4a, 0xb9, 0x89, 0x45, 0xd9, 0x4a, 0x76, 0xe9, 0x55,
        0x99, 0x05, 0x0a, 0xf9, 0xd9, 0x36, 0xfa, 0x05, 0x76, 0x36, 0xfa, 0x10,
        0x69, 0x7d, 0xb0, 0x5a, 0x00, 0x80, 0x3d, 0x1c, 0x5f, 0x41, 0x00, 0x00,
        0x00,
    ];

    fn reset_fixture() -> (u16, std::thread::JoinHandle<usize>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            let mut buf = [0u8; 1024];
            while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                let read = stream.read(&mut buf).unwrap();
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buf[..read]);
            }
            let socket = socket2::Socket::from(stream);
            socket.set_linger(Some(Duration::ZERO)).unwrap();
            drop(socket);
            1
        });
        (port, server)
    }


    #[tokio::test]
    async fn stealth_post_does_not_retry_connection_reset() {
        let (port, server) = reset_fixture();
        let client = StealthHttpClient::with_proxy(Arc::new(CookieJar::new()), None, true);
        let url = Url::parse(&format!("http://127.0.0.1:{port}/")).unwrap();
        let error = client
            .send_single(
                "POST",
                &url,
                &std::collections::HashMap::new(),
                b"payload",
                false,
                false,
            )
            .await
            .expect_err("POST must not be retried after a connection reset");

        assert!(matches!(error, ObscuraNetError::Network(_)));
        assert_eq!(server.join().unwrap(), 1);
    }

    // See `navigation_accepts_a_folded_response_header` in client.rs: the
    // stealth transport must read a folded header as Chrome does.
    #[tokio::test]
    async fn stealth_client_accepts_a_folded_response_header() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf).await;
            let response = "HTTP/1.1 200 OK\r\nContent-Security-Policy: default-src 'self'\n  https://a.example\n  https://b.example\r\ncontent-length: 6\r\nconnection: close\r\n\r\nfolded";
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.shutdown().await;
        });
        let client = StealthHttpClient::with_proxy(Arc::new(CookieJar::new()), None, true);
        let url = Url::parse(&format!("http://127.0.0.1:{port}/")).unwrap();

        let resp = client
            .fetch(&url)
            .await
            .expect("a folded header must not fail the response");
        assert_eq!(resp.status, 200);
        assert_eq!(resp.text(), "folded");
    }

    /// Serve one `Content-Encoding: gzip` response on an ephemeral port.
    async fn gzip_fixture() -> u16 {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    let _ = stream.read(&mut buf).await;
                    let head = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: text/html; charset=utf-8\r\ncontent-encoding: gzip\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                        GZIP_BODY.len()
                    );
                    let _ = stream.write_all(head.as_bytes()).await;
                    let _ = stream.write_all(GZIP_BODY).await;
                    let _ = stream.shutdown().await;
                });
            }
        });

        port
    }

    // The emulation profile advertises gzip, so origins compress. Without the
    // decoder the raw gzip bytes reach the HTML parser as document text.
    #[tokio::test]
    async fn stealth_client_decodes_gzip_response() {
        let port = gzip_fixture().await;
        let client = StealthHttpClient::with_proxy(Arc::new(CookieJar::new()), None, true);
        let url = Url::parse(&format!("http://127.0.0.1:{port}/")).unwrap();

        let resp = client.fetch(&url).await.expect("fixture must be reachable");
        assert_eq!(resp.status, 200);
        assert_eq!(resp.text(), PLAIN_BODY, "gzip body must be decompressed");
    }

    // #793: the opt-in must reach the DNS resolver. `validate_url` already
    // honours it for the localhost host, so a hostname target exercises the
    // resolver itself; before the fix the resolver was pinned to block and
    // refused loopback hostnames even with the flag set. Only the allowed
    // leg is asserted: CI sets OBSCURA_ALLOW_PRIVATE_NETWORK, which also
    // lifts the default block.
    #[tokio::test]
    async fn stealth_client_honors_allow_private_network_for_loopback_hostnames() {
        let port = gzip_fixture().await;
        let client = StealthHttpClient::with_proxy(Arc::new(CookieJar::new()), None, true);
        let url = Url::parse(&format!("http://localhost:{port}/")).unwrap();

        let resp = client
            .fetch(&url)
            .await
            .expect("loopback hostname must be reachable with the opt-in");
        assert_eq!(resp.status, 200);
    }
}
