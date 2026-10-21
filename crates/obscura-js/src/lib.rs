pub mod cdp_watchdog;
mod embedding_policy;
pub mod frame;
mod import_map;
pub mod indexed_db;
pub mod geolocation;
pub mod regional;
mod rtc;
mod rtc_dns;
mod rtc_transport;
pub mod markdown;
pub mod module_loader;
pub mod ops;
pub mod runtime;
pub mod v8_flags;
pub mod window;
pub mod storage;
mod write_stream;
mod webcrypto;
mod websocket;
mod svg_geometry;
mod text_decoder;
mod local_resource;
mod worker;

#[cfg(test)]
mod indexed_db_tests;
#[cfg(test)]
mod webcrypto_tests;

pub use markdown::HTML_TO_MARKDOWN_JS;
pub use v8_flags::set_v8_flags;

// Font metadata is available without the optional raster/shaping feature.
pub use obscura_render::configure_fonts;
// Screenshot rasterization (PNG bytes) from the render layer. Available when the
// render feature (which enables obscura-render/paint) is compiled in.
#[cfg(feature = "render")]
pub use obscura_render::{
    screenshot_png, screenshot_png_scrolled,
    screenshot_png_scrolled_at_animation_time,
    screenshot_png_scrolled_at_animation_time_with_surface_color,
    screenshot_png_scrolled_at_animation_time_with_surface_color_and_resources,
    validate_capture_region, AnimationSample, AnimationSampleMode, AnimationSampleTime,
    CaptureError, CaptureRegion, CssMediaType, ImageRequestProfile, RenderResourceCache,
    PdfDocument, PdfPaintError,
    MAX_CAPTURE_DIMENSION, MAX_CAPTURE_PIXELS,
};
