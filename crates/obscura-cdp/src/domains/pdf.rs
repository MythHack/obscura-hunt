#[cfg(feature = "render")]
use serde_json::json;
use serde_json::Value;

use crate::dispatch::CdpContext;

#[cfg(feature = "render")]
const MAX_BASE64_PDF_BYTES: usize = 64 * 1024 * 1024;
#[cfg(feature = "render")]
const MAX_PAGE_RANGES_BYTES: usize = 16 * 1024;
#[cfg(feature = "render")]
const MAX_PAGE_RANGE_PARTS: usize = 512;

#[cfg(feature = "render")]
fn base64_encoded_len(raw_len: usize) -> Option<usize> {
    raw_len.checked_add(2)?.checked_div(3)?.checked_mul(4)
}

#[cfg(feature = "render")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PdfTransferMode {
    ReturnAsBase64,
    ReturnAsStream,
}

#[cfg(feature = "render")]
#[derive(Clone, Debug)]
struct ParsedPdfOptions {
    pdf: obscura_browser::PdfOptions,
    transfer_mode: PdfTransferMode,
    requested_print_background: bool,
}

#[cfg(feature = "render")]
fn number(params: &Value, name: &str, default: f32) -> Result<f32, String> {
    match params.get(name) {
        None => Ok(default),
        Some(value) => value
            .as_f64()
            .filter(|value| value.is_finite())
            .map(|value| value as f32)
            .ok_or_else(|| format!("Invalid parameters: {name} must be a finite number")),
    }
}

#[cfg(feature = "render")]
fn boolean(params: &Value, name: &str, default: bool) -> Result<bool, String> {
    match params.get(name) {
        None => Ok(default),
        Some(Value::Bool(value)) => Ok(*value),
        Some(_) => Err(format!("Invalid parameters: {name} must be a boolean")),
    }
}

#[cfg(feature = "render")]
fn parse_page_ranges(value: &str) -> Result<Vec<obscura_browser::PdfPageRange>, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(Vec::new());
    }
    if value.len() > MAX_PAGE_RANGES_BYTES {
        return Err(format!(
            "Invalid parameters: pageRanges exceeds the {MAX_PAGE_RANGES_BYTES}-byte limit"
        ));
    }

    let mut ranges = Vec::new();
    for (index, part) in value.split(',').enumerate() {
        if index >= MAX_PAGE_RANGE_PARTS {
            return Err(format!(
                "Invalid parameters: pageRanges exceeds the {MAX_PAGE_RANGE_PARTS}-range limit"
            ));
        }
        let part = part.trim();
        if part.is_empty() {
            return Err("Invalid parameters: pageRanges contains an empty range".to_string());
        }
        let page = |text: &str| -> Result<Option<usize>, String> {
            let text = text.trim();
            if text.is_empty() {
                return Ok(None);
            }
            text.parse::<usize>()
                .ok()
                .filter(|page| *page > 0)
                .map(Some)
                .ok_or_else(|| {
                    format!("Invalid parameters: pageRanges has invalid page {text:?}")
                })
        };
        let (start, end) = match part.split_once('-') {
            Some((start, end)) if !end.contains('-') => (page(start)?, page(end)?),
            Some(_) => {
                return Err(format!(
                    "Invalid parameters: pageRanges has invalid range {part:?}"
                ))
            }
            None => {
                let page = page(part)?.ok_or_else(|| {
                    "Invalid parameters: pageRanges contains an empty page".to_string()
                })?;
                (Some(page), Some(page))
            }
        };
        if start.is_none() && end.is_none() {
            return Err("Invalid parameters: pageRanges range '-' is empty".to_string());
        }
        if matches!((start, end), (Some(start), Some(end)) if start > end) {
            return Err(format!(
                "Invalid parameters: pageRanges range {part:?} is descending"
            ));
        }
        ranges.push(obscura_browser::PdfPageRange { start, end });
    }

    // Normalize before crossing the CDP/browser boundary. This bounds later
    // selection work and makes repeated or overlapping user ranges occupy one
    // entry while preserving Chromium's document-order, print-once semantics.
    ranges.sort_by_key(|range| (range.start.unwrap_or(1), range.end.unwrap_or(usize::MAX)));
    let mut normalized: Vec<obscura_browser::PdfPageRange> = Vec::new();
    for range in ranges {
        let start = range.start.unwrap_or(1);
        if let Some(previous) = normalized.last_mut() {
            let previous_end = previous.end.unwrap_or(usize::MAX);
            if start <= previous_end.saturating_add(1) {
                previous.end = match (previous.end, range.end) {
                    (None, _) | (_, None) => None,
                    (Some(previous), Some(next)) => Some(previous.max(next)),
                };
                continue;
            }
        }
        normalized.push(obscura_browser::PdfPageRange {
            start: Some(start),
            end: range.end,
        });
    }
    Ok(normalized)
}

#[cfg(feature = "render")]
fn parse_options(params: &Value) -> Result<ParsedPdfOptions, String> {
    if !params.is_object() {
        return Err("Invalid parameters: expected an object".to_string());
    }
    let unsupported_true = [
        ("displayHeaderFooter", "headers and footers"),
        ("preferCSSPageSize", "CSS @page sizing"),
        ("generateDocumentOutline", "document outlines"),
        ("generateTaggedPDF", "tagged PDF"),
    ];
    for (name, capability) in unsupported_true {
        if boolean(params, name, false)? {
            return Err(format!(
                "Page.printToPDF does not yet support {capability} ({name}=true)"
            ));
        }
    }
    let requested_print_background = boolean(params, "printBackground", false)?;
    let scale = number(params, "scale", 1.0)?;
    if !(0.1..=2.0).contains(&scale) {
        return Err("Invalid parameters: scale must be between 0.1 and 2".to_string());
    }
    let page_ranges = match params.get("pageRanges") {
        Some(value) => {
            let ranges = value
                .as_str()
                .ok_or("Invalid parameters: pageRanges must be a string")?;
            parse_page_ranges(ranges)?
        }
        None => Vec::new(),
    };
    for name in ["headerTemplate", "footerTemplate"] {
        if let Some(value) = params.get(name) {
            let template = value
                .as_str()
                .ok_or_else(|| format!("Invalid parameters: {name} must be a string"))?;
            if !template.is_empty() {
                return Err(format!("Page.printToPDF {name} is not yet supported"));
            }
        }
    }
    let transfer_mode = match params.get("transferMode") {
        None => PdfTransferMode::ReturnAsBase64,
        Some(Value::String(value)) => match value.as_str() {
            "ReturnAsBase64" => PdfTransferMode::ReturnAsBase64,
            "ReturnAsStream" => PdfTransferMode::ReturnAsStream,
            _ => return Err("Invalid parameters: unknown transferMode".into()),
        },
        Some(_) => return Err("Invalid parameters: transferMode must be a string".into()),
    };

    let defaults = obscura_browser::PdfOptions::default();
    Ok(ParsedPdfOptions {
        pdf: obscura_browser::PdfOptions {
            landscape: boolean(params, "landscape", defaults.landscape)?,
            print_background: requested_print_background,
            scale,
            page_ranges,
            paper_width_in: number(params, "paperWidth", defaults.paper_width_in)?,
            paper_height_in: number(params, "paperHeight", defaults.paper_height_in)?,
            margin_top_in: number(params, "marginTop", defaults.margin_top_in)?,
            margin_bottom_in: number(params, "marginBottom", defaults.margin_bottom_in)?,
            margin_left_in: number(params, "marginLeft", defaults.margin_left_in)?,
            margin_right_in: number(params, "marginRight", defaults.margin_right_in)?,
        },
        transfer_mode,
        requested_print_background,
    })
}

pub async fn print_to_pdf(
    params: &Value,
    ctx: &mut CdpContext,
    session_id: &Option<String>,
) -> Result<Value, String> {
    #[cfg(feature = "render")]
    {
        let options = parse_options(params)?;
        let page = ctx
            .get_session_page_mut(session_id)
            .ok_or("No page for session")?;
        crate::domains::page::prepare_capture_resources_if_requested(page).await;
        let animation_sample = page.live_animation_sample();
        let pdf = page.pdf_with_animation_sample(options.pdf, animation_sample)
            .map_err(|error| error.to_string())?;
        let mut response = json!({
            "obscuraPrintMode": "print-media-vector",
            "obscuraPrintBackground": options.requested_print_background,
            "obscuraRequestedPrintBackground": options.requested_print_background,
            "obscuraTaggedPDF": false,
            "obscuraCapabilities": {
                "cssPagedMedia": false,
                "honorsPrintMedia": true,
                "honorsPrintBackground": true,
                "honorsScale": true,
                "pageRanges": true,
                "printColorAdjustExact": false,
                "taggedPdf": false,
                "selectableText": true,
                "embeddedSubsetFonts": true,
                "vectorGraphics": true,
            },
        });
        match options.transfer_mode {
            PdfTransferMode::ReturnAsBase64 => {
                use base64::Engine as _;
                let encoded_len = base64_encoded_len(pdf.len())
                    .ok_or("Page.printToPDF base64 response size overflow")?;
                if encoded_len > MAX_BASE64_PDF_BYTES {
                    return Err(format!(
                        "Page.printToPDF base64 response would be {encoded_len} bytes, exceeding the {MAX_BASE64_PDF_BYTES}-byte response limit; use transferMode=ReturnAsStream"
                    ));
                }
                response["data"] =
                    Value::String(base64::engine::general_purpose::STANDARD.encode(pdf));
            }
            PdfTransferMode::ReturnAsStream => {
                let handle = ctx
                    .io_streams
                    .insert(pdf)
                    .map_err(|error| format!("Page.printToPDF could not open stream: {error}"))?;
                response["data"] = Value::String(String::new());
                response["stream"] = Value::String(handle);
            }
        }
        Ok(response)
    }
    #[cfg(not(feature = "render"))]
    {
        let _ = (params, ctx, session_id);
        Err("Page.printToPDF requires a build with the render feature".to_string())
    }
}

#[cfg(all(test, feature = "render"))]
mod tests {
    use super::*;
    use crate::domains::page;
    use base64::Engine as _;

    #[test]
    fn parser_rejects_unsupported_capabilities_and_normalizes_page_ranges() {
        let options = parse_options(&json!({"transferMode":"ReturnAsStream", "scale":0.5,
            "pageRanges":"5-8,1-3,3-6,10,9,12-", "generateTaggedPDF":false})).unwrap();
        assert_eq!(options.transfer_mode, PdfTransferMode::ReturnAsStream);
        assert_eq!(options.pdf.scale, 0.5);
        assert_eq!(options.pdf.page_ranges, vec![
            obscura_browser::PdfPageRange { start: Some(1), end: Some(10) },
            obscura_browser::PdfPageRange { start: Some(12), end: None }]);
        for params in [json!({"generateTaggedPDF":true}), json!({"displayHeaderFooter":true}),
            json!({"preferCSSPageSize":true}), json!({"generateDocumentOutline":true}),
            json!({"headerTemplate":"<span>title</span>"}), json!({"scale":0.09}),
            json!({"pageRanges":"0"}), json!({"pageRanges":"3-2"}), json!({"pageRanges":"1,,2"}),
            json!({"transferMode":"unknown"})] {
            assert!(parse_options(&params).is_err(), "must reject {params}");
        }
        assert!(parse_page_ranges(&"1".repeat(MAX_PAGE_RANGES_BYTES + 1)).is_err());
        assert!(parse_page_ranges(&std::iter::repeat_n("1", MAX_PAGE_RANGE_PARTS + 1)
            .collect::<Vec<_>>().join(",")).is_err());
    }

    #[test]
    fn base64_size_preflight_is_exact_and_overflow_safe() {
        assert_eq!(base64_encoded_len(0), Some(0));
        assert_eq!(base64_encoded_len(1), Some(4));
        assert_eq!(base64_encoded_len(3), Some(4));
        assert_eq!(base64_encoded_len(4), Some(8));
        assert_eq!(base64_encoded_len(usize::MAX), None);
        let largest = (MAX_BASE64_PDF_BYTES / 4) * 3;
        assert_eq!(base64_encoded_len(largest), Some(MAX_BASE64_PDF_BYTES));
        assert!(base64_encoded_len(largest + 1).unwrap() > MAX_BASE64_PDF_BYTES);
    }

    #[tokio::test]
    async fn print_to_pdf_returns_paginated_pdf_with_requested_media_box() {
        let mut ctx = CdpContext::new();
        let page_id = ctx.create_page();
        let session_id = format!("{page_id}-session");
        ctx.sessions.insert(session_id.clone(), page_id);
        let session = Some(session_id);
        ctx.get_session_page_mut(&session).unwrap().set_viewport((100., 80.));
        page::handle("navigate", &json!({"url":
            "data:text/html,<style>html,body{margin:0}div{height:160px}</style><div>Hello</div><div>SECOND</div><div>THIRD</div>",
            "waitUntil":"load"}), &mut ctx, &session).await.unwrap();
        let options = json!({"paperWidth":4., "paperHeight":6.,
            "marginTop":0.5,"marginBottom":0.5,"marginLeft":0.5,"marginRight":0.5,
            "printBackground":true});
        let response = print_to_pdf(&options, &mut ctx, &session).await.unwrap();
        assert_eq!(response["obscuraPrintMode"], "print-media-vector");
        let bytes = base64::engine::general_purpose::STANDARD.decode(response["data"].as_str().unwrap()).unwrap();
        let document = lopdf::Document::load_mem(&bytes).unwrap();
        assert!(document.get_pages().len() >= 2);
        for id in document.get_pages().values() {
            let media = document.get_dictionary(*id).unwrap().get(b"MediaBox").unwrap().as_array().unwrap();
            let values = media.iter().map(|value| match value {
                lopdf::Object::Integer(value) => *value as f32,
                lopdf::Object::Real(value) => *value,
                _ => panic!("MediaBox coordinate must be numeric"),
            }).collect::<Vec<_>>();
            assert_eq!(values, vec![0., 0., 288., 432.]);
        }

        let mut stream_options = options.clone();
        stream_options["transferMode"] = json!("ReturnAsStream");
        let response = print_to_pdf(&stream_options, &mut ctx, &session).await.unwrap();
        let handle = response["stream"].as_str().unwrap();
        let mut streamed = Vec::new();
        loop {
            let chunk = crate::domains::io::handle("read", &json!({"handle":handle,"size":137}), &mut ctx).await.unwrap();
            assert_eq!(chunk["base64Encoded"], true);
            streamed.extend(base64::engine::general_purpose::STANDARD.decode(chunk["data"].as_str().unwrap()).unwrap());
            if chunk["eof"] == true { break; }
        }
        let streamed = lopdf::Document::load_mem(&streamed).unwrap();
        assert_eq!(streamed.get_pages().len(), document.get_pages().len());
        crate::domains::io::handle("close", &json!({"handle":handle}), &mut ctx).await.unwrap();
        assert!(crate::domains::io::handle("read", &json!({"handle":handle}), &mut ctx).await.is_err());

        let mut ranged = options;
        ranged["pageRanges"] = json!("2,2");
        let response = print_to_pdf(&ranged, &mut ctx, &session).await.unwrap();
        let bytes = base64::engine::general_purpose::STANDARD.decode(response["data"].as_str().unwrap()).unwrap();
        let selected = lopdf::Document::load_mem(&bytes).unwrap();
        assert_eq!(selected.get_pages().len(), 1);
    }
}
