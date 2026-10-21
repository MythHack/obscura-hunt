//! PDF encoding of the retained painter's actual primitives and shaped glyphs.
//!
//! Recording is scoped to one synchronous paint on the page's owning thread.
//! Screenshot callers keep their raster fast path; export never substitutes a
//! page bitmap for the recorded text and paths.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use krilla::geom::{Path, PathBuilder, Point, Size, Transform};
use krilla::image::Image;
use krilla::num::NormalizedF32;
use krilla::paint::{Fill, FillRule};
use krilla::surface::Surface;
use krilla::text::{Font, GlyphId, KrillaGlyph};
use krilla_svg::SurfaceExt;
use tiny_skia::{Pixmap, PixmapRef};

const MAX_RECORDING_BYTES: usize = 64 * 1024 * 1024;
const MAX_OPERATIONS: usize = 250_000;
const MAX_EXPORT_TIME: std::time::Duration = std::time::Duration::from_secs(30);
type FontCache = HashMap<cosmic_text::fontdb::ID, HashMap<cosmic_text::FontVariations, Font>>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PdfPaintError {
    PaintFailed,
    ResourceLimit,
    DeadlineExceeded,
    Cancelled,
    Encode(String),
}

impl std::fmt::Display for PdfPaintError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PaintFailed => f.write_str("retained PDF paint failed"),
            Self::ResourceLimit => f.write_str("PDF paint exceeded the bounded resource budget"),
            Self::DeadlineExceeded => f.write_str("PDF export exceeded its 30-second deadline"),
            Self::Cancelled => f.write_str("PDF export was cancelled"),
            Self::Encode(error) => write!(f, "PDF encoding failed: {error}"),
        }
    }
}

impl std::error::Error for PdfPaintError {}

/// A bounded PDF document. Pages are appended from the retained painter and
/// released individually; the encoder subsets each concrete shaped font face.
pub struct PdfDocument {
    document: krilla::Document,
    started: std::time::Instant,
    bytes: usize,
    fonts: FontCache,
    cancelled: Arc<dyn Fn() -> bool + Send + Sync>,
}

impl Default for PdfDocument {
    fn default() -> Self { Self::new() }
}

impl PdfDocument {
    pub fn new() -> Self {
        Self::with_cancellation(Arc::new(|| false))
    }

    pub fn with_cancellation(cancelled: Arc<dyn Fn() -> bool + Send + Sync>) -> Self {
        Self { document: krilla::Document::new(), started: std::time::Instant::now(), bytes: 0,
            fonts: HashMap::new(), cancelled }
    }

    pub fn append_page(
        &mut self,
        paper: (f32, f32),
        origin: (f32, f32),
        scale: f32,
        capture: impl FnOnce() -> Result<Pixmap, crate::CaptureError>,
    ) -> Result<(), PdfPaintError> {
        if self.started.elapsed() >= MAX_EXPORT_TIME { return Err(PdfPaintError::DeadlineExceeded); }
        if (self.cancelled)() { return Err(PdfPaintError::Cancelled); }
        let scope = RecordingScope::new(self.started, MAX_RECORDING_BYTES.saturating_sub(self.bytes),
            std::mem::take(&mut self.fonts), Arc::clone(&self.cancelled))?;
        let raster = match capture() {
            Ok(raster) => raster,
            Err(_) => return Err(RECORDING.with(|slot| slot.borrow().as_ref().and_then(|r| r.error.clone()))
                .unwrap_or(PdfPaintError::PaintFailed)),
        };
        let recording = scope.finish(&raster)?;
        self.fonts = recording.fonts;
        self.bytes = self.bytes.checked_add(recording.bytes).ok_or(PdfPaintError::ResourceLimit)?;
        let settings = krilla::page::PageSettings::from_wh(paper.0, paper.1)
            .ok_or_else(|| PdfPaintError::Encode("invalid page dimensions".into()))?;
        let mut page = self.document.start_page_with(settings);
        let mut surface = page.surface();
        let clip = krilla::geom::Rect::from_xywh(origin.0, origin.1,
            raster.width() as f32 * scale, raster.height() as f32 * scale)
            .ok_or(PdfPaintError::PaintFailed)?;
        let clip = rect_path(clip).ok_or(PdfPaintError::PaintFailed)?;
        surface.push_clip_path(&clip, &FillRule::NonZero);
        surface.push_transform(&Transform::from_row(scale, 0., 0., scale, origin.0, origin.1));
        let result = replay(&recording.operations, &mut surface, self.started, &self.cancelled, 0);
        surface.pop();
        surface.pop();
        surface.finish();
        page.finish();
        result
    }

    pub fn finish(self) -> Result<Vec<u8>, PdfPaintError> {
        if self.started.elapsed() >= MAX_EXPORT_TIME { return Err(PdfPaintError::DeadlineExceeded); }
        if (self.cancelled)() { return Err(PdfPaintError::Cancelled); }
        let output = self.document.finish().map_err(|error| PdfPaintError::Encode(error.to_string()))?;
        if output.len() > MAX_RECORDING_BYTES { return Err(PdfPaintError::ResourceLimit); }
        if self.started.elapsed() >= MAX_EXPORT_TIME { return Err(PdfPaintError::DeadlineExceeded); }
        if (self.cancelled)() { return Err(PdfPaintError::Cancelled); }
        Ok(output)
    }
}

#[derive(Clone)]
struct Operation {
    transform: Transform,
    mask: Option<(Image, Size)>,
    kind: Primitive,
}

#[derive(Clone)]
enum Primitive {
    // Surface copy-on-write shares immutable path geometry.
    Path(Arc<Path>, Fill),
    Glyph(GlyphRun),
    Image(Image, Size),
    Group(Arc<Vec<Operation>>, f32),
    Svg(Arc<usvg_pdf::Tree>, Size),
    Clip(Arc<Path>, Arc<Vec<Operation>>),
}

#[derive(Clone)]
struct GlyphRun {
    font: Font,
    font_id: cosmic_text::fontdb::ID,
    variations: cosmic_text::FontVariations,
    glyphs: Vec<KrillaGlyph>,
    text: String,
    position: Point,
    size: f32,
    color: [u8; 4],
    fill: Option<Arc<Fill>>,
    clip: Option<crate::Rect>,
    synthetic_italic: bool,
    masked: bool,
}

impl GlyphRun {
    fn append(&mut self, glyph: u16, text: &str, position: (f32, f32)) {
        let start = self.text.len();
        self.text.push_str(text);
        self.glyphs.push(KrillaGlyph::new(GlyphId::new(u32::from(glyph)), 0.,
            (position.0 - self.position.x) / self.size,
            (self.position.y - position.1) / self.size, 0., start..self.text.len(), None));
    }
}

fn last_glyph_run(operations: &mut [Operation]) -> Option<&mut GlyphRun> {
    match &mut operations.last_mut()?.kind {
        Primitive::Glyph(run) => Some(run),
        Primitive::Clip(_, operations) => last_glyph_run(Arc::make_mut(operations).as_mut_slice()),
        _ => None,
    }
}

struct Recording {
    surfaces: HashMap<usize, Arc<Vec<Operation>>>,
    masks: HashMap<usize, (Image, Size)>,
    fonts: FontCache,
    gradients: Vec<(tiny_skia::Shader<'static>, Fill)>,
    stops: Vec<(tiny_skia::GradientStop, krilla::paint::Stop)>,
    operations: usize,
    bytes: usize,
    limit: usize,
    started: std::time::Instant,
    error: Option<PdfPaintError>,
    raster_effect: usize,
    cancelled: Arc<dyn Fn() -> bool + Send + Sync>,
}

struct PageRecording {
    operations: Arc<Vec<Operation>>,
    bytes: usize,
    fonts: FontCache,
}

thread_local! {
    static RECORDING: RefCell<Option<Recording>> = const { RefCell::new(None) };
}

struct RecordingScope;
impl RecordingScope {
    fn new(started: std::time::Instant, limit: usize,
        fonts: FontCache,
        cancelled: Arc<dyn Fn() -> bool + Send + Sync>) -> Result<Self, PdfPaintError> {
        RECORDING.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.is_some() { return Err(PdfPaintError::PaintFailed); }
            *slot = Some(Recording { surfaces: HashMap::new(), masks: HashMap::new(),
                fonts, gradients: Vec::new(), stops: Vec::new(), operations: 0,
                bytes: 0, limit, started, error: None, raster_effect: 0, cancelled });
            Ok(Self)
        })
    }

    fn finish(self, raster: &Pixmap) -> Result<PageRecording, PdfPaintError> {
        RECORDING.with(|slot| {
            let mut recording = slot.borrow_mut().take().ok_or(PdfPaintError::PaintFailed)?;
            if let Some(error) = recording.error { return Err(error); }
            let operations = recording.surfaces.remove(&key(raster.as_ref()))
                .ok_or(PdfPaintError::PaintFailed)?;
            Ok(PageRecording { operations, bytes: recording.bytes, fonts: recording.fonts })
        })
    }
}
impl Drop for RecordingScope {
    fn drop(&mut self) { RECORDING.with(|slot| { slot.borrow_mut().take(); }); }
}

pub(crate) fn active() -> bool {
    RECORDING.with(|slot| slot.borrow().as_ref().is_some_and(|recording| recording.raster_effect == 0))
}
fn key(pixmap: PixmapRef<'_>) -> usize { pixmap.data().as_ptr() as usize }

/// PDF matrices are affine. Only genuine 3D effect planes use the native
/// local raster compositor; ordinary/affine text never enters this scope.
pub(crate) struct RasterEffectScope(bool);
pub(crate) fn raster_effect(enabled: bool) -> RasterEffectScope {
    let enabled = enabled && active();
    if enabled { RECORDING.with(|slot| { slot.borrow_mut().as_mut().unwrap().raster_effect += 1; }); }
    RasterEffectScope(enabled)
}
impl Drop for RasterEffectScope {
    fn drop(&mut self) {
        if self.0 { RECORDING.with(|slot| {
            if let Some(recording) = slot.borrow_mut().as_mut() { recording.raster_effect -= 1; }
        }); }
    }
}

pub(crate) fn resource_limit() {
    RECORDING.with(|slot| {
        if let Some(recording) = slot.borrow_mut().as_mut() { recording.error = Some(PdfPaintError::ResourceLimit); }
    });
}

pub(crate) fn draw_local_effect(target: &mut Pixmap, raster: &Pixmap) {
    let (mut left, mut top, mut right, mut bottom) = (raster.width(), raster.height(), 0, 0);
    for (index, pixel) in raster.pixels().iter().enumerate() {
        if pixel.alpha() == 0 { continue; }
        let (x, y) = (index as u32 % raster.width(), index as u32 / raster.width());
        left = left.min(x); top = top.min(y); right = right.max(x + 1); bottom = bottom.max(y + 1);
    }
    if right <= left || bottom <= top { return; }
    let Some(rect) = tiny_skia::IntRect::from_xywh(left as i32, top as i32, right-left, bottom-top) else { return; };
    let Some(local) = raster.clone_rect(rect) else { resource_limit(); return; };
    register_raster(local.as_ref());
    target.pdf_draw_pixmap(left as i32, top as i32, local.as_ref(), &tiny_skia::PixmapPaint::default(),
        tiny_skia::Transform::identity(), None);
}

impl Recording {
    fn charge(&mut self, bytes: usize) -> bool {
        if self.error.is_some() { return false; }
        if (self.cancelled)() { self.error = Some(PdfPaintError::Cancelled); return false; }
        if self.started.elapsed() >= MAX_EXPORT_TIME {
            self.error = Some(PdfPaintError::DeadlineExceeded);
            return false;
        }
        if bytes > self.limit.saturating_sub(self.bytes) || self.operations >= MAX_OPERATIONS {
            self.error = Some(PdfPaintError::ResourceLimit);
            return false;
        }
        self.bytes += bytes;
        true
    }

    fn push(&mut self, target: usize, operation: Operation) {
        if self.charge(std::mem::size_of::<Operation>()) {
            self.operations += 1;
            Arc::make_mut(self.surfaces.entry(target).or_default()).push(operation);
        }
    }

    fn mask(&mut self, mask: Option<&tiny_skia::Mask>) -> Option<(Image, Size)> {
        let mask = mask?;
        let id = mask.data().as_ptr() as usize;
        if let Some(cached) = self.masks.get(&id) { return Some(cached.clone()); }
        if !self.charge(mask.data().len().saturating_mul(4)) { return None; }
        let mut rgba = Vec::with_capacity(mask.data().len() * 4);
        for &alpha in mask.data() { rgba.extend_from_slice(&[alpha, alpha, alpha, 255]); }
        let image = Image::from_rgba8(rgba, mask.width(), mask.height());
        let size = Size::from_wh(mask.width() as f32, mask.height() as f32)?;
        self.masks.insert(id, (image.clone(), size));
        Some((image, size))
    }

    fn fill(&mut self, paint: &tiny_skia::Paint<'_>, rule: tiny_skia::FillRule) -> Option<Fill> {
        let mut fill = match &paint.shader {
            tiny_skia::Shader::SolidColor(color) => color_fill(*color),
            shader => self.gradients.iter().rev().find(|(known, _)| known == shader)?.1.clone(),
        };
        fill.rule = match rule { tiny_skia::FillRule::Winding => FillRule::NonZero,
            tiny_skia::FillRule::EvenOdd => FillRule::EvenOdd };
        Some(fill)
    }
}

fn color_fill(color: tiny_skia::Color) -> Fill {
    Fill { paint: krilla::color::rgb::Color::new((color.red() * 255.).round() as u8,
        (color.green() * 255.).round() as u8, (color.blue() * 255.).round() as u8).into(),
        opacity: NormalizedF32::new(color.alpha()).unwrap_or(NormalizedF32::ZERO), rule: FillRule::NonZero }
}

fn path(path: &tiny_skia::Path) -> Option<Path> {
    let mut builder = PathBuilder::new();
    for segment in path.segments() {
        match segment {
            tiny_skia::PathSegment::MoveTo(p) => builder.move_to(p.x, p.y),
            tiny_skia::PathSegment::LineTo(p) => builder.line_to(p.x, p.y),
            tiny_skia::PathSegment::QuadTo(p, q) => builder.quad_to(p.x, p.y, q.x, q.y),
            tiny_skia::PathSegment::CubicTo(p, q, r) => builder.cubic_to(p.x, p.y, q.x, q.y, r.x, r.y),
            tiny_skia::PathSegment::Close => builder.close(),
        }
    }
    builder.finish()
}
fn rect_path(rect: krilla::geom::Rect) -> Option<Path> {
    let mut builder = PathBuilder::new();
    builder.push_rect(rect);
    builder.finish()
}
fn transform(t: tiny_skia::Transform) -> Transform { Transform::from_row(t.sx, t.ky, t.kx, t.sy, t.tx, t.ty) }

pub(crate) fn new_pixmap(width: u32, height: u32) -> Option<Pixmap> {
    let pixmap = Pixmap::new(width, height)?;
    register_pixmap(&pixmap);
    Some(pixmap)
}
pub(crate) fn register_pixmap(pixmap: &Pixmap) {
    RECORDING.with(|slot| {
        if let Some(recording) = slot.borrow_mut().as_mut() {
            if recording.raster_effect == 0 {
                recording.surfaces.insert(key(pixmap.as_ref()), Arc::default());
            } else { recording.surfaces.remove(&key(pixmap.as_ref())); }
        }
    });
}
pub(crate) fn register_raster(pixmap: PixmapRef<'_>) {
    RECORDING.with(|slot| {
        if let Some(recording) = slot.borrow_mut().as_mut() { recording.surfaces.remove(&key(pixmap)); }
    });
}
pub(crate) fn pixmap_from_vec(bytes: Vec<u8>, size: tiny_skia::IntSize) -> Option<Pixmap> {
    let pixmap = Pixmap::from_vec(bytes, size)?; register_raster(pixmap.as_ref()); Some(pixmap)
}
pub(crate) fn new_mask(width: u32, height: u32) -> Option<tiny_skia::Mask> {
    let mask = tiny_skia::Mask::new(width, height)?;
    RECORDING.with(|slot| {
        if let Some(recording) = slot.borrow_mut().as_mut() {
            recording.masks.remove(&(mask.data().as_ptr() as usize));
        }
    });
    Some(mask)
}

/// Kept on the existing painter, rather than a second DOM/layout traversal.
pub(crate) trait PdfPaint {
    fn pdf_fill(&mut self, color: tiny_skia::Color);
    fn pdf_fill_path(&mut self, path: &tiny_skia::Path, paint: &tiny_skia::Paint<'_>, rule: tiny_skia::FillRule,
        transform: tiny_skia::Transform, mask: Option<&tiny_skia::Mask>);
    fn pdf_stroke_path(&mut self, path: &tiny_skia::Path, paint: &tiny_skia::Paint<'_>, stroke: &tiny_skia::Stroke,
        transform: tiny_skia::Transform, mask: Option<&tiny_skia::Mask>);
    fn pdf_fill_rect(&mut self, rect: tiny_skia::Rect, paint: &tiny_skia::Paint<'_>,
        transform: tiny_skia::Transform, mask: Option<&tiny_skia::Mask>);
    fn pdf_draw_pixmap(&mut self, x: i32, y: i32, source: PixmapRef<'_>, paint: &tiny_skia::PixmapPaint,
        transform: tiny_skia::Transform, mask: Option<&tiny_skia::Mask>);
}

impl PdfPaint for Pixmap {
    fn pdf_fill(&mut self, color: tiny_skia::Color) {
        self.fill(color);
        if active() {
            let mut paint = tiny_skia::Paint::default(); paint.set_color(color);
            if let Some(rect) = tiny_skia::Rect::from_xywh(0., 0., self.width() as f32, self.height() as f32) {
                self.pdf_fill_rect(rect, &paint, tiny_skia::Transform::identity(), None);
            }
        }
    }
    fn pdf_fill_path(&mut self, source: &tiny_skia::Path, paint: &tiny_skia::Paint<'_>, rule: tiny_skia::FillRule,
        ts: tiny_skia::Transform, mask: Option<&tiny_skia::Mask>) {
        if !active() { self.fill_path(source, paint, rule, ts, mask); return; }
        RECORDING.with(|slot| {
            let mut slot = slot.borrow_mut(); let recording = slot.as_mut().unwrap();
            if let (Some(path), Some(fill)) = (path(source), recording.fill(paint, rule)) {
                if !recording.charge(source.points().len().saturating_mul(32)) { return; }
                let mask = recording.mask(mask);
                recording.push(key(self.as_ref()), Operation { transform: transform(ts), mask,
                    kind: Primitive::Path(Arc::new(path), fill) });
            } else {
                // Shader patterns and sampled effects remain local raster content,
                // clipped to this primitive, never a replacement for the page.
                let Some(mut local) = Pixmap::new(self.width(), self.height()) else {
                    recording.error = Some(PdfPaintError::ResourceLimit); return;
                };
                local.fill_path(source, paint, rule, ts, mask);
                record_local_raster(recording, self, &local, source, ts);
            }
        });
    }
    fn pdf_stroke_path(&mut self, source: &tiny_skia::Path, paint: &tiny_skia::Paint<'_>, stroke: &tiny_skia::Stroke,
        ts: tiny_skia::Transform, mask: Option<&tiny_skia::Mask>) {
        if !active() { self.stroke_path(source, paint, stroke, ts, mask); return; }
        // Converting the stroke to its exact outline shares tiny-skia's dash,
        // cap and join algorithm and keeps the resulting shape vector-based.
        if let Some(outline) = source.stroke(stroke, 1.0) {
            self.pdf_fill_path(&outline, paint, tiny_skia::FillRule::Winding, ts, mask);
        }
    }
    fn pdf_fill_rect(&mut self, rect: tiny_skia::Rect, paint: &tiny_skia::Paint<'_>,
        ts: tiny_skia::Transform, mask: Option<&tiny_skia::Mask>) {
        if !active() { self.fill_rect(rect, paint, ts, mask); return; }
        self.pdf_fill_path(&tiny_skia::PathBuilder::from_rect(rect), paint, tiny_skia::FillRule::Winding, ts, mask);
    }
    fn pdf_draw_pixmap(&mut self, x: i32, y: i32, source: PixmapRef<'_>, paint: &tiny_skia::PixmapPaint,
        ts: tiny_skia::Transform, mask: Option<&tiny_skia::Mask>) {
        if !active() { self.draw_pixmap(x, y, source, paint, ts, mask); return; }
        RECORDING.with(|slot| {
            let mut slot = slot.borrow_mut(); let recording = slot.as_mut().unwrap();
            let kind = if let Some(operations) = recording.surfaces.get(&key(source)).filter(|ops| !ops.is_empty()) {
                Primitive::Group(Arc::clone(operations), paint.opacity)
            } else {
                if !recording.charge(source.data().len()) { return; }
                let size = Size::from_wh(source.width() as f32, source.height() as f32).unwrap();
                let image = image(source);
                Primitive::Group(Arc::new(vec![Operation { transform: Transform::identity(), mask: None,
                    kind: Primitive::Image(image, size) }]), paint.opacity)
            };
            let mask = recording.mask(mask);
            recording.push(key(self.as_ref()), Operation { transform: transform(ts.pre_translate(x as f32, y as f32)), mask, kind });
        });
    }
}

fn image(source: PixmapRef<'_>) -> Image {
    let mut bytes = Vec::with_capacity(source.data().len());
    for pixel in source.pixels() {
        let color = pixel.demultiply();
        bytes.extend_from_slice(&[color.red(), color.green(), color.blue(), color.alpha()]);
    }
    Image::from_rgba8(bytes, source.width(), source.height())
}

fn record_local_raster(recording: &mut Recording, target: &Pixmap, raster: &Pixmap, source: &tiny_skia::Path,
    ts: tiny_skia::Transform) {
    let Some(bounds) = source.clone().transform(ts).map(|p| p.bounds()) else { return; };
    let left = bounds.left().floor().max(0.) as u32;
    let top = bounds.top().floor().max(0.) as u32;
    let right = bounds.right().ceil().min(target.width() as f32).max(left as f32) as u32;
    let bottom = bounds.bottom().ceil().min(target.height() as f32).max(top as f32) as u32;
    if right == left || bottom == top { return; }
    let Some(rect) = tiny_skia::IntRect::from_xywh(left as i32, top as i32, right-left, bottom-top) else { return; };
    if !recording.charge((right-left) as usize * (bottom-top) as usize * 4) { return; }
    let Some(local) = raster.clone_rect(rect) else { return; };
    recording.push(key(target.as_ref()), Operation { transform: Transform::from_translate(left as f32, top as f32),
        mask: None, kind: Primitive::Image(image(local.as_ref()), Size::from_wh((right-left) as f32, (bottom-top) as f32).unwrap()) });
}

pub(crate) fn record_glyph(
    target: &Pixmap, font_system: &mut cosmic_text::FontSystem, glyph: &cosmic_text::LayoutGlyph,
    text: &str, position: (f32, f32), color: [u8; 4], mask: Option<&tiny_skia::Mask>,
    clip: Option<crate::Rect>, variations: Option<&cosmic_text::FontVariations>,
    fill: Option<Arc<Fill>>,
) {
    if color[3] == 0 && fill.is_none() { return; }
    RECORDING.with(|slot| {
        let mut slot = slot.borrow_mut(); let Some(recording) = slot.as_mut() else { return; };
        if !recording.charge(text.len() + std::mem::size_of::<KrillaGlyph>()) { return; }
        let empty = cosmic_text::FontVariations::default();
        let variations = variations.unwrap_or(&empty);
        let synthetic_italic = glyph.cache_key_flags.contains(cosmic_text::CacheKeyFlags::FAKE_ITALIC);
        // Preserve the shaper's positioned glyphs, but retain adjacent equal
        // style/font glyphs as one real PDF text run (not one allocation and
        // text object per codepoint). Masked runs stay separate to preserve
        // the exact alpha mask generation without pointer-identity guesses.
        if mask.is_none() {
            if let Some(run) = recording.surfaces.get_mut(&key(target.as_ref()))
                .and_then(|operations| last_glyph_run(Arc::make_mut(operations).as_mut_slice())) {
                let same_fill = match (&run.fill, &fill) {
                    (None, None) => true,
                    (Some(left), Some(right)) => Arc::ptr_eq(left, right),
                    _ => false,
                };
                if !run.masked && run.font_id == glyph.font_id && &run.variations == variations
                    && run.size == glyph.font_size && run.position.y == position.1
                    && run.color == color && same_fill && run.clip == clip
                    && run.synthetic_italic == synthetic_italic {
                    run.append(glyph.glyph_id, text, position);
                    return;
                }
            }
        }
        let font = if let Some(font) = recording.fonts.get(&glyph.font_id).and_then(|fonts| fonts.get(variations)) {
            font.clone()
        } else {
            let Some(font) = font_system.get_font(glyph.font_id) else {
                recording.error = Some(PdfPaintError::Encode("shaped font is unavailable".into())); return;
            };
            if !recording.charge(font.data().len()) { return; }
            let Some((data, index)) = font_system.db().face_source(glyph.font_id) else {
                recording.error = Some(PdfPaintError::Encode("shaped font source is unavailable".into())); return;
            };
            let data: Arc<dyn AsRef<[u8]> + Send + Sync> = match data {
                cosmic_text::fontdb::Source::Binary(data) => data,
                cosmic_text::fontdb::Source::SharedFile(_, data) => data,
                _ => {
                    let Some(data) = font_system.db().with_face_data(glyph.font_id, |data, _| data.to_vec()) else {
                        recording.error = Some(PdfPaintError::Encode("shaped font source is unavailable".into())); return;
                    };
                    Arc::new(data) as Arc<dyn AsRef<[u8]> + Send + Sync>
                }
            };
            let coords: Vec<_> = variations.iter().map(|variation|
                (krilla::text::Tag::new(variation.tag.as_bytes()), variation.value.0)).collect();
            let Some(font) = Font::new_variable(data.into(), index, &coords) else {
                recording.error = Some(PdfPaintError::Encode("shaped font cannot be embedded".into())); return;
            };
            recording.fonts.entry(glyph.font_id).or_default().insert(variations.clone(), font.clone()); font
        };
        let mut run = GlyphRun { font, font_id: glyph.font_id, variations: variations.clone(),
            glyphs: Vec::new(), text: String::new(), position: Point::from_xy(position.0, position.1),
            size: glyph.font_size, color, fill, clip, synthetic_italic, masked: mask.is_some() };
        run.append(glyph.glyph_id, text, position);
        let mask = recording.mask(mask);
        let slant = if synthetic_italic {
            let skew = 14f32.to_radians().tan();
            Transform::from_row(1., 0., -skew, 1., skew * position.1, 0.)
        } else { Transform::identity() };
        let operation = Operation { transform: slant, mask, kind: Primitive::Glyph(run) };
        if let Some(clip) = clip {
            let Some(rect) = krilla::geom::Rect::from_xywh(clip.x, clip.y, clip.width, clip.height) else { return; };
            let Some(path) = rect_path(rect) else {
                recording.error = Some(PdfPaintError::PaintFailed); return;
            };
            recording.push(key(target.as_ref()), Operation { transform: Transform::identity(), mask: None,
                kind: Primitive::Clip(Arc::new(path), Arc::new(vec![operation])) });
        } else { recording.push(key(target.as_ref()), operation); }
    });
}

pub(crate) fn record_svg(target: &Pixmap, bytes: &[u8], native: &usvg::Tree, width: u32, height: u32) {
    if !active() { return; }
    RECORDING.with(|slot| {
        let mut slot = slot.borrow_mut(); let recording = slot.as_mut().unwrap();
        if !recording.charge(bytes.len()) { return; }
        let mut options = usvg_pdf::Options::default();
        let Some(fonts) = pdf_svg_fonts(native, recording) else {
            if recording.error.is_none() { recording.error = Some(PdfPaintError::Encode("selected SVG font is unavailable".into())); }
            return;
        };
        options.fontdb = Arc::new(fonts);
        // Export only bytes already authorized by page resource loading.
        options.image_href_resolver.resolve_string = Box::new(|_, _| None);
        let tree = match usvg_pdf::Tree::from_data(bytes, &options) {
            Ok(tree) => tree, Err(error) => { recording.error = Some(PdfPaintError::Encode(error.to_string())); return; }
        };
        recording.push(key(target.as_ref()), Operation { transform: Transform::identity(), mask: None,
            kind: Primitive::Svg(Arc::new(tree), Size::from_wh(width as f32, height as f32).unwrap()) });
    });
}

fn pdf_svg_fonts(tree: &usvg::Tree, recording: &mut Recording) -> Option<usvg_pdf::fontdb::Database> {
    fn collect(group: &usvg::Group, ids: &mut std::collections::HashSet<fontdb::ID>, depth: usize) {
        if depth > 256 { return; }
        for node in group.children() {
            match node {
                usvg::Node::Group(group) => collect(group, ids, depth + 1),
                usvg::Node::Text(text) => {
                    for span in text.layouted() {
                        ids.extend(span.positioned_glyphs.iter().map(|glyph| glyph.font));
                    }
                }
                _ => {}
            }
            node.subroots(|group| collect(group, ids, depth + 1));
        }
    }
    let mut ids = std::collections::HashSet::new();
    collect(tree.root(), &mut ids, 0);
    let mut db = usvg_pdf::fontdb::Database::new();
    // The adapter's usvg version differs, but receives only concrete font
    // sources already selected by the native painter, including web fonts.
    // No new URL resolution, system scan, or permission policy is introduced.
    let mut sources = std::collections::HashSet::new();
    for id in ids {
        let (source, _) = tree.fontdb().face_source(id)?;
        let bytes: Arc<dyn AsRef<[u8]> + Send + Sync> = match source {
            fontdb::Source::Binary(bytes) => bytes,
            fontdb::Source::SharedFile(_, bytes) => bytes,
            fontdb::Source::File(_) => Arc::new(tree.fontdb().with_face_data(id, |bytes, _| bytes.to_vec())?),
        };
        let identity = (bytes.as_ref().as_ref().as_ptr() as usize, bytes.as_ref().as_ref().len());
        if !sources.insert(identity) { continue; }
        if !recording.charge(bytes.as_ref().as_ref().len()) { return None; }
        db.load_font_source(usvg_pdf::fontdb::Source::Binary(bytes));
    }
    db.set_sans_serif_family("Liberation Sans"); db.set_serif_family("Liberation Serif");
    db.set_monospace_family("Liberation Mono");
    Some(db)
}

pub(crate) fn gradient_stop(position: f32, color: tiny_skia::Color) -> tiny_skia::GradientStop {
    let stop = tiny_skia::GradientStop::new(position, color);
    RECORDING.with(|slot| {
        if let Some(recording) = slot.borrow_mut().as_mut() {
            if recording.charge(std::mem::size_of::<krilla::paint::Stop>()) {
                recording.stops.push((stop, krilla::paint::Stop {
                    offset: NormalizedF32::new(position.clamp(0., 1.)).unwrap_or(NormalizedF32::ZERO),
                    color: krilla::color::rgb::Color::new((color.red()*255.).round() as u8,
                        (color.green()*255.).round() as u8, (color.blue()*255.).round() as u8).into(),
                    opacity: NormalizedF32::new(color.alpha()).unwrap_or(NormalizedF32::ZERO),
                }));
            }
        }
    });
    stop
}

fn spread(mode: tiny_skia::SpreadMode) -> krilla::paint::SpreadMethod {
    match mode { tiny_skia::SpreadMode::Pad => krilla::paint::SpreadMethod::Pad,
        tiny_skia::SpreadMode::Reflect => krilla::paint::SpreadMethod::Reflect,
        tiny_skia::SpreadMode::Repeat => krilla::paint::SpreadMethod::Repeat }
}

fn gradient_stops(recording: &Recording, stops: &[tiny_skia::GradientStop]) -> Option<Vec<krilla::paint::Stop>> {
    stops.iter().map(|stop| recording.stops.iter().rev().find(|(known, _)| known == stop)
        .map(|(_, value)| value.clone())).collect()
}

pub(crate) fn linear_gradient(start: tiny_skia::Point, end: tiny_skia::Point,
    stops: Vec<tiny_skia::GradientStop>, mode: tiny_skia::SpreadMode, ts: tiny_skia::Transform)
    -> Option<tiny_skia::Shader<'static>> {
    let descriptor = RECORDING.with(|slot| slot.borrow().as_ref().and_then(|recording| gradient_stops(recording, &stops)));
    let shader = tiny_skia::LinearGradient::new(start, end, stops, mode, ts)?;
    if let Some(stops) = descriptor {
        RECORDING.with(|slot| {
            if let Some(recording) = slot.borrow_mut().as_mut() {
                recording.gradients.push((shader.clone(), Fill {
                    paint: krilla::paint::LinearGradient { x1: start.x, y1: start.y, x2: end.x, y2: end.y,
                        transform: transform(ts), spread_method: spread(mode), stops, anti_alias: true }.into(),
                    opacity: NormalizedF32::ONE, rule: FillRule::NonZero,
                }));
            }
        });
    }
    Some(shader)
}

pub(crate) fn radial_gradient(start: tiny_skia::Point, start_radius: f32,
    end: tiny_skia::Point, end_radius: f32, stops: Vec<tiny_skia::GradientStop>,
    mode: tiny_skia::SpreadMode, ts: tiny_skia::Transform) -> Option<tiny_skia::Shader<'static>> {
    let descriptor = RECORDING.with(|slot| slot.borrow().as_ref().and_then(|recording| gradient_stops(recording, &stops)));
    let shader = tiny_skia::RadialGradient::new(start, start_radius, end, end_radius, stops, mode, ts)?;
    if let Some(stops) = descriptor {
        RECORDING.with(|slot| {
            if let Some(recording) = slot.borrow_mut().as_mut() {
                recording.gradients.push((shader.clone(), Fill {
                    paint: krilla::paint::RadialGradient { fx: start.x, fy: start.y, fr: start_radius,
                        cx: end.x, cy: end.y, cr: end_radius, transform: transform(ts),
                        spread_method: spread(mode), stops, anti_alias: true }.into(),
                    opacity: NormalizedF32::ONE, rule: FillRule::NonZero,
                }));
            }
        });
    }
    Some(shader)
}

pub(crate) fn cancelled() -> bool {
    RECORDING.with(|slot| {
        let mut slot = slot.borrow_mut();
        slot.as_mut().is_some_and(|recording| !recording.charge(0))
    })
}

fn replay(operations: &[Operation], surface: &mut Surface<'_>, started: std::time::Instant,
    cancelled: &Arc<dyn Fn() -> bool + Send + Sync>, depth: usize) -> Result<(), PdfPaintError> {
    if depth > 256 { return Err(PdfPaintError::ResourceLimit); }
    for operation in operations {
        if started.elapsed() >= MAX_EXPORT_TIME { return Err(PdfPaintError::DeadlineExceeded); }
        if cancelled() { return Err(PdfPaintError::Cancelled); }
        if let Some((image, size)) = &operation.mask {
            let mut builder = surface.stream_builder();
            let mut mask_surface = builder.surface();
            mask_surface.draw_image(image.clone(), *size); mask_surface.finish();
            let mask = krilla::mask::Mask::new(builder.finish(), krilla::mask::MaskType::Luminosity);
            surface.push_mask(mask);
        }
        surface.push_transform(&operation.transform);
        let result = match &operation.kind {
            Primitive::Path(path, fill) => {
                surface.set_fill(Some(fill.clone())); surface.set_stroke(None); surface.draw_path(path); Ok(())
            }
            Primitive::Glyph(run) => {
                let fill = run.fill.as_ref().map(|fill| fill.as_ref().clone()).unwrap_or_else(||
                    color_fill(tiny_skia::Color::from_rgba8(run.color[0], run.color[1], run.color[2], run.color[3])));
                surface.set_fill(Some(fill)); surface.set_stroke(None);
                surface.draw_glyphs(run.position, &run.glyphs, run.font.clone(), &run.text, run.size, false);
                Ok(())
            }
            Primitive::Image(image, size) => { surface.draw_image(image.clone(), *size); Ok(()) }
            Primitive::Group(operations, opacity) => {
                surface.push_opacity(NormalizedF32::new(opacity.clamp(0., 1.)).unwrap_or(NormalizedF32::ZERO));
                let result = replay(operations, surface, started, cancelled, depth + 1); surface.pop(); result
            }
            Primitive::Clip(path, operations) => {
                surface.push_clip_path(path, &FillRule::NonZero);
                let result = replay(operations, surface, started, cancelled, depth + 1); surface.pop(); result
            }
            Primitive::Svg(tree, size) => surface.draw_svg(tree, *size, krilla_svg::SvgSettings::default())
                .ok_or(PdfPaintError::PaintFailed),
        };
        surface.pop();
        if operation.mask.is_some() { surface.pop(); }
        result?;
    }
    Ok(())
}

pub(crate) fn clone_mask(mask: &tiny_skia::Mask) -> tiny_skia::Mask {
    let copy = mask.clone();
    RECORDING.with(|slot| {
        if let Some(recording) = slot.borrow_mut().as_mut() {
            recording.masks.remove(&(copy.data().as_ptr() as usize));
        }
    });
    copy
}

pub(crate) fn clone_optional_mask(mask: Option<&tiny_skia::Mask>) -> Option<tiny_skia::Mask> {
    mask.map(clone_mask)
}

pub(crate) fn text_gradient(fill: &(f32, Vec<([u8; 4], Option<f32>)>),
    bounds: (f32, f32, f32, f32)) -> Fill {
    let (x, y, width, height) = bounds;
    let angle = fill.0.to_radians();
    let (dx, dy) = (angle.sin(), -angle.cos());
    let length = (width * dx).abs() + (height * dy).abs();
    let (cx, cy) = (x + width * 0.5, y + height * 0.5);
    let stops = crate::paint::normalized_stops(&fill.1).into_iter().map(|(offset, color)| {
        krilla::paint::Stop {
            offset: NormalizedF32::new(offset.clamp(0., 1.)).unwrap_or(NormalizedF32::ZERO),
            color: krilla::color::rgb::Color::new(color[0], color[1], color[2]).into(),
            opacity: NormalizedF32::new(f32::from(color[3]) / 255.).unwrap(),
        }
    }).collect();
    Fill { paint: krilla::paint::LinearGradient { x1: cx - dx * length * 0.5,
        y1: cy - dy * length * 0.5, x2: cx + dx * length * 0.5, y2: cy + dy * length * 0.5,
        transform: Transform::identity(), spread_method: krilla::paint::SpreadMethod::Pad,
        stops, anti_alias: true }.into(), opacity: NormalizedF32::ONE, rule: FillRule::NonZero }
}
