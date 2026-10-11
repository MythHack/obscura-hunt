#![cfg(feature = "render")]

use obscura_js::runtime::ObscuraJsRuntime;
use serde_json::json;

#[test]
fn computed_perspective_and_zoom_follow_stylesheet_cascade_and_live_geometry() {
    let mut rt = ObscuraJsRuntime::new();
    rt.set_dom(obscura_dom::parse_html(r#"<!doctype html><style>
        html { font-size:10px } body { margin:0 }
        #camera { --depth:calc(2rem + 3em); --factor:200%; font-size:20px;
            perspective:var(--depth) !important; zoom:var(--factor) !important; width:100px }
        #nested { zoom:150%; width:20px; height:10px }
        #ordinary { width:20px; height:10px }
        #invalid { perspective:40px; perspective:-2px; perspective:10%; zoom:1.25; zoom:-2; zoom:2px }
        #inherited { perspective:inherit; zoom:inherit; width:10px; height:5px }
        #unset { perspective:unset; zoom:unset; width:10px; height:5px }
        #after { width:10px; height:10px }
        </style><div id="default"></div><div id="camera" style="perspective:7px;zoom:3">
        <div id="nested"></div><div id="ordinary"></div><div id="inherited"></div><div id="unset"></div>
        </div><div id="after"></div><div id="invalid"></div>"#));
    rt.run_page_init();
    assert_eq!(rt.evaluate(r#"(() => {
        const style = id => getComputedStyle(document.getElementById(id));
        const geometry = id => { const r = document.getElementById(id).getBoundingClientRect(); return [r.width, r.height, r.top]; };
        globalThis.cameraStyle = style('camera');
        globalThis.cameraGeometry = () => geometry('camera');
        return [style('default').perspective, style('default').zoom,
            cameraStyle.perspective, cameraStyle.zoom, cameraStyle.width,
            style('ordinary').zoom, style('inherited').perspective, style('inherited').zoom,
            style('unset').perspective, style('unset').zoom,
            style('invalid').perspective, style('invalid').zoom,
            geometry('camera'), geometry('nested'), geometry('ordinary'), geometry('after')];
    })()"#).unwrap(), json!([
        "none", "1", "80px", "2", "100px", "1", "80px", "2", "none", "1", "40px", "1.25",
        [200, 80, 0], [60, 30, 0], [40, 20, 30], [10, 10, 80]
    ]));
    rt.evaluate("document.getElementById('camera').style.setProperty('--factor', '100%')").unwrap();
    assert_eq!(rt.evaluate("[cameraStyle.zoom, cameraStyle.perspective, cameraGeometry()]").unwrap(),
        json!(["1", "80px", [100, 35, 0]]));
}
