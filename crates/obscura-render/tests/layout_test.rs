//! Regression test: a Hacker-News-shaped nested `<table>` layout must come out
//! right using only the general engine (UA defaults + real CSS cascade), with
//! no per-site hardcoded selectors. This guards against reintroducing the
//! site-specific hacks that used to live in obscura-render for this exact markup.

use obscura_dom::tree_sink::parse_html;
use obscura_render::{layout_dom, layout_dom_with_images};
use std::collections::HashMap;

#[test]
fn word_spacing_changes_intrinsic_width() {
    let tree = parse_html(r#"<style>
        span { display:inline-block; font:40px/60px monospace; white-space:nowrap }
        #spaced { word-spacing:30px }
        </style><span id="normal">A B C</span><span id="spaced">A B C</span>"#);
    let layout = layout_dom(&tree, (600.0, 200.0));
    let width = |id| layout.rects[&tree.get_element_by_id(id).unwrap()].width;
    assert!((width("spaced") - width("normal") - 60.0).abs() < 0.1);
}

#[test]
fn word_spacing_rejects_invalid_and_unsupported_lengths() {
    for value in ["bogus", "30", "10%", "3 px", "min(30px,bogus)", "calc(30px) bogus",
        "calc(30)", "calc(2px * 3px)", "min(1px,max(2px,bogus))", "1px 2px", "1e99px"]
    {
        let tree = parse_html(&format!("<div id='copy' style='word-spacing:3px;word-spacing:{value}'>A B</div>"));
        let layout = layout_dom(&tree, (400.0, 200.0));
        assert_eq!(layout.styles[&tree.get_element_by_id("copy").unwrap()].word_spacing, Some(3.0), "{value}");
        assert!(!obscura_render::style::supports_declaration("word-spacing", value), "{value}");
    }
    for value in ["normal", "0", "-3px", ".75em", "2rem"] {
        assert!(obscura_render::style::supports_declaration("word-spacing", value), "{value}");
    }
}

#[test]
fn word_spacing_resolves_before_inheritance_and_preserves_cascade() {
    let tree = parse_html(r#"<style>
        section { font-size:20px; word-spacing:1.5em }
        span { display:inline-block; font-size:40px; white-space:nowrap }
        #normal { word-spacing:normal }
        #inherit { word-spacing:3px; word-spacing:inherit }
        #unset { word-spacing:3px; word-spacing:unset }
        #invalid { word-spacing:3px; word-spacing:bogus; word-spacing:calc(bogus) }
        #negative { word-spacing:-.25em }
        #revert { word-spacing:3px; word-spacing:revert }
        </style><section>
        <span id="inherited">A B C</span><span id="normal">A B C</span>
        <span id="inherit">A B C</span><span id="unset">A B C</span>
        <span id="invalid">A B C</span><span id="negative">A B C</span>
        <span id="revert">A B C</span></section>"#);
    let layout = layout_dom(&tree, (1800.0, 300.0));
    let width = |id| layout.rects[&tree.get_element_by_id(id).unwrap()].width;
    for (id, spacing) in [("inherited", 30.0), ("normal", 0.0), ("inherit", 30.0),
        ("unset", 30.0), ("invalid", 3.0), ("negative", -10.0), ("revert", 30.0)]
    {
        assert_eq!(layout.styles[&tree.get_element_by_id(id).unwrap()].word_spacing, Some(spacing), "{id}");
        assert!((width(id) - width("normal") - 2.0 * spacing).abs() <= 1.0, "{id}");
    }
}

#[cfg(feature = "paint")]
#[test]
fn word_spacing_changes_wrapping_and_native_control_widths() {
    let tree = parse_html(r#"<style>
        div { width:140px; font:40px/60px monospace }
        .spaced { word-spacing:30px }
        button, select { font:40px/60px monospace }
        </style><div id="normal">A B C</div><div id="spaced" class="spaced">A B C</div>
        <button id="button">A B C</button><button id="button-spaced" class="spaced">A B C</button>
        <select id="select"><option>A B C</option></select>
        <select id="select-spaced" class="spaced"><option>A B C</option></select>"#);
    let layout = layout_dom(&tree, (1000.0, 500.0));
    let rect = |id| layout.rects[&tree.get_element_by_id(id).unwrap()];
    assert_eq!(rect("normal").height, 60.0);
    assert_eq!(rect("spaced").height, 120.0);
    for (normal, spaced) in [("button", "button-spaced"), ("select", "select-spaced")] {
        assert!((rect(spaced).width - rect(normal).width - 60.0).abs() <= 1.0,
            "{normal}: {:?} -> {:?}", rect(normal), rect(spaced));
    }
}

#[cfg(feature = "paint")]
#[test]
fn word_spacing_handles_whitespace_nested_spans_and_generated_content() {
    for (content, extra_css, delta) in [
        ("  A   B  C  ", "", 60.0),
        ("A&nbsp;B&nbsp;C", "", 60.0),
        ("A  B", "white-space:pre", 60.0),
        ("A\tB", "white-space:pre", 0.0),
        ("A B C", "letter-spacing:2px", 60.0),
        ("A<span> B</span> C", "", 60.0),
        ("אב גד", "direction:rtl", 30.0),
    ] {
        let tree = parse_html(&format!(r#"<style>
            div {{ display:inline-block; font:40px/60px monospace; white-space:nowrap; {extra_css} }}
            #spaced {{ word-spacing:30px }}
            </style><div id="normal">{content}</div><div id="spaced">{content}</div>"#));
        let layout = layout_dom(&tree, (1000.0, 300.0));
        let width = |id| layout.rects[&tree.get_element_by_id(id).unwrap()].width;
        assert!((width("spaced") - width("normal") - delta).abs() <= 1.0, "{content:?}, {extra_css}");
    }
    let tree = parse_html(r#"<style>
        div { display:inline-block; font:40px/60px monospace; white-space:nowrap }
        #spaced { word-spacing:30px }
        div::before { content:'A B C'; font-size:20px }
        div::after { content:'A B C'; word-spacing:normal }
        </style><div id="normal"></div><div id="spaced"></div>"#);
    let layout = layout_dom(&tree, (1000.0, 300.0));
    let id = tree.get_element_by_id("spaced").unwrap();
    assert_eq!(layout.styles[&id].before_pseudo.as_ref().unwrap().word_spacing, Some(30.0));
    assert_eq!(layout.styles[&id].after_pseudo.as_ref().unwrap().word_spacing, Some(0.0));
    let normal = layout.rects[&tree.get_element_by_id("normal").unwrap()].width;
    assert!((layout.rects[&id].width - normal - 60.0).abs() <= 1.0);
}

#[cfg(feature = "paint")]
#[test]
fn word_spacing_moves_painted_glyphs() {
    let tree = parse_html(r#"<style>
        body { margin:0; background:white; color:black }
        div { height:80px; font:40px/60px monospace; white-space:nowrap }
        #spaced { word-spacing:30px }
        </style><div>A B C</div><div id="spaced">A B C</div>"#);
    let png = obscura_render::screenshot_png(&tree, (400.0, 180.0), None).unwrap();
    let pixmap = tiny_skia::Pixmap::decode_png(&png).unwrap();
    let starts = |top: usize| {
        let mut result = Vec::new();
        let mut previous = false;
        for x in 0..400 {
            let ink = (top..top + 70).any(|y| {
                let pixel = pixmap.pixels()[y * 400 + x];
                pixel.red() < 128 && pixel.green() < 128 && pixel.blue() < 128
            });
            if ink && !previous { result.push(x as i32); }
            previous = ink;
        }
        result
    };
    let normal = starts(0);
    let spaced = starts(80);
    assert_eq!(normal.len(), 3, "normal glyph columns: {normal:?}");
    assert_eq!(spaced.len(), 3, "spaced glyph columns: {spaced:?}");
    for i in 0..3 {
        assert!((spaced[i] - normal[i] - i as i32 * 30).abs() <= 1,
            "painted positions: {normal:?} -> {spaced:?}");
    }
}

const HN_HTML: &str = r##"
    <table border="0" cellpadding="0" cellspacing="0" width="85%" bgcolor="#f6f6ef">
        <tr>
            <td bgcolor="#ff6600">
                <table border="0" cellpadding="0" cellspacing="0" width="100%" style="padding:2px">
                    <tr>
                        <td style="width:18px;padding-right:4px"><a href="https://news.ycombinator.com"><img src="y18.svg" width="18" height="18"></a></td>
                        <td style="line-height:12pt; height:10px;"><span class="pagetop"><b class="hnname"><a href="news">Hacker News</a></b> <a href="newest">new</a> | <a href="front">past</a></span></td>
                        <td style="text-align:right;padding-right:4px;"><span class="pagetop"><a href="login?goto=news">login</a></span></td>
                    </tr>
                </table>
            </td>
        </tr>
        <tr>
            <td>
                <table border="0" cellpadding="0" cellspacing="0">
                    <tr class="athing submission" id="48761229">
                        <td align="right" valign="top" class="title"><span class="rank">24.</span></td>
                        <td valign="top" class="votelinks"><center><a id="up_48761229" href="vote?id=48761229"><div class="votearrow" title="upvote"></div></a></center></td>
                        <td class="title"><span class="titleline"><a href="https://www.zachtronics.com/exapunks/">Exapunks (2018)</a></span></td>
                    </tr>
                </table>
            </td>
        </tr>
    </table>
"##;

#[cfg(feature = "paint")]
#[test]
fn mixed_content_shaped_runs_retain_inline_owner_geometry() {
    for content in ["ABCD", "AB<b id='nested'>CD</b>"] {
        let tree = parse_html(&format!(
            "<style>body{{margin:0;font:20px/24px monospace}}</style>\
             <div id='parent'>AAAA<div style='height:30px'></div>\
             <span id='owner'>{content}</span></div>"));
        let layout = layout_dom(&tree, (200.0, 200.0));
        let owner = tree.get_element_by_id("owner").unwrap();
        let rect = layout.rects.get(&owner).expect("shaped inline owner has geometry");
        assert!(rect.x.abs() < 0.01 && (52.0..=57.0).contains(&rect.y)
            && (40.0..=55.0).contains(&rect.width) && (18.0..=26.0).contains(&rect.height),
            "inline following a 24px line and 30px block: {rect:?}");
        let pieces = layout.inline_fragments.get(&owner).expect("canonical inline fragments");
        assert!(!pieces.is_empty());
        for piece in pieces {
            assert!(piece.width > 0.0 && piece.height > 0.0
                && piece.x >= rect.x && piece.y >= rect.y
                && piece.x + piece.width <= rect.x + rect.width + 0.01
                && piece.y + piece.height <= rect.y + rect.height + 0.01,
                "inline fragments remain within their owner bounds: {piece:?}, {rect:?}");
        }
        let parent = tree.get_element_by_id("parent").unwrap();
        assert!((layout.rects[&parent].height - 78.0).abs() < 0.01,
            "preserving inline ownership must not add a line or change block flow");
        if let Some(nested) = tree.get_element_by_id("nested") {
            let nested = layout.rects.get(&nested).expect("nested inline owner has geometry");
            assert!((20.0..=28.0).contains(&nested.x) && (20.0..=28.0).contains(&nested.width)
                && (nested.y - rect.y).abs() < 1.0, "nested text bounds: {nested:?}");
        }
    }
}

/// Top-left of the tightest laid-out element box whose text contains
/// `needle`. Text geometry is no longer a per-word list: a pure-text
/// container collapses to a single cosmic-text inline formatting context
/// (see `inline`), and even in the word-split path the wrapping `<a>`/`<span>`
/// are flattened into their block, so the smallest element rect enclosing the
/// text is the meaningful, mode-independent anchor. Picking the smallest-area
/// match skips the giant ancestor tables that also "contain" the text.
fn find_by_text(
    tree: &obscura_dom::tree::DomTree,
    layout: &obscura_render::DomLayout,
    needle: &str,
) -> Option<(f32, f32)> {
    let mut best: Option<(f32, obscura_render::Rect)> = None;
    for (id, rect) in &layout.rects {
        if tree.text_content(*id).contains(needle) {
            let area = rect.width * rect.height;
            if best.as_ref().map(|(a, _)| area < *a).unwrap_or(true) {
                best = Some((area, *rect));
            }
        }
    }
    best.map(|(_, r)| (r.x, r.y))
}

#[test]
fn hn_shaped_table_lays_out_without_site_hardcoding() {
    let tree = parse_html(HN_HTML);
    let layout = layout_dom(&tree, (1000.0, 1000.0));

    // Every element got a rect; the tree isn't just being dropped.
    assert!(layout.rects.len() > 10, "expected many laid-out elements, got {}", layout.rects.len());

    // "Hacker News" (a bold link in the top bar) sits above the first
    // headline row ("Exapunks (2018)"): normal top-to-bottom flow, not a
    // hardcoded absolute position.
    let (brand_x, brand_y) = find_by_text(&tree, &layout, "Hacker News").expect("brand text laid out");
    let (_, headline_y) = find_by_text(&tree, &layout, "Exapunks (2018)").expect("headline text laid out");
    assert!(
        headline_y > brand_y,
        "headline should be below the header bar: brand.y={} headline.y={}",
        brand_y,
        headline_y
    );

    // Within the header row, "login" (right-aligned cell) sits to the right
    // of "Hacker News" (left cell) — plain flex/table layout, not a magic
    // per-class x-offset.
    let (login_x, _) = find_by_text(&tree, &layout, "login").expect("login text laid out");
    assert!(
        login_x > brand_x,
        "login cell should be right of the brand: brand.x={} login.x={}",
        brand_x,
        login_x
    );
}

#[test]
fn table_width_uses_local_space_and_keeps_width_hints_shrinkable() {
    let tree = parse_html(
        r#"
        <style>
          html,body{margin:0;font:16px/20px Arial}
          table{border-spacing:0;box-sizing:border-box} td{padding:0;box-sizing:border-box}
        </style>
        <div id="local" style="width:300px">
          <table id="local-table"><tr><td id="local-cell">alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu</td></tr></table>
        </div>
        <table id="outer" style="width:300px"><tr><td id="outer-cell">
          <table id="inner"><tr><td id="inner-cell">alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu</td></tr></table>
        </td></tr></table>
        <div id="narrow" style="width:300px">
          <table id="overflow" style="width:500px"><tr><td>fixed overflow</td></tr></table>
        </div>
        <table id="preferred" style="width:200px"><tr>
          <td id="preferred-wide" style="width:300px">A</td>
          <td id="preferred-narrow" style="width:50px">B</td>
        </tr></table>
        "#,
    );
    let layout = layout_dom(&tree, (400.0, 1000.0));
    let rect = |id| layout.rects[&tree.get_element_by_id(id).unwrap()];

    let local = rect("local-table");
    let local_cell = rect("local-cell");
    assert!((local.width - 300.0).abs() < 0.01, "local table: {local:?}");
    assert!(
        local.height >= 39.0 && (local_cell.width - 300.0).abs() < 0.01,
        "wrappable content must use the 300px local containing block: table={local:?} cell={local_cell:?}"
    );

    for id in ["outer", "outer-cell", "inner", "inner-cell"] {
        let box_rect = rect(id);
        assert!(
            (box_rect.width - 300.0).abs() < 0.01 && box_rect.height >= 39.0,
            "nested table sizing must settle outer-first for {id}: {box_rect:?}"
        );
    }
    let inner = rect("inner");
    for id in ["outer", "outer-cell"] {
        let container = rect(id);
        assert!(
            (container.y - inner.y).abs() < 0.01
                && (container.height - inner.height).abs() < 0.01,
            "normal source indentation around a nested block table must not create a line: \
             {id}={container:?}, inner={inner:?}"
        );
    }

    let overflow = rect("overflow");
    assert!(
        (overflow.width - 500.0).abs() < 0.01,
        "a definite table width may overflow its local CB and viewport: {overflow:?}"
    );

    let preferred = rect("preferred");
    let wide = rect("preferred-wide");
    let narrow = rect("preferred-narrow");
    assert!((preferred.width - 200.0).abs() < 0.01, "preferred table: {preferred:?}");
    assert!(
        wide.width > 160.0
            && wide.width < 175.0
            && narrow.width > 25.0
            && narrow.width < 40.0
            && (wide.width + narrow.width - 200.0).abs() < 0.01,
        "cell width hints are preferred contributions, not minimums: wide={wide:?} narrow={narrow:?}"
    );
}

#[test]
fn table_rowspans_are_clipped_to_the_originating_row_group() {
    let tree = parse_html(
        r#"
        <style>
          html,body{margin:0} table{border-spacing:0} td{padding:0}
          .span{width:20px}.short{width:20px;height:20px}.last{width:20px;height:60px}
        </style>
        <table id="zero"><tbody>
          <tr><td id="zero-span" class="span" rowspan="0"></td><td class="short"></td></tr>
          <tr><td class="short"></td></tr>
        </tbody><tbody><tr><td id="zero-next" class="last"></td></tr></tbody></table>
        <table id="oversized"><tbody>
          <tr><td id="oversized-span" class="span" rowspan="999"></td><td class="short"></td></tr>
          <tr><td class="short"></td></tr>
        </tbody><tbody><tr><td id="oversized-next" class="last"></td></tr></tbody></table>
        "#,
    );
    let layout = layout_dom(&tree, (400.0, 1000.0));
    let rect = |id| layout.rects[&tree.get_element_by_id(id).unwrap()];

    for (table_id, span_id, next_id) in [
        ("zero", "zero-span", "zero-next"),
        ("oversized", "oversized-span", "oversized-next"),
    ] {
        let table = rect(table_id);
        let spanning = rect(span_id);
        let next_group = rect(next_id);
        assert!(
            (spanning.height - 40.0).abs() < 0.01,
            "{span_id} must stop at the tbody boundary: {spanning:?}"
        );
        assert!(
            (next_group.x - table.x).abs() < 0.01,
            "{next_id} must restart at column zero in its row group: table={table:?} cell={next_group:?}"
        );
    }
}

#[test]
fn relative_units_resolve_against_viewport_and_font_size() {
    // 50vw of a 1000px viewport = 500px; 10em at the default 16px = 160px.
    // Both were previously mis-resolved (vw kept as raw px, em hardcoded to 16
    // regardless of context), so this guards the deferred-resolution pass.
    let html = r##"<div style="width:50vw;height:10em"></div>"##;
    let tree = parse_html(html);
    let layout = layout_dom(&tree, (1000.0, 800.0));
    let hit = layout.rects.values().any(|r| (r.width - 500.0).abs() < 1.0 && (r.height - 160.0).abs() < 1.0);
    assert!(hit, "expected a 500x160 box from 50vw/10em, rects: {:?}", layout.rects.values().map(|r| (r.width, r.height)).collect::<Vec<_>>());
}

#[test]
fn dynamic_viewport_units_use_the_live_viewport() {
    let tree = parse_html(
        r#"<body style="margin:0"><div style="width:25dvw;height:15dvh"></div></body>"#,
    );
    let layout = layout_dom(&tree, (1000.0, 800.0));
    assert!(
        layout
            .rects
            .values()
            .any(|rect| (rect.width - 250.0).abs() < 0.01
                && (rect.height - 120.0).abs() < 0.01),
        "dvw/dvh should resolve against the capture viewport"
    );
}

#[test]
fn fixed_inset_box_uses_viewport_not_document_height() {
    let tree = parse_html(
        r#"
        <body style="margin:0">
          <div style="height:5000px"></div>
          <div id="overlay" style="position:fixed;inset:0">
            <canvas id="surface" style="width:100%;height:100%"></canvas>
          </div>
        </body>
        "#,
    );
    let layout = layout_dom(&tree, (900.0, 1000.0));
    for id in ["overlay", "surface"] {
        let rect = layout.rects[&tree.get_element_by_id(id).unwrap()];
        assert!(
            (rect.width - 900.0).abs() < 0.01
                && (rect.height - 1000.0).abs() < 0.01,
            "{id} should fill the viewport rather than the 5000px document: {rect:?}"
        );
    }
}

#[test]
fn percentage_padding_top_reserves_aspect_ratio_box() {
    // The responsive aspect-ratio trick: an empty box with padding-top:56.25%
    // inside a 1000px-wide block reserves a 16:9 area (~562px tall), the room a
    // `position:absolute; inset:0` media child fills. Percentage padding
    // resolves against the containing block WIDTH on every side, so the box
    // gains real height instead of collapsing to zero.
    let html = r##"<div style="width:1000px"><div style="padding-top:56.25%"></div></div>"##;
    let tree = parse_html(html);
    let layout = layout_dom(&tree, (1200.0, 800.0));
    let hit = layout
        .rects
        .values()
        .any(|r| (r.width - 1000.0).abs() < 1.0 && (r.height - 562.5).abs() < 2.0);
    assert!(
        hit,
        "expected a ~1000x562 aspect-ratio box from padding-top:56.25%, rects: {:?}",
        layout.rects.values().map(|r| (r.width, r.height)).collect::<Vec<_>>()
    );
}

#[test]
fn percentage_padding_uses_final_flex_containing_block_width() {
    let tree = parse_html(
        r#"
        <body style="margin:0">
          <div style="display:flex;width:690px">
            <div id="first" style="display:flex;flex-direction:column;width:100%;max-width:500px">
              <div id="first-sizer" style="width:100%;padding-top:55%"></div>
            </div>
          </div>
          <div style="display:flex;width:690px">
            <div id="second" style="display:flex;flex-direction:column;width:100%;max-width:450px">
              <div id="second-sizer" style="width:100%;padding-top:106.7%"></div>
            </div>
          </div>
        </body>
        "#,
    );
    let layout = layout_dom(&tree, (1000.0, 1000.0));
    let rect = |id| layout.rects[&tree.get_element_by_id(id).unwrap()];
    let first = rect("first");
    let first_sizer = rect("first-sizer");
    let second = rect("second");
    let second_sizer = rect("second-sizer");

    assert!((first.width - 500.0).abs() < 0.01, "first flex item: {first:?}");
    assert!(
        (first_sizer.height - 275.0).abs() < 0.01,
        "55% padding must use the final 500px containing block: {first_sizer:?}"
    );
    assert!((second.width - 450.0).abs() < 0.01, "second flex item: {second:?}");
    assert!(
        (second_sizer.height - 480.15).abs() < 0.51,
        "106.7% padding must use the final 450px containing block: {second_sizer:?}"
    );

    for (id, expected) in [("first-sizer", 275.0), ("second-sizer", 480.15)] {
        let node = tree.get_element_by_id(id).unwrap();
        assert!(
            (layout.styles[&node].padding.top - expected).abs() < 0.05,
            "paint/content consumers must see final used padding for {id}: {:?}",
            layout.styles[&node].padding
        );
    }
}

#[test]
fn percentage_margin_resolves_against_block_content_box_not_outer_padding_box() {
    let tree = parse_html(
        r#"
        <body style="margin:0">
          <div id="parent" style="width:200px;padding:10px">
            <div id="horizontal" style="width:10px;height:10px;margin-left:10%"></div>
            <div id="vertical" style="width:10px;height:10px;margin-top:calc(10%)"></div>
          </div>
        </body>
        "#,
    );
    let layout = layout_dom(&tree, (600.0, 500.0));
    let parent = layout.rects[&tree.get_element_by_id("parent").unwrap()];
    let horizontal = layout.rects[&tree.get_element_by_id("horizontal").unwrap()];
    let vertical = layout.rects[&tree.get_element_by_id("vertical").unwrap()];
    assert!((parent.width - 220.0).abs() < 0.01, "content 200 + 20 padding: {parent:?}");
    let horizontal_style = &layout.styles[&tree.get_element_by_id("horizontal").unwrap()];
    assert!(
        (horizontal_style.margin.left - 20.0).abs() < 0.01,
        "10% of the 200px content box, not the 220px outer box: {:?}",
        horizontal_style.margin
    );
    assert!(
        (horizontal.x - (parent.x + 10.0 + 20.0)).abs() < 0.01,
        "used inset must include the content-box margin: {horizontal:?} parent={parent:?}"
    );
    let vertical_style = &layout.styles[&tree.get_element_by_id("vertical").unwrap()];
    assert!(
        (vertical_style.margin.top - 20.0).abs() < 0.01,
        "vertical/calc percentages use the same content-box width: {:?}",
        vertical_style.margin
    );
    assert!(
        (vertical.y - (horizontal.y + horizontal.height + 20.0)).abs() < 0.01,
        "vertical margin must use the final content-box basis: {vertical:?} horizontal={horizontal:?}"
    );
}

#[test]
fn percentage_margin_does_not_absorb_collapsed_descendant_margin() {
    let tree = parse_html(
        r#"
        <body style="margin:0">
          <div id="parent" style="width:200px;margin-top:10%">
            <div id="child" style="height:10px;margin-top:50px"></div>
          </div>
        </body>
        "#,
    );
    let layout = layout_dom(&tree, (400.0, 500.0));
    let parent = tree.get_element_by_id("parent").unwrap();
    let child = tree.get_element_by_id("child").unwrap();
    let parent_rect = layout.rects[&parent];
    let child_rect = layout.rects[&child];
    assert!(
        (layout.styles[&parent].margin.top - 40.0).abs() < 0.01,
        "parent own margin is 10% of the 400px body content box, not the 50px collapsed set: {:?}",
        layout.styles[&parent].margin
    );
    assert!(
        (parent_rect.y - 50.0).abs() < 0.01 && (child_rect.y - 50.0).abs() < 0.01,
        "positions still collapse to the larger 50px descendant margin: parent={parent_rect:?} child={child_rect:?}"
    );
}

#[test]
fn calc_margin_min_max_is_not_linearized_at_a_large_basis() {
    // Sampling a few basis widths cannot distinguish 10% from min(10%,100px).
    // At a 2000px parent the cap must remain 100px (a sampled linear factor
    // produced 200px); max() must still select the 200px percentage arm.
    let tree = parse_html(
        r#"
        <body style="margin:0">
          <div id="container" style="width:2000px">
            <div id="cap" style="width:10px;height:10px;margin-left:min(10%, 100px)"></div>
            <div id="floor" style="width:10px;height:10px;margin-left:max(10%, 100px)"></div>
          </div>
        </body>
        "#,
    );
    let layout = layout_dom(&tree, (2200.0, 700.0));
    let rect = |id| layout.rects[&tree.get_element_by_id(id).unwrap()];
    for (id, expected) in [("cap", 100.0), ("floor", 200.0)] {
        let style = &layout.styles[&tree.get_element_by_id(id).unwrap()];
        assert!(
            (style.margin.left - expected).abs() < 0.01,
            "{id}: nonlinear min()/max() margin must stay a general calc expression: {:?}",
            style.margin
        );
        assert!(
            (rect(id).x - expected).abs() < 0.01,
            "{id}: used position must use the final-basis calc result: {:?}",
            rect(id)
        );
    }
}

#[test]
fn calc_margin_keeps_font_relative_units_after_context_setup() {
    // The cascade-created calc handle receives this element's own 32px font
    // context before layout. Reparsing it in the margin loop would reset em to
    // the 16px default and produce 72px instead of 104px.
    let tree = parse_html(
        r#"
        <body style="margin:0">
          <div id="container" style="width:400px;font-size:32px">
            <div id="element" style="width:10px;height:10px;margin-left:calc(10% + 2em)"></div>
            <div id="host" style="width:400px;font-size:32px"></div>
          </div>
          <style>
            #host::before { content:""; display:block; height:10px; margin-left:calc(10% + 2em) }
          </style>
        </body>
        "#,
    );
    let layout = layout_dom(&tree, (800.0, 500.0));
    let element = layout.rects[&tree.get_element_by_id("element").unwrap()];
    let element_style = &layout.styles[&tree.get_element_by_id("element").unwrap()];
    assert!(
        (element_style.margin.left - 104.0).abs() < 0.01,
        "calc margin must keep the initialized 32px em basis: {:?}",
        element_style.margin
    );
    assert!((element.x - 104.0).abs() < 0.01, "{element:?}");
    let host = tree.get_element_by_id("host").unwrap();
    let before = layout.styles[&host].before_pseudo.as_ref().unwrap();
    assert!(
        (before.margin.left - 104.0).abs() < 0.01,
        "pseudo calc margin must keep its initialized font context: {:?}",
        before.margin
    );
    assert!(
        (layout.rects[&host].height - 10.0).abs() < 0.01,
        "pseudo must still participate: {:?}",
        layout.rects[&host]
    );
}

#[test]
fn inline_svg_siblings_share_a_row_and_keep_authored_height_under_max_width() {
    // Chromium UA display for svg is inline; no author display is present.
    // height:27px + max-width:64px must not re-derive height from the clamped
    // ratio-derived width (27px stayed 27px, not 17/18/19px), and the three
    // anchors must stay on one inline row instead of one block per line.
    let tree = parse_html(include_str!("../../../render-repros/inline-svg-row.html"));
    let layout = layout_dom(&tree, (1000.0, 600.0));
    let rect = |id| layout.rects[&tree.get_element_by_id(id).unwrap()];
    let logos = ["logo1", "logo2", "logo3"].map(rect);
    for (logo, (id, expected_width)) in logos
        .iter()
        .zip([("logo1", 64.0f32), ("logo2", 64.0f32), ("logo3", 62.765_625f32)])
    {
        assert!(
            (logo.height - 27.0).abs() < 0.01,
            "{id} must keep the authored 27px height when max-width caps width: {logo:?}"
        );
        assert!(
            logo.width > 0.0 && logo.width <= 64.0 + 0.01,
            "{id} must stay measurable and max-width capped: {logo:?}"
        );
        assert!(
            (logo.width - expected_width).abs() < 0.51,
            "{id} must match Chromium's used width (rect edges round): {logo:?}"
        );
    }
    assert!(
        (logos[0].y - logos[1].y).abs() < 0.01 && (logos[1].y - logos[2].y).abs() < 0.01,
        "inline SVG siblings must share one row: {logos:?}"
    );
    assert!(
        logos[1].x > logos[0].x && logos[2].x > logos[1].x,
        "inline SVG siblings must flow left-to-right: {logos:?}"
    );

    // The public fixture contains a collapsed source space between anchors.
    // Its advance and the 16px outer margin must both participate in the
    // shrink-to-fit width. These values come from the same fixture in Chromium.
    let logos_box = rect("logos");
    assert!(
        (logos_box.width - 247.671_875).abs() < 0.51,
        "margins and collapsed spaces must contribute to shrink-to-fit width: {logos_box:?}"
    );
    assert!(
        (logos[0].x - 572.328_125).abs() < 0.51,
        "right-aligned logos row must start at the Chromium x: {logos:?}"
    );
    for (index, pair) in logos.windows(2).enumerate() {
        let gap = pair[1].x - (pair[0].x + pair[0].width);
        assert!(
            (gap - 20.453_125).abs() < 1.01,
            "anchor margin plus collapsed whitespace must separate logo {index}: {logos:?}"
        );
    }

    // Baseline-aligned replaced boxes leave the containing line's font descent
    // below their bottom margin edge. The footer's 27px SVG line is 31px in
    // Chromium, which also places the next 30px-margin footer at y=81.
    for id in ["footer1", "footer2"] {
        let footer = rect(id);
        assert!((footer.height - 31.0).abs() < 0.51,
            "baseline SVG line box must include the 4px font descent: {id}={footer:?}");
    }
    let second_row = rect("logo4");
    assert!(
        (second_row.y - 81.0).abs() < 0.51,
        "second footer must follow the 31px line box and 30px margin: {second_row:?}"
    );
}

#[cfg(feature = "paint")]
#[test]
fn inline_svg_whitespace_collapses_across_anchor_boundaries() {
    // Chromium, 16px Arial: one space advances 4.453125px. Non-atomic
    // anchors share the line's collapse state even when margins keep their
    // boxes, while preserved whitespace retains all nine authored spaces.
    for white_space in ["normal", "nowrap", "pre", "pre-wrap", "break-spaces"] {
        for margin in [0.0, 16.0] {
            let preserved = matches!(white_space, "pre" | "pre-wrap" | "break-spaces");
            let tree = parse_html(&format!(r#"<!doctype html><style>
                body{{margin:0;font:16px Arial,sans-serif}}
                .footer{{display:flex;justify-content:space-between;width:400px}}
                #logos{{white-space:{white_space}}}
                a{{margin-right:{margin}px}} svg{{width:64px;height:27px}}
                </style><div class="footer"><span>Supported by</span><div id="logos"><a><span> </span> <svg id="first"></svg> <span> </span></a> <a> <span> </span><svg id="second"></svg> <span> </span></a></div></div>"#));
            let layout = layout_dom(&tree, (800.0, 300.0));
            let rect = |id| layout.rects[&tree.get_element_by_id(id).unwrap()];
            let group = rect("logos");
            let first = rect("first");
            let second = rect("second");
            let expected_width = 128.0 + 2.0 * margin
                + if preserved { 40.078_125 } else { 4.453_125 };
            let expected_gap = margin + if preserved { 22.265_625 } else { 4.453_125 };
            let expected_leading = if preserved { 8.906_25 } else { 0.0 };
            assert!((group.width - expected_width).abs() < 0.75,
                "{white_space}, margin {margin}: shrink-to-fit group={group:?}");
            assert!((group.x + group.width - 400.0).abs() < 0.75,
                "{white_space}, margin {margin}: flex alignment group={group:?}");
            assert!((first.x - group.x - expected_leading).abs() < 0.75,
                "{white_space}, margin {margin}: leading whitespace first={first:?}, group={group:?}");
            assert!((second.x - first.x - first.width - expected_gap).abs() < 0.75,
                "{white_space}, margin {margin}: inter-anchor whitespace first={first:?}, second={second:?}");
            assert!((first.width - 64.0).abs() < 0.01 && (second.width - 64.0).abs() < 0.01,
                "{white_space}, margin {margin}: replaced widths first={first:?}, second={second:?}");
            assert!((first.y - second.y).abs() < 0.01,
                "{white_space}, margin {margin}: SVGs must share a row: {first:?}, {second:?}");
        }
    }
}

#[test]
fn inline_svg_baseline_reserves_font_descent_only_for_baseline_alignment() {
    // Chromium oracle for a 27px SVG in a 16px Arial line: baseline alignment
    // produces a 31px line (27px ascent + 4px descent), while top/middle/bottom
    // alignment produce a 27px line and must not inherit a synthetic strut.
    let tree = parse_html(
        r#"
        <!doctype html><body style="margin:0;font:16px Arial,sans-serif">
          <div id="baseline"><a><svg style="height:27px;line-height:27px;width:54px" viewBox="0 0 2 1"></svg></a></div>
          <div id="top"><a><svg style="height:27px;line-height:27px;width:54px;vertical-align:top" viewBox="0 0 2 1"></svg></a></div>
          <div id="middle"><a><svg style="height:27px;line-height:27px;width:54px;vertical-align:middle" viewBox="0 0 2 1"></svg></a></div>
          <div id="bottom"><a><svg style="height:27px;line-height:27px;width:54px;vertical-align:bottom" viewBox="0 0 2 1"></svg></a></div>
        </body>
        "#,
    );
    let layout = layout_dom(&tree, (800.0, 400.0));
    let height = |id| layout.rects[&tree.get_element_by_id(id).unwrap()].height;
    assert!(
        (height("baseline") - 31.0).abs() < 0.51,
        "baseline SVG line must include font descent: {}",
        height("baseline")
    );
    for id in ["top", "middle", "bottom"] {
        assert!(
            (height(id) - 27.0).abs() < 0.51,
            "{id}-aligned SVG line must stay at the replaced height: {}",
            height(id)
        );
    }
}

#[test]
fn inline_horizontal_margins_survive_flattening_for_percentage_and_calc() {
    // A shaped inline formatting context must preserve final-used percentage
    // and calc margins, including collapsed inter-anchor whitespace.
    let tree = parse_html(
        r#"
        <body style="margin:0;font:16px Arial,sans-serif">
          <div id="row" style="width:300px">
            <span id="percent" style="margin-right:10%"><svg style="height:27px;line-height:27px;width:54px" viewBox="0 0 2 1"></svg></span>
            <span id="calc" style="margin-left:calc(5% + 5px)"><svg style="height:27px;line-height:27px;width:54px" viewBox="0 0 2 1"></svg></span>
          </div>
        </body>
        "#,
    );
    let layout = layout_dom(&tree, (800.0, 400.0));
    let rect = |id| layout.rects[&tree.get_element_by_id(id).unwrap()];
    let first = rect("percent");
    let second = rect("calc");
    let gap = second.x - (first.x + first.width);
    assert!(
        (gap - 54.453_125).abs() < 1.01,
        "30px percentage margin + 20px calc margin + collapsed space must advance the second owner: first={first:?} second={second:?} gap={gap}"
    );
}

#[test]
fn shaped_inline_margins_resolve_after_flex_shrink() {
    for (margin, expected_wide, expected_narrow) in [
        ("10%", 20.0, 10.0),
        ("calc(10% + 5px)", 25.0, 15.0),
        ("clamp(12px, 10%, 18px)", 18.0, 12.0),
        ("-10%", -20.0, -10.0),
    ] {
        for (row_width, expected_margin) in [(400.0, expected_wide), (200.0, expected_narrow)] {
            let html = format!(r#"<!doctype html><style>
                body{{margin:0;font:16px Arial}} .row{{display:flex;width:{row_width}px}}
                .cell{{flex:1;min-width:0}} span{{margin-left:{margin}}}
                </style><div class="row"><div id="left" class="cell"><span id="a">A</span></div>
                <div id="right" class="cell"><span id="b">B</span></div></div>"#);
            let tree = parse_html(&html);
            let layout = layout_dom(&tree, (800.0, 400.0));
            for (cell, span) in [("left", "a"), ("right", "b")] {
                let cell = layout.rects[&tree.get_element_by_id(cell).unwrap()];
                let span_id = tree.get_element_by_id(span).unwrap();
                let span = layout.rects[&span_id];
                assert!((cell.width - row_width / 2.0).abs() < 0.02, "{cell:?}");
                assert!((span.x - cell.x - expected_margin).abs() < 0.02,
                    "{margin} in {row_width}px flex row: cell={cell:?}, inline={span:?}");
                assert!((layout.styles[&span_id].margin.left - expected_margin).abs() < 0.02,
                    "paint and used style must agree on the final containing-block basis");
            }
        }
    }
}

#[test]
fn inline_svg_ratio_derived_auto_axis_survives_nonbinding_max_width() {
    let tree = parse_html(
        r#"
        <body style="margin:0">
          <div style="width:400px">
            <svg id="ratio" style="display:block;height:20px;max-width:200px" viewBox="0 0 2 1"></svg>
          </div>
        </body>
        "#,
    );
    let layout = layout_dom(&tree, (800.0, 400.0));
    let ratio = layout.rects[&tree.get_element_by_id("ratio").unwrap()];
    assert!(
        (ratio.width - 40.0).abs() < 0.01 && (ratio.height - 20.0).abs() < 0.01,
        "2:1 viewBox at height:20px must keep the ratio-derived 40px width; \
         max-width:200px is not binding and must not stretch to 200px: {ratio:?}"
    );
}

#[test]
fn percentage_margins_use_final_max_width_constrained_content_box() {
    // Old behavior: the top-down containing-block estimate ignored max-width,
    // so 25% + 8.333% widths/margins resolved against 1432px instead of the
    // row's real 1312px content box (offset 119.333 instead of 109.333).
    let tree = parse_html(
        r#"
        <body style="margin:0">
          <div id="container" style="width:100%;max-width:1320px;padding:16px;box-sizing:border-box">
            <div id="row" style="display:flex;margin:0 -12px">
              <div id="col1" style="width:25%;height:10px"></div>
              <div id="col2" style="width:16.66666667%;height:10px;margin-left:8.33333333%"></div>
              <div id="col3" style="width:16.66666667%;height:10px"></div>
              <div id="col4" style="width:16.66666667%;height:10px"></div>
              <div id="col5" style="width:16.66666667%;height:10px"></div>
            </div>
          </div>
        </body>
        "#,
    );
    let layout = layout_dom(&tree, (1440.0, 900.0));
    let rect = |id| layout.rects[&tree.get_element_by_id(id).unwrap()];
    let container = rect("container");
    let row = rect("row");
    let col1 = rect("col1");
    let col2 = rect("col2");
    assert!((container.width - 1320.0).abs() < 0.01, "max-width: {container:?}");
    assert!((row.width - 1312.0).abs() < 0.01, "negative margins: {row:?}");
    assert!((col1.width - 328.0).abs() < 0.01, "25% of the used row width: {col1:?}");
    let offset = col2.x - (col1.x + col1.width);
    assert!(
        (offset - 109.333_333).abs() < 0.51,
        "8.333% margin must use the final 1312px row content box, not the 1432px estimate \
         (physical offsets are device-pixel rounded): {offset}"
    );
    let col2_style = &layout.styles[&tree.get_element_by_id("col2").unwrap()];
    assert!(
        (col2_style.margin.left - 109.333_333).abs() < 0.02,
        "CSSOM/paint must see the used margin: {:?}",
        col2_style.margin
    );
    for id in ["col2", "col3", "col4", "col5"] {
        let column = rect(id);
        assert!(
            (column.y - row.y).abs() < 0.01,
            "all columns must share one flex row: row={row:?} {id}={column:?}"
        );
    }
    let last = rect("col5");
    assert!(
        (last.x + last.width - (row.x + row.width)).abs() < 0.05,
        "the percentage margins/widths must fill the row exactly: {last:?} row={row:?}"
    );
}

#[test]
fn percentage_margins_follow_nested_resized_containers() {
    let html = r#"
        <body style="margin:0">
          <div id="container" style="width:100%;max-width:600px;padding:10px;box-sizing:border-box">
            <div id="row" style="display:flex;margin:0 -10px">
              <div id="col" style="width:25%;height:10px"></div>
              <div id="offset" style="width:25%;height:10px;margin-left:5%"></div>
            </div>
          </div>
        </body>
        "#;
    for (viewport, container_width, row_width, column_width, margin) in [
        (1000.0, 600.0, 600.0, 150.0, 30.0),
        (500.0, 500.0, 500.0, 125.0, 25.0),
    ] {
        let tree = parse_html(html);
        let layout = layout_dom(&tree, (viewport, 600.0));
        let rect = |id| layout.rects[&tree.get_element_by_id(id).unwrap()];
        assert!((rect("container").width - container_width).abs() < 0.01);
        assert!((rect("row").width - row_width).abs() < 0.01);
        assert!((rect("col").width - column_width).abs() < 0.01);
        let offset_style = &layout.styles[&tree.get_element_by_id("offset").unwrap()];
        assert!(
            (offset_style.margin.left - margin).abs() < 0.01,
            "5% margin must track the resized row {row_width}: {:?}",
            offset_style.margin
        );
        let offset_column = rect("offset");
        assert!(
            (offset_column.x - (rect("col").x + rect("col").width) - margin).abs() < 0.01,
            "used margin must move the flex item: {offset_column:?}"
        );
    }
}

#[test]
fn percentage_margins_use_the_final_grid_area_width() {
    let tree = parse_html(
        r#"
        <body style="margin:0">
          <div id="grid" style="width:100%;max-width:400px;padding:10px;box-sizing:border-box;display:grid;grid-template-columns:1fr 1fr">
            <div id="cell" style="height:10px;margin-left:10%"></div>
            <div style="height:10px"></div>
          </div>
        </body>
        "#,
    );
    let layout = layout_dom(&tree, (800.0, 600.0));
    let grid = layout.rects[&tree.get_element_by_id("grid").unwrap()];
    let cell = layout.rects[&tree.get_element_by_id("cell").unwrap()];
    let style = &layout.styles[&tree.get_element_by_id("cell").unwrap()];
    // 400 border-box - 20 padding = 380 content / 2 tracks = 190 grid area.
    assert!(
        (style.margin.left - 19.0).abs() < 0.01,
        "grid item margin must use its 190px grid area, not the 380px content box: {:?}",
        style.margin
    );
    assert!(
        (cell.x - (grid.x + 10.0 + 19.0)).abs() < 0.01,
        "grid item inset must use the area-resolved margin: {cell:?} grid={grid:?}"
    );
}

#[test]
fn percentage_margins_preserve_negative_auto_and_block_axis_signs() {
    let tree = parse_html(
        r#"
        <body style="margin:0">
          <div id="row" style="width:400px">
            <div id="negative" style="width:100px;height:10px;margin-left:-10%"></div>
            <div id="auto" style="width:100px;height:10px;margin-left:auto;margin-right:auto"></div>
            <div id="block-axis" style="width:100px;height:10px;margin-top:10%"></div>
          </div>
        </body>
        "#,
    );
    let layout = layout_dom(&tree, (800.0, 600.0));
    let row = layout.rects[&tree.get_element_by_id("row").unwrap()];
    let negative = layout.rects[&tree.get_element_by_id("negative").unwrap()];
    let negative_style = &layout.styles[&tree.get_element_by_id("negative").unwrap()];
    assert!(
        (negative_style.margin.left + 40.0).abs() < 0.01 && negative_style.margin.left < 0.0,
        "negative percentage margin must stay negative: {:?}",
        negative_style.margin
    );
    assert!((negative.x - (row.x - 40.0)).abs() < 0.01, "{negative:?}");
    let auto_style = &layout.styles[&tree.get_element_by_id("auto").unwrap()];
    assert!(auto_style.margin_auto[1] && auto_style.margin_auto[3]);
    assert!(
        (auto_style.margin.left - 0.0).abs() < 0.01,
        "auto margins must not be overwritten with a used percentage: {:?}",
        auto_style.margin
    );
    let auto = layout.rects[&tree.get_element_by_id("auto").unwrap()];
    assert!((auto.x - row.x - 150.0).abs() < 0.01, "auto centering: {auto:?}");
    let block_style = &layout.styles[&tree.get_element_by_id("block-axis").unwrap()];
    assert!(
        (block_style.margin.top - 40.0).abs() < 0.01,
        "percentage block-axis margins resolve against width: {:?}",
        block_style.margin
    );
}

#[test]
fn percentage_margins_resolve_for_generated_and_positioned_pseudos() {
    let tree = parse_html(
        r#"
        <body style="margin:0">
          <div id="host" style="width:100%;max-width:1320px;padding:16px;box-sizing:border-box"></div>
          <div id="positioned" style="position:relative;width:200px;height:40px;border:10px solid"></div>
          <style>
            #host::before { content:""; display:block; height:10px; margin-left:10% }
            #positioned::before { content:""; position:absolute; width:20px; height:10px; margin-left:10% }
          </style>
        </body>
        "#,
    );
    let layout = layout_dom(&tree, (1440.0, 600.0));
    let host = tree.get_element_by_id("host").unwrap();
    let host_rect = layout.rects[&host];
    let before = layout.styles[&host].before_pseudo.as_ref().unwrap();
    assert!((host_rect.width - 1320.0).abs() < 0.01);
    assert!(
        (before.margin.left - 128.8).abs() < 0.01,
        "in-flow pseudo margin must use the final 1288px host content box: {:?}",
        before.margin
    );
    assert!(
        (host_rect.height - 42.0).abs() < 0.01,
        "block pseudo participates in the host content box (10px + 2*16px padding): {host_rect:?}"
    );

    let positioned = tree.get_element_by_id("positioned").unwrap();
    let absolute = layout.styles[&positioned].before_pseudo.as_ref().unwrap();
    assert!(
        (absolute.margin.left - 20.0).abs() < 0.01,
        "positioned pseudo margin must use its host padding box: {:?}",
        absolute.margin
    );
}

#[test]
fn percentage_margins_in_calc_min_max_use_the_final_basis() {
    let tree = parse_html(
        r#"
        <body style="margin:0">
          <div id="container" style="width:100%;max-width:400px">
            <div id="pure" style="width:50px;height:10px;margin-left:calc(8.33333333%)"></div>
            <div id="mixed" style="width:50px;height:10px;margin-left:calc(50% - 20px)"></div>
            <div id="negative" style="width:50px;height:10px;margin-left:calc(10% - 100px)"></div>
            <div id="minimum" style="width:50px;height:10px;margin-left:min(10%, 100px)"></div>
            <div id="maximum" style="width:50px;height:10px;margin-left:max(5%, 30px)"></div>
          </div>
        </body>
        "#,
    );
    let layout = layout_dom(&tree, (800.0, 600.0));
    let rect = |id| layout.rects[&tree.get_element_by_id(id).unwrap()];
    for (id, expected) in [
        ("pure", 33.333_333),
        ("mixed", 180.0),
        ("negative", -60.0),
        ("minimum", 40.0),
        ("maximum", 30.0),
    ] {
        let style = &layout.styles[&tree.get_element_by_id(id).unwrap()];
        assert!(
            (style.margin.left - expected).abs() < 0.02,
            "{id}: calc/min/max margin must resolve against the final 400px basis: {:?}",
            style.margin
        );
        assert!(
            (rect(id).x - expected).abs() < 0.51,
            "{id}: used fold position must match the late basis \
             (physical x is device-pixel rounded): {:?}",
            rect(id)
        );
    }
}

#[test]
fn final_flex_main_size_drives_auto_cross_size_through_aspect_ratio() {
    let tree = parse_html(include_str!("../../../render-repros/flex-post-ratio-cross-size.html"));
    let layout = layout_dom(&tree, (1080.0, 1000.0));
    let rect = |id| layout.rects[&tree.get_element_by_id(id).unwrap()];

    let row = rect("row");
    let media = rect("media");
    let visual = rect("visual");
    let copy = rect("copy");
    let after = rect("after");

    assert!((media.width - 520.0).abs() < 0.01, "flexed media width: {media:?}");
    assert!(
        (media.height - 520.0 / 1.72).abs() < 0.51,
        "auto cross-size must use the final 520px main size: {media:?}"
    );
    assert!(
        (visual.width - 520.0).abs() < 0.01 && (visual.height - 520.0 / 1.75571).abs() < 0.51,
        "absolute visual must use the ratio-sized containing block: {visual:?}"
    );
    assert!(
        (copy.x - 560.0).abs() < 0.01 && (copy.width - 520.0).abs() < 0.01,
        "the sibling must receive the other flexible share: {copy:?}"
    );
    assert!(
        (row.height - media.height).abs() < 0.01 && (after.y - (row.height + 150.0)).abs() < 0.01,
        "the ratio-sized item must contribute to its line and following flow: row={row:?} after={after:?}"
    );
}

#[test]
fn post_flex_aspect_transfer_respects_content_and_border_boxes() {
    let tree = parse_html(
        r#"
        <style>
          html,body{margin:0}
          #row{display:flex;align-items:flex-start;width:400px}
          .item{flex:1 0 0;width:1px;height:auto;aspect-ratio:2;padding:10px;border:5px solid}
          #content{box-sizing:content-box}
          #border{box-sizing:border-box}
          .single{display:flex;align-items:flex-start;width:200px}
          .plain{flex:1 0 0;width:1px;aspect-ratio:2}
          #minimum{height:auto;min-width:0;min-height:130px}
          #maximum{height:auto;max-width:1000px;max-height:70px}
          #explicit{height:80px}
        </style>
        <div id="row"><div id="content" class="item"></div><div id="border" class="item"></div></div>
        <div class="single"><div id="minimum" class="plain"></div></div>
        <div class="single"><div id="maximum" class="plain"></div></div>
        <div class="single"><div id="explicit" class="plain"></div></div>
        "#,
    );
    let layout = layout_dom(&tree, (500.0, 300.0));
    let rect = |id| layout.rects[&tree.get_element_by_id(id).unwrap()];
    let content = rect("content");
    let border = rect("border");
    let row = rect("row");
    let minimum = rect("minimum");
    let maximum = rect("maximum");
    let explicit = rect("explicit");

    assert!(
        (content.width - 200.0).abs() < 0.01 && (content.height - 115.0).abs() < 0.01,
        "content-box ratio must exclude then restore 30px of padding and border: {content:?}"
    );
    assert!(
        (border.width - 200.0).abs() < 0.01 && (border.height - 100.0).abs() < 0.01,
        "authored border-box ratio must apply to the outer box: {border:?}"
    );
    assert!((row.height - 115.0).abs() < 0.01, "the tallest transferred cross-size sets the line: {row:?}");
    assert!(
        (minimum.width - 200.0).abs() < 0.01 && (minimum.height - 130.0).abs() < 0.01,
        "the transferred cross-size must obey min-height: {minimum:?}"
    );
    assert!(
        (maximum.width - 200.0).abs() < 0.01 && (maximum.height - 70.0).abs() < 0.01,
        "the transferred cross-size must obey max-height: {maximum:?}"
    );
    assert!(
        (explicit.width - 200.0).abs() < 0.01 && (explicit.height - 80.0).abs() < 0.01,
        "an explicit cross-size must not be replaced by the preferred ratio: {explicit:?}"
    );
}

#[cfg(feature = "paint")]
#[test]
fn scrolled_ratio_sized_flex_clip_keeps_absolute_visual_visible() {
    let tree = parse_html(
        r#"
        <style>
          html,body{margin:0;background:#030712}
          #spacer{height:8300px}
          #row{display:flex;align-items:flex-start;gap:40px;width:1080px;overflow:hidden}
          #media{flex:1 0 0;width:1px;height:auto;aspect-ratio:1.72;position:relative;overflow:hidden;background:#7048e8}
          #visual{position:absolute;top:0;left:0;width:100%;height:auto;aspect-ratio:1.75571;background:#2f9e44}
          #copy{flex:1 0 0;width:1px;height:150px;background:#1971c2}
          #tail{height:500px}
        </style>
        <div id="spacer"></div><div id="row"><div id="media"><div id="visual"></div></div><div id="copy"></div></div><div id="tail"></div>
        "#,
    );
    let pixmap = obscura_render::paint_dom_scrolled(&tree, (1080.0, 400.0), None, (0.0, 8300.0))
        .expect("scrolled ratio fixture paints");
    let visual = pixmap.pixel(260, 100).expect("visual center pixel");
    let clip_tail = pixmap.pixel(260, 299).expect("clip tail pixel");

    assert!(
        visual.red() == 47 && visual.green() == 158 && visual.blue() == 68,
        "the absolute visual must survive its overflow clip: {visual:?}"
    );
    assert!(
        clip_tail.red() == 112 && clip_tail.green() == 72 && clip_tail.blue() == 232,
        "the ratio-sized clip owner must extend below the visual: {clip_tail:?}"
    );
}

#[test]
fn mixed_percentage_padding_shorthand_preserves_fixed_sides_and_content_origin() {
    let tree = parse_html(
        r#"
        <body style="margin:0">
          <div style="width:500px">
            <div id="mixed" style="box-sizing:border-box;width:100%;padding:10% 20px 5% 7px">
              <div id="ink" style="height:10px"></div>
            </div>
          </div>
        </body>
        "#,
    );
    let layout = layout_dom(&tree, (1000.0, 1000.0));
    let mixed_id = tree.get_element_by_id("mixed").unwrap();
    let mixed = layout.rects[&mixed_id];
    let ink = layout.rects[&tree.get_element_by_id("ink").unwrap()];
    let padding = layout.styles[&mixed_id].padding;

    assert!((mixed.width - 500.0).abs() < 0.01, "border-box width: {mixed:?}");
    assert!((mixed.height - 85.0).abs() < 0.01, "mixed padding height: {mixed:?}");
    assert!(
        (padding.top - 50.0).abs() < 0.01
            && (padding.right - 20.0).abs() < 0.01
            && (padding.bottom - 25.0).abs() < 0.01
            && (padding.left - 7.0).abs() < 0.01,
        "resolved mixed shorthand: {padding:?}"
    );
    assert!(
        (ink.x - mixed.x - 7.0).abs() < 0.01 && (ink.y - mixed.y - 50.0).abs() < 0.01,
        "content origin must use the resolved padding: mixed={mixed:?} ink={ink:?}"
    );
}

#[test]
fn positioned_pseudo_percentage_padding_uses_final_host_width() {
    let tree = parse_html(
        r#"
        <style>
          #host { position:relative; width:400px; height:100px; border:10px solid }
          #host::before {
            content:"x";
            position:absolute;
            width:100px;
            height:50px;
            padding:10% 5%;
          }
        </style>
        <div id="host"></div>
        "#,
    );
    let layout = layout_dom(&tree, (1000.0, 1000.0));
    let host = tree.get_element_by_id("host").unwrap();
    let pseudo = layout.styles[&host].before_pseudo.as_ref().unwrap();

    assert!(
        (pseudo.padding.top - 40.0).abs() < 0.01
            && (pseudo.padding.right - 20.0).abs() < 0.01
            && (pseudo.padding.bottom - 40.0).abs() < 0.01
            && (pseudo.padding.left - 20.0).abs() < 0.01,
        "positioned pseudo padding must use its final containing block: {:?}",
        pseudo.padding
    );
}

#[test]
fn inset_absolute_uses_nearest_positioned_ancestor() {
    let html = r##"
        <body style="margin:0">
          <div id="cb" style="position:relative;margin:40px 0 0 60px;width:400px;height:240px;padding:20px;border:10px solid black;box-sizing:border-box">
            <div style="position:static;margin:50px 0 0 70px;width:120px;height:80px">
              <span id="abs" style="position:absolute;left:15px;top:25px;width:80px;height:60px"></span>
              <div id="abs-end" style="position:absolute;right:30px;bottom:20px;width:70px;height:50px"></div>
              <div id="abs-percent" style="position:absolute;left:50%;top:50%;width:30px;height:30px"></div>
              <div id="fixed" style="position:fixed;left:520px;top:40px;width:90px;height:55px"></div>
            </div>
          </div>
        </body>
    "##;
    let tree = parse_html(html);
    let layout = layout_dom(&tree, (900.0, 700.0));
    let cb = layout.rects[&tree.get_element_by_id("cb").expect("containing block")];
    let abs = layout.rects[&tree.get_element_by_id("abs").expect("absolute child")];
    let abs_end = layout.rects[&tree.get_element_by_id("abs-end").expect("end-inset child")];
    let abs_percent = layout.rects[&tree.get_element_by_id("abs-percent").expect("percent-inset child")];
    let fixed = layout.rects[&tree.get_element_by_id("fixed").expect("fixed child")];

    // Absolute insets are measured from the positioned ancestor's padding
    // edge, not from the intervening static wrapper.
    assert!((abs.x - cb.x - 25.0).abs() < 1.0, "wrong absolute x: cb={cb:?} abs={abs:?}");
    assert!((abs.y - cb.y - 35.0).abs() < 1.0, "wrong absolute y: cb={cb:?} abs={abs:?}");
    assert_eq!(
        layout.styles[&tree.get_element_by_id("abs").unwrap()].display,
        obscura_render::Display::Block,
        "positioned inline should be blockified"
    );
    assert!((abs_end.x - 350.0).abs() < 1.0, "wrong right-inset x: {abs_end:?}");
    assert!((abs_end.y - 200.0).abs() < 1.0, "wrong bottom-inset y: {abs_end:?}");
    assert!((abs_percent.x - 260.0).abs() < 1.0, "wrong percent-inset x: {abs_percent:?}");
    assert!((abs_percent.y - 160.0).abs() < 1.0, "wrong percent-inset y: {abs_percent:?}");
    assert!((fixed.x - 520.0).abs() < 1.0, "fixed box did not use viewport x: {fixed:?}");
    assert!((fixed.y - 40.0).abs() < 1.0, "fixed box did not use viewport y: {fixed:?}");
}

#[test]
fn inset_absolute_fills_positioned_inline_flex_ancestor() {
    let tree = parse_html(
        r#"<body style="margin:0">
          <div style="display:flex">
            <div id="button" style="position:relative;display:inline-flex;width:115px;height:40px;padding:0 12px;box-sizing:border-box">
              <a id="overlay" style="position:absolute;inset:unset;top:50%;left:50%;width:max(48px,100%);height:max(48px,100%);transform:translate(-50%,-50%)"></a>
            </div>
          </div>
        </body>"#,
    );
    let layout = layout_dom(&tree, (1120.0, 780.0));
    let button = layout.rects[&tree.get_element_by_id("button").unwrap()];
    let overlay = layout.rects[&tree.get_element_by_id("overlay").unwrap()];

    assert_eq!((button.width, button.height), (115.0, 40.0));
    assert_eq!(overlay.width, 91.0);
    assert_eq!(overlay.height, 48.0, "functional percentage height must use the 40px containing block");
}

#[test]
fn absolute_auto_axes_preserve_static_position_after_reparenting() {
    let tree = parse_html(include_str!("../../../render-repros/absolute-static-position.html"));
    let layout = layout_dom(&tree, (900.0, 1000.0));
    let rect = |id| layout.rects[&tree.get_element_by_id(id).unwrap()];
    let static_inline = rect("top-static-inline");
    let static_block = rect("left-static-block");
    let outer_static = rect("outer-static");
    let nested_static = rect("nested-static");
    assert!(
        (static_inline.x - 125.0).abs() < 0.01
            && (static_inline.y - 60.0).abs() < 0.01,
        "static inline axis: {static_inline:?}"
    );
    assert!(
        (static_block.x - 210.0).abs() < 0.01
            && (static_block.y - 97.0).abs() < 0.01,
        "static block axis: {static_block:?}"
    );
    assert!(
        (outer_static.x - 84.0).abs() < 0.01 && (outer_static.y - 390.0).abs() < 0.01,
        "outer static candidate: {outer_static:?}"
    );
    assert!(
        (nested_static.x - 154.0).abs() < 0.01
            && (nested_static.y - 413.0).abs() < 0.01,
        "nested static candidate: {nested_static:?}"
    );
}

#[test]
fn legacy_center_keeps_block_flow_and_centers_descendants() {
    let tree = parse_html(include_str!("../../../render-repros/legacy-center.html"));
    let layout = layout_dom(&tree, (900.0, 1000.0));
    let rect = |selector| {
        let id = tree.query_selector_all(selector).unwrap()[0];
        layout.rects[&id]
    };
    let inline = rect(".inline-box");
    let block = rect(".block-box");
    let nested = rect(".nested-inline");
    let auto = rect(".auto-block");
    let table = rect(".center-table");
    let table_inline = rect(".table-inline");
    let cell_center = rect(".cell-center");
    let overridden = rect(".override-box");
    let pure = rect("#pure-center");

    assert!(
        (inline.x - 150.0).abs() < 0.01 && (inline.y - 0.0).abs() < 0.01,
        "centered inline content: {inline:?}"
    );
    assert!(
        (block.x - 150.0).abs() < 0.01 && (block.y - 20.0).abs() < 0.01,
        "centered block descendant: {block:?}"
    );
    assert!(
        (nested.x - 170.0).abs() < 0.01 && (nested.y - 40.0).abs() < 0.01,
        "inherited nested alignment: {nested:?}"
    );
    assert!(
        (auto.x - 0.0).abs() < 0.01
            && (auto.y - 60.0).abs() < 0.01
            && (auto.width - 400.0).abs() < 0.01,
        "auto-width block remains fill-available: {auto:?}"
    );
    assert!(
        (table.x - 100.0).abs() < 0.01
            && (table.y - 80.0).abs() < 0.01
            && (table.width - 200.0).abs() < 0.01,
        "table outer box is centered: {table:?}"
    );
    assert!(
        (table_inline.x - 100.0).abs() < 0.01
            && (table_inline.y - 80.0).abs() < 0.01,
        "table contents reset legacy alignment: {table_inline:?}"
    );
    assert!(
        (cell_center.x - 100.0).abs() < 0.01
            && (cell_center.y - 100.0).abs() < 0.01
            && (cell_center.width - 200.0).abs() < 0.01,
        "center in table cell fills the cell: {cell_center:?}"
    );
    assert!(
        (overridden.x - 0.0).abs() < 0.01 && (overridden.y - 120.0).abs() < 0.01,
        "author text-align override: {overridden:?}"
    );
    assert!(
        (pure.x - 0.0).abs() < 0.01
            && (pure.y - 240.0).abs() < 0.01
            && (pure.width - 400.0).abs() < 0.01,
        "pure-text center remains fill-available: {pure:?}"
    );
}

#[test]
fn list_indentation_is_reset_from_the_container() {
    let tree = parse_html(include_str!("../../../render-repros/list-indentation.html"));
    let layout = layout_dom(&tree, (900.0, 1000.0));
    let rect = |selector| {
        let id = tree.query_selector_all(selector).unwrap()[0];
        layout.rects[&id]
    };
    let default_item = rect("#default li");
    let reset_item = rect("#reset li");
    let ordered_item = rect("#ordered-reset li");
    let ex_box = rect("#ex-box");

    assert!(
        (default_item.x - 40.0).abs() < 0.01
            && (default_item.width - 360.0).abs() < 0.01,
        "default list indentation: {default_item:?}"
    );
    assert!(
        (reset_item.x - 0.0).abs() < 0.01
            && (reset_item.y - 40.0).abs() < 0.01
            && (reset_item.width - 400.0).abs() < 0.01,
        "reset unordered list: {reset_item:?}"
    );
    assert!(
        (ordered_item.x - 0.0).abs() < 0.01
            && (ordered_item.y - 80.0).abs() < 0.01
            && (ordered_item.width - 400.0).abs() < 0.01,
        "reset ordered list: {ordered_item:?}"
    );
    assert!(
        (ex_box.x - 0.0).abs() < 0.01
            && (ex_box.y - 120.0).abs() < 0.01
            && (ex_box.width - 44.0).abs() < 0.01
            && (ex_box.height - 22.0).abs() < 0.01,
        "ex padding: {ex_box:?}"
    );
}

#[test]
fn replaced_max_height_clamps_intrinsic_aspect_transfer() {
    let tree = parse_html(include_str!(
        "../../../render-repros/max-height-replaced.html"
    ));
    let image_id = tree.get_element_by_id("image").unwrap();
    let intrinsic = HashMap::from([(image_id, (648.0, 440.0))]);
    let layout = layout_dom_with_images(&tree, (900.0, 1000.0), &intrinsic);
    let card = layout.rects[&tree.get_element_by_id("card").unwrap()];
    let image = layout.rects[&image_id];
    assert!(
        (card.x - 0.0).abs() < 0.01
            && (card.y - 0.0).abs() < 0.01
            && (card.width - 394.0).abs() < 0.01
            && (card.height - 608.0).abs() < 0.01,
        "card: {card:?}"
    );
    assert!(
        (image.x - 32.0).abs() < 0.01
            && (image.y - 64.0).abs() < 0.01
            && (image.width - 330.0).abs() < 0.01
            && (image.height - 200.0).abs() < 0.01,
        "image: {image:?}"
    );
}

#[test]
fn transforms_establish_absolute_and_fixed_containing_blocks() {
    let html = r##"
        <body style="margin:0">
          <div id="outer" style="position:relative;margin:40px 0 0 50px;width:600px;height:400px;border:10px solid black;padding:20px;box-sizing:border-box">
            <div id="transformer" style="transform:translate(30px,20px);margin:40px 0 0 70px;width:300px;height:200px;border:5px solid black;padding:10px;box-sizing:border-box">
              <div style="position:static;margin:20px;width:80px;height:50px">
                <span id="abs" style="position:absolute;left:20px;top:25px;width:70px;height:55px"></span>
                <div id="fixed-transformed" style="position:fixed;left:150px;top:100px;width:80px;height:50px"></div>
              </div>
            </div>
            <div id="identity-transform" style="transform:rotate(0deg);margin:-80px 0 0 400px;width:100px;height:80px">
              <div id="identity-abs" style="position:absolute;right:10px;bottom:10px;width:20px;height:20px"></div>
            </div>
            <div id="fixed-viewport" style="position:fixed;left:700px;top:40px;width:90px;height:60px"></div>
          </div>
        </body>
    "##;
    let tree = parse_html(html);
    let layout = layout_dom(&tree, (900.0, 700.0));
    let transformer_id = tree.get_element_by_id("transformer").unwrap();
    let transformer = layout.rects[&transformer_id];
    let abs = layout.rects[&tree.get_element_by_id("abs").unwrap()];
    let fixed_transformed = layout.rects[&tree.get_element_by_id("fixed-transformed").unwrap()];
    let identity_transform = layout.rects[&tree.get_element_by_id("identity-transform").unwrap()];
    let identity_abs = layout.rects[&tree.get_element_by_id("identity-abs").unwrap()];
    let fixed_viewport = layout.rects[&tree.get_element_by_id("fixed-viewport").unwrap()];

    // Layout rects precede paint transforms. Insets use the transformer's
    // padding box; the shared translate is recorded separately for the whole
    // DOM subtree.
    assert!((abs.x - transformer.x - 25.0).abs() < 1.0, "wrong transformed abs x: transformer={transformer:?} abs={abs:?}");
    assert!((abs.y - transformer.y - 30.0).abs() < 1.0, "wrong transformed abs y: transformer={transformer:?} abs={abs:?}");
    assert!((fixed_transformed.x - transformer.x - 155.0).abs() < 1.0, "wrong transformed fixed x: {fixed_transformed:?}");
    assert!((fixed_transformed.y - transformer.y - 105.0).abs() < 1.0, "wrong transformed fixed y: {fixed_transformed:?}");
    assert_eq!(layout.translates[&transformer_id], (30.0, 20.0));
    assert!((identity_abs.x - identity_transform.x - 70.0).abs() < 1.0, "unsupported transform did not establish abs x: {identity_abs:?}");
    assert!((identity_abs.y - identity_transform.y - 50.0).abs() < 1.0, "unsupported transform did not establish abs y: {identity_abs:?}");
    assert!((fixed_viewport.x - 700.0).abs() < 1.0, "positioned ancestor captured fixed x: {fixed_viewport:?}");
    assert!((fixed_viewport.y - 40.0).abs() < 1.0, "positioned ancestor captured fixed y: {fixed_viewport:?}");
}

#[test]
fn modern_effects_establish_containing_blocks_independently() {
    let tree = parse_html(include_str!("../../../render-repros/modern-containing-block-triggers.html"));
    let layout = layout_dom(&tree, (900.0, 700.0));
    let cases = [
        ("filter-cb", "filter-badge"),
        ("perspective-cb", "perspective-badge"),
        ("contain-cb", "contain-badge"),
        ("will-change-cb", "will-change-badge"),
        ("visibility-cb", "visibility-badge"),
    ];
    for (cb_id, badge_id) in cases {
        let cb = layout.rects[&tree.get_element_by_id(cb_id).unwrap()];
        let badge = layout.rects[&tree.get_element_by_id(badge_id).unwrap()];
        assert!((badge.x - cb.x - 75.0).abs() < 1.0, "{badge_id} wrong x: cb={cb:?} badge={badge:?}");
        assert!((badge.y - cb.y - 55.0).abs() < 1.0, "{badge_id} wrong y: cb={cb:?} badge={badge:?}");
    }

    let filter_cb = layout.rects[&tree.get_element_by_id("filter-cb").unwrap()];
    let filter_fixed = layout.rects[&tree.get_element_by_id("filter-fixed").unwrap()];
    assert!((filter_fixed.x - filter_cb.x - 5.0).abs() < 1.0);
    assert!((filter_fixed.y - filter_cb.y - 5.0).abs() < 1.0);
    let contain_cb = layout.rects[&tree.get_element_by_id("contain-cb").unwrap()];
    let contain_fixed = layout.rects[&tree.get_element_by_id("contain-fixed").unwrap()];
    assert!((contain_fixed.x - contain_cb.x - 5.0).abs() < 1.0);
    assert!((contain_fixed.y - contain_cb.y - 5.0).abs() < 1.0);

    // Pinned Chromium 145 does not make container-type:inline-size a
    // positioning containing block. Keep it as a negative control.
    let container_badge = layout.rects[&tree.get_element_by_id("container-badge").unwrap()];
    assert!((container_badge.x - 775.0).abs() < 1.0, "container-type control x: {container_badge:?}");
    assert!((container_badge.y - 175.0).abs() < 1.0, "container-type control y: {container_badge:?}");
}

#[test]
fn long_text_run_wraps_across_multiple_lines() {
    // A long single text node with no inline elements breaking it up must
    // wrap within a narrow container instead of overflowing on one line. The
    // container's height is the mode-independent proof: several wrapped lines
    // make it much taller than one line, whether text is shaped by cosmic-text
    // (paint) or split into word boxes (layout-only).
    let html = r##"<div style="width:100px">This sentence has plenty of words to wrap across several lines</div>"##;
    let tree = parse_html(html);
    let layout = layout_dom(&tree, (1000.0, 1000.0));

    // Tightest element enclosing the text: the 100px div itself.
    let mut div_rect: Option<obscura_render::Rect> = None;
    for (id, rect) in &layout.rects {
        if tree.text_content(*id).contains("several lines") {
            if div_rect.as_ref().map(|d| rect.width * rect.height < d.width * d.height).unwrap_or(true) {
                div_rect = Some(*rect);
            }
        }
    }
    let div_rect = div_rect.expect("text container laid out");
    assert!(div_rect.width <= 101.0, "container should hold its 100px width, got {}", div_rect.width);
    assert!(
        div_rect.height > 60.0,
        "text should wrap onto several lines in a 100px-wide box (tall container), got height {}",
        div_rect.height
    );
}

#[test]
fn negative_flex_margin_overlays_at_container_start() {
    let html = r##"
        <html><head><style>
          html,body{margin:0}
          #document{display:flex;width:900px;height:220px}
          #main{width:100%;height:200px}
          #body{margin-left:225px;height:200px}
          #sidebar{display:flex;width:225px;height:180px;margin-left:-100%}
        </style></head><body>
          <div id="document">
            <div id="main"><div id="body"></div></div>
            <div id="sidebar"></div>
          </div>
        </body></html>
    "##;
    let tree = parse_html(html);
    let layout = layout_dom(&tree, (900.0, 1000.0));
    let document = layout.rects[&tree.get_element_by_id("document").unwrap()];
    let main = layout.rects[&tree.get_element_by_id("main").unwrap()];
    let body = layout.rects[&tree.get_element_by_id("body").unwrap()];
    let sidebar = layout.rects[&tree.get_element_by_id("sidebar").unwrap()];
    assert!((document.x - 0.0).abs() < 0.01, "document: {document:?}");
    assert!((main.x - 0.0).abs() < 0.01, "main: {main:?}");
    assert!((body.x - 225.0).abs() < 0.01, "body: {body:?}");
    assert!((sidebar.x - 0.0).abs() < 0.01, "sidebar: {sidebar:?}");
}

#[test]
fn auto_height_column_flex_counts_negative_main_axis_margins_once() {
    let html = r##"
        <html><head><style>
          html,body{margin:0}
          #column{display:flex;flex-direction:column}
          #first,#second,#third{position:relative;flex-shrink:0}
          #first{height:100px}
          #second{height:100px;margin-top:-15px}
          #third{height:666px;margin-top:20px}
          #contained{container-type:size;display:flex;flex-direction:column}
          #contained-child{height:100px;margin-top:-15px}
        </style></head><body>
          <div id="column">
            <div id="first"></div>
            <div id="second"></div>
            <div id="third"></div>
          </div>
          <div id="contained"><div id="contained-child"></div></div>
        </body></html>
    "##;
    let tree = parse_html(html);
    let layout = layout_dom(&tree, (900.0, 1200.0));
    let rect = |id| layout.rects[&tree.get_element_by_id(id).unwrap()];
    let column = rect("column");
    let first = rect("first");
    let second = rect("second");
    let third = rect("third");

    assert!(
        (column.height - 871.0).abs() < 0.01,
        "100 + (100 - 15) + (666 + 20) must size the auto-height column: {column:?}"
    );
    assert!((first.y - column.y).abs() < 0.01, "first: {first:?}");
    assert!(
        (second.y - (first.y + first.height - 15.0)).abs() < 0.01,
        "negative margin must overlap the preceding item: first={first:?} second={second:?}"
    );
    assert!(
        (third.y - (second.y + second.height + 20.0)).abs() < 0.01,
        "the following item must retain normal sequential placement: second={second:?} third={third:?}"
    );
    assert!(
        rect("contained").height.abs() < 0.01,
        "size containment must keep its intentional zero intrinsic block size"
    );
}

#[test]
fn percentage_flex_overlay_uses_auto_width_parent_content_box() {
    let html = r##"
        <html><head><style>
          html,body{margin:0}
          body{margin-left:1em;margin-right:1em}
          #document{display:flex}
          #main{float:left;width:100%;height:200px}
          #body{margin-left:min(25vw,350px);height:200px}
          #sidebar{
            display:flex;
            width:min(25vw,350px);
            height:180px;
            margin-left:-100%;
            float:none;
            position:sticky;
            top:0
          }
        </style></head><body>
          <div id="document">
            <div id="main"><div id="body"></div></div>
            <div id="sidebar"></div>
          </div>
        </body></html>
    "##;
    let tree = parse_html(html);
    let layout = layout_dom(&tree, (1280.0, 1400.0));
    let document = layout.rects[&tree.get_element_by_id("document").unwrap()];
    let main = layout.rects[&tree.get_element_by_id("main").unwrap()];
    let body = layout.rects[&tree.get_element_by_id("body").unwrap()];
    let sidebar = layout.rects[&tree.get_element_by_id("sidebar").unwrap()];
    assert!((document.x - 16.0).abs() < 0.01, "document: {document:?}");
    assert!((document.width - 1248.0).abs() < 0.01, "document: {document:?}");
    assert!((main.x - 16.0).abs() < 0.01, "main: {main:?}");
    assert!((body.x - 336.0).abs() < 0.01, "body: {body:?}");
    assert!((sidebar.x - 16.0).abs() < 0.01, "sidebar: {sidebar:?}");
    assert!((sidebar.width - 320.0).abs() < 0.01, "sidebar: {sidebar:?}");
}

#[test]
fn native_float_band_matches_bootstrap_clearfix_geometry() {
    let tree = parse_html(include_str!(
        "../../../render-repros/bootstrap-float-clearfix.html"
    ));
    let layout = layout_dom(&tree, (900.0, 1000.0));
    let rect = |selector| {
        let id = tree.query_selector(selector).unwrap().unwrap();
        layout.rects[&id]
    };
    let row = rect(".row");
    let column = rect(".column");
    let centered = rect(".centered");
    let card = rect(".card");
    assert!(
        (row.x - 0.0).abs() < 0.01
            && (row.y - 0.0).abs() < 0.01
            && (row.width - 900.0).abs() < 0.01
            && (row.height - 1000.0).abs() < 0.01,
        "row: {row:?}"
    );
    assert!(
        (column.x - 0.0).abs() < 0.01
            && (column.y - 0.0).abs() < 0.01
            && (column.width - 900.0).abs() < 0.01
            && (column.height - 1000.0).abs() < 0.01,
        "column: {column:?}"
    );
    assert!(
        (centered.x - 0.0).abs() < 0.01
            && (centered.y - 0.0).abs() < 0.01
            && (centered.width - 900.0).abs() < 0.01
            && (centered.height - 900.0).abs() < 0.01,
        "centered: {centered:?}"
    );
    assert!(
        (card.x - 250.0).abs() < 0.01
            && (card.y - 200.0).abs() < 0.01
            && (card.width - 400.0).abs() < 0.01
            && (card.height - 500.0).abs() < 0.01,
        "card: {card:?}"
    );
}

#[test]
fn native_percentage_float_uses_padded_containing_block_content_width() {
    let tree = parse_html(
        r#"<style>
            html,body{margin:0}
            #band{box-sizing:border-box;width:300px;padding:20px}
            #float{float:left;width:50%;height:40px}
            #clear{clear:left}
        </style>
        <div id="band"><div id="float"></div><div id="clear"></div></div>"#,
    );
    let layout = layout_dom(&tree, (600.0, 300.0));
    let band = layout.rects[&tree.get_element_by_id("band").unwrap()];
    let float = layout.rects[&tree.get_element_by_id("float").unwrap()];
    let clear = layout.rects[&tree.get_element_by_id("clear").unwrap()];
    assert!(
        (band.width - 300.0).abs() < 0.01 && (band.height - 80.0).abs() < 0.01,
        "band: {band:?}"
    );
    assert!(
        (float.x - 20.0).abs() < 0.01
            && (float.y - 20.0).abs() < 0.01
            && (float.width - 130.0).abs() < 0.01
            && (float.height - 40.0).abs() < 0.01,
        "float: {float:?}"
    );
    assert!(
        (clear.x - 20.0).abs() < 0.01 && (clear.y - 60.0).abs() < 0.01,
        "clear: {clear:?}"
    );
}

#[test]
fn native_float_clearance_is_side_sensitive() {
    let tree = parse_html(
        r#"<style>
            html,body{margin:0}
            .band{width:200px;overflow:hidden}
            .float{float:right;width:60px;height:50px}
            .clear{height:10px}
            #matching .clear{clear:right}
            #other-side .clear{clear:left}
        </style>
        <div id="matching" class="band"><div class="float"></div><div class="clear"></div></div>
        <div id="other-side" class="band"><div class="float"></div><div class="clear"></div></div>"#,
    );
    let layout = layout_dom(&tree, (600.0, 300.0));
    let matching = tree.get_element_by_id("matching").unwrap();
    let other = tree.get_element_by_id("other-side").unwrap();
    let matching_clear = tree
        .query_selector("#matching .clear")
        .unwrap()
        .unwrap();
    let other_clear = tree
        .query_selector("#other-side .clear")
        .unwrap()
        .unwrap();
    let matching_rect = layout.rects[&matching];
    let other_rect = layout.rects[&other];
    let matching_clear_rect = layout.rects[&matching_clear];
    let other_clear_rect = layout.rects[&other_clear];
    assert!(
        (matching_clear_rect.y - 50.0).abs() < 0.01
            && (matching_rect.height - 60.0).abs() < 0.01,
        "matching: {matching_rect:?} clear: {matching_clear_rect:?}"
    );
    assert!(
        (other_clear_rect.y - 60.0).abs() < 0.01
            && (other_rect.height - 50.0).abs() < 0.01,
        "other: {other_rect:?} clear: {other_clear_rect:?}"
    );
}

#[test]
fn nested_clearfix_uses_the_tallest_float_in_the_shared_bfc() {
    let tree = parse_html(
        r#"<style>
            *{box-sizing:border-box}
            html,body{margin:0}
            #container{height:100px;padding:0 15px}
            #container::before,#container::after,#header::before,#header::after,
            #collapse::before,#collapse::after{display:table;content:" "}
            #container::after,#header::after,#collapse::after{clear:both}
            #header{float:left;height:100px}
            #inner-float{float:left;width:208px;height:100px;margin-right:33px}
            #collapse{padding:0 15px}
            #nav{float:left;width:434px;height:50px;margin-top:23px}
            #right{float:right;width:80px;height:50px;margin-top:23px}
        </style>
        <div id="container"><div id="header"><div id="inner-float"></div></div><div id="collapse"><div id="nav"></div><div id="right"></div></div></div>"#,
    );
    let layout = layout_dom(&tree, (1280.0, 900.0));
    let rect = |id| layout.rects[&tree.get_element_by_id(id).unwrap()];
    let collapse = rect("collapse");
    let nav = rect("nav");
    let right = rect("right");

    assert!(
        (collapse.x - 15.0).abs() < 0.01
            && (collapse.y - 0.0).abs() < 0.01
            && (collapse.width - 1250.0).abs() < 0.01
            && (collapse.height - 100.0).abs() < 0.01,
        "collapse: {collapse:?}"
    );
    assert!(
        (nav.x - 256.0).abs() < 0.01
            && (nav.y - 23.0).abs() < 0.01
            && (nav.width - 434.0).abs() < 0.01
            && (nav.height - 50.0).abs() < 0.01,
        "nav: {nav:?}"
    );
    assert!(
        (right.x - 1170.0).abs() < 0.01
            && (right.y - 23.0).abs() < 0.01
            && (right.width - 80.0).abs() < 0.01
            && (right.height - 50.0).abs() < 0.01,
        "right: {right:?}"
    );
}

#[test]
fn nested_clearfix_does_not_cross_a_bfc_boundary() {
    let tree = parse_html(
        r#"<style>
            *{box-sizing:border-box}
            html,body{margin:0}
            #container{height:100px;padding:0 15px}
            #container::before,#container::after,#collapse::before,#collapse::after{display:table;content:" "}
            #container::after,#collapse::after{clear:both}
            #header{float:left;width:241px;height:100px}
            #collapse{overflow:hidden;padding:0 15px}
            #nav{float:left;width:434px;height:50px;margin-top:23px}
            #right{float:right;width:80px;height:50px;margin-top:23px}
        </style>
        <div id="container"><div id="header"></div><div id="collapse"><div id="nav"></div><div id="right"></div></div></div>"#,
    );
    let layout = layout_dom(&tree, (1280.0, 900.0));
    let collapse = layout.rects[&tree.get_element_by_id("collapse").unwrap()];

    assert!(
        (collapse.x - 256.0).abs() < 0.01
            && (collapse.y - 0.0).abs() < 0.01
            && (collapse.width - 1009.0).abs() < 0.01
            && (collapse.height - 73.0).abs() < 0.01,
        "collapse BFC: {collapse:?}"
    );
}

#[test]
fn float_flow_zone_preserves_blocks_inline_runs_and_clearance() {
    let tree = parse_html(include_str!("../../../render-repros/float-flow-bands.html"));
    let layout = layout_dom(&tree, (900.0, 1000.0));
    let rect = |id| layout.rects[&tree.get_element_by_id(id).unwrap()];
    let float = rect("float");
    let intro = rect("intro");
    let release = rect("release");
    let download = rect("download");
    let archive = rect("archive");
    let sponsors = rect("sponsors");
    let logos = rect("logos");
    let cleared = rect("cleared");
    assert!((float.x - 650.0).abs() < 0.01 && (float.height - 400.0).abs() < 0.01, "float: {float:?}");
    assert!((intro.x - 0.0).abs() < 0.01 && (intro.width - 650.0).abs() < 0.01, "intro: {intro:?}");
    assert!((release.y - 100.0).abs() < 0.01 && (release.width - 650.0).abs() < 0.01, "release: {release:?}");
    assert!((download.x - 0.0).abs() < 0.01 && (download.y - 150.0).abs() < 0.01, "download: {download:?}");
    assert!((archive.x - 60.0).abs() < 0.01 && (archive.y - 150.0).abs() < 0.01, "archive: {archive:?}");
    assert!((sponsors.y - 180.0).abs() < 0.01, "sponsors: {sponsors:?}");
    assert!((logos.y - 230.0).abs() < 0.01, "logos: {logos:?}");
    assert!((cleared.y - 400.0).abs() < 0.01 && (cleared.width - 900.0).abs() < 0.01, "cleared: {cleared:?}");
    assert!(
        !layout.rects.contains_key(&tree.get_element_by_id("mobile-duplicate").unwrap()),
        "display:none duplicate generated a box"
    );
}

#[test]
fn float_exclusion_continues_through_non_bfc_block_wrappers() {
    // Chromium keeps each ordinary block's outer box at the containing
    // block's full width. Only line boxes inside those blocks query the
    // float-reduced available band; backgrounds and borders extend beneath
    // the float. This is deliberately distinct from a new BFC, which avoids
    // the float as one box.
    let tree = parse_html(include_str!(
        "../../../render-repros/float-bfc-continuation.html"
    ));
    let layout = layout_dom(&tree, (900.0, 1000.0));
    let rect = |name| layout.rects[&tree.get_element_by_id(name).unwrap()];
    let float = rect("intro-float");
    let lead = rect("lead");
    let heading = rect("heading");
    let beside = rect("beside");
    let after = rect("after");
    assert!(
        (float.x - 650.0).abs() < 0.01
            && (float.y - 0.0).abs() < 0.01
            && (float.width - 250.0).abs() < 0.01
            && (float.height - 400.0).abs() < 0.01,
        "float: {float:?}"
    );
    assert!(
        (lead.x - 0.0).abs() < 0.01
            && (lead.y - 0.0).abs() < 0.01
            && (lead.width - 900.0).abs() < 0.01
            && (lead.height - 250.0).abs() < 0.01,
        "lead: {lead:?}"
    );
    assert!(
        (heading.x - 0.0).abs() < 0.01
            && (heading.y - 250.0).abs() < 0.01
            && (heading.width - 900.0).abs() < 0.01
            && (heading.height - 50.0).abs() < 0.01,
        "heading: {heading:?}"
    );
    assert!(
        (beside.x - 0.0).abs() < 0.01
            && (beside.y - 300.0).abs() < 0.01
            && (beside.width - 900.0).abs() < 0.01
            && (beside.height - 100.0).abs() < 0.01,
        "beside: {beside:?}"
    );
    assert!(
        (after.x - 0.0).abs() < 0.01
            && (after.y - 400.0).abs() < 0.01
            && (after.width - 900.0).abs() < 0.01
            && (after.height - 70.0).abs() < 0.01,
        "after: {after:?}"
    );
}

#[test]
fn full_width_percentage_float_uses_definite_containing_block() {
    let tree = parse_html(
        r#"
        <style>
          html, body { margin: 0; width: 100%; height: 100% }
          #host { width: 100%; height: 100% }
          #host::before, #host::after { content: " "; display: table }
          #host::after { clear: both }
          #float {
            float: left;
            display: flex;
            flex-direction: column;
            align-items: center;
            width: 100%;
            height: 100%;
          }
          #badges { display: flex; gap: 20px }
          .badge { width: 100px; height: 48px }
        </style>
        <div id="host">
          <div id="float">
            <div id="badges">
              <div class="badge"></div>
              <div class="badge"></div>
              <div class="badge"></div>
            </div>
          </div>
        </div>
        "#,
    );
    let layout = layout_dom(&tree, (800.0, 600.0));
    let rect = |id| layout.rects[&tree.get_element_by_id(id).unwrap()];
    let host = rect("host");
    let float = rect("float");
    let badges = rect("badges");
    assert!(
        (host.x - 0.0).abs() < 0.01
            && (host.y - 0.0).abs() < 0.01
            && (host.width - 800.0).abs() < 0.01
            && (host.height - 600.0).abs() < 0.01,
        "host: {host:?}"
    );
    assert!(
        (float.x - 0.0).abs() < 0.01
            && (float.y - 0.0).abs() < 0.01
            && (float.width - 800.0).abs() < 0.01
            && (float.height - 600.0).abs() < 0.01,
        "float: {float:?}"
    );
    assert!(
        (badges.x - 230.0).abs() < 0.01
            && (badges.y - 0.0).abs() < 0.01
            && (badges.width - 340.0).abs() < 0.01
            && (badges.height - 48.0).abs() < 0.01,
        "nested centered badges: {badges:?}"
    );
}

#[test]
fn clearfix_contains_a_full_width_definite_height_float() {
    let tree = parse_html(
        r#"
        <style>
          html, body { margin: 0 }
          #host { width: 100% }
          #host::before, #host::after { content: " "; display: table }
          #host::after { clear: both }
          #float { float: left; width: 100%; height: 600px }
        </style>
        <div id="host"><div id="float"></div></div>
        "#,
    );
    let layout = layout_dom(&tree, (800.0, 700.0));
    let rect = |id| layout.rects[&tree.get_element_by_id(id).unwrap()];
    let host = rect("host");
    let float = rect("float");
    assert!(
        (host.x - 0.0).abs() < 0.01
            && (host.y - 0.0).abs() < 0.01
            && (host.width - 800.0).abs() < 0.01
            && (host.height - 600.0).abs() < 0.01,
        "host: {host:?}"
    );
    assert!(
        (float.x - 0.0).abs() < 0.01
            && (float.y - 0.0).abs() < 0.01
            && (float.width - 800.0).abs() < 0.01
            && (float.height - 600.0).abs() < 0.01,
        "float: {float:?}"
    );
}

#[test]
fn auto_width_clearfix_keeps_a_content_sized_percentage_float_at_block_start() {
    let tree = parse_html(
        r#"
        <style>
          html, body { margin: 0 }
          #host { min-height: 600px }
          #host::before, #host::after { content: " "; display: table }
          #host::after { clear: both }
          #float {
            position: relative;
            display: flex;
            flex-direction: column;
            float: left;
            width: 100%;
            min-height: 1px;
          }
          #content {
            display: flex;
            align-items: center;
            justify-content: center;
            min-height: 600px;
          }
          #card { width: 400px; height: 300px }
        </style>
        <section id="host">
          <div id="float">
            <div id="content"><div id="card"></div></div>
          </div>
        </section>
        "#,
    );
    let layout = layout_dom(&tree, (800.0, 700.0));
    let rect = |id| layout.rects[&tree.get_element_by_id(id).unwrap()];
    let host = rect("host");
    let float = rect("float");
    let content = rect("content");
    let card = rect("card");
    assert!(
        (host.x - 0.0).abs() < 0.01
            && (host.y - 0.0).abs() < 0.01
            && (host.width - 800.0).abs() < 0.01
            && (host.height - 600.0).abs() < 0.01,
        "host: {host:?}"
    );
    assert!(
        (float.x - 0.0).abs() < 0.01
            && (float.y - 0.0).abs() < 0.01
            && (float.width - 800.0).abs() < 0.01
            && (float.height - 600.0).abs() < 0.01,
        "float: {float:?}"
    );
    assert!(
        (content.x - 0.0).abs() < 0.01
            && (content.y - 0.0).abs() < 0.01
            && (content.width - 800.0).abs() < 0.01
            && (content.height - 600.0).abs() < 0.01,
        "content: {content:?}"
    );
    assert!(
        (card.x - 200.0).abs() < 0.01
            && (card.y - 150.0).abs() < 0.01
            && (card.width - 400.0).abs() < 0.01
            && (card.height - 300.0).abs() < 0.01,
        "card: {card:?}"
    );
}

#[test]
fn min_height_alone_does_not_resolve_a_floats_percentage_height() {
    let tree = parse_html(
        r#"
        <style>
          html, body { margin: 0 }
          #host { min-height: 600px }
          #host::before, #host::after { content: " "; display: table }
          #host::after { clear: both }
          #float { float: left; width: 100%; height: 100% }
        </style>
        <div id="host"><div id="float"></div></div>
        "#,
    );
    let layout = layout_dom(&tree, (800.0, 700.0));
    let host = layout.rects[&tree.get_element_by_id("host").unwrap()];
    let float = layout.rects[&tree.get_element_by_id("float").unwrap()];
    assert!(
        (host.width - 800.0).abs() < 0.01 && (host.height - 600.0).abs() < 0.01,
        "host: {host:?}"
    );
    assert!(
        (float.x - 0.0).abs() < 0.01
            && (float.y - 0.0).abs() < 0.01
            && (float.width - 800.0).abs() < 0.01
            && float.height.abs() < 0.01,
        "an indefinite percentage height must behave as auto: {float:?}"
    );
}

#[test]
fn min_height_floor_keeps_a_full_width_float_at_block_start() {
    let tree = parse_html(
        r#"
        <style>
          html, body { margin: 0 }
          #host { width: 100%; min-height: 600px }
          #float { float: left; width: 100%; height: 600px }
          #following { height: 20px }
        </style>
        <div id="host"><div id="float"></div></div>
        <div id="following"></div>
        "#,
    );
    let layout = layout_dom(&tree, (800.0, 700.0));
    let rect = |id| layout.rects[&tree.get_element_by_id(id).unwrap()];
    let host = rect("host");
    let float = rect("float");
    let following = rect("following");
    assert!(
        (host.width - 800.0).abs() < 0.01 && (host.height - 600.0).abs() < 0.01,
        "host: {host:?}"
    );
    assert!(
        (float.x - 0.0).abs() < 0.01
            && (float.y - 0.0).abs() < 0.01
            && (float.width - 800.0).abs() < 0.01
            && (float.height - 600.0).abs() < 0.01,
        "float: {float:?}"
    );
    assert!((following.y - 600.0).abs() < 0.01, "following: {following:?}");
}

#[test]
fn percentage_float_and_inline_sibling_share_definite_height_band() {
    let tree = parse_html(
        r#"
        <style>
          html, body { margin: 0 }
          #host {
            box-sizing: border-box;
            width: 400px;
            height: 46px;
            border: 1px solid;
          }
          #float, #flow {
            box-sizing: border-box;
            display: inline-block;
            width: 50%;
            height: 100%;
          }
          #float { float: left }
        </style>
        <div id="host"><button id="float"></button><button id="flow"></button></div>
        "#,
    );
    let layout = layout_dom(&tree, (800.0, 200.0));
    let rect = |id| layout.rects[&tree.get_element_by_id(id).unwrap()];
    let float = rect("float");
    let flow = rect("flow");
    assert!(
        (float.x - 1.0).abs() < 0.01
            && (float.y - 1.0).abs() < 0.01
            && (float.width - 199.0).abs() < 0.01
            && (float.height - 44.0).abs() < 0.01,
        "float: {float:?}"
    );
    assert!(
        (flow.x - 200.0).abs() < 0.01
            && (flow.y - 1.0).abs() < 0.01
            && (flow.width - 199.0).abs() < 0.01
            && (flow.height - 44.0).abs() < 0.01,
        "flow: {flow:?}"
    );
}

#[test]
fn replaced_image_contributes_intrinsic_size_in_ordered_grid() {
    let tree = parse_html(include_str!("../../../render-repros/replaced-grid-order.html"));
    let image_id = tree.get_element_by_id("image").unwrap();
    let intrinsic = HashMap::from([(image_id, (1.0, 1.0))]);
    let layout = layout_dom_with_images(&tree, (900.0, 1000.0), &intrinsic);
    let rect = |id| layout.rects[&tree.get_element_by_id(id).unwrap()];
    let description = rect("description");
    let media = rect("media");
    let image = rect("image");
    assert!(
        (description.x - 0.0).abs() < 0.01
            && (description.y - 0.0).abs() < 0.01
            && (description.width - 300.0).abs() < 0.01,
        "description: {description:?}"
    );
    assert!(
        (media.x - 300.0).abs() < 0.01
            && (media.y - 0.0).abs() < 0.01
            && (media.height - 300.0).abs() < 0.01,
        "media: {media:?}"
    );
    assert!(
        (image.x - 320.0).abs() < 0.01
            && (image.y - 20.0).abs() < 0.01
            && (image.width - 260.0).abs() < 0.01
            && (image.height - 260.0).abs() < 0.01,
        "image: {image:?}"
    );
}

#[test]
fn auto_block_wrapper_contains_percentage_replaced_child() {
    let tree = parse_html(include_str!("../../../render-repros/replaced-block-wrapper.html"));
    let image_id = tree.get_element_by_id("image").unwrap();
    let intrinsic = HashMap::from([(image_id, (720.0, 424.0))]);
    let layout = layout_dom_with_images(&tree, (900.0, 1000.0), &intrinsic);
    let rect = |id| layout.rects[&tree.get_element_by_id(id).unwrap()];
    let media = rect("media");
    let frame = rect("frame");
    let wrapper = rect("wrapper");
    let image = rect("image");
    assert!(
        (media.x - 450.0).abs() < 0.01
            && (media.width - 450.0).abs() < 0.01,
        "media: {media:?}"
    );
    assert!(
        (frame.x - 450.0).abs() < 0.01
            && (frame.width - 450.0).abs() < 0.01,
        "frame: {frame:?}"
    );
    assert!(
        (wrapper.x - 498.0).abs() < 0.01
            && (wrapper.width - 354.0).abs() < 0.01,
        "wrapper: {wrapper:?}"
    );
    assert!(
        (image.x - 498.0).abs() < 0.01
            && (image.width - 354.0).abs() < 0.01
            && (image.height - 208.0).abs() < 0.02,
        "image: {image:?}"
    );
}

#[test]
fn text_alignment_does_not_shrink_flex_items() {
    let tree = parse_html(include_str!("../../../render-repros/text-align-flex-items.html"));
    let layout = layout_dom(&tree, (900.0, 1000.0));
    let rect = |id| layout.rects[&tree.get_element_by_id(id).unwrap()];
    let stack = rect("stack");
    let media = rect("media");
    let label = rect("label");
    let chip = rect("chip");
    assert!(
        (stack.width - 400.0).abs() < 0.01
            && (media.x - 20.0).abs() < 0.01
            && (media.width - 360.0).abs() < 0.01,
        "stack: {stack:?}, media: {media:?}"
    );
    assert!(
        (label.x - 20.0).abs() < 0.01
            && (label.width - 360.0).abs() < 0.01
            && (chip.x - 150.0).abs() < 0.01
            && (chip.width - 100.0).abs() < 0.01,
        "label: {label:?}, chip: {chip:?}"
    );
}

#[test]
fn text_alignment_does_not_shrink_block_children_or_wrap_inline_block_rows() {
    let tree = parse_html(
        r#"<html><head><style>
           html,body{margin:0}
           section{width:900px;text-align:center}
           p{margin:0}
           a{display:inline-block;width:100px;height:40px;margin-right:10px}
           </style></head><body><section>
           <p id="row"><!--[--><a id="one">Why Vue</a><!--]--><a id="two"></a><!--marker--><a id="three"></a><a id="four"></a></p>
           </section></body></html>"#,
    );
    let layout = layout_dom(&tree, (900.0, 1000.0));
    let rect = |id| layout.rects[&tree.get_element_by_id(id).unwrap()];
    let row = rect("row");
    let items = [rect("one"), rect("two"), rect("three"), rect("four")];
    assert!(
        (row.width - 900.0).abs() < 0.01,
        "text-align must not shrink-wrap an auto-width block child: {row:?}"
    );
    assert!(
        items.windows(2).all(|pair| (pair[0].y - pair[1].y).abs() < 0.01),
        "inline-block items should share one line when the block has room: {items:?}"
    );
    assert!(
        items[0].height <= 40.01,
        "auto-width inline-block text should use one max-content line: {:?}",
        items[0]
    );
}

#[test]
fn non_replaced_inline_ignores_used_width_height_and_min_max_sizes() {
    let tree = parse_html(
        r#"<style>
          html,body,p{margin:0}
          #host{width:300px;font-size:16px;line-height:20px}
          #token{
            position:relative;
            width:100%;height:100px;
            min-width:100%;min-height:100px;
            max-width:100%;max-height:100px;
            padding:0 5px;
            background:red
          }
          #after{position:relative;background:blue}
        </style>
        <p id="host">A <code id="token">token</code> <span id="after">after</span></p>"#,
    );
    let laid = layout_dom(&tree, (400.0, 200.0));
    let token_id = tree.get_element_by_id("token").unwrap();
    let after_id = tree.get_element_by_id("after").unwrap();
    let token = laid.rects[&token_id];
    let after = laid.rects[&after_id];

    assert_eq!(
        laid.styles[&token_id].width,
        obscura_render::Dimension::Percent(1.0),
        "computed style must retain the authored percentage"
    );
    assert!(
        token.width > 20.0 && token.width < 100.0,
        "a non-replaced inline's used width must hug its text and padding: {token:?}"
    );
    assert!(
        token.height < 40.0,
        "height/min-height/max-height do not apply to a non-replaced inline: {token:?}"
    );
    assert!(
        after.x >= token.x + token.width - 0.01 && (after.y - token.y).abs() < 0.01,
        "the following inline must remain on the same line: {token:?} {after:?}"
    );
}

#[test]
fn inline_svg_derives_auto_height_from_view_box() {
    let tree = parse_html(
        r#"<html><head><style>html,body{margin:0}#logo{width:112px}</style></head>
           <body><svg id="logo" viewBox="-10.5 -9.45 21 18.9"></svg></body></html>"#,
    );
    let layout = layout_dom(&tree, (900.0, 1000.0));
    let logo = layout.rects[&tree.get_element_by_id("logo").unwrap()];
    assert!((logo.width - 112.0).abs() < 0.01, "svg width: {logo:?}");
    assert!(
        (logo.height - 101.0).abs() < 0.01,
        "viewBox should supply the intrinsic auto-height (device-pixel rounded): {logo:?}"
    );
}

/// Chromium 150 resolves a ratio-only inline SVG as an atomic replaced item
/// during both intrinsic grid track sizing and the final percentage-width
/// pass. The 270px track leaves 254px after the grid item's inline padding;
/// the 320:96 viewBox then contributes 76.1875px of height.
#[test]
fn ratio_only_inline_svg_sizes_inside_an_auto_grid_row() {
    let tree = parse_html(
        r#"<html><head><style>
             html,body{margin:0}
             #grid{display:grid;grid-template-columns:270px}
             #cell{display:grid;place-content:center;padding:16px 8px}
             #logo{display:block;width:100%;max-width:320px}
           </style></head><body>
             <div id="grid"><div id="cell">
               <svg id="logo" viewBox="0 0 320 96"><path d="M0 0h320v96z"/></svg>
             </div></div>
           </body></html>"#,
    );
    let layout = layout_dom(&tree, (800.0, 600.0));
    let rect = |id| layout.rects[&tree.get_element_by_id(id).unwrap()];
    let cell = rect("cell");
    let logo = rect("logo");
    let logo_style = &layout.styles[&tree.get_element_by_id("logo").unwrap()];
    assert!(
        (logo.width - 254.0).abs() < 0.01,
        "SVG width: {logo:?}; declared width={:?} height={:?} min={:?} max={:?} display={:?} ratio={:?}",
        logo_style.width,
        logo_style.height,
        logo_style.min_width,
        logo_style.max_width,
        logo_style.display,
        logo_style.aspect_ratio,
    );
    assert!(
        (logo.height - 76.1875).abs() < 0.25,
        "viewBox ratio must transfer the final grid width before Obscura's device-pixel rounding: {logo:?}"
    );
    assert!(
        (cell.height - 108.1875).abs() < 0.25,
        "SVG contribution plus block padding must size the auto row: {cell:?}"
    );
}

/// A percentage-sized absolutely positioned SVG resolves against its
/// containing block like any other replaced element. The special intrinsic
/// sizing used for a ratio-only SVG in an auto grid track must not turn this
/// authored percentage into grid-style self-stretch (or the 300px default
/// object size wins instead).
#[test]
fn absolute_inline_svg_keeps_percentage_width_and_view_box_ratio() {
    let tree = parse_html(
        r#"<html><head><style>
             html,body{margin:0}
             #container{position:relative;width:800px;height:300px}
             #art{display:block;position:absolute;right:0;top:0;width:20%;height:auto}
           </style></head><body>
             <div id="container">
               <svg id="art" width="162" height="162" viewBox="0 0 162 162"></svg>
             </div>
           </body></html>"#,
    );
    let layout = layout_dom(&tree, (1000.0, 600.0));
    let art = layout.rects[&tree.get_element_by_id("art").unwrap()];
    assert!((art.x - 640.0).abs() < 0.01, "percentage width must anchor from the right: {art:?}");
    assert!((art.width - 160.0).abs() < 0.01, "percentage width: {art:?}");
    assert!((art.height - 160.0).abs() < 0.01, "viewBox ratio: {art:?}");
}

/// Chromium 150 includes collapsed descendant margins in a grid item's
/// intrinsic block-size. The nested 64px margin reaches through `section`,
/// making the first item 200px tall; the second row therefore begins after
/// that item and the 20px row gap, rather than overlapping the descendant.
#[test]
fn grid_item_intrinsic_height_includes_collapsed_descendant_margins() {
    let tree = parse_html(
        r#"<html><head><style>
             html,body{margin:0}
             #outer{display:grid;width:300px;grid-template-rows:auto auto;row-gap:20px}
             #item{position:relative}
             #lead{height:20px;margin-top:16px}
             #deep{height:100px;margin-top:64px}
             #after{height:10px}
           </style></head><body>
             <div id="outer">
               <div id="item">
                 <div id="lead"></div>
                 <section><div id="deep"></div></section>
               </div>
               <div id="after"></div>
             </div>
           </body></html>"#,
    );
    let layout = layout_dom(&tree, (800.0, 600.0));
    let rect = |id| layout.rects[&tree.get_element_by_id(id).unwrap()];
    let outer = rect("outer");
    let item = rect("item");
    let deep = rect("deep");
    let after = rect("after");

    assert!(
        (item.height - 200.0).abs() < 0.01 && (deep.y + deep.height - 200.0).abs() < 0.01,
        "the grid item must contain the collapsed nested margin: item={item:?} deep={deep:?}"
    );
    assert!(
        (after.y - 220.0).abs() < 0.01 && (outer.height - 230.0).abs() < 0.01,
        "later grid rows must not overlap overflowing intrinsic content: outer={outer:?} after={after:?}"
    );
}

/// Collapsible margin sets retain their largest positive and most-negative
/// members separately. A negative nested margin therefore shortens the grid
/// item's intrinsic contribution and overlaps the preceding block in exactly
/// the same way during measurement and final layout.
#[test]
fn grid_item_intrinsic_height_keeps_negative_collapsed_margin() {
    let tree = parse_html(
        r#"<html><head><style>
             html,body{margin:0}
             #outer{display:grid;width:300px;grid-template-rows:auto auto;row-gap:20px}
             #item{position:relative}
             #lead{height:20px;margin-top:16px}
             #deep{height:100px;margin-top:-24px}
             #after{height:10px}
           </style></head><body>
             <div id="outer">
               <div id="item">
                 <div id="lead"></div>
                 <section><div id="deep"></div></section>
               </div>
               <div id="after"></div>
             </div>
           </body></html>"#,
    );
    let layout = layout_dom(&tree, (800.0, 600.0));
    let rect = |id| layout.rects[&tree.get_element_by_id(id).unwrap()];
    let outer = rect("outer");
    let item = rect("item");
    let deep = rect("deep");
    let after = rect("after");

    assert!(
        (item.height - 112.0).abs() < 0.01
            && (deep.y - 12.0).abs() < 0.01
            && (deep.height - 100.0).abs() < 0.01,
        "negative collapsed margin geometry must survive measurement: item={item:?} deep={deep:?}"
    );
    assert!(
        (after.y - 132.0).abs() < 0.01 && (outer.height - 142.0).abs() < 0.01,
        "the following grid row must use the shortened contribution: outer={outer:?} after={after:?}"
    );
}

#[test]
fn inline_block_flex_items_keep_block_inner_flow() {
    let tree = parse_html(include_str!("../../../render-repros/inline-block-flex-items.html"));
    let layout = layout_dom(&tree, (900.0, 1000.0));
    let rect = |id| layout.rects[&tree.get_element_by_id(id).unwrap()];
    let brand = rect("brand");
    let title = rect("title");
    let links = rect("links");
    let one = rect("one");
    let two = rect("two");
    let three = rect("three");
    let chips = rect("chips");
    let alpha = rect("alpha");
    let beta = rect("beta");
    let gamma = rect("gamma");
    assert!(
        (brand.x - 0.0).abs() < 0.01
            && (brand.width - 382.0).abs() < 0.01
            && (title.x - 32.0).abs() < 0.01
            && (title.width - 350.0).abs() < 0.01,
        "brand: {brand:?}, title: {title:?}"
    );
    assert!(
        (links.x - 525.0).abs() < 0.01
            && (links.width - 90.0).abs() < 0.01
            && (one.y - 0.0).abs() < 0.01
            && (two.y - 20.0).abs() < 0.01
            && (three.y - 40.0).abs() < 0.01,
        "links: {links:?}, items: {one:?} {two:?} {three:?}"
    );
    assert!(
        (chips.x - 0.0).abs() < 0.01
            && (chips.y - 140.0).abs() < 0.01
            && (chips.width - 90.0).abs() < 0.01
            && (chips.height - 20.0).abs() < 0.01
            && (alpha.x - 0.0).abs() < 0.01
            && (beta.x - 30.0).abs() < 0.01
            && (gamma.x - 60.0).abs() < 0.01,
        "chips: {chips:?}, items: {alpha:?} {beta:?} {gamma:?}"
    );
}

#[test]
fn inline_block_block_children_stack_and_shrink_to_the_widest_child() {
    let tree = parse_html(include_str!("../../../render-repros/inline-block-block-children.html"));
    let layout = layout_dom(&tree, (300.0, 200.0));
    let rect = |id| layout.rects[&tree.get_element_by_id(id).unwrap()];
    for (id, expected) in [
        ("plain", (0.0, 0.0, 144.0, 74.0)),
        ("plain-one", (12.0, 12.0, 80.0, 20.0)),
        ("plain-two", (12.0, 32.0, 120.0, 30.0)),
        ("margins", (0.0, 74.0, 144.0, 89.0)),
        ("margin-one", (12.0, 86.0, 80.0, 20.0)),
        ("margin-two", (12.0, 121.0, 120.0, 30.0)),
    ] {
        let actual = rect(id);
        assert!((actual.x - expected.0).abs() < 0.01
            && (actual.y - expected.1).abs() < 0.01
            && (actual.width - expected.2).abs() < 0.01
            && (actual.height - expected.3).abs() < 0.01,
            "{id}: {actual:?}, expected {expected:?}");
    }
}

#[test]
fn percentage_height_under_indefinite_parent_uses_content_height() {
    let tree = parse_html(include_str!(
        "../../../render-repros/percentage-height-indefinite-parent.html"
    ));
    let layout = layout_dom(&tree, (900.0, 1000.0));
    for id in ["wrapper", "stack", "editor", "pre", "code"] {
        let rect = layout.rects[&tree.get_element_by_id(id).unwrap()];
        assert!(
            rect.height >= 24.0,
            "{id} collapsed despite its indefinite percentage height: {rect:?}"
        );
    }
}

#[test]
fn item_self_alignment_places_flex_and_grid_items() {
    let tree = parse_html(include_str!("../../../render-repros/item-self-alignment.html"));
    let layout = layout_dom(&tree, (900.0, 1000.0));
    let rect = |id| layout.rects[&tree.get_element_by_id(id).unwrap()];
    let flex_center = rect("flex-center");
    let flex_end = rect("flex-end");
    let grid_end = rect("grid-end");
    let grid_center = rect("grid-center");
    let grid_place = rect("grid-place");
    let parent_one = rect("parent-one");
    let parent_two = rect("parent-two");
    let line_one = rect("line-one");
    let line_two = rect("line-two");
    let track_one = rect("track-one");
    let track_two = rect("track-two");
    assert!(
        (flex_center.x - 0.0).abs() < 0.01
            && (flex_center.y - 40.0).abs() < 0.01
            && (flex_end.x - 40.0).abs() < 0.01
            && (flex_end.y - 70.0).abs() < 0.01,
        "flex items: {flex_center:?} {flex_end:?}"
    );
    assert!(
        (grid_end.x - 110.0).abs() < 0.01
            && (grid_end.y - 165.0).abs() < 0.01
            && (grid_center.x - 200.0).abs() < 0.01
            && (grid_center.y - 220.0).abs() < 0.01
            && (grid_place.x - 420.0).abs() < 0.01
            && (grid_place.y - 160.0).abs() < 0.01,
        "grid items: {grid_end:?} {grid_center:?} {grid_place:?}"
    );
    assert!(
        (parent_one.x - 55.0).abs() < 0.01
            && (parent_one.y - 340.0).abs() < 0.01
            && (parent_two.x - 195.0).abs() < 0.01
            && (parent_two.y - 330.0).abs() < 0.01,
        "parent-aligned grid items: {parent_one:?} {parent_two:?}"
    );
    assert!(
        (line_one.x - 0.0).abs() < 0.01
            && (line_one.y - 380.0).abs() < 0.01
            && (line_two.x - 0.0).abs() < 0.01
            && (line_two.y - 480.0).abs() < 0.01,
        "aligned flex lines: {line_one:?} {line_two:?}"
    );
    assert!(
        (track_one.x - 100.0).abs() < 0.01
            && (track_one.y - 570.0).abs() < 0.01
            && (track_two.x - 100.0).abs() < 0.01
            && (track_two.y - 620.0).abs() < 0.01,
        "aligned grid tracks: {track_one:?} {track_two:?}"
    );
}

#[test]
fn stretched_inline_size_container_has_zero_intrinsic_grid_contribution() {
    // Chromium 140: size containment affects intrinsic track sizing, but not
    // the item's later stretch alignment. In the first case the grid therefore
    // shrink-wraps to zero. In the definite-width control the contained item
    // contributes zero, then stretches to the 50px left after the fixed track.
    // Its 240px child remains visible overflow in both cases.
    let tree = parse_html(
        r#"
        <style>
          html, body { margin: 0 }
          #shrink-host { display: flex; width: 600px }
          .grid { display: grid }
          .item { container-type: inline-size }
          .child { width: 240px; height: 20px }
          #fixed-grid {
            display: grid;
            width: 150px;
            grid-template-columns: auto 100px;
          }
          #fixed { height: 20px }
        </style>
        <div id="shrink-host">
          <div id="shrink-grid" class="grid">
            <div id="shrink-item" class="item"><div id="shrink-child" class="child"></div></div>
          </div>
        </div>
        <div id="fixed-grid" class="grid">
            <div id="fixed-item" class="item"><div id="fixed-child" class="child"></div></div>
            <div id="fixed"></div>
        </div>
        "#,
    );
    let layout = layout_dom(&tree, (800.0, 600.0));
    let rect = |id| layout.rects[&tree.get_element_by_id(id).unwrap()];
    let shrink_grid = rect("shrink-grid");
    let shrink_item = rect("shrink-item");
    let shrink_child = rect("shrink-child");
    let fixed_grid = rect("fixed-grid");
    let fixed_item = rect("fixed-item");
    let fixed_child = rect("fixed-child");
    let fixed = rect("fixed");
    assert!(
        shrink_grid.width.abs() < 0.01
            && shrink_item.width.abs() < 0.01
            && (shrink_child.width - 240.0).abs() < 0.01
            && (fixed_grid.width - 150.0).abs() < 0.01
            && (fixed_item.width - 50.0).abs() < 0.01
            && (fixed_child.width - 240.0).abs() < 0.01
            && (fixed.x - 50.0).abs() < 0.01
            && (fixed.width - 100.0).abs() < 0.01,
        "contained intrinsic contribution leaked into the grid track: \
         shrink=({shrink_grid:?}, {shrink_item:?}, {shrink_child:?}) \
         fixed=({fixed_grid:?}, {fixed_item:?}, {fixed_child:?}, {fixed:?})"
    );
}

#[test]
fn stretched_size_container_has_zero_intrinsic_grid_row_contribution() {
    // Chromium 140: block-axis size containment removes the tall child's
    // intrinsic row contribution, but leaves the grid item's auto height
    // available for final stretch alignment. Padding and border remain inside
    // the fixed row's stretched 100px border box.
    let tree = parse_html(
        r#"
        <style>
          html, body { margin: 0 }
          .grid { display: grid }
          .item { container-type: size }
          .child { width: 20px; height: 240px }
          #fixed-grid {
            margin-top: 20px;
            grid-template-rows: 100px;
          }
          #fixed-item {
            padding-block: 10px;
            border-top: 5px solid black;
            border-bottom: 5px solid black;
          }
        </style>
        <div id="shrink-grid" class="grid">
          <div id="shrink-item" class="item">
            <div id="shrink-child" class="child"></div>
          </div>
        </div>
        <div id="fixed-grid" class="grid">
          <div id="fixed-item" class="item">
            <div id="fixed-child" class="child"></div>
          </div>
        </div>
        "#,
    );
    let layout = layout_dom(&tree, (800.0, 600.0));
    let rect = |id| layout.rects[&tree.get_element_by_id(id).unwrap()];
    let shrink_grid = rect("shrink-grid");
    let shrink_item = rect("shrink-item");
    let shrink_child = rect("shrink-child");
    let fixed_grid = rect("fixed-grid");
    let fixed_item = rect("fixed-item");
    let fixed_child = rect("fixed-child");
    assert!(
        shrink_grid.height.abs() < 0.01
            && shrink_item.height.abs() < 0.01
            && (shrink_child.height - 240.0).abs() < 0.01
            && (fixed_grid.height - 100.0).abs() < 0.01
            && (fixed_item.height - 100.0).abs() < 0.01
            && (fixed_child.y - fixed_item.y - 15.0).abs() < 0.01
            && (fixed_child.height - 240.0).abs() < 0.01,
        "contained intrinsic contribution leaked into the grid row: \
         shrink=({shrink_grid:?}, {shrink_item:?}, {shrink_child:?}) \
         fixed=({fixed_grid:?}, {fixed_item:?}, {fixed_child:?})"
    );
}

#[test]
fn non_stretched_auto_grid_item_shrink_wraps_inside_grid_area() {
    let tree = parse_html(
        r#"
        <style>
          body { margin: 0 }
          .panel {
            display: grid;
            align-items: start;
            justify-items: center;
            box-sizing: border-box;
          }
          demo-wrap { display: block }
          #wide-panel {
            margin-left: 112px;
            width: 1168px;
            padding: 16px 48px;
          }
          #wide-inner {
            display: grid;
            grid-template-columns: 1fr 1fr;
            box-sizing: border-box;
            width: 100%;
            max-width: 1200px;
            padding: 32px;
          }
          #wide-inner span {
            display: inline-block;
            width: 56px;
            height: 20px;
          }
          #narrow-panel { width: 400px }
          #narrow-inner { width: 200px; height: 20px }
        </style>
        <div id="wide-panel" class="panel">
          <demo-wrap id="wide-wrapper">
            <div id="wide-inner">
              <div>
                <span></span><span></span><span></span><span></span><span></span><span></span>
                <span></span><span></span><span></span><span></span><span></span>
              </div>
              <div>
                <span></span><span></span><span></span><span></span><span></span><span></span>
                <span></span><span></span><span></span><span></span><span></span>
              </div>
            </div>
          </demo-wrap>
        </div>
        <div id="narrow-panel" class="panel">
          <demo-wrap id="narrow-wrapper"><div id="narrow-inner"></div></demo-wrap>
        </div>
        "#,
    );
    let layout = layout_dom(&tree, (1280.0, 1000.0));
    let rect = |id| layout.rects[&tree.get_element_by_id(id).unwrap()];
    let wide_panel = rect("wide-panel");
    let wide_wrapper = rect("wide-wrapper");
    let wide_inner = rect("wide-inner");
    let narrow_wrapper = rect("narrow-wrapper");
    assert!(
        (wide_wrapper.x - 160.0).abs() < 0.01
            && (wide_wrapper.width - 1072.0).abs() < 0.01
            && (wide_inner.x - 160.0).abs() < 0.01
            && (wide_inner.width - 1072.0).abs() < 0.01,
        "wide shrink-wrapped item should be clamped to the grid area: \
         panel={wide_panel:?} wrapper={wide_wrapper:?} inner={wide_inner:?}"
    );
    assert!(
        (narrow_wrapper.x - 100.0).abs() < 0.01
            && (narrow_wrapper.width - 200.0).abs() < 0.01,
        "a narrower item should keep its intrinsic width and remain centered: {narrow_wrapper:?}"
    );
}

#[test]
fn percentage_max_width_clamps_grid_item_intrinsic_size() {
    let tree = parse_html(
        r#"
        <style>
          body { margin: 0 }
          #grid { display: grid; width: 1200px }
          #item { max-width: 100% }
          #gallery { display: flex; gap: 8px }
          .column { flex: none; height: 20px }
        </style>
        <div id="grid">
          <div id="item">
            <div id="gallery">
              <div class="column" style="width:303px"></div>
              <div class="column" style="width:200px"></div>
              <div class="column" style="width:500px"></div>
              <div class="column" style="width:200px"></div>
              <div class="column" style="width:293px"></div>
            </div>
          </div>
        </div>
        "#,
    );
    let layout = layout_dom(&tree, (1280.0, 200.0));
    let grid = layout.rects[&tree.get_element_by_id("grid").unwrap()];
    let item = layout.rects[&tree.get_element_by_id("item").unwrap()];
    let gallery = layout.rects[&tree.get_element_by_id("gallery").unwrap()];
    assert!(
        (grid.width - 1200.0).abs() < 0.01
            && (item.width - 1200.0).abs() < 0.01
            && (gallery.width - 1200.0).abs() < 0.01,
        "max-width:100% must clamp the grid item while its flex contents overflow: \
         grid={grid:?} item={item:?} gallery={gallery:?}"
    );
}

#[test]
fn named_area_shorthand_survives_end_longhand_override() {
    let tree = parse_html(
        r#"
        <style>
          body { margin: 0 }
          #grid {
            display: grid;
            width: 1200px;
            grid-template-columns:
              [extended-full-start] 16px
              [full-start] 200px
              [content-start] 768px
              [content-end] 200px
              [full-end] 16px
              [extended-full-end];
          }
          #grid > * { grid-column: content }
          #hero { grid-column-end: extended-full-end }
        </style>
        <div id="grid"><div id="hero"></div></div>
        "#,
    );
    let layout = layout_dom(&tree, (1280.0, 200.0));
    let hero = layout.rects[&tree.get_element_by_id("hero").unwrap()];
    assert!(
        (hero.x - 216.0).abs() < 0.01 && (hero.width - 984.0).abs() < 0.01,
        "the end longhand must retain the shorthand's named content start: {hero:?}"
    );
}

#[test]
fn auto_width_flex_button_keeps_native_intrinsic_sizing() {
    let tree = parse_html(
        r#"
        <style>
          body { margin: 0 }
          #box { width: 600px }
          button {
            display: flex;
            margin: 0 auto;
            padding: 19px 24px;
            border: 2px solid;
            gap: 6px;
            font-size: 24px;
          }
          button::before { content: ""; width: 1em; height: 1em }
        </style>
        <div id="box"><button id="search">Search</button></div>
        "#,
    );
    let layout = layout_dom(&tree, (800.0, 200.0));
    let button = layout.rects[&tree.get_element_by_id("search").unwrap()];
    assert!(
        button.width > 120.0
            && button.width < 220.0
            && (button.x - (600.0 - button.width) / 2.0).abs() < 0.01,
        "the auto-width flex button should shrink-wrap and center: {button:?}"
    );
}

#[test]
fn flex_auto_margin_absorbs_space_before_justify_content() {
    let tree = parse_html(
        r#"
        <style>
          body { margin: 0 }
          #bar {
            display: flex;
            width: 1280px;
            gap: 16px;
            justify-content: flex-end;
          }
          #crumb { width: 42px; height: 20px; margin: 0 auto 0 0 }
          #theme { width: 91px; height: 20px }
          #language { width: 131px; height: 20px }
        </style>
        <div id="bar">
          <div id="crumb"></div>
          <div id="theme"></div>
          <div id="language"></div>
        </div>
        "#,
    );
    let layout = layout_dom(&tree, (1280.0, 200.0));
    let rect = |id| layout.rects[&tree.get_element_by_id(id).unwrap()];
    let crumb = rect("crumb");
    let theme = rect("theme");
    let language = rect("language");
    assert!(
        (crumb.x - 0.0).abs() < 0.01
            && (theme.x - 1042.0).abs() < 0.01
            && (language.x - 1149.0).abs() < 0.01,
        "main-axis auto margin must consume free space before justify-content: \
         crumb={crumb:?} theme={theme:?} language={language:?}"
    );
}

#[test]
fn nested_is_descendant_utility_blockifies_matching_lines() {
    let tree = parse_html(
        r#"
        <style>
          pre { display: flex }
          code code { position: relative }
          :is(.\*\*\:\[\.line\]\:block *).line { display: block }
        </style>
        <div class="**:[.line]:block">
          <pre><code><code><span id="one" class="line">one</span><span id="two" class="line">two</span></code></code></pre>
        </div>
        "#,
    );
    let layout = layout_dom(&tree, (500.0, 200.0));
    let one = tree.get_element_by_id("one").unwrap();
    let two = tree.get_element_by_id("two").unwrap();
    assert_eq!(layout.styles[&one].display, obscura_render::Display::Block);
    assert_eq!(layout.styles[&two].display, obscura_render::Display::Block);
    assert!(layout.rects[&two].y > layout.rects[&one].y);
    assert_eq!(layout.rects[&one].width, layout.rects[&two].width);
}

#[test]
fn opposing_floats_share_header_band_through_inline_wrapper() {
    let tree = parse_html(include_str!("../../../render-repros/opposing-header-floats.html"));
    let layout = layout_dom(&tree, (900.0, 1000.0));
    let rect = |id| layout.rects[&tree.get_element_by_id(id).unwrap()];
    let logo = rect("logo");
    let tagline = rect("tagline");
    let menu = rect("menu");
    assert!(
        (logo.x - 0.0).abs() < 0.01
            && (logo.y - 0.0).abs() < 0.01
            && (logo.width - 200.0).abs() < 0.01
            && (logo.height - 100.0).abs() < 0.01,
        "logo: {logo:?}"
    );
    assert!(
        (tagline.x - 600.0).abs() < 0.01
            && (tagline.y - 60.0).abs() < 0.01
            && (tagline.width - 300.0).abs() < 0.01
            && (tagline.height - 40.0).abs() < 0.01,
        "tagline: {tagline:?}"
    );
    assert!(
        (menu.x - 0.0).abs() < 0.01
            && (menu.y - 100.0).abs() < 0.01
            && (menu.width - 900.0).abs() < 0.01,
        "menu: {menu:?}"
    );
}

#[test]
fn mixed_float_run_shares_one_band() {
    let tree = parse_html(include_str!("../../../render-repros/mixed-float-run.html"));
    let layout = layout_dom(&tree, (900.0, 1000.0));
    let rect = |id| layout.rects[&tree.get_element_by_id(id).unwrap()];
    let left_one = rect("left-one");
    let left_two = rect("left-two");
    let right_one = rect("right-one");
    assert!(
        (left_one.x - 0.0).abs() < 0.01
            && (left_one.y - 0.0).abs() < 0.01
            && (left_one.width - 80.0).abs() < 0.01
            && (left_one.height - 30.0).abs() < 0.01,
        "first left float: {left_one:?}"
    );
    assert!(
        (left_two.x - 80.0).abs() < 0.01
            && (left_two.y - 0.0).abs() < 0.01
            && (left_two.width - 80.0).abs() < 0.01
            && (left_two.height - 30.0).abs() < 0.01,
        "second left float: {left_two:?}"
    );
    assert!(
        (right_one.x - 340.0).abs() < 0.01
            && (right_one.y - 0.0).abs() < 0.01
            && (right_one.width - 60.0).abs() < 0.01
            && (right_one.height - 30.0).abs() < 0.01,
        "right float: {right_one:?}"
    );
}

#[test]
fn right_float_navigation_shares_inline_band() {
    let tree = parse_html(include_str!(
        "../../../render-repros/right-float-navigation.html"
    ));
    let layout = layout_dom(&tree, (900.0, 1000.0));
    let rect = |id| layout.rects[&tree.get_element_by_id(id).unwrap()];
    let left_one = rect("left-one");
    let left_two = rect("left-two");
    let right_one = rect("right-one");
    let right_two = rect("right-two");
    let right_three = rect("right-three");
    assert!(
        (left_one.x - 0.0).abs() < 0.01
            && (left_one.y - 0.0).abs() < 0.01
            && (left_two.x - 64.0).abs() < 0.01
            && (left_two.y - 0.0).abs() < 0.01,
        "inline flow: {left_one:?} {left_two:?}"
    );
    assert!(
        (right_three.x - 280.0).abs() < 0.01
            && (right_three.y - 0.0).abs() < 0.01
            && (right_two.x - 310.0).abs() < 0.01
            && (right_two.y - 0.0).abs() < 0.01
            && (right_one.x - 360.0).abs() < 0.01
            && (right_one.y - 0.0).abs() < 0.01,
        "right float order: {right_three:?} {right_two:?} {right_one:?}"
    );
}

#[test]
fn functional_font_sizes_resolve_against_live_viewport() {
    let tree = parse_html(
        r#"
        <style>
          #feature { font-size: clamp(1rem, 9vw, 38pt) }
          #subtitle { font-size: clamp(.5rem, 4vw, 1rem) }
        </style>
        <h2 id="feature">Features</h2>
        <h3 id="subtitle">Subtitle</h3>
        "#,
    );
    let wide = layout_dom(&tree, (900.0, 1000.0));
    let feature = tree.get_element_by_id("feature").unwrap();
    let subtitle = tree.get_element_by_id("subtitle").unwrap();
    assert!(
        (wide.styles[&feature].font_size.unwrap() - 50.654).abs() < 0.05,
        "wide feature font: {:?}",
        wide.styles[&feature].font_size
    );
    assert_eq!(wide.styles[&subtitle].font_size, Some(16.0));

    let narrow = layout_dom(&tree, (320.0, 640.0));
    assert!(
        (narrow.styles[&feature].font_size.unwrap() - 28.8).abs() < 0.05,
        "narrow feature font: {:?}",
        narrow.styles[&feature].font_size
    );
    assert!(
        (narrow.styles[&subtitle].font_size.unwrap() - 12.8).abs() < 0.05,
        "narrow subtitle font: {:?}",
        narrow.styles[&subtitle].font_size
    );
}

#[test]
fn responsive_grid_child_spans_all_tracks_via_end_longhand() {
    let tree = parse_html(
        r#"
        <style>
          .layout {
            display: grid;
            width: 800px;
            grid-template-columns: repeat(4, 1fr);
            column-gap: 20px;
          }
          @media (max-width: 1007px) {
            .layout > * { grid-column-end: span 4 }
          }
        </style>
        <div class="layout"><section id="hero">Full-width content</section></div>
        "#,
    );
    let layout = layout_dom(&tree, (900.0, 1000.0));
    let hero = layout.rects[&tree.get_element_by_id("hero").unwrap()];
    assert!(
        (hero.width - 800.0).abs() < 0.01,
        "span-4 child should fill all four tracks: {hero:?}"
    );
}

#[test]
fn grid_column_auto_span_shorthand_places_six_cards_in_two_rows() {
    let tree = parse_html(
        r#"
        <style>
          .gallery {
            display: grid;
            width: 1376px;
            grid-template-columns: repeat(12, minmax(0, 1fr));
            column-gap: 16px;
            row-gap: 64px;
          }
          .card {
            grid-column: auto / span 4;
            height: 100px;
          }
        </style>
        <div class="gallery">
          <div class="card" id="card-1"></div>
          <div class="card" id="card-2"></div>
          <div class="card" id="card-3"></div>
          <div class="card" id="card-4"></div>
          <div class="card" id="card-5"></div>
          <div class="card" id="card-6"></div>
        </div>
        "#,
    );
    let layout = layout_dom(&tree, (1440.0, 900.0));
    let cards: Vec<_> = (1..=6)
        .map(|index| {
            let id = tree
                .get_element_by_id(&format!("card-{index}"))
                .expect("card exists");
            layout.rects[&id]
        })
        .collect();

    for card in &cards {
        assert!(
            (card.width - 448.0).abs() < 0.01,
            "four of twelve tracks plus internal gaps should be 448px: {card:?}"
        );
    }
    assert_eq!(cards[0].y, cards[1].y);
    assert_eq!(cards[1].y, cards[2].y);
    assert_eq!(cards[3].y, cards[4].y);
    assert_eq!(cards[4].y, cards[5].y);
    assert!(
        (cards[3].y - cards[0].y - 164.0).abs() < 0.01,
        "fourth card should start a second row after the 64px row gap: {:?}",
        cards
    );
    assert!(
        (cards[1].x - cards[0].x - 464.0).abs() < 0.01
            && (cards[2].x - cards[1].x - 464.0).abs() < 0.01,
        "cards should advance by four tracks plus the outer column gap: {:?}",
        cards
    );
}

#[test]
fn length_line_height_computes_before_inheritance() {
    let tree = parse_html(
        r#"
        <style>
          #parent { font-size: 64px; line-height: 5rem }
          #child { font-size: 20px }
          #ratio { font-size: 20px; line-height: 2 }
        </style>
        <div id="parent"><span id="child">Inherited length</span></div>
        <div id="ratio">Unitless ratio</div>
        "#,
    );
    let layout = layout_dom(&tree, (900.0, 1000.0));
    let parent = tree.get_element_by_id("parent").unwrap();
    let child = tree.get_element_by_id("child").unwrap();
    let ratio = tree.get_element_by_id("ratio").unwrap();
    assert_eq!(
        layout.styles[&parent].line_height,
        Some(obscura_render::LineHeight::Px(80.0))
    );
    assert_eq!(
        layout.styles[&child].line_height,
        Some(obscura_render::LineHeight::Px(80.0))
    );
    assert_eq!(
        layout.styles[&ratio].line_height,
        Some(obscura_render::LineHeight::Ratio(2.0))
    );
}

#[test]
fn font_weight_computes_numerically_through_inheritance() {
    let tree = parse_html(
        r#"
        <div id="medium" style="font-weight:500">
          <span id="inherited">medium</span>
          <strong id="normal" style="font-weight:normal">normal</strong>
          <span id="bolder" style="font-weight:bolder">bolder</span>
          <span id="lighter" style="font-weight:lighter">lighter</span>
        </div>
        "#,
    );
    let layout = layout_dom(&tree, (900.0, 1000.0));
    let weight = |id| {
        layout.styles[&tree.get_element_by_id(id).unwrap()]
            .font_weight
            .as_deref()
    };
    assert_eq!(weight("medium"), Some("500"));
    assert_eq!(weight("inherited"), Some("500"));
    assert_eq!(weight("normal"), Some("400"));
    assert_eq!(weight("bolder"), Some("700"));
    assert_eq!(weight("lighter"), Some("100"));
}

#[test]
fn heading_font_weight_css_wide_keywords_override_the_ua_bold_rule() {
    let tree = parse_html(
        r#"
        <style>
          @layer base {
            #parent { font-weight: 500 }
            #inherited { font-weight: inherit }
            #unset { font-weight: unset }
            #initial { font-weight: initial }
          }
        </style>
        <div id="parent">
          <h1 id="ua">UA bold</h1>
          <h1 id="inherited">Inherited medium</h1>
          <h1 id="unset">Unset medium</h1>
          <h1 id="initial">Initial normal</h1>
        </div>
        "#,
    );
    let layout = layout_dom(&tree, (900.0, 1000.0));
    let weight = |id| {
        layout.styles[&tree.get_element_by_id(id).unwrap()]
            .font_weight
            .as_deref()
    };
    assert_eq!(weight("ua"), Some("700"));
    assert_eq!(weight("inherited"), Some("500"));
    assert_eq!(weight("unset"), Some("500"));
    assert_eq!(weight("initial"), Some("400"));
}

#[test]
fn contextual_right_inset_uses_root_font_tokens() {
    let tree = parse_html(
        r#"
        <style>
          :root { --spacing: .25rem }
          body { margin: 0 }
          #header { position: relative; width: 900px; height: 80px }
          #search {
            position: absolute;
            right: calc(var(--spacing)*14);
            width: 100px;
            height: 36px;
          }
        </style>
        <div id="header"><button id="search">Search</button></div>
        "#,
    );
    let layout = layout_dom(&tree, (900.0, 1000.0));
    let search = tree.get_element_by_id("search").unwrap();
    assert_eq!(
        layout.styles[&search].inset[1],
        Some(obscura_render::Dimension::Px(56.0))
    );
    let rect = layout.rects[&search];
    assert!(
        (rect.x - 744.0).abs() < 0.01,
        "right inset should place 100px box at 900 - 56 - 100: {rect:?}"
    );
}

#[test]
fn false_legacy_supports_probe_does_not_override_modern_line_height() {
    // Tailwind v4 uses this old-browser probe around a leading-variable reset.
    // Chromium rejects the condition. Flattening every @supports body made
    // the reset win, invalidated the var(), and inflated three 96px lines from
    // 288px to an inherited 432px.
    let tree = parse_html(
        r#"
        <style>
          @supports (((-webkit-hyphens:none)) and
                    (not (margin-trim:inline))) or
                    ((-moz-orient:inline) and
                    (not (color:rgb(from red r g b)))) {
            * { --leading:initial }
          }
          :root { --hero-line-height:1 }
          body { margin:0; line-height:1.5 }
          #hero {
            margin:0;
            width:600px;
            font-size:96px;
            line-height:var(--leading,var(--hero-line-height))
          }
        </style>
        <h1 id="hero">one<br>two<br>three</h1>
        "#,
    );
    let layout = layout_dom(&tree, (1280.0, 720.0));
    let hero = layout.rects[&tree.get_element_by_id("hero").unwrap()];
    assert!(
        (hero.height - 288.0).abs() < 0.01,
        "three 96px modern lines should be 288px tall: {hero:?}"
    );
}

#[test]
fn custom_property_css_wide_keywords_preserve_var_fallback_semantics() {
    let tree = parse_html(
        r#"
        <style>
          :root {
            --tone:#112233;
            --light:initial;
            --toggle:var(--light) #18191b;
            --page:var(--toggle,#ffffff)
          }
          #invalid { --tone:initial; color:var(--tone,#445566) }
          #inherited { --tone:unset; color:var(--tone,#445566) }
          #nested { background-color:var(--page) }
        </style>
        <div id="invalid">fallback</div>
        <div id="inherited">inherit</div>
        <div id="nested">nested fallback</div>
        "#,
    );
    let layout = layout_dom(&tree, (800.0, 600.0));
    let invalid = tree.get_element_by_id("invalid").unwrap();
    let inherited = tree.get_element_by_id("inherited").unwrap();
    let nested = tree.get_element_by_id("nested").unwrap();
    assert_eq!(
        layout.styles[&invalid].color,
        Some([0x44, 0x55, 0x66, 0xff]),
        "`initial` is guaranteed-invalid and var() must use its fallback"
    );
    assert_eq!(
        layout.styles[&inherited].color,
        Some([0x11, 0x22, 0x33, 0xff]),
        "`unset` inherits a custom property's parent value"
    );
    assert_eq!(
        layout.styles[&nested].background_color,
        Some([0xff, 0xff, 0xff, 0xff]),
        "an invalid nested custom property must activate the outer fallback"
    );
}

#[test]
fn stable_root_scrollbar_gutter_reduces_the_initial_containing_block() {
    let tree = parse_html(
        r#"
        <style>
          html { overflow-y:auto; scrollbar-gutter:stable }
          body { margin:0 }
          #fill { width:100%; height:40px }
          #center { width:401px; height:40px; margin-inline:auto }
        </style>
        <div id="fill"></div>
        <div id="center"></div>
        "#,
    );
    let layout = layout_dom(&tree, (900.0, 600.0));
    let fill = layout.rects[&tree.get_element_by_id("fill").unwrap()];
    let center = layout.rects[&tree.get_element_by_id("center").unwrap()];
    assert!(
        (fill.width - 885.0).abs() < 0.01,
        "stable classic gutter should reserve 15px: {fill:?}"
    );
    assert!(
        (center.x - 242.0).abs() < 0.01,
        "401px box should center in the 885px ICB: {center:?}"
    );
}

#[test]
fn auto_grid_track_freezes_at_growth_limit_during_distribution() {
    // The first auto track reaches its max-content growth limit before the
    // second. The remaining free space belongs only to the second track.
    // Giving the frozen track that remainder again makes the grid overflow
    // even though the two automatic minimums fit the definite container.
    let tree = parse_html(
        r#"
        <style>
          * { box-sizing:border-box }
          html, body { margin:0 }
          #grid {
            display:grid;
            width:1016px;
            grid-template-columns:auto auto;
            grid-template-rows:320px;
            column-gap:16px;
          }
          #copy {
            grid-column:1;
            font:400 48px/57.6px sans-serif;
          }
          #art {
            grid-column:2;
            overflow:hidden;
          }
          #art > div { width:560px; height:560px }
        </style>
        <div id="grid">
          <h1 id="copy">Resources for Developers,<br> by Developers</h1>
          <div id="art"><div></div></div>
        </div>
        "#,
    );
    let layout = layout_dom(&tree, (1280.0, 720.0));
    let rect = |id| layout.rects[&tree.get_element_by_id(id).unwrap()];
    let grid = rect("grid");
    let copy = rect("copy");
    let art = rect("art");
    assert!(
        ((copy.x + copy.width + 16.0) - art.x).abs() < 0.01,
        "the authored column gap must separate the final tracks: {copy:?} {art:?}"
    );
    assert!(
        ((art.x + art.width) - (grid.x + grid.width)).abs() < 0.01,
        "free-space redistribution must not grow a track after it freezes: {grid:?} {copy:?} {art:?}"
    );
}

#[test]
fn grid_area_line_shorthand_overlays_children_in_the_same_cell() {
    let tree = parse_html(
        r#"
        <style>
          * { box-sizing:border-box }
          html, body { margin:0 }
          #grid {
            display:grid;
            width:320px;
            grid-template-columns:320px;
            grid-template-rows:180px;
          }
          #grid > * {
            grid-area:1 / 1 / 1 / 1;
            width:320px;
            height:180px;
          }
        </style>
        <div id="grid">
          <div id="left"></div>
          <div id="right"></div>
          <div id="slider"></div>
        </div>
        "#,
    );
    let layout = layout_dom(&tree, (640.0, 480.0));
    let rect = |id| layout.rects[&tree.get_element_by_id(id).unwrap()];
    let grid = rect("grid");
    for id in ["left", "right", "slider"] {
        let child = rect(id);
        assert!(
            (child.x - grid.x).abs() < 0.01 && (child.y - grid.y).abs() < 0.01,
            "{id} should overlay the first grid cell instead of auto-placing: \
             grid={grid:?} child={child:?}"
        );
    }
    assert!(
        (grid.height - 180.0).abs() < 0.01,
        "overlaid children must not create implicit rows: {grid:?}"
    );
}

/// Chromium 150 oracle for the full-span nested column-subgrid pattern used by
/// Mozilla's springboard/data rows. The 20px child gap is centered on the
/// ancestor's zero-width grid lines, so descendant contributions inflate the
/// ancestor tracks by [10, 20, 10]. The remaining 240px then stretches the
/// three auto tracks equally: 130px, 180px, 190px.
#[test]
fn nested_full_span_column_subgrid_shares_ancestor_tracks() {
    let tree = parse_html(
        r#"
        <style>
          * { box-sizing:border-box }
          html, body { margin:0 }
          #parent {
            display:grid;
            width:500px;
            grid-template-columns:repeat(3, auto);
          }
          .subgrid, .inner {
            display:grid;
            grid-column:1 / -1;
            grid-template-columns:subgrid;
          }
          .inner > * { grid-area:auto }
          .inner { column-gap:20px }
          .leaf { height:20px }
          .a { width:40px }
          .b { width:80px }
          .c { width:100px }
        </style>
        <div id="parent">
          <div id="row-one" class="subgrid">
            <div id="inner-one" class="inner">
              <div id="a" class="leaf a"></div>
              <div id="b" class="leaf b"></div>
              <div id="c" class="leaf c"></div>
            </div>
          </div>
          <div class="subgrid">
            <div class="inner">
              <div class="leaf a"></div>
              <div class="leaf b"></div>
              <div class="leaf c"></div>
            </div>
          </div>
        </div>
        "#,
    );
    let layout = layout_dom(&tree, (800.0, 600.0));
    let rect = |id| layout.rects[&tree.get_element_by_id(id).unwrap()];
    let parent = rect("parent");
    let row = rect("row-one");
    let inner = rect("inner-one");
    let a = rect("a");
    let b = rect("b");
    let c = rect("c");

    assert!(
        (parent.width - 500.0).abs() < 0.01
            && (parent.height - 40.0).abs() < 0.01
            && (row.width - 500.0).abs() < 0.01
            && (row.height - 20.0).abs() < 0.01
            && (inner.width - 500.0).abs() < 0.01
            && (inner.height - 20.0).abs() < 0.01,
        "nested subgrids must share one row of ancestor tracks: \
         parent={parent:?} row={row:?} inner={inner:?}"
    );
    assert!(
        (a.x - 0.0).abs() < 0.01
            && (a.width - 40.0).abs() < 0.01
            && (b.x - 140.0).abs() < 0.01
            && (b.width - 80.0).abs() < 0.01
            && (c.x - 320.0).abs() < 0.01
            && (c.width - 100.0).abs() < 0.01,
        "Chrome column positions: a={a:?} b={b:?} c={c:?}"
    );

    let repeated = layout_dom(&tree, (800.0, 600.0));
    for id in ["parent", "row-one", "inner-one", "a", "b", "c"] {
        let node = tree.get_element_by_id(id).unwrap();
        assert_eq!(
            layout.rects[&node], repeated.rects[&node],
            "the two-pass reduction must be stable across repeated layouts: {id}"
        );
    }
}

/// Cases outside the bounded reduction must retain the existing fallback,
/// rather than freezing an invented set of ancestor tracks. The narrow parent
/// cannot fit every max-content growth limit, and the second row only spans a
/// subset of its parent columns.
#[test]
fn unsafe_column_subgrid_cases_decline_the_fast_path() {
    let tree = parse_html(
        r#"
        <style>
          * { box-sizing:border-box }
          html, body { margin:0 }
          .parent {
            display:grid;
            grid-template-columns:repeat(3, auto);
          }
          #narrow { width:200px }
          #partial { width:500px }
          #placed { width:500px }
          #named { width:500px }
          .subgrid, .inner {
            display:grid;
            grid-column:1 / -1;
            grid-template-columns:subgrid;
          }
          #partial-row { grid-column:1 / 3 }
          #placed-b { grid-column:2 }
          #named-b { grid-column:missing-line }
          .inner { column-gap:20px }
          .leaf { height:20px }
          .a { width:40px }
          .b { width:80px }
          .c { width:100px }
        </style>
        <div id="narrow" class="parent">
          <div class="subgrid"><div class="inner">
            <div class="leaf a"></div>
            <div id="narrow-b" class="leaf b"></div>
            <div class="leaf c"></div>
          </div></div>
        </div>
        <div id="partial" class="parent">
          <div id="partial-row" class="subgrid"><div class="inner">
            <div class="leaf a"></div>
            <div id="partial-b" class="leaf b"></div>
            <div class="leaf c"></div>
          </div></div>
        </div>
        <div id="placed" class="parent">
          <div class="subgrid"><div class="inner">
            <div class="leaf a"></div>
            <div id="placed-b" class="leaf b"></div>
            <div class="leaf c"></div>
          </div></div>
        </div>
        <div id="named" class="parent">
          <div class="subgrid"><div class="inner">
            <div class="leaf a"></div>
            <div id="named-b" class="leaf b"></div>
            <div class="leaf c"></div>
          </div></div>
        </div>
        "#,
    );
    let layout = layout_dom(&tree, (800.0, 600.0));
    let rect = |id| layout.rects[&tree.get_element_by_id(id).unwrap()];
    let narrow = rect("narrow");
    let narrow_b = rect("narrow-b");
    let partial = rect("partial");
    let partial_b = rect("partial-b");
    let placed = rect("placed");
    let placed_b = rect("placed-b");
    let named = rect("named");
    let named_b = rect("named-b");

    assert!(
        (narrow.height - 60.0).abs() < 0.01 && (narrow_b.x - narrow.x).abs() < 0.01,
        "max-content overflow must retain the stacked fallback: {narrow:?} {narrow_b:?}"
    );
    assert!(
        (partial.height - 60.0).abs() < 0.01 && (partial_b.x - partial.x).abs() < 0.01,
        "partial-span subgrid must retain the stacked fallback: {partial:?} {partial_b:?}"
    );
    assert!(
        (placed.height - 40.0).abs() < 0.01 && placed_b.x > placed.x,
        "an explicitly placed leaf must retain the existing placement fallback: \
         {placed:?} {placed_b:?}"
    );
    assert!(
        (named.height - 60.0).abs() < 0.01 && (named_b.x - named.x).abs() < 0.01,
        "an unresolved named placement must not enter the numeric subgrid reduction: \
         {named:?} {named_b:?}"
    );
}

#[test]
fn empty_block_before_participates_in_normal_flow_geometry() {
    let tree = parse_html(
        r#"
        <style>
          html, body { margin:0 }
          body { font-size:20px; line-height:20px }
          #host { width:200px; background:#eee }
          #host::before {
            content:"";
            display:block;
            width:80px;
            height:40px;
            margin-bottom:10px;
            background:#06c;
          }
          #next { width:20px; height:10px; background:#0a0 }
        </style>
        <div id="host">TEXT</div><div id="next"></div>
        "#,
    );
    let layout = layout_dom(&tree, (800.0, 600.0));
    let host = layout.rects[&tree.get_element_by_id("host").unwrap()];
    let next = layout.rects[&tree.get_element_by_id("next").unwrap()];
    assert!(
        (host.height - 70.0).abs() < 0.01,
        "40px generated block + 10px margin + 20px line must size host: {host:?}"
    );
    assert!(
        (next.y - 70.0).abs() < 0.01,
        "following flow content must start after generated geometry: {next:?}"
    );
}

#[test]
fn empty_inline_block_pseudo_is_an_atomic_flex_item() {
    let tree = parse_html(
        r#"
        <style>
          html, body { margin:0 }
          #host { display:flex; width:100px; height:20px }
          #host::before {
            content:"";
            display:inline-block;
            width:.5em;
            height:.5em;
            margin-right:5px;
            border:2px solid #06c;
          }
          #label { width:20px; height:10px }
        </style>
        <div id="host"><span id="label"></span></div>
        "#,
    );
    let layout = layout_dom(&tree, (800.0, 600.0));
    let label = layout.rects[&tree.get_element_by_id("label").unwrap()];
    assert!(
        (label.x - 17.0).abs() < 0.01,
        "8px pseudo content width + 2px borders + 5px margin must precede label: {label:?}"
    );
}

#[test]
fn light_dark_uses_the_final_inherited_color_scheme_across_the_cascade() {
    let tree = parse_html(
        r#"
        <style>
          :root {
            --surface:light-dark(#f5f5f5,#18191b);
          }
          #dark {
            background-color:var(--surface);
            color:light-dark(#111111,#eeeeee);
            border:1px solid light-dark(#dddddd,#333333);
          }
          #dark {
            color-scheme:dark;
          }
          #dark::before {
            content:"";
            display:block;
            width:10px;
            height:10px;
            background:light-dark(#ff0000,#0000ff);
          }
          #child {
            background-image:linear-gradient(
              light-dark(#ffffff,#101010),
              light-dark(#eeeeee,#202020)
            );
          }
          #icon rect {
            fill:light-dark(#c3c7cb,#51565d);
            stroke:light-dark(#ffffff,#000000);
          }
          #mixed {
            background:light-dark(#abcdef,#123456);
            color-scheme:light dark;
          }
        </style>
        <section id="dark">
          <div id="child"></div>
          <svg id="icon"><rect width="10" height="10"/></svg>
        </section>
        <div id="mixed"></div>
        "#,
    );
    let layout = layout_dom(&tree, (800.0, 600.0));
    let dark = tree.get_element_by_id("dark").unwrap();
    let child = tree.get_element_by_id("child").unwrap();
    let mixed = tree.get_element_by_id("mixed").unwrap();
    let rect = tree.query_selector("#icon rect").unwrap().unwrap();

    let dark_style = &layout.styles[&dark];
    assert!(dark_style.color_scheme_dark);
    assert_eq!(dark_style.background_color, Some([0x18, 0x19, 0x1b, 0xff]));
    assert_eq!(dark_style.color, Some([0xee, 0xee, 0xee, 0xff]));
    assert_eq!(dark_style.border_color, Some([0x33, 0x33, 0x33, 0xff]));
    assert_eq!(
        dark_style
            .before_pseudo
            .as_deref()
            .and_then(|pseudo| pseudo.background_color),
        Some([0x00, 0x00, 0xff, 0xff]),
        "pseudo must use its host's inherited dark scheme"
    );

    let child_style = &layout.styles[&child];
    assert!(child_style.color_scheme_dark);
    let (_, stops) = child_style
        .background_gradient
        .as_ref()
        .expect("dark gradient");
    assert_eq!(stops[0].0, [0x10, 0x10, 0x10, 0xff]);
    assert_eq!(stops[1].0, [0x20, 0x20, 0x20, 0xff]);

    let rect_style = &layout.styles[&rect];
    assert!(rect_style.color_scheme_dark);
    assert_eq!(rect_style.svg_fill.as_deref(), Some("#51565dff"));
    assert_eq!(rect_style.svg_stroke.as_deref(), Some("#000000ff"));

    let mixed_style = &layout.styles[&mixed];
    assert!(
        !mixed_style.color_scheme_dark,
        "a scheme list admitting light uses the current light preference"
    );
    assert_eq!(mixed_style.background_color, Some([0xab, 0xcd, 0xef, 0xff]));
}

/// Chromium 150 maps width/height from the selected <picture><source> onto
/// the associated image as presentation hints. The selected 800/400 source
/// therefore reserves a 500x250 box before it loads; below the media
/// breakpoint, the fallback img's 600/600 ratio reserves 500x500.
#[test]
fn selected_picture_source_dimensions_override_fallback_image_ratio() {
    let tree = parse_html(
        r#"
        <style>
          html, body { margin:0 }
          img { display:block; width:500px; height:auto }
        </style>
        <picture>
          <source
            type="image/AVIF; codecs=av01"
            media="(min-width:800px)"
            srcset="unsupported.avif"
            width="2000"
            height="100">
          <source
            type=" IMAGE/PNG ; profile=srgb "
            media="(min-width:800px)"
            srcset="chosen.png"
            width="800"
            height="400">
          <img id="image" src="fallback.png" width="600" height="600">
        </picture>
        "#,
    );
    let image = tree.get_element_by_id("image").unwrap();

    let wide = layout_dom(&tree, (1000.0, 600.0));
    let wide_rect = wide.rects[&image];
    assert!((wide_rect.width - 500.0).abs() < 0.01, "{wide_rect:?}");
    assert!((wide_rect.height - 250.0).abs() < 0.01, "{wide_rect:?}");
    assert_eq!(wide.styles[&image].aspect_ratio, Some(2.0));

    let narrow = layout_dom(&tree, (700.0, 600.0));
    let narrow_rect = narrow.rects[&image];
    assert!((narrow_rect.width - 500.0).abs() < 0.01, "{narrow_rect:?}");
    assert!((narrow_rect.height - 500.0).abs() < 0.01, "{narrow_rect:?}");
    assert_eq!(narrow.styles[&image].aspect_ratio, Some(1.0));
}

#[test]
fn responsive_escaped_utility_classes_apply_logical_padding() {
    let tree = parse_html(
        r#"
        <style>
          :root { --spacing: .25rem }
          *, ::before, ::after { box-sizing: border-box }
          @media (min-width: 40rem) {
            .sm\:px-2 { padding-inline: calc(var(--spacing) * 2) }
            .sm\:py-4 { padding-block: calc(var(--spacing) * 4) }
          }
        </style>
        <a id="cell" class="sm:px-2 sm:py-4"
           style="display:block;width:270px">
          <span id="child" style="display:block;height:20px"></span>
        </a>
        "#,
    );
    let cell_id = tree.get_element_by_id("cell").unwrap();
    assert_eq!(
        tree.query_selector(r".sm\:px-2").unwrap(),
        Some(cell_id),
        "the escaped utility selector must decode to the literal class token"
    );
    let layout = layout_dom(&tree, (1280.0, 900.0));
    let cell = layout.rects[&cell_id];
    let child = layout.rects[&tree.get_element_by_id("child").unwrap()];

    assert!((cell.width - 270.0).abs() < 0.01, "{cell:?}");
    assert!((cell.height - 52.0).abs() < 0.01, "{cell:?}");
    assert!((child.x - cell.x - 8.0).abs() < 0.01, "{child:?}");
    assert!((child.y - cell.y - 16.0).abs() < 0.01, "{child:?}");
}

/// HTML width/height attributes are provisional presentational hints. Once
/// the image is decoded, its natural dimensions replace that mapped ratio.
#[test]
fn decoded_image_ratio_replaces_mapped_html_ratio() {
    let tree = parse_html(
        r#"
        <style>
          html, body { margin:0 }
          img { display:block; width:500px; height:auto }
        </style>
        <img id="image" src="image.png" width="600" height="600">
        "#,
    );
    let image = tree.get_element_by_id("image").unwrap();
    let intrinsic = HashMap::from([(image, (800.0, 400.0))]);
    let layout = layout_dom_with_images(&tree, (1000.0, 600.0), &intrinsic);
    let rect = layout.rects[&image];

    assert!((rect.width - 500.0).abs() < 0.01, "{rect:?}");
    assert!((rect.height - 250.0).abs() < 0.01, "{rect:?}");
    assert_eq!(layout.styles[&image].aspect_ratio, Some(2.0));
}

/// Unlike HTML presentation hints, an authored `aspect-ratio` declaration
/// remains authoritative after the resource's natural dimensions are known.
#[test]
fn authored_aspect_ratio_wins_over_decoded_image_ratio() {
    let tree = parse_html(
        r#"
        <style>
          html, body { margin:0 }
          img {
            display:block;
            width:500px;
            height:auto;
            aspect-ratio:4 / 1;
          }
        </style>
        <img id="image" src="image.png" width="600" height="600">
        "#,
    );
    let image = tree.get_element_by_id("image").unwrap();
    let intrinsic = HashMap::from([(image, (800.0, 400.0))]);
    let layout = layout_dom_with_images(&tree, (1000.0, 600.0), &intrinsic);
    let rect = layout.rects[&image];

    assert!((rect.width - 500.0).abs() < 0.01, "{rect:?}");
    assert!((rect.height - 125.0).abs() < 0.01, "{rect:?}");
    assert_eq!(layout.styles[&image].aspect_ratio, Some(4.0));
}

#[test]
fn both_auto_replaced_max_height_preserves_intrinsic_ratio() {
    let tree = parse_html(
        r#"
        <style>
          html, body { margin:0 }
          img { display:block; max-height:128px }
        </style>
        <img id="image" src="image.png">
        "#,
    );
    let image = tree.get_element_by_id("image").unwrap();
    let intrinsic = HashMap::from([(image, (512.0, 323.0))]);
    let layout = layout_dom_with_images(&tree, (1000.0, 600.0), &intrinsic);
    let rect = layout.rects[&image];

    // The layout result is pixel-snapped; the exact pre-snap result is
    // independently covered by the inline constraint-table unit test.
    assert_eq!((rect.width, rect.height), (203.0, 128.0), "{rect:?}");
}

#[test]
fn height_sized_oversized_image_keeps_intrinsic_ratio_without_an_author_cap() {
    let tree = parse_html(
        r#"
        <style>
          html, body { margin:0 }
          #wrapper {
            position:relative;
            width:1280px;
            height:692px;
            overflow:hidden;
          }
          #image {
            position:absolute;
            left:50%;
            bottom:0;
            width:auto;
            height:100%;
            transform:translateX(-50%);
          }
        </style>
        <div id="wrapper"><img id="image" src="image.jpg"></div>
        "#,
    );
    let image = tree.get_element_by_id("image").unwrap();
    let intrinsic = HashMap::from([(image, (3008.0, 692.0))]);
    let layout = layout_dom_with_images(&tree, (1280.0, 1400.0), &intrinsic);
    let rect = layout.rects[&image];

    // Layout stores the pre-transform border box at the authored 50% inset;
    // paint/CSSOM applies translateX(-50%) and exposes the final x=-864.
    assert!((rect.x - 640.0).abs() < 0.01, "{rect:?}");
    assert!((rect.width - 3008.0).abs() < 0.01, "{rect:?}");
    assert!((rect.height - 692.0).abs() < 0.01, "{rect:?}");
}

#[test]
fn authored_percentage_max_width_still_clamps_an_oversized_image() {
    let tree = parse_html(
        r#"
        <style>
          html, body { margin:0 }
          #wrapper { width:1280px }
          #image { display:block; width:auto; height:auto; max-width:100% }
        </style>
        <div id="wrapper"><img id="image" src="image.jpg"></div>
        "#,
    );
    let image = tree.get_element_by_id("image").unwrap();
    let intrinsic = HashMap::from([(image, (3008.0, 692.0))]);
    let layout = layout_dom_with_images(&tree, (1280.0, 1400.0), &intrinsic);
    let rect = layout.rects[&image];

    assert!((rect.width - 1280.0).abs() < 0.01, "{rect:?}");
    assert!(
        (rect.height - (1280.0 * 692.0 / 3008.0)).abs() < 1.0,
        "{rect:?}"
    );
}

/// Chromium 150 applies CSS Sizing's compressible replaced-element rule when
/// `max-width:100%` is cyclic during intrinsic sizing. Each image contributes
/// zero to its column's automatic minimum, so the row can flex-shrink the
/// natural 303/200/500/200/293px bases into exactly 1184px including gaps.
#[test]
fn percentage_max_replaced_items_shrink_in_a_flex_row() {
    let tree = parse_html(
        r#"
        <style>
          html, body { margin:0 }
          #row { display:flex; gap:8px; width:1184px; overflow:hidden }
          .col { display:flex; flex-direction:column; gap:8px }
          img { display:block; max-width:100%; height:auto }
        </style>
        <div id="row">
          <div id="c1" class="col"><img id="i1" width="303" height="255"></div>
          <div id="c2" class="col"><img id="i2" width="200" height="400"></div>
          <div id="c3" class="col"><img id="i3" width="500" height="250"></div>
          <div id="c4" class="col"><img id="i4" width="200" height="400"></div>
          <div id="c5" class="col"><img id="i5" width="293" height="354"></div>
        </div>
        "#,
    );
    let ids = ["i1", "i2", "i3", "i4", "i5"]
        .map(|id| tree.get_element_by_id(id).unwrap());
    let intrinsic = HashMap::from([
        (ids[0], (303.0, 255.0)),
        (ids[1], (200.0, 400.0)),
        (ids[2], (500.0, 250.0)),
        (ids[3], (200.0, 400.0)),
        (ids[4], (293.0, 354.0)),
    ]);
    let layout = layout_dom_with_images(&tree, (1280.0, 1000.0), &intrinsic);
    let row = layout.rects[&tree.get_element_by_id("row").unwrap()];
    let columns = ["c1", "c2", "c3", "c4", "c5"]
        .map(|id| layout.rects[&tree.get_element_by_id(id).unwrap()]);
    let images = ids.map(|id| layout.rects[&id]);
    let expected_widths: [f32; 5] =
        [233.328125, 154.0, 385.03125, 154.015625, 225.625];

    assert!((row.width - 1184.0).abs() < 0.01, "{row:?}");
    assert!(
        (row.height - 308.03125).abs() < 0.05,
        "row={row:?}, columns={columns:?}, images={images:?}"
    );
    for (index, expected) in expected_widths.into_iter().enumerate() {
        // Taffy exposes device-pixel-snapped border boxes; retain Chromium's
        // fractional flex result above as the oracle, then compare its snapped
        // width at this one-CSS-pixel device scale.
        let expected = expected.round();
        assert!(
            (columns[index].width - expected).abs() < 0.01,
            "column {index}: {:?}",
            columns[index]
        );
        assert!(
            (images[index].width - expected).abs() < 0.01,
            "image {index}: {:?}",
            images[index]
        );
    }
    assert!(
        ((images[0].width / images[0].height) - (303.0 / 255.0)).abs() < 0.001,
        "final layout must preserve the first intrinsic ratio: {:?}",
        images[0]
    );
    assert!(
        ((images[4].width / images[4].height) - (293.0 / 354.0)).abs() < 0.001,
        "final layout must preserve the last intrinsic ratio: {:?}",
        images[4]
    );
}

/// Functional percentage-only maximums have the same computed sizing type as
/// a bare percentage. Framework and utility stylesheets emit both spellings,
/// so the replaced-element intrinsic contribution cannot depend on syntax.
#[test]
fn functional_percentage_max_replaced_items_shrink_in_a_flex_row() {
    let tree = parse_html(
        r#"
        <style>
          html, body { margin:0 }
          #row { display:flex; gap:8px; width:1184px; overflow:hidden }
          .col { display:flex; flex-direction:column; gap:8px }
          img { display:block; max-width:calc(100%); height:auto }
        </style>
        <div id="row">
          <div id="c1" class="col"><img id="i1" width="303" height="255"></div>
          <div id="c2" class="col"><img id="i2" width="200" height="400"></div>
          <div id="c3" class="col"><img id="i3" width="500" height="250"></div>
          <div id="c4" class="col"><img id="i4" width="200" height="400"></div>
          <div id="c5" class="col"><img id="i5" width="293" height="354"></div>
        </div>
        "#,
    );
    let ids = ["i1", "i2", "i3", "i4", "i5"]
        .map(|id| tree.get_element_by_id(id).unwrap());
    let intrinsic = HashMap::from([
        (ids[0], (303.0, 255.0)),
        (ids[1], (200.0, 400.0)),
        (ids[2], (500.0, 250.0)),
        (ids[3], (200.0, 400.0)),
        (ids[4], (293.0, 354.0)),
    ]);
    let layout = layout_dom_with_images(&tree, (1280.0, 1000.0), &intrinsic);
    let columns = ["c1", "c2", "c3", "c4", "c5"]
        .map(|id| layout.rects[&tree.get_element_by_id(id).unwrap()]);
    let images = ids.map(|id| layout.rects[&id]);
    let expected_widths: [f32; 5] =
        [233.328125, 154.0, 385.03125, 154.015625, 225.625];

    for (index, expected) in expected_widths.into_iter().enumerate() {
        let expected = expected.round();
        assert_eq!(columns[index].width, expected, "column {index}: {:?}", columns[index]);
        assert_eq!(images[index].width, expected, "image {index}: {:?}", images[index]);
    }
}

/// The compressible-percentage exception is specific to proper replaced
/// sizing. A non-replaced descendant with `max-width:100%` keeps its ordinary
/// min-content contribution while its flex item is intrinsically sized; its
/// fixed child must therefore continue to impose a 400px automatic minimum.
#[test]
fn percentage_max_non_replaced_descendant_keeps_its_intrinsic_minimum() {
    let tree = parse_html(
        r#"
        <style>
          html, body { margin:0 }
          #row { display:flex; width:300px; overflow:hidden }
          #column { display:flex; flex-direction:column }
          #bounded { max-width:100% }
          #floor { width:400px; height:10px }
          #side { flex:0 0 100px; height:10px }
        </style>
        <div id="row">
          <div id="column"><div id="bounded"><div id="floor"></div></div></div>
          <div id="side"></div>
        </div>
        "#,
    );
    let layout = layout_dom(&tree, (800.0, 400.0));
    let rect = |id| layout.rects[&tree.get_element_by_id(id).unwrap()];

    assert_eq!(rect("row").width, 300.0);
    assert_eq!(rect("column").width, 400.0);
    assert_eq!(rect("bounded").width, 400.0);
    assert_eq!(rect("floor").width, 400.0);
    assert_eq!(rect("side").width, 100.0);
}

#[test]
fn cyclic_descendant_percentages_do_not_inflate_a_flex_items_intrinsic_minimum() {
    let tree = parse_html(
        r#"
        <style>
          html, body { margin:0 }
          #row { display:flex; width:1376px; gap:100px }
          #left { width:100%; max-width:343px }
          #left-floor { width:343px; height:20px }
          #middle { display:flex; width:100%; gap:16px }
          #middle > div { width:calc(33.333% - 16px); height:20px }
          #right { flex:0 0 136px; height:20px }
        </style>
        <div id="row">
          <div id="left"><div id="left-floor"></div></div>
          <div id="middle"><div id="m1"></div><div id="m2"></div><div id="m3"></div></div>
          <div id="right"></div>
        </div>
        "#,
    );
    let layout = layout_dom(&tree, (1600.0, 600.0));
    let rect = |id| layout.rects[&tree.get_element_by_id(id).unwrap()];
    let row = rect("row");
    let left = rect("left");
    let middle = rect("middle");
    let right = rect("right");
    let middle_children = [rect("m1"), rect("m2"), rect("m3")];

    assert!((row.width - 1376.0).abs() < 0.01, "{row:?}");
    assert!((left.width - 343.0).abs() < 0.01, "{left:?}");
    assert!(
        (middle.width - 697.0).abs() < 0.01,
        "cyclic descendant percentages must not become a 1376px intrinsic floor: \
         row={row:?} left={left:?} middle={middle:?} right={right:?}"
    );
    assert!((right.width - 136.0).abs() < 0.01, "{right:?}");
    for child in middle_children {
        assert!(
            (216.0..=217.0).contains(&child.width),
            "the final reflow must resolve calc() against the 697px flex item: {child:?}"
        );
    }
}

#[test]
fn cyclic_percentage_flex_row_keeps_its_intrinsic_content_width() {
    let tree = parse_html(
        r#"
        <style>
          html, body { margin:0 }
          #scroller { display:flex; width:768px }
          #month { flex:0 0 auto; width:auto; max-width:336px; padding:0 24px }
          #week { display:flex; width:100% }
          .day { flex:1 0 48px; width:48px; height:48px }
        </style>
        <div id="scroller"><div id="month"><div><div id="week">
          <div class="day"></div><div class="day"></div><div class="day"></div>
          <div class="day"></div><div class="day"></div><div class="day"></div>
          <div class="day"></div>
        </div></div></div></div>
        "#,
    );
    let layout = layout_dom(&tree, (1000.0, 400.0));
    let rect = |id| layout.rects[&tree.get_element_by_id(id).unwrap()];

    assert_eq!(rect("week").width, 336.0, "percentage row: {:?}", rect("week"));
    assert_eq!(
        rect("month").width,
        384.0,
        "content width plus inline padding: {:?}",
        rect("month")
    );
}

/// Chromium resolves both spellings to the same 163px content width: the
/// percentage is cyclic while the link's flex-item width is being measured,
/// then resolves against that link's final width. Carbon Ads uses the bare
/// spelling; treating its 500px image as a permanent intrinsic floor widened
/// Bootstrap's 195px sidebar card to 532px.
#[test]
fn bare_and_functional_cyclic_image_percentages_use_the_final_flex_width() {
    let tree = parse_html(
        r#"
        <style>
          * { box-sizing:border-box }
          html, body { margin:0 }
          .stack { display:flex; flex-flow:column wrap; width:195px }
          .wrap { display:flex; flex-wrap:wrap; padding:16px }
          .link { display:block; position:relative; flex:1 0 100%; overflow:hidden }
          .image { display:block; height:auto }
          #bare-image { width:100% }
          #functional-image { width:calc(100%) }
        </style>
        <div class="stack"><div id="bare-wrap" class="wrap">
          <a id="bare-link" class="link">
            <img id="bare-image" class="image" width="100" height="100">
          </a>
        </div></div>
        <div class="stack"><div id="functional-wrap" class="wrap">
          <a id="functional-link" class="link">
            <img id="functional-image" class="image" width="100" height="100">
          </a>
        </div></div>
        "#,
    );
    let bare_image = tree.get_element_by_id("bare-image").unwrap();
    let functional_image = tree.get_element_by_id("functional-image").unwrap();
    let intrinsic = HashMap::from([
        (bare_image, (500.0, 500.0)),
        (functional_image, (500.0, 500.0)),
    ]);
    let layout = layout_dom_with_images(&tree, (1280.0, 720.0), &intrinsic);
    let rect = |id| layout.rects[&tree.get_element_by_id(id).unwrap()];

    for (prefix, wrap_id, link_id, image_id) in [
        ("bare", "bare-wrap", "bare-link", "bare-image"),
        (
            "functional",
            "functional-wrap",
            "functional-link",
            "functional-image",
        ),
    ] {
        let wrap = rect(wrap_id);
        let link = rect(link_id);
        let image = rect(image_id);
        assert_eq!(wrap.width, 195.0, "{prefix} percentage wrapper: {wrap:?}");
        assert_eq!(link.width, 163.0, "{prefix} percentage link: {link:?}");
        assert_eq!(image.width, 163.0, "{prefix} percentage image: {image:?}");
        assert_eq!(image.height, 163.0, "{prefix} image ratio: {image:?}");
    }
}

/// The cyclic percentage affects only the min-content contribution. With
/// positive free space, the same boxes retain their natural max-content bases.
#[test]
fn percentage_max_replaced_items_keep_natural_max_content_size() {
    let tree = parse_html(
        r#"
        <style>
          html, body { margin:0 }
          #row { display:flex; gap:8px; width:2000px }
          .col { display:flex; flex-direction:column }
          img { display:block; max-width:100%; height:auto }
        </style>
        <div id="row">
          <div id="c1" class="col"><img id="i1" width="303" height="255"></div>
          <div id="c2" class="col"><img id="i2" width="500" height="250"></div>
        </div>
        "#,
    );
    let first = tree.get_element_by_id("i1").unwrap();
    let second = tree.get_element_by_id("i2").unwrap();
    let intrinsic = HashMap::from([
        (first, (303.0, 255.0)),
        (second, (500.0, 250.0)),
    ]);
    let layout = layout_dom_with_images(&tree, (2200.0, 1000.0), &intrinsic);
    let first = layout.rects[&first];
    let second = layout.rects[&second];

    assert!(
        (first.width - 303.0).abs() < 0.01
            && (first.height - 255.0).abs() < 0.01,
        "{first:?}"
    );
    assert!(
        (second.width - 500.0).abs() < 0.01
            && (second.height - 250.0).abs() < 0.01,
        "{second:?}"
    );
}

/// Tailwind's `tracking-tighter` is not a paint-only transform: it changes
/// shaped advances before line breaking. At this exact Chromium oracle width,
/// the heading is two 40px lines with -1.8px tracking and three lines at
/// `normal`. This guards the general shaping path, not a framework selector.
#[test]
fn negative_letter_spacing_changes_intrinsic_line_breaks() {
    let tree = parse_html(
        r#"
        <style>
          html, body { margin:0 }
          :root { --tracking-tighter:-.05em }
          .heading {
            box-sizing:border-box;
            width:512px;
            font-size:36px;
            line-height:40px;
            font-weight:500;
            padding-inline:8px;
          }
          #tight { letter-spacing:var(--tracking-tighter) }
          #reset { letter-spacing:normal }
        </style>
        <section style="letter-spacing:-2px">
          <div id="tight" class="heading">Rapidly build modern websites without ever leaving your HTML.</div>
          <div id="reset" class="heading">Rapidly build modern websites without ever leaving your HTML.</div>
        </section>
        "#,
    );
    let layout = layout_dom(&tree, (512.0, 400.0));
    let tight_id = tree.get_element_by_id("tight").unwrap();
    let reset_id = tree.get_element_by_id("reset").unwrap();
    let tight = layout.rects[&tight_id];
    let reset = layout.rects[&reset_id];

    assert_eq!(layout.styles[&tight_id].letter_spacing, Some(-1.8000001));
    assert_eq!(layout.styles[&reset_id].letter_spacing, Some(0.0));
    assert_eq!(tight.height, 80.0, "negative tracking must shape to two lines: {tight:?}");
    assert_eq!(reset.height, 120.0, "`normal` must reset inherited tracking: {reset:?}");
}

#[test]
fn text_indent_is_inherited_and_initial_resets_to_zero() {
    let tree = parse_html(
        r#"
        <style>
          #parent { font-size:20px; text-indent:2em }
          #initial { text-indent:initial }
          #unset { text-indent:unset }
        </style>
        <section id="parent">
          <p id="inherited">Inherited first line</p>
          <p id="initial">Reset first line</p>
          <p id="unset">Unset inherits</p>
        </section>
        "#,
    );
    let layout = layout_dom(&tree, (500.0, 300.0));
    let style = |id| &layout.styles[&tree.get_element_by_id(id).unwrap()];

    assert_eq!(
        style("parent").text_indent,
        Some(obscura_render::Dimension::Px(40.0))
    );
    assert_eq!(
        style("inherited").text_indent,
        Some(obscura_render::Dimension::Px(40.0))
    );
    assert_eq!(
        style("initial").text_indent,
        Some(obscura_render::Dimension::Px(0.0))
    );
    assert_eq!(
        style("unset").text_indent,
        Some(obscura_render::Dimension::Px(40.0))
    );
}

/// Modern utility frameworks emit logical insets for full-width fixed bars
/// (`inset-x-0` in Tailwind is `inset-inline:0`). Both inline edges constrain
/// an auto-width positioned box, so it must stretch across the viewport.
#[test]
fn logical_inline_insets_stretch_a_fixed_auto_width_box() {
    let tree = parse_html(
        r#"
        <style>
          html, body { margin:0 }
          #bar {
            position:fixed;
            inset-inline:0;
            inset-block-start:0;
            height:56px;
          }
        </style>
        <div id="bar"><span>Brand</span><nav>Links</nav></div>
        "#,
    );
    let layout = layout_dom(&tree, (1440.0, 1000.0));
    let bar = layout.rects[&tree.get_element_by_id("bar").unwrap()];

    assert_eq!(bar.x, 0.0, "{bar:?}");
    assert_eq!(bar.y, 0.0, "{bar:?}");
    assert_eq!(bar.width, 1440.0, "{bar:?}");
    assert_eq!(bar.height, 56.0, "{bar:?}");
}

#[test]
fn flex_automatic_minimum_uses_overflow_on_the_matching_axis() {
    let tree = parse_html(
        r#"
        <style>
          html, body { margin:0 }
          .row { display:flex; width:40px }
          .item { flex-shrink:1 }
          .wide { width:100px; height:10px }
          #block-clipped { overflow-y:clip }
          #inline-clipped { overflow-x:hidden }
        </style>
        <div class="row"><div id="block-clipped" class="item"><div class="wide"></div></div></div>
        <div class="row"><div id="inline-clipped" class="item"><div class="wide"></div></div></div>
        "#,
    );
    let layout = layout_dom(&tree, (200.0, 100.0));
    let rect = |id| layout.rects[&tree.get_element_by_id(id).unwrap()];

    assert_eq!(
        rect("block-clipped").width,
        100.0,
        "block-axis overflow must not remove the inline automatic minimum"
    );
    assert_eq!(
        rect("inline-clipped").width,
        40.0,
        "inline-axis non-visible overflow allows the flex item to shrink"
    );
}

fn tailwind_line_counter_fixture() -> obscura_dom::tree::DomTree {
    parse_html(
        r#"
        <style>
          html, body { margin:0; background:white }
          code { display:block; position:relative; width:120px; counter-reset:line }
          .line {
            display:block;
            height:20px;
            margin-left:24px;
            counter-increment:line;
          }
          .line::before {
            content:counter(line);
            display:inline-block;
            position:absolute;
            left:0;
            width:16px;
            color:black;
            text-align:right;
          }
        </style>
        <code>
          <span id="line-one" class="line">alpha</span>
          <span id="line-two" class="line">beta</span>
          <span id="line-three" class="line">gamma</span>
        </code>
        "#,
    )
}

#[test]
fn css_counters_resolve_generated_line_numbers_in_tree_order() {
    let tree = tailwind_line_counter_fixture();
    let layout = layout_dom(&tree, (120.0, 80.0));
    for (id, expected) in [("line-one", "1"), ("line-two", "2"), ("line-three", "3")] {
        let style = &layout.styles[&tree.get_element_by_id(id).unwrap()];
        assert_eq!(
            style
                .before_pseudo
                .as_ref()
                .and_then(|pseudo| pseudo.before_content.as_deref()),
            Some(expected),
            "{id}"
        );
    }
}

#[test]
fn nested_counter_scopes_feed_counters_function_and_expire_at_parent() {
    let tree = parse_html(
        r#"
        <style>
          ol { counter-reset:item; list-style:none }
          li { counter-increment:item }
          li::before { content:counters(item, ".") " " }
        </style>
        <ol>
          <li id="outer-one">one
            <ol><li id="inner-one">nested</li></ol>
          </li>
          <li id="outer-two">two</li>
        </ol>
        "#,
    );
    let layout = layout_dom(&tree, (400.0, 200.0));
    let generated = |id| {
        layout.styles[&tree.get_element_by_id(id).unwrap()]
            .before_content
            .clone()
    };

    assert_eq!(generated("outer-one").as_deref(), Some("1 "));
    assert_eq!(generated("inner-one").as_deref(), Some("1.1 "));
    assert_eq!(generated("outer-two").as_deref(), Some("2 "));
}

#[cfg(feature = "paint")]
#[test]
fn generated_counter_text_reaches_the_positioned_pseudo_paint_path() {
    let tree = tailwind_line_counter_fixture();
    let pixmap =
        obscura_render::paint_dom(&tree, (120.0, 80.0), None).expect("counter fixture paints");
    for (line, y_range) in [(1, 0..20), (2, 20..40), (3, 40..60)] {
        let dark_gutter_pixels = y_range
            .flat_map(|y| (0..20).map(move |x| (x, y)))
            .filter(|&(x, y)| {
                let pixel = pixmap.pixel(x, y).expect("inside pixmap");
                pixel.red() < 100 && pixel.green() < 100 && pixel.blue() < 100
            })
            .count();
        assert!(
            dark_gutter_pixels > 2,
            "generated line {line} should paint at its own static block-axis position"
        );
    }
}

#[cfg(feature = "paint")]
#[test]
fn positioned_counter_inside_nested_inline_code_uses_the_code_gutter() {
    let tree = parse_html(
        r#"
        <style>
          html, body { margin:0; background:white }
          pre {
            display:flex;
            margin:0;
            padding:12px;
            width:220px;
            background:black;
            white-space:pre;
          }
          code.outer { display:block; width:100% }
          code.inner { display:inline }
          .with-line-numbers code { counter-reset:line; position:relative }
          .line {
            display:block;
            isolation:isolate;
            height:24px;
            margin-left:36px;
            counter-increment:line;
            color:white;
            font:13px/24px monospace;
          }
          .line::before {
            content:counter(line);
            width:24px;
            text-align:right;
            color:red;
            display:inline-block;
            position:absolute;
            left:0;
            font-family:monospace;
          }
        </style>
        <div class="with-line-numbers">
          <pre><code class="outer"><code class="inner"><span class="line"><span style="color:pink">&lt;div</span><span>&gt;</span></span>
            <span class="line"><span style="color:pink">&lt;span</span><span>&gt;text&lt;/span&gt;</span></span>
            <span class="line"><span style="color:pink">&lt;/div</span><span>&gt;</span></span></code></code></pre>
        </div>
        "#,
    );
    let pixmap = obscura_render::paint_dom(&tree, (260.0, 100.0), None)
        .expect("nested Tailwind-shaped code fixture paints");
    let red_ink = |x_range: std::ops::Range<u32>| {
        (0..72)
            .flat_map(|y| x_range.clone().map(move |x| (x, y)))
            .filter(|&(x, y)| {
                let pixel = pixmap.pixel(x, y).expect("inside pixmap");
                pixel.red() > 160 && pixel.green() < 100 && pixel.blue() < 100
            })
            .count()
    };
    assert!(
        red_ink(12..36) > 4,
        "line counters must paint in the positioned code gutter"
    );
    assert_eq!(
        red_ink(45..72),
        0,
        "line counters must not overpaint the margin-shifted source text"
    );
}

#[cfg(feature = "paint")]
#[test]
fn positioned_generated_text_honors_text_alignment_inside_its_box() {
    let tree = parse_html(
        r#"
        <style>
          html, body { margin:0; background:white }
          .row {
            position:relative;
            width:40px;
            height:24px;
          }
          .row::before {
            content:counter(line);
            position:absolute;
            inset:0 auto auto 0;
            width:24px;
            height:20px;
            color:black;
            font:16px/20px monospace;
          }
          .one { counter-reset:line 1 }
          .ten { counter-reset:line 10 }
          .right::before { text-align:right }
          .center::before { text-align:center }
          .left::before { text-align:left }
        </style>
        <div class="row right one"></div>
        <div class="row right ten"></div>
        <div class="row center one"></div>
        <div class="row center ten"></div>
        <div class="row left one"></div>
        <div class="row left ten"></div>
        "#,
    );
    let pixmap =
        obscura_render::paint_dom(&tree, (80.0, 144.0), None).expect("aligned counters paint");
    let ink_bounds = |row: u32| {
        let mut min_x = u32::MAX;
        let mut max_x = 0;
        for y in row * 24..row * 24 + 20 {
            for x in 0..24 {
                let pixel = pixmap.pixel(x, y).expect("inside pixmap");
                if pixel.red() < 100 && pixel.green() < 100 && pixel.blue() < 100 {
                    min_x = min_x.min(x);
                    max_x = max_x.max(x);
                }
            }
        }
        assert_ne!(min_x, u32::MAX, "row {row} should contain generated text");
        (min_x, max_x)
    };

    let right_one = ink_bounds(0);
    let right_ten = ink_bounds(1);
    let center_one = ink_bounds(2);
    let center_ten = ink_bounds(3);
    let left_one = ink_bounds(4);
    let left_ten = ink_bounds(5);

    assert!(
        right_one.1.abs_diff(right_ten.1) <= 1,
        "right-aligned 1 and 10 should share an end edge: {right_one:?} {right_ten:?}"
    );
    assert!(
        left_one.0.abs_diff(left_ten.0) <= 1,
        "left-aligned 1 and 10 should share a start edge: {left_one:?} {left_ten:?}"
    );
    assert!(
        (center_one.0 + center_one.1).abs_diff(center_ten.0 + center_ten.1) <= 2,
        "center-aligned 1 and 10 should share a center: {center_one:?} {center_ten:?}"
    );
    assert!(
        left_one.0 < center_one.0 && center_one.0 < right_one.0,
        "single-digit ink should move across the pseudo box: \
         left={left_one:?} center={center_one:?} right={right_one:?}"
    );
}

/// A percentage block size on an in-flow child resolves against the
/// containing block's definite content box. Floating the containing block
/// changes its outer inline participation, not that block-axis percentage
/// basis.
#[test]
fn percentage_height_inside_floated_and_inline_block_controls_uses_content_box() {
    let tree = parse_html(
        r#"
        <style>
          html, body { margin:0 }
          #row { width:400px; height:44px }
          .control {
            box-sizing:border-box;
            width:50%;
            height:100%;
            padding-block:1px;
          }
          #floated { float:left }
          #inline { display:inline-block }
          .icon {
            display:inline-block;
            width:24px;
            height:100%;
          }
        </style>
        <div id="row">
          <button id="floated" class="control"><span id="float-icon" class="icon"></span></button><button id="inline" class="control"><span id="inline-icon" class="icon"></span></button>
        </div>
        "#,
    );
    let layout = layout_dom(&tree, (400.0, 200.0));
    let rect = |id| layout.rects[&tree.get_element_by_id(id).unwrap()];

    assert_eq!(rect("floated").height, 44.0);
    assert_eq!(rect("inline").height, 44.0);
    assert_eq!(
        rect("float-icon").height,
        42.0,
        "a floated containing block retains its definite content-box basis"
    );
    assert_eq!(
        rect("inline-icon").height,
        42.0,
        "border-box padding must not leak into the child percentage basis"
    );
}

/// A percentage height computes to auto when its containing block's height is
/// content-dependent. The resulting post-layout height must not be recycled as
/// a new percentage basis on a repair pass.
#[test]
fn percentage_height_chain_under_auto_height_atomic_parent_stays_indefinite() {
    let tree = parse_html(
        r#"
        <style>
          html, body { margin:0 }
          .control { height:100%; padding-block:2px }
          #floated { float:left }
          #inline { display:inline-block }
          .icon { display:inline-block; height:100% }
          .content { height:13px }
        </style>
        <section><div id="floated" class="control"><div id="float-icon" class="icon"><div class="content"></div></div></div></section>
        <section><div id="inline" class="control"><div id="inline-icon" class="icon"><div class="content"></div></div></div></section>
        "#,
    );
    let layout = layout_dom(&tree, (400.0, 200.0));
    let id = |name| tree.get_element_by_id(name).unwrap();

    for name in ["floated", "inline", "float-icon", "inline-icon"] {
        assert_eq!(
            layout.styles[&id(name)].height,
            obscura_render::Dimension::Auto,
            "{name} must retain an indefinite block-axis percentage basis"
        );
    }
    assert_eq!(layout.rects[&id("float-icon")].height, 13.0);
    assert_eq!(layout.rects[&id("inline-icon")].height, 13.0);
}

#[test]
fn fit_content_width_shrink_wraps_a_grid_item_instead_of_stretching() {
    let tree = parse_html(
        r#"
        <body style="margin:0">
          <div style="display:grid;width:500px">
            <a id="cta" style="display:block;width:fit-content;padding:8px 12px">Launch Brave</a>
          </div>
        </body>
        "#,
    );
    let layout = layout_dom(&tree, (800.0, 200.0));
    let id = tree.get_element_by_id("cta").unwrap();
    let rect = layout.rects[&id];

    assert!(
        layout.styles[&id].width_fit_content,
        "the intrinsic keyword must survive the computed-style translation"
    );
    assert!(
        rect.width > 80.0 && rect.width < 200.0,
        "a short CTA should shrink-wrap within its grid area, not stretch to 500px: {rect:?}"
    );
}

#[test]
fn fit_content_width_uses_available_space_and_wraps_breakable_text() {
    let tree = parse_html(
        r#"
        <body style="margin:0">
          <div style="width:140px">
            <div id="fit" style="width:fit-content;font-size:16px;line-height:20px">several breakable words need wrapping here</div>
          </div>
        </body>
        "#,
    );
    let layout = layout_dom(&tree, (500.0, 200.0));
    let rect = layout.rects[&tree.get_element_by_id("fit").unwrap()];

    assert!(
        (rect.width - 140.0).abs() < 0.1,
        "fit-content should clamp max-content to the containing block: {rect:?}"
    );
    assert!(
        rect.height >= 40.0,
        "the clamped fit-content box must reflow its breakable text: {rect:?}"
    );
}

#[test]
fn fit_content_width_keeps_an_unbreakable_min_content_floor() {
    let tree = parse_html(
        r#"
        <body style="margin:0">
          <div style="width:60px">
            <div id="fit" style="width:fit-content;font-size:16px">supercalifragilisticexpialidocious</div>
          </div>
        </body>
        "#,
    );
    let layout = layout_dom(&tree, (500.0, 200.0));
    let rect = layout.rects[&tree.get_element_by_id("fit").unwrap()];

    assert!(
        rect.width > 60.0,
        "min-content must win when an unbreakable token is wider than the available space: {rect:?}"
    );
}

#[test]
fn closed_details_lays_out_only_its_first_direct_summary() {
    let tree = parse_html(
        r#"
        <body style="margin:0">
          <details id="closed" style="width:200px">
            source text before the summary
            <div id="before" style="height:30px">
              <summary id="nested">nested summary is not the details summary</summary>
            </div>
            <summary id="first" style="height:20px">visible summary</summary>
            <div id="content" style="height:50px">
              <span id="descendant">hidden descendant</span>
            </div>
            <summary id="second" style="height:40px">later summary is content</summary>
          </details>
        </body>
        "#,
    );
    let layout = layout_dom(&tree, (400.0, 200.0));
    let id = |name| tree.get_element_by_id(name).unwrap();

    assert!(layout.rects.contains_key(&id("closed")));
    assert!(layout.rects.contains_key(&id("first")));
    for hidden in ["before", "nested", "content", "descendant", "second"] {
        assert!(
            !layout.rects.contains_key(&id(hidden)),
            "closed details content must not generate a box: {hidden}"
        );
    }
    assert!(
        (layout.rects[&id("closed")].height - 20.0).abs() < 0.1,
        "only the first direct summary contributes to closed details geometry"
    );
}

#[test]
fn open_details_lays_out_all_of_its_content() {
    let tree = parse_html(
        r#"
        <body style="margin:0">
          <details id="open-details" open style="width:200px">
            <summary id="summary" style="height:20px">visible summary</summary>
            <div id="content" style="height:50px">
              <span id="descendant" style="display:block;height:10px">visible descendant</span>
            </div>
            <summary id="second" style="height:40px">later summary is ordinary content</summary>
          </details>
        </body>
        "#,
    );
    let layout = layout_dom(&tree, (400.0, 200.0));
    let id = |name| tree.get_element_by_id(name).unwrap();

    for visible in [
        "open-details",
        "summary",
        "content",
        "descendant",
        "second",
    ] {
        assert!(
            layout.rects.contains_key(&id(visible)),
            "open details must retain its rendered content: {visible}"
        );
    }
    assert!(
        (layout.rects[&id("open-details")].height - 110.0).abs() < 0.1,
        "all direct children contribute to open details geometry"
    );
}

#[test]
fn display_inherit_transitively_preserves_contents_and_block_display() {
    let tree = parse_html(
        r#"
        <style>
          html, body { margin:0 }
          #row { display:flex; width:200px }
          #contents { display:contents }
          .through { display:inherit }
          .item { display:block; width:40px; height:20px }
          #block-parent { display:block }
          #block-child { display:inherit; height:10px }
        </style>
        <div id="row">
          <div id="contents">
            <div id="inherit-one" class="through">
              <div id="inherit-two" class="through">
                <span id="first" class="item"></span>
                <span id="second" class="item"></span>
              </div>
            </div>
          </div>
        </div>
        <section id="block-parent"><span id="block-child"></span></section>
        <table><tr><td id="native-cell">
          <div id="native-inherit" style="min-width:50px;display:inherit"></div>
          <div id="native-inherit-reversed" style="display:inherit;min-width:50px"></div>
        </td></tr></table>
        "#,
    );
    let layout = layout_dom(&tree, (400.0, 200.0));
    let id = |name| tree.get_element_by_id(name).unwrap();

    for wrapper in ["contents", "inherit-one", "inherit-two"] {
        let wrapper = id(wrapper);
        assert!(layout.styles[&wrapper].display_contents);
        assert!(
            !layout.rects.contains_key(&wrapper),
            "inherited display:contents wrapper must not generate a box"
        );
    }
    let first = layout.rects[&id("first")];
    let second = layout.rects[&id("second")];
    assert!((first.y - second.y).abs() < 0.1);
    assert!((second.x - first.x - first.width).abs() < 0.1);

    let block_child = &layout.styles[&id("block-child")];
    assert_eq!(block_child.display, obscura_render::Display::Block);
    assert!(!block_child.display_contents);

    let native_cell = &layout.styles[&id("native-cell")];
    let native_inherit = &layout.styles[&id("native-inherit")];
    let native_inherit_reversed = &layout.styles[&id("native-inherit-reversed")];
    assert!(native_cell.internal_flex_container);
    assert_eq!(native_inherit.display, obscura_render::Display::Flex);
    assert!(
        native_inherit.internal_flex_container,
        "inherited table-cell display must reconstruct its internal cell-content wrapper"
    );
    assert_eq!(native_inherit.min_width, obscura_render::Dimension::Px(50.0));
    assert!(native_inherit_reversed.internal_flex_container);
    assert_eq!(
        native_inherit_reversed.min_width,
        obscura_render::Dimension::Px(50.0)
    );
}

#[cfg(feature = "paint")]
#[test]
fn closed_details_content_does_not_paint() {
    let tree = parse_html(
        r#"
        <style>
          html, body { margin:0; background:white }
          summary { display:block; width:40px; height:20px; background:#00ff00 }
          .content { width:40px; height:20px; background:#ff0000 }
        </style>
        <details>
          <summary></summary>
          <div class="content"></div>
        </details>
        "#,
    );
    let pixmap =
        obscura_render::paint_dom(&tree, (40.0, 40.0), None).expect("details fixture paints");
    let summary = pixmap.pixel(5, 5).expect("summary pixel");
    assert!(
        summary.green() > 240 && summary.red() < 20,
        "the first summary must paint"
    );
    let hidden = pixmap.pixel(5, 25).expect("closed content pixel");
    assert!(
        hidden.red() > 240 && hidden.green() > 240 && hidden.blue() > 240,
        "closed details content must leave the page background untouched"
    );
}

#[test]
fn grid_replaced_normal_and_explicit_stretch_match_browser_geometry() {
    let tree = parse_html(
        r#"
        <style>
          html, body { margin:0 }
          .grid { display:grid; grid-template:200px / 300px; width:300px; height:200px }
        </style>
        <div class="grid"><img id="normal"></div>
        <div class="grid"><img id="min-inline" style="min-width:50%"></div>
        <div class="grid"><img id="min-block" style="min-height:50%"></div>
        <div class="grid"><img id="normal-inline" style="justify-self:normal;min-width:50%"></div>
        <div class="grid"><img id="start" style="justify-self:start;min-width:50%"></div>
        <div class="grid"><img id="auto-margin" style="min-width:50%;margin-left:auto"></div>
        <div class="grid"><img id="stretch-inline" style="justify-self:stretch"></div>
        <div class="grid"><img id="stretch-block" style="align-self:stretch"></div>
        <div class="grid"><img id="stretch-both" style="place-self:stretch"></div>
        <div class="grid"><img id="definite-inline" style="width:120px;align-self:stretch"></div>
        <div class="grid"><img id="definite-block" style="height:80px;justify-self:stretch"></div>
        <div class="grid" style="justify-items:stretch"><img id="parent-stretch"></div>
        <div class="grid" style="justify-items:normal"><img id="parent-normal" style="min-width:50%"></div>
        <div class="grid"><img id="intrinsic-border-box" style="box-sizing:border-box;width:120px;padding:10px;border:5px solid"></div>
        <div class="grid"><img id="intrinsic-content-box" style="box-sizing:content-box;width:120px;padding:10px;border:5px solid"></div>
        <div class="grid"><img id="authored-border-box" style="box-sizing:border-box;width:120px;padding:10px;border:5px solid;aspect-ratio:1"></div>
        <div class="grid"><img id="authored-content-box" style="box-sizing:content-box;width:120px;padding:10px;border:5px solid;aspect-ratio:1"></div>
        <div class="grid"><img id="conflicting-min-inline" style="min-width:250px;max-height:80px"></div>
        <div class="grid"><img id="conflicting-min-block" style="max-width:120px;min-height:100px"></div>
        "#,
    );
    let ids = [
        "normal",
        "min-inline",
        "min-block",
        "normal-inline",
        "start",
        "auto-margin",
        "stretch-inline",
        "stretch-block",
        "stretch-both",
        "definite-inline",
        "definite-block",
        "parent-stretch",
        "parent-normal",
        "intrinsic-border-box",
        "intrinsic-content-box",
        "authored-border-box",
        "authored-content-box",
        "conflicting-min-inline",
        "conflicting-min-block",
    ];
    let intrinsic: HashMap<_, _> = ids
        .iter()
        .map(|name| (tree.get_element_by_id(name).unwrap(), (100.0, 50.0)))
        .collect();
    let layout = layout_dom_with_images(&tree, (800.0, 3000.0), &intrinsic);
    let rect = |name| layout.rects[&tree.get_element_by_id(name).unwrap()];
    let size = |name| {
        let rect = rect(name);
        (rect.width, rect.height)
    };

    assert_eq!(size("normal"), (100.0, 50.0));
    assert_eq!(size("min-inline"), (150.0, 75.0));
    assert_eq!(size("min-block"), (200.0, 100.0));
    assert_eq!(size("normal-inline"), (150.0, 75.0));
    assert_eq!(size("start"), (150.0, 75.0));
    assert_eq!(size("auto-margin"), (150.0, 75.0));
    assert_eq!(size("stretch-inline"), (300.0, 150.0));
    assert_eq!(size("stretch-block"), (400.0, 200.0));
    assert_eq!(size("stretch-both"), (300.0, 200.0));
    assert_eq!(size("definite-inline"), (120.0, 200.0));
    assert_eq!(size("definite-block"), (300.0, 80.0));
    assert_eq!(size("parent-stretch"), (300.0, 150.0));
    assert_eq!(size("parent-normal"), (150.0, 75.0));
    assert_eq!(size("intrinsic-border-box"), (120.0, 75.0));
    assert_eq!(size("intrinsic-content-box"), (150.0, 90.0));
    assert_eq!(size("authored-border-box"), (120.0, 120.0));
    assert_eq!(size("authored-content-box"), (150.0, 150.0));
    assert_eq!(size("conflicting-min-inline"), (250.0, 80.0));
    assert_eq!(size("conflicting-min-block"), (120.0, 100.0));

    let item = rect("auto-margin");
    let parent = tree
        .get_node(tree.get_element_by_id("auto-margin").unwrap())
        .and_then(|node| node.parent)
        .unwrap();
    assert_eq!(item.x - layout.rects[&parent].x, 150.0);
}

#[test]
fn nested_grid_input_contributes_intrinsic_height_without_preventing_stretch() {
    let tree = parse_html(
        r#"
        <style>
          html, body { margin:0; font:14px Arial; line-height:22px }
          .outer { display:grid; width:214px; align-items:center }
          .editor { display:inline-grid; grid-area:1 / 1 / 2 / 3;
                    grid-template-columns:0 min-content }
          input { display:block; width:100%; grid-area:1 / 2;
                  font:inherit; min-width:2px; padding:0; border:0 }
          .short { grid-template-rows:12px }
        </style>
        <div class="outer"><div class="editor" id="auto-editor"><input id="auto-input"></div></div>
        <div class="outer"><div class="editor short" id="short-editor"><input id="short-input"></div></div>
        "#,
    );
    let layout = layout_dom(&tree, (800.0, 600.0));
    for (input, editor, height) in [
        ("auto-input", "auto-editor", 22.0),
        ("short-input", "short-editor", 12.0),
    ] {
        let input_rect = layout.rects[&tree.get_element_by_id(input).unwrap()];
        let editor_rect = layout.rects[&tree.get_element_by_id(editor).unwrap()];
        assert_eq!(input_rect.height, height, "{input}");
        assert_eq!(editor_rect.height, height, "{editor}");
        assert!(input_rect.width >= 2.0, "{input}");
    }
}

#[test]
fn grid_replaced_classification_keeps_controls_stretched_and_media_natural() {
    let tree = parse_html(
        r#"
        <style>
          html, body { margin:0 }
          .grid { display:grid; grid-template:300px / 400px; width:400px; height:300px }
        </style>
        <div class="grid"><svg id="svg" viewBox="0 0 100 50"></svg></div>
        <div class="grid"><svg id="svg-border-box" viewBox="0 0 100 50" style="box-sizing:border-box;width:120px;padding:10px;border:5px solid"></svg></div>
        <div class="grid"><img id="mapped-border-box" width="100" height="50" style="box-sizing:border-box;width:120px;height:auto;padding:10px;border:5px solid"></div>
        <div class="grid"><canvas id="canvas"></canvas></div>
        <div class="grid"><video id="video"></video></div>
        <div class="grid"><audio id="audio"></audio></div>
        <div class="grid"><audio id="audio-controls" controls></audio></div>
        <div class="grid"><iframe id="iframe"></iframe></div>
        <div class="grid"><embed id="embed" src="about:blank"></embed></div>
        <div class="grid"><embed id="empty-embed"></embed></div>
        <div class="grid"><object id="object"></object></div>
        <div class="grid"><progress id="progress"></progress></div>
        <div class="grid"><meter id="meter"></meter></div>
        <div class="grid"><input id="image-input" type="image" width="100" height="50"></div>
        <div class="grid"><button id="button">Hi</button></div>
        <div class="grid" id="auto-button-grid"><button id="auto-button" style="margin-left:auto">Hi</button></div>
        <div class="grid"><input id="text-input"></div>
        <div class="grid"><select id="select"><option>Hi</option></select></div>
        "#,
    );
    let layout = layout_dom(&tree, (800.0, 5000.0));
    let size = |name| {
        let rect = layout.rects[&tree.get_element_by_id(name).unwrap()];
        (rect.width, rect.height)
    };

    assert_eq!(size("svg"), (400.0, 200.0));
    assert_eq!(size("svg-border-box"), (120.0, 75.0));
    assert_eq!(size("mapped-border-box"), (120.0, 75.0));
    for name in ["canvas", "video", "embed", "object"] {
        assert_eq!(size(name), (300.0, 150.0), "{name}");
    }
    assert_eq!(size("iframe"), (304.0, 154.0));
    assert_eq!(size("empty-embed"), (0.0, 0.0));
    assert_eq!(size("audio"), (0.0, 0.0));
    assert_eq!(size("audio-controls"), (300.0, 54.0));
    assert_eq!(size("progress"), (160.0, 16.0));
    assert_eq!(size("meter"), (80.0, 16.0));
    assert_eq!(size("image-input"), (100.0, 50.0));
    assert_eq!(
        layout.styles[&tree.get_element_by_id("button").unwrap()].width,
        obscura_render::Dimension::Auto,
        "grid-normal stretch must preserve the button's auto inline size"
    );
    for name in ["button", "text-input", "select"] {
        assert_eq!(size(name), (400.0, 300.0), "{name}");
    }
    let auto_button = layout.rects[&tree.get_element_by_id("auto-button").unwrap()];
    let auto_button_grid = layout.rects[&tree.get_element_by_id("auto-button-grid").unwrap()];
    assert!(auto_button.width > 20.0 && auto_button.width < 30.0);
    assert_eq!(auto_button.height, 300.0);
    assert_eq!(auto_button.x + auto_button.width, auto_button_grid.x + auto_button_grid.width);
}

#[test]
fn inline_block_percentage_height_uses_definite_block_content_height() {
    let tree = parse_html(r#"<!doctype html>
        <style>
          html, body { margin:0 }
          .switch { position:relative; width:32px; height:16px; line-height:1 }
          input { position:absolute; width:100%; height:100%; margin:0; opacity:0; z-index:-1000 }
          label { display:inline-block; box-sizing:border-box; width:100%; height:100%; border:1px solid }
          .padded { height:30px; box-sizing:border-box; padding:4px; border:1px solid }
          .indefinite { height:auto; min-height:40px }
        </style>
        <div class="switch"><input type="checkbox"><label id="percentage"></label></div>
        <div class="switch"><input type="checkbox"><label id="pixels" style="height:16px"></label></div>
        <div class="switch padded"><label id="content-box"></label></div>
        <div class="switch padded"><label id="half" style="height:50%"></label></div>
        <div class="switch indefinite"><label id="indefinite"></label></div>
    "#);
    let layout = layout_dom(&tree, (800.0, 600.0));
    for (name, width, height) in [
        ("percentage", 32.0, 16.0), ("pixels", 32.0, 16.0),
        ("content-box", 22.0, 20.0), ("half", 22.0, 10.0), ("indefinite", 32.0, 2.0),
    ] {
        let rect = layout.rects[&tree.get_element_by_id(name).unwrap()];
        assert_eq!(rect.width, width, "{name}");
        assert_eq!(rect.height, height, "{name}");
    }
}

#[test]
fn inherited_font_shorthand_overrides_native_control_typography() {
    let tree = parse_html(r#"
        <style>
          body { font:700 20px/30px Arial }
          input { display:block; border:0; padding:0 }
        </style>
        <input id="inherit" style="font:inherit">
        <input id="unset" style="font:unset">
    "#);
    let layout = layout_dom(&tree, (800.0, 600.0));
    for name in ["inherit", "unset"] {
        let node = tree.get_element_by_id(name).unwrap();
        let style = &layout.styles[&node];
        assert_eq!(style.font_size, Some(20.0), "{name}");
        assert_eq!(style.line_height, Some(obscura_render::LineHeight::Px(30.0)), "{name}");
        assert_eq!(style.font_weight.as_deref(), Some("700"), "{name}");
        assert_eq!(style.font_family.as_deref(), Some("arial"), "{name}");
        assert_eq!(layout.rects[&node].height, 30.0, "{name}");
    }
}

#[test]
fn grid_ordinary_aspect_ratio_preserves_normal_alignment_provenance() {
    let tree = parse_html(
        r#"
        <style>
          html, body { margin:0 }
          .grid { display:grid; grid-template:200px / 300px; width:300px; height:200px }
          .item { aspect-ratio:2 }
        </style>
        <div class="grid"><div class="item" id="normal"></div></div>
        <div class="grid"><div class="item" id="align-stretch" style="align-self:stretch"></div></div>
        <div class="grid"><div class="item" id="justify-stretch" style="justify-self:stretch"></div></div>
        <div class="grid"><div class="item" id="both-stretch" style="place-self:stretch"></div></div>
        <div class="grid"><div class="item" id="both-normal" style="place-self:normal"></div></div>
        <div class="grid"><div class="item" id="align-start" style="align-self:start"></div></div>
        <div class="grid"><div class="item" id="justify-start" style="justify-self:start"></div></div>
        <div class="grid" style="align-items:stretch"><div class="item" id="parent-align-stretch"></div></div>
        <div class="grid" style="justify-items:stretch"><div class="item" id="parent-justify-stretch"></div></div>
        "#,
    );
    let layout = layout_dom(&tree, (800.0, 2000.0));
    let size = |name| {
        let rect = layout.rects[&tree.get_element_by_id(name).unwrap()];
        (rect.width, rect.height)
    };

    assert_eq!(size("normal"), (300.0, 150.0));
    assert_eq!(size("align-stretch"), (400.0, 200.0));
    assert_eq!(size("justify-stretch"), (300.0, 150.0));
    assert_eq!(size("both-stretch"), (300.0, 200.0));
    assert_eq!(size("both-normal"), (300.0, 150.0));
    assert_eq!(size("align-start"), (300.0, 150.0));
    assert_eq!(size("justify-start"), (400.0, 200.0));
    assert_eq!(size("parent-align-stretch"), (400.0, 200.0));
    assert_eq!(size("parent-justify-stretch"), (300.0, 150.0));
}

#[test]
fn deeply_nested_wrappers_cascade_without_stack_overflow() {
    // The style cascade used to recurse once per DOM level with a fat frame,
    // so a page-controlled tree a few dozen levels deep overflowed the native
    // stack and aborted the process. The traversal is iterative now; depth
    // must change neither survival nor rule matching.
    let open: String = (0..400).map(|_| "<span>").collect::<Vec<_>>().join("");
    let close = "</span>".repeat(400);
    let tree = parse_html(&format!(
        r#"<style>
          html, body {{ margin:0; font:16px/18px monospace }}
          div span #deep {{ display:block; width:50px }}
        </style>
        <div>{open}<span id="deep">deep</span>{close}<input type="checkbox"></div>"#
    ));
    let layout = layout_dom(&tree, (400.0, 200.0));
    let rect = layout.rects[&tree.get_element_by_id("deep").unwrap()];
    // A descendant-combinator match at depth 400: the matcher's ancestor
    // filter is maintained across the whole iterative walk.
    assert!(
        (rect.width - 50.0).abs() < 0.01,
        "the descendant rule must match at depth 400, got {rect:?}"
    );
}

#[test]
fn float_ending_at_rounded_segment_boundary_does_not_panic() {
    use taffy::{compute::FloatContext, Clear, FloatDirection, Size};

    fn place(
        context: &mut FloatContext,
        height: f32,
        direction: FloatDirection,
    ) -> taffy::Point<f32> {
        context.place_floated_box(
            Size { width: 100.0, height },
            1859.0,
            [0.0, 0.0],
            direction,
            Clear::None,
        )
    }

    let mut context = FloatContext::new();
    context.set_width(1000.0);
    place(&mut context, 421.3333, FloatDirection::Left);
    place(&mut context, 400.3333, FloatDirection::Right);

    assert_eq!(place(&mut context, 400.3333, FloatDirection::Left).y, 1859.0);
}

#[cfg(feature = "paint")]
#[test]
fn perspective_computes_cascaded_lengths_and_establishes_containing_blocks() {
    let tree = parse_html(r#"<style>
        html { font-size:10px } body { margin:0 }
        #camera { --depth:calc(2rem + 3em); font-size:20px; perspective:var(--depth) !important;
            margin:30px; width:100px; height:80px }
        #invalid { perspective:40px; perspective:-1px; perspective:20%; perspective:banana }
        #inherit { perspective:inherit } #unset { perspective:unset }
        </style><div id="default"></div><div id="camera" style="perspective:7px">
        <div id="inherit"></div><div id="unset"></div>
        <div id="badge" style="position:fixed;left:4px;top:6px;width:10px;height:10px"></div>
        </div><div id="invalid"></div>"#);
    let mut resources = obscura_render::RenderResourceCache::with_loader(|_: &str| None);
    let prepared = obscura_render::prepare_dom(&tree, (400.0, 300.0), None, &mut resources).unwrap();
    let computed = |name| prepared.computed_style(tree.get_element_by_id(name).unwrap()).unwrap();
    assert_eq!(computed("default")["perspective"], "none");
    assert_eq!(computed("camera")["perspective"], "80px");
    assert_eq!(computed("inherit")["perspective"], "80px");
    assert_eq!(computed("unset")["perspective"], "none");
    assert_eq!(computed("invalid")["perspective"], "40px");
    let layout = prepared.layout();
    let camera = layout.rects[&tree.get_element_by_id("camera").unwrap()];
    let badge = layout.rects[&tree.get_element_by_id("badge").unwrap()];
    assert_eq!((badge.x - camera.x, badge.y - camera.y), (4.0, 6.0));
    assert!(obscura_render::style::supports_declaration("perspective", "80px"));
}

#[cfg(feature = "paint")]
#[test]
fn perspective_projects_pixels_cssom_bounds_and_pointer_plane_together() {
    let tree = parse_html(r#"<!doctype html><style>
        html,body { margin:0; background:white }
        #stage { position:absolute;left:100px;top:80px;width:200px;height:200px;
            perspective:400px;perspective-origin:0 0 }
        #plane { width:100px;height:100px;background:red;transform:translateZ(200px) }
        </style><div id="stage"><div id="plane"></div></div>"#);
    let mut resources = obscura_render::RenderResourceCache::with_loader(|_: &str| None);
    let mut prepared = obscura_render::prepare_dom(&tree, (500.0, 400.0), None, &mut resources).unwrap();
    let plane = tree.get_element_by_id("plane").unwrap();
    let rect = prepared.document_rect(plane).unwrap();
    assert!((rect.x - 100.0).abs() < 0.01 && (rect.y - 80.0).abs() < 0.01, "{rect:?}");
    assert!((rect.width - 200.0).abs() < 0.01 && (rect.height - 200.0).abs() < 0.01, "{rect:?}");
    let scroll = prepared.resolve_scroll_state(&tree, (0.0, 0.0), &HashMap::new());
    assert_eq!(prepared.hit_test(&tree, &scroll, 250.0, 100.0), Some(plane));
    let local = prepared.viewport_to_local(&tree, plane, &scroll, 250.0, 100.0).unwrap();
    assert!((local.0 - 75.0).abs() < 0.01 && (local.1 - 10.0).abs() < 0.01);
    assert_eq!(prepared.local_to_viewport(&tree, plane, &scroll, 100.0, 100.0), Some((300.0, 280.0)));
    let image = obscura_render::paint_prepared(&tree, &mut prepared, &mut resources, (0.0, 0.0)).unwrap();
    let pixel = image.pixel(250, 100).unwrap();
    assert_eq!((pixel.red(), pixel.green(), pixel.blue()), (255, 0, 0));
    let pixel = image.pixel(310, 100).unwrap();
    assert_eq!((pixel.red(), pixel.green(), pixel.blue()), (255, 255, 255));
}

/// A perspective camera is projective in depth, but its children at z = 0 are
/// not projected: Chromium paints them with their authored geometry, without
/// resampling the layer. A composited camera layer used to shrink an
/// absolutely positioned badge by one pixel on every edge.
#[cfg(feature = "paint")]
#[test]
fn flat_descendants_of_a_perspective_camera_paint_without_resampling() {
    let tree = parse_html(r#"<!doctype html><style>
        html,body { margin:0; background:white }
        #stage { position:absolute;left:20px;top:20px;width:100px;height:80px;
            perspective:1000px }
        #badge { position:absolute;right:5px;bottom:5px;width:20px;height:20px;
            background:rgb(25,113,194) }
        </style><div id="stage"><span id="badge"></span></div>"#);
    let mut resources = obscura_render::RenderResourceCache::with_loader(|_: &str| None);
    let mut prepared = obscura_render::prepare_dom(&tree, (300.0, 200.0), None, &mut resources).unwrap();
    let badge = tree.get_element_by_id("badge").unwrap();
    let rect = prepared.document_rect(badge).unwrap();
    assert!((rect.x - 95.0).abs() < 0.01 && (rect.y - 75.0).abs() < 0.01, "{rect:?}");
    let image = obscura_render::paint_prepared(&tree, &mut prepared, &mut resources, (0.0, 0.0)).unwrap();
    for (x, y) in [(95, 75), (114, 75), (95, 94), (114, 94)] {
        let pixel = image.pixel(x, y).unwrap();
        assert_eq!(
            (pixel.red(), pixel.green(), pixel.blue()),
            (25, 113, 194),
            "flat perspective child must paint its authored rectangle at {x},{y}"
        );
    }
}

#[cfg(feature = "paint")]
#[test]
fn projective_own_translation_is_not_dropped_by_the_flat_fast_path() {
    let tree = parse_html(r#"<!doctype html><style>
        html,body { margin:0; background:white }
        #stage { position:absolute;left:10px;top:20px;width:200px;height:60px;
            perspective:1000px }
        #moved { position:absolute;left:20px;top:10px;width:10px;height:10px;
            background:rgb(25,113,194); transform-origin:0 0;
            transform:perspective(1000px) translateX(50px) }
        </style><div id="stage"><div id="moved"></div></div>"#);
    let mut resources = obscura_render::RenderResourceCache::with_loader(|_: &str| None);
    let mut prepared = obscura_render::prepare_dom(&tree, (300.0, 150.0), None, &mut resources).unwrap();
    let moved = tree.get_element_by_id("moved").unwrap();
    let rect = prepared.document_rect(moved).unwrap();
    assert!(
        (rect.x - 80.0).abs() < 0.01 && (rect.y - 30.0).abs() < 0.01,
        "the projective translate must reach CSSOM geometry: {rect:?}"
    );
    let image = obscura_render::paint_prepared(&tree, &mut prepared, &mut resources, (0.0, 0.0)).unwrap();
    let shifted = image.pixel(85, 35).unwrap();
    assert_eq!(
        (shifted.red(), shifted.green(), shifted.blue()),
        (25, 113, 194),
        "an own perspective+translateX transform must paint at the translated position"
    );
    let unshifted = image.pixel(35, 35).unwrap();
    assert_eq!(
        (unshifted.red(), unshifted.green(), unshifted.blue()),
        (255, 255, 255),
        "the fast path must not paint at the pre-translation origin"
    );
}

#[cfg(feature = "paint")]
#[test]
fn preserve_3d_sorts_crossing_planes_per_pixel_and_hit_point() {
    for (red_transform, blue_transform, colors, left_color, right_color) in [
        ("rotateY(45deg)", "rotateY(-45deg)", ["red", "blue"], (255,0,0), (0,0,255)),
        ("rotateY(45deg)", "rotateY(-45deg)", ["rgba(255,0,0,.5)", "rgba(0,0,255,.5)"], (191,63,127), (127,63,191)),
        ("translateZ(50px)", "translateZ(50px)", ["red", "blue"], (255,0,0), (255,0,0)),
    ] {
        let tree = parse_html(&format!(r#"<!doctype html><style>
            html,body {{ margin:0; background:white }}
            #stage {{ position:absolute;left:100px;top:100px;width:200px;height:200px;
                perspective:400px;transform-style:preserve-3d }}
            .plane {{ position:absolute;left:50px;top:50px;width:100px;height:100px }}
            #red {{ background:{};transform:{red_transform};z-index:2 }}
            #blue {{ background:{};transform:{blue_transform};z-index:1 }}
            </style><div id="stage"><div class="plane" id="red"></div><div class="plane" id="blue"></div></div>"#, colors[0], colors[1]));
        let mut resources = obscura_render::RenderResourceCache::with_loader(|_: &str| None);
        let mut prepared = obscura_render::prepare_dom(&tree, (400.0, 400.0), None, &mut resources).unwrap();
        let scroll = prepared.resolve_scroll_state(&tree, (0.0, 0.0), &HashMap::new());
        assert_eq!(prepared.hit_test(&tree, &scroll, 170.0, 200.0), tree.get_element_by_id("red"));
        assert_eq!(prepared.hit_test(&tree, &scroll, 230.0, 200.0), tree.get_element_by_id(if red_transform == blue_transform { "red" } else { "blue" }));
        let image = obscura_render::paint_prepared(&tree, &mut prepared, &mut resources, (0.0, 0.0)).unwrap();
        let left = image.pixel(170, 200).unwrap();
        let right = image.pixel(230, 200).unwrap();
        assert_eq!((left.red(), left.green(), left.blue()), left_color, "{red_transform}, {}", colors[0]);
        assert_eq!((right.red(), right.green(), right.blue()), right_color, "{blue_transform}, {}", colors[1]);
    }
}

#[cfg(feature = "paint")]
#[test]
fn perspective_flat_boundary_discards_descendant_depth_but_preserved_boundary_keeps_it() {
    for (mode, width) in [("flat", 400.0 / 3.0), ("preserve-3d", 200.0)] {
        let tree = parse_html(&format!(r#"<!doctype html><style>
            html,body {{ margin:0; background:white }}
            #stage {{ width:200px;height:200px;perspective:400px;perspective-origin:0 0;transform-style:preserve-3d }}
            #wrapper {{ width:100px;height:100px;transform:translateZ(100px);transform-style:{mode} }}
            #child {{ width:100px;height:100px;background:red;transform:translateZ(100px) }}
            </style><div id="stage"><div id="wrapper"><div id="child"></div></div></div>"#));
        let mut resources = obscura_render::RenderResourceCache::with_loader(|_: &str| None);
        let mut prepared = obscura_render::prepare_dom(&tree, (300.0, 250.0), None, &mut resources).unwrap();
        let child = tree.get_element_by_id("child").unwrap();
        assert!((prepared.document_rect(child).unwrap().width - width).abs() < 0.02, "{mode}");
        let scroll = prepared.resolve_scroll_state(&tree, (0.0, 0.0), &HashMap::new());
        assert_eq!(prepared.hit_test(&tree, &scroll, width - 5.0, 20.0), Some(child));
        assert_ne!(prepared.hit_test(&tree, &scroll, width + 5.0, 20.0), Some(child));
        let image = obscura_render::paint_prepared(&tree, &mut prepared, &mut resources, (0.0, 0.0)).unwrap();
        let inside = image.pixel((width - 5.0) as u32, 20).unwrap();
        let outside = image.pixel((width + 5.0) as u32, 20).unwrap();
        assert_eq!((inside.red(), inside.green(), inside.blue()), (255, 0, 0), "{mode}");
        assert_eq!((outside.red(), outside.green(), outside.blue()), (255, 255, 255), "{mode}");
    }
}

#[cfg(feature = "paint")]
#[test]
fn rotate_y_origin_projects_exact_corners_not_bounding_box_corners() {
    let tree = parse_html(r#"<!doctype html><style>html,body{margin:0;background:white}
        #plane{width:100px;height:100px;background:red;transform-origin:0 0;
            transform:perspective(400px) rotateY(60deg)}
        </style><div id="plane"></div>"#);
    let mut resources = obscura_render::RenderResourceCache::with_loader(|_: &str| None);
    let mut prepared = obscura_render::prepare_dom(&tree, (200.0, 200.0), None, &mut resources).unwrap();
    let plane = tree.get_element_by_id("plane").unwrap();
    let scroll = prepared.resolve_scroll_state(&tree, (0.0, 0.0), &HashMap::new());
    let corner = prepared.local_to_viewport(&tree, plane, &scroll, 100.0, 100.0).unwrap();
    assert!((corner.0 - 41.1013).abs() < 0.02 && (corner.1 - 82.2026).abs() < 0.02, "{corner:?}");
    assert_eq!(prepared.hit_test(&tree, &scroll, 20.0, 30.0), Some(plane));
    assert_ne!(prepared.hit_test(&tree, &scroll, 35.0, 95.0), Some(plane));
    let image = obscura_render::paint_prepared(&tree, &mut prepared, &mut resources, (0.0, 0.0)).unwrap();
    let inside = image.pixel(20, 30).unwrap();
    let outside = image.pixel(35, 95).unwrap();
    assert_eq!((inside.red(), inside.green(), inside.blue()), (255, 0, 0));
    assert_eq!((outside.red(), outside.green(), outside.blue()), (255, 255, 255));
}

#[cfg(feature = "paint")]
#[test]
fn camera_clipped_translucent_plane_paints_without_backface_or_singular_leaks() {
    for (transform, hidden) in [
        ("perspective(100px) rotateY(-60deg)", false),
        ("rotateY(180deg)", true),
        ("scaleX(0)", true),
        ("scaleZ(0)", true),
        ("perspective(100px) translateZ(100px)", true),
        ("perspective(100px) translateZ(101px)", true),
    ] {
        let tree = parse_html(&format!(r#"<!doctype html><style>html,body{{margin:0;background:white}}
            #plane{{width:200px;height:100px;background:rgba(255,0,0,.5);transform-origin:0 0;
                backface-visibility:hidden;transform:{transform}}}
            </style><div id="plane"></div>"#));
        let mut resources = obscura_render::RenderResourceCache::with_loader(|_: &str| None);
        let mut prepared = obscura_render::prepare_dom(&tree, (300.0, 200.0), None, &mut resources).unwrap();
        let plane = tree.get_element_by_id("plane").unwrap();
        let scroll = prepared.resolve_scroll_state(&tree, (0.0, 0.0), &HashMap::new());
        assert_eq!(prepared.viewport_to_local(&tree, plane, &scroll, 50.0, 20.0).is_none(), hidden, "{transform}");
        assert_eq!(prepared.hit_test(&tree, &scroll, 50.0, 20.0) == Some(plane), !hidden, "{transform}");
        assert_eq!(prepared.local_to_viewport(&tree, plane, &scroll, 10.0, 10.0).is_none(), hidden, "{transform}");
        let image = obscura_render::paint_prepared(&tree, &mut prepared, &mut resources, (0.0, 0.0)).unwrap();
        let pixel = image.pixel(50, 20).unwrap();
        assert_eq!((pixel.red(), pixel.green(), pixel.blue()), if hidden { (255,255,255) } else { (255,127,127) }, "{transform}");
    }
}

#[cfg(feature = "paint")]
#[test]
fn untransformed_geometry_and_paint_keep_the_identity_path() {
    let tree = parse_html(r#"<!doctype html><style>html,body{margin:0;background:white}
        #plane{position:absolute;left:30px;top:40px;width:100px;height:80px;background:red}
        </style><div id="plane"></div>"#);
    let mut resources = obscura_render::RenderResourceCache::with_loader(|_: &str| None);
    let mut prepared = obscura_render::prepare_dom(&tree, (200.0, 200.0), None, &mut resources).unwrap();
    assert!(prepared.layout().transforms.is_empty());
    let plane = tree.get_element_by_id("plane").unwrap();
    let scroll = prepared.resolve_scroll_state(&tree, (0.0, 0.0), &HashMap::new());
    assert_eq!(prepared.local_to_viewport(&tree, plane, &scroll, 10.0, 15.0), Some((40.0, 55.0)));
    assert_eq!(prepared.viewport_to_local(&tree, plane, &scroll, 40.0, 55.0), Some((10.0, 15.0)));
    assert_eq!(prepared.hit_test(&tree, &scroll, 40.0, 55.0), Some(plane));
    assert_ne!(prepared.hit_test(&tree, &scroll, 140.0, 55.0), Some(plane));
    let image = obscura_render::paint_prepared(&tree, &mut prepared, &mut resources, (0.0, 0.0)).unwrap();
    let inside = image.pixel(40, 55).unwrap();
    let outside = image.pixel(140, 55).unwrap();
    assert_eq!((inside.red(), inside.green(), inside.blue()), (255, 0, 0));
    assert_eq!((outside.red(), outside.green(), outside.blue()), (255, 255, 255));
}

#[cfg(feature = "paint")]
#[test]
fn zoom_changes_native_layout_nested_flow_paint_and_hit_testing() {
    let tree = parse_html(r#"<!doctype html><style>
        body { margin:0 } #zoomed { --factor:200%; zoom:var(--factor) !important; width:100px; background:red }
        #nested { zoom:150%; width:20px; height:10px; background:lime }
        #ordinary { width:20px; height:10px; background:blue }
        #after { width:10px; height:10px; background:black }
        #invalid { zoom:1.25; zoom:-2; zoom:2px; zoom:banana }
        #inherit { zoom:inherit; width:10px; height:5px }
        #unset { zoom:unset; width:10px; height:5px }
        button { font:20px/24px monospace } #control-zoom { zoom:2 }
        </style><div id="default"></div><div id="zoomed" style="zoom:3">
        <div id="nested"></div><div id="ordinary"></div><div id="inherit"></div><div id="unset"></div>
        </div><div id="after"></div><div id="invalid"></div>
        <button id="control">AB</button><button id="control-zoom">AB</button>"#);
    let mut resources = obscura_render::RenderResourceCache::with_loader(|_: &str| None);
    let mut prepared = obscura_render::prepare_dom(&tree, (400.0, 300.0), None, &mut resources).unwrap();
    let node = |name| tree.get_element_by_id(name).unwrap();
    let rect = |name| prepared.document_rect(node(name)).unwrap();
    let computed = |name| prepared.computed_style(node(name)).unwrap();
    assert_eq!(computed("default")["zoom"], "1");
    assert_eq!(computed("zoomed")["zoom"], "2");
    assert_eq!(computed("zoomed")["width"], "100px");
    assert_eq!(computed("ordinary")["zoom"], "1");
    assert_eq!(computed("inherit")["zoom"], "2");
    assert_eq!(computed("unset")["zoom"], "1");
    assert_eq!(computed("invalid")["zoom"], "1.25");
    assert_eq!((rect("zoomed").width, rect("zoomed").height), (200.0, 80.0));
    assert_eq!((rect("nested").width, rect("nested").height), (60.0, 30.0));
    assert_eq!((rect("ordinary").width, rect("ordinary").height, rect("ordinary").y), (40.0, 20.0, 30.0));
    assert_eq!(rect("after").y, 80.0);
    let normal_control = rect("control");
    let zoom_control = rect("control-zoom");
    assert!((zoom_control.width - normal_control.width * 2.0).abs() <= 1.0);
    assert!((zoom_control.height - normal_control.height * 2.0).abs() <= 1.0);
    let scroll = prepared.resolve_scroll_state(&tree, (0.0, 0.0), &HashMap::new());
    assert_eq!(prepared.hit_test(&tree, &scroll, 50.0, 20.0), Some(node("nested")));
    let image = obscura_render::paint_prepared(&tree, &mut prepared, &mut resources, (0.0, 0.0)).unwrap();
    let pixel = image.pixel(50, 20).unwrap();
    assert_eq!((pixel.red(), pixel.green(), pixel.blue()), (0, 255, 0));
    let pixel = image.pixel(150, 70).unwrap();
    assert_eq!((pixel.red(), pixel.green(), pixel.blue()), (255, 0, 0));
}

#[cfg(feature = "paint")]
#[test]
fn zoom_scales_background_images_once_in_used_paint_coordinates() {
    for (size, side) in [
        ("10px 10px", 20),
        ("auto", 20),
        ("", 20),
        ("50% 50%", 20),
        ("calc(25% + 1em) auto", 30),
        ("1rem 1rem", 20),
        ("calc(5vw + 5vh) 10px", 20),
        ("contain", 40),
        ("cover", 40),
    ] {
        let tree = parse_html(&format!(r#"<!doctype html><style>
            html {{ font-size:10px }} body {{ margin:0 }}
            #image {{ zoom:2; width:20px; height:20px; font-size:10px; background-color:white;
                background-image:url("data:image/svg+xml,%3Csvg%20xmlns='http://www.w3.org/2000/svg'%20width='10'%20height='10'%20viewBox='0%200%2010%2010'%3E%3Crect%20width='10'%20height='10'%20fill='red'/%3E%3C/svg%3E");
                background-size:{size}; background-repeat:no-repeat }}
            </style><div id="image"></div>"#));
        let image = obscura_render::paint_dom(&tree, (100.0, 100.0), None).unwrap();
        let inside = image.pixel(side - 1, side - 1).unwrap();
        assert_eq!((inside.red(), inside.green(), inside.blue()), (255, 0, 0), "{size}");
        let outside = image.pixel(side, side).unwrap();
        assert_eq!((outside.red(), outside.green(), outside.blue()), (255, 255, 255), "{size}");
    }
}

#[cfg(feature = "paint")]
#[test]
fn generated_pseudo_zoom_changes_box_paint_and_following_flow_without_compounding() {
    use obscura_render::{
        paint_prepared, prepare_dom_with_dynamic_fonts_and_stylesheet_cache,
        prepare_dom_with_retained_attribute_styles, RenderResourceCache,
    };

    for (host_zoom, pseudo_zoom, side) in [
        (1, "2", 20), (2, "inherit", 40), (2, "normal", 20), (2, "150%", 30),
    ] {
        let tree = parse_html(&format!(r#"<!doctype html><style>
            body {{ margin:0 }} #host {{ zoom:{host_zoom}; width:50px; background:white }}
            #host::before, #host::after {{ content:''; display:block; width:10px; height:10px; zoom:{pseudo_zoom} }}
            #host::before {{ background:red }} #host::after {{ background:lime }}
            #child {{ width:10px; height:5px; background:blue }}
            #after {{ width:10px; height:5px }} #after.changed {{ width:11px }}
            </style><div id="host"><div id="child"></div></div><div id="after" class=""></div>"#));
        let host = tree.get_element_by_id("host").unwrap();
        let child = tree.get_element_by_id("child").unwrap();
        let after = tree.get_element_by_id("after").unwrap();
        let child_height = 5 * host_zoom;
        let mut resources = RenderResourceCache::with_loader(|_: &str| None);
        let mut cache = obscura_render::css::StylesheetCache::default();
        let mut prepared = prepare_dom_with_dynamic_fonts_and_stylesheet_cache(
            &tree, (150.0, 120.0), None, &mut resources, &[], &mut cache,
        ).unwrap();
        for (before, next) in [("", "changed"), ("changed", "")] {
            assert_eq!(prepared.document_rect(child).unwrap().y, side as f32, "{host_zoom} {pseudo_zoom}");
            assert_eq!(prepared.document_rect(host).unwrap().height, (side * 2 + child_height) as f32);
            assert_eq!(prepared.document_rect(after).unwrap().y, (side * 2 + child_height) as f32);
            let image = paint_prepared(&tree, &mut prepared, &mut resources, (0.0, 0.0)).unwrap();
            let pixel = image.pixel(side - 1, side - 1).unwrap();
            assert_eq!((pixel.red(), pixel.green(), pixel.blue()), (255, 0, 0));
            let pixel = image.pixel(side, 1).unwrap();
            assert_eq!((pixel.red(), pixel.green(), pixel.blue()), (255, 255, 255));
            let pixel = image.pixel(1, side + 1).unwrap();
            assert_eq!((pixel.red(), pixel.green(), pixel.blue()), (0, 0, 255));
            let pixel = image.pixel(side - 1, side * 2 + child_height - 1).unwrap();
            assert_eq!((pixel.red(), pixel.green(), pixel.blue()), (0, 255, 0));
            tree.with_node_mut(after, |node| node.set_attribute("class", next.into()));
            prepared = prepare_dom_with_retained_attribute_styles(
                &tree, (150.0, 120.0), None, &mut resources, &[], &mut cache, prepared,
                &[obscura_render::dom::AttributeStyleMutation {
                    node: after, name: "class".into(), old_value: Some(before.into()), new_value: Some(next.into()),
                }],
            ).unwrap();
        }
        assert_eq!(prepared.document_rect(child).unwrap().y, side as f32);
        assert_eq!(prepared.document_rect(after).unwrap().y, (side * 2 + child_height) as f32);
    }
}
