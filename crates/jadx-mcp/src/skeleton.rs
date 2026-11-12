//! Java declaration skeletons: class, field and method declarations rendered from bytecode
//! metadata with placeholder method bodies.
//!
//! This is what [`crate::engine::EngineProject`] serves as class source until a real code
//! generator is plugged in behind the [`crate::project::Project`] trait.

use crate::project::{ClassEntry, FieldEntry, MethodEntry};

pub const ACC_PUBLIC: u32 = 0x1;
pub const ACC_PRIVATE: u32 = 0x2;
pub const ACC_PROTECTED: u32 = 0x4;
pub const ACC_STATIC: u32 = 0x8;
pub const ACC_FINAL: u32 = 0x10;
pub const ACC_SYNCHRONIZED: u32 = 0x20;
pub const ACC_VOLATILE: u32 = 0x40;
pub const ACC_BRIDGE: u32 = 0x40;
pub const ACC_TRANSIENT: u32 = 0x80;
pub const ACC_VARARGS: u32 = 0x80;
pub const ACC_NATIVE: u32 = 0x100;
pub const ACC_INTERFACE: u32 = 0x200;
pub const ACC_ABSTRACT: u32 = 0x400;
pub const ACC_STRICT: u32 = 0x800;
pub const ACC_SYNTHETIC: u32 = 0x1000;
pub const ACC_ANNOTATION: u32 = 0x2000;
pub const ACC_ENUM: u32 = 0x4000;

const BODY_PLACEHOLDER: &str = "{ /* body not decompiled */ }";

/// Keyword of the type declaration.
pub fn class_keyword(access: u32) -> &'static str {
    if access & ACC_ANNOTATION != 0 {
        "@interface"
    } else if access & ACC_INTERFACE != 0 {
        "interface"
    } else if access & ACC_ENUM != 0 {
        "enum"
    } else {
        "class"
    }
}

/// Modifier names of a class in source order.
pub fn class_modifiers(access: u32) -> Vec<&'static str> {
    let mut out = Vec::new();
    push_visibility(&mut out, access);
    let is_interface = access & ACC_INTERFACE != 0;
    if access & ACC_STATIC != 0 {
        out.push("static");
    }
    if access & ACC_ABSTRACT != 0 && !is_interface {
        out.push("abstract");
    }
    if access & ACC_FINAL != 0 && access & ACC_ENUM == 0 {
        out.push("final");
    }
    out
}

pub fn field_modifiers(access: u32) -> Vec<&'static str> {
    let mut out = Vec::new();
    push_visibility(&mut out, access);
    for (flag, name) in [
        (ACC_STATIC, "static"),
        (ACC_FINAL, "final"),
        (ACC_VOLATILE, "volatile"),
        (ACC_TRANSIENT, "transient"),
    ] {
        if access & flag != 0 {
            out.push(name);
        }
    }
    out
}

pub fn method_modifiers(access: u32, in_interface: bool) -> Vec<&'static str> {
    let mut out = Vec::new();
    push_visibility(&mut out, access);
    for (flag, name) in [
        (ACC_ABSTRACT, "abstract"),
        (ACC_STATIC, "static"),
        (ACC_FINAL, "final"),
        (ACC_SYNCHRONIZED, "synchronized"),
        (ACC_NATIVE, "native"),
        (ACC_STRICT, "strictfp"),
    ] {
        if access & flag != 0 && !(in_interface && flag == ACC_ABSTRACT) {
            out.push(name);
        }
    }
    out
}

fn push_visibility(out: &mut Vec<&'static str>, access: u32) {
    if access & ACC_PUBLIC != 0 {
        out.push("public");
    } else if access & ACC_PROTECTED != 0 {
        out.push("protected");
    } else if access & ACC_PRIVATE != 0 {
        out.push("private");
    }
}

/// Human readable flag names for metadata output.
pub fn flag_names(access: u32, kind: FlagKind) -> Vec<&'static str> {
    let table: &[(u32, &str)] = match kind {
        FlagKind::Class => &[
            (ACC_PUBLIC, "public"),
            (ACC_FINAL, "final"),
            (ACC_INTERFACE, "interface"),
            (ACC_ABSTRACT, "abstract"),
            (ACC_SYNTHETIC, "synthetic"),
            (ACC_ANNOTATION, "annotation"),
            (ACC_ENUM, "enum"),
            (ACC_PRIVATE, "private"),
            (ACC_PROTECTED, "protected"),
            (ACC_STATIC, "static"),
        ],
        FlagKind::Field => &[
            (ACC_PUBLIC, "public"),
            (ACC_PRIVATE, "private"),
            (ACC_PROTECTED, "protected"),
            (ACC_STATIC, "static"),
            (ACC_FINAL, "final"),
            (ACC_VOLATILE, "volatile"),
            (ACC_TRANSIENT, "transient"),
            (ACC_SYNTHETIC, "synthetic"),
            (ACC_ENUM, "enum"),
        ],
        FlagKind::Method => &[
            (ACC_PUBLIC, "public"),
            (ACC_PRIVATE, "private"),
            (ACC_PROTECTED, "protected"),
            (ACC_STATIC, "static"),
            (ACC_FINAL, "final"),
            (ACC_SYNCHRONIZED, "synchronized"),
            (ACC_BRIDGE, "bridge"),
            (ACC_VARARGS, "varargs"),
            (ACC_NATIVE, "native"),
            (ACC_ABSTRACT, "abstract"),
            (ACC_STRICT, "strict"),
            (ACC_SYNTHETIC, "synthetic"),
            (0x10000, "constructor"),
        ],
    };
    table
        .iter()
        .filter(|(flag, _)| access & flag != 0)
        .map(|(_, name)| *name)
        .collect()
}

#[derive(Debug, Clone, Copy)]
pub enum FlagKind {
    Class,
    Field,
    Method,
}

fn join_modifiers(mods: &[&str]) -> String {
    if mods.is_empty() {
        String::new()
    } else {
        format!("{} ", mods.join(" "))
    }
}

/// Declaration line of a method without body.
pub fn method_declaration(class: &ClassEntry, method: &MethodEntry) -> String {
    let in_interface = class.access & ACC_INTERFACE != 0;
    let mods = join_modifiers(&method_modifiers(method.access, in_interface));
    let params: Vec<String> = method
        .arg_types
        .iter()
        .enumerate()
        .map(|(i, ty)| format!("{ty} p{i}"))
        .collect();
    let params = params.join(", ");
    let body = if method.access & (ACC_ABSTRACT | ACC_NATIVE) != 0 {
        ";"
    } else {
        BODY_PLACEHOLDER
    };
    match method.name.as_str() {
        "<clinit>" => format!("static {body}"),
        "<init>" => format!("{mods}{}({params}) {body}", class.short_name),
        name => format!("{mods}{} {name}({params}) {body}", method.return_type),
    }
}

/// Skeleton of one method with the line of the enclosing class.
pub fn method_skeleton(class: &ClassEntry, method: &MethodEntry) -> String {
    format!(
        "// in {}\n{}\n",
        class.full_name,
        method_declaration(class, method)
    )
}

/// Skeleton of a whole class.
pub fn class_skeleton(
    class: &ClassEntry,
    fields: &[FieldEntry],
    methods: &[MethodEntry],
) -> String {
    let mut out = String::new();
    out.push_str("// Declaration skeleton: method bodies are not decompiled.\n");
    if !class.package.is_empty() {
        out.push_str(&format!("package {};\n\n", class.package));
    }
    let keyword = class_keyword(class.access);
    let mods = join_modifiers(&class_modifiers(class.access));
    out.push_str(&format!("{mods}{keyword} {}", class.short_name));
    if let Some(sup) = &class.super_class {
        let skip = sup == "java.lang.Object"
            || (class.access & ACC_ENUM != 0 && sup == "java.lang.Enum")
            || class.access & ACC_INTERFACE != 0;
        if !skip {
            out.push_str(&format!(" extends {sup}"));
        }
    }
    if !class.interfaces.is_empty() {
        let word = if class.access & ACC_INTERFACE != 0 {
            "extends"
        } else {
            "implements"
        };
        out.push_str(&format!(" {word} {}", class.interfaces.join(", ")));
    }
    out.push_str(" {\n");
    for field in fields {
        let mods = join_modifiers(&field_modifiers(field.access));
        out.push_str(&format!("    {mods}{} {}", field.ty, field.name));
        if let Some(value) = &field.const_value {
            out.push_str(&format!(" = {value}"));
        }
        out.push_str(";\n");
    }
    if !fields.is_empty() && !methods.is_empty() {
        out.push('\n');
    }
    for method in methods {
        out.push_str(&format!("    {}\n", method_declaration(class, method)));
    }
    out.push_str("}\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn class() -> ClassEntry {
        ClassEntry {
            id: "a.b.Foo".into(),
            full_name: "a.b.Foo".into(),
            short_name: "Foo".into(),
            package: "a.b".into(),
            access: ACC_PUBLIC | ACC_FINAL,
            super_class: Some("a.b.Base".into()),
            interfaces: vec!["java.lang.Runnable".into()],
            is_inner: false,
            method_count: 2,
            field_count: 1,
        }
    }

    #[test]
    fn skeleton_contains_declarations() {
        let methods = vec![
            MethodEntry {
                id: "a.b.Foo.run()V".into(),
                class_id: "a.b.Foo".into(),
                name: "run".into(),
                descriptor: "()V".into(),
                access: ACC_PUBLIC,
                arg_types: vec![],
                return_type: "void".into(),
                signature: None,
                has_code: true,
            },
            MethodEntry {
                id: "a.b.Foo.<init>(I)V".into(),
                class_id: "a.b.Foo".into(),
                name: "<init>".into(),
                descriptor: "(I)V".into(),
                access: ACC_PUBLIC,
                arg_types: vec!["int".into()],
                return_type: "void".into(),
                signature: None,
                has_code: true,
            },
        ];
        let fields = vec![FieldEntry {
            id: "a.b.Foo.X:I".into(),
            class_id: "a.b.Foo".into(),
            name: "X".into(),
            descriptor: "I".into(),
            ty: "int".into(),
            access: ACC_PUBLIC | ACC_STATIC | ACC_FINAL,
            signature: None,
            const_value: Some("5".into()),
        }];
        let text = class_skeleton(&class(), &fields, &methods);
        assert!(text.contains("package a.b;"));
        assert!(text
            .contains("public final class Foo extends a.b.Base implements java.lang.Runnable {"));
        assert!(text.contains("public static final int X = 5;"));
        assert!(text.contains("public void run() { /* body not decompiled */ }"));
        assert!(text.contains("public Foo(int p0) { /* body not decompiled */ }"));
    }

    #[test]
    fn flag_names_follow_kind() {
        assert_eq!(
            flag_names(0x4011, FlagKind::Class),
            ["public", "final", "enum"]
        );
        assert!(flag_names(0x10000 | ACC_STATIC, FlagKind::Method).contains(&"constructor"));
    }
}
