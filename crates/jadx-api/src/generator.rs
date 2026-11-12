//! Class code generation hook and the default declaration-only generator.

use jadx_core::model::node::{ClassNode, MethodNode, Root};
use jadx_core::model::types::{ArgType, WildcardBound, CLASS_ENUM, CLASS_OBJECT};
use jadx_dex_input::DexFile;
use jadx_input_api::ConstValue;

use crate::access::{AccessInfo, AccessKind, ACC_ABSTRACT, ACC_FINAL, ACC_STATIC};
use crate::args::JadxArgs;
use crate::code::{CodeWriter, SimpleCodeInfo};
use crate::paths::DEFAULT_PACKAGE_NAME;

/// Everything a generator may read while it produces the code of a class.
pub struct GenContext<'a> {
    pub root: &'a Root,
    /// The loaded dex files and, for every class of `root`, the index of its dex file.
    pub dexes: &'a [DexFile],
    pub class_dex: &'a [usize],
    pub args: &'a JadxArgs,
    /// Member classes (not inlined) of every class, in output order.
    pub inner_classes: &'a [Vec<usize>],
}

impl GenContext<'_> {
    pub fn class(&self, idx: usize) -> &ClassNode {
        &self.root.classes[idx]
    }

    /// Dex file the class was loaded from.
    pub fn dex_of(&self, idx: usize) -> &DexFile {
        &self.dexes[self.class_dex[idx]]
    }

    /// Classes written inside the class (member classes that are generated).
    pub fn inner_of(&self, idx: usize) -> &[usize] {
        &self.inner_classes[idx]
    }
}

/// Errors and warnings reported while a class is generated.
#[derive(Debug, Default, Clone)]
pub struct ClassDiagnostics {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

impl ClassDiagnostics {
    pub fn error(&mut self, msg: impl Into<String>) {
        self.errors.push(msg.into());
    }

    pub fn warn(&mut self, msg: impl Into<String>) {
        self.warnings.push(msg.into());
    }
}

/// Produces the code of one top level class (inner classes included). Called from several
/// threads at once for different classes.
pub trait ClassCodeGenerator: Send + Sync {
    /// Generate the code of the top level class `cls` (index into `ctx.root.classes`). An `Err`
    /// is counted as an error of the class; the declaration only version is saved instead.
    fn generate(
        &self,
        ctx: &GenContext<'_>,
        cls: usize,
        diag: &mut ClassDiagnostics,
    ) -> Result<SimpleCodeInfo, String>;
}

/// Writes the declarations of a class (package, class header, fields, method signatures) with a
/// placeholder instead of the method bodies.
#[derive(Debug, Default, Clone, Copy)]
pub struct DeclarationGenerator;

impl ClassCodeGenerator for DeclarationGenerator {
    fn generate(
        &self,
        ctx: &GenContext<'_>,
        cls: usize,
        _diag: &mut ClassDiagnostics,
    ) -> Result<SimpleCodeInfo, String> {
        Ok(generate_declarations(ctx, cls))
    }
}

pub const PLACEHOLDER_BODY: &str = "/* not yet decompiled */";

/// Declaration only code of a class (used by [`DeclarationGenerator`] and as fallback when a
/// generator fails).
pub fn generate_declarations(ctx: &GenContext<'_>, cls: usize) -> SimpleCodeInfo {
    let mut w = CodeWriter::new(&ctx.args.code_indent_str, &ctx.args.code_new_line_str);
    let node = ctx.class(cls);
    let names = TypeNames::new(ctx, display_package(ctx.args, node.info.package()));
    if !names.pkg.is_empty() {
        w.add("package ").add(&names.pkg).add(";");
        w.new_line();
    }
    write_class(ctx, cls, &mut w, &names);
    w.finish()
}

/// Package name as written in the code: the default package is renamed.
fn display_package(args: &JadxArgs, pkg: &str) -> String {
    if pkg.is_empty() && args.is_rename_valid() {
        DEFAULT_PACKAGE_NAME.to_string()
    } else {
        pkg.to_string()
    }
}

/// Type name writer for the code of one class: names in the same package and in `java.lang` are
/// written short, all others fully qualified (no import statements).
pub struct TypeNames<'a> {
    root: &'a Root,
    /// Package of the class being written.
    pkg: String,
    rename_default_pkg: bool,
}

impl<'a> TypeNames<'a> {
    pub fn new(ctx: &GenContext<'a>, pkg: String) -> Self {
        TypeNames {
            root: ctx.root,
            pkg,
            rename_default_pkg: ctx.args.is_rename_valid(),
        }
    }

    pub fn s(&self, ty: &ArgType) -> String {
        match ty {
            ArgType::Object(name) => self.class_name(name),
            ArgType::Generic { name, generics } => {
                let list: Vec<String> = generics.iter().map(|g| self.s(g)).collect();
                format!("{}<{}>", self.class_name(name), list.join(", "))
            }
            ArgType::GenericType { name, .. } => name.clone(),
            ArgType::Wildcard { ty, bound } => {
                if *bound == WildcardBound::Unbound {
                    "?".to_string()
                } else {
                    format!("{}{}", bound.as_str(), self.s(ty))
                }
            }
            ArgType::OuterGeneric { outer, inner } => {
                format!("{}.{}", self.s(outer), self.short_inner(inner))
            }
            ArgType::Array(e) => format!("{}[]", self.s(e)),
            ArgType::Primitive(p) => p.to_string(),
            ArgType::Unknown(_) => "Object".to_string(),
        }
    }

    fn short_inner(&self, inner: &ArgType) -> String {
        match inner {
            ArgType::Generic { name, generics } => {
                let short = name.rsplit(['$', '.']).next().unwrap_or(name);
                let list: Vec<String> = generics.iter().map(|g| self.s(g)).collect();
                format!("{short}<{}>", list.join(", "))
            }
            ArgType::Object(name) => name.rsplit(['$', '.']).next().unwrap_or(name).to_string(),
            other => self.s(other),
        }
    }

    fn class_name(&self, name: &str) -> String {
        let mut full = match self.root.class_index(name) {
            Some(i) => self.root.classes[i].info.full_name().to_string(),
            None => name.replace('$', "."),
        };
        if self.rename_default_pkg && !full.contains('.') && self.root.class_index(name).is_some() {
            full = format!("{DEFAULT_PACKAGE_NAME}.{full}");
        }
        if let Some(rest) = full.strip_prefix("java.lang.") {
            if !rest.contains('.') {
                return rest.to_string();
            }
        }
        if !self.pkg.is_empty() {
            if let Some(rest) = full
                .strip_prefix(&self.pkg)
                .and_then(|r| r.strip_prefix('.'))
            {
                return rest.to_string();
            }
        }
        full
    }

    fn type_params(&self, params: &[ArgType]) -> String {
        let list: Vec<String> = params
            .iter()
            .map(|p| match p {
                ArgType::GenericType { name, extend_types } => {
                    let bounds: Vec<String> = extend_types
                        .iter()
                        .filter(|e| e.object_name().as_deref() != Some(CLASS_OBJECT))
                        .map(|e| self.s(e))
                        .collect();
                    if bounds.is_empty() {
                        name.clone()
                    } else {
                        format!("{name} extends {}", bounds.join(" & "))
                    }
                }
                other => self.s(other),
            })
            .collect();
        format!("<{}>", list.join(", "))
    }
}

fn write_class(ctx: &GenContext<'_>, idx: usize, w: &mut CodeWriter, tn: &TypeNames<'_>) {
    let cls = ctx.class(idx);
    let show_hidden = ctx.args.show_inconsistent_code;
    let mut af = AccessInfo::new(cls.access, AccessKind::Class);
    let is_enum = cls.is_enum();
    let kind_word = if af.is_interface() {
        af = af.remove(ACC_ABSTRACT | ACC_STATIC);
        if af.is_annotation() {
            "@interface"
        } else {
            "interface"
        }
    } else if is_enum {
        af = af.remove(ACC_FINAL | ACC_ABSTRACT | ACC_STATIC);
        "enum"
    } else {
        "class"
    };
    w.start_line_str(&af.make_string(show_hidden));
    w.add(kind_word).add(" ").add(cls.info.short_name());
    if !cls.generics.is_empty() {
        w.add(&tn.type_params(&cls.generics));
    }
    let interfaces = &cls.interfaces;
    if af.is_interface() {
        // an annotation always has the annotation interface as super interface
        let list: Vec<String> = interfaces
            .iter()
            .filter(|i| {
                !(af.is_annotation()
                    && i.object_name().as_deref() == Some("java.lang.annotation.Annotation"))
            })
            .map(|i| tn.s(i))
            .collect();
        if !list.is_empty() {
            w.add(" extends ").add(&list.join(", "));
        }
    } else {
        if let Some(sup) = &cls.super_class {
            let name = sup.object_name().unwrap_or_default();
            if name != CLASS_OBJECT && !(is_enum && name == CLASS_ENUM) {
                w.add(" extends ").add(&tn.s(sup));
            }
        }
        if !interfaces.is_empty() {
            let list: Vec<String> = interfaces.iter().map(|i| tn.s(i)).collect();
            w.add(" implements ").add(&list.join(", "));
        }
    }
    w.add(" {");
    w.inc_indent();
    let mut first = true;
    for f in &cls.fields {
        let faf = AccessInfo::new(f.access, AccessKind::Field);
        w.start_line_str(&faf.make_string(show_hidden));
        w.add(&tn.s(&f.ty)).add(" ").add(&f.info.name);
        if let Some(v) = &f.const_value {
            w.add(" = ").add(&const_str(v));
        }
        w.add(";");
        first = false;
    }
    for m in &cls.methods {
        if !first {
            w.new_line();
        }
        first = false;
        write_method(ctx, cls, m, w, tn);
    }
    for &inner in ctx.inner_of(idx) {
        if !first {
            w.new_line();
        }
        first = false;
        write_class(ctx, inner, w, tn);
    }
    w.dec_indent();
    w.start_line_str("}");
}

fn write_method(
    ctx: &GenContext<'_>,
    cls: &ClassNode,
    m: &MethodNode,
    w: &mut CodeWriter,
    tn: &TypeNames<'_>,
) {
    let show_hidden = ctx.args.show_inconsistent_code;
    let af = AccessInfo::new(m.access, AccessKind::Method);
    w.start_line();
    if m.info.is_class_init() {
        w.add("static {");
    } else {
        // interface methods without body are written without the implicit modifiers
        let af = if AccessInfo::new(cls.access, AccessKind::Class).is_interface() {
            af.remove(ACC_ABSTRACT)
        } else {
            af
        };
        w.add(&af.make_string(show_hidden));
        if !m.type_params.is_empty() {
            w.add(&tn.type_params(&m.type_params)).add(" ");
        }
        if m.is_constructor() {
            w.add(cls.info.short_name());
        } else {
            w.add(&tn.s(&m.return_type)).add(" ").add(&m.info.name);
        }
        w.add("(");
        for (i, a) in m.arg_types.iter().enumerate() {
            if i > 0 {
                w.add(", ");
            }
            w.add(&tn.s(a)).add(&format!(" arg{i}"));
        }
        w.add(")");
        if m.code_offset.is_none() {
            w.add(";");
            return;
        }
        w.add(" {");
    }
    w.inc_indent();
    w.start_line_str(PLACEHOLDER_BODY);
    w.dec_indent();
    w.start_line_str("}");
}

/// Java literal of a constant field value.
pub fn const_str(v: &ConstValue) -> String {
    match v {
        ConstValue::Boolean(b) => b.to_string(),
        ConstValue::Byte(x) => x.to_string(),
        ConstValue::Short(x) => x.to_string(),
        ConstValue::Char(c) => format!("'{}'", escape_units(&[*c], '\'')),
        ConstValue::Int(x) => x.to_string(),
        ConstValue::Long(x) => format!("{x}L"),
        ConstValue::Float(bits) => {
            let f = f32::from_bits(*bits);
            if f.is_nan() {
                "Float.NaN".to_string()
            } else if f.is_infinite() {
                if f > 0.0 {
                    "Float.POSITIVE_INFINITY"
                } else {
                    "Float.NEGATIVE_INFINITY"
                }
                .to_string()
            } else {
                format!("{f:?}f")
            }
        }
        ConstValue::Double(bits) => {
            let d = f64::from_bits(*bits);
            if d.is_nan() {
                "Double.NaN".to_string()
            } else if d.is_infinite() {
                if d > 0.0 {
                    "Double.POSITIVE_INFINITY"
                } else {
                    "Double.NEGATIVE_INFINITY"
                }
                .to_string()
            } else {
                format!("{d:?}d")
            }
        }
        ConstValue::Str(units) => format!("\"{}\"", escape_units(units, '"')),
    }
}

fn escape_units(units: &[u16], quote: char) -> String {
    let mut s = String::new();
    for r in char::decode_utf16(units.iter().copied()) {
        match r {
            Ok('\n') => s.push_str("\\n"),
            Ok('\r') => s.push_str("\\r"),
            Ok('\t') => s.push_str("\\t"),
            Ok('\u{8}') => s.push_str("\\b"),
            Ok('\u{c}') => s.push_str("\\f"),
            Ok('\\') => s.push_str("\\\\"),
            Ok(c) if c == quote => {
                s.push('\\');
                s.push(c);
            }
            Ok(c) if (c as u32) < 0x20 => s.push_str(&format!("\\u{:04x}", c as u32)),
            Ok(c) => s.push(c),
            Err(e) => s.push_str(&format!("\\u{:04x}", e.unpaired_surrogate())),
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literals() {
        assert_eq!(const_str(&ConstValue::Int(5)), "5");
        assert_eq!(const_str(&ConstValue::Long(-1)), "-1L");
        assert_eq!(const_str(&ConstValue::Char(b'q' as u16)), "'q'");
        assert_eq!(const_str(&ConstValue::Char(b'\'' as u16)), "'\\''");
        let s: Vec<u16> = "a\"b\n".encode_utf16().collect();
        assert_eq!(const_str(&ConstValue::Str(s)), "\"a\\\"b\\n\"");
        assert_eq!(const_str(&ConstValue::from_f32(1.5)), "1.5f");
        assert_eq!(const_str(&ConstValue::Boolean(false)), "false");
    }
}
