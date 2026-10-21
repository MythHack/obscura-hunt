//! Local SVG geometry shared by rendering and scraping builds. No layout,
//! transform, pathLength calibration, or retained geometry snapshot is involved.

use std::f64::consts::TAU;
use cssparser::{AtRuleParser, CowRcStr, DeclarationParser, Delimiter, Parser,
    ParserState, ParseError, QualifiedRuleParser, RuleBodyItemParser, RuleBodyParser};
use deno_core::{op2, v8, OpDecl, OpState};
use deno_error::JsErrorBox;
use kurbo::{Arc, CubicBez, ParamCurveArclen, Point, QuadBez, SvgArc};
use obscura_dom::tree::{DomTree, Node, NodeId};
use svgtypes::{Length, LengthUnit, PathParser, PathSegment, PointsParser, ViewBox};

// CSS declaration syntax is owned by cssparser, not a split(';') approximation.
struct InlineProperty<'a>(&'a str);
impl<'i> DeclarationParser<'i> for InlineProperty<'_> {
    type Declaration = (&'i str, bool);
    type Error = ();
    fn parse_value(&mut self, name: CowRcStr<'i>, input: &mut Parser<'i>, _: &ParserState)
        -> Result<Self::Declaration, ParseError<()>> {
        if !name.eq_ignore_ascii_case(self.0) { return Err(ParseError::custom(())); }
        let value = input.parse_until_before(Delimiter::Bang, |input| {
            let start = input.position();
            while input.next_including_whitespace_and_comments().is_ok() {}
            Ok::<_, ParseError<()>>(input.slice_from(start).trim())
        })?;
        let important = input.try_parse(cssparser::parse_important).is_ok();
        input.expect_exhausted()?;
        Ok((value, important))
    }
}
impl<'i> AtRuleParser<'i> for InlineProperty<'_> {
    type Prelude = (); type AtRule = (&'i str, bool); type Error = ();
}
impl<'i> QualifiedRuleParser<'i> for InlineProperty<'_> {
    type Prelude = (); type QualifiedRule = (&'i str, bool); type Error = ();
}
impl<'i> RuleBodyItemParser<'i, (&'i str, bool), ()> for InlineProperty<'_> {
    fn parse_declarations(&self) -> bool { true }
    fn parse_qualified(&self) -> bool { false }
}
fn inline_parsed<'a, T>(node: &'a Node, property: &str, parse: impl Fn(&'a str) -> Option<T>) -> Option<T> {
    let mut input = Parser::new(node.get_attribute("style")?);
    let mut parser = InlineProperty(property);
    let mut selected = None;
    for (value, important) in RuleBodyParser::new(&mut input, &mut parser).flatten() {
        if let Some(value) = parse(value) {
            if selected.as_ref().is_none_or(|(_, previous)| !previous || important) {
                selected = Some((value, important));
            }
        }
    }
    selected.map(|(value, _)| value)
}

struct GeometryFont { size: f64, root_size: f64, family: Option<String>, weight: u16, italic: bool }
impl Default for GeometryFont {
    fn default() -> Self { Self { size:16.0,root_size:16.0,family:None,weight:400,italic:false } }
}
impl GeometryFont {
    fn ex(&self, owner: &crate::ops::ObscuraState) -> f64 {
        f64::from(crate::ops::document_font_metrics(owner,self.family.as_deref(),self.weight,self.italic).x_height)*self.size
    }
}
fn geometry_font(owner: &crate::ops::ObscuraState, tree: &DomTree, id: NodeId,
    window: (f64,f64), depth: usize) -> GeometryFont {
    if depth == 256 { return GeometryFont::default(); }
    tree.with_node(id, |node| {
        let parent=node.parent.map_or_else(GeometryFont::default,|id|geometry_font(owner,tree,id,window,depth+1));
        let Some(name)=node.as_element() else { return parent; };
        // Reuse the paint engine's shorthand, CSS-wide value and numeric parser.
        // This work is lazy: absolute/percentage geometry never parses a font.
        let mut style=obscura_render::compute_style(&name.local,node.get_attribute("style"));
        let size=if let Some(expression)=style.font_size_expression.as_deref() {
            obscura_render::style::resolve_contextual_length(expression,parent.size as f32,parent.root_size as f32,
                window.0 as f32/100.0,window.1 as f32/100.0,parent.size as f32).map(f64::from)
        } else if let Some(raw)=style.font_size_raw {
            match raw {
                obscura_render::Dimension::Percent(v)|obscura_render::Dimension::Em(v) => Some(f64::from(v)*parent.size),
                obscura_render::Dimension::Ex(v) => Some(f64::from(v)*parent.ex(owner)),
                raw => match raw.resolve(parent.size as f32,parent.root_size as f32,window.0 as f32/100.0,window.1 as f32/100.0) {
                    obscura_render::Dimension::Px(v) => Some(f64::from(v)), _ => None,
                },
            }
        } else {
            style.font_size.map(f64::from).or_else(||node.get_attribute("font-size")
                .and_then(|raw|resolve_length(raw,parent.size,||parent.size,||parent.ex(owner))))
        }.filter(|n|n.is_finite() && *n>=0.0).unwrap_or(parent.size);
        let weight=obscura_render::style::computed_font_weight(style.font_weight.as_deref()
            .or_else(||node.get_attribute("font-weight")),parent.weight);
        let italic=style.font_style_italic.or_else(||node.get_attribute("font-style")
            .and_then(|raw|match raw { "normal"=>Some(false),"italic"=>Some(true),s if s.starts_with("oblique")=>Some(true),_=>None }))
            .unwrap_or(parent.italic);
        let family=style.font_family.take().or_else(||node.get_attribute("font-family").map(str::to_owned))
            .or(parent.family);
        let root_size=if node.parent==Some(tree.document()) { size } else { parent.root_size };
        GeometryFont { size,root_size,family,weight,italic }
    }).unwrap_or_default()
}
struct FontUnits<'a> {
    owner: &'a crate::ops::ObscuraState, tree: &'a DomTree, node: NodeId, window: (f64,f64),
    font: std::cell::OnceCell<GeometryFont>, ex: std::cell::OnceCell<f64>,
}
impl<'a> FontUnits<'a> {
    fn new(owner: &'a crate::ops::ObscuraState, tree: &'a DomTree, node: NodeId, window: (f64,f64)) -> Self {
        Self { owner,tree,node,window,font:std::cell::OnceCell::new(),ex:std::cell::OnceCell::new() }
    }
    fn font(&self) -> &GeometryFont { self.font.get_or_init(||geometry_font(self.owner,self.tree,self.node,self.window,0)) }
    fn ex(&self) -> f64 { *self.ex.get_or_init(||self.font().ex(self.owner)) }
}
fn resolve_length(raw: &str, reference: f64, em: impl FnOnce() -> f64, ex: impl FnOnce() -> f64) -> Option<f64> {
    let value: Length = raw.trim().parse().ok()?;
    let scale = match value.unit {
        LengthUnit::None | LengthUnit::Px => 1.0,
        LengthUnit::In => 96.0,
        LengthUnit::Cm => 96.0 / 2.54,
        LengthUnit::Mm => 96.0 / 25.4,
        LengthUnit::Pt => 96.0 / 72.0,
        LengthUnit::Pc => 16.0,
        LengthUnit::Percent => reference / 100.0,
        LengthUnit::Em => em(),
        LengthUnit::Ex => ex(),
    };
    let result = value.number * scale;
    result.is_finite().then_some(result)
}
fn length(raw: &str, reference: f64, font: &FontUnits) -> Option<f64> {
    resolve_length(raw,reference,||font.font().size,||font.ex())
}
fn dimension(node: &Node, name: &str, reference: f64, font: &FontUnits, css: bool) -> Option<f64> {
    let resolve = |raw: &str| length(raw,reference,font).filter(|n| *n >= 0.0);
    if css {
        if let Some(value) = inline_parsed(node, name, |raw| {
            if raw.eq_ignore_ascii_case("auto") { Some(None) } else { resolve(raw).map(Some) }
        }) { return value; }
    }
    node.get_attribute(name).map(|raw| resolve(raw).unwrap_or(0.0))
}
fn viewport(owner: &crate::ops::ObscuraState, tree: &DomTree, node: NodeId,
    window: (f64, f64), depth: usize) -> (f64, f64) {
    if depth == 256 { return window; }
    tree.with_node(node, |node| {
        let inherited = node.parent.map_or(window, |parent| viewport(owner,tree,parent,window,depth+1));
        if !node.as_element().is_some_and(|name| &*name.local == "svg" && &*name.ns == "http://www.w3.org/2000/svg") {
            return inherited;
        }
        if let Some(viewbox) = node.get_attribute("viewBox").and_then(|v| v.parse::<ViewBox>().ok()) {
            return (viewbox.w, viewbox.h);
        }
        let font = FontUnits::new(owner,tree,node.id,window);
        (dimension(node,"width",inherited.0,&font,true).unwrap_or(inherited.0),
         dimension(node,"height",inherited.1,&font,true).unwrap_or(inherited.1))
    }).unwrap_or(window)
}

fn arc_length(arc: Arc) -> f64 {
    // Relative tolerance also bounds subdivision count for very large geometry.
    let accuracy = arc.radii.x.abs().max(arc.radii.y.abs()) * 1e-8;
    let mut current = kurbo::ParamCurve::start(&arc);
    let mut total = 0.0;
    arc.to_cubic_beziers(accuracy.max(f64::MIN_POSITIVE), |a,b,end| {
        total += CubicBez::new(current,a,b,end).arclen(accuracy.max(1e-10));
        current = end;
    });
    total
}
fn path_length(data: &str) -> (f64, bool) {
    let mut current = Point::ZERO;
    let mut start = current;
    let mut cubic_control = None;
    let mut quadratic_control = None;
    let mut total = 0.0;
    for segment in PathParser::from(data) {
        let Ok(segment) = segment else { return (total, false) }; // SVG attributes retain the valid prefix.
        let point = |abs, x, y| if abs { Point::new(x,y) } else { current + (x,y) };
        let mut next_cubic = None;
        let mut next_quadratic = None;
        match segment {
            PathSegment::MoveTo { abs,x,y } => { current = point(abs,x,y); start = current; }
            PathSegment::LineTo { abs,x,y } => {
                let end = point(abs,x,y); total += current.distance(end); current = end;
            }
            PathSegment::HorizontalLineTo { abs,x } => {
                let end = Point::new(if abs { x } else { current.x+x },current.y);
                total += current.distance(end); current = end;
            }
            PathSegment::VerticalLineTo { abs,y } => {
                let end = Point::new(current.x,if abs { y } else { current.y+y });
                total += current.distance(end); current = end;
            }
            PathSegment::CurveTo { abs,x1,y1,x2,y2,x,y } => {
                let a=point(abs,x1,y1); let b=point(abs,x2,y2); let end=point(abs,x,y);
                total += CubicBez::new(current,a,b,end).arclen(1e-7);
                next_cubic=Some(b); current=end;
            }
            PathSegment::SmoothCurveTo { abs,x2,y2,x,y } => {
                let a=cubic_control.map_or(current, |control: Point| current+(current-control));
                let b=point(abs,x2,y2); let end=point(abs,x,y);
                total += CubicBez::new(current,a,b,end).arclen(1e-7);
                next_cubic=Some(b); current=end;
            }
            PathSegment::Quadratic { abs,x1,y1,x,y } => {
                let a=point(abs,x1,y1); let end=point(abs,x,y);
                total += QuadBez::new(current,a,end).arclen(1e-7);
                next_quadratic=Some(a); current=end;
            }
            PathSegment::SmoothQuadratic { abs,x,y } => {
                let a=quadratic_control.map_or(current, |control: Point| current+(current-control));
                let end=point(abs,x,y);
                total += QuadBez::new(current,a,end).arclen(1e-7);
                next_quadratic=Some(a); current=end;
            }
            PathSegment::EllipticalArc { abs,rx,ry,x_axis_rotation,large_arc,sweep,x,y } => {
                let end=point(abs,x,y);
                if current != end && rx != 0.0 && ry != 0.0 {
                    // Kurbo's near-zero shortcut is absolute. Normalize tiny radii
                    // before endpoint-to-centre conversion so SVG radius correction
                    // remains correct even when distant endpoints expand those radii.
                    let scale = (1e-3 / rx.abs().min(ry.abs())).max(1.0);
                    let arc=SvgArc { from:Point::new(current.x*scale,current.y*scale),
                        to:Point::new(end.x*scale,end.y*scale),
                        radii:kurbo::Vec2::new(rx.abs()*scale,ry.abs()*scale),
                        x_rotation:x_axis_rotation.to_radians(),large_arc,sweep };
                    total += Arc::from_svg_arc(&arc).map_or_else(|| current.distance(end), |arc| arc_length(arc)/scale);
                } else { total += current.distance(end); }
                current=end;
            }
            PathSegment::ClosePath { .. } => { total += current.distance(start); current=start; }
        }
        cubic_control=next_cubic; quadratic_control=next_quadratic;
    }
    (total, true)
}
fn css_path(value: &str) -> Option<CowRcStr<'_>> {
    let mut input=Parser::new(value);
    input.expect_function_matching("path").ok()?;
    let path=input.parse_nested_block(|input| {
        let path=input.expect_string_cloned()?;
        input.expect_exhausted()?;
        Ok::<_,ParseError<()>>(path)
    }).ok()?;
    input.expect_exhausted().ok()?;
    Some(path)
}
fn geometry(owner: &crate::ops::ObscuraState, tree: &DomTree, node: &Node, tag: &str, window: (f64,f64)) -> f64 {
    if tag == "path" {
        if let Some(value)=inline_parsed(node,"d",|style| {
            if style.eq_ignore_ascii_case("none") { return Some(0.0); }
            let path=css_path(style)?;
            let (length,valid)=path_length(&path);
            valid.then_some(length)
        }) { return value; }
        return path_length(node.get_attribute("d").unwrap_or("")).0;
    }
    if tag == "polyline" || tag == "polygon" {
        let mut points=PointsParser::from(node.get_attribute("points").unwrap_or(""));
        let Some((x,y))=points.next() else { return 0.0; };
        let start=Point::new(x,y); let mut previous=start; let mut total=0.0;
        for (x,y) in points { let next=Point::new(x,y);total+=previous.distance(next);previous=next; }
        return total + if tag == "polygon" { previous.distance(start) } else { 0.0 };
    }
    let (width,height)=viewport(owner,tree,node.id,window,0);
    let diagonal=width.hypot(height)/2.0f64.sqrt();
    let font=FontUnits::new(owner,tree,node.id,window);
    let dim=|name,reference| dimension(node,name,reference,&font,true).unwrap_or(0.0);
    match tag {
        "line" => {
            let coord=|name,reference| node.get_attribute(name).and_then(|s| length(s,reference,&font)).unwrap_or(0.0);
            (coord("x2",width)-coord("x1",width)).hypot(coord("y2",height)-coord("y1",height))
        }
        "circle" => TAU*dim("r",diagonal),
        "ellipse" => {
            let rx=dimension(node,"rx",width,&font,true);
            let ry=dimension(node,"ry",height,&font,true);
            let rx=rx.or(ry).unwrap_or(0.0);let ry=ry.unwrap_or(rx);
            if rx == 0.0 || ry == 0.0 { return 4.0*rx.max(ry); }
            arc_length(Arc::new(Point::ZERO,(rx,ry),0.0,TAU,0.0))
        }
        "rect" => {
            let w=dim("width",width);let h=dim("height",height);
            let rx=dimension(node,"rx",width,&font,true);
            let ry=dimension(node,"ry",height,&font,true);
            let x=rx.or(ry).unwrap_or(0.0).min(w/2.0);
            let y=ry.or(rx).unwrap_or(0.0).min(h/2.0);
            if x == 0.0 || y == 0.0 { return 2.0*(w+h); }
            2.0*(w+h-2.0*x-2.0*y)+arc_length(Arc::new(Point::ZERO,(x,y),0.0,TAU,0.0))
        }
        _ => unreachable!("SVG geometry receiver was validated"),
    }
}

#[op2(fast)]
fn op_svg_geometry_length(scope: &mut v8::PinScope, state: &OpState,
    receiver: v8::Local<v8::Object>, nid: u32) -> Result<f64,JsErrorBox> {
    let context=receiver.get_creation_context(scope).ok_or_else(|| JsErrorBox::type_error("Illegal invocation"))?;
    if !receiver.strict_equals(context.global(scope).into()) { return Err(JsErrorBox::type_error("Illegal invocation")); }
    let context=v8::Global::new(scope,context);
    let (_,_,owner)=crate::window::context_document(state,&context)?;
    let owner=owner.borrow();
    let tree=owner.dom.as_ref().ok_or_else(|| JsErrorBox::type_error("SVG document is unavailable"))?;
    tree.with_node(NodeId::new(nid), |node| {
        let name=node.as_element().ok_or_else(|| JsErrorBox::type_error("Illegal invocation"))?;
        let tag=&*name.local;
        if &*name.ns != "http://www.w3.org/2000/svg" || !matches!(tag,"path"|"line"|"polyline"|"polygon"|"rect"|"circle"|"ellipse") {
            return Err(JsErrorBox::type_error("Illegal invocation"));
        }
        if !node.connected && tag != "path" {
            return Err(JsErrorBox::new("DOMExceptionInvalidStateError","This element is non-rendered element."));
        }
        let viewport=(f64::from(owner.viewport.0),f64::from(owner.viewport.1));
        let result=std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| geometry(&owner,tree,node,tag,viewport)))
            .map_err(|_|JsErrorBox::generic("SVG geometry calculation failed"))?;
        if !result.is_finite() { return Err(JsErrorBox::range_error("SVG geometry exceeds the finite numeric range")); }
        Ok(result)
    }).unwrap_or_else(|| Err(JsErrorBox::type_error("SVG node is unavailable")))
}

pub(crate) fn ops() -> Vec<OpDecl> { vec![op_svg_geometry_length()] }
