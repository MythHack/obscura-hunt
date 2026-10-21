//! Bounded text/vector PDF export over the retained print-media painter.
//!
//! Layout, shaping, scrolling, and paint ordering are shared with screenshots.
//! CSS paged media, tagged PDF, headers/footers and outlines are not implemented.

use obscura_js::CaptureRegion;
use crate::Page;

const POINTS_PER_INCH: f32 = 72.0;
const MAX_PAPER_INCHES: f32 = 200.0;
const MAX_PDF_PAGES: usize = 250;
const MAX_PDF_DOCUMENT_PAGES: usize = 1_000_000;
const MAX_PDF_PAGE_PIXELS: u64 = 16 * 1024 * 1024;
const MAX_PDF_TOTAL_PAINT_PIXELS: u64 = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PdfPageRange {
    /// One-based inclusive first page. `None` means the first page.
    pub start: Option<usize>,
    /// One-based inclusive last page. `None` means the final page.
    pub end: Option<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PdfOptions {
    pub landscape: bool,
    pub print_background: bool,
    pub scale: f32,
    pub page_ranges: Vec<PdfPageRange>,
    pub paper_width_in: f32,
    pub paper_height_in: f32,
    pub margin_top_in: f32,
    pub margin_bottom_in: f32,
    pub margin_left_in: f32,
    pub margin_right_in: f32,
}

impl Default for PdfOptions {
    fn default() -> Self {
        Self { landscape: false, print_background: false, scale: 1.0,
            page_ranges: Vec::new(), paper_width_in: 8.5, paper_height_in: 11.0,
            margin_top_in: 0.3937, margin_bottom_in: 0.3937,
            margin_left_in: 0.3937, margin_right_in: 0.3937 }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PdfError {
    #[error("PDF paper dimensions must be finite and between 0 and 200 inches")]
    InvalidPaperSize,
    #[error("PDF margins must be finite, non-negative, and leave a printable area")]
    InvalidMargins,
    #[error("PDF scale must be finite and between 0.1 and 2")]
    InvalidScale,
    #[error("PDF page ranges select no pages from this document")]
    EmptyPageRange,
    #[error("the page has no retained renderable document")]
    NoRenderableDocument,
    #[error("PDF pagination would exceed the {0}-page safety limit")]
    TooManyPages(usize),
    #[error("PDF paint would exceed the bounded page or document resource budget")]
    WorkLimitExceeded,
    #[error("document-space PDF paint failed: {0}")]
    PaintFailed(String),
    #[error("encoded PDF would exceed the 64 MiB safety limit")]
    OutputLimitExceeded,
}

#[derive(Clone, Copy, Debug)]
struct PaginationPlan { points_per_css_pixel: f32, css_page_height: f32, page_count: usize }

impl PdfOptions {
    fn page_geometry(&self) -> Result<(f32, f32, f32, f32, f32, f32), PdfError> {
        if [self.paper_width_in, self.paper_height_in].iter()
            .any(|value| !value.is_finite() || *value <= 0. || *value > MAX_PAPER_INCHES) {
            return Err(PdfError::InvalidPaperSize);
        }
        if [self.margin_top_in, self.margin_bottom_in, self.margin_left_in, self.margin_right_in]
            .iter().any(|value| !value.is_finite() || *value < 0.) {
            return Err(PdfError::InvalidMargins);
        }
        let (width, height) = if self.landscape { (self.paper_height_in, self.paper_width_in) }
            else { (self.paper_width_in, self.paper_height_in) };
        let (width, height) = (width * POINTS_PER_INCH, height * POINTS_PER_INCH);
        let printable_width = width - (self.margin_left_in + self.margin_right_in) * POINTS_PER_INCH;
        let printable_height = height - (self.margin_top_in + self.margin_bottom_in) * POINTS_PER_INCH;
        if printable_width <= 0. || printable_height <= 0. { return Err(PdfError::InvalidMargins); }
        Ok((width, height, printable_width, printable_height,
            self.margin_left_in * POINTS_PER_INCH, self.margin_top_in * POINTS_PER_INCH))
    }
}

fn pagination_plan(content_width: f32, content_height: f32, printable_width: f32,
    printable_height: f32, scale: f32) -> Result<PaginationPlan, PdfError> {
    if !scale.is_finite() || !(0.1..=2.).contains(&scale) { return Err(PdfError::InvalidScale); }
    let points_per_css_pixel = printable_width / content_width * scale;
    let css_page_height = printable_height / points_per_css_pixel;
    if !points_per_css_pixel.is_finite() || points_per_css_pixel <= 0.
        || !css_page_height.is_finite() || css_page_height <= 0. { return Err(PdfError::WorkLimitExceeded); }
    let pages = (content_height / css_page_height).ceil().max(1.);
    if !pages.is_finite() || pages > MAX_PDF_DOCUMENT_PAGES as f32 {
        return Err(PdfError::TooManyPages(MAX_PDF_DOCUMENT_PAGES));
    }
    Ok(PaginationPlan { points_per_css_pixel, css_page_height, page_count: pages as usize })
}

fn validate_selected_paint_work(content_width: f32, content_height: f32,
    plan: PaginationPlan, selected_pages: &[usize]) -> Result<(), PdfError> {
    let width = content_width.ceil();
    if !width.is_finite() || width <= 0. || width > obscura_js::MAX_CAPTURE_DIMENSION as f32 {
        return Err(PdfError::WorkLimitExceeded);
    }
    let mut total = 0u64;
    for &page in selected_pages {
        if page >= plan.page_count { return Err(PdfError::EmptyPageRange); }
        let height = (content_height - page as f32 * plan.css_page_height).min(plan.css_page_height).ceil();
        if !height.is_finite() || height <= 0. || height > obscura_js::MAX_CAPTURE_DIMENSION as f32 {
            return Err(PdfError::WorkLimitExceeded);
        }
        let pixels = (width as u64).checked_mul(height as u64).ok_or(PdfError::WorkLimitExceeded)?;
        total = total.checked_add(pixels).ok_or(PdfError::WorkLimitExceeded)?;
        if pixels > MAX_PDF_PAGE_PIXELS || total > MAX_PDF_TOTAL_PAINT_PIXELS {
            return Err(PdfError::WorkLimitExceeded);
        }
    }
    Ok(())
}

fn selected_page_indices(page_count: usize, ranges: &[PdfPageRange]) -> Result<Vec<usize>, PdfError> {
    if page_count == 0 { return Err(PdfError::EmptyPageRange); }
    if ranges.is_empty() {
        if page_count > MAX_PDF_PAGES { return Err(PdfError::TooManyPages(MAX_PDF_PAGES)); }
        return Ok((0..page_count).collect());
    }
    let mut selected = std::collections::BTreeSet::new();
    for range in ranges {
        let start = range.start.unwrap_or(1);
        let end = range.end.unwrap_or(page_count);
        if start == 0 || end == 0 || start > end { return Err(PdfError::EmptyPageRange); }
        if start > page_count { continue; }
        let end = end.min(page_count);
        if end - start + 1 > MAX_PDF_PAGES { return Err(PdfError::TooManyPages(MAX_PDF_PAGES)); }
        for page in start..=end {
            selected.insert(page - 1);
            if selected.len() > MAX_PDF_PAGES { return Err(PdfError::TooManyPages(MAX_PDF_PAGES)); }
        }
    }
    if selected.is_empty() { return Err(PdfError::EmptyPageRange); }
    Ok(selected.into_iter().collect())
}

impl Page {
    /// Export real shaped text, subset fonts, vector paint, and local images.
    /// The full document width fits the printable width; vertical slices share
    /// one print-media layout. Virtual page scrolling repeats fixed content
    /// without changing the live page's scroll state.
    pub fn pdf(&self, options: PdfOptions) -> Result<Vec<u8>, PdfError> {
        self.pdf_with_animation_sample(options, self.live_animation_sample())
    }

    pub fn pdf_at_animation_time(&self, options: PdfOptions,
        time: obscura_js::AnimationSampleTime) -> Result<Vec<u8>, PdfError> {
        self.pdf_with_animation_sample(options, obscura_js::AnimationSample {
            time, mode: obscura_js::AnimationSampleMode::LocalOverride })
    }

    pub fn pdf_with_animation_sample(&self, options: PdfOptions,
        animation_sample: obscura_js::AnimationSample) -> Result<Vec<u8>, PdfError> {
        let (width, height, printable_width, printable_height, left, top) = options.page_geometry()?;
        let js = self.js.as_ref().ok_or(PdfError::NoRenderableDocument)?;
        if !js.set_animation_sample(animation_sample) { return Err(PdfError::NoRenderableDocument); }
        let previous_media = js.set_render_media(
            obscura_js::CssMediaType::Print.with_reduced_motion(js.prefers_reduced_motion())
                .with_dark_color_scheme(js.prefers_dark_color_scheme()));
        let result = (|| {
            let (content_width, content_height) = js.prepared_content_size().ok_or(PdfError::NoRenderableDocument)?;
            if !content_width.is_finite() || !content_height.is_finite()
                || content_width <= 0. || content_height <= 0. { return Err(PdfError::NoRenderableDocument); }
            let plan = pagination_plan(content_width, content_height, printable_width, printable_height, options.scale)?;
            let selected = selected_page_indices(plan.page_count, &options.page_ranges)?;
            validate_selected_paint_work(content_width, content_height, plan, &selected)?;
            let mut document = js.new_pdf_document();
            for page in selected {
                let y = page as f32 * plan.css_page_height;
                let slice_height = (content_height - y).min(plan.css_page_height);
                js.append_pdf_page(&mut document, CaptureRegion::new(0., y, content_width, slice_height, 1.),
                    (0., y), (width, height), (left, top), plan.points_per_css_pixel, options.print_background)
                    .map_err(|error| PdfError::PaintFailed(error.to_string()))?;
            }
            document.finish().map_err(|error| match error {
                obscura_js::PdfPaintError::ResourceLimit => PdfError::OutputLimitExceeded,
                _ => PdfError::PaintFailed(error.to_string()),
            })
        })();
        js.set_render_media(previous_media);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(markup: &str, viewport: (f32, f32)) -> Page {
        let context = std::sync::Arc::new(crate::BrowserContext::new("pdf".into()));
        let mut page = Page::new("pdf-page".into(), context);
        page.set_viewport(viewport);
        let mut js = obscura_js::runtime::ObscuraJsRuntime::new();
        js.set_dom(obscura_dom::parse_html(markup));
        js.set_url("https://example.test/pdf");
        js.set_viewport(f64::from(viewport.0), f64::from(viewport.1));
        js.run_page_init();
        page.js = Some(js);
        page
    }

    fn paper(width: f32, height: f32) -> PdfOptions {
        PdfOptions { paper_width_in: width / 72., paper_height_in: height / 72.,
            margin_top_in: 0., margin_bottom_in: 0., margin_left_in: 0., margin_right_in: 0.,
            print_background: true, ..PdfOptions::default() }
    }


    #[test]
    fn pdf_selected_document_pages_advance_without_changing_live_scroll_state() {
        let page = page(r#"<style>html,body{margin:0;width:100px}div{height:80px;box-sizing:border-box;padding-top:20px;font:12px 'Liberation Sans'}
            header{position:fixed;top:0;left:0;z-index:5;font:10px 'Liberation Sans'}</style>
            <header>FIXED</header><div>FIRST</div><div>SECOND</div><div>THIRD</div>"#, (100., 80.));
        let before = page.js.as_ref().unwrap().scroll_offset();
        let mut options = paper(100., 80.);
        options.page_ranges = vec![PdfPageRange { start: Some(3), end: Some(3) },
            PdfPageRange { start: Some(2), end: Some(3) }];
        let bytes = page.pdf(options).unwrap();
        let document = lopdf::Document::load_mem(&bytes).unwrap();
        assert_eq!(document.get_pages().len(), 2);
        let pages = document.get_pages();
        assert_ne!(document.get_page_content(pages[&1]).unwrap(),
            document.get_page_content(pages[&2]).unwrap(), "selected pages must advance through the document");
        assert_eq!(page.js.as_ref().unwrap().scroll_offset(), before);
    }

    #[test]
    fn pdf_selects_print_media_and_restores_screen_render_state() {
        let page = page(r#"<style>html,body{margin:0}#print{display:none}
            @media print{#screen{display:none}#print{display:block;height:240px}}</style>
            <div id="screen">SCREEN</div><div id="print">PRINT</div>"#, (200., 100.));
        let before = page.screenshot((200., 100.)).unwrap();
        let bytes = page.pdf(paper(200., 100.)).unwrap();
        let document = lopdf::Document::load_mem(&bytes).unwrap();
        assert_eq!(document.get_pages().len(), 3, "pagination must use the print-only element's height");
        assert_eq!(page.screenshot((200., 100.)).unwrap(), before);
    }

    #[test]
    fn options_reject_invalid_geometry_and_scale() {
        let mut options = PdfOptions::default(); options.paper_width_in = f32::NAN;
        assert_eq!(options.page_geometry(), Err(PdfError::InvalidPaperSize));
        options = PdfOptions::default(); options.margin_left_in = 9.;
        assert_eq!(options.page_geometry(), Err(PdfError::InvalidMargins));
        assert_eq!(pagination_plan(800., 2000., 600., 700., 0.09).unwrap_err(), PdfError::InvalidScale);
    }

    #[test]
    fn ranges_deduplicate_in_document_order_and_bound_selected_work() {
        assert_eq!(selected_page_indices(6, &[PdfPageRange { start: Some(3), end: Some(5) },
            PdfPageRange { start: Some(1), end: Some(3) }]).unwrap(), vec![0,1,2,3,4]);
        assert_eq!(selected_page_indices(3, &[PdfPageRange { start: Some(9), end: None }]), Err(PdfError::EmptyPageRange));
        let plan = pagination_plan(1000., 400000., 600., 700., 1.).unwrap();
        assert_eq!(selected_page_indices(plan.page_count, &[]), Err(PdfError::TooManyPages(MAX_PDF_PAGES)));
        let selected = selected_page_indices(plan.page_count, &[PdfPageRange { start: Some(1), end: Some(1) }]).unwrap();
        validate_selected_paint_work(1000., 400000., plan, &selected).unwrap();
        let plan = pagination_plan(5000., 5000., 600., 700., 1.).unwrap();
        assert_eq!(validate_selected_paint_work(5000., 5000., plan, &[0]), Err(PdfError::WorkLimitExceeded));
    }
}
