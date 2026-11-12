//! `MethodThrowsVisitor`: collect the exceptions a method throws (thrown in the code, thrown by
//! called methods and declared) to generate the `throws` clause.
//!
//! The analysis of a method may visit the called methods, the driver runs the recursion (see
//! `ClassDriver::throws_visit`), this module has the analysis of the code and the checks.

use crate::model::cfg::{bflag, MethodCfg};
use crate::model::insn::{iflag, InsnArg, InsnKind, InsnType};
use crate::model::node::{MethodId, Root};
use crate::model::ssa::ArgRef;
use crate::model::types::{ArgType, CLASS_EXCEPTION, CLASS_THROWABLE};
use crate::type_compare::TypeCompare;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExceptionType {
    ThrowsRequired,
    Runtime,
    UnknownType,
    NoException,
}

/// `validateException`
pub fn validate_exception(root: &Root, ty: &ArgType) -> ExceptionType {
    if *ty == ArgType::object_type() {
        return ExceptionType::NoException;
    }
    let name = ty.object_name();
    let known = ty.is_type_known() && name.as_deref().is_some_and(|n| root.clsp.is_cls_known(n));
    if !known {
        return ExceptionType::UnknownType;
    }
    if is_implements(root, ty, &ArgType::runtime_exception())
        || is_implements(root, ty, &ArgType::object("java.lang.Error"))
    {
        return ExceptionType::Runtime;
    }
    if is_implements(root, ty, &ArgType::throwable())
        || is_implements(root, ty, &ArgType::exception())
    {
        return ExceptionType::ThrowsRequired;
    }
    ExceptionType::NoException
}

fn is_implements(root: &Root, ty: &ArgType, base: &ArgType) -> bool {
    if ty == base {
        return true;
    }
    match (ty.object_name(), base.object_name()) {
        (Some(t), Some(b)) => root.clsp.is_implements(&t, &b),
        _ => false,
    }
}

/// `isBaseException`: is `possible_parent` an exception class of `exception`.
pub fn is_base_exception(root: &Root, exception: &str, possible_parent: &str) -> bool {
    exception == possible_parent || root.clsp.is_implements(exception, possible_parent)
}

/// `mergeExceptions`
pub fn merge_exceptions(root: &Root, set: &mut crate::model::node::ThrowsAttr) {
    if set.contains(CLASS_EXCEPTION) {
        set.retain(|e| e == CLASS_EXCEPTION);
        return;
    }
    if set.contains(CLASS_THROWABLE) {
        set.retain(|e| e == CLASS_THROWABLE);
        return;
    }
    let items: Vec<String> = set.items().to_vec();
    let mut remove = Vec::new();
    for ex1 in &items {
        for ex2 in &items {
            if ex1 != ex2 && is_base_exception(root, ex1, ex2) {
                remove.push(ex1.clone());
            }
        }
    }
    set.retain(|e| !remove.iter().any(|r| r == e));
}

/// An action found in the code of a method.
pub enum Step {
    /// A thrown exception of the type.
    Throw(ArgType),
    /// A call of a method of the input.
    CallApp(MethodId),
    /// A call of a framework method: the declared exceptions.
    CallClsp(Vec<ArgType>),
}

pub struct Planned {
    /// Catch types of the surrounding `try` (exceptions that are caught are not thrown).
    pub excluded: Vec<String>,
    pub step: Step,
}

/// `processInstructions`
pub fn plan(root: &Root, cfg: &MethodCfg) -> Vec<Planned> {
    let mut out = Vec::new();
    'blocks: for &b in &cfg.blocks {
        let block = cfg.block(b);
        // skip e.g. throw instructions of synchronized regions
        let skip = block.flags & (bflag::REMOVE | bflag::DONT_GENERATE) != 0;
        let mut excluded = Vec::new();
        if let Some(handlers) = &block.catch {
            for &h in handlers {
                let handler = &cfg.handlers[h];
                if handler.is_catch_all() {
                    continue 'blocks;
                }
                if let Some(n) = handler.arg_type().object_name() {
                    excluded.push(n);
                }
            }
        }
        for &insn in &block.insns {
            check_insn(root, cfg, insn, &excluded, skip, &mut out);
        }
    }
    out
}

fn check_insn(
    root: &Root,
    cfg: &MethodCfg,
    insn: crate::model::insn::InsnId,
    excluded: &[String],
    skip: bool,
    out: &mut Vec<Planned>,
) {
    let n = cfg.insn(insn);
    if !skip && n.ty == InsnType::Throw && !n.contains_flag(iflag::DONT_GENERATE) {
        match n.args.first() {
            Some(InsnArg::Register(r)) => {
                let ty = cfg.arg_type(ArgRef::Arg(insn, 0));
                if ty == ArgType::throwable() {
                    let assign = r.svar.and_then(|v| cfg.svar_assign_insn(v));
                    if let Some(a) = assign {
                        let ai = cfg.insn(a);
                        if ai.ty == InsnType::MoveException
                            && ai.result.as_ref().is_some_and(|r| r.custom_declare)
                        {
                            // arg variable is from catch statement, ignore Throwable rethrow
                            return;
                        }
                    }
                }
                out.push(Planned {
                    excluded: excluded.to_vec(),
                    step: Step::Throw(ty),
                });
            }
            Some(InsnArg::Wrapped(w)) => out.push(Planned {
                excluded: excluded.to_vec(),
                step: Step::Throw(w.ty.clone()),
            }),
            _ => {}
        }
        return;
    }
    if n.ty != InsnType::Invoke {
        return;
    }
    let InsnKind::Invoke { mth, .. } = &n.kind else {
        return;
    };
    let signature =
        crate::model::info::make_short_id(&mth.name, &mth.arg_types, Some(&mth.return_type));
    match root.class_index(&mth.decl_class.raw_name()) {
        Some(cls) => {
            if let Some(callee) = search_overridden_method(root, cls, mth, &signature) {
                out.push(Planned {
                    excluded: excluded.to_vec(),
                    step: Step::CallApp(callee),
                });
            }
        }
        None => {
            let Some(details) = root.clsp.cls_details(&mth.decl_class.raw_name()) else {
                return;
            };
            // do not check full signature, classpath methods can be trusted
            let found = details
                .methods
                .iter()
                .find(|m| m.short_id().starts_with(&signature));
            if let Some(m) = found {
                if !m.throws.is_empty() {
                    out.push(Planned {
                        excluded: excluded.to_vec(),
                        step: Step::CallClsp(m.throws.clone()),
                    });
                }
            }
        }
    }
}

/// `searchOverriddenMethod`
fn search_overridden_method(
    root: &Root,
    cls: usize,
    mth: &crate::model::info::MethodInfo,
    signature: &str,
) -> Option<MethodId> {
    let short_id = mth.short_id();
    let methods = &root.classes[cls].methods;
    // search by exact full signature (with return value) to fight obfuscation
    if let Some(i) = methods.iter().position(|m| m.info.short_id() == short_id) {
        return Some((cls, i));
    }
    // search by signature without return value and check if return value is wider type
    for (i, sup) in methods.iter().enumerate() {
        if sup.info.short_id().starts_with(signature)
            && sup.access & crate::model::node::ACC_STATIC == 0
        {
            let tc = TypeCompare::new(&root.clsp);
            let res = tc.compare_types(&sup.info.return_type, &mth.return_type);
            if res.is_wider() {
                return Some((cls, i));
            }
        }
    }
    None
}
