//! ICU defaults are process-global. Pin them before the first isolate and never
//! mutate them for a second profile, even after the original page has closed.
use std::sync::{LazyLock, Mutex};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeRegion { pub locale: String, pub timezone: String }
static REGION: LazyLock<Mutex<Option<NativeRegion>>> = LazyLock::new(|| Mutex::new(None));

pub fn validate_language(language: &str) -> Result<(), String> {
    // Validate BCP47 structure, not a frozen IANA registry: current official
    // CLDR may contain language subtags assigned after this crate's release.
    language.parse::<language_tags::LanguageTag>().map_err(|_| format!("Invalid BCP47 language tag: {language}"))?;
    Ok(())
}
pub fn validate(locale: &str, timezone: &str) -> Result<(), String> {
    validate_language(locale)?;
    timezone.parse::<chrono_tz::Tz>().map_err(|_| format!("Invalid IANA timezone: {timezone}"))?;
    Ok(())
}
pub fn configure(locale: &str, timezone: &str) -> Result<(), String> {
    validate(locale, timezone)?;
    let requested = NativeRegion { locale: locale.to_owned(), timezone: timezone.to_owned() };
    let mut current = REGION.lock().map_err(|_| "Native regional configuration is poisoned")?;
    if let Some(current) = current.as_ref() {
        return if *current == requested { Ok(()) } else {
            Err(format!("Native regional profile conflict: process uses {}/{}, requested {}/{}; launch a separate browser process", current.locale, current.timezone, locale, timezone))
        };
    }
    deno_core::v8::icu::set_default_locale(locale);
    // Apply to ICU on every platform, not TZ only: both Intl and Date use the
    // same native zone, including Windows and Unix with a conflicting host TZ.
    if !deno_core::v8::icu::set_default_time_zone(timezone) { return Err(format!("ICU rejected timezone {timezone}")); }
    *current = Some(requested);
    Ok(())
}

/// Observe defaults without admitting a profile or creating an isolate. A
/// GeoIP-derived profile may still select its native defaults afterwards.
pub fn inherited_defaults() -> NativeRegion {
    let current = REGION.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
    current.unwrap_or_else(|| NativeRegion { locale: "en-US".into(), timezone: deno_core::v8::icu::get_default_time_zone() })
}
pub fn current() -> NativeRegion {
    let mut current = REGION.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    current.get_or_insert_with(|| {
        let timezone = std::env::var("OBSCURA_TIMEZONE").ok().filter(|value| !value.trim().is_empty())
            .or_else(|| std::env::var("TZ").ok().filter(|value| !value.trim().is_empty()))
            .unwrap_or_else(|| deno_core::v8::icu::get_default_time_zone());
        // Legacy direct runtime users inherit the native zone. Public browser
        // configuration validates explicit values before arriving here.
        let locale = "en-US".to_string();
        deno_core::v8::icu::set_default_locale(&locale);
        let timezone = if deno_core::v8::icu::set_default_time_zone(timezone.trim()) {
            timezone.trim().to_string()
        } else {
            deno_core::v8::icu::get_default_time_zone()
        };
        NativeRegion { locale, timezone }
    }).clone()
}
