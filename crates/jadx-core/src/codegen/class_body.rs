//! `ClassGen` (class level): class declaration, fields, enum constants, methods, inner classes
//! and the imports block.

use jadx_input_api::ConstValue;

use super::annotation_gen;
use super::class_gen::ClassGen;
use super::code_writer::{CodeInfo, CodeWriter};
use super::insn_gen::InsnGen;
use super::method_gen::{no_code_cfg, MethodGen};
use super::type_gen::{literal_to_string, signature};
use super::{CodegenError, CommentsLevel};
use crate::access::{
    AccessInfo, AccessKind, ACC_ABSTRACT, ACC_INTERFACE, ACC_PRIVATE, ACC_STATIC, ACC_SYNTHETIC,
};
use crate::model::info::ClassInfo;
use crate::model::node::{ClassNode, FieldNode};
use crate::model::types::{ArgType, CLASS_OBJECT};

type Res<T = ()> = Result<T, CodegenError>;

impl<'a> ClassGen<'a> {
    fn node(&self) -> &'a ClassNode {
        &self.env().root.classes[self.class_index()]
    }

    /// `makeClass`
    pub fn make_class(&self) -> Res<CodeInfo> {
        if self.node().package_info {
            return self.make_package_info();
        }
        let mut body = new_writer(self);
        self.add_class_code(&mut body)?;

        let mut code = new_writer(self);
        self.add_package(&mut code);
        code.new_line();
        self.add_imports(&mut code);
        code.add_writer(&body);
        Ok(code.finish())
    }

    fn make_package_info(&self) -> Res<CodeInfo> {
        let mut code = new_writer(self);
        annotation_gen::add_annotations(self, &mut code, &self.node().attrs.annotations)?;
        code.new_line();
        self.add_package(&mut code);
        code.new_line();
        self.add_imports(&mut code);
        Ok(code.finish())
    }

    fn add_package(&self, code: &mut CodeWriter) {
        let env = self.env();
        let pkg = env.pkg(self.class_info());
        if pkg.is_empty() {
            code.add("// default package");
        } else {
            code.add("package ").add(&pkg).add_char(';');
        }
    }

    fn add_imports(&self, code: &mut CodeWriter) {
        let env = self.env();
        let mut imports: Vec<(Vec<u16>, String)> = self
            .imports()
            .iter()
            .map(|ci| {
                let name = env.full_name(ci);
                (name.encode_utf16().collect(), name)
            })
            .collect();
        if imports.is_empty() {
            return;
        }
        imports.sort();
        for (_, name) in imports {
            code.start_line_str("import ");
            code.add(&name);
            code.add_char(';');
        }
        code.new_line();
    }

    /// `addClassCode`
    pub fn add_class_code(&self, code: &mut CodeWriter) -> Res {
        if self.node().dont_generate {
            return Ok(());
        }
        self.add_class_declaration(code)?;
        self.add_class_body(code, false)
    }

    /// `addClassDeclaration`
    pub fn add_class_declaration(&self, code: &mut CodeWriter) -> Res {
        let env = self.env();
        let cls = self.node();
        let mut af = AccessInfo::new(cls.access, AccessKind::Class);
        if af.is_interface() {
            af = af.remove(ACC_ABSTRACT).remove(ACC_STATIC);
        }
        // 'static' and 'private' modifier not allowed for top classes (not inner)
        if !cls.info.is_inner() {
            af = af.remove(ACC_STATIC).remove(ACC_PRIVATE);
        }
        self.add_class_comments(code);

        self.set_in_class_header(true);
        annotation_gen::add_annotations(self, code, &cls.attrs.annotations)?;
        code.start_line();
        code.add(&af.make_string(env.check_comments_level(CommentsLevel::Info)));
        if af.is_interface() {
            if af.is_annotation() {
                code.add_char('@');
            }
            code.add("interface ");
        } else if af.is_enum() {
            code.add("enum ");
        } else {
            code.add("class ");
        }
        code.add(&env.short_name(&cls.info));

        self.add_generic_type_parameters(code, &cls.generics, true)?;
        code.add_char(' ');

        if let Some(sup) = &cls.super_class {
            if sup.object_name().as_deref() != Some(CLASS_OBJECT) && !cls.remove_super_class {
                code.add("extends ");
                self.use_class(code, sup)?;
                code.add_char(' ');
            }
        }

        if !cls.interfaces.is_empty() && !af.is_annotation() {
            if AccessInfo::new(cls.access, AccessKind::Class).is_interface() {
                code.add("extends ");
            } else {
                code.add("implements ");
            }
            for (i, interf) in cls.interfaces.iter().enumerate() {
                self.use_class(code, interf)?;
                if i + 1 < cls.interfaces.len() {
                    code.add(", ");
                }
            }
            code.add_char(' ');
        }
        self.set_in_class_header(false);
        Ok(())
    }

    /// Comments before the class declaration.
    fn add_class_comments(&self, code: &mut CodeWriter) {
        let env = self.env();
        let cls = self.node();
        if self.has_alias(&cls.info) {
            self.add_renamed_comment(code, &cls.info.raw_name(), cls.alias_reason.as_deref());
        }
        if env.check_comments_level(CommentsLevel::Info) {
            if let Some(file) = self.source_file_info() {
                code.start_line();
                code.add("/* JADX INFO: compiled from: ")
                    .add(&file)
                    .add(" */");
            }
            if let Some(file) = self.input_file_info() {
                code.start_line();
                code.add("/* JADX INFO: loaded from: ")
                    .add(&file)
                    .add(" */");
            }
        }
    }

    /// `ClassInfo.hasAlias`
    fn has_alias(&self, ci: &ClassInfo) -> bool {
        let env = self.env();
        if env.short_name(ci) != ci.short_name() {
            return true;
        }
        ci.parent_class().is_some_and(|p| self.has_alias(p))
    }

    /// `CodeGenUtils.addRenamedComment`
    pub fn add_renamed_comment(
        &self,
        code: &mut CodeWriter,
        orig_name: &str,
        reason: Option<&str>,
    ) {
        if !self.env().check_comments_level(CommentsLevel::Info) {
            return;
        }
        code.start_line();
        code.add("/* JADX INFO: renamed from: ").add(orig_name);
        if let Some(r) = reason {
            code.add(", reason: ").add(r);
        }
        code.add(" */");
    }

    /// Source file name that is shown as a comment (`addSourceFileInfo`).
    fn source_file_info(&self) -> Option<String> {
        let cls = self.node();
        let file = checked_source_file(cls)?;
        let top = self.env().root.top_parent_index(self.class_index());
        let top_name = self.env().root.classes[top].info.short_name().to_string();
        if top_name.contains(&file) {
            // ignore similar name
            return None;
        }
        Some(file)
    }

    /// Input file name that is shown as a comment (`addInputFileInfo`).
    fn input_file_info(&self) -> Option<String> {
        let env = self.env();
        let cls = self.node();
        let input = cls.attrs.input_file_name.as_deref()?;
        if let Some(p) = env.root.parent_index(self.class_index()) {
            if env.root.classes[p].attrs.input_file_name.as_deref() == Some(input) {
                // don't add same comment for inner classes
                return None;
            }
        }
        Some(input.to_string())
    }

    /// `addGenericTypeParameters`
    pub fn add_generic_type_parameters(
        &self,
        code: &mut CodeWriter,
        generics: &[ArgType],
        class_declaration: bool,
    ) -> Res<bool> {
        if generics.is_empty() {
            return Ok(false);
        }
        let env = self.env();
        code.add_char('<');
        for (i, generic_info) in generics.iter().enumerate() {
            if i != 0 {
                code.add(", ");
            }
            if let ArgType::GenericType { name, .. } = generic_info {
                code.add(name);
            } else {
                self.use_class(code, generic_info)?;
            }
            let list = generic_info.extend_types();
            if !list.is_empty() {
                code.add(" extends ");
                for (j, g) in list.iter().enumerate() {
                    if let ArgType::GenericType { name, .. } = g {
                        code.add(name);
                    } else {
                        self.use_class(code, g)?;
                        if class_declaration && !self.class_info().is_inner() && env.use_imports {
                            self.add_import(&env.class_info(g)?);
                        }
                    }
                    if j + 1 < list.len() {
                        code.add(" & ");
                    }
                }
            }
        }
        code.add_char('>');
        Ok(true)
    }

    /// `addClassBody`
    pub fn add_class_body(&self, code: &mut CodeWriter, print_class_name: bool) -> Res {
        let env = self.env();
        code.add_char('{');
        if print_class_name && env.check_comments_level(CommentsLevel::Info) {
            code.add(" // from class: ")
                .add(self.class_info().full_name());
        }
        self.set_body_gen_started(true);
        self.cls_decl_offset.set(code.len());
        code.inc_indent();
        self.add_fields(code)?;
        self.add_inner_cls_and_methods(code)?;
        code.dec_indent();
        code.start_line_char('}');
        Ok(())
    }

    fn add_inner_cls_and_methods(&self, code: &mut CodeWriter) -> Res {
        let env = self.env();
        let cidx = self.class_index();
        enum Node {
            Class(usize),
            Method(usize),
        }
        struct Item {
            line: u32,
            is_method: u8,
            alias: String,
            node: Node,
        }
        let mut items: Vec<Item> = Vec::new();
        for &ic in &env.inner_classes[cidx] {
            if self.skip_class(ic) {
                continue;
            }
            items.push(Item {
                line: 0,
                is_method: 0,
                alias: env.short_name(&env.root.classes[ic].info),
                node: Node::Class(ic),
            });
        }
        for (mi, m) in self.node().methods.iter().enumerate() {
            if !env.fallback && (m.dont_generate || self.is_empty_anonymous_constructor(mi)) {
                continue;
            }
            let line = env.loader.map_or(0, |l| l.source_line(cidx, mi));
            items.push(Item {
                line,
                is_method: 1,
                alias: self.method_sort_key(m),
                node: Node::Method(mi),
            });
        }
        items.sort_by(|a, b| {
            a.line
                .cmp(&b.line)
                .then(a.is_method.cmp(&b.is_method))
                .then_with(|| a.alias.encode_utf16().cmp(b.alias.encode_utf16()))
        });
        for it in items {
            match it.node {
                Node::Class(c) => self.add_inner_class(code, c),
                Node::Method(m) => self.add_method(code, m),
            }
        }
        Ok(())
    }

    /// Empty constructors of inlined anonymous classes are hidden.
    fn is_empty_anonymous_constructor(&self, mi: usize) -> bool {
        if !self.is_inlined() || !self.node().methods[mi].anonymous_constructor {
            return false;
        }
        let cfg = self.env().method_cfg(self.class_index(), mi);
        match cfg.as_ref() {
            Ok(cfg) => cfg.regions.method_region.is_some_and(|r| {
                crate::passes::regions::utils::is_empty(
                    cfg,
                    Some(crate::model::region::Container::Region(r)),
                )
            }),
            Err(_) => false,
        }
    }

    fn skip_class(&self, cls: usize) -> bool {
        if self.env().fallback {
            return false;
        }
        self.env().root.classes[cls].dont_generate
    }

    /// `getShortAlias` of a method.
    fn method_sort_key(&self, m: &crate::model::node::MethodNode) -> String {
        let args: String = m
            .info
            .arg_types
            .iter()
            .map(|t| signature(&self.resolve_alias_type(t)))
            .collect();
        format!(
            "{}({}){}",
            m.alias.as_deref().unwrap_or(&m.info.name),
            args,
            signature(&self.resolve_alias_type(&m.info.return_type))
        )
    }

    /// `ArgType.tryToResolveClassAlias`
    fn resolve_alias_type(&self, ty: &ArgType) -> ArgType {
        let env = self.env();
        if ty.is_generic_type() {
            return ty.clone();
        }
        if let Some(el) = ty.array_element().filter(|_| ty.is_array()) {
            let root_el = ty.array_root_element();
            let alias = self.resolve_alias_type(root_el);
            if &alias == root_el {
                return ty.clone();
            }
            let _ = el;
            return ArgType::array_dim(alias, ty.array_dimension());
        }
        if ty.is_object() && ty.wildcard_type().is_none() && !ty.is_generic() {
            if let Ok(ci) = env.class_info(ty) {
                if self.has_alias(&ci) {
                    return ArgType::object(&env.full_name(&ci));
                }
            }
        }
        ty.clone()
    }

    fn add_inner_class(&self, code: &mut CodeWriter, inner: usize) {
        let env = self.env();
        let in_gen = ClassGen::with_parent(env, inner, self.parent_gen());
        code.new_line();
        if let Err(e) = in_gen.add_class_code(code) {
            env.add_error(format!("Inner class code generation error: {}", e.msg));
        }
    }

    /// `addMethod`
    fn add_method(&self, code: &mut CodeWriter, mth: usize) {
        let env = self.env();
        if code.len() != self.cls_decl_offset.get() {
            code.new_line();
        }
        let saved_indent = code.indent();
        if let Err(e) = self.add_method_code(code, mth) {
            env.add_error(format!("Method generation error: {}", e.msg));
            self.add_error_comment(code, &e.msg);
            code.set_indent(saved_indent);
        }
    }

    /// `addErrors`: error comment of a node.
    fn add_error_comment(&self, code: &mut CodeWriter, msg: &str) {
        if self.env().check_comments_level(CommentsLevel::Error) {
            code.start_line_str("/*  JADX ERROR: ").add(msg).add("*/");
        }
    }

    /// Records an error of the method code generation.
    pub fn add_method_error(&self, e: &CodegenError) {
        self.env()
            .add_error(format!("Method code generation error: {}", e.msg));
    }

    /// `addMethodCode`
    pub fn add_method_code(&self, code: &mut CodeWriter, mth: usize) -> Res {
        let env = self.env();
        let cidx = self.class_index();
        let node = &self.node().methods[mth];
        let loaded = env.method_cfg(cidx, mth);
        let load_err = loaded.as_ref().as_ref().err().cloned();
        let fallback_cfg;
        let cfg = match loaded.as_ref() {
            Ok(c) => c,
            Err(_) => {
                fallback_cfg = no_code_cfg(env.root, cidx, mth);
                &fallback_cfg
            }
        };
        self.add_errors_and_comments(code, cfg, load_err.as_deref());
        if node.code_offset.is_none() {
            let mgen = MethodGen::new(self, cfg, mth);
            mgen.add_definition(code)?;
            code.add_char(';');
            return Ok(());
        }
        let mut bad_code = cfg.is_inconsistent();
        if bad_code && env.show_inconsistent_code {
            bad_code = false;
        }
        let has_error = load_err.is_some() || !cfg.errors.is_empty();
        if bad_code || self.is_fallback_mode() || has_error {
            let fb = ClassGen::new_fallback(env, cidx);
            let mgen = MethodGen::new(&fb, cfg, mth);
            self.method_body(code, &mgen, true)
        } else {
            let mgen = MethodGen::new(self, cfg, mth);
            self.method_body(code, &mgen, false)
        }
    }

    fn method_body(&self, code: &mut CodeWriter, mgen: &MethodGen<'_>, fallback: bool) -> Res {
        if mgen.add_definition(code)? {
            code.add_char(' ');
        }
        code.add_char('{');
        code.inc_indent();
        if fallback {
            mgen.dump_instructions(code);
        } else {
            mgen.add_instructions(code)?;
        }
        code.dec_indent();
        code.start_line_char('}');
        Ok(())
    }

    /// `CodeGenUtils.addErrorsAndComments` for a method.
    fn add_errors_and_comments(
        &self,
        code: &mut CodeWriter,
        cfg: &crate::model::cfg::MethodCfg,
        load_err: Option<&str>,
    ) {
        let env = self.env();
        let mut comments: Vec<String> = cfg
            .warnings
            .iter()
            .map(|w| format!("JADX WARN: {w}"))
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        comments.sort();
        if env.check_comments_level(CommentsLevel::Warn) {
            for c in comments {
                code.start_line_str("/* ");
                code.add_multi_line(&c);
                code.add(" */");
            }
        }
        if env.check_comments_level(CommentsLevel::Error) {
            if let Some(e) = load_err {
                code.start_line_str("/*  JADX ERROR: Method load error");
                code.add("\n        ").add(e).add("*/");
            }
            let mut errors: Vec<&String> = cfg.errors.iter().collect();
            errors.sort();
            errors.dedup();
            for e in errors {
                code.start_line_str("/*  JADX ERROR: ").add(e).add("*/");
            }
        }
    }

    fn add_fields(&self, code: &mut CodeWriter) -> Res {
        self.add_enum_fields(code)?;
        for f in &self.node().fields {
            self.add_field(code, f)?;
        }
        Ok(())
    }

    /// `addField`
    pub fn add_field(&self, code: &mut CodeWriter, f: &FieldNode) -> Res {
        let env = self.env();
        if f.dont_generate {
            return Ok(());
        }
        let has_alias = f.alias.as_deref().is_some_and(|a| a != f.info.name);
        if !f.attrs.annotations.is_empty() || has_alias {
            code.new_line();
        }
        if has_alias {
            self.add_renamed_comment(code, &f.info.name, f.alias_reason.as_deref());
        }
        annotation_gen::add_annotations(self, code, &f.attrs.annotations)?;

        code.start_line_str(
            &AccessInfo::new(f.access, AccessKind::Field)
                .make_string(env.check_comments_level(CommentsLevel::Info)),
        );
        self.use_type(code, &f.ty)?;
        code.add_char(' ');
        code.add(f.alias.as_deref().unwrap_or(&f.info.name));

        if let Some(init) = &f.init_insn {
            let cfg = env.method_cfg(self.class_index(), init.mth);
            if let Ok(cfg) = cfg.as_ref() {
                let mgen = MethodGen::new(self, cfg, init.mth);
                let ig = InsnGen::new(&mgen, false);
                code.add(" = ");
                if let Err(e) = ig.make_insn(
                    init.insn,
                    code,
                    Some(super::insn_gen::Flags::BodyOnlyNoWrap),
                ) {
                    env.add_error(format!("Failed to generate init code: {}", e.msg));
                }
            }
        } else if let Some(v) = &f.const_value {
            code.add(" = ");
            self.add_const_value(code, v, &f.ty)?;
        }
        code.add_char(';');
        Ok(())
    }

    fn add_const_value(&self, code: &mut CodeWriter, v: &ConstValue, ty: &ArgType) -> Res {
        let env = self.env();
        let lit = |l: i64| -> Res<String> {
            literal_to_string(l, ty, &env.string_utils, self.is_fallback_mode(), false)
        };
        match v {
            ConstValue::Boolean(b) => code.add(&lit(i64::from(*b))?),
            ConstValue::Byte(x) => code.add(&lit(i64::from(*x))?),
            ConstValue::Short(x) => code.add(&lit(i64::from(*x))?),
            ConstValue::Char(c) => code.add(&lit(i64::from(*c))?),
            ConstValue::Int(x) => code.add(&lit(i64::from(*x))?),
            ConstValue::Long(x) => code.add(&lit(*x)?),
            ConstValue::Float(bits) => code.add(&lit(i64::from(*bits as i32))?),
            ConstValue::Double(bits) => code.add(&lit(*bits as i64)?),
            ConstValue::Str(units) => code.add(&env.string_utils.unescape_string(units)),
        };
        Ok(())
    }

    fn is_fields_present(&self) -> bool {
        self.node().fields.iter().any(|f| !f.dont_generate)
    }

    fn is_methods_present(&self) -> bool {
        self.node().methods.iter().any(|m| !m.dont_generate)
    }

    fn is_inner_classes_present(&self) -> bool {
        let env = self.env();
        env.inner_classes[self.class_index()]
            .iter()
            .any(|&c| env.root.classes[c].anonymous.is_none())
    }

    /// `addEnumFields`
    fn add_enum_fields(&self, code: &mut CodeWriter) -> Res {
        let env = self.env();
        let cls = self.node();
        let Some(enum_attr) = &cls.enum_class else {
            return Ok(());
        };
        let cidx = self.class_index();
        let n = enum_attr.fields.len();
        for (i, f) in enum_attr.fields.iter().enumerate() {
            let field = &cls.fields[f.field];
            code.start_line_str(field.alias.as_deref().unwrap_or(&field.info.name));
            let cfg = env.method_cfg(cidx, enum_attr.static_method);
            if let Ok(cfg) = cfg.as_ref() {
                let insn = cfg.insn(f.ctr_insn);
                if let crate::model::insn::InsnKind::Constructor { call_mth, .. } = &insn.kind {
                    let call_node = env.root.resolve_method(call_mth);
                    let call_node = call_node.map(|(c, m)| &env.root.classes[c].methods[m]);
                    let skip_count = call_node
                        .and_then(|m| m.skip_args.as_ref())
                        .map_or(0, |s| s.iter().filter(|b| **b).count());
                    if insn.args.len() > skip_count {
                        let mgen = MethodGen::new(self, cfg, enum_attr.static_method);
                        let ig = InsnGen::new(&mgen, false);
                        ig.generate_method_arguments(code, insn, 0, call_node)?;
                    }
                }
            }
            if let Some(c) = f.cls {
                code.add_char(' ');
                let g = ClassGen::with_parent(env, c, self.parent_gen());
                g.add_class_body(code, true)?;
            }
            if i + 1 < n {
                code.add_char(',');
            }
        }
        if self.is_methods_present() || self.is_fields_present() || self.is_inner_classes_present()
        {
            if enum_attr.fields.is_empty() {
                code.start_line();
            }
            code.add_char(';');
            if self.is_fields_present() {
                code.new_line();
            }
        }
        Ok(())
    }
}

fn new_writer(cls_gen: &ClassGen<'_>) -> CodeWriter {
    let env = cls_gen.env();
    CodeWriter::with_options(&env.code_indent_str, &env.code_new_line_str, false)
}

/// Source file attribute of the class if it carries information (`checkSourceFilenameAttr`).
pub fn checked_source_file(cls: &ClassNode) -> Option<String> {
    let original = cls.attrs.source_file.as_deref()?;
    let mut file_name = original;
    if let Some(stripped) = file_name.strip_suffix(".java") {
        file_name = stripped;
    }
    if file_name.is_empty() || file_name == "SourceFile" {
        return None;
    }
    let info = &cls.info;
    let name = info.short_name();
    if file_name == name {
        return None;
    }
    let mut parent = info.parent_class();
    while let Some(p) = parent {
        let parent_name = p.short_name();
        if parent_name == file_name || parent_name.starts_with(&format!("{file_name}$")) {
            return None;
        }
        parent = p.parent_class();
    }
    if file_name.contains('$') && file_name.ends_with(&format!("${name}")) {
        return None;
    }
    if name.contains('$') && name.starts_with(file_name) {
        return None;
    }
    Some(original.to_string())
}

#[allow(dead_code)]
const _: u32 = ACC_INTERFACE | ACC_SYNTHETIC;
