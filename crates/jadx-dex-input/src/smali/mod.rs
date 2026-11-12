//! Smali text output in the format of the reference disassembler (baksmali), without debug info.
//!
//! Layout rules (blank lines, label naming/ordering, escaping) mirror the reference output and
//! are verified by `tests/oracle/smali_oracle.sh`.

mod javafmt;

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt::Write as _;

use crate::annotations::{Annotation, ClassAnnotations, EncodedAnnotation, EncodedValue};
use crate::class_data::{CodeItem, EncodedField, EncodedMethod};
use crate::error::Result;
use crate::file::{ClassDef, DexFile};
use crate::insns::{decode_all, Format, Insn, Payload, RefKind};

use javafmt::{double_comment, float_comment};
pub use javafmt::{double_to_string, float_to_string};

const ACC_STATIC: u32 = 0x8;

/// Access flag names in output order, with the contexts they are valid in.
struct Flag {
    value: u32,
    name: &'static str,
    class: bool,
    method: bool,
    field: bool,
}

const FLAGS: &[Flag] = &[
    Flag {
        value: 0x1,
        name: "public",
        class: true,
        method: true,
        field: true,
    },
    Flag {
        value: 0x2,
        name: "private",
        class: true,
        method: true,
        field: true,
    },
    Flag {
        value: 0x4,
        name: "protected",
        class: true,
        method: true,
        field: true,
    },
    Flag {
        value: 0x8,
        name: "static",
        class: true,
        method: true,
        field: true,
    },
    Flag {
        value: 0x10,
        name: "final",
        class: true,
        method: true,
        field: true,
    },
    Flag {
        value: 0x20,
        name: "synchronized",
        class: false,
        method: true,
        field: false,
    },
    Flag {
        value: 0x40,
        name: "volatile",
        class: false,
        method: false,
        field: true,
    },
    Flag {
        value: 0x40,
        name: "bridge",
        class: false,
        method: true,
        field: false,
    },
    Flag {
        value: 0x80,
        name: "transient",
        class: false,
        method: false,
        field: true,
    },
    Flag {
        value: 0x80,
        name: "varargs",
        class: false,
        method: true,
        field: false,
    },
    Flag {
        value: 0x100,
        name: "native",
        class: false,
        method: true,
        field: false,
    },
    Flag {
        value: 0x200,
        name: "interface",
        class: true,
        method: false,
        field: false,
    },
    Flag {
        value: 0x400,
        name: "abstract",
        class: true,
        method: true,
        field: false,
    },
    Flag {
        value: 0x800,
        name: "strictfp",
        class: false,
        method: true,
        field: false,
    },
    Flag {
        value: 0x1000,
        name: "synthetic",
        class: true,
        method: true,
        field: true,
    },
    Flag {
        value: 0x2000,
        name: "annotation",
        class: true,
        method: false,
        field: false,
    },
    Flag {
        value: 0x4000,
        name: "enum",
        class: true,
        method: false,
        field: true,
    },
    Flag {
        value: 0x10000,
        name: "constructor",
        class: false,
        method: true,
        field: false,
    },
    Flag {
        value: 0x20000,
        name: "declared-synchronized",
        class: false,
        method: true,
        field: false,
    },
];

#[derive(Clone, Copy)]
enum Scope {
    Class,
    Method,
    Field,
}

/// Each flag followed by a space, e.g. `"public static "`.
fn access_flags(flags: u32, scope: Scope) -> String {
    let mut s = String::new();
    for f in FLAGS {
        let valid = match scope {
            Scope::Class => f.class,
            Scope::Method => f.method,
            Scope::Field => f.field,
        };
        if valid && flags & f.value != 0 {
            s.push_str(f.name);
            s.push(' ');
        }
    }
    s
}

/// Line-oriented output with indentation.
struct Out {
    buf: String,
    indent: usize,
}

impl Out {
    fn new() -> Self {
        Out {
            buf: String::new(),
            indent: 0,
        }
    }
    fn line(&mut self, text: &str) {
        if !text.is_empty() {
            for _ in 0..self.indent {
                self.buf.push(' ');
            }
            self.buf.push_str(text);
        }
        self.buf.push('\n');
    }
    fn blank(&mut self) {
        self.buf.push('\n');
    }
    fn start(&mut self) {
        for _ in 0..self.indent {
            self.buf.push(' ');
        }
    }
    fn push(&mut self, s: &str) {
        self.buf.push_str(s);
    }
    fn end(&mut self) {
        self.buf.push('\n');
    }
}

/// Escape a UTF-16 string the way the reference writer does (inside double quotes).
fn quoted_string(units: &[u16]) -> String {
    let mut s = String::with_capacity(units.len() + 2);
    s.push('"');
    for &c in units {
        match c {
            0x22 => s.push_str("\\\""),
            0x27 => s.push_str("\\'"),
            0x5c => s.push_str("\\\\"),
            0x0a => s.push_str("\\n"),
            0x0d => s.push_str("\\r"),
            0x09 => s.push_str("\\t"),
            0x20..=0x7e => s.push(c as u8 as char),
            _ => {
                let _ = write!(s, "\\u{c:04x}");
            }
        }
    }
    s.push('"');
    s
}

/// Character literal as written by the reference writer.
fn quoted_char(c: u16) -> String {
    match c {
        0x20..=0x7e => {
            let ch = c as u8 as char;
            if matches!(ch, '\'' | '"' | '\\') {
                format!("'\\{ch}'")
            } else {
                format!("'{ch}'")
            }
        }
        0x09 => "'\\t'".into(),
        0x0a => "'\\n'".into(),
        0x0d => "'\\r'".into(),
        _ => format!("'\\u{c:04x}'"),
    }
}

/// `0x..`/`-0x..` with an `L` suffix only when the value does not fit an int.
fn hex_int_or_long(v: i64) -> String {
    let mut s = hex_signed(v);
    if v < i32::MIN as i64 || v > i32::MAX as i64 {
        s.push('L');
    }
    s
}

fn hex_signed(v: i64) -> String {
    if v < 0 {
        format!("-0x{:x}", (v as i128).unsigned_abs())
    } else {
        format!("0x{v:x}")
    }
}

fn param_width(ty: &str) -> u32 {
    if ty == "J" || ty == "D" {
        2
    } else {
        1
    }
}

struct Ctx<'a> {
    dex: &'a DexFile,
}

impl<'a> Ctx<'a> {
    fn type_desc(&self, idx: u32) -> Result<String> {
        self.dex.type_descriptor(idx)
    }

    fn field_text(&self, idx: u32) -> Result<String> {
        let f = self.dex.field_ref(idx)?;
        Ok(format!("{}->{}:{}", f.class, f.name, f.ty))
    }

    fn proto_text(&self, idx: u32) -> Result<String> {
        let p = self.dex.proto_id(idx)?;
        let args = self.dex.type_list(p.parameters_off)?.concat();
        Ok(format!(
            "({}){}",
            args,
            self.dex.type_descriptor(p.return_type_idx)?
        ))
    }

    fn method_text(&self, idx: u32) -> Result<String> {
        let m = self.dex.method_ref(idx)?;
        Ok(format!(
            "{}->{}({}){}",
            m.class,
            m.name,
            m.arg_types.concat(),
            m.return_type
        ))
    }

    fn string_text(&self, idx: u32) -> Result<String> {
        Ok(quoted_string(&self.dex.string_utf16(idx)?))
    }

    fn write_value(&self, out: &mut Out, v: &EncodedValue) -> Result<()> {
        match v {
            EncodedValue::Byte(b) => out.push(&format!("{}t", hex_signed(*b as i64))),
            EncodedValue::Short(s) => out.push(&format!("{}s", hex_signed(*s as i64))),
            EncodedValue::Char(c) => out.push(&quoted_char(*c)),
            EncodedValue::Int(i) => out.push(&hex_signed(*i as i64)),
            EncodedValue::Long(l) => out.push(&format!("{}L", hex_signed(*l))),
            EncodedValue::Float(f) => out.push(&format!("{}f", float_to_string(*f))),
            EncodedValue::Double(d) => out.push(&double_to_string(*d)),
            EncodedValue::MethodType(i) => out.push(&self.proto_text(*i)?),
            EncodedValue::MethodHandle(i) => out.push(&self.method_handle_text(*i)?),
            EncodedValue::String(i) => out.push(&self.string_text(*i)?),
            EncodedValue::Type(i) => out.push(&self.type_desc(*i)?),
            EncodedValue::Field(i) => out.push(&self.field_text(*i)?),
            EncodedValue::Method(i) => out.push(&self.method_text(*i)?),
            EncodedValue::Enum(i) => out.push(&format!(".enum {}", self.field_text(*i)?)),
            EncodedValue::Array(items) if items.is_empty() => out.push("{}"),
            EncodedValue::Array(items) => {
                out.push("{");
                out.end();
                out.indent += 4;
                for (n, item) in items.iter().enumerate() {
                    out.start();
                    self.write_value(out, item)?;
                    if n + 1 < items.len() {
                        out.push(",");
                    }
                    out.end();
                }
                out.indent -= 4;
                out.start();
                out.push("}");
            }
            EncodedValue::Annotation(a) => {
                out.push(&format!(".subannotation {}", self.type_desc(a.type_idx)?));
                out.end();
                self.write_elements(out, a)?;
                out.start();
                out.push(".end subannotation");
            }
            EncodedValue::Null => out.push("null"),
            EncodedValue::Boolean(b) => out.push(if *b { "true" } else { "false" }),
        }
        Ok(())
    }

    fn method_handle_text(&self, idx: u32) -> Result<String> {
        let (kind, ref_idx) = self.dex.method_handle(idx)?;
        let (name, is_field) = match kind {
            0 => ("static-put", true),
            1 => ("static-get", true),
            2 => ("instance-put", true),
            3 => ("instance-get", true),
            4 => ("invoke-static", false),
            5 => ("invoke-instance", false),
            6 => ("invoke-constructor", false),
            7 => ("invoke-direct", false),
            _ => ("invoke-interface", false),
        };
        let target = if is_field {
            self.field_text(ref_idx)?
        } else {
            self.method_text(ref_idx)?
        };
        Ok(format!("{name}@{target}"))
    }

    fn write_elements(&self, out: &mut Out, a: &EncodedAnnotation) -> Result<()> {
        out.indent += 4;
        for (name_idx, value) in &a.elements {
            out.start();
            out.push(&self.dex.string(*name_idx)?);
            out.push(" = ");
            self.write_value(out, value)?;
            out.end();
        }
        out.indent -= 4;
        Ok(())
    }

    fn write_annotations(&self, out: &mut Out, anns: &[Annotation]) -> Result<()> {
        for (i, a) in anns.iter().enumerate() {
            if i > 0 {
                out.blank();
            }
            out.line(&format!(
                ".annotation {} {}",
                a.visibility.name(),
                self.type_desc(a.annotation.type_idx)?
            ));
            self.write_elements(out, &a.annotation)?;
            out.line(".end annotation");
        }
        Ok(())
    }
}

/// Render one class as smali (reference-disassembler layout, no debug info).
pub fn class_to_smali(dex: &DexFile, def: &ClassDef) -> Result<String> {
    let cx = Ctx { dex };
    let mut out = Out::new();
    let class_type = dex.type_descriptor(def.class_idx)?;
    out.line(&format!(
        ".class {}{}",
        access_flags(def.access_flags, Scope::Class),
        class_type
    ));
    if let Some(sup) = dex.opt_type_descriptor(def.superclass_idx)? {
        out.line(&format!(".super {sup}"));
    }
    if def.source_file_idx != crate::header::NO_INDEX {
        out.line(&format!(
            ".source {}",
            quoted_string(&dex.string_utf16(def.source_file_idx)?)
        ));
    }
    let ifaces = dex.type_list(def.interfaces_off)?;
    if !ifaces.is_empty() {
        out.blank();
        out.line("# interfaces");
        for i in &ifaces {
            out.line(&format!(".implements {i}"));
        }
    }
    let anns = dex.class_annotations(def)?;
    if !anns.class.is_empty() {
        out.blank();
        out.blank();
        out.line("# annotations");
        cx.write_annotations(&mut out, &anns.class)?;
    }
    let Some(data) = dex.class_data(def)? else {
        return Ok(out.buf);
    };

    let static_values = dex.static_values(def)?;
    let set_in_clinit = fields_set_in_clinit(dex, &class_type, &data.direct_methods)?;

    write_fields(
        &cx,
        &mut out,
        "# static fields",
        &data.static_fields,
        &anns,
        Some((&static_values, &set_in_clinit)),
    )?;
    write_fields(
        &cx,
        &mut out,
        "# instance fields",
        &data.instance_fields,
        &anns,
        None,
    )?;
    write_methods(
        &cx,
        &mut out,
        "# direct methods",
        &data.direct_methods,
        &anns,
    )?;
    write_methods(
        &cx,
        &mut out,
        "# virtual methods",
        &data.virtual_methods,
        &anns,
    )?;
    Ok(out.buf)
}

fn is_default_value(v: &EncodedValue) -> bool {
    match v {
        EncodedValue::Byte(0) | EncodedValue::Short(0) | EncodedValue::Char(0) => true,
        EncodedValue::Int(0) | EncodedValue::Long(0) | EncodedValue::Null => true,
        EncodedValue::Boolean(false) => true,
        EncodedValue::Float(f) => *f == 0.0,
        EncodedValue::Double(d) => *d == 0.0,
        _ => false,
    }
}

/// Names (`name:type`) of this class' fields written by `sput*` in `<clinit>`.
fn fields_set_in_clinit(
    dex: &DexFile,
    class_type: &str,
    direct: &[EncodedMethod],
) -> Result<HashSet<String>> {
    let mut set = HashSet::new();
    for m in direct {
        if m.code_off == 0 {
            continue;
        }
        let mr = dex.method_id(m.method_idx)?;
        if dex.string(mr.name_idx)? != "<clinit>" {
            continue;
        }
        let code = dex.code_item(m.code_off)?;
        for insn in decode_all(&code.insns)? {
            if insn.info.name.starts_with("sput") && insn.info.ref1 == RefKind::Field {
                let f = dex.field_ref(insn.index)?;
                if f.class == class_type {
                    set.insert(format!("{}:{}", f.name, f.ty));
                }
            }
        }
    }
    Ok(set)
}

fn write_fields(
    cx: &Ctx<'_>,
    out: &mut Out,
    title: &str,
    fields: &[EncodedField],
    anns: &ClassAnnotations,
    statics: Option<(&Vec<EncodedValue>, &HashSet<String>)>,
) -> Result<()> {
    if fields.is_empty() {
        return Ok(());
    }
    out.blank();
    out.blank();
    out.line(title);
    for (i, f) in fields.iter().enumerate() {
        if i > 0 {
            out.blank();
        }
        let fr = cx.dex.field_ref(f.field_idx)?;
        let mut initial = None;
        if let Some((values, in_clinit)) = statics {
            initial = values.get(i);
            let is_static_final = f.access_flags & ACC_STATIC != 0 && f.access_flags & 0x10 != 0;
            if let Some(v) = initial {
                if in_clinit.contains(&format!("{}:{}", fr.name, fr.ty)) && is_static_final {
                    if is_default_value(v) {
                        initial = None;
                    } else {
                        out.line("# The value of this static final field might be set in the static constructor");
                    }
                }
            }
        }
        out.start();
        out.push(&format!(
            ".field {}{}:{}",
            access_flags(f.access_flags, Scope::Field),
            fr.name,
            fr.ty
        ));
        if let Some(v) = initial {
            out.push(" = ");
            cx.write_value(out, v)?;
        }
        out.end();
        if let Some(list) = anns.fields.get(&f.field_idx) {
            if !list.is_empty() {
                out.indent += 4;
                cx.write_annotations(out, list)?;
                out.indent -= 4;
                out.line(".end field");
            }
        }
    }
    Ok(())
}

fn write_methods(
    cx: &Ctx<'_>,
    out: &mut Out,
    title: &str,
    methods: &[EncodedMethod],
    anns: &ClassAnnotations,
) -> Result<()> {
    if methods.is_empty() {
        return Ok(());
    }
    out.blank();
    out.blank();
    out.line(title);
    for (i, m) in methods.iter().enumerate() {
        if i > 0 {
            out.blank();
        }
        write_method(cx, out, m, anns)?;
    }
    Ok(())
}

fn write_method(
    cx: &Ctx<'_>,
    out: &mut Out,
    m: &EncodedMethod,
    anns: &ClassAnnotations,
) -> Result<()> {
    let dex = cx.dex;
    let mr = dex.method_ref(m.method_idx)?;
    out.line(&format!(
        ".method {}{}({}){}",
        access_flags(m.access_flags, Scope::Method),
        mr.name,
        mr.arg_types.concat(),
        mr.return_type
    ));
    out.indent += 4;
    let code = if m.code_off != 0 {
        Some(dex.code_item(m.code_off)?)
    } else {
        None
    };
    if let Some(c) = &code {
        out.line(&format!(".registers {}", c.registers_size));
    }
    // parameters with annotations
    if let Some(param_anns) = anns.parameters.get(&m.method_idx) {
        let mut reg = if m.access_flags & ACC_STATIC != 0 {
            0
        } else {
            1
        };
        for (i, ty) in mr.arg_types.iter().enumerate() {
            let list = param_anns.get(i).map(Vec::as_slice).unwrap_or(&[]);
            if !list.is_empty() {
                out.line(&format!(".param p{reg}    # {ty}"));
                out.indent += 4;
                cx.write_annotations(out, list)?;
                out.indent -= 4;
                out.line(".end param");
            }
            reg += param_width(ty);
        }
    }
    if let Some(list) = anns.methods.get(&m.method_idx) {
        cx.write_annotations(out, list)?;
    }
    if let Some(c) = &code {
        let param_regs = (if m.access_flags & ACC_STATIC != 0 {
            0
        } else {
            1
        }) + mr.arg_types.iter().map(|t| param_width(t)).sum::<u32>();
        write_code(cx, out, c, param_regs)?;
    }
    out.indent -= 4;
    out.line(".end method");
    Ok(())
}

struct RegNames {
    first_param: i64,
}

impl RegNames {
    fn name(&self, r: u32) -> String {
        if (r as i64) >= self.first_param {
            format!("p{}", r as i64 - self.first_param)
        } else {
            format!("v{r}")
        }
    }
}

fn write_code(cx: &Ctx<'_>, out: &mut Out, code: &CodeItem, param_regs: u32) -> Result<()> {
    let insns = decode_all(&code.insns)?;
    let code_units = code.insns.len() as u32;
    let regs = RegNames {
        first_param: code.registers_size as i64 - param_regs as i64,
    };

    // payload address -> address of the switch instruction referencing it
    let mut payload_base: BTreeMap<u32, u32> = BTreeMap::new();
    let mut labels: BTreeMap<u32, BTreeSet<String>> = BTreeMap::new();
    let mut add_label = |addr: i64, prefix: &str| {
        labels
            .entry(addr as u32)
            .or_default()
            .insert(format!("{prefix}{:x}", addr as u32));
    };
    for insn in &insns {
        let at = insn.offset as i64;
        let target = at + insn.target as i64;
        match insn.info.format {
            Format::Format10t | Format::Format20t | Format::Format30t => add_label(target, "goto_"),
            Format::Format21t | Format::Format22t => add_label(target, "cond_"),
            Format::Format31t => match insn.info.name {
                "packed-switch" => {
                    payload_base.insert(target as u32, insn.offset);
                    add_label(target, "pswitch_data_");
                }
                "sparse-switch" => {
                    payload_base.insert(target as u32, insn.offset);
                    add_label(target, "sswitch_data_");
                }
                _ => add_label(target, "array_"),
            },
            _ => {}
        }
    }
    for insn in &insns {
        match &insn.payload {
            Some(Payload::PackedSwitch { targets, .. }) => {
                let base = payload_base.get(&insn.offset).copied().unwrap_or(0) as i64;
                for t in targets {
                    add_label(base + *t as i64, "pswitch_");
                }
            }
            Some(Payload::SparseSwitch { targets, .. }) => {
                let base = payload_base.get(&insn.offset).copied().unwrap_or(0) as i64;
                for t in targets {
                    add_label(base + *t as i64, "sswitch_");
                }
            }
            _ => {}
        }
    }

    // try/catch: start labels, end labels, handler labels and `.catch` directives (at try end)
    let mut end_labels: BTreeMap<u32, BTreeSet<String>> = BTreeMap::new();
    let mut catches: BTreeMap<u32, Vec<String>> = BTreeMap::new();
    for t in &code.tries {
        let start = t.start_addr;
        let end = start.wrapping_add(t.insn_count as u32);
        add_label(start as i64, "try_start_");
        end_labels
            .entry(end)
            .or_default()
            .insert(format!("try_end_{end:x}"));
        let h = &code.handlers[t.handler_idx];
        let range = format!("{{:try_start_{start:x} .. :try_end_{end:x}}}");
        for (type_idx, addr) in &h.handlers {
            add_label(*addr as i64, "catch_");
            catches.entry(end).or_default().push(format!(
                ".catch {} {} :catch_{:x}",
                cx.type_desc(*type_idx)?,
                range,
                addr
            ));
        }
        if let Some(addr) = h.catch_all {
            add_label(addr as i64, "catchall_");
            catches
                .entry(end)
                .or_default()
                .push(format!(".catchall {range} :catchall_{addr:x}"));
        }
    }

    let emit_end = |out: &mut Out, addr: u32| {
        if let Some(set) = end_labels.get(&addr) {
            for l in set {
                out.line(&format!(":{l}"));
            }
        }
        if let Some(list) = catches.get(&addr) {
            for c in list {
                out.line(c);
            }
        }
    };

    for insn in &insns {
        emit_end(out, insn.offset);
        out.blank();
        if let Some(set) = labels.get(&insn.offset) {
            for l in set {
                out.line(&format!(":{l}"));
            }
        }
        write_insn(cx, out, insn, &regs, &payload_base)?;
    }
    emit_end(out, code_units);
    Ok(())
}

fn reg_range(regs: &RegNames, insn: &Insn) -> String {
    let count = insn.literal as u32;
    let mut s = String::from("{");
    if count > 0 {
        let start = insn.range_start();
        // like the reference: a range is either all `p` registers or all `v` registers
        let end = start + count - 1;
        if (start as i64) >= regs.first_param {
            let _ = write!(
                s,
                "p{} .. p{}",
                start as i64 - regs.first_param,
                end as i64 - regs.first_param
            );
        } else {
            let _ = write!(s, "v{start} .. v{end}");
        }
    }
    s.push('}');
    s
}

fn reg_list(regs: &RegNames, insn: &Insn) -> String {
    let names: Vec<String> = insn.regs().iter().map(|r| regs.name(*r)).collect();
    format!("{{{}}}", names.join(", "))
}

fn write_insn(
    cx: &Ctx<'_>,
    out: &mut Out,
    insn: &Insn,
    regs: &RegNames,
    payload_base: &BTreeMap<u32, u32>,
) -> Result<()> {
    let name = insn.info.name;
    let r = |i: usize| regs.name(insn.regs[i]);
    let label_of = |prefix: &str| {
        format!(
            ":{prefix}{:x}",
            (insn.offset as i64 + insn.target as i64) as u32
        )
    };
    let reference = |kind: RefKind, idx: u32| -> Result<String> {
        match kind {
            RefKind::String => cx.string_text(idx),
            RefKind::Type => cx.type_desc(idx),
            RefKind::Field => cx.field_text(idx),
            RefKind::Method => cx.method_text(idx),
            RefKind::Proto => cx.proto_text(idx),
            RefKind::MethodHandle => cx.method_handle_text(idx),
            RefKind::CallSite => cx.call_site_text(idx),
            RefKind::None => Ok(String::new()),
        }
    };
    use Format::*;
    let text = match insn.info.format {
        Format10x => name.to_string(),
        Format10t | Format20t | Format30t => format!("{name} {}", label_of("goto_")),
        Format11n => format!("{name} {}, {}", r(0), hex_int_or_long(insn.literal)),
        Format21s | Format21ih | Format21lh | Format31i | Format51l => {
            let mut t = format!("{name} {}, {}", r(0), hex_int_or_long(insn.literal));
            let comment = if name.starts_with("const-wide") {
                double_comment(insn.literal)
            } else {
                float_comment(insn.literal as i32)
            };
            if let Some(c) = comment {
                t.push_str("    # ");
                t.push_str(&c);
            }
            t
        }
        Format11x => format!("{name} {}", r(0)),
        Format12x | Format22x | Format32x => format!("{name} {}, {}", r(0), r(1)),
        Format21c | Format31c => {
            format!(
                "{name} {}, {}",
                r(0),
                reference(insn.info.ref1, insn.index)?
            )
        }
        Format21t => format!("{name} {}, {}", r(0), label_of("cond_")),
        Format22b | Format22s => format!("{name} {}, {}, {}", r(0), r(1), hex_signed(insn.literal)),
        Format22c => format!(
            "{name} {}, {}, {}",
            r(0),
            r(1),
            reference(insn.info.ref1, insn.index)?
        ),
        Format22t => format!("{name} {}, {}, {}", r(0), r(1), label_of("cond_")),
        Format23x => format!("{name} {}, {}, {}", r(0), r(1), r(2)),
        Format31t => {
            let prefix = match name {
                "packed-switch" => "pswitch_data_",
                "sparse-switch" => "sswitch_data_",
                _ => "array_",
            };
            format!("{name} {}, {}", r(0), label_of(prefix))
        }
        Format35c => format!(
            "{name} {}, {}",
            reg_list(regs, insn),
            reference(insn.info.ref1, insn.index)?
        ),
        Format3rc => format!(
            "{name} {}, {}",
            reg_range(regs, insn),
            reference(insn.info.ref1, insn.index)?
        ),
        Format45cc => format!(
            "{name} {}, {}, {}",
            reg_list(regs, insn),
            reference(insn.info.ref1, insn.index)?,
            reference(insn.info.ref2, insn.index2)?
        ),
        Format4rcc => format!(
            "{name} {}, {}, {}",
            reg_range(regs, insn),
            reference(insn.info.ref1, insn.index)?,
            reference(insn.info.ref2, insn.index2)?
        ),
        PackedSwitchPayload | SparseSwitchPayload | ArrayPayload => {
            return write_payload(out, insn, payload_base);
        }
    };
    out.line(&text);
    Ok(())
}

fn write_payload(out: &mut Out, insn: &Insn, payload_base: &BTreeMap<u32, u32>) -> Result<()> {
    let base = payload_base.get(&insn.offset).copied().unwrap_or(0) as i64;
    let lab = |prefix: &str, t: i32| format!(":{prefix}{:x}", (base + t as i64) as u32);
    match insn.payload.as_ref().expect("payload decoded") {
        Payload::PackedSwitch { first_key, targets } => {
            out.line(&format!(".packed-switch {}", hex_signed(*first_key as i64)));
            out.indent += 4;
            for t in targets {
                out.line(&lab("pswitch_", *t));
            }
            out.indent -= 4;
            out.line(".end packed-switch");
        }
        Payload::SparseSwitch { keys, targets } => {
            out.line(".sparse-switch");
            out.indent += 4;
            for (k, t) in keys.iter().zip(targets) {
                out.line(&format!(
                    "{} -> {}",
                    hex_signed(*k as i64),
                    lab("sswitch_", *t)
                ));
            }
            out.indent -= 4;
            out.line(".end sparse-switch");
        }
        Payload::ArrayData {
            element_width,
            data,
        } => {
            out.line(&format!(".array-data {element_width}"));
            out.indent += 4;
            let w = *element_width as usize;
            for chunk in data.chunks(w.max(1)) {
                let mut v: u64 = 0;
                for (i, b) in chunk.iter().enumerate() {
                    v |= (*b as u64) << (8 * i);
                }
                let signed = if w >= 8 || w == 0 {
                    v as i64
                } else {
                    let shift = 64 - 8 * w as u32;
                    ((v << shift) as i64) >> shift
                };
                let suffix = match w {
                    1 => "t",
                    2 => "s",
                    _ => "",
                };
                let mut text = format!("{}{}", hex_int_or_long(signed), suffix);
                let comment = match w {
                    8 => double_comment(signed),
                    4 => float_comment(signed as i32),
                    _ => None,
                };
                if let Some(c) = comment {
                    text.push_str("    # ");
                    text.push_str(&c);
                }
                out.line(&text);
            }
            out.indent -= 4;
            out.line(".end array-data");
        }
    }
    Ok(())
}

impl<'a> Ctx<'a> {
    /// `call_site_N("name", proto, extra...)@linker method`
    fn call_site_text(&self, idx: u32) -> Result<String> {
        let items = self.dex.call_site(idx)?;
        let bad = || crate::error::DexError::BadContainer(format!("malformed call site {idx}"));
        let (
            EncodedValue::MethodHandle(handle),
            EncodedValue::String(name),
            EncodedValue::MethodType(proto),
        ) = (
            items.first().ok_or_else(bad)?,
            items.get(1).ok_or_else(bad)?,
            items.get(2).ok_or_else(bad)?,
        )
        else {
            return Err(bad());
        };
        let mut out = Out::new();
        out.push(&format!(
            "call_site_{idx}({}, {}",
            self.string_text(*name)?,
            self.proto_text(*proto)?
        ));
        for extra in &items[3..] {
            out.push(", ");
            self.write_value(&mut out, extra)?;
        }
        out.push(")@");
        let (kind, ref_idx) = self.dex.method_handle(*handle)?;
        if kind != 4 {
            return Err(bad());
        }
        out.push(&self.method_text(ref_idx)?);
        Ok(out.buf)
    }
}

impl DexFile {
    /// Smali text of the class at `class_def` index (reference-disassembler layout).
    pub fn class_smali(&self, class_def_idx: u32) -> Result<String> {
        let def = self.class_def(class_def_idx)?;
        class_to_smali(self, &def)
    }
}
