use crate::tree::{DomTree, NodeData, NodeId};

// A unit of pending serialization work. Held on an explicit heap stack instead
// of the call stack so a deeply nested tree cannot overflow the thread stack and
// abort the process (a stack overflow is a hard abort that `op_dom`'s
// catch_unwind cannot recover). `descendants()` is iterative + capped for the
// same reason; the serializer must be too.
enum SerializeWork {
    // Serialize this node; `include_self` mirrors the old recursive param.
    Node(NodeId, bool),
    // Emit a previously-opened element's closing tag, after its children.
    CloseTag(String),
}

impl DomTree {
    pub fn outer_html(&self, node_id: NodeId) -> String {
        if self.is_xml_node(node_id) { return self.serialize_xml(node_id); }
        let mut buf = String::new();
        self.serialize_worklist(vec![SerializeWork::Node(node_id, true)], &mut buf);
        buf
    }

    pub fn inner_html(&self, node_id: NodeId) -> String {
        if self.is_xml_node(node_id) {
            return self.children(node_id).into_iter().map(|child| self.serialize_xml(child)).collect();
        }
        let mut buf = String::new();
        self.serialize_worklist(self.child_work(self.content_source(node_id)), &mut buf);
        buf
    }

    // The node whose children represent `node_id`'s markup. For a <template>
    // that is its contents document, since the parser puts template children
    // there rather than under the element (issue #463); for everything else it
    // is the node itself.
    fn content_source(&self, node_id: NodeId) -> NodeId {
        self.with_node(node_id, |n| match &n.data {
            NodeData::Element { template_contents: Some(contents), .. } => Some(*contents),
            _ => None,
        })
        .flatten()
        .unwrap_or(node_id)
    }

    // Children as work items in document order (top of the LIFO stack first).
    fn child_work(&self, node_id: NodeId) -> Vec<SerializeWork> {
        self.children(node_id)
            .into_iter()
            .rev()
            .map(|c| SerializeWork::Node(c, true))
            .collect()
    }

    fn serialize_worklist(&self, mut stack: Vec<SerializeWork>, buf: &mut String) {
        // Defense in depth, mirroring descendants(): a well-formed subtree emits
        // at most one Node plus one CloseTag per node, so 2*nodes.len() work
        // items bound a valid walk. Exceeding it means the graph is cyclic (the
        // append_child / insert_before guards prevent that); stop rather than
        // spin forever. On a valid tree this bound is never reached.
        let max_steps = self
            .node_slot_count()
            .saturating_mul(2)
            .saturating_add(16);
        let mut steps = 0usize;

        while let Some(work) = stack.pop() {
            steps += 1;
            if steps > max_steps {
                eprintln!("obscura: serialize worklist cap hit - tree has a cycle");
                break;
            }

            let (node_id, include_self) = match work {
                SerializeWork::CloseTag(tag) => {
                    buf.push_str("</");
                    buf.push_str(&tag);
                    buf.push('>');
                    continue;
                }
                SerializeWork::Node(node_id, include_self) => (node_id, include_self),
            };

            let node = match self.get_node(node_id) {
                Some(n) => n,
                None => continue,
            };

            match &node.data {
                NodeData::Document => {
                    for w in self.child_work(node_id) {
                        stack.push(w);
                    }
                }
                NodeData::Doctype { name, .. } => {
                    buf.push_str("<!DOCTYPE ");
                    buf.push_str(name);
                    buf.push('>');
                }
                NodeData::Element { name, attrs, template_contents, .. } => {
                    let tag = name.local.as_ref();
                    if include_self {
                        buf.push('<');
                        buf.push_str(tag);
                        for attr in attrs {
                            buf.push(' ');
                            if let Some(prefix) = &attr.name.prefix {
                                buf.push_str(prefix);
                                buf.push(':');
                            }
                            buf.push_str(attr.name.local.as_ref());
                            buf.push_str("=\"");
                            escape_attr(&attr.value, buf);
                            buf.push('"');
                        }
                        buf.push('>');
                    }

                    if !is_void_element(tag) {
                        // Push the closing tag first so it pops after all the
                        // children we push next.
                        if include_self {
                            stack.push(SerializeWork::CloseTag(tag.to_string()));
                        }
                        // A <template> serializes its contents document, not its
                        // own (always empty) children (issue #463). template_contents
                        // is already in hand from this node, so read it directly
                        // rather than re-fetching and cloning the node.
                        let source = template_contents.unwrap_or(node_id);
                        for w in self.child_work(source) {
                            stack.push(w);
                        }
                    }
                }
                NodeData::Text { contents } => {
                    let parent_is_raw = node.parent
                        .and_then(|pid| {
                            self.with_node(pid, |p| {
                                p.as_element()
                                    .map(|name| is_raw_text_element(name.local.as_ref()))
                                    .unwrap_or(false)
                            })
                        })
                        .unwrap_or(false);

                    if parent_is_raw {
                        buf.push_str(contents);
                    } else {
                        escape_text(contents, buf);
                    }
                }
                NodeData::Comment { contents } => {
                    buf.push_str("<!--");
                    // The HTML parser can never produce a comment that closes early,
                    // but script can via document.createComment(...). The tokenizer
                    // ends a comment on ANY of four sequences: "-->", "--!>", a
                    // leading ">", or a leading "->" (comment-start / -start-dash
                    // abrupt-close). Every one requires a ">". Emitting it verbatim
                    // would close the comment early and let the trailing text parse
                    // as live markup (mXSS). Entities are not decoded inside
                    // comments, so escaping every ">" to "&gt;" neutralizes all four
                    // forms at once and keeps the data as a single comment.
                    // (Supersedes the earlier "-->"-only guard, which left the
                    // leading-">", leading-"->", and "--!>" forms exploitable.)
                    if contents.contains('>') {
                        buf.push_str(&contents.replace('>', "&gt;"));
                    } else {
                        buf.push_str(contents);
                    }
                    buf.push_str("-->");
                }
                NodeData::ProcessingInstruction { target, data } => {
                    buf.push_str("<?");
                    buf.push_str(target);
                    buf.push(' ');
                    buf.push_str(data);
                    buf.push('>');
                }
            }
        }
    }
}

enum XmlWork {
    Node(NodeId),
    Close(String, usize),
}

fn xml_binding<'a>(bindings: &'a [(String, String)], prefix: &str) -> Option<&'a str> {
    bindings.iter().rev().find(|(p, _)| p == prefix).map(|(_, uri)| uri.as_str())
}

fn xml_declare(bindings: &mut Vec<(String, String)>, declarations: &mut Vec<(String, String)>, prefix: &str, uri: &str) {
    if xml_binding(bindings, prefix).unwrap_or("") == uri { return; }
    if let Some((_, value)) = declarations.iter_mut().find(|(p, _)| p == prefix) { *value = uri.into(); }
    else { declarations.push((prefix.into(), uri.into())); }
    bindings.push((prefix.into(), uri.into()));
}

impl DomTree {
    /// XML serialization is namespace-aware even for a node imported from HTML.
    /// Scope restoration and node traversal use a bounded explicit work stack.
    pub fn serialize_xml(&self, root: NodeId) -> String {
        let mut output = String::new();
        let mut bindings = vec![("xml".into(), "http://www.w3.org/XML/1998/namespace".into())];
        let mut stack = vec![XmlWork::Node(root)];
        let mut steps = self.node_slot_count().saturating_mul(2).saturating_add(1);
        let mut generated = 0usize;
        while let Some(work) = stack.pop() {
            if steps == 0 {
                eprintln!("obscura: XML serialize worklist cap hit - tree has a cycle");
                break;
            }
            steps -= 1;
            let id = match work {
                XmlWork::Close(name, scope) => {
                    output.push_str("</"); output.push_str(&name); output.push('>');
                    bindings.truncate(scope);
                    continue;
                }
                XmlWork::Node(id) => id,
            };
            let Some(node) = self.borrow_node(id) else { continue };
            match &node.data {
                NodeData::Document => {
                    for child in self.children(id).into_iter().rev() { stack.push(XmlWork::Node(child)); }
                }
                NodeData::Element { name, attrs, .. } => {
                    let scope = bindings.len();
                    let mut declarations = Vec::new();
                    for attr in attrs.iter().filter(|attr| &*attr.name.ns == "http://www.w3.org/2000/xmlns/") {
                        let prefix = if attr.name.prefix.is_some() { &*attr.name.local } else { "" };
                        if prefix != "xml" { xml_declare(&mut bindings, &mut declarations, prefix, &attr.value); }
                    }
                    let prefix = name.prefix.as_ref().map_or("", |prefix| &**prefix);
                    xml_declare(&mut bindings, &mut declarations, prefix, &name.ns);
                    let tag = if prefix.is_empty() { name.local.to_string() } else { format!("{}:{}", prefix, name.local) };
                    let mut attributes = Vec::with_capacity(attrs.len());
                    for attr in attrs.iter().filter(|attr| &*attr.name.ns != "http://www.w3.org/2000/xmlns/") {
                        let mut prefix = attr.name.prefix.as_ref().map_or_else(String::new, |prefix| prefix.to_string());
                        if !attr.name.ns.is_empty() {
                            if prefix.is_empty() || xml_binding(&bindings, &prefix).is_some_and(|uri| uri != &*attr.name.ns) {
                                if let Some((existing, _)) = bindings.iter().rev().find(|(p, uri)| !p.is_empty() && uri == &*attr.name.ns) {
                                    prefix = existing.clone();
                                } else {
                                    loop {
                                        generated += 1;
                                        prefix = format!("ns{}", generated);
                                        if xml_binding(&bindings, &prefix).is_none() { break; }
                                    }
                                }
                            }
                            xml_declare(&mut bindings, &mut declarations, &prefix, &attr.name.ns);
                        }
                        let qualified = if prefix.is_empty() { attr.name.local.to_string() } else { format!("{}:{}", prefix, attr.name.local) };
                        attributes.push((qualified, attr.value.as_str()));
                    }
                    output.push('<'); output.push_str(&tag);
                    for (prefix, uri) in declarations {
                        output.push_str(" xmlns");
                        if !prefix.is_empty() { output.push(':'); output.push_str(&prefix); }
                        output.push_str("=\""); escape_xml_attr(&uri, &mut output); output.push('"');
                    }
                    for (name, value) in attributes {
                        output.push(' '); output.push_str(&name); output.push_str("=\"");
                        escape_xml_attr(value, &mut output); output.push('"');
                    }
                    let children = self.children(id);
                    if children.is_empty() {
                        output.push_str("/>"); bindings.truncate(scope);
                    } else {
                        output.push('>'); stack.push(XmlWork::Close(tag, scope));
                        for child in children.into_iter().rev() { stack.push(XmlWork::Node(child)); }
                    }
                }
                NodeData::Text { contents } => {
                    if self.is_cdata(id) {
                        // XMLSerializer uses the non-well-formed serialization mode.
                        output.push_str("<![CDATA["); output.push_str(contents); output.push_str("]]>");
                    } else {
                        for part in contents.split_inclusive('\r') {
                            if let Some(text) = part.strip_suffix('\r') { escape_text(text, &mut output); output.push_str("&#13;"); }
                            else { escape_text(part, &mut output); }
                        }
                    }
                }
                NodeData::Comment { contents } => {
                    output.push_str("<!--"); output.push_str(contents); output.push_str("-->");
                }
                NodeData::ProcessingInstruction { target, data } => {
                    output.push_str("<?"); output.push_str(target);
                    if !data.is_empty() { output.push(' '); output.push_str(data); }
                    output.push_str("?>");
                }
                NodeData::Doctype { name, public_id, system_id } => {
                    output.push_str("<!DOCTYPE "); output.push_str(name);
                    if !public_id.is_empty() { output.push_str(" PUBLIC \""); output.push_str(public_id); output.push('"'); }
                    if !system_id.is_empty() {
                        output.push_str(if public_id.is_empty() { " SYSTEM \"" } else { " \"" });
                        output.push_str(system_id); output.push('"');
                    }
                    output.push('>');
                }
            }
        }
        output
    }
}

fn escape_xml_attr(value: &str, output: &mut String) {
    for c in value.chars() {
        match c {
            '&' => output.push_str("&amp;"), '<' => output.push_str("&lt;"),
            '"' => output.push_str("&quot;"), '\t' => output.push_str("&#9;"),
            '\n' => output.push_str("&#10;"), '\r' => output.push_str("&#13;"),
            c => output.push(c),
        }
    }
}

fn escape_text(s: &str, buf: &mut String) {
    for c in s.chars() {
        match c {
            '&' => buf.push_str("&amp;"),
            '<' => buf.push_str("&lt;"),
            '>' => buf.push_str("&gt;"),
            _ => buf.push(c),
        }
    }
}

fn escape_attr(s: &str, buf: &mut String) {
    for c in s.chars() {
        match c {
            '&' => buf.push_str("&amp;"),
            '"' => buf.push_str("&quot;"),
            _ => buf.push(c),
        }
    }
}

fn is_void_element(tag: &str) -> bool {
    matches!(
        tag,
        "area" | "base" | "br" | "col" | "embed" | "hr" | "img" | "input" | "link" | "meta"
            | "param" | "source" | "track" | "wbr"
    )
}

fn is_raw_text_element(tag: &str) -> bool {
    matches!(tag, "script" | "style" | "textarea" | "title")
}

#[cfg(test)]
mod tests {
    use html5ever::{LocalName, QualName};

    use crate::tree::{NodeData, ShadowRootMode};
    use crate::tree_sink::parse_html;

    #[test]
    fn test_outer_html() {
        let tree = parse_html(r#"<div id="test"><p>Hello</p></div>"#);
        let div = tree.get_element_by_id("test").unwrap();
        let html = tree.outer_html(div);
        assert!(html.contains(r#"<div id="test">"#));
        assert!(html.contains("<p>Hello</p>"));
        assert!(html.contains("</div>"));
    }

    #[test]
    fn test_inner_html() {
        let tree = parse_html(r#"<div id="test"><p>Hello</p><p>World</p></div>"#);
        let div = tree.get_element_by_id("test").unwrap();
        let html = tree.inner_html(div);
        assert!(html.contains("<p>Hello</p>"));
        assert!(html.contains("<p>World</p>"));
        assert!(!html.contains("<div"));
    }

    #[test]
    fn test_serialize_attributes() {
        let tree = parse_html(r#"<a href="https://example.com" class="link">Click</a>"#);
        let a = tree.query_selector("a").unwrap().unwrap();
        let html = tree.outer_html(a);
        assert!(html.contains("href=\"https://example.com\""));
        assert!(html.contains("class=\"link\""));
    }

    #[test]
    fn test_serialize_special_chars() {
        let tree = parse_html("<p>Hello &amp; World &lt;3</p>");
        let p = tree.query_selector("p").unwrap().unwrap();
        let html = tree.outer_html(p);
        assert!(html.contains("&amp;"));
        assert!(html.contains("&lt;"));
    }

    #[test]
    fn test_void_elements() {
        let tree = parse_html(r#"<img src="test.png"><br>"#);
        let img = tree.query_selector("img").unwrap().unwrap();
        let html = tree.outer_html(img);
        assert!(html.contains("<img"));
        assert!(!html.contains("</img>"));
    }

    #[test]
    fn comment_serialization_neutralizes_all_terminator_forms() {
        use crate::tree::NodeData;

        // A comment ends on any of "-->", "--!>", a leading ">", or a leading
        // "->". `document.createComment(...)` accepts arbitrary strings, so a
        // scripted comment can carry each closing form followed by a real tag.
        // Serializing must keep the payload inside a single comment; if it
        // closes early, the trailing "<img>" becomes live markup (mXSS).
        let payloads = [
            "><img src=x onerror=alert(1)>", // leading ">" abrupt-closes empty comment
            "-><img src=x>",                 // leading "->" abrupt-closes
            "a--!><img src=x>",              // internal "--!>" closes (incorrectly-closed)
            "a--><img src=x>",               // internal "-->" closes (the previously-fixed form)
        ];

        for payload in payloads {
            let tree = parse_html(r#"<div id="host"></div>"#);
            let host = tree.get_element_by_id("host").unwrap();
            let comment = tree.new_node(NodeData::Comment { contents: payload.to_string() });
            tree.append_child(host, comment);

            let serialized = tree.outer_html(host);

            // Re-parsing the serialized markup must not surface an <img>: the
            // payload has to stay inside the comment.
            let reparsed = parse_html(&serialized);
            assert!(
                reparsed.query_selector("img").unwrap().is_none(),
                "payload {payload:?} escaped the comment; serialized = {serialized}"
            );
            // And the serialized comment data must carry no raw ">".
            let inner = &serialized[serialized.find("<!--").unwrap() + 4..];
            let inner = &inner[..inner.find("-->").unwrap()];
            assert!(
                !inner.contains('>'),
                "comment data still contains a raw '>': {serialized}"
            );
        }
    }

    #[test]
    fn host_serialization_excludes_its_native_shadow_tree() {
        let tree = parse_html(r#"<x-card id="host"><span id="light">light</span></x-card>"#);
        let host = tree.get_element_by_id("host").unwrap();
        let root = tree.attach_shadow_root(host, ShadowRootMode::Open).unwrap();
        let shadow = tree.new_node(NodeData::Element {
            name: QualName::new(None, ns!(html), LocalName::from("strong")),
            attrs: Vec::new(),
            template_contents: None,
            mathml_annotation_xml_integration_point: false,
        });
        let shadow_text = tree.new_node(NodeData::Text {
            contents: "shadow".to_string(),
        });
        tree.append_child(root, shadow);
        tree.append_child(shadow, shadow_text);

        assert_eq!(tree.inner_html(host), r#"<span id="light">light</span>"#);
        assert_eq!(
            tree.outer_html(host),
            r#"<x-card id="host"><span id="light">light</span></x-card>"#
        );
        assert_eq!(tree.inner_html(root), "<strong>shadow</strong>");
    }
}
