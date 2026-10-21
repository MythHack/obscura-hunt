#![cfg(feature = "paint")]

use obscura_dom::parse_html;
use obscura_render::{RenderResourceCache, prepare_dom};
use std::cell::RefCell;
use std::rc::Rc;

const SERIF: &[u8] = include_bytes!("../assets/liberation-serif.ttf");

#[test]
fn cyrillic_only_subset_shapes_like_unrestricted_font_without_loading_unused_subsets() {
    let markup = |range: &str| format!(r#"<html><head><style>
        @font-face {{font-family:Fixture;src:url(cyrillic.ttf);{range}}}
        @font-face {{font-family:Fixture;src:url(unused-latin.ttf);unicode-range:U+20-7E}}
        @font-face {{font-family:Fixture;src:url(unused-greek.ttf);unicode-range:U+0370-03FF}}
        #sample {{display:inline-block;width:max-content;font:40px Fixture;white-space:nowrap}}
        </style></head><body><span id="sample">ЖЖЖЖяяяя</span></body></html>"#);
    let render = |html: String| {
        let tree = parse_html(&html);
        let loads = Rc::new(RefCell::new(Vec::new()));
        let received = loads.clone();
        let mut resources = RenderResourceCache::with_loader(move |url: &str| {
            received.borrow_mut().push(url.to_string());
            (url == "https://font.example/cyrillic.ttf").then(|| SERIF.to_vec())
        });
        let width = prepare_dom(&tree, (600.0, 150.0), Some("https://font.example/page"), &mut resources)
            .unwrap().document_rect(tree.get_element_by_id("sample").unwrap()).unwrap().width;
        let requests = loads.borrow().clone();
        (width, requests)
    };
    let (subset, requests) = render(markup("unicode-range:U+0400-04FF"));
    let (control, control_requests) = render(markup(""));
    let fallback_tree = parse_html(r#"<span id="sample" style="display:inline-block;width:max-content;
        font:40px Fixture;white-space:nowrap">ЖЖЖЖяяяя</span>"#);
    let fallback = prepare_dom(&fallback_tree, (600.0, 150.0), None,
        &mut RenderResourceCache::with_loader(|_: &str| None)).unwrap()
        .document_rect(fallback_tree.get_element_by_id("sample").unwrap()).unwrap().width;
    assert_eq!(subset, control);
    assert_ne!(subset, fallback, "a fallback face must not mask a skipped Cyrillic subset");
    assert_eq!(requests, vec!["https://font.example/cyrillic.ttf"]);
    assert_eq!(control_requests, requests);
}

#[test]
fn a_subset_becomes_required_when_document_text_changes() {
    let tree = parse_html(r#"<style>
        @font-face {font-family:Fixture;src:url(latin.ttf);unicode-range:U+20-7E}
        @font-face {font-family:Fixture;src:url(cyrillic.ttf);unicode-range:U+04??}
        @font-face {font-family:Fixture;src:url(unused-bold.ttf);font-weight:700}
        @font-face {font-family:Fixture;src:url(unused-italic.ttf);font-style:italic}
        @font-face {font-family:Unused;src:url(unused-family.ttf)}
        @media(min-width:1000px) {
            @font-face {font-family:Fixture;src:url(unused-media.ttf)}
        }
        </style><span id="sample" style="font:40px Fixture">Latin</span>
        <span style="display:none;font-family:Unused">Hidden</span>
        <span style="font-family:monospace,Unused">Local fallback</span>"#);
    let requests = Rc::new(RefCell::new(Vec::new()));
    let seen = requests.clone();
    let mut resources = RenderResourceCache::with_loader(move |url: &str| {
        seen.borrow_mut().push(url.to_string());
        Some(SERIF.to_vec())
    });
    prepare_dom(&tree, (600.0, 150.0), Some("https://font.example/page"), &mut resources).unwrap();
    assert_eq!(*requests.borrow(), vec!["https://font.example/latin.ttf"]);
    let text = tree.get_node(tree.get_element_by_id("sample").unwrap()).unwrap().first_child.unwrap();
    tree.with_node_mut(text, |node| {
        if let obscura_dom::NodeData::Text { contents } = &mut node.data {
            *contents = "Ж".into();
        }
    });
    prepare_dom(&tree, (600.0, 150.0), Some("https://font.example/page"), &mut resources).unwrap();
    assert_eq!(*requests.borrow(), vec![
        "https://font.example/latin.ttf", "https://font.example/cyrillic.ttf"
    ]);
}

fn woff2_fixture() -> Vec<u8> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.decode(
        include_str!("fonts/obscura-vf-test.woff2.b64").split_whitespace().collect::<String>()
    ).unwrap()
}

fn uncompressed_woff_fixture() -> Vec<u8> {
    let count = u16::from_be_bytes(SERIF[4..6].try_into().unwrap()) as usize;
    let mut output = vec![0; 44 + count * 20];
    output[..4].copy_from_slice(b"wOFF");
    output[4..8].copy_from_slice(&SERIF[..4]);
    output[12..14].copy_from_slice(&(count as u16).to_be_bytes());
    let mut sfnt_size = 12 + count * 16;
    for index in 0..count {
        let table = &SERIF[12 + index * 16..28 + index * 16];
        let offset = u32::from_be_bytes(table[8..12].try_into().unwrap()) as usize;
        let length = u32::from_be_bytes(table[12..16].try_into().unwrap()) as usize;
        let position = output.len();
        let entry = &mut output[44 + index * 20..64 + index * 20];
        entry[..4].copy_from_slice(&table[..4]);
        entry[4..8].copy_from_slice(&(position as u32).to_be_bytes());
        entry[8..12].copy_from_slice(&(length as u32).to_be_bytes());
        entry[12..16].copy_from_slice(&(length as u32).to_be_bytes());
        entry[16..20].copy_from_slice(&table[4..8]);
        output.extend_from_slice(&SERIF[offset..offset + length]);
        output.resize((output.len() + 3) & !3, 0);
        sfnt_size += (length + 3) & !3;
    }
    let length = output.len() as u32;
    output[8..12].copy_from_slice(&length.to_be_bytes());
    output[16..20].copy_from_slice(&(sfnt_size as u32).to_be_bytes());
    output
}

#[test]
fn real_sfnt_woff_and_transformed_woff2_fonts_decode_through_one_public_interface() {
    assert!(obscura_render::decode_font_bytes(SERIF).is_some());
    let woff = uncompressed_woff_fixture();
    let decoded = obscura_render::decode_font_bytes(&woff).expect("real uncompressed WOFF");
    assert!(decoded.starts_with(b"\0\x01\0\0"));
    let woff2 = woff2_fixture();
    let decoded = obscura_render::decode_font_bytes(&woff2).expect("real transformed WOFF2");
    assert!(decoded.starts_with(b"\0\x01\0\0"));
    assert!(obscura_render::decode_font_bytes(&decoded).is_some());
}

#[test]
fn malformed_fonts_and_dishonest_woff_table_sizes_are_rejected() {
    for bytes in [&b"\0"[..], &b"\0\x01\0\0"[..], &b"OTTO"[..], &b"ttcf"[..]] {
        assert!(obscura_render::decode_font_bytes(bytes).is_none());
    }
    let mut woff = uncompressed_woff_fixture();
    woff[16..20].copy_from_slice(&32u32.to_be_bytes());
    woff[56..60].copy_from_slice(&(33u32 * 1024 * 1024).to_be_bytes());
    assert!(obscura_render::decode_font_bytes(&woff).is_none());
    let mut woff2 = woff2_fixture();
    woff2[16..20].copy_from_slice(&(33u32 * 1024 * 1024).to_be_bytes());
    assert!(obscura_render::decode_font_bytes(&woff2).is_none());
    // A tiny advertised total cannot authorize an oversized real table stream.
    let mut directory_bomb = vec![0; 48];
    directory_bomb[..4].copy_from_slice(b"wOF2");
    directory_bomb[4..8].copy_from_slice(b"\0\x01\0\0");
    directory_bomb[12..14].copy_from_slice(&1u16.to_be_bytes());
    directory_bomb[16..20].copy_from_slice(&32u32.to_be_bytes());
    directory_bomb.extend_from_slice(&[0, 0x90, 0x80, 0x80, 0x01]);
    let length = directory_bomb.len() as u32;
    directory_bomb[8..12].copy_from_slice(&length.to_be_bytes());
    assert!(obscura_render::decode_font_bytes(&directory_bomb).is_none());
    let mut compressed_bomb = woff2_fixture();
    compressed_bomb.resize(8 * 1024 * 1024 + 1, 0);
    assert!(obscura_render::decode_font_bytes(&compressed_bomb).is_none());
}
