//! Developer subcommands for the class file reader (oracle comparison formats, see
//! `tests/oracle/java/JavaInputDump.java`):
//!
//!   jadx-rs java-classes <file.class|.jar>   class, field and method data per class
//!   jadx-rs java-insns   <file.class|.jar>   decoded instructions, tries and debug info per method

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::Path;

use anyhow::{Context, Result};
use jadx_input_api::{AnnotationRef, ConstValue, InsnIndexType, Opcode, Payload};
use jadx_java_input::{
    load_path, pack_annotations, Annotation, EncodedValue, JavaClass, MethodHandleRef,
};

pub fn run(cmd: &str, path: &Path) -> Result<()> {
    let res = load_path(path).with_context(|| format!("loading {}", path.display()))?;
    for w in &res.warnings {
        eprintln!("warning: {w}");
    }
    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());
    for cls in &res.classes {
        let mut text = String::new();
        let r = match cmd {
            "java-classes" => dump_class(&mut text, cls),
            _ => dump_insns(&mut text, cls),
        };
        if let Err(e) = r {
            eprintln!("warning: {}: {e}", cls.source());
            continue;
        }
        out.write_all(text.as_bytes())?;
    }
    Ok(())
}

fn escape(units: &[u16]) -> String {
    let mut s = String::from("\"");
    for &c in units {
        if (0x20..=0x7e).contains(&c) && c != b'\\' as u16 && c != b'"' as u16 {
            s.push(c as u8 as char);
        } else {
            let _ = write!(s, "\\u{c:04x}");
        }
    }
    s.push('"');
    s
}

fn float_bits(v: f32) -> u32 {
    if v.is_nan() {
        0x7fc0_0000
    } else {
        v.to_bits()
    }
}

fn double_bits(v: f64) -> u64 {
    if v.is_nan() {
        0x7ff8_0000_0000_0000
    } else {
        v.to_bits()
    }
}

fn proto(args: &[String], ret: &str) -> String {
    format!("({}){}", args.concat(), ret)
}

fn val(v: &EncodedValue) -> String {
    match v {
        EncodedValue::Null => "null".into(),
        EncodedValue::Boolean(b) => format!("BOOL:{b}"),
        EncodedValue::Byte(x) => format!("BYTE:{x}"),
        EncodedValue::Short(x) => format!("SHORT:{x}"),
        EncodedValue::Char(x) => format!("CHAR:{x}"),
        EncodedValue::Int(x) => format!("INT:{x}"),
        EncodedValue::Long(x) => format!("LONG:{x}"),
        EncodedValue::Float(x) => format!("FLOAT:0x{:x}", float_bits(*x)),
        EncodedValue::Double(x) => format!("DOUBLE:0x{:x}", double_bits(*x)),
        EncodedValue::String(s) => format!("STR:{}", escape(s)),
        EncodedValue::Type(t) => format!("TYPE:{t}"),
        EncodedValue::Enum(f) => format!("ENUM:{}->{}:{}", f.parent_class, f.name, f.ty),
        EncodedValue::MethodType(p) => format!("MTYPE:{}", proto(&p.arg_types, &p.return_type)),
        EncodedValue::MethodHandle(h) => match &h.target {
            MethodHandleRef::Field(f) => {
                format!(
                    "MH:{}:{}->{}:{}",
                    h.kind.name(),
                    f.parent_class,
                    f.name,
                    f.ty
                )
            }
            MethodHandleRef::Method(m) => format!(
                "MH:{}:{}->{}{}",
                h.kind.name(),
                m.parent_class,
                m.name,
                proto(&m.arg_types, &m.return_type)
            ),
        },
        EncodedValue::Array(items) => {
            format!("[{}]", items.iter().map(val).collect::<Vec<_>>().join(","))
        }
        EncodedValue::Annotation(a) => ann(a),
    }
}

fn ann(a: &Annotation) -> String {
    let values: Vec<String> = a
        .values
        .iter()
        .map(|(k, v)| format!("{k}={}", val(v)))
        .collect();
    format!(
        "@{}[{}]{{{}}}",
        a.class,
        a.visibility.name(),
        values.join(",")
    )
}

fn const_val(v: &ConstValue) -> String {
    match v {
        ConstValue::Boolean(b) => format!("BOOL:{b}"),
        ConstValue::Byte(x) => format!("BYTE:{x}"),
        ConstValue::Short(x) => format!("SHORT:{x}"),
        ConstValue::Char(x) => format!("CHAR:{x}"),
        ConstValue::Int(x) => format!("INT:{x}"),
        ConstValue::Long(x) => format!("LONG:{x}"),
        ConstValue::Float(b) => format!("FLOAT:0x{b:x}"),
        ConstValue::Double(b) => format!("DOUBLE:0x{b:x}"),
        ConstValue::Str(s) => format!("STR:{}", escape(s)),
    }
}

fn refs(list: &[AnnotationRef]) -> String {
    let mut v: Vec<String> = list
        .iter()
        .map(|r| match r {
            AnnotationRef::Class(c) => format!("Class:{c}"),
            AnnotationRef::Field(f) => format!("Field:{}->{}:{}", f.parent_class, f.name, f.ty),
        })
        .collect();
    v.sort();
    format!("[{}]", v.join(","))
}

fn opt<T: std::fmt::Display>(v: &Option<T>) -> String {
    match v {
        Some(x) => x.to_string(),
        None => "null".into(),
    }
}

fn annotations(out: &mut String, prefix: &str, list: &[Annotation]) {
    let mut packed = pack_annotations(list);
    packed.sort_by(|a, b| a.class.cmp(&b.class));
    for a in packed {
        let _ = writeln!(out, "{prefix} {}", ann(a));
    }
}

fn dump_class(out: &mut String, cls: &JavaClass) -> jadx_java_input::Result<()> {
    let api = cls.class_api()?;
    let _ = writeln!(
        out,
        "C {}\t{}\t0x{:x}\t{}\t{}",
        cls.source(),
        api.type_desc,
        api.access_flags,
        api.super_type.as_deref().unwrap_or("null"),
        api.interfaces.join(",")
    );
    let _ = writeln!(
        out,
        "CS sig={} src={}",
        opt(&api.signature),
        opt(&cls.source_file()?)
    );
    let mut inner = cls.inner_classes()?;
    inner.sort_by(|a, b| a.inner_class.cmp(&b.inner_class));
    let fmt_inner = |inner: &str, outer: &Option<String>, name: &Option<String>, flags: u32| {
        format!("{inner}|{}|{}|0x{flags:x}", opt(outer), opt(name))
    };
    for i in &inner {
        let _ = writeln!(
            out,
            "CI {}",
            fmt_inner(&i.inner_class, &i.outer_class, &i.name, i.access_flags)
        );
    }
    match &api.inner_class {
        Some(i) => {
            let _ = writeln!(
                out,
                "CN {}",
                fmt_inner(&api.type_desc, &i.outer_class, &i.name, i.access_flags)
            );
        }
        None => {
            let _ = writeln!(out, "CN none");
        }
    }
    annotations(out, "CA", &cls.annotations()?);
    let _ = writeln!(out, "CR {}", refs(&api.annotation_refs));
    let fields = cls.fields()?;
    for (f, a) in fields.iter().zip(&api.fields) {
        let _ = writeln!(out, "F {}\t{}\t0x{:x}", a.name, a.ty, a.access_flags);
        let _ = writeln!(
            out,
            "FS sig={} const={}",
            opt(&a.signature),
            a.const_value
                .as_ref()
                .map(const_val)
                .unwrap_or_else(|| "none".into())
        );
        annotations(out, "FA", &f.annotations);
        let _ = writeln!(out, "FR {}", refs(&a.annotation_refs));
    }
    let methods = cls.methods()?;
    for (m, a) in methods.iter().zip(&api.methods) {
        let _ = writeln!(
            out,
            "M {}{}\t0x{:x}\tcode={}",
            a.name,
            m.descriptor,
            a.access_flags,
            a.code_offset
                .map(|c| c.to_string())
                .unwrap_or_else(|| "none".into())
        );
        let throws = match &m.exceptions {
            Some(l) => format!("[{}]", l.join(",")),
            None => "none".into(),
        };
        let params = match &m.parameters {
            Some(l) => format!(
                "[{}]",
                l.iter()
                    .map(|p| format!("{}:0x{:x}", opt(&p.name), p.access_flags))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            None => "none".into(),
        };
        let dflt = m
            .annotation_default
            .as_ref()
            .map(val)
            .unwrap_or_else(|| "none".into());
        let _ = writeln!(
            out,
            "MS sig={} throws={throws} params={params} default={dflt}",
            opt(&a.signature)
        );
        annotations(out, "MA", &m.annotations);
        for (i, p) in m.param_annotations.iter().enumerate() {
            annotations(out, &format!("MP{i}"), p);
        }
        let _ = writeln!(out, "MR {}", refs(&a.annotation_refs));
    }
    Ok(())
}

fn dump_insns(out: &mut String, cls: &JavaClass) -> jadx_java_input::Result<()> {
    let ty = cls.type_desc()?;
    for m in cls.methods()? {
        let Some(code) = m.code_offset else {
            continue;
        };
        let proto = jadx_java_input::parse_method_descriptor(&m.descriptor)?;
        let info = cls.code_info(code)?;
        let _ = writeln!(
            out,
            "M {}->{}({}){} regs={} units={} args={}",
            ty,
            m.name,
            proto.arg_types.concat(),
            proto.return_type,
            info.registers_count(),
            info.code_length,
            info.args_start_reg()
        );
        let result = (|| -> jadx_java_input::Result<String> {
            let mut s = String::new();
            for d in cls.decode_insns(code)? {
                insn_line(&mut s, cls, &d)?;
            }
            for t in cls.tries(code)? {
                let c = &t.catch;
                let list: Vec<String> = c
                    .types
                    .iter()
                    .zip(&c.handlers)
                    .map(|(t, h)| format!("{}->{h}", if t.is_empty() { "null" } else { t }))
                    .collect();
                let _ = writeln!(
                    s,
                    "T {} {} [{}] all={}",
                    t.start,
                    t.end,
                    list.join(","),
                    c.catch_all
                );
            }
            match cls.debug_info(code)? {
                None => s.push_str("D none\n"),
                Some(di) => {
                    s.push_str("D lines=");
                    for (k, v) in &di.lines {
                        let _ = write!(s, "{k}={v},");
                    }
                    s.push('\n');
                    for v in &di.vars {
                        let _ = writeln!(
                            s,
                            "V {} {} {} {} {} {}",
                            v.reg,
                            opt(&v.name),
                            opt(&v.ty),
                            opt(&v.signature),
                            v.start,
                            v.end
                        );
                    }
                }
            }
            Ok(s)
        })();
        match result {
            Ok(s) => out.push_str(&s),
            Err(_) => out.push_str("E LOAD_ERROR\n"),
        }
    }
    Ok(())
}

fn insn_line(
    out: &mut String,
    cls: &JavaClass,
    d: &jadx_input_api::InsnData,
) -> jadx_java_input::Result<()> {
    let regs: Vec<String> = d.regs.iter().map(|r| (*r as i32).to_string()).collect();
    let it = match d.index_type {
        InsnIndexType::None => "NONE",
        InsnIndexType::TypeRef => "TYPE_REF",
        InsnIndexType::StringRef => "STRING_REF",
        InsnIndexType::FieldRef => "FIELD_REF",
        InsnIndexType::MethodRef => "METHOD_REF",
        InsnIndexType::CallSite => "CALL_SITE",
    };
    let _ = write!(
        out,
        "I {:x} {} r=[{}] l={} t={} i={} it={}",
        d.offset,
        d.opcode.name(),
        regs.join(","),
        d.literal,
        d.target,
        d.index,
        it
    );
    if d.opcode.name().starts_with("INVOKE_") {
        let _ = write!(out, " res={}", d.result_reg.map(|r| r as i32).unwrap_or(-1));
    }
    match &d.payload {
        Some(Payload::Switch(sp)) => {
            let ints = |v: &[i32]| {
                format!(
                    "[{}]",
                    v.iter()
                        .map(|x| x.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            let _ = write!(
                out,
                " switch keys={} targets={}",
                ints(&sp.keys),
                ints(&sp.targets)
            );
        }
        Some(Payload::Array(_)) => {}
        None => {
            if d.opcode == Opcode::InvokeCustom {
                let cs = cls.call_site(d.index)?;
                let _ = write!(
                    out,
                    " cs=[{}]",
                    cs.values.iter().map(val).collect::<Vec<_>>().join(",")
                );
            }
        }
    }
    out.push('\n');
    Ok(())
}
