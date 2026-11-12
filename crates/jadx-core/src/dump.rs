//! Canonical text dump of instruction nodes, used by the oracle comparisons
//! (`tests/oracle/java/InsnNodeDump.java` prints the same format from the reference).

use std::fmt::Write as _;

use jadx_input_api::ArrayData;

use crate::model::insn::{FillArrayData, IndexValue, InsnArg, InsnKind, InsnNode, RegisterArg};

fn reg(r: &RegisterArg) -> String {
    format!(
        "r{}:{}{}",
        r.reg_num,
        r.init_type,
        if r.immutable_type { "!" } else { "" }
    )
}

fn arg(a: &InsnArg) -> String {
    match a {
        InsnArg::Register(r) => reg(r),
        InsnArg::Literal(l) => format!("#{}:{}", l.literal, l.ty),
    }
}

fn ints(v: &[i32]) -> String {
    format!(
        "[{}]",
        v.iter()
            .map(|x| x.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn data_string(d: &FillArrayData) -> String {
    match &d.data {
        ArrayData::Empty => "[]".into(),
        ArrayData::Bytes(v) => format!(
            "[{}]",
            v.iter()
                .map(|b| (*b as i8).to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        ArrayData::Shorts(v) => format!(
            "[{}]",
            v.iter()
                .map(|x| x.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        ArrayData::Ints(v) => ints(v),
        ArrayData::Longs(v) => format!(
            "[{}]",
            v.iter()
                .map(|x| x.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn fill_string(d: &FillArrayData) -> String {
    format!("{}:{}:{}", d.size, d.elem_type, data_string(d))
}

/// One line describing `n` (without trailing newline).
pub fn dump_insn(n: &InsnNode) -> String {
    let mut s = String::new();
    let _ = write!(s, "I {:x} {}", n.offset, n.ty.name());
    let _ = write!(
        s,
        " res={}",
        n.result.as_ref().map(reg).unwrap_or_else(|| "-".into())
    );
    let _ = write!(
        s,
        " args=[{}]",
        n.args.iter().map(arg).collect::<Vec<_>>().join(",")
    );
    match &n.kind {
        InsnKind::None => {}
        InsnKind::Goto { target } => {
            let _ = write!(s, " goto={target}");
        }
        InsnKind::If { op, target } => {
            let _ = write!(s, " if={} target={target}", op.name());
        }
        InsnKind::Switch {
            data_target,
            packed,
            data,
            def,
        } => {
            let _ = write!(s, " switch packed={packed} data={data_target} def={def}");
            match data {
                Some(d) => {
                    let _ = write!(s, " keys={} targets={}", ints(&d.keys), ints(&d.targets));
                }
                None => s.push_str(" nodata"),
            }
        }
        InsnKind::SwitchData(d) => {
            let _ = write!(
                s,
                " swdata keys={} targets={}",
                ints(&d.keys),
                ints(&d.targets)
            );
        }
        InsnKind::ConstStr(v) => {
            let units: Vec<String> = v.iter().map(|u| format!("{u:x}")).collect();
            let _ = write!(s, " str=[{}]", units.join(","));
        }
        InsnKind::ConstClass(t) => {
            let _ = write!(s, " cls={t}");
        }
        InsnKind::Index(IndexValue::Type(t)) => {
            let _ = write!(s, " idx={t}");
        }
        InsnKind::Index(IndexValue::Field(f)) => {
            let _ = write!(s, " idx={f}");
        }
        InsnKind::Invoke { invoke_type, mth } => {
            let _ = write!(s, " invoke={} {mth}", invoke_type.name());
        }
        InsnKind::Arith(op) => {
            let _ = write!(s, " op={}", op.name());
        }
        InsnKind::NewArray(t) => {
            let _ = write!(s, " arr={t}");
        }
        InsnKind::FilledNewArray(t) => {
            let _ = write!(s, " elem={t}");
        }
        InsnKind::FillArray { target, data } => {
            let _ = write!(s, " fill target={target}");
            match data {
                Some(d) => {
                    let _ = write!(s, " data={}", fill_string(d));
                }
                None => s.push_str(" nodata"),
            }
        }
        InsnKind::FillArrayData(d) => {
            let _ = write!(s, " fdata={}", fill_string(d));
        }
    }
    if !n.jumps.is_empty() {
        let j: Vec<String> = n
            .jumps
            .iter()
            .map(|j| format!("{}>{}", j.src, j.dest))
            .collect();
        let _ = write!(s, " jumps=[{}]", j.join(","));
    }
    if n.error.is_some() {
        s.push_str(" ERROR");
    }
    s
}
