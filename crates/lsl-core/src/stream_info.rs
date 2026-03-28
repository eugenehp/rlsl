//! StreamInfo: describes the properties of a data stream.

use crate::config::CONFIG;
use crate::types::*;
use crate::xml_dom::{xml_unescape, XmlNode};
use parking_lot::Mutex;
use std::sync::Arc;

/// Stream information. Describes the properties of a data stream.
/// Shared via Arc so it can be safely passed between outlet/inlet/server components.
#[derive(Clone)]
pub struct StreamInfo {
    inner: Arc<Mutex<StreamInfoData>>,
}

pub struct StreamInfoData {
    // core data fields
    pub name: String,
    pub type_: String,
    pub channel_count: u32,
    pub nominal_srate: f64,
    pub channel_format: ChannelFormat,
    pub source_id: String,
    // network identity
    pub version: i32,
    pub uid: String,
    pub created_at: f64,
    pub session_id: String,
    pub hostname: String,
    pub v4address: String,
    pub v4data_port: u16,
    pub v4service_port: u16,
    pub v6address: String,
    pub v6data_port: u16,
    pub v6service_port: u16,
    // XML description subtree
    pub desc: XmlNode,
}

impl StreamInfo {
    pub fn new(
        name: &str,
        type_: &str,
        channel_count: u32,
        nominal_srate: f64,
        channel_format: ChannelFormat,
        source_id: &str,
    ) -> Self {
        let hostname = hostname::get()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        StreamInfo {
            inner: Arc::new(Mutex::new(StreamInfoData {
                name: name.to_string(),
                type_: type_.to_string(),
                channel_count,
                nominal_srate,
                channel_format,
                source_id: source_id.to_string(),
                version: CONFIG.use_protocol_version,
                uid: uuid::Uuid::new_v4().to_string(),
                created_at: 0.0,
                session_id: CONFIG.session_id.clone(),
                hostname,
                v4address: String::new(),
                v4data_port: 0,
                v4service_port: 0,
                v6address: String::new(),
                v6data_port: 0,
                v6service_port: 0,
                desc: XmlNode::new("desc"),
            })),
        }
    }

    // === Accessors ===

    pub fn name(&self) -> String {
        self.inner.lock().name.clone()
    }
    pub fn type_(&self) -> String {
        self.inner.lock().type_.clone()
    }
    pub fn channel_count(&self) -> u32 {
        self.inner.lock().channel_count
    }
    pub fn nominal_srate(&self) -> f64 {
        self.inner.lock().nominal_srate
    }
    pub fn channel_format(&self) -> ChannelFormat {
        self.inner.lock().channel_format
    }
    pub fn source_id(&self) -> String {
        self.inner.lock().source_id.clone()
    }
    pub fn version(&self) -> i32 {
        self.inner.lock().version
    }
    pub fn uid(&self) -> String {
        self.inner.lock().uid.clone()
    }
    pub fn created_at(&self) -> f64 {
        self.inner.lock().created_at
    }
    pub fn session_id(&self) -> String {
        self.inner.lock().session_id.clone()
    }
    pub fn hostname(&self) -> String {
        self.inner.lock().hostname.clone()
    }
    pub fn v4address(&self) -> String {
        self.inner.lock().v4address.clone()
    }
    pub fn v4data_port(&self) -> u16 {
        self.inner.lock().v4data_port
    }
    pub fn v4service_port(&self) -> u16 {
        self.inner.lock().v4service_port
    }
    pub fn v6address(&self) -> String {
        self.inner.lock().v6address.clone()
    }
    pub fn v6data_port(&self) -> u16 {
        self.inner.lock().v6data_port
    }
    pub fn v6service_port(&self) -> u16 {
        self.inner.lock().v6service_port
    }

    pub fn channel_bytes(&self) -> usize {
        self.inner.lock().channel_format.channel_bytes()
    }
    pub fn sample_bytes(&self) -> usize {
        let d = self.inner.lock();
        d.channel_format.channel_bytes() * d.channel_count as usize
    }

    /// Get a clone of the desc node
    pub fn desc(&self) -> XmlNode {
        self.inner.lock().desc.clone()
    }

    // === Setters ===

    pub fn set_uid(&self, uid: &str) {
        self.inner.lock().uid = uid.to_string();
    }
    pub fn set_created_at(&self, t: f64) {
        self.inner.lock().created_at = t;
    }
    pub fn set_session_id(&self, s: &str) {
        self.inner.lock().session_id = s.to_string();
    }
    pub fn set_hostname(&self, s: &str) {
        self.inner.lock().hostname = s.to_string();
    }
    pub fn set_v4address(&self, s: &str) {
        self.inner.lock().v4address = s.to_string();
    }
    pub fn set_v4data_port(&self, p: u16) {
        self.inner.lock().v4data_port = p;
    }
    pub fn set_v4service_port(&self, p: u16) {
        self.inner.lock().v4service_port = p;
    }
    pub fn set_v6address(&self, s: &str) {
        self.inner.lock().v6address = s.to_string();
    }
    pub fn set_v6data_port(&self, p: u16) {
        self.inner.lock().v6data_port = p;
    }
    pub fn set_v6service_port(&self, p: u16) {
        self.inner.lock().v6service_port = p;
    }
    pub fn set_channel_format(&self, fmt: ChannelFormat) {
        self.inner.lock().channel_format = fmt;
    }
    pub fn set_source_id(&self, s: &str) {
        self.inner.lock().source_id = s.to_string();
    }
    pub fn set_version(&self, v: i32) {
        self.inner.lock().version = v;
    }
    pub fn set_name(&self, s: &str) {
        self.inner.lock().name = s.to_string();
    }

    /// Reset the UID to a new random value
    pub fn reset_uid(&self) -> String {
        let uid = uuid::Uuid::new_v4().to_string();
        self.inner.lock().uid = uid.clone();
        uid
    }

    /// Generate the shortinfo XML (without desc contents)
    pub fn to_shortinfo_message(&self) -> String {
        let d = self.inner.lock();
        let mut xml = String::with_capacity(1024);
        xml.push_str("<?xml version=\"1.0\"?>\n<info>");
        append_field(&mut xml, "name", &d.name);
        append_field(&mut xml, "type", &d.type_);
        append_field(&mut xml, "channel_count", &d.channel_count.to_string());
        append_field(&mut xml, "channel_format", d.channel_format.as_str());
        append_field(&mut xml, "source_id", &d.source_id);
        append_field(&mut xml, "nominal_srate", &format!("{}", d.nominal_srate));
        append_field(
            &mut xml,
            "version",
            &format!("{}", d.version as f64 / 100.0),
        );
        append_field(&mut xml, "created_at", &format!("{}", d.created_at));
        append_field(&mut xml, "uid", &d.uid);
        append_field(&mut xml, "session_id", &d.session_id);
        append_field(&mut xml, "hostname", &d.hostname);
        append_field(&mut xml, "v4address", &d.v4address);
        append_field(&mut xml, "v4data_port", &d.v4data_port.to_string());
        append_field(&mut xml, "v4service_port", &d.v4service_port.to_string());
        append_field(&mut xml, "v6address", &d.v6address);
        append_field(&mut xml, "v6data_port", &d.v6data_port.to_string());
        append_field(&mut xml, "v6service_port", &d.v6service_port.to_string());
        xml.push_str("<desc></desc>");
        xml.push_str("</info>\n");
        xml
    }

    /// Generate the full info XML (with desc contents)
    pub fn to_fullinfo_message(&self) -> String {
        let d = self.inner.lock();
        let mut xml = String::with_capacity(2048);
        xml.push_str("<?xml version=\"1.0\"?>\n<info>");
        append_field(&mut xml, "name", &d.name);
        append_field(&mut xml, "type", &d.type_);
        append_field(&mut xml, "channel_count", &d.channel_count.to_string());
        append_field(&mut xml, "channel_format", d.channel_format.as_str());
        append_field(&mut xml, "source_id", &d.source_id);
        append_field(&mut xml, "nominal_srate", &format!("{}", d.nominal_srate));
        append_field(
            &mut xml,
            "version",
            &format!("{}", d.version as f64 / 100.0),
        );
        append_field(&mut xml, "created_at", &format!("{}", d.created_at));
        append_field(&mut xml, "uid", &d.uid);
        append_field(&mut xml, "session_id", &d.session_id);
        append_field(&mut xml, "hostname", &d.hostname);
        append_field(&mut xml, "v4address", &d.v4address);
        append_field(&mut xml, "v4data_port", &d.v4data_port.to_string());
        append_field(&mut xml, "v4service_port", &d.v4service_port.to_string());
        append_field(&mut xml, "v6address", &d.v6address);
        append_field(&mut xml, "v6data_port", &d.v6data_port.to_string());
        append_field(&mut xml, "v6service_port", &d.v6service_port.to_string());
        xml.push_str(&d.desc.to_xml());
        xml.push_str("</info>\n");
        xml
    }

    /// Parse from a shortinfo XML message
    pub fn from_shortinfo_message(xml: &str) -> Option<StreamInfo> {
        let name = extract_tag(xml, "name")?;
        let type_ = extract_tag(xml, "type").unwrap_or_default();
        let channel_count = extract_tag(xml, "channel_count")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let nominal_srate = extract_tag(xml, "nominal_srate")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.0);
        let channel_format = extract_tag(xml, "channel_format")
            .map(|s| ChannelFormat::from_name(&s))
            .unwrap_or(ChannelFormat::Undefined);
        let source_id = extract_tag(xml, "source_id").unwrap_or_default();
        let version = extract_tag(xml, "version")
            .and_then(|s| s.parse::<f64>().ok())
            .map(|v| (v * 100.0) as i32)
            .unwrap_or(LSL_PROTOCOL_VERSION);
        let uid = extract_tag(xml, "uid").unwrap_or_default();
        let created_at = extract_tag(xml, "created_at")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.0);
        let session_id = extract_tag(xml, "session_id").unwrap_or_default();
        let hostname = extract_tag(xml, "hostname").unwrap_or_default();
        let v4address = extract_tag(xml, "v4address").unwrap_or_default();
        let v4data_port = extract_tag(xml, "v4data_port")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let v4service_port = extract_tag(xml, "v4service_port")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let v6address = extract_tag(xml, "v6address").unwrap_or_default();
        let v6data_port = extract_tag(xml, "v6data_port")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let v6service_port = extract_tag(xml, "v6service_port")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        let info = StreamInfo {
            inner: Arc::new(Mutex::new(StreamInfoData {
                name,
                type_,
                channel_count,
                nominal_srate,
                channel_format,
                source_id,
                version,
                uid,
                created_at,
                session_id,
                hostname,
                v4address,
                v4data_port,
                v4service_port,
                v6address,
                v6data_port,
                v6service_port,
                desc: XmlNode::new("desc"),
            })),
        };
        Some(info)
    }

    /// Check if a query string matches this stream info.
    /// Supports simple XPath-like queries: empty always matches,
    /// name='X' / type='X' / source_id='X' etc.
    pub fn matches_query(&self, query: &str) -> bool {
        if query.is_empty() {
            return true;
        }
        let d = self.inner.lock();
        // Support: "clause and clause" with "or" between groups
        // e.g. "name='A' and type='B'" or "name='C'"
        for or_group in query.split(" or ") {
            let or_group = or_group.trim();
            if or_group.is_empty() {
                continue;
            }
            let mut all_match = true;
            for clause in or_group.split(" and ") {
                let clause = clause.trim();
                if clause.is_empty() {
                    continue;
                }
                if !match_clause(clause, &d) {
                    all_match = false;
                    break;
                }
            }
            if all_match {
                return true;
            }
        }
        false
    }

    /// Lock and provide read-only access to inner data
    pub fn with_data<R>(&self, f: impl FnOnce(&StreamInfoData) -> R) -> R {
        f(&self.inner.lock())
    }

    /// Lock and provide mutable access to inner data
    pub fn with_data_mut<R>(&self, f: impl FnOnce(&mut StreamInfoData) -> R) -> R {
        f(&mut self.inner.lock())
    }
}

fn match_clause(clause: &str, d: &StreamInfoData) -> bool {
    let clause = clause.trim();

    // not(...) — negation
    if clause.starts_with("not(") && clause.ends_with(')') {
        let inner = &clause[4..clause.len() - 1];
        return !match_clause(inner, d);
    }

    // starts-with(field,'value')
    if clause.starts_with("starts-with(") && clause.ends_with(')') {
        let inner = &clause[12..clause.len() - 1];
        if let Some(comma) = inner.find(',') {
            let field = inner[..comma].trim();
            let val = inner[comma + 1..]
                .trim()
                .trim_matches('\'')
                .trim_matches('"');
            return get_field(field, d).starts_with(val);
        }
        return true;
    }

    // contains(field,'value')
    if clause.starts_with("contains(") && clause.ends_with(')') {
        let inner = &clause[9..clause.len() - 1];
        if let Some(comma) = inner.find(',') {
            let field = inner[..comma].trim();
            let val = inner[comma + 1..]
                .trim()
                .trim_matches('\'')
                .trim_matches('"');
            return get_field(field, d).contains(val);
        }
        return true;
    }

    // field!='value' — inequality
    if let Some(pos) = clause.find("!=") {
        let field = clause[..pos].trim();
        let val = clause[pos + 2..]
            .trim()
            .trim_matches('\'')
            .trim_matches('"');
        return get_field(field, d) != val;
    }

    // field>='value' — greater-or-equal
    if let Some(pos) = clause.find(">=") {
        let field = clause[..pos].trim();
        let val = clause[pos + 2..]
            .trim()
            .trim_matches('\'')
            .trim_matches('"');
        if let (Ok(fv), Ok(vv)) = (get_field(field, d).parse::<f64>(), val.parse::<f64>()) {
            return fv >= vv;
        }
        return get_field(field, d).as_str() >= val;
    }

    // field<='value'
    if let Some(pos) = clause.find("<=") {
        let field = clause[..pos].trim();
        let val = clause[pos + 2..]
            .trim()
            .trim_matches('\'')
            .trim_matches('"');
        if let (Ok(fv), Ok(vv)) = (get_field(field, d).parse::<f64>(), val.parse::<f64>()) {
            return fv <= vv;
        }
        return get_field(field, d).as_str() <= val;
    }

    // field>'value' — greater
    if let Some(pos) = clause.find('>') {
        if !clause[..pos].contains('=') {
            let field = clause[..pos].trim();
            let val = clause[pos + 1..]
                .trim()
                .trim_matches('\'')
                .trim_matches('"');
            if let (Ok(fv), Ok(vv)) = (get_field(field, d).parse::<f64>(), val.parse::<f64>()) {
                return fv > vv;
            }
        }
    }

    // field<'value' — less
    if let Some(pos) = clause.find('<') {
        if !clause[..pos].contains('=') && !clause[..pos].contains('!') {
            let field = clause[..pos].trim();
            let val = clause[pos + 1..]
                .trim()
                .trim_matches('\'')
                .trim_matches('"');
            if let (Ok(fv), Ok(vv)) = (get_field(field, d).parse::<f64>(), val.parse::<f64>()) {
                return fv < vv;
            }
        }
    }

    // field='value' — equality
    if let Some(eq_pos) = clause.find('=') {
        let field = clause[..eq_pos].trim();
        let val = clause[eq_pos + 1..]
            .trim()
            .trim_matches('\'')
            .trim_matches('"');
        return get_field(field, d) == val;
    }

    true // can't parse: pass
}

fn get_field(field: &str, d: &StreamInfoData) -> String {
    match field {
        "name" => d.name.clone(),
        "type" => d.type_.clone(),
        "source_id" => d.source_id.clone(),
        "channel_count" => d.channel_count.to_string(),
        "nominal_srate" => format!("{}", d.nominal_srate),
        "channel_format" => d.channel_format.as_str().to_string(),
        "uid" => d.uid.clone(),
        "session_id" => d.session_id.clone(),
        "hostname" => d.hostname.clone(),
        "created_at" => format!("{}", d.created_at),
        "version" => d.version.to_string(),
        _ => String::new(),
    }
}

fn append_field(xml: &mut String, tag: &str, value: &str) {
    xml.push('<');
    xml.push_str(tag);
    xml.push('>');
    xml_escape_into(xml, value);
    xml.push_str("</");
    xml.push_str(tag);
    xml.push('>');
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

fn extract_tag(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{}>", tag);
    let close = format!("</{}>", tag);
    let start = xml.find(&open)?;
    let after_open = start + open.len();
    let end = xml[after_open..].find(&close)?;
    let content = &xml[after_open..after_open + end];
    Some(xml_unescape(content.trim()))
}
