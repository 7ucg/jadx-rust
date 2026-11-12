//! Names and canonical identities of classes, methods and fields.
//!
//! A reference in an instruction names a method or field through the class it was invoked on
//! (`Lsub;->run()V`) although the member is declared in a super type. The index stores every
//! reference under the identity of the declaration (`Lbase;->run()V`) when the declaration is
//! part of the root, and under the reference as written otherwise (framework members).

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use jadx_core::model::info::type_signature;
use jadx_core::model::node::{MethodId, Root};
use jadx_input_api::{FieldRefData, MethodRefData};

/// Field of a class: `(class index, field index)`.
pub type FieldId = (usize, usize);

/// Maximum number of super types visited while looking for a declaration.
const MAX_HIERARCHY_VISITS: usize = 512;

/// Dotted raw class name of a type descriptor (`Lfoo/Bar$Baz;` -> `foo.Bar$Baz`). Array
/// descriptors map to the name of their element class, primitives to `None`.
pub fn raw_name_of_desc(desc: &str) -> Option<String> {
    let d = desc.trim_start_matches('[');
    let inner = d.strip_prefix('L')?.strip_suffix(';')?;
    Some(inner.replace('/', "."))
}

/// Descriptor text of a method as written in references: `Lcls;->name(args)ret`.
pub fn method_ref_text(r: &MethodRefData) -> String {
    let mut s = String::with_capacity(
        r.parent_class.len() + r.name.len() + r.return_type.len() + 8 + r.arg_types.len() * 12,
    );
    s.push_str(&r.parent_class);
    s.push_str("->");
    s.push_str(&r.name);
    s.push('(');
    for a in &r.arg_types {
        s.push_str(a);
    }
    s.push(')');
    s.push_str(&r.return_type);
    s
}

/// Descriptor text of a field as written in references: `Lcls;->name:type`.
pub fn field_ref_text(r: &FieldRefData) -> String {
    format!("{}->{}:{}", r.parent_class, r.name, r.ty)
}

/// Parse `Lcls;->name(args)ret` back into a reference.
pub fn parse_method_ref(text: &str) -> Option<MethodRefData> {
    let (parent, rest) = text.split_once("->")?;
    let (name, sig) = rest.split_once('(')?;
    let (args, ret) = sig.split_once(')')?;
    let mut arg_types = Vec::new();
    let b = args.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let start = i;
        while b[i] == b'[' {
            i += 1;
            if i >= b.len() {
                return None;
            }
        }
        if b[i] == b'L' {
            i += args[i..].find(';')?;
        }
        i += 1;
        arg_types.push(args[start..i].to_owned());
    }
    Some(MethodRefData {
        parent_class: parent.to_owned(),
        name: name.to_owned(),
        arg_types,
        return_type: ret.to_owned(),
    })
}

/// Parse `Lcls;->name:type` back into a reference.
pub fn parse_field_ref(text: &str) -> Option<FieldRefData> {
    let (parent, rest) = text.split_once("->")?;
    let (name, ty) = rest.split_once(':')?;
    if parent.is_empty() || name.is_empty() || ty.is_empty() {
        return None;
    }
    Some(FieldRefData {
        parent_class: parent.to_owned(),
        name: name.to_owned(),
        ty: ty.to_owned(),
    })
}

/// Resolves references against the classes of a root.
pub struct Resolver<'r> {
    root: &'r Root,
    method_maps: Vec<OnceLock<HashMap<String, usize>>>,
    field_maps: Vec<OnceLock<HashMap<String, usize>>>,
}

impl<'r> Resolver<'r> {
    pub fn new(root: &'r Root) -> Self {
        Resolver {
            root,
            method_maps: (0..root.classes.len()).map(|_| OnceLock::new()).collect(),
            field_maps: (0..root.classes.len()).map(|_| OnceLock::new()).collect(),
        }
    }

    pub fn root(&self) -> &'r Root {
        self.root
    }

    /// Class of a type descriptor.
    pub fn class_by_desc(&self, desc: &str) -> Option<usize> {
        self.root.class_index(&raw_name_of_desc(desc)?)
    }

    fn method_map(&self, cls: usize) -> &HashMap<String, usize> {
        self.method_maps[cls].get_or_init(|| {
            self.root.classes[cls]
                .methods
                .iter()
                .enumerate()
                .map(|(i, m)| (m.info.short_id(), i))
                .collect()
        })
    }

    fn field_map(&self, cls: usize) -> &HashMap<String, usize> {
        self.field_maps[cls].get_or_init(|| {
            self.root.classes[cls]
                .fields
                .iter()
                .enumerate()
                .map(|(i, f)| (f.info.short_id(), i))
                .collect()
        })
    }

    /// Super class and interfaces of a class that are part of the root.
    fn parents(&self, cls: usize) -> Vec<usize> {
        let c = &self.root.classes[cls];
        c.super_class
            .iter()
            .chain(c.interfaces.iter())
            .filter_map(|t| t.object_name())
            .filter_map(|n| self.root.class_index(&n))
            .collect()
    }

    /// Search `start` and its super types (depth first, super class before interfaces) for a
    /// member accepted by `find`.
    fn search<T>(&self, start: usize, find: impl Fn(usize) -> Option<T>) -> Option<T> {
        let mut seen = HashSet::new();
        let mut stack = vec![start];
        while let Some(cls) = stack.pop() {
            if !seen.insert(cls) || seen.len() > MAX_HIERARCHY_VISITS {
                continue;
            }
            if let Some(found) = find(cls) {
                return Some(found);
            }
            let mut parents = self.parents(cls);
            parents.reverse(); // pop the super class first
            stack.extend(parents);
        }
        None
    }

    /// Declaration of a method reference inside the root.
    pub fn resolve_method(&self, r: &MethodRefData) -> Option<MethodId> {
        let start = self.class_by_desc(&r.parent_class)?;
        let short = short_id_of(r);
        self.search(start, |c| self.method_map(c).get(&short).map(|&m| (c, m)))
    }

    /// Declaration of a field reference inside the root.
    pub fn resolve_field(&self, r: &FieldRefData) -> Option<FieldId> {
        let start = self.class_by_desc(&r.parent_class)?;
        let short = format!("{}:{}", r.name, r.ty);
        self.search(start, |c| self.field_map(c).get(&short).map(|&f| (c, f)))
    }

    /// Canonical text of a method reference: the declaration when it is part of the root, the
    /// reference as written otherwise.
    pub fn canonical_method_text(&self, r: &MethodRefData) -> String {
        match self.resolve_method(r) {
            Some(id) => self.method_text(id),
            None => method_ref_text(r),
        }
    }

    pub fn canonical_field_text(&self, r: &FieldRefData) -> String {
        match self.resolve_field(r) {
            Some(id) => self.field_text(id),
            None => field_ref_text(r),
        }
    }

    /// `Lcls;->name(args)ret` of a method of the root.
    pub fn method_text(&self, id: MethodId) -> String {
        let c = &self.root.classes[id.0];
        format!(
            "{}->{}",
            type_signature(c.info.arg_type()),
            c.methods[id.1].info.short_id()
        )
    }

    /// `Lcls;->name:type` of a field of the root.
    pub fn field_text(&self, id: FieldId) -> String {
        let c = &self.root.classes[id.0];
        format!(
            "{}->{}",
            type_signature(c.info.arg_type()),
            c.fields[id.1].info.short_id()
        )
    }
}

fn short_id_of(r: &MethodRefData) -> String {
    let mut s =
        String::with_capacity(r.name.len() + r.return_type.len() + 2 + r.arg_types.len() * 12);
    s.push_str(&r.name);
    s.push('(');
    for a in &r.arg_types {
        s.push_str(a);
    }
    s.push(')');
    s.push_str(&r.return_type);
    s
}

/// Human readable method: `pkg.Cls.name(args):ret` with the raw class name.
pub fn display_method(root: &Root, id: MethodId) -> String {
    let c = &root.classes[id.0];
    let m = &c.methods[id.1];
    let args: Vec<String> = m.info.arg_types.iter().map(|a| a.to_string()).collect();
    format!(
        "{}.{}({}):{}",
        c.info.raw_name(),
        m.info.name,
        args.join(", "),
        m.info.return_type
    )
}

/// Human readable field: `pkg.Cls.name:type`.
pub fn display_field(root: &Root, id: FieldId) -> String {
    let c = &root.classes[id.0];
    let f = &c.fields[id.1];
    format!("{}.{}:{}", c.info.raw_name(), f.info.name, f.info.ty)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_names() {
        assert_eq!(
            raw_name_of_desc("Lfoo/Bar$Baz;").as_deref(),
            Some("foo.Bar$Baz")
        );
        assert_eq!(raw_name_of_desc("[[Lfoo/Bar;").as_deref(), Some("foo.Bar"));
        assert_eq!(raw_name_of_desc("I"), None);
        assert_eq!(raw_name_of_desc("[I"), None);
    }

    #[test]
    fn method_reference_text_roundtrip() {
        let r = MethodRefData {
            parent_class: "Lp/A;".into(),
            name: "f".into(),
            arg_types: vec!["I".into(), "[[J".into(), "Lq/B;".into(), "[Lq/C;".into()],
            return_type: "Ljava/lang/String;".into(),
        };
        assert_eq!(parse_method_ref(&method_ref_text(&r)), Some(r));
        assert_eq!(parse_method_ref("garbage"), None);
        assert_eq!(parse_method_ref("La;->f([)V"), None);
    }

    #[test]
    fn reference_texts() {
        let m = MethodRefData {
            parent_class: "La/B;".into(),
            name: "f".into(),
            arg_types: vec!["I".into(), "Ljava/lang/String;".into()],
            return_type: "V".into(),
        };
        assert_eq!(method_ref_text(&m), "La/B;->f(ILjava/lang/String;)V");
        assert_eq!(short_id_of(&m), "f(ILjava/lang/String;)V");
        let f = FieldRefData {
            parent_class: "La/B;".into(),
            name: "x".into(),
            ty: "I".into(),
        };
        assert_eq!(field_ref_text(&f), "La/B;->x:I");
    }
}
