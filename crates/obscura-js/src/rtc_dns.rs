//! RFC 8484 resolution for ICE endpoints without a direct DNS escape.
//! The resolver URL is operator configuration, never page input; there is no
//! default third-party resolver or unproxied retry.
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use base64::Engine;
use hickory_proto::op::{Message, MessageType, Query, ResponseCode};
use hickory_proto::rr::{Name, RData, RecordType};
use obscura_net::{CookieJar, ObscuraHttpClient, RequestCredentials, ResourceRequest};
#[cfg(feature = "stealth")]
use obscura_net::StealthHttpClient;

fn failure(message: impl Into<String>) -> io::Error { io::Error::new(io::ErrorKind::PermissionDenied, message.into()) }

pub(crate) async fn resolve_rtc_host(
    selected: Arc<ObscuraHttpClient>,
    #[cfg(feature = "stealth")] selected_stealth: Option<Arc<StealthHttpClient>>,
    host: String,
    port: u16,
) -> io::Result<Vec<SocketAddr>> {
    #[cfg(feature = "stealth")]
    let (proxy, allow_private, allow_network) = if let Some(client) = &selected_stealth {
        (client.proxy_url().map(str::to_owned), client.allow_private_network, client.allow_network.clone())
    } else { (selected.proxy_url().map(str::to_owned), selected.allow_private_network, selected.allow_network.clone()) };
    #[cfg(not(feature = "stealth"))]
    let (proxy, allow_private, allow_network) = (selected.proxy_url().map(str::to_owned), selected.allow_private_network, selected.allow_network.clone());
    let authorize = |ip| obscura_ssrf::address_allowed(ip, allow_private, &allow_network);
    if let Ok(address) = host.parse::<IpAddr>() {
        return if authorize(address) { Ok(vec![SocketAddr::new(address, port)]) } else { Err(failure("RTC destination rejected by network address policy")) };
    }
    if proxy.is_none() {
        let addresses = tokio::time::timeout(std::time::Duration::from_secs(10), tokio::net::lookup_host((host.as_str(), port)))
            .await.map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "RTC DNS timed out"))??;
        let mut pinned = Vec::new();
        for address in addresses {
            if !authorize(address.ip()) { return Err(failure("RTC DNS response includes an address rejected by network policy")); }
            pinned.push(address);
        }
        pinned.sort_unstable(); pinned.dedup();
        return if pinned.is_empty() { Err(io::Error::new(io::ErrorKind::NotFound, "RTC hostname has no authorized addresses")) } else { Ok(pinned) };
    }
    let resolver = std::env::var("OBSCURA_RTC_DOH_URL").map_err(|_| failure("Proxied RTC hostname resolution requires OBSCURA_RTC_DOH_URL; no direct DNS fallback"))?;
    let resolver = url::Url::parse(&resolver).map_err(|_| failure("Invalid RTC DoH URL"))?;
    if resolver.host_str().is_none_or(|host| host.parse::<IpAddr>().is_err()) {
        return Err(failure("RTC DoH bootstrap URL must contain a literal resolver IP with valid TLS identity; never resolve the resolver through ambient DNS"));
    }
    if !resolver.username().is_empty() || resolver.password().is_some() || resolver.fragment().is_some()
        || !(resolver.scheme() == "https" || (resolver.scheme() == "http" && resolver.host_str().is_some_and(|host|
            host == "localhost" || host.parse::<IpAddr>().is_ok_and(|address| address.is_loopback())))) {
        return Err(failure("RTC DoH requires HTTPS (or an explicitly authorized loopback HTTP fixture), without URL credentials or fragments"));
    }
    // A fresh jar and clients keep both DNS responses and the resolver origin
    // out of page cookies, headers, cache and network interception.
    let jar = Arc::new(CookieJar::new());
    let mut ordinary = ObscuraHttpClient::with_full_options(jar.clone(), proxy.as_deref(), allow_private);
    ordinary.allow_network.clone_from(&allow_network);
    ordinary.timeout = std::time::Duration::from_secs(10);
    #[cfg(feature = "stealth")]
    let stealth = selected_stealth.as_ref().map(|_| {
        let mut client = StealthHttpClient::with_proxy(jar, proxy.as_deref(), allow_private);
        client.allow_network.clone_from(&allow_network);
        client
    });
    let name = Name::from_ascii(&host).map_err(|_| failure("Invalid RTC DNS hostname"))?;
    let lookup = |kind| {
        let name = name.clone();
        let mut endpoint = resolver.clone();
        let ordinary = &ordinary;
        #[cfg(feature = "stealth")]
        let stealth = &stealth;
        async move {
            // RFC 8484 recommends zero IDs for useful HTTP caching.
            let id = 0;
            let mut request = Message::new();
            request.set_id(id).set_recursion_desired(true).add_query(Query::query(name.clone(), kind));
            let wire = request.to_vec().map_err(|_| failure("Cannot encode RTC DNS query"))?;
            endpoint.query_pairs_mut().append_pair("dns", &base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(wire));
            let mut profile = ResourceRequest::navigation().with_max_response_bytes(64 * 1024);
            profile.credentials = RequestCredentials::Omit;
            let response = {
                #[cfg(feature = "stealth")]
                if let Some(client) = stealth { client.fetch_resource_with_callbacks(&endpoint, profile, None).await }
                else { ordinary.fetch_resource_with_callbacks(&endpoint, profile, None).await }
                #[cfg(not(feature = "stealth"))]
                ordinary.fetch_resource_with_callbacks(&endpoint, profile, None).await
            }.map_err(|_| failure("RTC DoH request failed through configured route; no direct DNS fallback"))?;
            if response.status != 200 || !response.header("content-type").is_some_and(|value| value.split(';').next().is_some_and(|value| value.trim().eq_ignore_ascii_case("application/dns-message"))) {
                return Err(failure("RTC DoH response requires 200 application/dns-message"));
            }
            let response = Message::from_vec(&response.body).map_err(|_| failure("Malformed RTC DNS response"))?;
            if response.id() != id || response.message_type() != MessageType::Response || response.truncated()
                || response.queries().len() != 1 || response.queries()[0].name() != &name || response.queries()[0].query_type() != kind {
                return Err(failure("RTC DNS response does not match its query"));
            }
            if response.response_code() == ResponseCode::NXDomain { return Ok(Vec::new()); }
            if response.response_code() != ResponseCode::NoError {
                return Err(io::Error::new(io::ErrorKind::NotFound, "RTC resolver failed the DNS query"));
            }
            let mut addresses = Vec::new();
            for answer in response.answers() {
                let address = match answer.data() { RData::A(value) => IpAddr::V4(value.0), RData::AAAA(value) => IpAddr::V6(value.0), _ => continue };
                if !authorize(address) { return Err(failure("RTC DNS response includes an address rejected by network policy")); }
                addresses.push(SocketAddr::new(address, port));
            }
            Ok::<_, io::Error>(addresses)
        }
    };
    let lookup_both = async { tokio::try_join!(lookup(RecordType::A), lookup(RecordType::AAAA)) };
    let (mut ipv4, ipv6) = tokio::time::timeout(std::time::Duration::from_secs(10), lookup_both).await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "RTC routed DNS timed out"))??;
    ipv4.extend(ipv6); ipv4.sort_unstable(); ipv4.dedup();
    if ipv4.is_empty() { return Err(io::Error::new(io::ErrorKind::NotFound, "RTC hostname has no authorized addresses")); }
    Ok(ipv4)
}
