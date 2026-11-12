//! Binary XML (`AndroidManifest.xml`, `res/**/*.xml`) to text conversion.

use std::collections::{HashMap, HashSet};

use crate::android_map::android_res_name;
use crate::consts::*;
use crate::error::{ResError, Result};
use crate::java_map::JavaOrderMap;
use crate::manifest_attrs::ManifestAttributes;
use crate::names::is_valid_xml_name;
use crate::stream::{java_trim, ParserStream};
use crate::strings::{parse_string_pool_no_type, BinaryXmlStrings};
use crate::text::{escape_xml, hex32, CodeWriter};
use crate::values::ValuesParser;

/// Maps a class name found in the XML to its alias in the decompiled code, if renamed.
pub type ClassAliasFn<'a> = &'a dyn Fn(&str) -> Option<String>;

/// Inputs of the XML decoder that come from the rest of the application.
#[derive(Clone, Copy)]
pub struct XmlContext<'a> {
    /// Resource id to `type/name` map of the app resource table.
    pub res_names: &'a HashMap<i32, String>,
    /// Enum and flag attribute decoding.
    pub attrs: &'a ManifestAttributes,
    /// Renamed class lookup for tag names and `android:name` values.
    pub class_alias: Option<ClassAliasFn<'a>>,
    /// Put all attributes of an element on one line.
    pub skip_pretty_print: bool,
}

/// Decoder for binary XML files. The instance keeps the state the reference keeps between files:
/// the package name of the manifest and the replacement names for invalid tag names.
pub struct BinaryXmlParser<'a> {
    ctx: XmlContext<'a>,
    tag_attr_deobf_names: HashMap<String, String>,
    app_package_name: Option<String>,
}

impl<'a> BinaryXmlParser<'a> {
    pub fn new(ctx: XmlContext<'a>) -> Self {
        BinaryXmlParser {
            ctx,
            tag_attr_deobf_names: HashMap::new(),
            app_package_name: None,
        }
    }

    /// Converts a binary XML file to text. Data that is not binary XML is returned as UTF-8 text.
    pub fn parse(&mut self, data: &[u8]) -> Result<String> {
        let mut is = ParserStream::new(data);
        is.read_i16(); // version
        let header = is.read_i16();
        if header != 0x0008 {
            return Ok(crate::jstr::java_utf8_decode(data));
        }
        let mut st = State {
            parser: self,
            is,
            writer: CodeWriter::new(),
            strings: BinaryXmlStrings::empty(),
            ns_map: JavaOrderMap::new(),
            ns_map_generated: HashSet::new(),
            defined_namespaces: HashSet::new(),
            current_tag: "ERROR".to_string(),
            first_element: true,
            is_last_end: true,
            namespace_depth: 0,
            resource_ids: None,
        };
        st.writer.add("<?xml version=\"1.0\" encoding=\"utf-8\"?>");
        st.decode()?;
        Ok(st.writer.finish())
    }

    fn deobf_class_name(&self, class_name: &str) -> String {
        if let Some(f) = self.ctx.class_alias {
            if class_name.contains('.') {
                let mut potential = class_name.to_string();
                if let Some(pkg) = &self.app_package_name {
                    if potential.starts_with('.') {
                        potential = format!("{pkg}{potential}");
                    }
                }
                if let Some(alias) = f(&potential) {
                    return alias;
                }
            }
        }
        class_name.to_string()
    }

    fn valid_tag_attr_name(&mut self, original: &str) -> String {
        if is_valid_xml_name(original) {
            return original.to_string();
        }
        if let Some(n) = self.tag_attr_deobf_names.get(original) {
            return n.clone();
        }
        // the reference picks random letters, a counter keeps the output reproducible
        let mut n = self.tag_attr_deobf_names.len();
        let generated = loop {
            let mut s = String::new();
            let mut v = n;
            for _ in 0..6 {
                s.insert(0, (b'a' + (v % 26) as u8) as char);
                v /= 26;
            }
            if !self.tag_attr_deobf_names.values().any(|x| *x == s) {
                break s;
            }
            n += 1;
        };
        self.tag_attr_deobf_names
            .insert(original.to_string(), generated.clone());
        generated
    }
}

struct State<'p, 'a, 'd> {
    parser: &'p mut BinaryXmlParser<'a>,
    is: ParserStream<'d>,
    writer: CodeWriter,
    strings: BinaryXmlStrings,
    ns_map: JavaOrderMap,
    ns_map_generated: HashSet<String>,
    defined_namespaces: HashSet<String>,
    current_tag: String,
    first_element: bool,
    is_last_end: bool,
    namespace_depth: i32,
    resource_ids: Option<Vec<i32>>,
}

impl State<'_, '_, '_> {
    fn die(&self, msg: &str) -> ResError {
        ResError::new(format!(
            "Decode error: {msg}, position: 0x{:x}",
            self.is.pos()
        ))
    }

    fn decode(&mut self) -> Result<()> {
        let size = i64::from(self.is.read_i32());
        while (self.is.pos() as i64) < size {
            let ty = self.is.read_i16();
            match ty {
                RES_NULL_TYPE => {}
                RES_STRING_POOL_TYPE => {
                    self.strings = parse_string_pool_no_type(&mut self.is)?;
                }
                RES_XML_RESOURCE_MAP_TYPE => self.parse_resource_map()?,
                RES_XML_START_NAMESPACE_TYPE => self.parse_name_space()?,
                RES_XML_CDATA_TYPE => self.parse_cdata()?,
                RES_XML_END_NAMESPACE_TYPE => self.parse_name_space_end()?,
                RES_XML_START_ELEMENT_TYPE => self.parse_element()?,
                RES_XML_END_ELEMENT_TYPE => self.parse_element_end()?,
                _ => {
                    if self.namespace_depth == 0 {
                        // padding at the end of the file
                        return Ok(());
                    }
                    return Err(self.die(&format!("Type: 0x{ty:x} not yet implemented")));
                }
            }
        }
        Ok(())
    }

    fn parse_resource_map(&mut self) -> Result<()> {
        if self.is.read_i16() != 0x8 {
            return Err(self.die("Header size of resmap is not 8!"));
        }
        let size = self.is.read_i32();
        let len = (size - 8) / 4;
        if len < 0 || i64::from(len) * 4 > self.is.remaining() {
            return Err(self.die("Invalid resource map size"));
        }
        let ids = (0..len).map(|_| self.is.read_i32()).collect();
        self.resource_ids = Some(ids);
        Ok(())
    }

    fn parse_name_space(&mut self) -> Result<()> {
        let header_size = self.is.read_i16();
        if header_size < 0x10 {
            return Err(self.die("NAMESPACE header is not 0x10 big"));
        }
        let size = self.is.read_i32();
        if size < 0x18 {
            return Err(self.die("NAMESPACE header chunk is not 0x18 big"));
        }
        let _begin_line = self.is.read_i32();
        let _comment = self.is.read_i32();
        let begin_prefix = self.is.read_i32();
        let begin_uri = self.is.read_i32();
        self.is.skip(i64::from(header_size - 0x10))?;

        let ns_key = self.get_string(begin_uri);
        let ns_value = self.get_string(begin_prefix);
        if not_blank(&ns_key) && !self.ns_map.contains_value(&ns_value) {
            self.ns_map.put_if_absent(&ns_key, &ns_value);
        }
        self.namespace_depth += 1;
        Ok(())
    }

    fn parse_name_space_end(&mut self) -> Result<()> {
        let header_size = self.is.read_i16();
        if header_size < 0x10 {
            return Err(self.die("NAMESPACE end is not 0x10 big"));
        }
        let _data_size = self.is.read_i32();
        let _end_line = self.is.read_i32();
        let _comment = self.is.read_i32();
        let end_prefix = self.is.read_i32();
        let end_uri = self.is.read_i32();
        self.is.skip(i64::from(header_size - 0x10))?;
        self.namespace_depth -= 1;

        let ns_key = self.get_string(end_uri);
        let ns_value = self.get_string(end_prefix);
        if not_blank(&ns_key) && !self.ns_map.contains_value(&ns_value) {
            self.ns_map.put_if_absent(&ns_key, &ns_value);
        }
        Ok(())
    }

    fn parse_cdata(&mut self) -> Result<()> {
        if self.is.read_i16() != 0x10 {
            return Err(self.die("CDATA header is not 0x10"));
        }
        if self.is.read_i32() != 0x1C {
            return Err(self.die("CDATA header chunk is not 0x1C"));
        }
        let _line = self.is.read_i32();
        self.is.skip(4)?;
        let str_index = self.is.read_i32();
        let s = self.get_string(str_index);
        if !self.is_last_end {
            self.is_last_end = true;
            self.writer.add_char('>');
        }
        self.writer.add(&escape_xml(&s));
        let size = i64::from(self.is.read_i16());
        self.is.skip(size - 2)
    }

    fn parse_element(&mut self) -> Result<()> {
        if self.first_element {
            self.first_element = false;
        } else {
            self.writer.inc_indent();
        }
        if self.is.read_i16() != 0x10 {
            return Err(self.die("ELEMENT HEADER SIZE is not 0x10"));
        }
        let start_pos = self.is.pos() as i64;
        let element_size = i64::from(self.is.read_i32());
        let _begin_line = self.is.read_i32();
        let _comment = self.is.read_i32();
        let _start_ns = self.is.read_i32();
        let start_ns_name = self.is.read_i32();
        if !self.is_last_end && self.current_tag != "ERROR" {
            self.writer.add_char('>');
        }
        self.is_last_end = false;
        let name = self.get_string(start_ns_name);
        let name = self.parser.deobf_class_name(&name);
        self.current_tag = self.parser.valid_tag_attr_name(&name);
        self.writer
            .start_line_char('<')
            .add(&self.current_tag.clone());
        let attribute_start = self.is.read_i16();
        if attribute_start != 0x14 {
            return Err(self.die("startNS's attributeStart is not 0x14"));
        }
        let attribute_size = self.is.read_i16();
        if attribute_size < 0x14 {
            return Err(self.die("startNS's attributeSize is less than 0x14"));
        }
        let attribute_count = self.is.read_i16();
        let _id_index = self.is.read_i16();
        let _class_index = self.is.read_i16();
        let _style_index = self.is.read_i16();
        if self.current_tag == "manifest" || self.defined_namespaces.len() != self.ns_map.len() {
            for (key, value) in self.ns_map.iter_java_order() {
                if !self.defined_namespaces.contains(&key) {
                    self.defined_namespaces.insert(key.clone());
                    let ns_value = self.parser.valid_tag_attr_name(&value);
                    self.writer.add(" xmlns");
                    if !java_trim(&ns_value).is_empty() {
                        self.writer.add_char(':');
                        self.writer.add(&ns_value);
                    }
                    self.writer.add("=\"").add(&escape_xml(&key)).add_char('"');
                }
            }
        }
        let mut attr_cache: HashSet<String> = HashSet::new();
        let attr_new_line = attribute_count != 1 && !self.parser.ctx.skip_pretty_print;
        for _ in 0..attribute_count {
            self.parse_attribute(attr_new_line, &mut attr_cache, attribute_size)?;
        }
        let end_pos = self.is.pos() as i64;
        if end_pos - start_pos + 4 < element_size {
            self.is.skip(element_size - (end_pos - start_pos + 4))?;
        }
        Ok(())
    }

    fn parse_attribute(
        &mut self,
        new_line: bool,
        attr_cache: &mut HashSet<String>,
        attribute_size: i32,
    ) -> Result<()> {
        let attribute_ns = self.is.read_i32();
        let attribute_name = self.is.read_i32();
        let _raw_value = self.is.read_i32();
        self.is.skip(3)?;
        let data_type = self.is.read_i8();
        let data = self.is.read_i32();
        self.is.skip(i64::from(attribute_size - 0x14))?;

        let short_ns_name: Option<String> = if attribute_ns != -1 {
            self.get_attribute_ns(attribute_ns, new_line)
        } else if self.is_android_attribute(attribute_name) {
            // obfuscators may remove the namespace, restore it for framework attributes
            Some(self.get_ns_name(ANDROID_NS_URL, new_line))
        } else {
            None
        };
        let raw_attr_name = self.get_attribute_name(attribute_name);
        let attr_name = self.parser.valid_tag_attr_name(&raw_attr_name);
        let attr_full_name = match &short_ns_name {
            Some(ns) => format!("{ns}:{attr_name}"),
            None => attr_name.clone(),
        };
        // do not dump duplicated values
        if !attr_cache.insert(attr_full_name.clone()) {
            return Ok(());
        }

        if new_line {
            self.writer.start_line().add_indent();
        } else {
            self.writer.add_char(' ');
        }
        self.writer.add(&attr_full_name).add("=\"");
        let decoded = self
            .parser
            .ctx
            .attrs
            .decode(&attr_full_name, i64::from(data));
        if let Some(mut decoded_attr) = decoded {
            self.memorize_package_name(&attr_name, Some(&decoded_attr));
            if attr_full_name == "android:name" {
                decoded_attr = self.parser.deobf_class_name(&decoded_attr);
            }
            self.writer.add(&escape_xml(&decoded_attr));
        } else {
            self.decode_attribute(data_type, data, &attr_full_name);
        }
        self.writer.add_char('"');
        Ok(())
    }

    fn get_attribute_ns(&mut self, attribute_ns: i32, new_line: bool) -> Option<String> {
        let mut attr_url = self.get_string(attribute_ns);
        if attr_url.is_empty() {
            if is_res_internal_id(attribute_ns) {
                return None;
            }
            attr_url = ANDROID_NS_URL.to_string();
        }
        Some(self.get_ns_name(&attr_url, new_line))
    }

    fn get_ns_name(&mut self, attr_url: &str, new_line: bool) -> String {
        if let Some(n) = self.ns_map.get(attr_url) {
            return n.to_string();
        }
        self.generate_name_for_ns(attr_url, new_line)
    }

    fn is_android_attribute(&self, id: i32) -> bool {
        match &self.resource_ids {
            Some(ids) if id >= 0 && (id as usize) < ids.len() => {
                // android framework resources use package id 0x01
                (ids[id as usize] as u32) >> 24 == 0x01
            }
            _ => false,
        }
    }

    fn generate_name_for_ns(&mut self, attr_url: &str, new_line: bool) -> String {
        let attr_name;
        if attr_url == ANDROID_NS_URL {
            attr_name = ANDROID_NS_VALUE.to_string();
            self.ns_map.put(ANDROID_NS_URL, &attr_name);
        } else {
            let mut i = 1;
            loop {
                let candidate = format!("ns{i}");
                if !self.ns_map_generated.contains(&candidate)
                    && !self.ns_map.contains_value(&candidate)
                {
                    self.ns_map_generated.insert(candidate.clone());
                    // the generated value is not added to the map, because the url might be
                    // used by a neighbor element, but never defined
                    attr_name = candidate;
                    break;
                }
                i += 1;
            }
        }
        if new_line {
            self.writer.start_line().add_indent();
        } else {
            self.writer.add_char(' ');
        }
        self.writer
            .add("xmlns:")
            .add(&attr_name)
            .add("=\"")
            .add(attr_url)
            .add("\" ");
        attr_name
    }

    fn get_attribute_name(&self, id: i32) -> String {
        // entries of the framework resource map win over the strings of the XML pool
        if let Some(ids) = &self.resource_ids {
            if id >= 0 && (id as usize) < ids.len() {
                if let Some(s) = android_res_name(ids[id as usize]) {
                    return match s.find('/') {
                        Some(p) => s[p + 1..].to_string(),
                        None => s.to_string(),
                    };
                }
            }
        }
        let s = self.get_string(id);
        if s.is_empty() {
            return format!("NOT_FOUND_0x{}", hex32(id));
        }
        s
    }

    fn get_string(&self, id: i32) -> String {
        if id >= 0 && id < self.strings.size() {
            return self.strings.get(id);
        }
        format!("NOT_FOUND_STR_0x{}", hex32(id))
    }

    fn decode_attribute(&mut self, data_type: i32, data: i32, attr_full_name: &str) {
        if data_type == TYPE_REFERENCE {
            let res_names = self.parser.ctx.res_names;
            if let Some(res_name) = res_names.get(&data) {
                self.writer.add_char('@');
                if res_name.starts_with("id/") {
                    self.writer.add_char('+');
                }
                self.writer.add(res_name);
            } else if let Some(a) = android_res_name(data) {
                self.writer.add("@android:").add(a);
            } else if data == 0 {
                self.writer.add("@null");
            } else {
                self.writer.add("0x").add(&hex32(data));
            }
        } else {
            let mut s = {
                let vp = ValuesParser::new(&self.strings, self.parser.ctx.res_names);
                vp.decode_value(data_type, data)
            };
            self.memorize_package_name(attr_full_name, s.as_deref());
            if attr_full_name == "android:name" {
                if let Some(v) = &s {
                    s = Some(self.parser.deobf_class_name(v));
                }
            }
            match s {
                Some(v) => self.writer.add(&escape_xml(&v)),
                None => self.writer.add("null"),
            };
        }
    }

    fn parse_element_end(&mut self) -> Result<()> {
        if self.is.read_i16() != 0x10 {
            return Err(self.die("ELEMENT END header is not 0x10"));
        }
        if self.is.read_i32() != 0x18 {
            return Err(self.die("ELEMENT END header chunk is not 0x18 big"));
        }
        let _end_line = self.is.read_i32();
        let _comment = self.is.read_i32();
        let _element_ns = self.is.read_i32();
        let element_name_id = self.is.read_i32();
        let name = self.get_string(element_name_id);
        let name = self.parser.deobf_class_name(&name);
        let elem_name = self.parser.valid_tag_attr_name(&name);
        if self.current_tag == elem_name && !self.is_last_end {
            self.writer.add("/>");
        } else {
            self.writer.start_line_str("</");
            self.writer.add(&elem_name).add_char('>');
        }
        self.is_last_end = true;
        if self.writer.indent() != 0 {
            self.writer.dec_indent();
        }
        Ok(())
    }

    fn memorize_package_name(&mut self, attr_full_name: &str, value: Option<&str>) {
        if self.current_tag == "manifest" && attr_full_name == "package" {
            self.parser.app_package_name = value.map(str::to_string);
        }
    }
}

fn not_blank(s: &str) -> bool {
    !java_trim(s).is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool_utf16(strings: &[&str]) -> Vec<u8> {
        // string pool chunk, UTF-16
        let mut offsets = Vec::new();
        let mut data = Vec::new();
        for s in strings {
            offsets.push(data.len() as u32);
            let units: Vec<u16> = s.encode_utf16().collect();
            data.extend_from_slice(&(units.len() as u16).to_le_bytes());
            for u in units {
                data.extend_from_slice(&u.to_le_bytes());
            }
            data.extend_from_slice(&[0, 0]);
        }
        while data.len() % 4 != 0 {
            data.push(0);
        }
        let header = 28u32;
        let strings_start = header + 4 * strings.len() as u32;
        let size = strings_start + data.len() as u32;
        let mut out = Vec::new();
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&28u16.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&(strings.len() as u32).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&strings_start.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        for o in offsets {
            out.extend_from_slice(&o.to_le_bytes());
        }
        out.extend_from_slice(&data);
        out
    }

    fn chunk(ty: u16, header: u16, body: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&ty.to_le_bytes());
        out.extend_from_slice(&header.to_le_bytes());
        out.extend_from_slice(&((8 + body.len()) as u32).to_le_bytes());
        out.extend_from_slice(body);
        out
    }

    fn i32le(v: i32) -> [u8; 4] {
        v.to_le_bytes()
    }

    #[test]
    fn decodes_small_document() {
        // strings: 0 "android", 1 uri, 2 "root", 3 "child", 4 "text"
        let pool = pool_utf16(&[
            "android",
            "http://schemas.android.com/apk/res/android",
            "root",
            "child",
            "text",
        ]);
        let mut ns_body = Vec::new();
        for v in [1, -1, 0, 1] {
            ns_body.extend_from_slice(&i32le(v));
        }
        let start_ns = chunk(0x100, 0x10, &ns_body);
        let mut end_ns = chunk(0x101, 0x10, &ns_body);
        end_ns.truncate(end_ns.len());
        let element = |name: i32| {
            let mut b = Vec::new();
            for v in [1, -1, -1, name] {
                b.extend_from_slice(&i32le(v));
            }
            b.extend_from_slice(&0x14u16.to_le_bytes());
            b.extend_from_slice(&0x14u16.to_le_bytes());
            b.extend_from_slice(&0u16.to_le_bytes());
            b.extend_from_slice(&0u16.to_le_bytes());
            b.extend_from_slice(&0u16.to_le_bytes());
            b.extend_from_slice(&0u16.to_le_bytes());
            chunk(0x102, 0x10, &b)
        };
        let end_element = |name: i32| {
            let mut b = Vec::new();
            for v in [1, -1, -1, name] {
                b.extend_from_slice(&i32le(v));
            }
            chunk(0x103, 0x10, &b)
        };
        let mut body = pool;
        body.extend(start_ns);
        body.extend(element(2));
        body.extend(element(3));
        body.extend(end_element(3));
        body.extend(end_element(2));
        body.extend(end_ns);
        let mut file = Vec::new();
        file.extend_from_slice(&3u16.to_le_bytes());
        file.extend_from_slice(&8u16.to_le_bytes());
        file.extend_from_slice(&((8 + body.len()) as u32).to_le_bytes());
        file.extend(body);

        let names = HashMap::new();
        let attrs = ManifestAttributes::new();
        let mut p = BinaryXmlParser::new(XmlContext {
            res_names: &names,
            attrs: &attrs,
            class_alias: None,
            skip_pretty_print: false,
        });
        let text = p.parse(&file).unwrap();
        assert_eq!(
            text,
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<root xmlns:android=\"http://schemas.android.com/apk/res/android\">\n    <child/>\n</root>"
        );
    }

    #[test]
    fn plain_text_passes_through() {
        let names = HashMap::new();
        let attrs = ManifestAttributes::new();
        let mut p = BinaryXmlParser::new(XmlContext {
            res_names: &names,
            attrs: &attrs,
            class_alias: None,
            skip_pretty_print: false,
        });
        assert_eq!(p.parse(b"<a/>").unwrap(), "<a/>");
    }
}
