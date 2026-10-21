//! Missing-issuer recovery stays inside the original TLS handshake. Material
//! downloads run on a separate reactor, never on the blocked verifier's reactor.
//! Downloaded certificates are untrusted chain inputs, never trust anchors.

use std::collections::{HashSet, VecDeque};
use std::sync::{mpsc, Arc, LazyLock, Mutex, OnceLock};
use std::time::{Duration, Instant};

use openssl::nid::Nid;
use openssl::pkcs7::Pkcs7;
use openssl::x509::{X509, X509Ref, X509VerifyResult};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{CertificateError, DigitallySignedStruct, SignatureScheme};
use url::Url;

const ROUNDS: usize = 3;
const REDIRECTS: usize = 3;
const MATERIAL_BYTES: usize = 64 * 1024;
const CACHE_URLS: usize = 64;
const FETCH_TIME: Duration = Duration::from_secs(5);
const TOTAL_TIME: Duration = Duration::from_secs(15);
const NEGATIVE_TIME: Duration = Duration::from_secs(5 * 60);
type Material = Arc<Vec<Vec<u8>>>;

#[derive(Clone, Debug)]
pub(crate) struct AiaPolicy {
    pub proxy_url: Option<String>,
    pub allow_private_network: bool,
    pub allow_network: Vec<obscura_ssrf::NetworkRange>,
}

#[derive(Clone, Copy, Debug)]
enum Transport {
    Ordinary,
    #[cfg(feature = "stealth")]
    Stealth,
}

/// The same configured root sources as the ordinary client, in canonical DER.
/// The platform verifier merges these roots; it does not replace its roots.
pub(crate) fn configured_root_der() -> &'static [CertificateDer<'static>] {
    static ROOTS: LazyLock<Vec<CertificateDer<'static>>> = LazyLock::new(|| {
        let mut roots = Vec::new();
        for path in crate::client::configured_root_paths() {
            let parsed: Result<Vec<Vec<u8>>, String> = std::fs::read(&path).map_err(|e| e.to_string()).and_then(|bytes| {
                if let Ok(certs) = X509::stack_from_pem(&bytes) {
                    if !certs.is_empty() {
                        return certs.into_iter().map(|cert| cert.to_der().map_err(|e| e.to_string())).collect();
                    }
                }
                X509::from_der(&bytes).and_then(|cert| cert.to_der())
                    .map(|cert| vec![cert]).map_err(|e| e.to_string())
            });
            match parsed {
                Ok(certs) => roots.extend(certs.into_iter().map(CertificateDer::from)),
                Err(error) => tracing::warn!(%error, path = %path.display(), "failed to load configured CA certificate"),
            }
        }
        roots
    });
    &ROOTS
}

fn native_rustls_verifier() -> Result<Arc<rustls_platform_verifier::Verifier>, String> {
    let provider = rustls::crypto::CryptoProvider::get_default().cloned()
        .unwrap_or_else(|| Arc::new(rustls::crypto::aws_lc_rs::default_provider()));
    let roots = configured_root_der();
    let verifier = if roots.is_empty() {
        rustls_platform_verifier::Verifier::new(provider)
    } else {
        #[cfg(any(all(unix, not(target_os = "android")), target_os = "windows"))]
        { rustls_platform_verifier::Verifier::new_with_extra_roots(roots.iter().cloned(), provider) }
        #[cfg(not(any(all(unix, not(target_os = "android")), target_os = "windows")))]
        { return Err("platform verifier cannot merge configured roots on this platform".into()); }
    }.map_err(|e| e.to_string())?;
    Ok(Arc::new(verifier))
}

fn rustls_config(verifier: Arc<dyn ServerCertVerifier>) -> Result<rustls::ClientConfig, String> {
    let provider = rustls::crypto::CryptoProvider::get_default().cloned()
        .unwrap_or_else(|| Arc::new(rustls::crypto::aws_lc_rs::default_provider()));
    let mut config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions().map_err(|e| e.to_string())?
        .dangerous().with_custom_certificate_verifier(verifier).with_no_client_auth();
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok(config)
}

pub(crate) fn configure_reqwest(
    builder: reqwest::ClientBuilder,
    policy: AiaPolicy,
) -> Result<reqwest::ClientBuilder, String> {
    let verifier = Arc::new(RecoveringVerifier {
        native: native_rustls_verifier()?,
        recovery: Recovery::new(policy, Transport::Ordinary),
    });
    Ok(builder.tls_backend_preconfigured(rustls_config(verifier)?))
}

#[derive(Debug)]
struct RecoveringVerifier {
    native: Arc<rustls_platform_verifier::Verifier>,
    recovery: Recovery,
}

impl RecoveringVerifier {
    fn revalidate(&self, leaf: &CertificateDer<'_>, chain: &[CertificateDer<'_>], name: &ServerName<'_>, ocsp: &[u8], now: UnixTime, deadline: Instant) -> Result<ServerCertVerified, rustls::Error> {
        #[cfg(target_os = "windows")]
        { self.native.verify_server_cert_with_deadline(leaf, chain, name, ocsp, now, Some(deadline)) }
        #[cfg(not(target_os = "windows"))]
        {
            if Instant::now() >= deadline {
                return Err(rustls::Error::InvalidCertificate(CertificateError::UnknownIssuer));
            }
            self.native.verify_server_cert(leaf, chain, name, ocsp, now)
        }
    }
}

impl ServerCertVerifier for RecoveringVerifier {
    fn verify_server_cert(&self, leaf: &CertificateDer<'_>, chain: &[CertificateDer<'_>], name: &ServerName<'_>, ocsp: &[u8], now: UnixTime) -> Result<ServerCertVerified, rustls::Error> {
        let original = match self.native.verify_server_cert(leaf, chain, name, ocsp, now) {
            Ok(verified) => return Ok(verified),
            Err(error @ rustls::Error::InvalidCertificate(CertificateError::UnknownIssuer)) => error,
            Err(error) => return Err(error),
        };
        let mut chain: Vec<CertificateDer<'static>> = chain.iter().map(|cert| cert.clone().into_owned()).collect();
        let mut attempt = Attempt::new();
        for _ in 0..ROUNDS {
            let Some(issuer) = self.recovery.next_issuer(leaf.as_ref(), chain.iter().map(|cert| cert.as_ref()), &mut attempt) else { break; };
            chain.extend(issuer.into_iter().map(CertificateDer::from));
            if Instant::now() >= attempt.deadline { break; }
            let result = self.revalidate(leaf, &chain, name, ocsp, now, attempt.deadline);
            if Instant::now() >= attempt.deadline { break; }
            match result {
                Ok(verified) if Instant::now() < attempt.deadline => return Ok(verified),
                Ok(_) => break,
                Err(rustls::Error::InvalidCertificate(CertificateError::UnknownIssuer)) => {}
                Err(error) => return Err(error),
            }
        }
        Err(original)
    }

    fn verify_tls12_signature(&self, message: &[u8], cert: &CertificateDer<'_>, signature: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.native.verify_tls12_signature(message, cert, signature)
    }

    fn verify_tls13_signature(&self, message: &[u8], cert: &CertificateDer<'_>, signature: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.native.verify_tls13_signature(message, cert, signature)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> { self.native.supported_verify_schemes() }
    fn requires_raw_public_keys(&self) -> bool { self.native.requires_raw_public_keys() }
    fn root_hint_subjects(&self) -> Option<&[rustls::DistinguishedName]> { self.native.root_hint_subjects() }
}

#[derive(Debug)]
struct MaterialVerifier {
    native: Arc<rustls_platform_verifier::Verifier>,
    deadline: Mutex<Instant>,
}

impl ServerCertVerifier for MaterialVerifier {
    fn verify_server_cert(&self, leaf: &CertificateDer<'_>, chain: &[CertificateDer<'_>], name: &ServerName<'_>, ocsp: &[u8], now: UnixTime) -> Result<ServerCertVerified, rustls::Error> {
        let deadline = *self.deadline.lock().map_err(|_| rustls::Error::General("AIA verifier unavailable".into()))?;
        if Instant::now() >= deadline { return Err(rustls::Error::InvalidCertificate(CertificateError::UnknownIssuer)); }
        #[cfg(target_os = "windows")]
        let result = self.native.verify_server_cert_with_deadline(leaf, chain, name, ocsp, now, Some(deadline));
        #[cfg(not(target_os = "windows"))]
        let result = self.native.verify_server_cert(leaf, chain, name, ocsp, now);
        if Instant::now() >= deadline { return Err(rustls::Error::InvalidCertificate(CertificateError::UnknownIssuer)); }
        result
    }
    fn verify_tls12_signature(&self, message: &[u8], cert: &CertificateDer<'_>, signature: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.native.verify_tls12_signature(message, cert, signature)
    }
    fn verify_tls13_signature(&self, message: &[u8], cert: &CertificateDer<'_>, signature: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.native.verify_tls13_signature(message, cert, signature)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> { self.native.supported_verify_schemes() }
    fn requires_raw_public_keys(&self) -> bool { self.native.requires_raw_public_keys() }
    fn root_hint_subjects(&self) -> Option<&[rustls::DistinguishedName]> { self.native.root_hint_subjects() }
}

struct Attempt {
    deadline: Instant,
    urls: HashSet<Url>,
    certificates: HashSet<Vec<u8>>,
}

impl Attempt {
    fn new() -> Self {
        Self { deadline: Instant::now() + TOTAL_TIME, urls: HashSet::new(), certificates: HashSet::new() }
    }
}

#[derive(Debug)]
struct CacheEntry {
    url: Url,
    material: Option<Material>,
    inserted: Instant,
}

struct Job {
    url: Url,
    deadline: Instant,
    reply: mpsc::SyncSender<Result<Vec<u8>, String>>,
}

#[derive(Debug)]
struct Recovery {
    policy: AiaPolicy,
    transport: Transport,
    cache: Mutex<VecDeque<CacheEntry>>,
    worker: OnceLock<Result<mpsc::SyncSender<Job>, String>>,
}

impl Recovery {
    fn new(policy: AiaPolicy, transport: Transport) -> Self {
        Self { policy, transport, cache: Mutex::new(VecDeque::new()), worker: OnceLock::new() }
    }

    fn next_issuer<'a>(&self, leaf: &[u8], chain: impl Iterator<Item = &'a [u8]>, attempt: &mut Attempt) -> Option<Vec<Vec<u8>>> {
        if Instant::now() >= attempt.deadline { return None; }
        let leaf = X509::from_der(leaf).ok()?;
        let chain: Vec<X509> = chain.filter_map(|cert| X509::from_der(cert).ok()).collect();
        let mut visited = HashSet::new();
        let mut pending = vec![leaf];
        let mut frontier = Vec::new();
        // Traverse every authenticated peer path, including cross-signs.
        // Unrelated certificates never acquire permission to supply AIA URLs.
        while let Some(child) = pending.pop() {
            let der = child.to_der().ok()?;
            if !visited.insert(der) || direct_issuer(&child, &child) { continue; }
            let issuers: Vec<_> = chain.iter().filter(|issuer| direct_issuer(issuer, &child)).cloned().collect();
            if issuers.is_empty() { frontier.push(child); } else { pending.extend(issuers); }
        }
        for child in frontier {
            if !attempt.certificates.insert(child.to_der().ok()?) { continue; }
            let Some(access) = child.authority_info() else { continue; };
            for location in access.iter().filter(|entry| entry.method().nid() == Nid::AD_CA_ISSUERS) {
                if Instant::now() >= attempt.deadline || attempt.urls.len() >= CACHE_URLS { return None; }
                let Some(uri) = location.location().uri() else { continue; };
                let Ok(url) = Url::parse(uri) else { continue; };
                if !matches!(url.scheme(), "http" | "https") || !attempt.urls.insert(url.clone()) { continue; }
                let Some(material) = self.material(&url, attempt.deadline) else { continue; };
                let certificates: Vec<_> = material.iter().filter_map(|der| X509::from_der(der).ok().map(|cert| (der, cert))).collect();
                let mut additions = Vec::new();
                let mut pending = vec![child.clone()];
                let mut included = visited.clone();
                while let Some(bundle_child) = pending.pop() {
                    for (der, issuer) in &certificates {
                        if included.contains(*der) || direct_issuer(issuer, issuer) || !direct_issuer(issuer, &bundle_child) { continue; }
                        included.insert((*der).clone());
                        additions.push((*der).clone());
                        pending.push(issuer.clone());
                    }
                }
                // Native pathbuilding, not bundle order, decides which of the
                // authenticated cross-signs actually reaches a trusted root.
                if !additions.is_empty() { return Some(additions); }
            }
        }
        None
    }

    fn material(&self, url: &Url, deadline: Instant) -> Option<Material> {
        // No lock is held across worker submission or network I/O.
        {
            let mut cache = self.cache.lock().ok()?;
            if let Some(index) = cache.iter().position(|entry| &entry.url == url) {
                let entry = cache.remove(index)?;
                if entry.material.is_some() || entry.inserted.elapsed() < NEGATIVE_TIME {
                    let material = entry.material.clone();
                    cache.push_back(entry);
                    return material;
                }
            }
        }
        let material = self.download(url.clone(), deadline).and_then(|bytes| parse_material(&bytes));
        if let Err(error) = &material {
            tracing::debug!(%url, %error, "AIA material recovery refused; retaining certificate failure");
        }
        let material = material.ok().map(Arc::new);
        if let Ok(mut cache) = self.cache.lock() {
            cache.retain(|entry| &entry.url != url);
            if cache.len() == CACHE_URLS { cache.pop_front(); }
            cache.push_back(CacheEntry { url: url.clone(), material: material.clone(), inserted: Instant::now() });
        }
        material
    }

    fn download(&self, url: Url, deadline: Instant) -> Result<Vec<u8>, String> {
        let worker = self.worker.get_or_init(|| {
            let (sender, receiver) = mpsc::sync_channel::<Job>(1);
            let policy = self.policy.clone();
            let transport = self.transport;
            std::thread::Builder::new().name("obscura-aia".into()).spawn(move || {
                // Clients and pools are constructed, used and dropped on this
                // reactor. Waiting TLS callbacks never supply their own client.
                let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build();
                let Ok(runtime) = runtime else { return; };
                runtime.block_on(async move {
                    let client = MaterialClient::new(&policy, transport);
                    while let Ok(job) = receiver.recv() {
                        let result = match &client {
                            Ok(client) => {
                                if Instant::now() >= job.deadline {
                                    let _ = job.reply.send(Err("AIA total deadline exceeded".into()));
                                    continue;
                                }
                                let limit = job.deadline.min(Instant::now() + FETCH_TIME);
                                tokio::time::timeout_at(tokio::time::Instant::from_std(limit), client.fetch(job.url, &policy, limit))
                                    .await.map_err(|_| "AIA retrieval deadline exceeded".to_string()).and_then(|result| result)
                            }
                            Err(error) => Err(error.clone()),
                        };
                        let _ = job.reply.send(result);
                    }
                });
            }).map_err(|e| e.to_string())?;
            Ok(sender)
        }).as_ref().map_err(Clone::clone)?;
        let remaining = deadline.checked_duration_since(Instant::now()).ok_or("AIA total deadline exceeded")?;
        let (reply, receiver) = mpsc::sync_channel(1);
        worker.try_send(Job { url, deadline, reply }).map_err(|_| "AIA worker unavailable or busy".to_string())?;
        receiver.recv_timeout(remaining).map_err(|_| "AIA total deadline exceeded".to_string())?
    }
}

fn direct_issuer(issuer: &X509Ref, child: &X509Ref) -> bool {
    issuer.issued(child) == X509VerifyResult::OK
        && issuer.public_key().and_then(|key| child.verify(&key)).unwrap_or(false)
}

fn parse_material(bytes: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    if bytes.len() > MATERIAL_BYTES { return Err("AIA response exceeds 64KiB".into()); }
    let certificates = if let Ok(certificate) = X509::from_der(bytes) {
        vec![certificate]
    } else if let Ok(certificates) = X509::stack_from_pem(bytes) {
        if certificates.is_empty() { return parse_pkcs7(bytes); }
        certificates
    } else {
        return parse_pkcs7(bytes);
    };
    certificates.into_iter().map(|cert| cert.to_der().map_err(|e| e.to_string())).collect()
}

fn parse_pkcs7(bytes: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    let bundle = Pkcs7::from_der(bytes).or_else(|_| Pkcs7::from_pem(bytes)).map_err(|e| e.to_string())?;
    let certificates = bundle.signed().and_then(|signed| signed.certificates()).ok_or("AIA PKCS#7 has no certificates")?;
    if certificates.is_empty() { return Err("AIA PKCS#7 has no certificates".into()); }
    certificates.iter().map(|cert| cert.to_der().map_err(|e| e.to_string())).collect()
}

enum MaterialClient {
    Ordinary { client: reqwest::Client, verifier: Arc<MaterialVerifier> },
    #[cfg(feature = "stealth")]
    Stealth(wreq::Client),
}

impl MaterialClient {
    fn new(policy: &AiaPolicy, transport: Transport) -> Result<Self, String> {
        match transport {
            Transport::Ordinary => {
                let verifier = Arc::new(MaterialVerifier { native: native_rustls_verifier()?, deadline: Mutex::new(Instant::now()) });
                let mut builder = reqwest::Client::builder()
                    .no_proxy()
                    .tls_backend_preconfigured(rustls_config(verifier.clone())?)
                    .redirect(reqwest::redirect::Policy::none()).retry(reqwest::retry::never())
                    .dns_resolver(Arc::new(crate::client::SsrfGuardResolver::with_networks(policy.allow_private_network, policy.allow_network.clone())))
                    .timeout(FETCH_TIME);
                if let Some(proxy) = &policy.proxy_url {
                    builder = builder.proxy(reqwest::Proxy::all(proxy).map_err(|e| e.to_string())?);
                }
                builder.build().map(|client| Self::Ordinary { client, verifier }).map_err(|e| e.to_string())
            }
            #[cfg(feature = "stealth")]
            Transport::Stealth => {
                let mut builder = wreq::Client::builder()
                    .no_proxy()
                    .emulation(wreq_util::Emulation::builder().profile(wreq_util::Profile::Chrome145).platform(wreq_util::Platform::Windows).build())
                    .redirect(wreq::redirect::Policy::none()).retry(wreq::retry::Policy::never())
                    .dns_resolver(Arc::new(crate::client::SsrfGuardResolver::with_networks(policy.allow_private_network, policy.allow_network.clone())))
                    .timeout(FETCH_TIME);
                builder = configured_wreq_roots(builder);
                if let Some(proxy) = &policy.proxy_url {
                    builder = builder.proxy(wreq::Proxy::all(proxy).map_err(|e| e.to_string())?);
                }
                builder.build().map(Self::Stealth).map_err(|e| e.to_string())
            }
        }
    }

    async fn fetch(&self, mut url: Url, policy: &AiaPolicy, deadline: Instant) -> Result<Vec<u8>, String> {
        let mut visited = HashSet::new();
        for redirect in 0..=REDIRECTS {
            if Instant::now() >= deadline { return Err("AIA retrieval deadline exceeded".into()); }
            if !visited.insert(url.clone()) { return Err("AIA redirect cycle".into()); }
            validate_material_url(&url, policy).await?;
            match self {
                Self::Ordinary { client, verifier } => {
                    *verifier.deadline.lock().map_err(|_| "AIA verifier unavailable")? = deadline;
                    let mut response = client.get(url.clone()).send().await.map_err(|e| e.to_string())?;
                    if response.status().is_redirection() {
                        if redirect == REDIRECTS { return Err("AIA redirect limit exceeded".into()); }
                        url = redirect_url(&url, response.headers().get("location").and_then(|value| value.to_str().ok()))?;
                        continue;
                    }
                    if !response.status().is_success() { return Err(format!("AIA returned HTTP {}", response.status())); }
                    if response.content_length().is_some_and(|length| length > MATERIAL_BYTES as u64) { return Err("AIA response exceeds 64KiB".into()); }
                    let mut body = Vec::new();
                    while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
                        append_material(&mut body, &chunk)?;
                    }
                    return Ok(body);
                }
                #[cfg(feature = "stealth")]
                Self::Stealth(client) => {
                    use futures_util::StreamExt;
                    let response = client.get(url.as_str()).send().await.map_err(|e| e.to_string())?;
                    if response.status().is_redirection() {
                        if redirect == REDIRECTS { return Err("AIA redirect limit exceeded".into()); }
                        url = redirect_url(&url, response.headers().get("location").and_then(|value| value.to_str().ok()))?;
                        continue;
                    }
                    if !response.status().is_success() { return Err(format!("AIA returned HTTP {}", response.status())); }
                    if response.content_length().is_some_and(|length| length > MATERIAL_BYTES as u64) { return Err("AIA response exceeds 64KiB".into()); }
                    let stream = response.bytes_stream();
                    futures_util::pin_mut!(stream);
                    let mut body = Vec::new();
                    while let Some(chunk) = stream.next().await { append_material(&mut body, &chunk.map_err(|e| e.to_string())?)?; }
                    return Ok(body);
                }
            }
        }
        Err("AIA redirect limit exceeded".into())
    }
}

fn append_material(body: &mut Vec<u8>, chunk: &[u8]) -> Result<(), String> {
    if chunk.len() > MATERIAL_BYTES.saturating_sub(body.len()) { return Err("AIA response exceeds 64KiB".into()); }
    body.extend_from_slice(chunk);
    Ok(())
}

fn redirect_url(url: &Url, location: Option<&str>) -> Result<Url, String> {
    url.join(location.ok_or("AIA redirect has no valid Location")?).map_err(|e| e.to_string())
}

async fn validate_material_url(url: &Url, policy: &AiaPolicy) -> Result<(), String> {
    if !matches!(url.scheme(), "http" | "https") { return Err("AIA requires HTTP(S)".into()); }
    if !url.username().is_empty() || url.password().is_some() { return Err("AIA URL credentials are not allowed".into()); }
    let host = url.host_str().ok_or("AIA URL has no host")?.trim_start_matches('[').trim_end_matches(']');
    let allow = policy.allow_private_network || obscura_ssrf::env_allows_private_network();
    // Check even for CONNECT/SOCKS, where the proxy may perform target DNS and
    // therefore bypass the native client's direct-connect resolver.
    let addresses: Vec<_> = tokio::net::lookup_host((host, url.port_or_known_default().ok_or("AIA URL has no port")?))
        .await.map_err(|e| e.to_string())?.collect();
    if addresses.is_empty() { return Err("AIA hostname has no addresses".into()); }
    if let Some(address) = addresses.iter().find(|address| !obscura_ssrf::address_allowed(address.ip(), allow, &policy.allow_network)) {
        return Err(format!("AIA address {} is not authorized", address.ip()));
    }
    Ok(())
}

#[cfg(feature = "stealth")]
fn configured_wreq_roots(mut builder: wreq::ClientBuilder) -> wreq::ClientBuilder {
    if crate::client::custom_cert_store_requested(std::env::var_os("SSL_CERT_FILE").as_deref(), std::env::var_os("SSL_CERT_DIR").as_deref()) {
        match wreq::tls::trust::CertStore::builder().set_default_paths().build() {
            Ok(store) => builder = builder.tls_cert_store(store),
            Err(error) => tracing::warn!(%error, "configured TLS store failed; retaining existing default roots"),
        }
    }
    builder
}

#[cfg(feature = "stealth")]
pub(crate) fn configure_wreq(builder: wreq::ClientBuilder, policy: AiaPolicy) -> Result<wreq::ClientBuilder, String> {
    let builder = configured_wreq_roots(builder);
    let recovery = Recovery::new(policy, Transport::Stealth);
    Ok(builder.tls_cert_verify_callback(move |context| {
        match verify_boring(context, &recovery) {
            Ok(verified) => verified,
            Err(error) => {
                tracing::debug!(%error, "native AIA verifier failed closed");
                context.set_error(Err(btls::x509::X509VerifyError::UNSPECIFIED));
                false
            }
        }
    }))
}

#[cfg(feature = "stealth")]
fn verify_boring(context: &mut btls::x509::X509StoreContextRef, recovery: &Recovery) -> Result<bool, btls::error::ErrorStack> {
    use btls::stack::Stack;
    use btls::x509::{verify::X509VerifyParam, X509StoreContext, X509VerifyError};
    if context.verify_cert()? { return Ok(true); }
    let original = context.verify_result();
    let missing = |result: btls::x509::X509VerifyResult| matches!(result, Err(error) if error == X509VerifyError::UNABLE_TO_GET_ISSUER_CERT_LOCALLY || error == X509VerifyError::UNABLE_TO_VERIFY_LEAF_SIGNATURE);
    if !missing(original) { return Ok(false); }
    let Some(leaf) = context.cert().map(ToOwned::to_owned) else { return Ok(false); };
    let Some(ssl) = context.ex_data(X509StoreContext::ssl_idx()?) else { return Ok(false); };
    let store = ssl.ssl_context().cert_store().to_owned();
    let mut parameters = X509VerifyParam::new()?;
    parameters.copy_from(context.verify_param())?;
    let mut chain = Stack::new()?;
    if let Some(peer) = context.untrusted() {
        for cert in peer { chain.push(cert.to_owned())?; }
    }
    let leaf_der = leaf.to_der()?;
    let mut attempt = Attempt::new();
    for _ in 0..ROUNDS {
        let der_chain: Vec<_> = chain.iter().map(|cert| cert.to_der()).collect::<Result<_, _>>()?;
        let Some(issuer) = recovery.next_issuer(&leaf_der, der_chain.iter().map(Vec::as_slice), &mut attempt) else { break; };
        for issuer in issuer { chain.push(btls::x509::X509::from_der(&issuer)?)?; }
        if Instant::now() >= attempt.deadline { break; }
        // Reinitialization is mandatory. Reusing an already verified context
        // can retain an old path and does not perform a fresh full validation.
        let mut fresh = X509StoreContext::new()?;
        let (verified, result) = fresh.init(&store, &leaf, &chain, |fresh| {
            fresh.verify_param_mut().copy_from(&parameters)?;
            let verified = fresh.verify_cert()?;
            if Instant::now() >= attempt.deadline { return Ok((false, original)); }
            Ok((verified, fresh.verify_result()))
        })?;
        context.set_error(result);
        if verified { return Ok(true); }
        if !missing(result) { return Ok(false); }
    }
    context.set_error(original);
    Ok(false)
}
