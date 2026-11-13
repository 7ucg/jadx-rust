//! JSON code generation (`--output-format json`): a fallback codegen mode for the GUI with a
//! bytecode offset mapping per generated line, ported from
//! `jadx.core.codegen.json.JsonCodeGen`.
//!
//! The JSON tree mirrors the reference's `JsonClass`/`JsonField`/`JsonMethod`/`JsonCodeLine`
//! (Gson, `LOWER_CASE_WITH_DASHES` field names, null fields omitted): `package`, `name`,
//! `alias`, `type` (`class`/`interface`/`enum`), `extends`, `implements`, `dex`, `declaration`,
//! `access-flags`, `fields`, `methods`, `inner-classes`, `imports` on the class; `name`,
//! `alias`, `declaration`, `access-flags` on a field; `name`, `alias`, `signature`,
//! `return-type`, `arguments`, `declaration`, `access-flags`, `lines`, `offset` on a method;
//! `code`, `source-line`, `offset` on a line of a method body.

use serde_json::{json, Map, Value};

use jadx_core::access::ACC_INTERFACE;
use jadx_core::codegen::method_gen::no_code_cfg;
use jadx_core::codegen::{ClassGen, CodeWriter, GenEnv, MethodGen};
use jadx_core::model::node::{ClassNode, MethodNode};
use jadx_core::model::types::{ArgType, CLASS_OBJECT};

/// `JsonCodeGen.process`: the JSON source of the top level class `cls` (inner classes nested
/// inside it), pretty printed.
pub fn process(env: &GenEnv<'_>, cls: usize) -> Result<String, String> {
    let value = process_cls(env, cls, None)?;
    serde_json::to_string_pretty(&value).map_err(|e| e.to_string())
}

fn plain_writer(env: &GenEnv<'_>) -> CodeWriter {
    CodeWriter::with_options(&env.code_indent_str, &env.code_new_line_str, false)
}

/// `JsonCodeGen.getTypeAlias`
fn type_alias(class_gen: &ClassGen<'_>, ty: &ArgType) -> Result<String, String> {
    let mut w = plain_writer(class_gen.env());
    class_gen.use_type(&mut w, ty).map_err(|e| e.msg)?;
    Ok(w.finish().code)
}

/// `JsonCodeGen.getClassTypeStr`
fn class_type_str(node: &ClassNode) -> &'static str {
    if node.is_enum() {
        "enum"
    } else if node.access & ACC_INTERFACE != 0 {
        "interface"
    } else {
        "class"
    }
}

/// `JsonCodeGen.processCls`
fn process_cls(
    env: &GenEnv<'_>,
    cls: usize,
    parent: Option<&ClassGen<'_>>,
) -> Result<Value, String> {
    let class_gen = match parent {
        Some(p) => ClassGen::with_parent(env, cls, p),
        None => ClassGen::new(env, cls),
    };
    let node = &env.root.classes[cls];

    let mut obj = Map::new();
    obj.insert("package".into(), json!(env.pkg(&node.info)));
    if let Some(dex) = &node.attrs.input_file_name {
        obj.insert("dex".into(), json!(dex));
    }
    obj.insert("name".into(), json!(node.info.full_name()));
    if node.alias.is_some() {
        obj.insert("alias".into(), json!(env.full_name(&node.info)));
    }
    obj.insert("type".into(), json!(class_type_str(node)));
    obj.insert("access-flags".into(), json!(node.access));

    if let Some(sup) = &node.super_class {
        if sup.object_name().as_deref() != Some(CLASS_OBJECT) && !node.remove_super_class {
            obj.insert("extends".into(), json!(type_alias(&class_gen, sup)?));
        }
    }
    if !node.interfaces.is_empty() {
        let mut interfaces = Vec::with_capacity(node.interfaces.len());
        for i in &node.interfaces {
            interfaces.push(type_alias(&class_gen, i)?);
        }
        obj.insert("implements".into(), json!(interfaces));
    }

    let mut decl_w = plain_writer(env);
    class_gen
        .add_class_declaration(&mut decl_w)
        .map_err(|e| e.msg)?;
    obj.insert("declaration".into(), json!(decl_w.finish().code));

    obj.insert("fields".into(), json!(build_fields(&class_gen, node)?));
    obj.insert(
        "methods".into(),
        json!(build_methods(env, &class_gen, cls)?),
    );

    let inner = &env.inner_classes[cls];
    if !inner.is_empty() {
        let mut inner_classes = Vec::new();
        for &ic in inner {
            if env.root.classes[ic].dont_generate {
                continue;
            }
            inner_classes.push(process_cls(env, ic, Some(&class_gen))?);
        }
        if !inner_classes.is_empty() {
            obj.insert("inner-classes".into(), json!(inner_classes));
        }
    }

    if !node.info.is_inner() {
        let mut imports: Vec<String> = class_gen
            .imports()
            .iter()
            .map(|ci| env.full_name(ci))
            .collect();
        imports.sort();
        obj.insert("imports".into(), json!(imports));
    }

    Ok(Value::Object(obj))
}

/// `JsonCodeGen.addFields`
fn build_fields(class_gen: &ClassGen<'_>, node: &ClassNode) -> Result<Vec<Value>, String> {
    let mut out = Vec::new();
    for f in &node.fields {
        if f.dont_generate {
            continue;
        }
        let mut obj = Map::new();
        obj.insert("name".into(), json!(f.info.name));
        if let Some(alias) = &f.alias {
            obj.insert("alias".into(), json!(alias));
        }
        let mut w = plain_writer(class_gen.env());
        class_gen.add_field(&mut w, f).map_err(|e| e.msg)?;
        obj.insert("declaration".into(), json!(w.finish().code));
        obj.insert("access-flags".into(), json!(f.access));
        out.push(Value::Object(obj));
    }
    Ok(out)
}

/// `JsonCodeGen.addMethods`
fn build_methods(
    env: &GenEnv<'_>,
    class_gen: &ClassGen<'_>,
    cls: usize,
) -> Result<Vec<Value>, String> {
    let node = &env.root.classes[cls];
    let mut out = Vec::new();
    for mi in 0..node.methods.len() {
        let m = &env.root.classes[cls].methods[mi];
        if m.dont_generate {
            continue;
        }
        let mut obj = Map::new();
        obj.insert("name".into(), json!(m.info.name));
        if let Some(alias) = &m.alias {
            obj.insert("alias".into(), json!(alias));
        }
        obj.insert("signature".into(), json!(m.info.short_id()));
        obj.insert(
            "return-type".into(),
            json!(type_alias(class_gen, &m.return_type)?),
        );
        let mut args = Vec::with_capacity(m.info.arg_types.len());
        for a in &m.info.arg_types {
            args.push(type_alias(class_gen, a)?);
        }
        obj.insert("arguments".into(), json!(args));

        let loaded = env.method_cfg(cls, mi);
        let mth_id = format!(
            "{}.{}",
            class_gen.class_info().full_name(),
            m.info.short_id()
        );
        let fallback_cfg;
        let cfg = match loaded.as_ref() {
            Ok(c) => c,
            Err(e) => {
                env.add_error(format!("Method load error in {mth_id}: {e}"));
                fallback_cfg = no_code_cfg(env.root, cls, mi);
                &fallback_cfg
            }
        };
        for e in &cfg.errors {
            env.add_error(format!("Method {mth_id}: {e}"));
        }
        for w in &cfg.warnings {
            env.add_warning(format!("Method {mth_id}: {w}"));
        }
        let mgen = MethodGen::new(class_gen, cfg, mi);

        let mut decl_w = plain_writer(env);
        mgen.add_definition(&mut decl_w).map_err(|e| e.msg)?;
        obj.insert("declaration".into(), json!(decl_w.finish().code));
        obj.insert("access-flags".into(), json!(m.access));
        obj.insert("lines".into(), json!(build_lines(env, &mth_id, m, &mgen)?));
        obj.insert(
            "offset".into(),
            json!(format!("0x{:x}", m.code_offset.unwrap_or(0))),
        );
        out.push(Value::Object(obj));
    }
    Ok(out)
}

/// `JsonCodeGen.fillMthCode`
fn build_lines(
    env: &GenEnv<'_>,
    mth_id: &str,
    m: &MethodNode,
    mgen: &MethodGen<'_>,
) -> Result<Vec<Value>, String> {
    let Some(code_offset) = m.code_offset else {
        return Ok(Vec::new());
    };
    let mut w = plain_writer(env);
    if let Err(e) = mgen.add_instructions(&mut w) {
        env.add_error(format!(
            "Method {mth_id}: Method code generation error: {}",
            e.msg
        ));
        return Ok(vec![
            json!({ "code": format!("/* JADX ERROR: Method generation error: {} */", e.msg) }),
        ]);
    }
    let info = w.finish();
    if info.code.is_empty() {
        return Ok(Vec::new());
    }
    // The reference writer never strips its leading (always empty) line, so its first split
    // element is always `""`; this writer's `finish` already strips it (`CodeWriter::finish`),
    // put it back so the line numbers below line up the same way as the reference's `i + 2`.
    let mth_code_offset = code_offset as u64 + 16;
    let mut lines: Vec<&str> = info.code.split(env.code_new_line_str.as_str()).collect();
    lines.insert(0, "");
    let mut out = Vec::with_capacity(lines.len());
    for (i, line) in lines.iter().enumerate() {
        let writer_line = i + 1;
        let mut obj = Map::new();
        obj.insert("code".into(), json!(line));
        if let Some(src) = info.line_map.get(&writer_line) {
            obj.insert("source-line".into(), json!(src));
        }
        if let Some(off) = info.offset_map.get(&writer_line) {
            let abs = mth_code_offset + (*off as u64) * 2;
            obj.insert("offset".into(), json!(format!("0x{abs:x}")));
        }
        out.push(Value::Object(obj));
    }
    Ok(out)
}
