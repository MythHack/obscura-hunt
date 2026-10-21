//! Local regional identity resolution. No database downloads or implicit providers.
use std::collections::BTreeMap;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RegionalConfig {
    /// `off`, `auto`, or a literal IPv4/IPv6 address. Explicit addresses never probe.
    pub geoip: String,
    pub database: Option<PathBuf>,
    pub database_manifest: Option<PathBuf>,
    /// Official CLDR supplementalData.xml, containing territoryInfo.
    pub territory_data: Option<PathBuf>,
    pub territory_manifest: Option<PathBuf>,
    /// An operator-selected IP echo URL. Required for auto; no provider fan-out.
    pub discovery_url: Option<String>,
    pub seed: u64,
    pub locale: Option<String>,
    pub timezone: Option<String>,
    pub languages: Option<Vec<String>>,
    pub position: Option<obscura_js::geolocation::Position>,
}

/// A locally supplied, hash-bound declaration of the operator's right to use data.
/// `usable_until` encodes any supplier update/deletion obligation, not a browser
/// invented freshness interval. No expiry is required when the licence has none.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataManifest {
    pub supplier: String,
    pub version: String,
    pub license: String,
    pub attribution: String,
    pub source: String,
    pub sha256: String,
    pub update_obligations: String,
    pub usable_until: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IpProvenance {
    pub address: IpAddr,
    pub family: String,
    pub source: String,
    pub observed_at: u64,
    /// This is a point-in-time observation, not an assertion about rotated exits.
    pub scope: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResolvedRegion {
    pub locale: String,
    pub timezone: String,
    pub languages: Vec<String>,
    pub position: Option<obscura_js::geolocation::Position>,
    pub country: Option<String>,
    pub ip: Option<IpProvenance>,
    pub database: Option<DataManifest>,
    pub database_build_epoch: Option<u64>,
    pub territory_data: Option<DataManifest>,
    pub sources: BTreeMap<String, String>,
    pub seed: u64,
    #[serde(default)]
    pub routing_fingerprint: Option<String>,
}

impl ResolvedRegion {
    pub fn accept_language(&self) -> String {
        self.languages.iter().enumerate().map(|(index, tag)| {
            if index == 0 { tag.clone() } else { format!("{tag};q={:.1}", (1.0 - index as f64 / 10.0).max(0.1)) }
        }).collect::<Vec<_>>().join(",")
    }
}

fn env_value(name: &str) -> Option<String> {
    std::env::var(name).ok().map(|value| value.trim().to_owned()).filter(|value| !value.is_empty())
}

impl RegionalConfig {
    pub fn from_env() -> Result<Self, String> {
        let mut config: Self = match env_value("OBSCURA_REGIONAL_CONFIG") {
            Some(json) => serde_json::from_str(&json).map_err(|error| format!("Invalid regional configuration: {error}"))?,
            None => Self::default(),
        };
        if let Some(value) = env_value("OBSCURA_GEOIP") { config.geoip = value; }
        if let Some(value) = env_value("OBSCURA_GEOIP_DB") { config.database = Some(value.into()); }
        if let Some(value) = env_value("OBSCURA_GEOIP_MANIFEST") { config.database_manifest = Some(value.into()); }
        if let Some(value) = env_value("OBSCURA_TERRITORY_DATA") { config.territory_data = Some(value.into()); }
        if let Some(value) = env_value("OBSCURA_TERRITORY_MANIFEST") { config.territory_manifest = Some(value.into()); }
        if let Some(value) = env_value("OBSCURA_GEOIP_DISCOVERY_URL") { config.discovery_url = Some(value); }
        if let Some(value) = env_value("OBSCURA_REGIONAL_SEED") {
            config.seed = value.parse().map_err(|_| "OBSCURA_REGIONAL_SEED must be a u64".to_string())?;
        }
        if let Some(value) = env_value("OBSCURA_LOCALE") { config.locale = Some(value); }
        if let Some(value) = env_value("OBSCURA_TIMEZONE") { config.timezone = Some(value); }
        if let Some(value) = env_value("OBSCURA_LANGUAGES") {
            config.languages = Some(value.split(',').map(str::trim).map(str::to_owned).collect());
        }
        if let Some(value) = env_value("OBSCURA_GEOLOCATION") {
            let fields = value.split(',').map(str::trim).map(str::parse::<f64>).collect::<Result<Vec<_>, _>>()
                .map_err(|_| "OBSCURA_GEOLOCATION requires latitude,longitude[,accuracy_m]".to_string())?;
            if fields.len() != 2 && fields.len() != 3 { return Err("OBSCURA_GEOLOCATION requires latitude,longitude[,accuracy_m]".into()); }
            config.position = Some(obscura_js::geolocation::Position::new(fields[0], fields[1], fields.get(2).copied().unwrap_or(100.0))?);
        }
        Ok(config)
    }

    pub fn resolve_blocking(self, proxy: Option<String>, stealth: bool, allow_private: bool) -> Result<ResolvedRegion, String> {
        std::thread::Builder::new().name("obscura-region".into()).spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|error| error.to_string())?;
            runtime.block_on(self.resolve(proxy.as_deref(), stealth, allow_private))
        }).map_err(|error| error.to_string())?.join().map_err(|_| "Regional resolver panicked".to_string())?
    }

    /// Resolve once per context, before any page client or isolate is used.
    pub async fn resolve(&self, proxy: Option<&str>, stealth: bool, allow_private: bool) -> Result<ResolvedRegion, String> {
        let mut sources = BTreeMap::new();
        let defaults = obscura_js::regional::inherited_defaults();
        let mut languages = vec![defaults.locale.clone()];
        if let Some((language, _)) = defaults.locale.split_once('-') { languages.push(language.to_owned()); }
        let mut resolved = ResolvedRegion {
            locale: defaults.locale, timezone: env_value("TZ").unwrap_or(defaults.timezone),
            languages, position: None, country: None,
            ip: None, database: None, database_build_epoch: None, territory_data: None,
            sources: BTreeMap::new(), seed: self.seed,
            routing_fingerprint: (self.geoip.trim() == "auto").then(|| routing_fingerprint(proxy)),
        };
        for field in ["locale", "timezone", "languages"] { sources.insert(field.into(), "inherited-default".into()); }
        if !matches!(self.geoip.trim(), "" | "off") {
            let database_path = self.database.as_ref().ok_or("GeoIP requires a local MMDB database")?;
            let database_manifest = read_manifest(self.database_manifest.as_ref().ok_or("GeoIP requires a database licence/provenance manifest")?)?;
            let territory_path = self.territory_data.as_ref().ok_or("GeoIP requires local CLDR territory data")?;
            let territory_manifest = read_manifest(self.territory_manifest.as_ref().ok_or("GeoIP requires a territory-data licence/provenance manifest")?)?;
            let data = load_data(database_path, territory_path, &database_manifest, &territory_manifest)?;
            let ip = if self.geoip.trim() == "auto" {
                let endpoint = self.discovery_url.as_deref().ok_or("auto GeoIP requires an explicit discovery_url")?;
                let endpoint = url::Url::parse(endpoint).map_err(|_| "Invalid GeoIP discovery URL")?;
                if !matches!(endpoint.scheme(), "https" | "http") || !endpoint.username().is_empty() || endpoint.password().is_some() {
                    return Err("GeoIP discovery requires an HTTP(S) URL without credentials".into());
                }
                let jar = Arc::new(obscura_net::CookieJar::new());
                let mut client = obscura_net::ObscuraHttpClient::with_full_options(jar.clone(), proxy, allow_private);
                client.timeout = Duration::from_secs(10);
                let mut request = obscura_net::ResourceRequest::navigation().with_max_response_bytes(256);
                request.credentials = obscura_net::RequestCredentials::Omit;
                // Use the browser's native selected transport and its redirect,
                // DNS, TLS and address policy, but never page cookies/headers.
                let fetch = async {
                    #[cfg(feature = "stealth")]
                    if stealth {
                        let client = obscura_net::StealthHttpClient::with_proxy(jar, proxy, allow_private);
                        return client.fetch_resource_with_callbacks(&endpoint, request, None).await;
                    }
                    let _ = stealth;
                    client.fetch_resource_with_callbacks(&endpoint, request, None).await
                };
                let response = tokio::time::timeout(Duration::from_secs(10), fetch).await
                    .map_err(|_| "GeoIP egress discovery timed out")?
                    .map_err(|_| "GeoIP egress discovery failed through the configured route (no direct fallback)")?;
                if !(200..300).contains(&response.status) || response.body.len() > 256 {
                    return Err("GeoIP discovery requires a successful, bounded literal-IP response".into());
                }
                let address = std::str::from_utf8(&response.body).map_err(|_| "GeoIP discovery response is not UTF-8")?
                    .trim().parse::<IpAddr>().map_err(|_| "GeoIP discovery response must be one literal IP address")?;
                IpProvenance { address, family: if address.is_ipv4() { "IPv4" } else { "IPv6" }.into(),
                    source: if proxy.is_some() { "configured-context-proxy" } else { "configured-client-route" }.into(),
                    observed_at: now(), scope: "Observed route/address family at configuration time only; rotating exits and the other family are not verified".into() }
            } else {
                let address = self.geoip.trim().parse::<IpAddr>().map_err(|_| "GeoIP must be off, auto, or a literal IPv4/IPv6 address")?;
                IpProvenance { address, family: if address.is_ipv4() { "IPv4" } else { "IPv6" }.into(), source: "explicit-ip (regional identity, not transport ownership)".into(), observed_at: now(), scope: "Explicit regional lookup only; no egress discovery".into() }
            };
            let record = data.database.lookup::<Record>(ip.address).map_err(|error| format!("MMDB lookup failed: {error}"))?
                .ok_or("GeoIP database has no record for the selected address")?;
            let country = record.country.and_then(|country| country.iso_code).ok_or("GeoIP record has no country code")?;
            if country.len() != 2 || !country.bytes().all(|byte| byte.is_ascii_uppercase()) { return Err("Invalid GeoIP territory code".into()); }
            let location = record.location.ok_or("GeoIP record has no location")?;
            resolved.position = Some(obscura_js::geolocation::Position::new(
                location.latitude.ok_or("GeoIP record has no latitude")?, location.longitude.ok_or("GeoIP record has no longitude")?,
                f64::from(location.accuracy_radius.ok_or("GeoIP record has unknown accuracy radius; cannot claim precise coordinates")?) * 1000.0)?);
            resolved.timezone = location.time_zone.ok_or("GeoIP record has no timezone")?;
            resolved.locale = select_language(&data.languages, &data.version, &country, self.seed)?;
            let primary = resolved.locale.split('-').next().unwrap_or(&resolved.locale).to_string();
            resolved.languages = vec![resolved.locale.clone()];
            if primary != resolved.locale { resolved.languages.push(primary); }
            resolved.country = Some(country);
            resolved.ip = Some(ip);
            resolved.database_build_epoch = Some(data.database.metadata.build_epoch);
            resolved.database = Some(database_manifest);
            resolved.territory_data = Some(territory_manifest);
            for field in ["locale", "timezone", "languages", "position", "country"] { sources.insert(field.into(), "geoip-local-record/CLDR-population".into()); }
        }
        if let Some(locale) = &self.locale { resolved.locale.clone_from(locale); sources.insert("locale".into(), "explicit-config".into()); }
        if let Some(timezone) = &self.timezone { resolved.timezone.clone_from(timezone); sources.insert("timezone".into(), "explicit-config".into()); }
        if let Some(languages) = &self.languages { resolved.languages.clone_from(languages); sources.insert("languages".into(), "explicit-config".into()); }
        else if self.locale.is_some() { resolved.languages = vec![resolved.locale.clone()]; sources.insert("languages".into(), "explicit-locale".into()); }
        if self.locale.is_none() && self.languages.is_some() {
            resolved.locale = resolved.languages.first().ok_or("Regional languages cannot be empty")?.clone();
            sources.insert("locale".into(), "explicit-language".into());
        }
        if let Some(position) = self.position { position.validate()?; resolved.position = Some(position); sources.insert("position".into(), "explicit-config".into()); }
        obscura_js::regional::validate(&resolved.locale, &resolved.timezone)?;
        if resolved.languages.is_empty() || resolved.languages.len() > 10 { return Err("Regional language list requires 1 to 10 tags".into()); }
        for language in &resolved.languages { obscura_js::regional::validate_language(language)?; }
        resolved.sources = sources;
        Ok(resolved)
    }
}

fn now() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() }
fn read_manifest(path: &PathBuf) -> Result<DataManifest, String> {
    let bytes = read_bounded(path, 64 * 1024)?;
    let manifest: DataManifest = serde_json::from_slice(&bytes).map_err(|error| format!("Invalid data provenance manifest: {error}"))?;
    if [&manifest.supplier, &manifest.version, &manifest.license, &manifest.attribution, &manifest.source, &manifest.update_obligations].iter().any(|value| value.trim().is_empty())
        || manifest.sha256.len() != 64 || !manifest.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("Data manifest requires supplier, version, licence, attribution, source, SHA-256 and update obligations".into());
    }
    if manifest.usable_until.is_some_and(|expiry| expiry <= now()) { return Err("GeoIP/territory data is outside its declared licensed-use/update period".into()); }
    Ok(manifest)
}
fn read_bounded(path: &PathBuf, limit: u64) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path).and_then(|file| file.take(limit + 1).read_to_end(&mut bytes))
        .map_err(|error| format!("Cannot read regional data {}: {error}", path.display()))?;
    if bytes.len() as u64 > limit { return Err(format!("Regional data {} exceeds {limit} bytes", path.display())); }
    Ok(bytes)
}
#[derive(Deserialize)]
struct Record { country: Option<Country>, location: Option<Location> }
#[derive(Deserialize)]
struct Country { iso_code: Option<String> }
#[derive(Deserialize)]
struct Location { latitude: Option<f64>, longitude: Option<f64>, time_zone: Option<String>, accuracy_radius: Option<u16> }
struct Data {
    database: maxminddb::Reader<Vec<u8>>,
    /// Sorted language tags and cumulative integer population weights per territory.
    languages: BTreeMap<String, Vec<(String, u64)>>,
    version: String,
}
fn select_language(languages: &BTreeMap<String, Vec<(String, u64)>>, version: &str, country: &str, seed: u64) -> Result<String, String> {
    let rows = languages.get(country).ok_or("CLDR has no positive population weights for selected territory")?;
    let total = rows.last().ok_or("No weighted languages")?.1;
    // Rejection sampling avoids modulo bias. Population proportions may
    // overlap and need not add to 100; normalize their positive sum only.
    let threshold = total.wrapping_neg() % total;
    for counter in 0u64.. {
        let mut hash = Sha256::new();
        hash.update(b"obscura-territory-population-v1\0"); hash.update(version.as_bytes());
        hash.update([0]); hash.update(country.as_bytes()); hash.update(seed.to_be_bytes()); hash.update(counter.to_be_bytes());
        let digest = hash.finalize();
        let sample = u64::from_be_bytes(digest[..8].try_into().map_err(|_| "Invalid language sample")?);
        if sample < threshold { continue; }
        let target = sample % total;
        let index = rows.partition_point(|(_, cumulative)| *cumulative <= target);
        return Ok(regional_language(&rows[index].0, country));
    }
    unreachable!()
}
fn load_data(database: &PathBuf, territory: &PathBuf, db_manifest: &DataManifest, territory_manifest: &DataManifest) -> Result<Arc<Data>, String> {
    static CACHE: std::sync::LazyLock<std::sync::Mutex<BTreeMap<String, Arc<Data>>>> = std::sync::LazyLock::new(Default::default);
    let key = format!("{}:{}", db_manifest.sha256.to_ascii_lowercase(), territory_manifest.sha256.to_ascii_lowercase());
    let mut cache = CACHE.lock().map_err(|_| "Regional data cache is poisoned")?;
    if let Some(data) = cache.get(&key) { return Ok(data.clone()); }
    let database = read_bounded(database, 1024 * 1024 * 1024)?;
    let territory = read_bounded(territory, 16 * 1024 * 1024)?;
    if !hex::encode(Sha256::digest(&database)).eq_ignore_ascii_case(&db_manifest.sha256)
        || !hex::encode(Sha256::digest(&territory)).eq_ignore_ascii_case(&territory_manifest.sha256) { return Err("Regional data SHA-256 does not match its provenance manifest".into()); }
    let database = maxminddb::Reader::from_source(database).map_err(|error| format!("Invalid MMDB: {error}"))?;
    let xml = std::str::from_utf8(&territory).map_err(|_| "CLDR territory data is not UTF-8")?;
    let languages = parse_populations(xml)?;
    let data = Arc::new(Data { database, languages, version: territory_manifest.sha256.to_ascii_lowercase() });
    // At most four versions resident in the process; live context metadata does
    // not retain a complete database, and lookup results are never re-sampled.
    if cache.len() >= 4 { if let Some(key) = cache.keys().next().cloned() { cache.remove(&key); } }
    cache.insert(key, data.clone());
    Ok(data)
}
fn parse_populations(xml: &str) -> Result<BTreeMap<String, Vec<(String, u64)>>, String> {
    // CLDR declares its external DTD. Permit that declaration but never
    // install an external entity resolver or perform a network fetch.
    let document = roxmltree::Document::parse_with_options(xml, roxmltree::ParsingOptions {
        allow_dtd: true, nodes_limit: 1_000_000, entity_resolver: None,
    }).map_err(|error| format!("Invalid CLDR territory data: {error}"))?;
    let mut result = BTreeMap::new();
    for territory in document.descendants().filter(|node| node.has_tag_name("territory") && node.parent().is_some_and(|parent| parent.has_tag_name("territoryInfo"))) {
        let country = territory.attribute("type").ok_or("Missing CLDR territory type")?;
        let mut weights = BTreeMap::<String, u64>::new();
        for language in territory.children().filter(|node| node.has_tag_name("languagePopulation")) {
            let tag = language.attribute("type").ok_or("Missing CLDR language type")?.replace('_', "-");
            obscura_js::regional::validate_language(&tag)?;
            let percent: f64 = language.attribute("populationPercent").ok_or("Missing CLDR language population")?.parse().map_err(|_| "Invalid CLDR population percentage")?;
            if !percent.is_finite() || !(0.0..=100.0).contains(&percent) { return Err("CLDR population percentage outside 0..100".into()); }
            // Quantize to millionths of a percent, preserving every positive
            // source weight; an explicit zero alone is excluded.
            let weight = if percent == 0.0 { 0 } else { (percent * 1_000_000.0).round().max(1.0) as u64 };
            if weight > 0 { *weights.entry(tag).or_default() += weight; }
        }
        let mut total = 0u64;
        let rows = weights.into_iter().map(|(tag, weight)| { total += weight; (tag, total) }).collect::<Vec<_>>();
        if !rows.is_empty() { result.insert(country.to_owned(), rows); }
    }
    if result.is_empty() { return Err("No CLDR territoryInfo/languagePopulation records".into()); }
    Ok(result)
}

#[derive(Default)]
struct ProcessRegion { profile: Option<Arc<ResolvedRegion>>, claimed: bool }
static PROCESS_REGION: LazyLock<Mutex<ProcessRegion>> = LazyLock::new(Default::default);

pub fn install_process_region(region: ResolvedRegion) -> Result<Arc<ResolvedRegion>, String> {
    obscura_js::regional::configure(&region.locale, &region.timezone)?;
    let region = Arc::new(region);
    let mut installed = PROCESS_REGION.lock().map_err(|_| "Process regional configuration is poisoned")?;
    installed.profile = Some(region.clone());
    installed.claimed = false;
    // Positions/languages can remain context-local while native ICU is pinned.
    Ok(region)
}

fn routing_fingerprint(proxy: Option<&str>) -> String {
    let mut hash = Sha256::new();
    hash.update(proxy.unwrap_or("").as_bytes());
    hex::encode(hash.finalize())
}

/// Hand a prepared launch observation to the first logical context. Every
/// independent auto context re-observes its route, even with the same proxy URL.
pub fn context_region(proxy: Option<String>, stealth: bool, allow_private: bool) -> Result<Arc<ResolvedRegion>, String> {
    let mut installed = PROCESS_REGION.lock().map_err(|_| "Process regional configuration is poisoned")?;
    let inherited = match installed.profile.as_ref() {
        Some(region) => Some(region.clone()),
        None => env_value("OBSCURA_RESOLVED_REGION").map(|serialized| serde_json::from_str::<ResolvedRegion>(&serialized)
            .map(Arc::new).map_err(|error| format!("Invalid resolved regional configuration: {error}"))).transpose()?,
    };
    let region = match inherited {
        Some(region) if region.routing_fingerprint.as_ref().is_none_or(|fingerprint| !installed.claimed && *fingerprint == routing_fingerprint(proxy.as_deref())) => region,
        _ => Arc::new(RegionalConfig::from_env()?.resolve_blocking(proxy, stealth, allow_private)?),
    };
    obscura_js::regional::configure(&region.locale, &region.timezone)?;
    if installed.profile.is_none() { installed.profile = Some(region.clone()); }
    installed.claimed = true;
    Ok(region)
}


fn regional_language(language: &str, country: &str) -> String {
    let has_region = language.split('-').skip(1).any(|part| (part.len() == 2 && part.bytes().all(|byte| byte.is_ascii_alphabetic()))
        || (part.len() == 3 && part.bytes().all(|byte| byte.is_ascii_digit())));
    if has_region { language.to_owned() } else { format!("{language}-{country}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn territory_population_weights_are_stable_overlapping_and_exclude_zero() {
        let xml = r#"<!DOCTYPE supplementalData SYSTEM "unfetched.dtd"><supplementalData><territoryInfo>
            <territory type="SE"><languagePopulation type="sv" populationPercent="95"/>
            <languagePopulation type="en" populationPercent="86"/>
            <languagePopulation type="ia" populationPercent="0"/></territory>
            </territoryInfo></supplementalData>"#;
        let languages = parse_populations(xml).unwrap();
        assert_eq!(languages["SE"], vec![("en".into(), 86_000_000), ("sv".into(), 181_000_000)]);
        let mut english = 0;
        for seed in 0..4096 {
            let selected = select_language(&languages, "licensed-data-version", "SE", seed).unwrap();
            assert_eq!(selected, select_language(&languages, "licensed-data-version", "SE", seed).unwrap());
            match selected.as_str() { "en-SE" => english += 1, "sv-SE" => {}, other => panic!("Zero/unlisted population was selected: {other}") }
        }
        // Independent population expectation: 86/(95+86), not 86/100 and
        // not uniform language sampling. The fixed seed set has a wide margin.
        assert!((1800..2100).contains(&english), "Population weighting produced {english}/4096 English selections");
        assert_eq!(regional_language("zh-Hant", "TW"), "zh-Hant-TW");
        assert_eq!(regional_language("es-419", "MX"), "es-419");
    }

    #[tokio::test]
    async fn off_does_not_open_data_and_explicit_fields_remain_independent() {
        let config = RegionalConfig {
            geoip: "off".into(), database: Some("does-not-exist.mmdb".into()),
            discovery_url: Some("http://127.0.0.1:1/must-not-probe".into()),
            locale: Some("ja-JP".into()), timezone: Some("Asia/Tokyo".into()),
            languages: Some(vec!["fr-CA".into(),"fr".into()]),
            position: Some(obscura_js::geolocation::Position::new(35.0,139.0,2500.0).unwrap()),
            ..Default::default()
        };
        let resolved = config.resolve(Some("http://127.0.0.1:1"),false,false).await.unwrap();
        assert_eq!(resolved.locale,"ja-JP"); assert_eq!(resolved.timezone,"Asia/Tokyo");
        assert_eq!(resolved.accept_language(),"fr-CA,fr;q=0.9");
        assert_eq!(resolved.position.unwrap().accuracy,2500.0);
        assert!(resolved.database.is_none() && resolved.ip.is_none() && resolved.country.is_none());
        assert_eq!(resolved.sources["position"],"explicit-config");
    }
}
