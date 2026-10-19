use obscura_js::runtime::ObscuraJsRuntime;
use serde_json::json;

fn page(html: &str) -> ObscuraJsRuntime {
    let mut runtime = ObscuraJsRuntime::new();
    runtime.set_dom(obscura_dom::parse_html(html));
    runtime.run_page_init();
    runtime
}

#[test]
fn dom_matrix_transforms_points_and_inverts_actual_css_matrices() {
    let mut rt = page("<body></body>");
    assert_eq!(rt.evaluate(r#"(() => {
        const m = new DOMMatrix([2, 0, 0, 3, 7, 11]);
        const point = m.transformPoint(new DOMPoint(4, 5, 6, 1));
        const back = point.matrixTransform(m.inverse());
        const css = new DOMMatrix('matrix(2, 0, 0, 3, 7, 11)');
        const translated = m.translate(1, 2);
        const p3 = new DOMMatrix('matrix3d(1,0,0,0,0,1,0,0,0,0,1,-0.005,0,0,0,1)')
            .transformPoint({x: 4, y: 5, z: 100});
        const singular = new DOMMatrix([0, 0, 0, 0, 0, 0]).inverse();
        const fails = f => { try { f(); return ''; } catch (e) { return e.name; } };
        return [[m.a, m.d, m.e, m.f, m.is2D, m.isIdentity], point.toJSON(),
            Object.fromEntries(Object.entries(back.toJSON()).map(([k,v]) => [k,Math.round(v*1e9)/1e9])),
            Array.from(css.toFloat64Array()), [translated.e, translated.f],
            p3.toJSON(), [Number.isNaN(singular.a), Number.isNaN(singular.m44), singular.is2D],
            fails(() => new DOMMatrix([1, 2, 3])), fails(() => new DOMMatrix('matrix(1,2)')),
            fails(() => DOMMatrix.fromMatrix({a: 2, m11: 3})),
            DOMMatrix.fromFloat32Array(new Float32Array([2,0,0,3,7,11])).e,
            new DOMMatrix().scale(2, 3, 1, 5, 7).transformPoint({x: 6, y: 8}).toJSON(),
            new DOMMatrix().rotate(90).transformPoint({x: 2, y: 0}).y];
    })()"#).unwrap(), json!([
        [2, 3, 7, 11, true, false], {"x":15,"y":26,"z":6,"w":1},
        {"x":4,"y":5,"z":6,"w":1}, [2,0,0,0,0,3,0,0,0,0,1,0,7,11,0,1], [9,17],
        {"x":4,"y":5,"z":100,"w":0.5}, [true,true,false],
        "TypeError", "SyntaxError", "TypeError", 7, {"x":7,"y":10,"z":0,"w":1}, 2
    ]));
}

#[test]
fn dom_matrix_mutable_components_and_multiplication_preserve_column_major_order() {
    let mut rt = page("<body></body>");
    assert_eq!(rt.evaluate(r#"(() => {
        const a = new DOMMatrix([2,0,0,3,7,11]);
        const b = new DOMMatrix([1,0,0,1,5,6]);
        const right = a.multiply(b), left = b.multiply(a);
        a.a = 4; a.m42 = 9;
        const three = new DOMMatrix([1,0,0,0,0,1,0,0,0,0,1,0,3,4,5,1]);
        const back = three.inverse().transformPoint(three.transformPoint({x: 8,y: 9,z: 10}));
        const readOnly = new DOMMatrixReadOnly([2,0,0,3,7,11]);
        return [[right.e,right.f,left.e,left.f], [a.m11,a.f,a.isIdentity],
            back.toJSON(), [three.is2D,three.m43], readOnly.translate(1,2).toJSON(),
            new DOMPointReadOnly(2,3).matrixTransform(a).toJSON()];
    })()"#).unwrap(), json!([
        [17,29,12,17], [4,9,false], {"x":8,"y":9,"z":10,"w":1}, [false,5],
        {"a":2,"b":0,"c":0,"d":3,"e":9,"f":17,"m11":2,"m12":0,"m13":0,"m14":0,
         "m21":0,"m22":3,"m23":0,"m24":0,"m31":0,"m32":0,"m33":1,"m34":0,
         "m41":9,"m42":17,"m43":0,"m44":1,"is2D":true,"isIdentity":false},
        {"x":15,"y":18,"z":0,"w":1}
    ]));
}

#[cfg(feature = "render")]
#[test]
fn check_visibility_uses_boxes_and_requested_css_checks_not_area() {
    let mut rt = page(r#"<style>
        #none { display:none } #contents { display:contents }
        #transparent { opacity:0 } #hidden { visibility:hidden }
        #visible { visibility:visible } #zero { width:0;height:0 }
        </style><div id="none"><div id="child"></div></div>
        <div id="contents"><div id="boxed"></div></div>
        <div id="transparent"><div id="faded"></div></div>
        <div id="hidden"><div id="inherited"></div><div id="visible"></div></div>
        <div id="zero"></div><div id="skipped" style="content-visibility:hidden"><div id="inside"></div></div>"#);
    assert_eq!(rt.evaluate(r#"(() => {
        const inherited = getComputedStyle(document.getElementById('inherited'));
        const beforeLayout = inherited.visibility;
        void inherited.width;
        return [beforeLayout, inherited.visibility,
            getComputedStyle(document.getElementById('visible')).visibility];
    })()"#).unwrap(), json!(["hidden", "hidden", "visible"]));
    assert_eq!(rt.evaluate(r#"(() => {
        const el = id => document.getElementById(id);
        return [el('none').checkVisibility(), el('child').checkVisibility(),
            el('contents').checkVisibility(), el('boxed').checkVisibility(),
            el('zero').checkVisibility(), document.createElement('div').checkVisibility(),
            el('faded').checkVisibility(), el('faded').checkVisibility({opacityProperty:true}),
            el('faded').checkVisibility({checkOpacity:true}), el('hidden').checkVisibility(),
            el('inherited').checkVisibility({visibilityProperty:true}),
            el('visible').checkVisibility({checkVisibilityCSS:true}),
            el('skipped').checkVisibility(), el('inside').checkVisibility()];
    })()"#).unwrap(), json!([false,false,false,true,true,false,true,false,false,true,false,true,true,false]));
    rt.evaluate("document.getElementById('none').style.display = 'block'").unwrap();
    assert_eq!(rt.evaluate("document.getElementById('child').checkVisibility()").unwrap(), json!(true));
    rt.evaluate("document.getElementById('hidden').style.visibility = 'visible'").unwrap();
    assert_eq!(rt.evaluate("document.getElementById('inherited').checkVisibility({visibilityProperty:true})").unwrap(), json!(true));
}

#[cfg(feature = "render")]
#[test]
fn popover_transitions_share_native_selectors_styles_and_header_layout() {
    let mut rt = page(r#"<!doctype html><style>
        body { margin:0 }
        header { display:flex; align-items:flex-start }
        button { width:20px; height:24px; padding:0; border:0 }
        .menu { width:80px; height:80px; margin:0; padding:0; border:0 }
        .menu:popover-open { display:flex }
        #override { display:block; width:36px; height:40px; margin:0; padding:0; border:0 }
        </style><header id="header"><button></button><div id="menu" class="menu" popover></div>
        <div id="override" popover></div><menu-widget id="widget"></menu-widget></header>"#);
    assert_eq!(rt.evaluate(r#"(() => {
        const menu = document.getElementById('menu'), header = document.getElementById('header');
        const snapshot = () => {
            const rect = menu.getBoundingClientRect();
            return [menu.matches(':popover-open'), document.querySelector('header > .menu:popover-open') === menu,
                document.querySelectorAll(':is(.menu:popover-open, #missing)').length,
                menu.matches('.menu:not(:popover-open)'), getComputedStyle(menu).display,
                rect.width, rect.height, header.getBoundingClientRect().height];
        };
        const before = [];
        menu.addEventListener('beforetoggle', event => before.push([event.oldState,event.newState,event.cancelable,snapshot()]));
        const closed = snapshot();
        menu.addEventListener('beforetoggle', event => event.preventDefault(), {once:true});
        menu.showPopover(); const cancelled = snapshot(); before.length = 0;
        menu.showPopover(); const shown = snapshot();
        menu.hidePopover(); const hidden = snapshot();
        const override = document.getElementById('override'), overrideRect = override.getBoundingClientRect();
        const author = [override.matches(':popover-open'),getComputedStyle(override).display,
            getComputedStyle(override).position,overrideRect.width,overrideRect.height,header.getBoundingClientRect().height];
        const shadow = document.getElementById('widget').attachShadow({mode:'open'});
        shadow.innerHTML = '<style>[popover]{width:28px;height:32px;margin:0;padding:0;border:0}</style><div popover></div>';
        const nested = shadow.querySelector('[popover]');
        const nestedClosed = [getComputedStyle(nested).display,nested.getBoundingClientRect().height];
        nested.addEventListener('beforetoggle', () => nested.showPopover(), {once:true});
        const toggled = nested.togglePopover(true);
        const nestedShown = [shadow.querySelector(':popover-open') === nested,nested.getBoundingClientRect().width,
            nested.getBoundingClientRect().height,header.getBoundingClientRect().height];
        nested.hidePopover();
        nested.showPopover(); nested.popover = 'manual';
        const typeChanged = [nested.matches(':popover-open'),getComputedStyle(nested).display,nested.getBoundingClientRect().height];
        nested.showPopover(); nested.remove(); shadow.appendChild(nested);
        const reconnected = [nested.matches(':popover-open'),getComputedStyle(nested).display,nested.getBoundingClientRect().height];
        return [closed,cancelled,shown,hidden,before,author,nestedClosed,nestedShown,toggled,typeChanged,reconnected,
            shadow.querySelectorAll(':popover-open').length,nested.getClientRects().length,menu.getAttribute('style')];
    })()"#).unwrap(), json!([
        [false,false,0,true,"none",0,0,24], [false,false,0,true,"none",0,0,24], [true,true,1,false,"flex",80,80,24],
        [false,false,0,true,"none",0,0,24],
        [["closed","open",true,[false,false,0,true,"none",0,0,24]],
         ["open","closed",false,[true,true,1,false,"flex",80,80,24]]],
        [false,"block","fixed",36,40,24], ["none",0], [true,28,32,24], true,
        [false,"none",0], [false,"none",0], 0,0,null
    ]));
}


#[cfg(not(feature = "render"))]
#[test]
fn check_visibility_does_not_claim_an_unavailable_layout_box() {
    let mut rt = page("<div id='box'></div>");
    assert_eq!(rt.evaluate("document.getElementById('box').checkVisibility()").unwrap(), json!(false));
}

#[tokio::test(flavor = "current_thread")]
async fn media_writable_state_round_trips_without_claiming_decoder_support() {
    let mut rt = page("<video id='media' poster='data:image/png,poster'></video>");
    assert_eq!(rt.evaluate(r#"(() => {
        globalThis.media = document.getElementById('media');
        globalThis.changes = [];
        media.addEventListener('volumechange', () => changes.push([media.volume, media.muted]));
        media.muted = true; media.volume = .25; media.currentTime = 12;
        media.playbackRate = 1.5; media.defaultPlaybackRate = .75;
        media.preservesPitch = false; media.defaultMuted = true;
        media.autoplay = true; media.controls = true; media.loop = true; media.preload = 'none';
        const fails = f => { try { f(); return ''; } catch(e) { return e.name; } };
        globalThis.audio = new Audio();
        return [media.muted, media.volume, media.currentTime, media.playbackRate,
            media.defaultPlaybackRate, media.preservesPitch, media.defaultMuted,
            media.autoplay, media.controls, media.loop, media.preload,
            fails(() => media.volume = 2), fails(() => media.volume = -.1),
            fails(() => media.volume = NaN), fails(() => media.currentTime = Infinity),
            media.volume, media.paused, media.readyState, media.duration !== media.duration,
            media.canPlayType('video/mp4'), audio instanceof HTMLAudioElement];
    })()"#).unwrap(), json!([true,0.25,12,1.5,0.75,false,true,true,true,true,"none",
        "IndexSizeError","IndexSizeError","TypeError","TypeError",0.25,true,0,true,"",true]));
    rt.run_event_loop_until_quiescent(1_000, 20).await.unwrap();
    assert_eq!(rt.evaluate("changes").unwrap(), json!([[0.25,true],[0.25,true]]));
    rt.evaluate("globalThis.playError = ''; audio.play().catch(e => playError = e.name)").unwrap();
    rt.run_event_loop_until_quiescent(1_000, 20).await.unwrap();
    assert_eq!(rt.evaluate("[playError,audio.paused,audio.readyState]").unwrap(), json!(["NotSupportedError",true,0]));
}

#[tokio::test(flavor = "current_thread")]
async fn text_tracks_keep_live_identity_and_report_real_inline_load_and_errors() {
    let mut rt = page("<video id='media'></video>");
    assert_eq!(rt.evaluate(r#"(() => {
        globalThis.media = document.getElementById('media');
        globalThis.list = media.textTracks;
        globalThis.events = [];
        list.addEventListener('addtrack', e => events.push(['add',e.track.label]));
        list.addEventListener('removetrack', e => events.push(['remove',e.track.label]));
        list.addEventListener('change', () => events.push(['change']));
        globalThis.fresh = document.createElement('track');
        const before = fresh.readyState;
        globalThis.added = media.addTextTrack('metadata','Data','en');
        globalThis.trackElement = document.createElement('track');
        trackElement.id = 'captions'; trackElement.label = 'Captions'; trackElement.kind = 'captions';
        trackElement.src = 'data:text/vtt,' + encodeURIComponent('WEBVTT\n\ncue\n00:00:01.000 --> 00:00:03.000\nHello');
        trackElement.addEventListener('load', () => events.push(['load',trackElement.readyState]));
        media.appendChild(trackElement); trackElement.track.mode = 'hidden';
        return [before,list === media.textTracks,list.length,list[0] === trackElement.track,
            list[1] === added,list.getTrackById('captions') === trackElement.track,
            added.mode,trackElement.readyState,trackElement.track.cues.length];
    })()"#).unwrap(), json!([0,true,2,true,true,true,"hidden",0,0]));
    rt.run_event_loop_until_quiescent(1_000, 20).await.unwrap();
    assert_eq!(rt.evaluate(r#"(() => {
        const cue = trackElement.track.cues[0];
        const result = [trackElement.readyState,cue.id,cue.startTime,cue.endTime,cue.text,
            added.cues.length, events.some(e => e[0] === 'add' && e[1] === 'Data'),
            events.some(e => e[0] === 'add' && e[1] === 'Captions'),
            events.some(e => e[0] === 'load' && e[1] === 2)];
        trackElement.track.mode = 'disabled'; result.push(trackElement.track.cues === null);
        trackElement.remove(); result.push(list.length,list[0] === added);
        return result;
    })()"#).unwrap(), json!([2,"cue",1,3,"Hello",0,true,true,true,true,1,true]));
    rt.run_event_loop_until_quiescent(1_000, 20).await.unwrap();
    assert_eq!(rt.evaluate("events.some(e => e[0] === 'remove' && e[1] === 'Captions')").unwrap(), json!(true));
    rt.evaluate(r#"media.appendChild(fresh); fresh.src = 'data:text/vtt,not%20WebVTT';
        fresh.track.mode = 'hidden'; globalThis.trackError = 0;
        fresh.onerror = () => trackError++;
        globalThis.invalidKind = ''; try { media.addTextTrack('invalid'); } catch(e) { invalidKind=e.name; }
    "#).unwrap();
    rt.run_event_loop_until_quiescent(1_000, 20).await.unwrap();
    assert_eq!(rt.evaluate("[fresh.readyState,fresh.track.cues.length,trackError,invalidKind]").unwrap(),
        json!([3,0,1,"TypeError"]));
}

#[tokio::test(flavor = "current_thread")]
async fn external_text_tracks_load_guarded_resources_and_never_succeed_on_http_errors() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        use std::io::{Read as _, Write as _};
        for _ in 0..3 {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 2048];
            let read = stream.read(&mut request).unwrap();
            let missing = String::from_utf8_lossy(&request[..read]).contains("/missing.vtt");
            if !missing { std::thread::sleep(std::time::Duration::from_millis(100)); }
            let invalid = String::from_utf8_lossy(&request[..read]).contains("/invalid.vtt");
            let body = if missing { "missing" } else if invalid { "not a WebVTT file" }
                else { "WEBVTT\n\n00:00:01.000 --> 00:00:03.000\nExternal" };
            let status = if missing { "404 Not Found" } else { "200 OK" };
            stream.write_all(format!("HTTP/1.1 {status}\r\nContent-Type: text/vtt\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).unwrap();
        }
    });
    let mut rt = ObscuraJsRuntime::new();
    rt.set_dom(obscura_dom::parse_html("<video id='media'><track id='external'></video>"));
    rt.set_url(&format!("http://{address}/page"));
    rt.set_http_client(std::sync::Arc::new(obscura_net::ObscuraHttpClient::with_full_options(
        std::sync::Arc::new(obscura_net::CookieJar::new()), None, true,
    )));
    rt.run_page_init();
    rt.evaluate(r#"globalThis.external = document.getElementById('external');
        globalThis.states = []; external.onload = () => states.push(external.readyState);
        external.onerror = () => states.push(external.readyState);
        external.src = '/captions.vtt'; external.track.mode = 'hidden';
        globalThis.loadingState = -1;
        setTimeout(() => loadingState = external.readyState,0);"#).unwrap();
    rt.run_event_loop_until_quiescent(2_000, 30).await.unwrap();
    assert_eq!(rt.evaluate("[external.readyState,external.track.cues[0].text,states,loadingState]").unwrap(),
        json!([2,"External",[2],1]));
    rt.evaluate("external.src = '/missing.vtt'").unwrap();
    rt.run_event_loop_until_quiescent(2_000, 30).await.unwrap();
    assert_eq!(rt.evaluate("[external.readyState,external.track.cues.length,states]").unwrap(),
        json!([3,0,[2,3]]));
    rt.evaluate("external.src = '/invalid.vtt'").unwrap();
    rt.run_event_loop_until_quiescent(2_000, 30).await.unwrap();
    assert_eq!(rt.evaluate("[external.readyState,external.track.cues.length,states]").unwrap(),
        json!([3,0,[2,3,3]]));
}

#[test]
fn text_track_cue_membership_and_mode_errors_are_observable() {
    let mut rt = page("<video id='media'></video>");
    assert_eq!(rt.evaluate(r#"(() => {
        const media=document.getElementById('media');
        const a=media.addTextTrack('metadata'), b=media.addTextTrack('captions');
        const first=new VTTCue(1,3,'first'), later=new VTTCue(4,6,'later'); first.id='first';
        a.addCue(later); a.addCue(first);
        const order=Array.from(a.cues, cue => cue.text);
        b.addCue(first);
        const moved=[a.cues.length,b.cues.getCueById('first') === first];
        const fails=f => { try { f(); return ''; } catch(e) { return e.name; } };
        const missing=fails(() => a.removeCue(first));
        const mode=fails(() => b.mode='invalid');
        let conversions=0;
        const converted=media.addTextTrack({toString() { conversions++; return 'metadata'; }});
        const symbolLabel=fails(() => media.addTextTrack('metadata',Symbol()));
        b.mode='disabled';
        return [order,moved,missing,mode,b.cues === null,b.activeCues === null,
            converted.kind,conversions,symbolLabel,media.textTracks.length];
    })()"#).unwrap(), json!([["first","later"],[1,true],"NotFoundError","TypeError",true,true,
        "metadata",1,"TypeError",3]));
}

#[tokio::test(flavor = "current_thread")]
async fn track_source_limits_and_source_replacement_do_not_fabricate_success() {
    let mut rt = page("<video><track id='bounded'></video>");
    rt.evaluate(r#"globalThis.bounded=document.getElementById('bounded');
        globalThis.errors=0; bounded.onerror=() => errors++;
        bounded.src='data:text/vtt,' + encodeURIComponent('WEBVTT\n\n'+'x'.repeat(2*1024*1024));
        bounded.track.mode='hidden';"#).unwrap();
    rt.run_event_loop_until_quiescent(2_000, 20).await.unwrap();
    assert_eq!(rt.evaluate("[bounded.readyState,bounded.track.cues.length,errors]").unwrap(), json!([3,0,1]));
    rt.evaluate(r#"bounded.src='data:text/vtt,invalid';
        bounded.src='data:text/vtt,'+encodeURIComponent('WEBVTT\n\n00:00:00.000 --> 00:00:02.000\nReplacement');"#).unwrap();
    rt.run_event_loop_until_quiescent(1_000, 20).await.unwrap();
    assert_eq!(rt.evaluate("[bounded.readyState,bounded.track.cues[0].text,errors]").unwrap(), json!([2,"Replacement",1]));
}

#[tokio::test(flavor = "current_thread")]
async fn unsupported_media_sources_fail_without_fake_metadata_or_playback() {
    let mut rt = page("<video id='media' poster='data:image/png,poster' src='data:video/mp4,invalid'></video>");
    rt.evaluate(r#"globalThis.media=document.getElementById('media');
        globalThis.events=[]; media.onloadstart=() => events.push(['start',media.networkState]);
        media.onerror=() => events.push(['error',media.error.code]);
        media.volume=.25; media.muted=true; media.load();
        globalThis.playFailure=''; media.play().catch(e => playFailure=e.name);"#).unwrap();
    rt.run_event_loop_until_quiescent(1_000, 20).await.unwrap();
    assert_eq!(rt.evaluate("[media.networkState,media.readyState,media.error.code,media.paused,media.currentSrc,media.videoWidth,media.videoHeight,media.volume,media.muted,media.poster,playFailure,events]").unwrap(),
        json!([3,0,4,true,"",0,0,0.25,true,"data:image/png,poster","NotSupportedError",[["start",2],["error",4]]]));
}

#[test]
fn dom_matrix_sources_follow_union_sequence_and_typed_array_contracts() {
    let mut rt = page("<body></body>");
    assert_eq!(rt.evaluate(r#"(() => {
        const fails = f => { try { f(); return ''; } catch(e) { return e.name; } };
        const original = new DOMMatrix([2,0,0,3,7,11]);
        const copy = new DOMMatrix(original); original.e = 99;
        const array = new Float64Array([2,0,0,3,7,11]);
        array[Symbol.iterator] = function*() { yield 99; };
        const fromArray = DOMMatrix.fromFloat64Array(array); array[4] = 88;
        const boxed = new DOMMatrix({toString() { return 'matrix(2,0,0,3,7,11)'; }});
        const iterable = new DOMMatrix({*[Symbol.iterator]() { yield* ['2',0,0,'3',7,11]; }});
        const invalid = [[], [1,2,3,4,5], new Float32Array(17), [1n,0,0,1,0,0]];
        const whitespace = fails(() => new DOMMatrix(' '));
        let gets = 0;
        const dictionary = DOMMatrix.fromMatrix({get a() { gets++; return 2; }, m11:2, e:7});
        const three = DOMMatrix.fromMatrix({is2D:false});
        class Derived extends DOMMatrix {}
        const factory = Derived.fromMatrix({e:4});
        return [[copy.a,copy.d,copy.e,copy.f], fromArray.e, boxed.e, iterable.d,
            invalid.map(input => fails(() => new DOMMatrix(input))),
            fails(() => new DOMMatrix(null)), fails(() => new DOMMatrix({length:6})),
            fails(() => new DOMMatrix(Symbol())), whitespace,
            fails(() => DOMMatrix.fromFloat32Array(new Float64Array(6))),
            fails(() => DOMMatrix.fromFloat64Array(new Float64Array(7))),
            fails(() => DOMMatrix.fromMatrix(5)),
            fails(() => DOMMatrix.fromMatrix({is2D:true,m34:1})),
            [dictionary.a,dictionary.e,gets], [three.is2D,three.isIdentity],
            [factory instanceof DOMMatrix,factory instanceof Derived],
            new DOMMatrix('NONE/**/').isIdentity];
    })()"#).unwrap(), json!([[2,3,7,11],7,7,3,
        ["TypeError","TypeError","TypeError","TypeError"],
        "SyntaxError","SyntaxError","TypeError","SyntaxError",
        "TypeError","TypeError","TypeError","TypeError",[2,7,1],[false,true],[true,false],true]));
}

#[test]
fn dom_point_constructors_and_dictionaries_preserve_idl_coordinates() {
    let mut rt = page("<body></body>");
    assert_eq!(rt.evaluate(r#"(() => {
        const fails = f => { try { f(); return ''; } catch(e) { return e.name; } };
        const constructor = new DOMPoint({x:3},undefined,null,undefined);
        const converted = new DOMPoint('2',true,null,'4');
        const point = DOMPoint.fromPoint({x:'3',w:0});
        const transformed = new DOMMatrix([2,0,0,3,7,11]).transformPoint(point);
        const readOnly = DOMPointReadOnly.fromPoint({x:5,y:6});
        const before = readOnly.x; readOnly.x = 99;
        point.x = undefined; point.y = Infinity;
        class Derived extends DOMPoint {}
        const factory = Derived.fromPoint({x:4});
        return [[Number.isNaN(constructor.x),constructor.y,constructor.z,constructor.w],
            converted.toJSON(), transformed.toJSON(),
            [before,readOnly.x,readOnly instanceof DOMPoint],
            [Number.isNaN(point.x),point.y === Infinity],
            DOMPoint.fromPoint(null).toJSON(),
            fails(() => DOMPoint.fromPoint(3)), fails(() => new DOMPoint(1n)),
            fails(() => converted.x = 1n), converted.x,
            fails(() => new DOMMatrix().transformPoint('point')),
            [factory instanceof DOMPoint,factory instanceof Derived]];
    })()"#).unwrap(), json!([[true,0,0,1],{"x":2,"y":1,"z":0,"w":4},
        {"x":6,"y":0,"z":0,"w":0},[5,5,false],[true,true],{"x":0,"y":0,"z":0,"w":1},
        "TypeError","TypeError","TypeError",2,"TypeError",[true,false]]));
}

#[tokio::test(flavor = "current_thread")]
async fn track_removal_and_reinsertion_preserve_loaded_cues_and_readiness() {
    let mut rt = page("<video id='first'><track id='track'></video><video id='second'></video>");
    rt.evaluate(r#"globalThis.first=document.getElementById('first');
        globalThis.second=document.getElementById('second'); globalThis.track=document.getElementById('track');
        globalThis.firstList=first.textTracks; globalThis.secondList=second.textTracks;
        globalThis.text=track.track; globalThis.events=[];
        firstList.onremovetrack=e => events.push(['remove',e.track === text]);
        secondList.onaddtrack=e => events.push(['add',e.track === text]);
        track.onload=() => events.push(['load',track.readyState]);
        track.src='data:text/vtt,'+encodeURIComponent('WEBVTT\n\n00:00:00.000 --> 00:00:02.000\nRetained');
        text.mode='hidden';"#).unwrap();
    rt.run_event_loop_until_quiescent(1_000, 20).await.unwrap();
    assert_eq!(rt.evaluate(r#"(() => {
        globalThis.cues=text.cues; globalThis.cue=cues[0];
        track.remove();
        const removed=[firstList.length,track.readyState,text.cues === cues,text.cues[0] === cue];
        second.appendChild(track);
        return [removed,firstList.length,secondList.length,secondList[0] === text,
            track.readyState,text.cues[0].text];
    })()"#).unwrap(), json!([[0,2,true,true],0,1,true,2,"Retained"]));
    rt.run_event_loop_until_quiescent(1_000, 20).await.unwrap();
    assert_eq!(rt.evaluate("events").unwrap(), json!([["load",2],["remove",true],["add",true]]));
    assert_eq!(rt.evaluate(r#"(() => {
        track.src=track.src;
        return [track.readyState,text.cues === cues,text.cues.length];
    })()"#).unwrap(), json!([2,true,0]));
    rt.run_event_loop_until_quiescent(1_000, 20).await.unwrap();
    assert_eq!(rt.evaluate("[track.readyState,text.cues.length,events.filter(e => e[0] === 'load').length]").unwrap(),
        json!([2,0,1]));
}

#[tokio::test(flavor = "current_thread")]
async fn enabled_tracks_without_sources_fail_asynchronously_and_can_recover() {
    let mut rt = page("<video><track id='track'></video>");
    assert_eq!(rt.evaluate(r#"(() => {
        globalThis.track=document.getElementById('track'); globalThis.states=[];
        track.onerror=() => states.push(track.readyState); track.onload=() => states.push(track.readyState);
        track.track.mode='hidden'; return track.readyState;
    })()"#).unwrap().as_f64(), Some(0.0));
    rt.run_event_loop_until_quiescent(1_000, 20).await.unwrap();
    assert_eq!(rt.evaluate("[track.readyState,track.track.cues.length,states]").unwrap(), json!([3,0,[3]]));
    assert_eq!(rt.evaluate(r#"(() => {
        track.track.mode='disabled';
        track.src='data:text/vtt,'+encodeURIComponent('WEBVTT\n\n00:00:00.000 --> 00:00:02.000\nRecovered');
        return [track.readyState,track.track.cues === null];
    })()"#).unwrap(), json!([3,true]));
    rt.run_event_loop_until_quiescent(1_000, 20).await.unwrap();
    assert_eq!(rt.evaluate("states").unwrap(), json!([3]));
    rt.evaluate("track.track.mode='hidden'").unwrap();
    rt.run_event_loop_until_quiescent(1_000, 20).await.unwrap();
    assert_eq!(rt.evaluate("[track.readyState,track.track.cues[0].text,states]").unwrap(), json!([2,"Recovered",[3,2]]));
    assert_eq!(rt.evaluate("track.removeAttribute('src'); [track.readyState,track.track.cues.length]").unwrap(), json!([2,0]));
    rt.run_event_loop_until_quiescent(1_000, 20).await.unwrap();
    assert_eq!(rt.evaluate("[track.readyState,states]").unwrap(), json!([3,[3,2,3]]));
}

#[tokio::test(flavor = "current_thread")]
async fn in_flight_track_removal_does_not_cancel_loading_or_erase_cues() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        use std::io::{Read as _, Write as _};
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0u8; 2048];
        stream.read(&mut request).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(100));
        let body = "WEBVTT\n\n00:00:00.000 --> 00:00:02.000\nDetached";
        let _ = stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/vtt\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes());
    });
    let mut rt = ObscuraJsRuntime::new();
    rt.set_dom(obscura_dom::parse_html("<video id='media'><track id='track'></video>"));
    rt.set_url(&format!("http://{address}/page"));
    rt.set_http_client(std::sync::Arc::new(obscura_net::ObscuraHttpClient::with_full_options(
        std::sync::Arc::new(obscura_net::CookieJar::new()), None, true,
    )));
    rt.run_page_init();
    rt.evaluate(r#"globalThis.track=document.getElementById('track');
        globalThis.list=document.getElementById('media').textTracks; globalThis.events=[];
        track.onload=() => events.push(['load',track.readyState]); track.onerror=() => events.push(['error']);
        list.onremovetrack=e => events.push(['remove',e.track === track.track]);
        track.src='/delayed.vtt'; track.track.mode='hidden';
        setTimeout(() => { track.remove(); events.push(['detached',track.readyState,list.length]); },0);"#).unwrap();
    rt.run_event_loop_until_quiescent(2_000, 30).await.unwrap();
    assert_eq!(rt.evaluate("[track.readyState,track.track.cues[0].text,list.length,events]").unwrap(),
        json!([2,"Detached",0,[["detached",1,0],["remove",true],["load",2]]]));
}

#[tokio::test(flavor = "current_thread")]
async fn in_flight_track_source_replacement_reports_abort_error_before_new_load() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        use std::io::{Read as _, Write as _};
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0u8; 2048];
        stream.read(&mut request).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(100));
        let body = "WEBVTT\n\n00:00:00.000 --> 00:00:02.000\nObsolete";
        let _ = stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/vtt\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes());
    });
    let mut rt = ObscuraJsRuntime::new();
    rt.set_dom(obscura_dom::parse_html("<video><track id='track'></video>"));
    rt.set_url(&format!("http://{address}/page"));
    rt.set_http_client(std::sync::Arc::new(obscura_net::ObscuraHttpClient::with_full_options(
        std::sync::Arc::new(obscura_net::CookieJar::new()), None, true,
    )));
    rt.run_page_init();
    rt.evaluate(r#"globalThis.track=document.getElementById('track'); globalThis.events=[];
        track.onerror=() => events.push(['error',track.readyState,track.track.cues.length]);
        track.onload=() => events.push(['load',track.readyState,track.track.cues[0].text]);
        track.src='/delayed.vtt'; track.track.mode='hidden';
        setTimeout(() => {
            events.push(['replacing',track.readyState]);
            track.src='data:text/vtt,'+encodeURIComponent('WEBVTT\n\n00:00:00.000 --> 00:00:02.000\nReplacement');
        },0);"#).unwrap();
    rt.run_event_loop_until_quiescent(2_000, 30).await.unwrap();
    assert_eq!(rt.evaluate("[track.readyState,track.track.cues.length,events]").unwrap(),
        json!([2,1,[["replacing",1],["error",3,0],["load",2,"Replacement"]]]));
}

#[test]
fn check_visibility_converts_options_before_inspecting_detached_boxes() {
    let mut rt = page("<body></body>");
    assert_eq!(rt.evaluate(r#"(() => {
        const element=document.createElement('div');
        const fails=f => { try { f(); return ''; } catch(e) { return e.name; } };
        const reads=[];
        const options=Object.fromEntries(['checkOpacity','checkVisibilityCSS','contentVisibilityAuto',
            'opacityProperty','visibilityProperty'].map(name => [name,false]));
        for (const name of Object.keys(options)) Object.defineProperty(options,name,
            {get() { reads.push(name); return false; }});
        return [element.checkVisibility(null),element.checkVisibility(options),reads,
            fails(() => element.checkVisibility(1)),fails(() => element.checkVisibility('options')),
            fails(() => element.checkVisibility({get opacityProperty() { throw new RangeError(); }}))];
    })()"#).unwrap(), json!([false,false,
        ["checkOpacity","checkVisibilityCSS","contentVisibilityAuto","opacityProperty","visibilityProperty"],
        "TypeError","TypeError","RangeError"]));
}

#[tokio::test(flavor = "current_thread")]
async fn media_setters_reject_idl_errors_without_mutation_and_load_resets_rates() {
    let mut rt = page("<video id='media' muted></video>");
    assert_eq!(rt.evaluate(r#"(() => {
        globalThis.media=document.getElementById('media'); globalThis.rates=[];
        media.onratechange=() => rates.push(media.playbackRate);
        const fails=f => { try { f(); return ''; } catch(e) { return e.name; } };
        const before=[media.muted,media.defaultMuted,media.volume,media.currentTime,media.playbackRate];
        media.volume='.25'; media.currentTime='3'; media.playbackRate=2; media.defaultPlaybackRate=.5;
        media.defaultMuted=false; media.muted=false; media.muted=false;
        const errors=[fails(() => media.volume=1n),fails(() => media.currentTime=1n),
            fails(() => media.playbackRate=NaN),fails(() => media.defaultPlaybackRate=Infinity)];
        const unchanged=[media.volume,media.currentTime,media.playbackRate,media.defaultPlaybackRate];
        media.load();
        return [before,errors,unchanged,[media.currentTime,media.playbackRate,media.volume,media.muted,
            media.defaultMuted,media.readyState,media.paused]];
    })()"#).unwrap(), json!([[true,true,1,0,1],
        ["TypeError","TypeError","TypeError","TypeError"],[0.25,3,2,0.5],
        [0,0.5,0.25,false,false,0,true]]));
    rt.run_event_loop_until_quiescent(1_000, 20).await.unwrap();
    assert_eq!(rt.evaluate("rates").unwrap(), json!([0.5,0.5,0.5]));
    assert_eq!(rt.evaluate("media.defaultPlaybackRate=0; media.defaultPlaybackRate").unwrap().as_f64(), Some(0.0));
}

#[test]
fn geometry_math_uses_receiver_coordinates_and_preserves_all_exposed_operations() {
    let mut rt = page("<body></body>");
    assert_eq!(rt.evaluate(r#"(() => {
        const fails=f => { try { f(); return ''; } catch(e) { return e.name; } };
        const rounded=point => Object.fromEntries(Object.entries(point.toJSON()).map(([k,v]) => [k,Math.round(v*1e9)/1e9]));
        const matrix=new DOMMatrixReadOnly([2,0,0,3,7,11]);
        Object.defineProperty(matrix,'a',{get() { throw new Error('Shadow getter'); }});
        const translated=matrix.translate(1,2);
        const point=new DOMPoint(2,3);
        Object.defineProperty(point,'x',{get() { throw new Error('Shadow getter'); }});
        const transformed=point.matrixTransform({a:2,d:3,e:7,f:11});
        let conversions=0;
        const scaled=new DOMMatrix().scale({valueOf() { conversions++; return 2; }});
        const left=new DOMMatrix([2,0,0,3,7,11]);
        const same=left.preMultiplySelf({e:5,f:6}) === left;
        const three=new DOMMatrix(); three.m13=1; three.m13=0;
        const stayedThree=three.is2D; three.setMatrixValue('matrix(1,0,0,1,0,0)');
        const old=new DOMMatrix([2,0,0,3,7,11]);
        const invalid=fails(() => old.setMatrixValue('matrix(1,2)'));
        const coercing=new DOMMatrix([2,0,0,3,7,11]);
        const afterConversion=coercing.translate({valueOf() {
            coercing.setMatrixValue('matrix(4,0,0,5,20,30)'); return 1;
        }},2);
        coercing.e={valueOf() { coercing.setMatrixValue('matrix(1,0,0,1,0,0)'); return 9; }};
        const ownMethod=new DOMMatrix();
        ownMethod.translateSelf=() => { throw new Error('Shadow method'); };
        ownMethod.scaleSelf(2,3,1,5,7);
        return [[translated.a,translated.d,translated.e,translated.f], transformed.toJSON(),
            [scaled.a,scaled.d,conversions], [same,left.e,left.f],
            rounded(new DOMMatrix().rotate(90,90,90).transformPoint({x:1,y:2,z:3})),
            rounded(new DOMMatrix().rotateAxisAngle(1,0,0,90).transformPoint({y:2,z:3})),
            rounded(new DOMMatrix().rotateFromVector(0,1).transformPoint({x:2})),
            new DOMMatrix().rotateFromVector(-0,-0).isIdentity,
            rounded(new DOMMatrix().skewX(45).transformPoint({x:2,y:3})),
            rounded(new DOMMatrix().skewY(45).transformPoint({x:2,y:3})),
            new DOMMatrix().flipX().transformPoint({x:2,y:3}).toJSON(),
            new DOMMatrix().flipY().transformPoint({x:2,y:3}).toJSON(),
            new DOMMatrix().scaleNonUniform(2).transformPoint({x:2,y:3}).toJSON(),
            new DOMMatrix().scale3d(2,1,2,3).transformPoint({x:2,y:3,z:4}).toJSON(),
            [stayedThree,three.is2D,three.isIdentity], [invalid,old.e],
            fails(() => new DOMMatrix().setMatrixValue()),
            fails(() => DOMMatrixReadOnly.prototype.translate.call({})),
            fails(() => DOMPointReadOnly.prototype.matrixTransform.call({})),
            fails(() => String(new DOMMatrix([1,0,0,1,0,NaN]))),
            fails(() => DOMMatrix.prototype.rotateAxisAngleSelf.call({})),
            fails(() => DOMMatrixReadOnly.prototype.flipX.call({scale() { return 'fake'; }})),
            fails(() => DOMMatrixReadOnly.prototype.flipY.call({scale() { return 'fake'; }})),
            new DOMMatrix().rotateAxisAngleSelf(0,0,0,90).isIdentity,
            [afterConversion.e,afterConversion.f,coercing.e],
            ownMethod.transformPoint({x:6,y:8}).toJSON()];
    })()"#).unwrap(), json!([[2,3,9,17],{"x":11,"y":20,"z":0,"w":1},[2,2,1],[true,12,17],
        {"x":3,"y":2,"z":-1,"w":1},{"x":0,"y":-3,"z":2,"w":1},{"x":0,"y":2,"z":0,"w":1},true,
        {"x":5,"y":3,"z":0,"w":1},{"x":2,"y":5,"z":0,"w":1},
        {"x":-2,"y":3,"z":0,"w":1},{"x":2,"y":-3,"z":0,"w":1},
        {"x":4,"y":3,"z":0,"w":1},{"x":3,"y":4,"z":5,"w":1},
        [false,true,true],["SyntaxError",7],"TypeError","TypeError","TypeError","InvalidStateError",
        "TypeError","TypeError","TypeError",true,[24,40,9],{"x":7,"y":10,"z":0,"w":1}]));
}





#[test]
fn detached_documents_adopt_real_nodes_without_losing_identity_or_mutating_the_live_page() {
    let mut rt = page("<body><div id='live'>untouched</div></body>");
    assert_eq!(rt.evaluate(r#"(() => {
        const first = document.implementation.createHTMLDocument('Editor');
        const second = document.implementation.createHTMLDocument();
        const node = first.createElement('p'); node.append(first.createTextNode('draft'));
        first.body.appendChild(node);
        const text = node.firstChild, adopted = second.adoptNode(node);
        const detached = node.parentNode === null;
        second.body.appendChild(node);
        const imported = document.importNode(node,true);
        const parsed = new DOMParser().parseFromString('<p id="parsed">content</p>','text/html');
        const range = first.createRange();
        const xml = first.implementation.createDocument('urn:notes','n:root',first.implementation.createDocumentType('n:root','public','system'));
        const cdata = xml.createCDATASection('data'), pi = xml.createProcessingInstruction('target','value');
        xml.documentElement.append(cdata,pi);
        return [first instanceof Document,first.defaultView,first.title,first.doctype.name,
            adopted === node,detached,node.ownerDocument === second,text.ownerDocument === second,
            first.body.childNodes.length,second.body.textContent,imported !== node,
            imported.ownerDocument === document,imported.textContent,document.getElementById('live').textContent,
            parsed instanceof Document,parsed.getElementById('parsed').ownerDocument === parsed,
            range.startContainer === first,range.collapsed,first.location,first.hasFocus(),first.getSelection(),
            xml instanceof XMLDocument,xml.documentElement.namespaceURI,xml.doctype.publicId,xml.doctype.systemId,
            cdata.cloneNode().ownerDocument===xml,pi.cloneNode().ownerDocument===xml];
    })()"#).unwrap(),json!([true,null,"Editor","html",true,true,true,true,0,"draft",true,true,"draft","untouched",true,true,true,true,
        null,false,null,true,"urn:notes","public","system",true,true]));
    assert_eq!(rt.evaluate(r#"(() => {
        const parsed = new DOMParser().parseFromString('<broken>', 'application/xml');
        return [parsed.documentElement.localName,
            parsed.documentElement.ownerDocument === parsed, parsed.body];
    })()"#).unwrap(), json!(["parsererror", true, null]));
}
#[tokio::test(flavor = "current_thread")]
async fn mutation_observers_keep_original_values_filters_and_removed_subtrees_until_delivery() {
    let mut rt = page("<div id='root'><span id='child' data-x='old'>abc</span></div>");
    rt.evaluate(r#"globalThis.deliveries = []; globalThis.callbackThis = false;
        globalThis.root = document.getElementById('root'); globalThis.child = document.getElementById('child');
        globalThis.observer = new MutationObserver(function(records, self) {
            callbackThis = this === self;
            deliveries.push(records.map(r => [r.type,r.target.id || r.target.nodeName,r.attributeName,r.attributeNamespace,
                r.oldValue,r instanceof MutationRecord,r.addedNodes instanceof NodeList,r.removedNodes instanceof NodeList]));
        });
        observer.observe(root,{subtree:true,childList:true,attributeOldValue:true,attributeFilter:['data-x'],characterDataOldValue:true});
        child.setAttribute('data-x','new'); child.setAttribute('ignored','x');
        child.setAttributeNS('urn:other','p:data-x','namespaced');
        child.firstChild.nodeValue = 'def';
        root.removeChild(child); child.setAttribute('data-x','detached');
    "#).unwrap();
    rt.run_event_loop().await.unwrap();
    assert_eq!(rt.evaluate("[callbackThis,deliveries]").unwrap(),json!([true,[[
        ["attributes","child","data-x",null,"old",true,true,true],
        ["characterData","#text",null,null,"abc",true,true,true],
        ["childList","root",null,null,null,true,true,true],
        ["attributes","child","data-x",null,"new",true,true,true]
    ]]]));
    rt.evaluate("child.setAttribute('data-x','after'); observer.disconnect(); root.appendChild(child);").unwrap();
    rt.run_event_loop().await.unwrap();
    assert_eq!(rt.evaluate("deliveries.length").unwrap().as_f64(), Some(1.0));
    assert_eq!(rt.evaluate(r#"(() => {
        const fails = f => { try { f(); return ''; } catch(e) { return e.name; } };
        const o = new MutationObserver(()=>{}); o.observe(root,{attributes:true,attributeOldValue:true});
        o.observe(root,{childList:true}); root.setAttribute('data-x','not-recorded');
        root.replaceChildren('x','y');
        const r = o.takeRecords();
        return [fails(()=>new MutationObserver(null)),fails(()=>o.observe(root,{})),
            fails(()=>o.observe(root,{attributes:false,attributeOldValue:true})),
            fails(()=>new MutationRecord()),r.length,Array.from(r[0].addedNodes,n=>n.data),
            r[0].removedNodes[0]===child,r[0].oldValue];
    })()"#).unwrap(),json!(["TypeError","TypeError","TypeError","TypeError",1,["x","y"],true,null]));
}
#[test]
fn dom_preinsert_validity_and_fragment_moves_report_real_tree_changes() {
    let mut rt = page("<body><div id='old'><i id='move'></i></div><div id='new'><b id='tail'></b></div></body>");
    assert_eq!(rt.evaluate(r#"(() => {
        const old = document.getElementById('old'), dest = document.getElementById('new');
        const move = document.getElementById('move'), tail = document.getElementById('tail');
        const records = new MutationObserver(() => {}); records.observe(document.body,{childList:true,subtree:true});
        const fragment = document.createDocumentFragment(); fragment.append(move,document.createTextNode('x'));
        const fragRecords = new MutationObserver(() => {}); fragRecords.observe(fragment,{childList:true});
        dest.insertBefore(fragment,tail);
        const summarize = r => [r.target.id || r.target.nodeName,Array.from(r.addedNodes,n=>n.id || n.data),
            Array.from(r.removedNodes,n=>n.id || n.data),r.previousSibling && r.previousSibling.id,r.nextSibling && r.nextSibling.id];
        const result = records.takeRecords().map(summarize), emptied = fragRecords.takeRecords().map(summarize);
        const fails = f => { try { f(); return ''; } catch(e) { return e.name; } };
        const errors = [fails(()=>move.appendChild(document.body)),fails(()=>dest.appendChild({_nid:move._nid})),
            fails(()=>document.appendChild(document.createTextNode('bad'))),fails(()=>document.appendChild(document.createElement('html'))),
            fails(()=>dest.insertBefore(move,old)),fails(()=>dest.replaceChild(document,tail))];
        return [dest.textContent,Array.from(dest.childNodes,n=>n.id || n.data),fragment.childNodes.length,result,emptied,errors,
            move.compareDocumentPosition(tail),tail.compareDocumentPosition(move),move.ownerDocument === document];
    })()"#).unwrap(), json!(["x",["move","x","tail"],0,
        [["old",[],["move"],null,null],["new",["move","x"],[],null,"tail"]],
        [["#document-fragment",[],["move","x"],null,null]],
        ["HierarchyRequestError","TypeError","HierarchyRequestError","HierarchyRequestError","NotFoundError","HierarchyRequestError"],4,2,true]));
}
#[test]
fn click_reentrancy_does_not_flip_cancelled_controls_or_skip_legend_rules() {
    let mut rt = page("<fieldset disabled><legend><input id='free' type='checkbox'></legend><legend><input id='blocked' type='checkbox'></legend><div><legend><button id='wrapped'>No</button></legend></div></fieldset><input id='cancel' type='checkbox'>");
    assert_eq!(rt.evaluate(r#"(() => {
        const free = document.getElementById('free'), blocked = document.getElementById('blocked');
        const cancel = document.getElementById('cancel'), wrapped = document.getElementById('wrapped');
        let calls = 0, changes = 0, blockedCalls = 0;
        free.addEventListener('click', () => { if (++calls < 4) free.click(); });
        free.addEventListener('change', () => changes++);
        blocked.addEventListener('click', () => blockedCalls++);
        wrapped.addEventListener('click', () => blockedCalls++);
        free._clickInProgress = true; free.click(); blocked.click(); wrapped.click();
        cancel.indeterminate = true;
        cancel.addEventListener('click', e => { cancel.click(); e.preventDefault(); });
        cancel.click();
        free.click();
        const inert = document.createElement('div'); inert.setAttribute('disabled','');
        let inertCalls = 0; inert.addEventListener('click', () => inertCalls++); inert.click();
        return [calls,changes,free.checked,blocked.checked,blockedCalls,
            cancel.checked,cancel.indeterminate,inertCalls];
    })()"#).unwrap(), json!([2,2,false,false,0,false,true,1]));
}
#[test]
fn html_interfaces_do_not_misroute_style_loader_or_framework_value_trackers() {
    let mut rt = page("<head></head><body><div id='target'></div></body>");
    assert_eq!(rt.evaluate(r#"(() => {
        const head = document.head, div = document.getElementById('target');
        const input = document.createElement('input'), textarea = document.createElement('textarea');
        const foreign = document.createElementNS('urn:foreign', 'div');
        const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
        const styleTarget = head instanceof HTMLIFrameElement ? head.contentDocument.head : head;
        const style = document.createElement('style'); style.textContent = '.ready { color: green }';
        styleTarget.appendChild(style);
        Object.defineProperty(div, 'tagName', {value:'IFRAME'});
        let constructorError = ''; try { new HTMLDivElement(); } catch(e) { constructorError = e.name; }
        HTMLDivElement.prototype.brand = 'div'; HTMLIFrameElement.prototype.brand = 'iframe';
        return [style.parentNode === head, div instanceof HTMLDivElement,
            div instanceof HTMLIFrameElement, foreign instanceof HTMLDivElement,
            svg instanceof HTMLElement, input instanceof HTMLInputElement,
            input instanceof HTMLTextAreaElement, textarea instanceof HTMLTextAreaElement,
            !!Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set,
            !!Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'checked').set,
            Object.getPrototypeOf(document.createElement('div')) === HTMLDivElement.prototype,
            HTMLDivElement.prototype !== HTMLIFrameElement.prototype,constructorError,div.brand,
            document.createElement('iframe').brand,Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype,'value').set instanceof Function];
    })()"#).unwrap(), json!([true,true,false,false,false,true,false,true,true,true,true,true,"TypeError","div","iframe",true]));
    assert_eq!(rt.evaluate(r#"(() => {
        class PrimaryWidget extends HTMLElement {}
        class SecondaryWidget extends HTMLElement {}
        class DerivedDiv extends HTMLDivElement {}
        customElements.define('primary-widget', PrimaryWidget);
        customElements.define('secondary-widget', SecondaryWidget);
        const primary = document.createElement('primary-widget');
        const ordinary = document.createElement('div');
        return [ordinary instanceof PrimaryWidget, ordinary instanceof DerivedDiv,
            primary instanceof PrimaryWidget, primary instanceof SecondaryWidget];
    })()"#).unwrap(), json!([false, false, true, false]));
}

#[test]
fn ranges_clone_extract_delete_insert_and_surround_actual_dom_content() {
    let mut rt = page("<body><div id='edit'><b>ab</b><i>cd</i><u>ef</u></div></body>");
    assert_eq!(rt.evaluate(r#"(() => {
        const host = document.getElementById('edit'), middle = host.children[1];
        const first = host.firstChild.firstChild, last = host.lastChild.firstChild;
        const observer = new MutationObserver(()=>{}); observer.observe(host,{subtree:true,childList:true,characterDataOldValue:true});
        const range = document.createRange(); range.setStart(first,1); range.setEnd(last,1);
        const clone = range.cloneContents();
        const cloneHTML = Array.from(clone.childNodes,n=>n.outerHTML).join('');
        const before = host.innerHTML, extracted = range.extractContents();
        const extractHTML = Array.from(extracted.childNodes,n=>n.outerHTML).join('');
        const collapsed = [range.collapsed,range.startContainer===host,range.startOffset];
        const mutations = observer.takeRecords().map(r=>[r.type,r.oldValue]);
        const insert = document.createElement('strong'); insert.textContent='X'; range.insertNode(insert);
        const selected = [range.startContainer===host,range.startOffset,range.endContainer===host,range.endOffset];
        range.surroundContents(document.createElement('mark'));
        const wrapped = host.innerHTML; range.deleteContents();
        const textRange = document.createRange(); textRange.setStart(first,0); textRange.setEnd(first,1);
        const textCopy = textRange.extractContents();
        return [cloneHTML,before,extractHTML,extracted.childNodes[1]===middle,collapsed,mutations,
            selected,wrapped,host.innerHTML,textCopy.textContent,textRange.collapsed,textRange.startOffset,
            clone.ownerDocument===document,extracted.ownerDocument===document];
    })()"#).unwrap(),json!(["<b>b</b><i>cd</i><u>e</u>","<b>ab</b><i>cd</i><u>ef</u>",
        "<b>b</b><i>cd</i><u>e</u>",true,[true,true,1],
        [["characterData","ab"],["childList",null],["characterData","ef"]],
        [true,1,true,2],"<b>a</b><mark><strong>X</strong></mark><u>f</u>","<b></b><u>f</u>","a",true,0,true,true]));
}

#[test]
fn ranges_split_text_and_reject_invalid_mutation_without_losing_content() {
    let mut rt = page("<!doctype html><body><div id='edit'>abcd<span>ef</span></div></body>");
    assert_eq!(rt.evaluate(r#"(() => {
        const host = document.getElementById('edit'), text = host.firstChild;
        const fails = f => { try { f(); return ''; } catch(e) { return e.name; } };
        const range = document.createRange(); range.setStart(text,2); range.collapse(true);
        const fragment = document.createDocumentFragment(); const em = document.createElement('em'); em.textContent='X';
        fragment.append(em,document.createTextNode('Y')); range.insertNode(fragment);
        const inserted = [host.innerHTML,fragment.childNodes.length,range.startContainer===text,range.startOffset,
            range.endContainer===host,range.endOffset,range.toString()];
        range.setStart(text,1); range.setEnd(host.lastChild.firstChild,1);
        const invalid = fails(()=>range.surroundContents(document.createElement('mark')));
        const intact = host.innerHTML; range.deleteContents();
        const docRange = document.createRange(); docRange.selectNode(document.doctype);
        const doctypeError = fails(()=>docRange.cloneContents());
        const orphan = document.createTextNode('alone'); range.selectNodeContents(orphan);
        return [inserted,invalid,intact,host.innerHTML,range.collapsed,doctypeError,
            fails(()=>range.insertNode(document.createElement('b'))),orphan.data];
    })()"#).unwrap(),json!([["ab<em>X</em>Ycd<span>ef</span>",0,true,2,true,3,"XY"],
        "InvalidStateError","ab<em>X</em>Ycd<span>ef</span>","a<span>f</span>",false,
        "HierarchyRequestError","HierarchyRequestError","alone"]));
}

#[test]
fn native_frame_documents_keep_owner_and_reject_cross_arena_adoption_before_removal() {
    let mut rt = page("<body><iframe id='frame'></iframe><div id='parent'>parent</div></body>");
    rt.set_url("https://example.test/parent");
    assert_eq!(rt.evaluate(r#"(() => {
        const win = document.getElementById('frame').contentWindow, childDoc = win.document;
        const node = Document.prototype.createElement.call(childDoc,'p'); node.textContent='child'; childDoc.body.appendChild(node);
        const ownerGet = Object.getOwnPropertyDescriptor(Node.prototype,'ownerDocument').get;
        const other = Object.getOwnPropertyDescriptor(Document.prototype,'implementation').get.call(childDoc).createHTMLDocument('child detached');
        const sameArena = Document.prototype.adoptNode.call(other,node) === node;
        other.body.appendChild(node);
        const fails = f => { try { f(); return ''; } catch(e) { return e.name; } };
        const errors = [fails(()=>document.adoptNode(node)),fails(()=>document.body.appendChild(node)),
            fails(()=>document.importNode(node,true)),fails(()=>childDoc.adoptNode(document.getElementById('parent')))];
        const position = document.body.compareDocumentPosition(childDoc.body), reverse = childDoc.body.compareDocumentPosition(document.body);
        const range = document.createRange(); range.selectNodeContents(document.body);
        return [sameArena,node.ownerDocument===other,ownerGet.call(node)===other,node.parentNode===other.body,
            errors,document.getElementById('parent').parentNode===document.body,other.body.textContent,
            childDoc.body.ownerDocument===childDoc,document.body.ownerDocument===document,
            [position & 1,position & 6,reverse & 1,reverse & 6],fails(()=>range.comparePoint(childDoc.body,0))];
    })()"#).unwrap(),json!([true,true,true,true,["NotSupportedError","NotSupportedError","NotSupportedError","NotSupportedError"],
        true,"child",true,true,[1,4,1,2],"WrongDocumentError"]));
}

#[test]
fn iframe_sandbox_assignment_forwards_to_the_cached_token_list() {
    let mut rt = page("<body></body>");
    assert_eq!(rt.evaluate(r#"(() => {
        const frame = document.createElement('iframe');
        document.body.appendChild(frame);
        const list = frame.sandbox;
        const sameObject = frame.sandbox === list;
        frame.sandbox = 'allow-scripts allow-same-origin';
        const assigned = [frame.getAttribute('sandbox'), frame.hasAttribute('sandbox'),
            frame.sandbox === list, frame.sandbox.value, String(frame.sandbox),
            frame.sandbox.contains('allow-scripts'), frame.sandbox.length];
        list.add('allow-forms'); list.remove('allow-same-origin');
        const mutated = [frame.getAttribute('sandbox'), frame.sandbox.contains('allow-forms'),
            frame.sandbox.contains('allow-same-origin')];
        const strictAssignment = (() => { 'use strict'; frame.sandbox = 'allow-popups';
            return frame.getAttribute('sandbox'); })();
        frame.sandbox = '';
        const empty = [frame.hasAttribute('sandbox'), frame.sandbox.length, frame.getAttribute('sandbox')];
        const plain = document.createElement('div');
        plain.sandbox = 'allow-scripts';
        return [sameObject, assigned, mutated, strictAssignment, empty,
            plain.hasAttribute('sandbox'), plain.sandbox];
    })()"#).unwrap(), json!([
        true,
        ["allow-scripts allow-same-origin", true, true, "allow-scripts allow-same-origin",
         "allow-scripts allow-same-origin", true, 2],
        ["allow-scripts allow-forms", true, false],
        "allow-popups",
        [true, 0, ""],
        false,
        null,
    ]));
}

#[test]
fn fragment_replacement_and_postconnection_steps_keep_real_mutation_order() {
    let mut rt = page("<!doctype html><body></body>");
    assert_eq!(rt.evaluate(r#"(() => {
        const doc = document.implementation.createHTMLDocument(), fragment = doc.createDocumentFragment();
        fragment.append(doc.createElement('b'),doc.createTextNode('old'));
        const removed = fragment.firstChild;
        const observer = new MutationObserver(()=>{}); observer.observe(fragment,{childList:true,subtree:true,attributes:true});
        fragment.innerHTML='<i>new</i>'; removed.setAttribute('data-state','removed');
        const records = observer.takeRecords();
        const connection = document.createDocumentFragment(), script = document.createElement('script'), frame = document.createElement('iframe');
        frame.id='later-frame';
        script.textContent="globalThis.beforeFrame = document.getElementById('later-frame').contentWindow === null;";
        connection.append(script,frame); document.body.appendChild(connection);
        const type = document.implementation.createDocumentType('html','','');
        document.doctype.replaceWith(type); type.before(document.createComment('before')); type.after(document.createComment('after'));
        return [records.map(r=>[r.type,r.addedNodes.length,r.removedNodes.length]),records[0].removedNodes[0]===removed,
            removed.ownerDocument===doc,fragment.firstChild.ownerDocument===doc,beforeFrame,
            frame.contentWindow.document.body.ownerDocument===frame.contentDocument,
            document.firstChild.nodeName,document.childNodes[2].nodeName,
            fragment.firstChild.webkitMatchesSelector('i')];
    })()"#).unwrap(),json!([[["childList",1,2],["attributes",0,0]],true,true,true,true,true,"#comment","#comment",true]));
}
