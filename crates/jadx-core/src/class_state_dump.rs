//! Text dump of the class level state after the class passes (oracle comparison format, see
//! `tests/oracle/java/ClassStateDump.java`).

use std::fmt::Write as _;

use crate::model::cfg::MethodCfg;
use crate::model::node::{FieldReplace, Root};

/// State of the class `ci`: access flags, hidden members and replacements.
pub fn dump_class_state<'a>(
    root: &Root,
    ci: usize,
    cfg_of: &dyn Fn(crate::model::node::MethodId) -> Option<&'a MethodCfg>,
) -> String {
    let cls = &root.classes[ci];
    let mut out = format!("M CLASS {}\n", cls.info.raw_name());
    let mut en = String::new();
    if let Some(e) = &cls.enum_class {
        en.push_str(" enum=[");
        for f in &e.fields {
            let name = cfg_of((ci, e.static_method))
                .and_then(|c| match &c.insn(f.ctr_insn).args.first() {
                    Some(crate::model::insn::InsnArg::Wrapped(w)) => match &c.insn(w.insn).kind {
                        crate::model::insn::InsnKind::ConstStr(s) => {
                            Some(String::from_utf16_lossy(s))
                        }
                        _ => None,
                    },
                    _ => None,
                })
                .unwrap_or_else(|| "null".to_string());
            let line = cfg_of((ci, e.static_method))
                .map(|c| crate::region_dump::insn_line(c, f.ctr_insn))
                .unwrap_or_default();
            let _ = write!(
                en,
                "{}|{name}|{}|{line};",
                cls.fields[f.field].info.name,
                f.cls
                    .map_or_else(|| "-".to_string(), |c| root.classes[c].info.raw_name())
            );
        }
        en.push(']');
    }
    let _ = writeln!(
        out,
        "R C acc={:x} dont={} rmsuper={}{en}",
        cls.access,
        u8::from(cls.dont_generate),
        u8::from(cls.remove_super_class)
    );
    let order: Vec<usize> = cls
        .fields_order
        .clone()
        .unwrap_or_else(|| (0..cls.fields.len()).collect());
    for f in order.iter().map(|i| &cls.fields[*i]) {
        let repl = match &f.replace {
            None => "-".to_string(),
            Some(FieldReplace::ClassInstance(c)) => format!("cls:{}", c.raw_name()),
            Some(FieldReplace::Var { ctor, arg }) => {
                format!("var:{}:{arg}", cls.methods[*ctor].info.short_id())
            }
        };
        let init = match &f.init_insn {
            Some(i) => match cfg_of((ci, i.mth)) {
                Some(cfg) => format!(
                    " init={}|{}",
                    cls.methods[i.mth].info.short_id(),
                    crate::region_dump::insn_line(cfg, i.insn)
                ),
                None => String::new(),
            },
            None => String::new(),
        };
        let _ = writeln!(
            out,
            "R F {}:{} acc={:x} dont={} repl={repl}{init}",
            f.info.name,
            f.info.ty,
            f.access,
            u8::from(f.dont_generate)
        );
    }
    for m in &cls.methods {
        let skip: String = m
            .skip_args
            .as_ref()
            .map(|s| {
                s.iter()
                    .enumerate()
                    .filter(|(_, b)| **b)
                    .map(|(i, _)| format!("{i},"))
                    .collect()
            })
            .unwrap_or_default();
        let _ = writeln!(
            out,
            "R M {} acc={:x} dont={} noskip={} skip=[{skip}] skipfirst={} mrepl={}",
            m.info.short_id(),
            m.access,
            u8::from(m.dont_generate),
            u8::from(m.no_skip_args),
            u8::from(m.skip_first_arg),
            match m.replace {
                None => "-".to_string(),
                Some((c, i)) => format!(
                    "{}.{}",
                    root.classes[c].methods[i].info.decl_class.raw_name(),
                    root.classes[c].methods[i].info.short_id()
                ),
            }
        );
    }
    out
}
