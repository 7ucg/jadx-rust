//! `MethodInvokeVisitor`: additional info for method invocations (overload resolving casts,
//! variable arguments calls).

use crate::method_utils::{
    call_method, collect_overloaded_methods, first_arg_offset, has_instance_arg,
    insn_method_details, MethodDetails,
};
use crate::model::cfg::{bflag, MethodCfg};
use crate::model::insn::{iflag, IndexValue, InsnArg, InsnId, InsnKind, InsnNode, InsnType};
use crate::model::ssa::ArgRef;
use crate::model::types::ArgType;
use crate::passes::constructor::MethodEnv;
use crate::type_compare::{TypeCompare, TypeCompareEnum};
use crate::type_utils::{replace_type_variables_using_map, TypeMap, TypeUtils};

const ACC_VARARGS: u32 = 0x80;

/// `IMethodDetails` with possibly replaced type variables (`MutableMethodDetails`).
struct Det {
    /// Identity of the details: kind of the details and method; `None` for a mutable copy
    key: Option<(u8, String, String)>,
    args: Vec<ArgType>,
    var_arg: bool,
}

impl Det {
    fn same(&self, other: &Det) -> bool {
        match (&self.key, &other.key) {
            (Some(a), Some(b)) => a == b,
            _ => false,
        }
    }
}

fn det_key(root: &crate::model::node::Root, d: &MethodDetails) -> (u8, String, String) {
    match d {
        MethodDetails::App(c, m) => {
            let mn = &root.classes[*c].methods[*m];
            (0, mn.info.decl_class.raw_name(), mn.info.short_id())
        }
        MethodDetails::Clsp(m) => (1, m.decl_class.clone(), m.short_id()),
        MethodDetails::Simple(i) => (2, i.decl_class.raw_name(), i.short_id()),
    }
}

fn is_var_arg(root: &crate::model::node::Root, d: &MethodDetails) -> bool {
    match d {
        MethodDetails::App(c, m) => root.classes[*c].methods[*m].access & ACC_VARARGS != 0,
        MethodDetails::Clsp(m) => m.access_flags & ACC_VARARGS != 0,
        MethodDetails::Simple(_) => false,
    }
}

/// `getMethodInfo().getArgumentsTypes()`
fn info_arg_types(root: &crate::model::node::Root, d: &MethodDetails) -> Vec<ArgType> {
    match d {
        MethodDetails::App(c, m) => root.classes[*c].methods[*m].info.arg_types.clone(),
        MethodDetails::Clsp(m) => m.raw_arg_types.clone(),
        MethodDetails::Simple(i) => i.arg_types.clone(),
    }
}

pub fn method_invoke(cfg: &mut MethodCfg, env: &MethodEnv) -> Result<(), String> {
    for block in cfg.blocks.clone() {
        if cfg.block(block).contains(bflag::DONT_GENERATE) {
            continue;
        }
        for insn in cfg.block(block).insns.clone() {
            if cfg.insn(insn).contains_flag(iflag::DONT_GENERATE) {
                continue;
            }
            visit_insns(cfg, env, insn)?;
        }
    }
    Ok(())
}

/// `InsnNode.visitInsns`: the instruction first, then the wrapped instructions of its arguments.
fn visit_insns(cfg: &mut MethodCfg, env: &MethodEnv, insn: InsnId) -> Result<(), String> {
    if matches!(cfg.insn(insn).ty, InsnType::Invoke | InsnType::Constructor) {
        process_invoke(cfg, env, insn)?;
    }
    // arguments may be replaced by the processing
    let mut k = 0;
    while k < cfg.insn(insn).args.len() {
        if let InsnArg::Wrapped(w) = &cfg.insn(insn).args[k] {
            let inner = w.insn;
            visit_insns(cfg, env, inner)?;
        }
        k += 1;
    }
    // `TernaryInsn.visitInsns` also visits the condition
    let cond_insns = match &cfg.insn(insn).kind {
        InsnKind::Ternary { cond } => vec![*cond],
        InsnKind::TernaryCond(c) => c.collect_insns(),
        _ => Vec::new(),
    };
    for c in cond_insns {
        visit_insns(cfg, env, c)?;
    }
    Ok(())
}

fn process_invoke(cfg: &mut MethodCfg, env: &MethodEnv, insn: InsnId) -> Result<(), String> {
    let Some(call_mth) = call_method(cfg.insn(insn)).cloned() else {
        return Ok(());
    };
    if call_mth.args_count() == 0 {
        return Ok(());
    }
    match insn_method_details(env.root, cfg.insn(insn)) {
        None => process_unknown(cfg, insn, &call_mth),
        Some(details) => {
            if is_var_arg(env.root, &details) {
                let args = details.arg_types(env.root);
                if args.last().is_some_and(|l| l.is_array()) {
                    cfg.insns[insn].flags |= iflag::VARARG_CALL;
                }
            }
            process_overloaded(cfg, env, insn, &call_mth, &details)
        }
    }
}

fn first_offset(cfg: &MethodCfg, insn: InsnId) -> usize {
    first_arg_offset(cfg.insn(insn))
}

fn process_overloaded(
    cfg: &mut MethodCfg,
    env: &MethodEnv,
    insn: InsnId,
    call_mth: &crate::model::info::MethodInfo,
    mth_details: &MethodDetails,
) -> Result<(), String> {
    let root = env.root;
    let call_cls = get_call_class_from_invoke(cfg, env, insn, call_mth);
    let overload_methods = match &call_cls {
        Some(c) => collect_overloaded_methods(root, c, call_mth),
        None => Vec::new(),
    };
    if overload_methods.is_empty() {
        // not overloaded
        return Ok(());
    }

    // resolve generic type variables
    let type_vars_mapping = get_type_vars_mapping(cfg, env, insn, call_mth);
    let effective_mth = resolve_type_vars(root, mth_details, &type_vars_mapping)?;
    let mut effective_overloads: Vec<Det> = Vec::with_capacity(overload_methods.len() + 1);
    for m in &overload_methods {
        effective_overloads.push(resolve_type_vars(root, m, &type_vars_mapping)?);
    }
    // the called method itself is the last candidate
    effective_overloads.push(Det {
        key: effective_mth.key.clone(),
        args: effective_mth.args.clone(),
        var_arg: effective_mth.var_arg,
    });

    // search cast types to resolve overloading
    let args_offset = first_offset(cfg, insn);
    let compiler_var_types = collect_compiler_var_types(cfg, insn, args_offset)?;
    let cast_types = search_cast_types(
        env,
        &effective_mth,
        &effective_overloads,
        &compiler_var_types,
    );
    let result_cast_types = expand_types(env, &effective_mth, &cast_types);
    apply_args_cast(
        cfg,
        insn,
        args_offset,
        &compiler_var_types,
        &result_cast_types,
    )
}

/// Method details not found => add cast for 'null' args
fn process_unknown(
    cfg: &mut MethodCfg,
    insn: InsnId,
    call_mth: &crate::model::info::MethodInfo,
) -> Result<(), String> {
    let args_offset = first_offset(cfg, insn);
    let compiler_var_types = collect_compiler_var_types(cfg, insn, args_offset)?;
    let mut cast_types = compiler_var_types.clone();
    if replace_unknown_types(&mut cast_types, &call_mth.arg_types)? {
        apply_args_cast(cfg, insn, args_offset, &compiler_var_types, &cast_types)?;
    }
    Ok(())
}

fn get_call_class_from_invoke(
    cfg: &MethodCfg,
    env: &MethodEnv,
    insn: InsnId,
    call_mth: &crate::model::info::MethodInfo,
) -> Option<ArgType> {
    if let InsnKind::Constructor {
        call_type: crate::model::insn::CallType::Super,
        ..
    } = &cfg.insn(insn).kind
    {
        return env.root.classes[env.cls].super_class.clone();
    }
    if has_instance_arg(cfg.insn(insn)) {
        return Some(cfg.arg_type(ArgRef::Arg(insn, 0)));
    }
    // static call
    Some(call_mth.decl_class.arg_type().clone())
}

fn get_type_vars_mapping(
    cfg: &MethodCfg,
    env: &MethodEnv,
    insn: InsnId,
    call_mth: &crate::model::info::MethodInfo,
) -> TypeMap {
    let decl_cls_type = call_mth.decl_class.arg_type().clone();
    let call_cls_type = get_cls_call_type(cfg, insn, &decl_cls_type);
    let type_utils = TypeUtils::new(env.root);
    let cls_type_vars = type_utils.type_variables_mapping(&call_cls_type);
    let mth_type_vars = type_var_mapping_for_invoke(cfg, env, insn);
    // `Utils.mergeMaps`: the second map overrides the first
    let mut result = cls_type_vars;
    result.extend(mth_type_vars);
    result
}

fn get_cls_call_type(cfg: &MethodCfg, insn: InsnId, decl_cls_type: &ArgType) -> ArgType {
    if has_instance_arg(cfg.insn(insn)) {
        return cfg.arg_type(ArgRef::Arg(insn, 0));
    }
    if cfg.insn(insn).ty == InsnType::Constructor && cfg.insn(insn).result.is_some() {
        return cfg.arg_type(ArgRef::Result(insn));
    }
    decl_cls_type.clone()
}

/// `TypeUtils.getTypeVarMappingForInvoke`
fn type_var_mapping_for_invoke(cfg: &MethodCfg, env: &MethodEnv, insn: InsnId) -> TypeMap {
    let mut map = TypeMap::new();
    let Some(details) = insn_method_details(env.root, cfg.insn(insn)) else {
        return map;
    };
    let offset = first_offset(cfg, insn);
    let ret = details.return_type(env.root);
    if cfg.insn(insn).result.is_some() {
        add_type_var_mapping(&mut map, &ret, Some(cfg.arg_type(ArgRef::Result(insn))));
    }
    let arg_types = details.arg_types(env.root);
    let args_count = cfg.insn(insn).args.len();
    let count = arg_types.len().min(args_count.saturating_sub(offset));
    for (i, t) in arg_types.iter().enumerate().take(count) {
        add_type_var_mapping(
            &mut map,
            t,
            Some(cfg.arg_type(ArgRef::Arg(insn, i + offset))),
        );
    }
    map
}

fn add_type_var_mapping(map: &mut TypeMap, type_var: &ArgType, arg_type: Option<ArgType>) {
    let Some(arg_type) = arg_type else {
        return;
    };
    if !type_var.is_type_known() {
        return;
    }
    if type_var.is_generic_type() {
        map.insert(type_var.clone(), arg_type);
    }
}

fn resolve_type_vars(
    root: &crate::model::node::Root,
    details: &MethodDetails,
    mapping: &TypeMap,
) -> Result<Det, String> {
    let arg_types = details.arg_types(root);
    let mut fixed = false;
    let mut fixed_arg_types = Vec::with_capacity(arg_types.len());
    for (arg_num, arg_type) in arg_types.iter().enumerate() {
        if arg_type.contains_type_variable() {
            let mut resolved = replace_type_variables_using_map(arg_type, mapping);
            if resolved.as_ref().is_none_or(|r| r == arg_type) {
                // type variables erased from method info by compiler
                resolved = info_arg_types(root, details).get(arg_num).cloned();
            }
            fixed_arg_types.push(resolved.ok_or_else(|| "missing erased type".to_string())?);
            fixed = true;
        } else {
            fixed_arg_types.push(arg_type.clone());
        }
    }
    let return_type = details.return_type(root);
    if return_type.contains_type_variable() {
        let resolved = replace_type_variables_using_map(&return_type, mapping);
        if resolved.is_none_or(|r| r.contains_type_variable()) {
            fixed = true;
        }
    }
    Ok(Det {
        key: if fixed {
            None
        } else {
            Some(det_key(root, details))
        },
        args: if fixed { fixed_arg_types } else { arg_types },
        var_arg: is_var_arg(root, details),
    })
}

fn search_cast_types(
    env: &MethodEnv,
    mth_details: &Det,
    overloaded: &[Det],
    compiler_var_types: &[ArgType],
) -> Vec<ArgType> {
    // try compiler types
    if is_overload_resolved(env, mth_details, overloaded, compiler_var_types) {
        return compiler_var_types.to_vec();
    }
    let mut cast_types = compiler_var_types.to_vec();

    // replace unknown types
    let changed = replace_unknown_types(&mut cast_types, &mth_details.args).unwrap_or(false);
    if changed && is_overload_resolved(env, mth_details, overloaded, &cast_types) {
        return cast_types;
    }

    // replace generic types
    let mut changed = false;
    for (cast_type, mth_type) in cast_types.iter_mut().zip(&mth_details.args) {
        if !cast_type.is_generic() && mth_type.is_generic() {
            *cast_type = mth_type.clone();
            changed = true;
        }
    }
    if changed && is_overload_resolved(env, mth_details, overloaded, &cast_types) {
        return cast_types;
    }

    // if just one arg => cast will resolve; not resolved => cast all args instead
    mth_details.args.clone()
}

fn replace_unknown_types(
    cast_types: &mut [ArgType],
    mth_arg_types: &[ArgType],
) -> Result<bool, String> {
    let mut changed = false;
    for (i, ct) in cast_types.iter_mut().enumerate() {
        if !ct.is_type_known() {
            let mth_type = mth_arg_types
                .get(i)
                .ok_or_else(|| "Index out of bounds".to_string())?;
            *ct = mth_type.clone();
            changed = true;
        }
    }
    Ok(changed)
}

/// Use generified types if available
fn expand_types(env: &MethodEnv, details: &Det, cast_types: &[ArgType]) -> Vec<ArgType> {
    let compare = TypeCompare::new(&env.root.clsp);
    let mut list = Vec::with_capacity(cast_types.len());
    for (i, cast_type) in cast_types.iter().enumerate() {
        let Some(mth_type) = details.args.get(i) else {
            list.push(cast_type.clone());
            continue;
        };
        if compare.compare_types(mth_type, cast_type) == TypeCompareEnum::NarrowByGeneric {
            list.push(mth_type.clone());
        } else {
            list.push(cast_type.clone());
        }
    }
    list
}

fn is_overload_resolved(
    env: &MethodEnv,
    expected: &Det,
    overloaded: &[Det],
    cast_types: &[ArgType],
) -> bool {
    if overloaded.is_empty() {
        return false;
    }
    let strict = filter_applicable(env, overloaded, cast_types, |r| r.is_equal());
    if strict.len() == 1 {
        return same_method(strict[0], expected, overloaded);
    }
    let resolved = filter_applicable(env, overloaded, cast_types, |r| {
        r.is_narrow_or_equal() || r == TypeCompareEnum::WiderByGeneric
    });
    if resolved.len() == 1 {
        return same_method(resolved[0], expected, overloaded);
    }
    false
}

/// `strictMethods.get(0).equals(expectedMthDetails)`: the expected details are the last element of
/// the candidates list; a mutable copy is only equal to itself.
fn same_method(candidate: &Det, expected: &Det, overloaded: &[Det]) -> bool {
    let last = overloaded.last().expect("candidates");
    std::ptr::eq(candidate, last) || candidate.same(expected)
}

fn filter_applicable<'a>(
    env: &MethodEnv,
    methods: &'a [Det],
    types: &[ArgType],
    accept: impl Fn(TypeCompareEnum) -> bool,
) -> Vec<&'a Det> {
    methods
        .iter()
        .filter(|m| is_method_acceptable(env, m, types, &accept))
        .collect()
}

fn is_method_acceptable(
    env: &MethodEnv,
    details: &Det,
    types: &[ArgType],
    accept: &impl Fn(TypeCompareEnum) -> bool,
) -> bool {
    if details.args.len() != types.len() {
        return false;
    }
    let compare = TypeCompare::new(&env.root.clsp);
    for (mth_type, arg_type) in details.args.iter().zip(types) {
        if !accept(compare.compare_types(arg_type, mth_type)) {
            return false;
        }
    }
    true
}

fn collect_compiler_var_types(
    cfg: &MethodCfg,
    insn: InsnId,
    arg_offset: usize,
) -> Result<Vec<ArgType>, String> {
    let args_count = cfg.insn(insn).args.len();
    let mut result = Vec::with_capacity(args_count.saturating_sub(arg_offset));
    for i in arg_offset..args_count {
        result.push(get_compiler_var_type(cfg, insn, i));
    }
    Ok(result)
}

/// Return type as seen by compiler
fn get_compiler_var_type(cfg: &MethodCfg, insn: InsnId, index: usize) -> ArgType {
    match &cfg.insn(insn).args[index] {
        InsnArg::Literal(l) => {
            if l.literal == 0 && (l.ty.is_object() || l.ty.is_array()) {
                // null
                return ArgType::unknown_object();
            }
            if l.ty.is_primitive() && !l.explicit_primitive {
                return ArgType::int();
            }
            l.ty.clone()
        }
        InsnArg::Register(_) => cfg.arg_type(ArgRef::Arg(insn, index)),
        InsnArg::Wrapped(w) => get_insn_compiler_type(cfg, w.insn, &w.ty),
        InsnArg::Named(n) => n.ty.clone(),
    }
}

fn get_insn_compiler_type(cfg: &MethodCfg, insn: InsnId, arg_type: &ArgType) -> ArgType {
    let n = cfg.insn(insn);
    match n.ty {
        InsnType::Cast | InsnType::CheckCast => {
            if let InsnKind::Index(IndexValue::Type(t)) = &n.kind {
                return t.clone();
            }
            arg_type.clone()
        }
        _ => {
            if n.result.is_some() {
                cfg.arg_type(ArgRef::Result(insn))
            } else {
                arg_type.clone()
            }
        }
    }
}

fn apply_args_cast(
    cfg: &mut MethodCfg,
    insn: InsnId,
    args_offset: usize,
    compiler_var_types: &[ArgType],
    cast_types: &[ArgType],
) -> Result<(), String> {
    let args_count = cfg.insn(insn).args.len();
    for i in args_offset..args_count {
        let orig_pos = i - args_offset;
        let compiler_type = compiler_var_types
            .get(orig_pos)
            .ok_or_else(|| "Index out of bounds".to_string())?;
        let cast_type = cast_types
            .get(orig_pos)
            .ok_or_else(|| "Index out of bounds".to_string())?;
        if cast_type != compiler_type {
            let is_literal = matches!(cfg.insn(insn).args[i], InsnArg::Literal(_));
            if is_literal && compiler_type.is_primitive() && cast_type.is_primitive() {
                if let InsnArg::Literal(l) = &mut cfg.insns[insn].args[i] {
                    l.ty = cast_type.clone();
                    l.explicit_primitive = true;
                }
                continue;
            }
            let wrapped_cast = match &cfg.insn(insn).args[i] {
                InsnArg::Wrapped(w) if cfg.insn(w.insn).ty == InsnType::CheckCast => Some(w.insn),
                _ => None,
            };
            if let Some(wi) = wrapped_cast {
                cfg.insns[wi].kind = InsnKind::Index(IndexValue::Type(cast_type.clone()));
                continue;
            }
            // the argument moves into a new cast instruction
            let arg = cfg.insn(insn).args[i].clone();
            if let InsnArg::Register(r) = &arg {
                if let Some(v) = r.svar {
                    cfg.svar_remove_use(v, ArgRef::Arg(insn, i));
                }
            }
            let mut cast = InsnNode::new(
                InsnType::Cast,
                InsnKind::Index(IndexValue::Type(cast_type.clone())),
            );
            cast.flags |= iflag::EXPLICIT_CAST;
            let cast_id = cfg.add_insn(cast);
            crate::passes::insn_utils::add_arg(cfg, cast_id, arg);
            let wrapped = cfg.wrap_arg(cast_id)?;
            let wrapped = match wrapped {
                InsnArg::Wrapped(mut w) => {
                    w.ty = cast_type.clone();
                    InsnArg::Wrapped(w)
                }
                other => other,
            };
            cfg.insns[insn].args[i] = wrapped;
        } else {
            // protect already existed cast
            if let InsnArg::Wrapped(w) = &cfg.insn(insn).args[i] {
                let wi = w.insn;
                if cfg.insn(wi).ty == InsnType::CheckCast {
                    cfg.insns[wi].flags |= iflag::EXPLICIT_CAST;
                }
            }
        }
    }
    Ok(())
}
