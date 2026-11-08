//! Mutable XML DOM for stream descriptions.
//! Provides a pugixml-compatible tree structure used by StreamInfo's `<desc>` element.

use parking_lot::Mutex;
use std::sync::Arc;

/// Internal node data
#[derive(Debug)]
struct NodeData {
    name: String,
    value: String,
    children: Vec<XmlNode>,
    parent: Option<XmlNodeWeak>,
}

type XmlNodeInner = Arc<Mutex<NodeData>>;
type XmlNodeWeak = std::sync::Weak<Mutex<NodeData>>;

/// An XML element node handle. Cloning is cheap (Arc).
#[derive(Debug, Clone)]
pub struct XmlNode {
    inner: XmlNodeInner,
}

/// A null/empty sentinel node
static EMPTY_NODE: once_cell::sync::Lazy<XmlNode> = once_cell::sync::Lazy::new(|| XmlNode {
    inner: Arc::new(Mutex::new(NodeData {
        name: String::new(),
        value: String::new(),
        children: Vec::new(),
        parent: None,
    })),
});

impl XmlNode {
    /// Create a new named node
    pub fn new(name: &str) -> Self {
        XmlNode {
            inner: Arc::new(Mutex::new(NodeData {
                name: name.to_string(),
                value: String::new(),
                children: Vec::new(),
                parent: None,
            })),
        }
    }

    /// Create an empty/null node
    pub fn empty() -> Self {
        EMPTY_NODE.clone()
    }

    /// Check if this is the empty sentinel
    pub fn is_empty(&self) -> bool {
        self.inner.lock().name.is_empty()
    }

    /// Get the name of this node
    pub fn name(&self) -> String {
        self.inner.lock().name.clone()
    }

    /// Get the text value of this node
    pub fn value(&self) -> String {
        self.inner.lock().value.clone()
    }

    /// Set the name
    pub fn set_name(&self, name: &str) {
        self.inner.lock().name = name.to_string();
    }

    /// Set the text value
    pub fn set_value(&self, value: &str) {
        self.inner.lock().value = value.to_string();
    }

    /// Append a child element with the given name, return the new child
    pub fn append_child(&self, name: &str) -> XmlNode {
        let child = XmlNode::new(name);
        {
            child.inner.lock().parent = Some(Arc::downgrade(&self.inner));
        }
        self.inner.lock().children.push(child.clone());
        child
    }

    /// Prepend a child element
    pub fn prepend_child(&self, name: &str) -> XmlNode {
        let child = XmlNode::new(name);
        {
            child.inner.lock().parent = Some(Arc::downgrade(&self.inner));
        }
        self.inner.lock().children.insert(0, child.clone());
        child
    }

    /// Append a child element with name and text value
    pub fn append_child_value(&self, name: &str, value: &str) -> XmlNode {
        let child = self.append_child(name);
        child.set_value(value);
        child
    }

    /// Prepend a child element with name and text value
    pub fn prepend_child_value(&self, name: &str, value: &str) -> XmlNode {
        let child = self.prepend_child(name);
        child.set_value(value);
        child
    }

    /// Set the value of a named child (creating it if needed)
    pub fn set_child_value(&self, name: &str, value: &str) -> bool {
        let data = self.inner.lock();
        for child in &data.children {
            if child.name() == name {
                child.set_value(value);
                return true;
            }
        }
        // create it
        drop(data);
        self.append_child_value(name, value);
        true
    }

    /// Get a child by name
    pub fn child(&self, name: &str) -> XmlNode {
        let data = self.inner.lock();
        for child in &data.children {
            if child.name() == name {
                return child.clone();
            }
        }
        XmlNode::empty()
    }

    /// Get the text value of a named child
    pub fn child_value(&self, name: &str) -> String {
        let ch = self.child(name);
        if ch.is_empty() {
            String::new()
        } else {
            ch.value()
        }
    }

    /// Get the text content (value of first text child or self value)
    pub fn child_value_self(&self) -> String {
        self.value()
    }

    /// Get the first child
    pub fn first_child(&self) -> XmlNode {
        let data = self.inner.lock();
        data.children
            .first()
            .cloned()
            .unwrap_or_else(XmlNode::empty)
    }

    /// Get the last child
    pub fn last_child(&self) -> XmlNode {
        let data = self.inner.lock();
        data.children.last().cloned().unwrap_or_else(XmlNode::empty)
    }

    /// Get next sibling (requires parent)
    pub fn next_sibling(&self) -> XmlNode {
        self.sibling_offset(1, None)
    }

    /// Get next sibling with a given name
    pub fn next_sibling_named(&self, name: &str) -> XmlNode {
        self.sibling_offset(1, Some(name))
    }

    /// Get previous sibling
    pub fn previous_sibling(&self) -> XmlNode {
        self.sibling_offset(-1, None)
    }

    /// Get previous sibling with a given name
    pub fn previous_sibling_named(&self, name: &str) -> XmlNode {
        self.sibling_offset(-1, Some(name))
    }

    fn sibling_offset(&self, direction: i32, name_filter: Option<&str>) -> XmlNode {
        let parent_weak = {
            let data = self.inner.lock();
            match &data.parent {
                Some(w) => w.clone(),
                None => return XmlNode::empty(),
            }
        };
        let parent = match parent_weak.upgrade() {
            Some(p) => p,
            None => return XmlNode::empty(),
        };
        let parent_data = parent.lock();
        let self_ptr = Arc::as_ptr(&self.inner);
        let mut found_self = false;
        let iter: Box<dyn Iterator<Item = &XmlNode>> = if direction > 0 {
            Box::new(parent_data.children.iter())
        } else {
            Box::new(parent_data.children.iter().rev())
        };
        for child in iter {
            if found_self {
                if let Some(name) = name_filter {
                    if child.name() == name {
                        return child.clone();
                    }
                } else {
                    return child.clone();
                }
            }
            if Arc::as_ptr(&child.inner) == self_ptr {
                found_self = true;
            }
        }
        XmlNode::empty()
    }

    /// Get parent
    pub fn parent(&self) -> XmlNode {
        let data = self.inner.lock();
        match &data.parent {
            Some(w) => match w.upgrade() {
                Some(p) => XmlNode { inner: p },
                None => XmlNode::empty(),
            },
            None => XmlNode::empty(),
        }
    }

    /// Remove a child by name
    pub fn remove_child_named(&self, name: &str) {
        let mut data = self.inner.lock();
        data.children.retain(|c| c.name() != name);
    }

    /// Remove a specific child node
    pub fn remove_child(&self, child: &XmlNode) {
        let child_ptr = Arc::as_ptr(&child.inner);
        let mut data = self.inner.lock();
        data.children.retain(|c| Arc::as_ptr(&c.inner) != child_ptr);
    }

    /// Deep clone (copy) of this subtree
    pub fn deep_clone(&self) -> XmlNode {
        let data = self.inner.lock();
        let new_node = XmlNode::new(&data.name);
        new_node.set_value(&data.value);
        for child in &data.children {
            let child_clone = child.deep_clone();
            child_clone.inner.lock().parent = Some(Arc::downgrade(&new_node.inner));
            new_node.inner.lock().children.push(child_clone);
        }
        new_node
    }

    /// Append a copy of another subtree
    pub fn append_copy(&self, other: &XmlNode) -> XmlNode {
        let copy = other.deep_clone();
        copy.inner.lock().parent = Some(Arc::downgrade(&self.inner));
        self.inner.lock().children.push(copy.clone());
        copy
    }

    /// Prepend a copy of another subtree
    pub fn prepend_copy(&self, other: &XmlNode) -> XmlNode {
        let copy = other.deep_clone();
        copy.inner.lock().parent = Some(Arc::downgrade(&self.inner));
        self.inner.lock().children.insert(0, copy.clone());
        copy
    }

    /// Serialize this subtree to XML string
    pub fn to_xml(&self) -> String {
        let mut out = String::new();
        self.write_xml(&mut out, 0);
        out
    }

    fn write_xml(&self, out: &mut String, _depth: usize) {
        let data = self.inner.lock();
        if data.name.is_empty() {
            return;
        }
        out.push('<');
        out.push_str(&data.name);
        out.push('>');
        if !data.value.is_empty() {
            xml_escape_into(out, &data.value);
        }
        for child in &data.children {
            child.write_xml(out, _depth + 1);
        }
        out.push_str("</");
        out.push_str(&data.name);
        out.push('>');
    }

    /// Check pointer identity
    pub fn same_as(&self, other: &XmlNode) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

fn xml_escape_into(out: &mut String, s: &str) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
}

pub fn xml_escape(s: &str) -> String {
    let mut out = String::new();
    xml_escape_into(&mut out, s);
    out
}

/// Decode the predefined entities and numeric character references in one pass (so
/// `&amp;lt;` stays the text `&lt;`); anything else is kept as written.
pub fn xml_unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(k) = rest.find('&') {
        out.push_str(&rest[..k]);
        rest = &rest[k..];
        let decoded = rest.find(';').filter(|&e| e <= 12).and_then(|e| {
            let c = match &rest[1..e] {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                n if n.starts_with("#x") || n.starts_with("#X") => u32::from_str_radix(&n[2..], 16)
                    .ok()
                    .and_then(char::from_u32),
                n if n.starts_with('#') => n[1..].parse().ok().and_then(char::from_u32),
                _ => None,
            }?;
            Some((c, e))
        });
        match decoded {
            Some((c, e)) => {
                out.push(c);
                rest = &rest[e + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

impl XmlNode {
    /// Parse an XML fragment (elements and text, as [`XmlNode::to_xml`] writes them and liblsl's
    /// pugixml sends them, indented or not) and append its elements as children of this node.
    /// Text directly inside an element becomes its value; attributes, comments and processing
    /// instructions are skipped; parsing stops quietly at malformed input.
    pub fn append_parsed(&self, xml: &str) {
        let mut p = FragmentParser { s: xml, i: 0 };
        p.content(self, None);
    }
}

struct FragmentParser<'a> {
    s: &'a str,
    i: usize,
}

impl<'a> FragmentParser<'a> {
    /// Children and text of `parent` up to `</end>` (or the end of the input).
    fn content(&mut self, parent: &XmlNode, end: Option<&str>) {
        let mut text = String::new();
        let src = self.s;
        while self.i < src.len() {
            let r = &src[self.i..];
            if let Some(after) = r.strip_prefix("</") {
                let close = after.find('>').unwrap_or(after.len());
                let name = after[..close].trim();
                self.i += 2 + (close + 1).min(after.len());
                if end.is_none_or(|e| e == name) {
                    break;
                }
            } else if r.starts_with("<!--") {
                self.i += r.find("-->").map_or(r.len(), |k| k + 3);
            } else if let Some(cdata) = r.strip_prefix("<![CDATA[") {
                let k = cdata.find("]]>").unwrap_or(cdata.len());
                text.push_str(&cdata[..k]);
                self.i += 9 + (k + 3).min(cdata.len());
            } else if r.starts_with("<?") || r.starts_with("<!") {
                self.i += r.find('>').map_or(r.len(), |k| k + 1);
            } else if r.starts_with('<') {
                let close = r.find('>').unwrap_or(r.len());
                let tag = &r[1..close];
                self.i += (close + 1).min(r.len());
                let empty = tag.ends_with('/');
                let tag = tag.trim_end_matches('/');
                let name = tag.split_whitespace().next().unwrap_or("");
                if name.is_empty() {
                    continue;
                }
                let child = parent.append_child(name);
                if !empty {
                    self.content(&child, Some(name));
                }
            } else {
                let k = r.find('<').unwrap_or(r.len());
                text.push_str(&xml_unescape(&r[..k]));
                self.i += k;
            }
        }
        let t = text.trim();
        if !t.is_empty() && end.is_some() {
            parent.set_value(t);
        }
    }
}
