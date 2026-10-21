//! Shared native font registration, CSS face selection, and design metrics.
//!
//! The metadata feature owns this database without shaping or rasterization.
//! Paint clones its fontdb 0.16 database into cosmic-text, preserving face IDs.

use std::{
    collections::{hash_map::DefaultHasher, HashMap, VecDeque},
    hash::{Hash, Hasher},
    path::PathBuf,
    sync::{Arc, LazyLock, Mutex, OnceLock},
};
use fontdb_metadata as fontdb;
use crate::WebFont;
#[cfg(feature = "paint")]
use cosmic_text::FamilyOwned;
// Bundled faces. These keep text layout deterministic when system fonts are
// disabled and provide metric-compatible faces for the ubiquitous
// Arial/Helvetica (`sans-serif`), Times (`serif`), and Courier (`monospace`)
// stacks. Installed families with the authored name always win over these
// aliases; `system-ui` is resolved against the host platform and locale.
pub(crate) static SANS_R: &[u8] = include_bytes!("../assets/liberation-sans.ttf");
static SANS_B: &[u8] = include_bytes!("../assets/liberation-sans-bold.ttf");
static SANS_O: &[u8] = include_bytes!("../assets/liberation-sans-oblique.ttf");
static SANS_BO: &[u8] = include_bytes!("../assets/liberation-sans-boldoblique.ttf");
static SERIF_R: &[u8] = include_bytes!("../assets/liberation-serif.ttf");
static SERIF_B: &[u8] = include_bytes!("../assets/liberation-serif-bold.ttf");
static SERIF_O: &[u8] = include_bytes!("../assets/liberation-serif-oblique.ttf");
static SERIF_BO: &[u8] = include_bytes!("../assets/liberation-serif-boldoblique.ttf");
static MONO_R: &[u8] = include_bytes!("../assets/liberation-mono.ttf");
static MONO_B: &[u8] = include_bytes!("../assets/liberation-mono-bold.ttf");
static MONO_O: &[u8] = include_bytes!("../assets/liberation-mono-oblique.ttf");
static MONO_BO: &[u8] = include_bytes!("../assets/liberation-mono-boldoblique.ttf");
static SYSTEM_R: &[u8] = include_bytes!("../assets/dejavu-sans.ttf");
pub(crate) static SYSTEM_B: &[u8] = include_bytes!("../assets/dejavu-sans-bold.ttf");
static EMOJI_R: &[u8] = include_bytes!("../assets/noto-color-emoji.ttf");
#[cfg(test)]
pub(crate) static FALLBACK: &[u8] = SYSTEM_R;

pub(crate) const FAMILY: &str = "Liberation Sans";
pub(crate) const SERIF_FAMILY: &str = "Liberation Serif";
pub(crate) const MONO_FAMILY: &str = "Liberation Mono";
pub(crate) const SYSTEM_FAMILY: &str = "DejaVu Sans";

/// Static bundled fallback used only when the loaded-family map has no entry
/// for any authored token. `system-ui`/`ui-sans-serif` are resolved against the
/// host platform before this point.
fn resolve_font_family(fam: Option<&str>) -> &'static str {
    let Some(f) = fam else { return FAMILY };
    for tok in f.split(',') {
        if let Some(family) = bundled_family_for_css_token(tok) {
            return family;
        }
        // Unrecognized named webfont: keep scanning for a generic fallback.
    }
    FAMILY
}

/// Map the non-platform generic and legacy family keywords to a bundled face.
/// `system-ui`/`ui-sans-serif` are deliberately absent: they resolve through
/// the host's platform UI face seeded into the loaded-family map, and only
/// fall back to the bundled broad-coverage face when the platform has none.
fn bundled_family_for_css_token(token: &str) -> Option<&'static str> {
    let token = normalize_family_token(token);
    if token.is_empty() {
        return None;
    }
    if token == "monospace"
        || token.contains("mono")
        || token.contains("courier")
        || token.contains("consol")
        || token == "menlo"
        || token == "monaco"
        || token == "code"
    {
        return Some(MONO_FAMILY);
    }
    if token == "serif"
        || token == "ui-serif"
        || token == "georgia"
        || token.contains("times")
        || token == "cambria"
        || token.contains("garamond")
        || token.contains("liberation serif")
        || token == "roman"
    {
        return Some(SERIF_FAMILY);
    }
    if token == "sans-serif"
        || token == "ui-sans-serif"
        || token.contains("sans")
        || token == "arial"
        || token == "helvetica"
        || token == "helvetica neue"
        || token == "-apple-system"
        || token == "roboto"
        || token == "segoe ui"
        || token == "inter"
        || token == "verdana"
        || token == "tahoma"
    {
        return Some(FAMILY);
    }
    None
}

/// Normalize one CSS `font-family` token for case-insensitive matching.
fn normalize_family_token(token: &str) -> String {
    token
        .trim()
        .trim_matches(|c| c == '"' || c == '\'')
        .trim()
        .to_ascii_lowercase()
}

/// CSS `system-ui`/`ui-sans-serif` are platform generic families, not the
/// bundled `DejaVu Sans`.
pub(crate) fn is_system_ui_family_token(token: &str) -> bool {
    let token = normalize_family_token(token);
    matches!(
        token.as_str(),
        "system-ui" | "ui-sans-serif" | "-apple-system" | "blinkmacsystemfont"
    )
}

/// Bundled alias for a CSS family token, used where a rasterizer only accepts
/// family names (SVG). Installed faces are checked before this mapping.
pub(crate) fn bundled_family_alias(token: &str) -> Option<&'static str> {
    bundled_family_for_css_token(token)
}

#[derive(Clone)]
pub(crate) struct LoadedFamily {
    pub(crate) faces: Vec<LoadedFace>,
}

#[derive(Clone)]
pub(crate) struct LoadedFace {
    pub(crate) name: Arc<str>,
    pub(crate) font_id: Option<fontdb::ID>,
    /// Share lazy parsed metrics across aliases and cloned shaping databases.
    pub(crate) metrics: Arc<OnceLock<FaceMetrics>>,
    pub(crate) min_weight: u16,
    pub(crate) max_weight: u16,
    pub(crate) italic: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FaceMetrics {
    pub(crate) ascent: f32,
    pub(crate) descent: f32,
    pub(crate) line_gap: f32,
    pub(crate) units_per_em: f32,
    pub(crate) x_height: f32,
    pub(crate) subscript_offset: Option<f32>,
    pub(crate) superscript_offset: Option<f32>,
}

/// One loaded face plus an optional author-declared family, weight range, and
/// italic flag from an `@font-face` rule.
type FontDeclaration = (
    fontdb::ID,
    Option<String>,
    Option<(u16, u16)>,
    Option<bool>,
);

/// Authored fallback families plus the concrete face selected for each from
/// the engine database. The face id is essential: an `@font-face` resource
/// may be registered under a family whose internal name collides with a
/// different installed face, so re-resolving by name in cosmic's fallback
/// iterator would lose the authored descriptor/resource.
#[cfg(feature = "paint")]
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct FallbackSet {
    families: Vec<FamilyOwned>,
    font_ids: Vec<Option<fontdb::ID>>,
}

#[cfg(feature = "paint")]
impl FallbackSet {
    pub(crate) fn families(&self) -> &[FamilyOwned] {
        &self.families
    }

    pub(crate) fn font_ids(&self) -> &[Option<fontdb::ID>] {
        &self.font_ids
    }
}

#[derive(Clone)]
pub(crate) struct ResolvedFont {
    pub(crate) family: Arc<str>,
    pub(crate) font_id: Option<fontdb::ID>,
    pub(crate) metrics: FaceMetrics,
    pub(crate) synthetic_italic: bool,
    /// Ordered authored fallbacks are only needed by the shaper.
    #[cfg(feature = "paint")]
    pub(crate) fallbacks: Option<Arc<FallbackSet>>,
}


const WEB_FONT_CACHE_ENTRIES: usize = 8;
const WEB_FONT_CACHE_BYTES: usize = 64 * 1024 * 1024;

struct CachedWebFontSet {
    signature: u64,
    load_emoji: bool,
    fonts: Vec<WebFont>,
    bytes: usize,
    database: Arc<FontDatabase>,
}

/// Process-wide font setup captured before the first render.
#[derive(Clone, Debug)]
pub(crate) struct FontConfiguration {
    pub(crate) directories: Vec<PathBuf>,
    pub(crate) system_fonts: bool,
}

#[derive(Clone)]
pub(crate) struct FontDatabase {
    pub(crate) db: fontdb::Database,
    pub(crate) loaded: HashMap<String, LoadedFamily>,
    /// Concrete family `system-ui`/`ui-sans-serif` map to in this database.
    pub(crate) system_ui: Arc<str>,
    pub(crate) locale: String,
}

static FONT_CONFIGURATION: OnceLock<FontConfiguration> = OnceLock::new();
static SYSTEM_LOCALE: LazyLock<String> = LazyLock::new(|| sys_locale::get_locale().unwrap_or_else(|| "en-US".to_string()));
static BASE_FONT_DATABASE: OnceLock<Arc<FontDatabase>> = OnceLock::new();
static EMOJI_FONT_DATABASE: OnceLock<Arc<FontDatabase>> = OnceLock::new();
static WEB_FONT_DATABASES: LazyLock<Mutex<VecDeque<Arc<CachedWebFontSet>>>> = LazyLock::new(Mutex::default);

/// Configure process-wide fonts before the first metadata lookup or render.
///
/// `directories` are additive font directories loaded on top of the bundled
/// faces. `system_fonts` enables the fonts installed on the operating system;
/// it defaults to `true` when this function is never called. Pass `false` for
/// the deterministic bundled-only mode (configured directories still apply).
///
/// Returns false when fonts have already been configured or initialized.
pub fn configure_fonts(directories: Vec<PathBuf>, system_fonts: bool) -> bool {
    if BASE_FONT_DATABASE.get().is_some() {
        return false;
    }
    FONT_CONFIGURATION
        .set(FontConfiguration {
            directories,
            system_fonts,
        })
        .is_ok()
}

fn font_configuration() -> &'static FontConfiguration {
    FONT_CONFIGURATION.get_or_init(|| FontConfiguration {
        directories: Vec::new(),
        // Browser-like installed faces are the library default in every
        // build. Tests that need deterministic geometry construct a bundled
        // database through this same configuration seam instead of changing
        // production behavior.
        system_fonts: true,
    })
}

fn system_locale() -> &'static str {
    SYSTEM_LOCALE.as_str()
}

/// Effective system-font policy for the renderer.
pub(crate) fn effective_system_fonts() -> bool {
    font_configuration().system_fonts
}

pub(crate) fn configured_font_directories() -> &'static [PathBuf] {
    &font_configuration().directories
}

/// Concrete family the process resolved `system-ui` to.
pub(crate) fn system_ui_family() -> Arc<str> {
    Arc::clone(&base_font_database(false).system_ui)
}

fn load_font_directories(
    database: &mut fontdb::Database,
    directories: &[PathBuf],
) -> Vec<fontdb::ID> {
    let mut pending = directories.to_vec();
    let mut files = Vec::new();
    while let Some(directory) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            let path = entry.path();
            if file_type.is_dir() {
                pending.push(path);
            } else if file_type.is_file()
                && path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| {
                        matches!(
                            extension.to_ascii_lowercase().as_str(),
                            "ttf" | "ttc" | "otf" | "otc"
                        )
                    })
            {
                files.push(path);
            }
        }
    }

    files.sort_unstable();
    files.dedup();
    let mut ids = Vec::new();
    for path in files {
        let Ok(data) = std::fs::read(path) else {
            continue;
        };
        ids.extend(database.load_font_source(fontdb::Source::Binary(Arc::new(data))));
    }
    ids
}

pub(crate) fn register_loaded_faces(
    database: &fontdb::Database,
    declarations: Vec<FontDeclaration>,
) -> HashMap<String, LoadedFamily> {
    let mut loaded_families = HashMap::new();
    for (id, declared_family, declared_weight, declared_italic) in declarations {
        let Some(face) = database.face(id) else {
            continue;
        };
        let names = face.families.clone();
        let internal_name = names
            .first()
            .map(|(name, _)| Arc::<str>::from(name.as_str()))
            .unwrap_or_else(|| Arc::from(FAMILY));
        let shape_weight = face.weight.0;
        let metrics = Arc::new(OnceLock::new());
        let italic = declared_italic
            .unwrap_or(!matches!(face.style, fontdb::Style::Normal));
        let weight = declared_weight.unwrap_or((shape_weight, shape_weight));
        let declared_names: Vec<String> = declared_family
            .map(|name| vec![name])
            .unwrap_or_else(|| names.into_iter().map(|(name, _)| name).collect());
        for name in declared_names {
            let family = loaded_families
                .entry(name.to_ascii_lowercase())
                .or_insert_with(|| LoadedFamily { faces: Vec::new() });
            family.faces.push(LoadedFace {
                name: Arc::clone(&internal_name),
                font_id: Some(id),
                metrics: Arc::clone(&metrics),
                min_weight: weight.0,
                max_weight: weight.1,
                italic,
            });
        }
    }
    loaded_families
}

#[cfg(target_os = "windows")]
fn platform_ui_font_family() -> Option<String> {
    use std::mem::size_of;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SystemParametersInfoW, NONCLIENTMETRICSW, SPI_GETNONCLIENTMETRICS,
    };

    let mut metrics: NONCLIENTMETRICSW = unsafe { std::mem::zeroed() };
    metrics.cbSize = size_of::<NONCLIENTMETRICSW>() as u32;
    // This is the same shell message font Chromium asks Windows for when it
    // resolves the CSS `system-ui` family; the process locale can disagree
    // with the display language on multilingual installations.
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETNONCLIENTMETRICS,
            metrics.cbSize,
            &mut metrics as *mut NONCLIENTMETRICSW as *mut core::ffi::c_void,
            0,
        )
    };
    if ok == 0 {
        return None;
    }
    let face = &metrics.lfMessageFont.lfFaceName;
    let len = face
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(face.len());
    let name = String::from_utf16_lossy(&face[..len]);
    (!name.trim().is_empty()).then_some(name)
}

fn platform_system_ui_candidates(locale: &str) -> &'static [&'static str] {
    let locale = locale.to_ascii_lowercase();
    #[cfg(target_os = "windows")]
    {
        if locale.starts_with("zh") {
            if locale.contains("tw")
                || locale.contains("hk")
                || locale.contains("mo")
                || locale.contains("hant")
            {
                return &[
                    "Microsoft JhengHei UI",
                    "Microsoft JhengHei",
                    "Segoe UI",
                    "Tahoma",
                ];
            }
            return &[
                "Microsoft YaHei UI",
                "Microsoft YaHei",
                "Segoe UI",
                "Tahoma",
            ];
        }
        if locale.starts_with("ja") {
            return &[
                "Yu Gothic UI",
                "Yu Gothic",
                "Meiryo UI",
                "Meiryo",
                "Segoe UI",
                "Tahoma",
            ];
        }
        if locale.starts_with("ko") {
            return &["Malgun Gothic", "Segoe UI", "Tahoma"];
        }
        return &["Segoe UI", "Tahoma"];
    }
    #[cfg(target_os = "macos")]
    {
        return &[".AppleSystemUIFont", "SF Pro Text", "Helvetica Neue"];
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "android")))]
    {
        let _ = locale;
        // fontdb populates this from fontconfig when system fonts are loaded;
        // the configured sans family is used before the bundled fallback.
        return &[];
    }
    #[allow(unreachable_code)]
    &[]
}

fn resolve_system_ui_family(
    loaded: &HashMap<String, LoadedFamily>,
    locale: &str,
    platform_sans: Option<&str>,
) -> Arc<str> {
    #[cfg(target_os = "windows")]
    if let Some(ui_family) = platform_ui_font_family() {
        if let Some(family) = loaded.get(&ui_family.to_ascii_lowercase()) {
            if let Some(face) = family.faces.first() {
                return Arc::clone(&face.name);
            }
        }
    }
    for candidate in platform_system_ui_candidates(locale) {
        if let Some(family) = loaded.get(&candidate.to_ascii_lowercase()) {
            if let Some(face) = family.faces.first() {
                return Arc::clone(&face.name);
            }
        }
    }
    if let Some(candidate) = platform_sans {
        if let Some(family) = loaded.get(&candidate.to_ascii_lowercase()) {
            if let Some(face) = family.faces.first() {
                return Arc::clone(&face.name);
            }
        }
    }
    Arc::from(SYSTEM_FAMILY)
}

fn seed_generic_families(loaded: &mut HashMap<String, LoadedFamily>, system_ui: &Arc<str>) {
    let clone_named = |loaded: &HashMap<String, LoadedFamily>, name: &str| {
        loaded.get(&name.to_ascii_lowercase()).cloned()
    };
    if let Some(sans) = clone_named(loaded, FAMILY) {
        loaded.insert("sans-serif".to_string(), sans.clone());
    }
    if let Some(serif) = clone_named(loaded, SERIF_FAMILY) {
        loaded.insert("serif".to_string(), serif.clone());
        loaded.insert("ui-serif".to_string(), serif);
    }
    if let Some(mono) = clone_named(loaded, MONO_FAMILY) {
        loaded.insert("monospace".to_string(), mono.clone());
        loaded.insert("ui-monospace".to_string(), mono);
    }
    if let Some(ui) = clone_named(loaded, system_ui) {
        loaded.insert("system-ui".to_string(), ui.clone());
        loaded.insert("ui-sans-serif".to_string(), ui);
    }
}

pub(crate) fn build_font_database(config: &FontConfiguration) -> FontDatabase {
    let mut database = fontdb::Database::new();
    for bytes in [
        SANS_R, SANS_B, SANS_O, SANS_BO, SERIF_R, SERIF_B, SERIF_O, SERIF_BO, MONO_R, MONO_B, MONO_O,
        MONO_BO, SYSTEM_R, SYSTEM_B,
    ] {
        database.load_font_source(fontdb::Source::Binary(Arc::new(bytes)));
    }
    // Explicit directories are loaded before system faces so operator-supplied
    // fonts win an equal-family comparison.
    load_font_directories(&mut database, &config.directories);

    let platform_sans = if config.system_fonts {
        database.load_system_fonts();
        // A platform font configuration may retarget the generic sans family
        // while scanning (fontconfig). Read it after the scan so `system-ui`
        // prefers the platform's own sans choice over the bundled fallback.
        Some(
            database
                .family_name(&fontdb::Family::SansSerif)
                .to_string(),
        )
    } else {
        None
    };

    let declarations = database
        .faces()
        .map(|face| (face.id, None, None, None))
        .collect();
    let mut loaded = register_loaded_faces(&database, declarations);
    let system_ui = resolve_system_ui_family(
        &loaded,
        system_locale(),
        platform_sans.as_deref(),
    );
    seed_generic_families(&mut loaded, &system_ui);

    // Keep generic queries deterministic in cosmic-text (the same generic
    // names are also seeded into the loaded-family map above).
    database.set_sans_serif_family(FAMILY);
    database.set_serif_family(SERIF_FAMILY);
    database.set_monospace_family(MONO_FAMILY);

    FontDatabase {
        db: database,
        loaded,
        system_ui,
        locale: system_locale().to_string(),
    }
}

#[cfg(test)]
pub(crate) fn bundled_test_database(load_emoji: bool) -> FontDatabase {
    let base = build_font_database(&FontConfiguration {
        directories: Vec::new(),
        system_fonts: false,
    });
    if load_emoji {
        with_emoji_face(&base)
    } else {
        base
    }
}

fn with_emoji_face(base: &FontDatabase) -> FontDatabase {
    let mut database = base.db.clone();
    let declarations = database
        .load_font_source(fontdb::Source::Binary(Arc::new(EMOJI_R)))
        .into_iter()
        .map(|id| (id, None, None, None))
        .collect();
    let loaded_families = register_loaded_faces(&database, declarations);
    let mut all_loaded_families = base.loaded.clone();
    for (name, mut family) in loaded_families {
        all_loaded_families
            .entry(name)
            .or_insert_with(|| LoadedFamily { faces: Vec::new() })
            .faces
            .append(&mut family.faces);
    }
    FontDatabase {
        db: database,
        loaded: all_loaded_families,
        system_ui: Arc::clone(&base.system_ui),
        locale: base.locale.clone(),
    }
}

pub(crate) fn base_font_database(load_emoji: bool) -> &'static Arc<FontDatabase> {
    let base = BASE_FONT_DATABASE.get_or_init(|| Arc::new(build_font_database(font_configuration())));
    if !load_emoji {
        return base;
    }
    EMOJI_FONT_DATABASE.get_or_init(|| Arc::new(with_emoji_face(base)))
}

fn web_font_signature(fonts: &[WebFont], load_emoji: bool) -> u64 {
    let mut hasher = DefaultHasher::new();
    load_emoji.hash(&mut hasher);
    fonts.len().hash(&mut hasher);
    for font in fonts {
        font.family.hash(&mut hasher);
        font.weight.hash(&mut hasher);
        font.italic.hash(&mut hasher);
        let data = font.data.as_slice();
        data.len().hash(&mut hasher);
        data[..data.len().min(64)].hash(&mut hasher);
        if data.len() > 64 {
            data[data.len() - 64..].hash(&mut hasher);
        }
        if data.len() > 128 {
            let middle = data.len() / 2;
            data[middle - 32..middle + 32].hash(&mut hasher);
        }
    }
    hasher.finish()
}

/// Reuse owned font bodies and their registration across geometry and paint.
/// Both entry count and retained web-font bytes are bounded process-wide.
pub(crate) fn font_database(fonts: &[WebFont], load_emoji: bool) -> Arc<FontDatabase> {
    let base = base_font_database(load_emoji);
    if fonts.is_empty() {
        return Arc::clone(base);
    }
    let signature = web_font_signature(fonts, load_emoji);
    let mut cache = WEB_FONT_DATABASES
        .lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(entry) = cache.iter().find(|entry| entry.signature == signature
        && entry.load_emoji == load_emoji && entry.fonts == fonts) {
        return Arc::clone(&entry.database);
    }
    let mut database = (**base).clone();
    let mut declarations = Vec::new();
    for font in fonts {
        for id in database.db.load_font_source(fontdb::Source::Binary(font.data.clone())) {
            declarations.push((id, font.family.clone(), font.weight, font.italic));
        }
    }
    for (name, mut family) in register_loaded_faces(&database.db, declarations) {
        let entry = database.loaded.entry(name)
            .or_insert_with(|| LoadedFamily { faces: Vec::new() });
        family.faces.append(&mut entry.faces);
        entry.faces = family.faces;
    }
    let database = Arc::new(database);
    let bytes = fonts.iter().fold(0usize, |total, font| total.saturating_add(font.data.len()));
    if bytes <= WEB_FONT_CACHE_BYTES {
        while cache.len() >= WEB_FONT_CACHE_ENTRIES || cache.iter().map(|entry| entry.bytes).sum::<usize>()
            > WEB_FONT_CACHE_BYTES.saturating_sub(bytes) {
            cache.pop_front();
        }
        cache.push_back(Arc::new(CachedWebFontSet {
            signature, load_emoji, fonts: fonts.to_vec(), bytes, database: Arc::clone(&database),
        }));
    }
    database
}

fn resolved_stack<'a>(
    fam: Option<&'a str>, requested_weight: u16, requested_italic: bool,
    loaded: &'a HashMap<String, LoadedFamily>, database: &'a fontdb::Database,
) -> impl Iterator<Item = ResolvedFont> + 'a {
    fam.into_iter().flat_map(|stack| stack.split(',')).filter_map(move |token| {
        let name = token.trim().trim_matches(|c| c == '"' || c == '\'').trim();
        let family = loaded.get(&name.to_ascii_lowercase())
            .or_else(|| is_system_ui_family_token(name).then(|| loaded.get("system-ui")).flatten())
            .or_else(|| bundled_family_for_css_token(name)
                .and_then(|family| loaded.get(&family.to_ascii_lowercase())))?;
        select_loaded_face(family, requested_weight, requested_italic, database)
    })
}

fn default_font(fam: Option<&str>, weight: u16, italic: bool,
    loaded: &HashMap<String, LoadedFamily>, database: &fontdb::Database,
) -> ResolvedFont {
    let fallback = resolve_font_family(fam);
    if let Some(font) = loaded.get(&fallback.to_ascii_lowercase())
        .and_then(|family| select_loaded_face(family, weight, italic, database)) {
        return font;
    }
    ResolvedFont {
        family: Arc::from(fallback), font_id: None, metrics: bundled_face_metrics(fallback),
        synthetic_italic: false,
        #[cfg(feature = "paint")]
        fallbacks: None,
    }
}

#[cfg(feature = "paint")]
pub(crate) fn resolve_loaded_font(fam: Option<&str>, weight: u16, italic: bool,
    loaded: &HashMap<String, LoadedFamily>, database: &fontdb::Database,
) -> ResolvedFont {
    let mut stack = resolved_stack(fam, weight, italic, loaded, database);
    let Some(mut primary) = stack.next() else { return default_font(fam, weight, italic, loaded, database) };
    let mut fallbacks = FallbackSet { families: Vec::new(), font_ids: Vec::new() };
    for face in stack {
        let duplicate = match face.font_id {
            Some(id) => primary.font_id == Some(id) || fallbacks.font_ids.contains(&Some(id)),
            None => primary.family.eq_ignore_ascii_case(&face.family) || fallbacks.families.iter().any(|font|
                matches!(font, FamilyOwned::Name(name) if name.eq_ignore_ascii_case(&face.family))),
        };
        if !duplicate {
            fallbacks.families.push(FamilyOwned::Name(face.family.to_string().into()));
            fallbacks.font_ids.push(face.font_id);
        }
    }
    if !fallbacks.families.is_empty() { primary.fallbacks = Some(Arc::new(fallbacks)); }
    primary
}

/// Design metrics of the first selected face, in CSS em units. Selection is
/// identical to text shaping, including declared web-font weight/style ranges.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FontMetrics {
    pub ascent: f32,
    pub descent: f32,
    pub x_height: f32,
    /// Positive downward/upward offsets from OS/2. Missing metadata remains
    /// absent so the layout consumer can apply its CSS fallback explicitly.
    pub subscript_offset: Option<f32>,
    pub superscript_offset: Option<f32>,
}

impl FaceMetrics {
    pub(crate) fn normalized(self) -> FontMetrics {
        let units = self.units_per_em.max(1.0);
        FontMetrics {
            ascent: self.ascent / units, descent: self.descent / units, x_height: self.x_height / units,
            subscript_offset: self.subscript_offset.map(|offset| offset / units),
            superscript_offset: self.superscript_offset.map(|offset| offset / units),
        }
    }
}

/// Metadata-only face lookup. Initializes installed/configured fonts once and
/// shares the bounded decoded-web-font registry with the paint engine.
pub fn selected_font_metrics(family: Option<&str>, weight: u16, italic: bool, fonts: &[WebFont]) -> FontMetrics {
    let database = font_database(fonts, false);
    selected_metrics_from_database(family, weight, italic, &database.loaded, &database.db)
}

pub(crate) fn selected_metrics_from_database(family: Option<&str>, weight: u16, italic: bool,
    loaded: &HashMap<String, LoadedFamily>, database: &fontdb::Database,
) -> FontMetrics {
    resolved_stack(family, weight, italic, loaded, database).next()
        .unwrap_or_else(|| default_font(family, weight, italic, loaded, database)).metrics.normalized()
}

fn loaded_face_metrics(
    database: &fontdb::Database,
    face: &LoadedFace,
) -> FaceMetrics {
    *face.metrics.get_or_init(|| {
        face.font_id
            .and_then(|id| font_metrics(database, id))
            .unwrap_or_else(|| bundled_face_metrics(face.name.as_ref()))
    })
}

pub(crate) fn select_loaded_face(
    family: &LoadedFamily, requested_weight: u16, requested_italic: bool, database: &fontdb::Database,
) -> Option<ResolvedFont> {
    let exact_style = family.faces.iter().any(|face| face.italic == requested_italic);
    let candidates = || family.faces.iter().filter(|face| !exact_style || face.italic == requested_italic);
    let face = candidates().find(|face| (face.min_weight..=face.max_weight).contains(&requested_weight))
        .or_else(|| candidates().min_by_key(|face| weight_rank(requested_weight, face.min_weight)))?;
    Some(ResolvedFont {
        family: Arc::clone(&face.name), font_id: face.font_id,
        metrics: loaded_face_metrics(database, face), synthetic_italic: requested_italic && !face.italic,
        #[cfg(feature = "paint")]
        fallbacks: None,
    })
}

/// CSS Fonts' asymmetric missing-weight search. In particular, 600 selects
/// 700 (not 400) when a family only provides regular and bold faces.
pub(crate) fn match_font_weight(requested: u16, available: &[u16]) -> u16 {
    available.iter().copied().min_by_key(|weight| weight_rank(requested, *weight)).unwrap_or(requested)
}

fn weight_rank(requested: u16, weight: u16) -> (u8, u16) {
    if (400..=500).contains(&requested) {
        if (requested..=500).contains(&weight) { (0, weight - requested) }
        else if weight < requested { (1, requested - weight) }
        else { (2, weight - 500) }
    } else if requested < 400 {
        if weight <= requested { (0, requested - weight) } else { (1, weight - requested) }
    } else if weight >= requested { (0, weight - requested) } else { (1, requested - weight) }
}

/// Check the installed/bundled face selected by the shared font registry.
pub(crate) fn local_font_has_glyph(name: &str, weight: u16, italic: bool, ch: char) -> bool {
    let base = base_font_database(false);
    let family = base.loaded.get(&normalize_family_token(name))
        .or_else(|| is_system_ui_family_token(name).then(|| base.loaded.get("system-ui")).flatten())
        .or_else(|| bundled_family_for_css_token(name)
            .and_then(|family| base.loaded.get(&family.to_ascii_lowercase())));
    let Some(face) = family.and_then(|family| select_loaded_face(family, weight, italic, &base.db))
        .and_then(|font| font.font_id) else { return false; };
    base.db.with_face_data(face, |data, index| {
        ttf_parser::Face::parse(data, index).is_ok_and(|face| face.glyph_index(ch).is_some())
    }).unwrap_or(false)
}

pub(crate) fn bundled_face_metrics(family: &str) -> FaceMetrics {
    static SANS: LazyLock<FaceMetrics> = LazyLock::new(|| bundled_metrics(SANS_R));
    static SERIF: LazyLock<FaceMetrics> = LazyLock::new(|| bundled_metrics(SERIF_R));
    static MONO: LazyLock<FaceMetrics> = LazyLock::new(|| bundled_metrics(MONO_R));
    static SYSTEM: LazyLock<FaceMetrics> = LazyLock::new(|| bundled_metrics(SYSTEM_R));
    match family {
        SERIF_FAMILY => *SERIF, MONO_FAMILY => *MONO,
        SYSTEM_FAMILY => *SYSTEM, _ => *SANS,
    }
}

fn bundled_metrics(bytes: &[u8]) -> FaceMetrics {
    metrics_from_face(&ttf_parser::Face::parse(bytes, 0).expect("valid bundled font"))
}

fn metrics_from_face(face: &ttf_parser::Face<'_>) -> FaceMetrics {
    // OS/2 sxHeight is authoritative when present. Older faces expose the
    // actual lowercase-x outline instead; CSS permits .5em only when neither
    // source supplies a usable x-height.
    let x_height = face.x_height().filter(|height| *height > 0).map(f32::from)
        .or_else(|| face.glyph_index('x').and_then(|glyph| face.glyph_bounding_box(glyph))
            .map(|bounds| f32::from(bounds.y_max)).filter(|height| *height > 0.0))
        .unwrap_or(f32::from(face.units_per_em()) * 0.5);
    FaceMetrics {
        ascent: face.ascender().max(0) as f32, descent: -(face.descender().min(0) as f32),
        line_gap: face.line_gap().max(0) as f32, units_per_em: face.units_per_em() as f32, x_height,
        subscript_offset: face.subscript_metrics().map(|metrics| f32::from(metrics.y_offset)),
        superscript_offset: face.superscript_metrics().map(|metrics| f32::from(metrics.y_offset)),
    }
}

pub(crate) fn font_metrics(db: &fontdb::Database, id: fontdb::ID) -> Option<FaceMetrics> {
    db.with_face_data(id, |data, index| ttf_parser::Face::parse(data, index).ok()
        .map(|face| metrics_from_face(&face))).flatten()
}
