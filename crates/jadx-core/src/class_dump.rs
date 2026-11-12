//! Text dump of the class level state after the pre decompile passes (oracle comparison format,
//! see `tests/oracle/java/ClassInfoDump.java`).

use std::fmt::Write;

use jadx_input_api::ConstValue;

use crate::model::const_storage::FieldRef;
use crate::model::info::FieldInfo;
use crate::model::node::{MethodId, Root};

fn escape(units: &[u16]) -> String {
    let mut s = String::new();
    for &u in units {
        if (0x20..0x7f).contains(&u) && u != u16::from(b'\\') {
            s.push(char::from(u as u8));
        } else {
            let _ = write!(s, "\\u{u:04x}");
        }
    }
    s
}

fn const_repr(v: &ConstValue) -> String {
    match v {
        ConstValue::Boolean(b) => format!("Z:{b}"),
        ConstValue::Byte(x) => format!("B:{x}"),
        ConstValue::Short(x) => format!("S:{x}"),
        ConstValue::Char(x) => format!("C:{x}"),
        ConstValue::Int(x) => format!("I:{x}"),
        ConstValue::Long(x) => format!("J:{x}"),
        ConstValue::Float(b) => format!("F:{b:08x}"),
        ConstValue::Double(b) => format!("D:{b:016x}"),
        ConstValue::Str(u) => format!("T:{}", escape(u)),
    }
}

fn field_info_id(f: &FieldInfo) -> String {
    format!("{}.{}", f.decl_class.raw_name(), f.short_id())
}

fn mth_id(root: &Root, m: MethodId) -> String {
    let (c, i) = m;
    format!(
        "{}.{}",
        root.classes[c].info.raw_name(),
        root.classes[c].methods[i].info.short_id()
    )
}

fn cls_list(root: &Root, l: &[usize]) -> String {
    let v: Vec<String> = l.iter().map(|c| root.classes[*c].info.raw_name()).collect();
    format!("[{}]", v.join(","))
}

fn mth_list(root: &Root, l: &[MethodId]) -> String {
    let v: Vec<String> = l.iter().map(|m| mth_id(root, *m)).collect();
    format!("[{}]", v.join(","))
}

fn field_ref_id(root: &Root, f: &FieldRef) -> String {
    match f {
        FieldRef::Node(c, i) => field_info_id(&root.classes[*c].fields[*i].info),
        FieldRef::Info(info) => field_info_id(info),
    }
}

/// One line per class, field and method plus the const storage, classes sorted by raw name.
pub fn dump_class_info(root: &Root) -> String {
    let mut out = String::new();
    for c in root.sorted_class_order() {
        let cls = &root.classes[c];
        let raw = cls.info.raw_name();
        let (outer, base, inline) = match &cls.anonymous {
            Some(a) => (
                root.classes[a.outer_class].info.raw_name(),
                a.base_type.to_string(),
                a.inline_type.name().to_string(),
            ),
            None => ("-".into(), "-".into(), "-".into()),
        };
        let inlined_in = cls
            .inlined_into
            .map_or("-".to_string(), |o| root.classes[o].info.raw_name());
        let _ = writeln!(
            out,
            "C {raw} access=0x{:x} anon={} dontGenerate={} inlinedIn={inlined_in} outer={outer} base={base} inline={inline} deps={} codegenDeps={} useIn={} useInMth={} inlined={}",
            cls.access,
            cls.is_anonymous(),
            cls.dont_generate,
            cls_list(root, &cls.dependencies),
            cls_list(root, &cls.codegen_deps),
            cls_list(root, &cls.use_in),
            mth_list(root, &cls.use_in_mth),
            cls_list(root, &cls.inlined_classes),
        );
        for f in &cls.fields {
            let _ = writeln!(
                out,
                "F {raw}.{} access=0x{:x} instFld={} const={} useIn={}",
                f.info.short_id(),
                f.access,
                f.inline_instance_field,
                f.const_value.as_ref().map_or("-".into(), const_repr),
                mth_list(root, &f.use_in),
            );
        }
        for (mi, m) in cls.methods.iter().enumerate() {
            let unresolved: Vec<String> = m
                .unresolved_used
                .iter()
                .map(|u| format!("{}.{}", u.decl_class.raw_name(), u.short_id()))
                .collect();
            let _ = writeln!(
                out,
                "M {} access=0x{:x} anonCtr={} inlineCand={} self={} useIn={} used={} unresolved=[{}]",
                mth_id(root, (c, mi)),
                m.access,
                m.anonymous_constructor,
                m.inline_candidate,
                m.calls_self,
                mth_list(root, &m.use_in),
                mth_list(root, &m.used),
                unresolved.join(","),
            );
        }
    }
    let mut global: Vec<String> = root
        .const_storage
        .global_const_fields()
        .iter()
        .map(|(v, f)| format!("K global {} {}", const_repr(v), field_ref_id(root, f)))
        .collect();
    global.sort();
    let mut local: Vec<String> = root
        .const_storage
        .class_const_fields()
        .map(|(c, v, f)| {
            format!(
                "K class {} {} {}",
                root.classes[c].info.raw_name(),
                const_repr(v),
                field_ref_id(root, f)
            )
        })
        .collect();
    local.sort();
    for l in global.into_iter().chain(local) {
        out.push_str(&l);
        out.push('\n');
    }
    out
}
