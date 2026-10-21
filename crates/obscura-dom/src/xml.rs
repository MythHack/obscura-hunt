use crate::{Attribute, DomTree, NodeData, NodeId};
use html5ever::{LocalName, Namespace, Prefix, QualName};

const MAX_XML_BYTES: usize = 16 * 1024 * 1024;
const MAX_XML_NODES: u32 = 100_000;
const MAX_XML_DEPTH: usize = 512;

fn qualified(namespace: &str, name: &str) -> QualName {
    let (prefix, local) = name.split_once(':')
        .map_or((None, name), |(p, l)| (Some(Prefix::from(p)), l));
    QualName::new(prefix, Namespace::from(namespace), LocalName::from(local))
}

// Only called after roxmltree validates the entire document with DTD disabled.
// Character references are decoded after literal newline/attribute normalization.
fn decoded(mut source: &str, attribute: bool) -> Result<String, String> {
    let mut result = String::with_capacity(source.len());
    while let Some(c) = source.chars().next() {
        source = &source[c.len_utf8()..];
        match c {
            '&' => {
                let end = source.find(';').ok_or("Unclosed XML character reference")?;
                let reference = &source[..end];
                let value = match reference {
                    "amp" => Some('&'), "lt" => Some('<'), "gt" => Some('>'),
                    "apos" => Some('\''), "quot" => Some('"'),
                    r if r.starts_with("#x") => u32::from_str_radix(&r[2..], 16).ok().and_then(char::from_u32),
                    r if r.starts_with('#') => r[1..].parse::<u32>().ok().and_then(char::from_u32),
                    _ => None,
                }.ok_or("Unsupported XML entity reference")?;
                result.push(value);
                source = &source[end + 1..];
            }
            '\r' => {
                if source.starts_with('\n') { source = &source[1..]; }
                result.push(if attribute { ' ' } else { '\n' });
            }
            '\n' | '\t' if attribute => result.push(' '),
            c => result.push(c),
        }
    }
    Ok(result)
}

fn normalized_lines(source: &str) -> String {
    let mut result = String::with_capacity(source.len());
    let mut chars = source.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\r' {
            if chars.peek() == Some(&'\n') { chars.next(); }
            result.push('\n');
        } else { result.push(c); }
    }
    result
}

// roxmltree's tokenizer descends recursively. Bound lexical nesting before it
// sees untrusted input, skipping quoted regions and non-element XML tokens.
fn preflight(source: &str) -> Result<(), String> {
    let mut tail = source;
    let mut depth = 0usize;
    while let Some(start) = tail.find('<') {
        tail = &tail[start..];
        let special = if tail.starts_with("<!--") { Some((4, "-->")) }
            else if tail.starts_with("<![CDATA[") { Some((9, "]]>")) }
            else if tail.starts_with("<?") { Some((2, "?>")) } else { None };
        if let Some((skip, end)) = special {
            let length = tail[skip..].find(end).ok_or("Unclosed XML token")? + skip + end.len();
            tail = &tail[length..];
            continue;
        }
        if tail.starts_with("<!") { return Err("XML DTD and entity declarations are disabled".into()); }
        let mut quote = None;
        let mut end = None;
        for (index, byte) in tail.bytes().enumerate().skip(1) {
            if let Some(active) = quote {
                if byte == active { quote = None; }
            } else if byte == b'\'' || byte == b'"' { quote = Some(byte); }
            else if byte == b'>' { end = Some(index); break; }
        }
        let end = end.ok_or("Unclosed XML tag")?;
        if tail.starts_with("</") { depth = depth.saturating_sub(1); }
        else if tail.as_bytes().get(end.wrapping_sub(1)) != Some(&b'/') {
            depth += 1;
            if depth > MAX_XML_DEPTH { return Err("XML nesting exceeds the depth limit".into()); }
        }
        tail = &tail[end + 1..];
    }
    Ok(())
}

impl DomTree {
    /// Parse an inert XML document into an empty native Document arena node.
    /// DTDs (including external subsets) are rejected, never fetched or expanded.
    pub fn parse_xml_document(&self, document: NodeId, source: &str) -> Result<(), String> {
        if source.len() > MAX_XML_BYTES { return Err("XML input exceeds the byte limit".into()); }
        if !self.with_node(document, |node| node.is_document()).unwrap_or(false)
            || !self.children(document).is_empty() {
            return Err("XML destination must be an empty Document".into());
        }
        preflight(source)?;
        let parsed = roxmltree::Document::parse_with_options(source, roxmltree::ParsingOptions {
            allow_dtd: false, nodes_limit: MAX_XML_NODES, ..Default::default()
        }).map_err(|error| match error {
            roxmltree::Error::UnexpectedCloseTag(expected, actual, position) =>
                format!("opening and ending tag mismatch: {} and {} at {}", expected, actual, position),
            error => error.to_string(),
        })?;
        // roxmltree merges adjacent CDATA/text. Walk the validated source to keep
        // those DOM node boundaries, using its resolved element/attribute names.
        self.mark_xml_document(document);
        let mut elements = parsed.descendants().filter(|node| node.is_element());
        let mut parents = vec![document];
        let mut created = 1u32;
        let mut pos = usize::from(source.starts_with('\u{feff}')) * '\u{feff}'.len_utf8();
        while pos < source.len() {
            let tail = &source[pos..];
            let parent = *parents.last().ok_or("XML parent stack is empty")?;
            let (node, consumed) = if tail.starts_with("<!--") {
                let end = tail.find("-->").ok_or("Unclosed XML comment")?;
                (Some(self.new_node(NodeData::Comment { contents: normalized_lines(&tail[4..end]) })), end + 3)
            } else if tail.starts_with("<![CDATA[") {
                let end = tail.find("]]>").ok_or("Unclosed CDATA section")?;
                let text = normalized_lines(&tail[9..end]);
                (Some(self.create_cdata(text)), end + 3)
            } else if tail.starts_with("<?") {
                let end = tail.find("?>").ok_or("Unclosed processing instruction")?;
                let body = &tail[2..end];
                let split = body.find(|c| matches!(c, ' ' | '\t' | '\r' | '\n')).unwrap_or(body.len());
                let target = &body[..split];
                let node = if target == "xml" { None } else {
                    Some(self.new_node(NodeData::ProcessingInstruction {
                        target: target.into(), data: normalized_lines(body[split..].trim_start_matches(|c| matches!(c, ' ' | '\t' | '\r' | '\n'))),
                    }))
                };
                (node, end + 2)
            } else if tail.starts_with("</") {
                parents.pop();
                (None, tail.find('>').ok_or("Unclosed XML end tag")? + 1)
            } else if tail.starts_with('<') {
                let element = elements.next().ok_or("Missing validated XML element")?;
                let name_end = tail[1..].find(|c: char| c.is_whitespace() || c == '/' || c == '>')
                    .map(|end| end + 1).ok_or("Unclosed XML start tag")?;
                let name = qualified(element.tag_name().namespace().unwrap_or(""), &tail[1..name_end]);
                let mut attrs = Vec::with_capacity(element.attributes().len() + 1);
                let mut validated_attrs = element.attributes();
                let bytes = tail.as_bytes();
                let mut cursor = name_end;
                loop {
                    while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) { cursor += 1; }
                    if matches!(bytes.get(cursor), Some(b'/') | Some(b'>')) { break; }
                    let start = cursor;
                    while bytes.get(cursor).is_some_and(|c| !c.is_ascii_whitespace() && *c != b'=') { cursor += 1; }
                    let qname = &tail[start..cursor];
                    while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) { cursor += 1; }
                    cursor += 1; // '='
                    while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) { cursor += 1; }
                    let quote = *bytes.get(cursor).ok_or("Missing XML attribute quote")?;
                    cursor += 1;
                    let value_start = cursor;
                    while bytes.get(cursor).is_some_and(|c| *c != quote) { cursor += 1; }
                    let (namespace, value) = if qname == "xmlns" || qname.starts_with("xmlns:") {
                        ("http://www.w3.org/2000/xmlns/", decoded(&tail[value_start..cursor], true)?)
                    } else {
                        let attr = validated_attrs.next().ok_or("Missing validated XML attribute")?;
                        (attr.namespace().unwrap_or(""), attr.value().to_owned())
                    };
                    attrs.push(Attribute { name: qualified(namespace, qname), value });
                    cursor += 1;
                }
                let empty = bytes.get(cursor) == Some(&b'/');
                let consumed = cursor + if empty { 2 } else { 1 };
                let node = self.new_node(NodeData::Element {
                    name, attrs, template_contents: None, mathml_annotation_xml_integration_point: false,
                });
                if !empty { parents.push(node); }
                (Some(node), consumed)
            } else {
                let end = tail.find('<').unwrap_or(tail.len());
                let node = if parents.len() > 1 && end != 0 {
                    Some(self.new_node(NodeData::Text { contents: decoded(&tail[..end], false)? }))
                } else { None };
                (node, end)
            };
            if let Some(node) = node {
                created += 1;
                if created > MAX_XML_NODES {
                    self.remove(node);
                    for child in self.children(document) { self.remove(child); }
                    return Err("XML input exceeds the node limit".into());
                }
                self.append_child(parent, node);
            }
            pos += consumed;
        }
        Ok(())
    }
}
