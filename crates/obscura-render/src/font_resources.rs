//! Native font resource discovery and bounded decoding shared by geometry and
//! paint. HTTP ownership remains with the page transport or supplied byte cache.

use obscura_dom::tree::{DomTree, NodeId};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use crate::{RenderResourceCache, WebFont};
/// One script-created `FontFace` registered with the document's
/// `FontFaceSet`. The JS runtime owns the registry and supplies this snapshot
/// alongside the DOM when preparing a render; keeping it out of the DOM avoids
/// exposing an implementation-only `<style>` element to page selectors.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DynamicFontFace {
    pub family: String,
    pub source: String,
    pub style: String,
    pub weight: String,
    pub unicode_range: String,
}
/// Discover active face declarations once for both paint and metadata. A
/// computed-style snapshot selects actual text demand; metadata-only callers
/// inspect already-loaded faces without initiating layout or network I/O.
fn collect_font_rules(
    tree: &DomTree,
    base_url: Option<&str>,
    mut resolve: impl FnMut(&str, &str) -> (Option<Arc<Vec<u8>>>, bool),
    dynamic_fonts: &[DynamicFontFace],
    viewport: (f32, f32),
    media_type: crate::CssMediaType,
    styles: Option<&HashMap<NodeId, crate::LayoutStyle>>,
) -> Vec<WebFont> {
    struct FontRule {
        sources: Vec<(String, String)>,
        family: Option<String>,
        weight: Option<(u16, u16)>,
        italic: Option<bool>,
        ranges: Vec<(u32, u32)>,
        scope: Option<NodeId>,
    }

    let mut seen = HashSet::new();
    let mut fonts = Vec::new();
    let mut rules = Vec::new();
    let mut collect_rules = |css: &str, scope| {
        for face in crate::css::font_face_blocks_for_viewport_and_media(css, viewport, media_type) {
            let sources: Vec<_> = font_face_urls(&face).into_iter()
                .filter(|src| font_source_may_be_supported(src))
                .map(|src| (font_resource_key(&src, base_url), src)).collect();
            if sources.is_empty() { continue; }
            rules.push(FontRule {
                sources, family: font_face_family(&face), weight: font_face_weight(&face),
                italic: font_face_italic(&face), ranges: font_face_ranges(&face), scope,
            });
        }
    };
    let external = tree.external_stylesheets();
    for nid in crate::dom::rendered_descendants(tree, tree.document()) {
        let Some(node) = tree.borrow_node(nid) else { continue };
        let Some(element) = node.as_element() else { continue };
        let inline = &*element.local == "style";
        let linked = &*element.local == "link" && node.get_attribute("disabled").is_none()
            && node.get_attribute("rel").is_some_and(|rel| rel.split_ascii_whitespace()
                .any(|part| part.eq_ignore_ascii_case("stylesheet")));
        if (!inline && !linked) || !node.get_attribute("media").is_none_or(|media|
            media.trim().is_empty() || crate::css::media_query_applies_for_viewport_and_type(
                media, viewport, media_type)) { continue; }
        drop(node);
        let scope = tree.containing_shadow_root(nid);
        if let Some(sheet) = external.get(&nid) {
            if let Some(cssom) = &sheet.cssom_rules {
                if inline { for source in &sheet.sources { collect_rules(source, scope); } }
                for rule in cssom { collect_rules(rule, scope); }
                continue;
            }
            for source in &sheet.sources { collect_rules(source, scope); }
        }
        if inline { collect_rules(&tree.text_content(nid), scope); }
    }
    drop(collect_rules);
    for face in dynamic_fonts {
        let descriptor_block = format!(
            "src:{};font-weight:{};font-style:{};unicode-range:{}",
            face.source, face.weight, face.style, face.unicode_range
        );
        let sources: Vec<_> = font_face_urls(&descriptor_block)
            .into_iter()
            .filter(|src| font_source_may_be_supported(src))
            .map(|src| (font_resource_key(&src, base_url), src))
            .collect();
        if sources.is_empty() { continue; }
        rules.push(FontRule {
            sources,
            family: (!face.family.is_empty()).then(|| face.family.clone()),
            weight: font_face_weight(&descriptor_block),
            italic: font_face_italic(&descriptor_block),
            ranges: font_face_ranges(&descriptor_block), scope: None,
        });
    }
    let metadata_points = styles.is_none().then(|| document_font_codepoints(tree));
    if let Some(points) = &metadata_points {
        rules.retain(|rule| rule.ranges.iter().any(|&(start, end)|
            font_codepoints_intersect(points, start, end)));
    }

    // Explicit font preloads keep their authored priority and descriptors.
    let mut preloads = Vec::new();
    for nid in crate::dom::rendered_descendants(tree, tree.document()) {
        let preload = tree.with_node(nid, |node| {
            if &*node.as_element()?.local != "link" { return None; }
            let rel = node.get_attribute("rel").unwrap_or("");
            let as_value = node.get_attribute("as").unwrap_or("");
            if rel.split_ascii_whitespace().any(|token| token.eq_ignore_ascii_case("preload"))
                && as_value.eq_ignore_ascii_case("font") {
                node.get_attribute("href").map(str::to_owned)
            } else { None }
        }).flatten();
        if let Some(href) = preload { preloads.push(href); }
    }
    for src in preloads.iter().take(16) {
        let key = font_resource_key(src, base_url);
        if !seen.insert(key.clone()) { continue; }
        if let (Some(decoded), _) = resolve(&key, src) {
            let metadata = rules.iter().find(|rule| rule.sources.iter()
                .any(|(source_key, _)| *source_key == key));
            fonts.push(WebFont {
                data: decoded,
                family: metadata.and_then(|rule| rule.family.clone()),
                weight: metadata.and_then(|rule| rule.weight),
                italic: metadata.and_then(|rule| rule.italic),
            });
        }
    }
    let Some(styles) = styles else {
        for rule in rules {
            if fonts.len() >= 16 { break; }
            for (key, src) in rule.sources {
                if !seen.insert(key.clone()) { continue; }
                if let (Some(decoded), _) = resolve(&key, &src) {
                    fonts.push(WebFont {
                        data: decoded, family: rule.family, weight: rule.weight, italic: rule.italic,
                    });
                    break;
                }
            }
        }
        return fonts;
    };
    if rules.is_empty() { return fonts; }

    // Repeated runs share descriptor and glyph work rather than doing it for
    // every occurrence of a character in a long document.
    let mut demands: HashMap<(Option<NodeId>, String, u16, bool), HashSet<char>> = HashMap::new();
    let mut hidden = HashSet::new();
    for id in crate::dom::rendered_descendants(tree, tree.document()) {
        let parent = crate::dom::rendered_parent(tree, id);
        if parent.is_some_and(|parent| hidden.contains(&parent))
            || styles.get(&id).is_some_and(|style| style.display == crate::Display::None) {
            hidden.insert(id);
            continue;
        }
        let Some(node) = tree.borrow_node(id) else { continue; };
        let mut add = |style: &crate::LayoutStyle, text: &str, scope| {
            if style.visibility_hidden == Some(true) { return; }
            let Some(family) = style.font_family.as_ref() else { return; };
            let entry = demands.entry((scope, family.clone(), crate::style::used_font_weight(style),
                style.font_style_italic.unwrap_or(false))).or_default();
            entry.extend(text.chars().filter(|ch| !ch.is_control()));
        };
        if let Some(text) = node.text_content_of_text_node() {
            if let Some(style) = parent.and_then(|parent| styles.get(&parent)) {
                add(style, text, tree.containing_shadow_root(id));
            }
        } else if let Some(style) = styles.get(&id) {
            for pseudo in [style.before_pseudo.as_deref(), style.after_pseudo.as_deref()].into_iter().flatten() {
                if pseudo.display != crate::Display::None {
                    if let Some(text) = pseudo.before_content.as_deref() {
                        add(pseudo, text, tree.containing_shadow_root(id));
                    }
                }
            }
            if node.as_element().is_some_and(|element| matches!(&*element.local, "input" | "textarea")) {
                let text = node.get_attribute("value").filter(|text| !text.is_empty())
                    .or_else(|| node.get_attribute("placeholder")).unwrap_or("");
                add(style, text, tree.containing_shadow_root(id));
            }
        }
    }
    let mut loaded: HashMap<usize, (Option<Arc<Vec<u8>>>, bool)> = HashMap::new();
    let mut selected = std::collections::BTreeSet::new();
    for ((scope, stack, weight, italic), points) in demands {
        let families = split_css_top_level(&stack, ',');
        for ch in points {
            for family in &families {
                let name = family.trim().trim_matches(['\'', '"']).trim();
                let mut candidates: Vec<_> = rules.iter().enumerate().filter(|(_, rule)| {
                    rule.scope == scope && rule.family.as_deref().is_some_and(|family| family.eq_ignore_ascii_case(name))
                        && rule.ranges.iter().any(|&(start, end)| (start..=end).contains(&(ch as u32)))
                }).map(|(index, _)| index).collect();
                if candidates.is_empty() {
                    if crate::fonts::local_font_has_glyph(name, weight, italic, ch) { break; }
                    continue;
                }
                let has_style = candidates.iter().any(|&index| rules[index].italic.unwrap_or(false) == italic);
                if has_style { candidates.retain(|&index| rules[index].italic.unwrap_or(false) == italic); }
                let weights: Vec<_> = candidates.iter().map(|&index| {
                    let (min, max) = rules[index].weight.unwrap_or((400, 400));
                    weight.clamp(min, max)
                }).collect();
                let matched = crate::fonts::match_font_weight(weight, &weights);
                let Some(index) = candidates.into_iter().rev().find(|&index| {
                    let (min, max) = rules[index].weight.unwrap_or((400, 400));
                    (min..=max).contains(&matched)
                }) else { continue; };
                if loaded.len() >= 16usize.saturating_sub(fonts.len()) && !loaded.contains_key(&index) {
                    break;
                }
                let (decoded, pending) = loaded.entry(index).or_insert_with(|| {
                    for (key, src) in &rules[index].sources {
                        let outcome = resolve(key, src);
                        // A preferred unknown source is pending, not failed.
                        if outcome.0.is_some() || outcome.1 { return outcome; }
                    }
                    (None, false)
                });
                let covers = decoded.as_ref().is_some_and(|bytes| {
                    let count = ttf_parser::fonts_in_collection(bytes).unwrap_or(1);
                    (0..count).any(|index| ttf_parser::Face::parse(bytes, index)
                        .is_ok_and(|face| face.glyph_index(ch).is_some()))
                });
                if covers || *pending {
                    selected.insert(index);
                    break;
                }
            }
        }
    }
    for index in selected {
        if fonts.len() >= 16 { break; }
        let rule = &rules[index];
        let Some((Some(data), _)) = loaded.remove(&index) else { continue; };
        if fonts.iter().any(|font| font.family == rule.family && font.weight == rule.weight
            && font.italic == rule.italic && Arc::ptr_eq(&font.data, &data)) { continue; }
        fonts.push(WebFont {
            data, family: rule.family.clone(), weight: rule.weight, italic: rule.italic,
        });
    }
    fonts
}

#[cfg(feature = "paint")]
pub(crate) fn collect_web_fonts(tree: &DomTree, base_url: Option<&str>, cache: &mut RenderResourceCache,
    dynamic_fonts: &[DynamicFontFace], viewport: (f32, f32), media: crate::CssMediaType,
    styles: &HashMap<NodeId, crate::LayoutStyle>,
) -> Vec<WebFont> {
    collect_font_rules(tree, base_url, |key, src| {
        let decoded = fetch_and_decode_font(src, base_url, cache);
        let pending = decoded.is_none() && !src.starts_with("data:") && !src.starts_with("obscura-font:")
            && !cache.has_live_outcome(key);
        (decoded, pending)
    }, dynamic_fonts, viewport, media, Some(styles))
}

/// Exclude source formats that fontdb cannot consume before issuing a request.
/// Unknown and extensionless URLs are retained because their response bytes can
/// still identify a supported sfnt, WOFF, or WOFF2 resource.
fn font_source_may_be_supported(src: &str) -> bool {
    let path = src
        .split(['?', '#'])
        .next()
        .unwrap_or(src)
        .to_ascii_lowercase();
    !path.ends_with(".eot") && !path.ends_with(".svg")
}

fn font_resource_key(src: &str, base_url: Option<&str>) -> String {
    url::Url::parse(src)
        .ok()
        .or_else(|| {
            base_url
                .and_then(|base| url::Url::parse(base).ok())
                .and_then(|base| base.join(src).ok())
        })
        .map(|url| url.to_string())
        .unwrap_or_else(|| src.to_string())
}

#[cfg(feature = "paint")]
pub(crate) fn fetch_and_decode_font(
    src: &str,
    base_url: Option<&str>,
    cache: &mut RenderResourceCache,
) -> Option<std::sync::Arc<Vec<u8>>> {
    let key = font_resource_key(src, base_url);
    if let Some(decoded) = cache.cached_decoded_font(&key) { return Some(decoded); }
    let compressed = if src.starts_with("obscura-font:") {
        cache.cached_font_bytes(src)?
    } else if src.starts_with("data:") {
        Arc::from(font_data_url_bytes(src)?)
    } else {
        cache.get_or_load(&key, true)?
    };
    let decoded = Arc::new(decode_font_bytes(&compressed)?.into_owned());
    if !cache.has_cached_bytes(&key) { cache.seed_shared(key.clone(), compressed); }
    cache.cache_decoded_font(&key, decoded.clone());
    Some(decoded)
}

/// Decode and validate the same real fonts used by shaping. Raw sfnt data is
/// borrowed so a lifecycle check does not copy a complete font body.
pub fn decode_font_bytes(bytes: &[u8]) -> Option<std::borrow::Cow<'_, [u8]>> {
    if bytes.len() > FONT_MAX_DECODED {
        return None;
    }
    let decoded = match bytes.get(..4) {
        Some(b"wOF2") => {
            let bounds = woff2_allocation_bounds(bytes)?;
            let decoded = wuff::decompress_woff2_with_custom_brotli(bytes, &mut |compressed, size| {
                let invalid = || Box::new(wuff::WuffErr::GenericError) as Box<dyn std::error::Error>;
                if size != bounds.stream_size || size > FONT_MAX_DECODED {
                    return Err(invalid());
                }
                use brotli_decompressor::{BrotliDecompressStream, BrotliResult, BrotliState, StandardAlloc};
                let mut output = vec![0; size];
                let mut available_in = compressed.len();
                let mut input_offset = 0;
                let mut available_out = size;
                let mut output_offset = 0;
                let mut total_out = 0;
                let mut state = BrotliState::new(StandardAlloc::default(), StandardAlloc::default(), StandardAlloc::default());
                let result = BrotliDecompressStream(&mut available_in, &mut input_offset, compressed,
                    &mut available_out, &mut output_offset, &mut output, &mut total_out, &mut state);
                if !matches!(result, BrotliResult::ResultSuccess) || output_offset != size
                    || woff2_reconstruction_bounds(&bounds, &output).is_none() {
                    return Err(invalid());
                }
                Ok(output)
            }).ok()?;
            std::borrow::Cow::Owned(decoded)
        }
        Some(b"wOFF") => {
            woff1_allocation_bounds(bytes)?;
            std::borrow::Cow::Owned(wuff::decompress_woff1(bytes).ok()?)
        }
        Some(b"\0\x01\0\0" | b"OTTO" | b"ttcf") => std::borrow::Cow::Borrowed(bytes),
        _ => return None,
    };
    if decoded.len() > FONT_MAX_DECODED {
        return None;
    }
    let count = ttf_parser::fonts_in_collection(&decoded).unwrap_or(1);
    // Collections must have genuine sfnt faces, not just the right magic.
    if count == 0 || count > 64 || !(0..count).all(|index| {
        ttf_parser::Face::parse(&decoded, index).is_ok_and(|face| {
            let tables = face.tables();
            face.number_of_glyphs() > 0 && tables.cmap.is_some()
                && (tables.glyf.is_some() || tables.cff.is_some() || tables.cff2.is_some())
        })
    }) {
        return None;
    }
    Some(decoded)
}

const FONT_MAX_COMPRESSED: usize = 8 * 1024 * 1024;
const FONT_MAX_DECODED: usize = 32 * 1024 * 1024;

fn font_take<'a>(input: &mut &'a [u8], size: usize) -> Option<&'a [u8]> {
    let (value, rest) = input.split_at_checked(size)?;
    *input = rest;
    Some(value)
}

fn font_u16(input: &mut &[u8]) -> Option<usize> {
    Some(u16::from_be_bytes(font_take(input, 2)?.try_into().ok()?) as usize)
}

fn font_u32(input: &mut &[u8]) -> Option<usize> {
    Some(u32::from_be_bytes(font_take(input, 4)?.try_into().ok()?) as usize)
}

fn font_base128(input: &mut &[u8]) -> Option<usize> {
    let mut value = 0u32;
    for index in 0..5 {
        let byte = font_take(input, 1)?[0];
        if index == 0 && byte == 0x80 || value & 0xfe000000 != 0 {
            return None;
        }
        value = (value << 7) | u32::from(byte & 0x7f);
        if byte & 0x80 == 0 { return Some(value as usize); }
    }
    None
}

fn font_255(input: &mut &[u8]) -> Option<usize> {
    match font_take(input, 1)?[0] {
        253 => font_u16(input),
        254 => Some(506 + usize::from(font_take(input, 1)?[0])),
        255 => Some(253 + usize::from(font_take(input, 1)?[0])),
        value => Some(usize::from(value)),
    }
}

fn font_padded(size: usize) -> Option<usize> {
    Some(size.checked_add(3)? & !3)
}

fn font_bounded_add(total: &mut usize, size: usize) -> Option<()> {
    *total = total.checked_add(size)?;
    (*total <= FONT_MAX_DECODED).then_some(())
}

fn woff1_allocation_bounds(bytes: &[u8]) -> Option<()> {
    if bytes.len() > FONT_MAX_COMPRESSED || bytes.len() < 44 { return None; }
    let mut header = &bytes[8..];
    if font_u32(&mut header)? != bytes.len() { return None; }
    let count = font_u16(&mut header)?;
    if count == 0 || font_u16(&mut header)? != 0 { return None; }
    let advertised = font_u32(&mut header)?;
    if advertised == 0 || advertised > FONT_MAX_DECODED { return None; }
    let mut directory = bytes.get(44..44usize.checked_add(count.checked_mul(20)?)?)?;
    let mut reconstructed = 12 + count * 16;
    for _ in 0..count {
        font_take(&mut directory, 4)?;
        let offset = font_u32(&mut directory)?;
        let compressed = font_u32(&mut directory)?;
        let original = font_u32(&mut directory)?;
        font_take(&mut directory, 4)?;
        if compressed > original || offset < 44 + count * 20 {
            return None;
        }
        bytes.get(offset..offset.checked_add(compressed)?)?;
        font_bounded_add(&mut reconstructed, font_padded(original)?)?;
    }
    (reconstructed == advertised).then_some(())
}

struct Woff2BoundTable {
    tag: Option<[u8; 4]>,
    transformed: bool,
    original: usize,
    offset: usize,
    length: usize,
}

struct Woff2AllocationBounds {
    tables: Vec<Woff2BoundTable>,
    fonts: Vec<Vec<usize>>,
    stream_size: usize,
    header_size: usize,
}

fn woff2_allocation_bounds(bytes: &[u8]) -> Option<Woff2AllocationBounds> {
    if bytes.len() > FONT_MAX_COMPRESSED || bytes.len() < 48 { return None; }
    let mut header = &bytes[8..];
    if font_u32(&mut header)? != bytes.len() { return None; }
    let count = font_u16(&mut header)?;
    font_u16(&mut header)?;
    let advertised = font_u32(&mut header)?;
    if count == 0 || advertised == 0 || advertised > FONT_MAX_DECODED { return None; }
    let compressed_size = font_u32(&mut header)?;
    let mut directory = &bytes[48..];
    let mut tables = Vec::with_capacity(count.min(directory.len() / 2));
    let mut stream_size = 0;
    let mut original_size = 0;
    for _ in 0..count {
        let flags = font_take(&mut directory, 1)?[0];
        let tag = match flags & 63 {
            2 => Some(*b"hhea"), 3 => Some(*b"hmtx"),
            10 => Some(*b"glyf"), 11 => Some(*b"loca"),
            63 => Some(font_take(&mut directory, 4)?.try_into().ok()?),
            _ => None,
        };
        let version = flags >> 6;
        let transformed = if matches!(tag.as_ref(), Some(b"glyf" | b"loca")) {
            version == 0
        } else { version != 0 };
        let original = font_base128(&mut directory)?;
        let length = if transformed { font_base128(&mut directory)? } else { original };
        if transformed && (tag == Some(*b"loca") && length != 0
            || !matches!(tag.as_ref(), Some(b"glyf" | b"loca" | b"hmtx"))) {
            return None;
        }
        let offset = stream_size;
        font_bounded_add(&mut stream_size, length)?;
        font_bounded_add(&mut original_size, font_padded(original)?)?;
        tables.push(Woff2BoundTable { tag, transformed, original, offset, length });
    }
    let mut fonts = Vec::new();
    let mut header_size;
    let capacity_header;
    if bytes.get(4..8)? == b"ttcf" {
        let version = font_u32(&mut directory)?;
        if version != 0x10000 && version != 0x20000 { return None; }
        let font_count = font_255(&mut directory)?;
        if font_count == 0 || font_count > 64 { return None; }
        header_size = 12 + font_count * 4 + if version == 0x20000 { 12 } else { 0 };
        capacity_header = header_size;
        for _ in 0..font_count {
            let table_count = font_255(&mut directory)?;
            font_take(&mut directory, 4)?;
            if table_count == 0 || table_count > count { return None; }
            font_bounded_add(&mut header_size, 12 + table_count * 16)?;
            let mut indices = Vec::with_capacity(table_count.min(directory.len()));
            for _ in 0..table_count {
                let index = font_255(&mut directory)?;
                if index >= count { return None; }
                indices.push(index);
            }
            fonts.push(indices);
        }
    } else {
        // wuff's synthetic single-font directory includes a collection-header
        // capacity hint although the emitted sfnt header has only 12 bytes.
        header_size = 12 + count * 16;
        capacity_header = 16;
        fonts.push((0..count).collect());
    }
    font_bounded_add(&mut original_size, header_size)?;
    let capacity_size = original_size.checked_add(capacity_header)?;
    if capacity_size > FONT_MAX_DECODED && !(stream_size <= advertised && advertised <= capacity_size) {
        return None;
    }
    font_take(&mut directory, compressed_size)?;
    Some(Woff2AllocationBounds { tables, fonts, stream_size, header_size })
}

fn woff2_reconstruction_bounds(bounds: &Woff2AllocationBounds, stream: &[u8]) -> Option<()> {
    let mut sizes = Vec::with_capacity(bounds.tables.len());
    let mut glyphs = Vec::with_capacity(bounds.tables.len());
    for table in &bounds.tables {
        let data = stream.get(table.offset..table.offset.checked_add(table.length)?)?;
        if table.transformed && table.tag == Some(*b"glyf") {
            let (glyf, loca, count) = woff2_glyf_allocation_bounds(data)?;
            sizes.push(glyf);
            glyphs.push(Some((loca, count)));
        } else {
            sizes.push(table.length);
            glyphs.push(None);
        }
    }
    for indices in &bounds.fonts {
        let find = |tag| indices.iter().copied().find(|index| bounds.tables[*index].tag == Some(tag));
        if let Some(glyf_index) = find(*b"glyf") {
            if let Some((loca_size, count)) = glyphs[glyf_index] {
                let loca_index = find(*b"loca")?;
                let loca = &bounds.tables[loca_index];
                if !loca.transformed || loca.original != loca_size { return None; }
                sizes[loca_index] = loca_size;
                if let Some(hmtx_index) = find(*b"hmtx") {
                    if bounds.tables[hmtx_index].transformed {
                        let hhea = &bounds.tables[find(*b"hhea")?];
                        let mut metrics = stream.get(hhea.offset.checked_add(34)?..hhea.offset.checked_add(hhea.length)?)?;
                        let metrics = font_u16(&mut metrics)?;
                        if metrics == 0 || metrics > count { return None; }
                        let actual = count * 2 + metrics * 2;
                        if bounds.tables[hmtx_index].original != actual { return None; }
                        sizes[hmtx_index] = actual;
                    }
                }
            }
        }
    }
    let mut total = bounds.header_size;
    for size in sizes { font_bounded_add(&mut total, font_padded(size)?)?; }
    Some(())
}

// This scanner does not decode a font. It accounts for the exact bytes wuff
// will reconstruct and rejects oversized scratch/output allocations before
// entering its glyf transformer, including dishonest origLength values.
fn woff2_glyf_allocation_bounds(data: &[u8]) -> Option<(usize, usize, usize)> {
    let mut header = data;
    font_u16(&mut header)?;
    let flags = font_u16(&mut header)?;
    let count = font_u16(&mut header)?;
    let index_format = font_u16(&mut header)?;
    if index_format > 1 { return None; }
    let mut lengths = [0; 7];
    for length in &mut lengths { *length = font_u32(&mut header)?; }
    let mut payload = data.get(36..)?;
    let mut contours = font_take(&mut payload, lengths[0])?;
    let mut points = font_take(&mut payload, lengths[1])?;
    let mut point_flags = font_take(&mut payload, lengths[2])?;
    let mut triplets = font_take(&mut payload, lengths[3])?;
    let mut composites = font_take(&mut payload, lengths[4])?;
    let mut bbox = font_take(&mut payload, lengths[5])?;
    let mut instructions = font_take(&mut payload, lengths[6])?;
    let bitmap = font_take(&mut bbox, count.div_ceil(32) * 4)?;
    let overlap = if flags & 1 != 0 { Some(font_take(&mut payload, count.div_ceil(8))?) } else { None };
    let mut output = 0;
    for glyph in 0..count {
        let contour_count = font_u16(&mut contours)?;
        let has_bbox = bitmap[glyph / 8] & (0x80 >> (glyph % 8)) != 0;
        let mut glyph_size = 0;
        if contour_count == 0xffff {
            if !has_bbox { return None; }
            font_take(&mut bbox, 8)?;
            glyph_size = 10;
            let mut has_instructions = false;
            loop {
                let flags = font_u16(&mut composites)?;
                has_instructions |= flags & 0x100 != 0;
                let size = 2 + if flags & 1 != 0 { 4 } else { 2 }
                    + if flags & 8 != 0 { 2 } else if flags & 0x40 != 0 { 4 }
                      else if flags & 0x80 != 0 { 8 } else { 0 };
                font_take(&mut composites, size)?;
                font_bounded_add(&mut glyph_size, 2 + size)?;
                if flags & 0x20 == 0 { break; }
            }
            if has_instructions {
                let size = font_255(&mut triplets)?;
                font_take(&mut instructions, size)?;
                font_bounded_add(&mut glyph_size, 2 + size)?;
            }
        } else if contour_count > 0 {
            let mut point_count = 0usize;
            for _ in 0..contour_count {
                point_count = point_count.checked_add(font_255(&mut points)?)?;
                // wuff allocates its point vector before validating endPts.
                if point_count > 65536 { return None; }
            }
            let source_flags = font_take(&mut point_flags, point_count)?;
            glyph_size = 12 + contour_count * 2;
            let mut last_flag = None;
            let mut run = 0;
            let mut x = 0i32;
            let mut y = 0i32;
            for (point, &source_flag) in source_flags.iter().enumerate() {
                let (dx, dy) = woff2_triplet_delta(source_flag & 0x7f, &mut triplets)?;
                x = x.checked_add(dx)?;
                y = y.checked_add(dy)?;
                let axis = |delta: i32, short, same| {
                    if delta == 0 { (same, 0) }
                    else if (-255..=255).contains(&delta) { (short | if delta > 0 { same } else { 0 }, 1) }
                    else { (0, 2) }
                };
                let (x_flag, x_bytes) = axis(dx, 2, 16);
                let (y_flag, y_bytes) = axis(dy, 4, 32);
                let flag = u8::from(source_flag & 0x80 == 0) | x_flag | y_flag
                    | if point == 0 && overlap.is_some_and(|bitmap| bitmap[glyph / 8] & (0x80 >> (glyph % 8)) != 0) { 64 } else { 0 };
                font_bounded_add(&mut glyph_size, x_bytes + y_bytes)?;
                if last_flag == Some(flag) && run < 256 { run += 1; }
                else {
                    if run > 0 { font_bounded_add(&mut glyph_size, if run == 1 { 1 } else { 2 })?; }
                    last_flag = Some(flag);
                    run = 1;
                }
            }
            if run > 0 { font_bounded_add(&mut glyph_size, if run == 1 { 1 } else { 2 })?; }
            let instruction_size = font_255(&mut triplets)?;
            font_take(&mut instructions, instruction_size)?;
            font_bounded_add(&mut glyph_size, instruction_size)?;
            if has_bbox { font_take(&mut bbox, 8)?; }
            // Account for the transformer's per-glyph reserve, not only the
            // tighter encoded output size calculated above.
            if 12 + contour_count * 2 + point_count * 5 + instruction_size > FONT_MAX_DECODED { return None; }
        } else if has_bbox { return None; }
        font_bounded_add(&mut output, font_padded(glyph_size)?)?;
    }
    if index_format == 0 && output > 0x1fffe { return None; }
    Some((output, (count + 1) * if index_format == 0 { 2 } else { 4 }, count))
}

fn woff2_triplet_delta(flag: u8, input: &mut &[u8]) -> Option<(i32, i32)> {
    let sign = |flag: u8, value: i32| if flag & 1 != 0 { value } else { -value };
    let length = if flag < 84 { 1 } else if flag < 120 { 2 } else if flag < 124 { 3 } else { 4 };
    let data = font_take(input, length)?;
    let (x, y) = if flag < 10 {
        (0, (i32::from(flag & 14) << 7) + i32::from(data[0]))
    } else if flag < 20 {
        ((i32::from((flag - 10) & 14) << 7) + i32::from(data[0]), 0)
    } else if flag < 84 {
        let bits = flag - 20;
        (1 + i32::from(bits & 0x30) + i32::from(data[0] >> 4),
         1 + i32::from((bits & 0x0c) << 2) + i32::from(data[0] & 0x0f))
    } else if flag < 120 {
        let bits = flag - 84;
        (1 + (i32::from(bits / 12) << 8) + i32::from(data[0]),
         1 + (i32::from((bits % 12) >> 2) << 8) + i32::from(data[1]))
    } else if flag < 124 {
        ((i32::from(data[0]) << 4) + i32::from(data[1] >> 4),
         (i32::from(data[1] & 0x0f) << 8) + i32::from(data[2]))
    } else {
        ((i32::from(data[0]) << 8) + i32::from(data[1]),
         (i32::from(data[2]) << 8) + i32::from(data[3]))
    };
    Some((sign(flag, x), sign(if flag < 10 { flag } else { flag >> 1 }, y)))
}


/// Resolve FontFace sources with the renderer's source-order/format policy.
pub fn font_face_source_urls(source: &str, base_url: Option<&str>) -> Vec<String> {
    font_face_urls(&format!("src:{source}"))
        .into_iter()
        .filter(|src| font_source_may_be_supported(src))
        .take(32)
        .filter_map(|src| url::Url::parse(&src).ok().or_else(|| {
            base_url.and_then(|base| url::Url::parse(base).ok())
                .and_then(|base| base.join(&src).ok())
        }))
        .map(|mut url| { url.set_fragment(None); url.to_string() })
        .collect()
}

/// Decode data-backed fonts without allocating beyond the compressed limit.
pub fn font_data_url_bytes(source: &str) -> Option<Vec<u8>> {
    const MAX: usize = 8 * 1024 * 1024;
    let rest = source.strip_prefix("data:")?;
    let (meta, data) = rest.split_once(',')?;
    if meta.split(';').any(|label| label.eq_ignore_ascii_case("base64")) {
        if data.len() > MAX.div_ceil(3) * 4 {
            return None;
        }
        use base64::Engine;
        let mut bytes = vec![0; (data.len().div_ceil(4) * 3).min(MAX)];
        let length = base64::engine::general_purpose::STANDARD.decode_slice(data, &mut bytes).ok()?;
        bytes.truncate(length);
        Some(bytes)
    } else {
        if data.len() > MAX * 3 {
            return None;
        }
        let input = data.as_bytes();
        let mut bytes = Vec::with_capacity(input.len().min(MAX));
        let mut index = 0;
        while index < input.len() {
            if bytes.len() == MAX { return None; }
            if input[index] == b'%' && index + 2 < input.len() {
                let hi = (input[index + 1] as char).to_digit(16);
                let lo = (input[index + 2] as char).to_digit(16);
                if let (Some(hi), Some(lo)) = (hi, lo) {
                    bytes.push((hi * 16 + lo) as u8);
                    index += 3;
                    continue;
                }
            }
            bytes.push(input[index]);
            index += 1;
        }
        Some(bytes)
    }
}


fn font_face_declaration<'a>(face: &'a str, name: &str) -> Option<&'a str> {
    split_css_top_level(face, ';')
        .into_iter()
        .filter_map(|declaration| {
            let (property, value) = declaration.split_once(':')?;
            property
                .trim()
                .eq_ignore_ascii_case(name)
                .then_some(value.trim())
        })
        .last()
}

pub(crate) fn font_face_family(face: &str) -> Option<String> {
    font_face_declaration(face, "font-family")
        .map(|family| {
            family
                .trim()
                .trim_matches(|ch| matches!(ch, '"' | '\''))
                .to_string()
        })
        .filter(|family| !family.is_empty())
}

pub(crate) fn font_face_weight(face: &str) -> Option<(u16, u16)> {
    fn parse(value: &str) -> Option<u16> {
        match value.to_ascii_lowercase().as_str() {
            "normal" => Some(400),
            "bold" => Some(700),
            value => value
                .parse::<f32>()
                .ok()
                .filter(|weight| weight.is_finite() && (1.0..=1000.0).contains(weight))
                .map(|weight| weight.round() as u16),
        }
    }
    let mut values = font_face_declaration(face, "font-weight")?
        .split_ascii_whitespace()
        .filter_map(parse);
    let first = values.next()?;
    let second = values.next().unwrap_or(first);
    Some((first.min(second), first.max(second)))
}

pub(crate) fn font_face_italic(face: &str) -> Option<bool> {
    font_face_declaration(face, "font-style").and_then(|style| {
        let style = style.trim().to_ascii_lowercase();
        if style == "normal" {
            Some(false)
        } else if style == "italic" || style.starts_with("oblique") {
            Some(true)
        } else {
            None
        }
    })
}

pub(crate) fn font_face_ranges(face: &str) -> Vec<(u32, u32)> {
    let Some(range) = font_face_declaration(face, "unicode-range") else {
        return vec![(0, 0x10ffff)];
    };
    range.split(',').filter_map(|part| {
        let token = part.trim().to_ascii_lowercase();
        let value = token.strip_prefix("u+")?;
        let (start, end) = if value.contains('?') {
            (u32::from_str_radix(&value.replace('?', "0"), 16).ok()?,
                u32::from_str_radix(&value.replace('?', "f"), 16).ok()?)
        } else if let Some((start, end)) = value.split_once('-') {
            (u32::from_str_radix(start, 16).ok()?, u32::from_str_radix(end, 16).ok()?)
        } else {
            let point = u32::from_str_radix(value, 16).ok()?;
            (point, point)
        };
        (start <= end && start <= 0x10ffff).then_some((start, end.min(0x10ffff)))
    }).collect()
}


fn font_codepoints_intersect(points: &[u64], start: u32, end: u32) -> bool {
    let first = start as usize / 64;
    let last = (end as usize / 64).min(points.len().saturating_sub(1));
    if first > last || points.is_empty() {
        return false;
    }
    (first..=last).any(|index| {
        let low = if index == first { u64::MAX << (start % 64) } else { u64::MAX };
        let high = if index == end as usize / 64 { u64::MAX >> (63 - end % 64) } else { u64::MAX };
        points[index] & low & high != 0
    })
}

pub(crate) fn add_font_codepoints(points: &mut Vec<u64>, text: &str) -> () {
    for ch in text.chars() {
        let point = ch as usize;
        if points.len() <= point / 64 {
            points.resize(point / 64 + 1, 0);
        }
        points[point / 64] |= 1u64 << (point % 64);
    }
}

fn document_font_codepoints(tree: &DomTree) -> Vec<u64> {
    let mut points = Vec::new();
    for id in crate::dom::rendered_descendants(tree, tree.document()) {
        tree.with_node(id, |node| {
            let obscura_dom::tree::NodeData::Text { contents } = &node.data else { return };
            let inert = node.parent.and_then(|parent| tree.with_node(parent, |parent| {
                parent.as_element().is_some_and(|element| {
                    matches!(&*element.local, "script" | "style" | "template" | "noscript")
                })
            })).unwrap_or(false);
            if !inert {
                add_font_codepoints(&mut points, contents);
            }
        });
    }
    points
}

pub(crate) fn font_face_urls(face: &str) -> Vec<String> {
    let Some(src) = font_face_declaration(face, "src") else {
        return Vec::new();
    };
    let lower = src.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut cursor = 0;
    while let Some(relative) = lower[cursor..].find("url(") {
        let start = cursor + relative + 4;
        let Some(end_relative) = src[start..].find(')') else {
            break;
        };
        let end = start + end_relative;
        let value = src[start..end]
            .trim()
            .trim_matches(|ch| ch == '"' || ch == '\'')
            .trim();
        if !value.is_empty() {
            out.push(value.to_string());
        }
        cursor = end + 1;
    }
    out
}

pub(crate) fn split_css_top_level(value: &str, separator: char) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut depth = 0i32;
    let mut quote = None;
    let mut escaped = false;
    for (index, ch) in value.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if let Some(active) = quote {
            if ch == active {
                quote = None;
            }
            continue;
        }
        if matches!(ch, '"' | '\'') {
            quote = Some(ch);
            continue;
        }
        match ch {
            '(' => depth += 1,
            ')' => depth = (depth - 1).max(0),
            ch if ch == separator && depth == 0 => {
                out.push(&value[start..index]);
                start = index + ch.len_utf8();
            }
            _ => {}
        }
    }
    out.push(&value[start..]);
    out
}

/// Inspect only already-loaded faces. Geometry cannot initiate synchronous
/// network I/O or force a layout, and uses the same document font selection
/// inputs as paint once the owning FontFace load has completed.
pub fn loaded_web_fonts(tree: &DomTree, base_url: Option<&str>, cache: &RenderResourceCache,
    dynamic_fonts: &[DynamicFontFace], viewport: (f32, f32), media: crate::CssMediaType,
) -> Vec<WebFont> {
    collect_font_rules(tree, base_url, |key, _src| (cache.cached_decoded_font(key), false),
        dynamic_fonts, viewport, media, None)
}
