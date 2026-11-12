//! `EnumVisitor`: restore enum classes: the constants are found in the class initializer, the
//! compiler generated members are hidden.

use std::collections::HashMap;

use jadx_input_api::ConstValue;

use crate::model::cfg::MethodCfg;
use crate::model::info::{FieldInfo, MethodInfo};
use crate::model::insn::{
    iflag, BlockId, CallType, IndexValue, InsnArg, InsnId, InsnKind, InsnNode, InsnType, InvokeType,
};
use crate::model::node::{
    EnumClassAttr, EnumFieldAttr, MethodId, Root, ACC_CONSTRUCTOR, ACC_ENUM, ACC_FINAL, ACC_STATIC,
    ACC_SYNTHETIC,
};
use crate::model::region::Container;
use crate::model::ssa::ArgRef;
use crate::model::types::{ArgType, CLASS_ENUM};
use crate::name_mapper::is_valid_and_printable;
use crate::passes::driver::MethodState;
use crate::passes::insn_utils::{register_args, replace_insn_by_ref, wrap_instruction_in_place};

type Methods = HashMap<MethodId, MethodState>;

const ENUM_SUPER_CONSTRUCTOR_ID: &str = "java.lang.Enum.<init>(Ljava/lang/String;I)V";
const ACC_ABSTRACT: u32 = 0x400;
const VISIBILITY: u32 = 0x1 | 0x2 | 0x4;

struct EnumField {
    field: usize,
    co: InsnId,
    name: Option<String>,
    cls: Option<usize>,
}

struct Data {
    cls: usize,
    /// index of the class init method
    ci: usize,
    static_blocks: Vec<BlockId>,
    /// instructions to remove: (method index, instruction)
    to_remove: Vec<(usize, InsnId)>,
    values_field: usize,
    values_init: InsnId,
}

/// The code of the class init method (taken out of the method map) and the other methods.
struct Ctx<'a> {
    root: &'a mut Root,
    methods: &'a mut Methods,
    clinit: &'a mut MethodCfg,
}

impl Ctx<'_> {
    fn cfg(&self, cls: usize, ci: usize, m: usize) -> Option<&MethodCfg> {
        if m == ci {
            Some(&*self.clinit)
        } else {
            self.methods.get(&(cls, m)).and_then(|s| s.cfg.as_ref())
        }
    }
}

pub fn enum_visitor(root: &mut Root, methods: &mut Methods, cls: usize) -> Result<(), String> {
    if !root.classes[cls].is_enum() {
        return Ok(());
    }
    let converted = match convert_to_enum(root, methods, cls) {
        Ok(c) => c,
        Err(e) => {
            if std::env::var("JADX_RS_ERRORS").is_ok() {
                eprintln!("ENUM ERROR {}: {e}", root.classes[cls].info.raw_name());
            }
            false
        }
    };
    if !converted && std::env::var("JADX_RS_ERRORS").is_ok() {
        eprintln!("ENUM FAILED {}", root.classes[cls].info.raw_name());
    }
    if !converted && root.classes[cls].access & ACC_ENUM != 0 {
        root.classes[cls].access &= !ACC_ENUM;
        root.classes[cls].comments.push((
            crate::codegen::CommentsLevel::Warn,
            "Failed to restore enum class, 'enum' modifier and super class removed".to_string(),
        ));
    }
    Ok(())
}

fn convert_to_enum(root: &mut Root, methods: &mut Methods, cls: usize) -> Result<bool, String> {
    if root.classes[cls]
        .super_class
        .as_ref()
        .and_then(|s| s.object_name())
        .is_some_and(|n| n == CLASS_ENUM)
    {
        root.classes[cls].remove_super_class = true;
    }
    let Some(ci) = root.classes[cls]
        .methods
        .iter()
        .position(|m| m.info.is_class_init())
    else {
        root.classes[cls].comments.push((
            crate::codegen::CommentsLevel::Warn,
            "Enum class init method not found".to_string(),
        ));
        return Ok(false);
    };
    let Some(mut state) = methods.remove(&(cls, ci)) else {
        return Ok(false);
    };
    let Some(mut clinit) = state.cfg.take() else {
        methods.insert((cls, ci), state);
        return Ok(false);
    };
    let res = {
        let mut ctx = Ctx {
            root,
            methods,
            clinit: &mut clinit,
        };
        convert_with_clinit(&mut ctx, cls, ci)
    };
    state.cfg = Some(clinit);
    methods.insert((cls, ci), state);
    let (ok, foreign) = res?;
    // unbind instructions of other methods
    for (m, insn) in foreign {
        if let Some(cfg) = methods.get_mut(&(cls, m)).and_then(|s| s.cfg.as_mut()) {
            let _ = cfg.unbind_insn(insn);
        }
    }
    Ok(ok)
}

type Foreign = Vec<(usize, InsnId)>;

fn convert_with_clinit(ctx: &mut Ctx, cls: usize, ci: usize) -> Result<(bool, Foreign), String> {
    let Some(region) = ctx.clinit.regions.method_region else {
        return Ok((false, Vec::new()));
    };
    if ctx.clinit.blocks.is_empty() {
        return Ok((false, Vec::new()));
    }
    // collect blocks on linear part of static method (ignore branching on method end)
    let mut static_blocks = Vec::new();
    for sub in ctx.clinit.regions.sub_blocks(region) {
        match sub {
            Container::Block(b) => static_blocks.push(b),
            _ => break,
        }
    }
    if static_blocks.is_empty() {
        return Ok((false, Vec::new()));
    }
    let mut data = Data {
        cls,
        ci,
        static_blocks,
        to_remove: Vec::new(),
        values_field: 0,
        values_init: 0,
    };
    if !search_values_field(ctx, &mut data) {
        return Ok((false, Vec::new()));
    }
    let arr_arg = ctx.clinit.insn(data.values_init).args.first().cloned();
    let mut enum_fields: Option<Vec<EnumField>> = None;
    match arr_arg {
        Some(InsnArg::Wrapped(w)) => {
            enum_fields = extract_enum_fields_from_insn(ctx, &mut data, ci, w.insn)?;
        }
        Some(InsnArg::Register(r)) => {
            // Kotlin 1.9+ $ENTRIES pattern: array register has multiple uses
            if let Some(ArgRef::Result(assign)) = r.svar.map(|v| ctx.clinit.svars[v].assign) {
                enum_fields = extract_enum_fields_from_insn(ctx, &mut data, ci, assign)?;
            }
        }
        _ => {}
    }
    let Some(mut enum_fields) = enum_fields else {
        ctx.root.classes[cls].comments.push((
            crate::codegen::CommentsLevel::Warn,
            "Unknown enum class pattern. Please report as an issue!".to_string(),
        ));
        return Ok((false, Vec::new()));
    };
    data.to_remove.push((ci, data.values_init));

    // all checks complete, perform transform
    ctx.root.classes[cls].enum_class = Some(EnumClassAttr {
        fields: enum_fields
            .iter()
            .map(|f| EnumFieldAttr {
                field: f.field,
                ctr_insn: f.co,
                cls: None,
            })
            .collect(),
        static_method: ci,
    });
    let attr_fields: Vec<(usize, InsnId)> = enum_fields.iter().map(|f| (f.field, f.co)).collect();
    for (idx, (field, co)) in attr_fields.iter().enumerate() {
        let name = enum_fields[idx].name.clone();
        {
            let f = &mut ctx.root.classes[cls].fields[*field];
            let cur = f.alias.clone().unwrap_or_else(|| f.info.name.clone());
            if let Some(name) = &name {
                if &cur != name && is_valid_and_printable(name) {
                    f.alias = Some(name.clone());
                }
            }
            f.dont_generate = true;
        }
        let res = process_constructor_insn(ctx, &mut data, &mut enum_fields[idx], *co);
        if let Some(attr) = ctx.root.classes[cls].enum_class.as_mut() {
            attr.fields[idx].cls = enum_fields[idx].cls;
        }
        res?;
    }
    ctx.root.classes[cls].fields[data.values_field].dont_generate = true;
    // `InsnRemover.removeAllAndUnbind`
    let mut foreign: Foreign = Vec::new();
    let mut local: Vec<InsnId> = Vec::new();
    for &(m, i) in &data.to_remove {
        if m == ci {
            local.push(i);
        } else {
            foreign.push((m, i));
        }
    }
    for &i in &local {
        ctx.clinit.unbind_all_args(i);
    }
    for &i in &local {
        ctx.clinit.unbind_result(i)?;
        ctx.clinit.insns[i].flags |= iflag::DONT_GENERATE;
    }
    for b in ctx.clinit.blocks.clone() {
        ctx.clinit.arena[b].insns.retain(|i| !local.contains(i));
    }
    let insns_count: usize = ctx
        .clinit
        .blocks
        .iter()
        .map(|b| ctx.clinit.block(*b).insns.len())
        .sum();
    if insns_count == 0 {
        ctx.root.classes[cls].methods[ci].dont_generate = true;
    } else if !data.to_remove.is_empty() {
        crate::passes::regions::shrink_method(ctx.clinit)?;
    }
    remove_enum_methods(ctx, cls, ci, data.values_field)?;
    fix_access_flags(ctx.root, cls);
    Ok((true, foreign))
}

fn fix_access_flags(root: &mut Root, cls: usize) {
    // remove invalid access flags
    root.classes[cls].access &= !(ACC_FINAL | ACC_ABSTRACT | ACC_STATIC);
    for m in root.classes[cls].methods.iter_mut() {
        if m.info.is_constructor() {
            m.access &= !VISIBILITY;
        }
    }
}

/// Search "$VALUES" field (holds all enum values)
fn search_values_field(ctx: &mut Ctx, data: &mut Data) -> bool {
    let cls_type = ctx.root.classes[data.cls].info.arg_type().clone();
    let c = &ctx.root.classes[data.cls];
    let mut candidates: Vec<usize> = c
        .fields
        .iter()
        .enumerate()
        .filter(|(_, f)| f.access & ACC_STATIC != 0)
        .filter(|(_, f)| f.ty.is_array())
        .filter(|(_, f)| *f.ty.array_root_element() == cls_type)
        .map(|(i, _)| i)
        .collect();
    if candidates.is_empty() {
        return false;
    }
    if candidates.len() > 1 {
        candidates.retain(|i| c.fields[*i].access & ACC_SYNTHETIC != 0);
    }
    if candidates.len() > 1 {
        if let Some(v) = candidates
            .iter()
            .copied()
            .find(|i| c.fields[*i].info.name == "$VALUES")
        {
            candidates = vec![v];
        }
    }
    if candidates.len() != 1 {
        return false;
    }
    data.values_field = candidates[0];
    // search "$VALUES" array init and collect enum fields
    let search = &c.fields[data.values_field].info;
    for &b in &data.static_blocks {
        for &insn in &ctx.clinit.block(b).insns {
            let n = ctx.clinit.insn(insn);
            if n.ty == InsnType::Sput
                && matches!(&n.kind, InsnKind::Index(IndexValue::Field(f)) if f == search)
            {
                data.values_init = insn;
                return true;
            }
        }
    }
    false
}

fn extract_enum_fields_from_insn(
    ctx: &mut Ctx,
    data: &mut Data,
    src: usize,
    insn: InsnId,
) -> Result<Option<Vec<EnumField>>, String> {
    let ty = ctx.cfg(data.cls, data.ci, src).map(|c| c.insn(insn).ty);
    match ty {
        Some(InsnType::FilledNewArray) => extract_from_filled_array(ctx, data, src, insn),
        Some(InsnType::Invoke) => extract_from_invoke(ctx, data, src, insn),
        Some(InsnType::NewArray) => {
            let cfg = ctx.cfg(data.cls, data.ci, src).expect("cfg");
            // empty enum
            Ok(
                matches!(cfg.insn(insn).args.first(), Some(InsnArg::Literal(l)) if l.literal == 0)
                    .then(Vec::new),
            )
        }
        _ => Ok(None),
    }
}

fn extract_from_invoke(
    ctx: &mut Ctx,
    data: &mut Data,
    src: usize,
    insn: InsnId,
) -> Result<Option<Vec<EnumField>>, String> {
    // handle redirection of values array fill (added in java 15)
    let cfg = ctx.cfg(data.cls, data.ci, src).expect("cfg");
    let InsnKind::Invoke { mth, .. } = &cfg.insn(insn).kind else {
        return Ok(None);
    };
    let Some((vc, vm)) = ctx.root.resolve_method(mth) else {
        return Ok(None);
    };
    if ctx.root.classes[vc].methods[vm].info.return_type.is_void() || vc != data.cls {
        return Ok(None);
    }
    let Some(vcfg) = ctx.cfg(data.cls, data.ci, vm) else {
        return Ok(None);
    };
    let pre_exit = &vcfg.block(vcfg.exit).preds;
    if pre_exit.len() != 1 {
        return Ok(None);
    }
    let Some(&ret) = vcfg.block(pre_exit[0]).insns.last() else {
        return Ok(None);
    };
    let wrapped = match vcfg.insn(ret).args.as_slice() {
        [InsnArg::Wrapped(w)] => w.insn,
        _ => return Ok(None),
    };
    let fields = extract_enum_fields_from_insn(ctx, data, vm, wrapped)?;
    if fields.is_some() && ctx.root.classes[vc].methods[vm].use_in == [(data.cls, data.ci)] {
        let m = &mut ctx.root.classes[vc].methods[vm];
        m.dont_generate = true;
        if m.info.name == "$values" {
            // Kotlin synthetic method used for init values
            m.alias = Some("values".to_string());
        }
    }
    Ok(fields)
}

fn extract_from_filled_array(
    ctx: &mut Ctx,
    data: &mut Data,
    src: usize,
    arr_fill: InsnId,
) -> Result<Option<Vec<EnumField>>, String> {
    let args = ctx
        .cfg(data.cls, data.ci, src)
        .expect("cfg")
        .insn(arr_fill)
        .args
        .clone();
    let mut fields = Vec::new();
    for arg in args {
        let field = match arg {
            InsnArg::Wrapped(w) => process_field_by_wrapped_insn(ctx, data, src, w.insn)?,
            InsnArg::Register(r) => process_field_by_register(ctx, data, src, &r)?,
            _ => None,
        };
        let Some(f) = field else {
            return Ok(None);
        };
        fields.push(f);
    }
    data.to_remove.push((src, arr_fill));
    Ok(Some(fields))
}

fn process_field_by_wrapped_insn(
    ctx: &mut Ctx,
    data: &mut Data,
    src: usize,
    insn: InsnId,
) -> Result<Option<EnumField>, String> {
    let cfg = ctx.cfg(data.cls, data.ci, src).expect("cfg");
    if cfg.insn(insn).ty == InsnType::Sget {
        return process_field_by_field(ctx, data, src, insn);
    }
    if cfg.insn(insn).ty == InsnType::Constructor && src == data.ci {
        let offset = cfg.insn(insn).offset;
        let fi = create_fake_field(ctx.root, data.cls, &format!("EF{offset}"));
        return create_field_by_constructor(ctx, data, fi, insn);
    }
    Ok(None)
}

fn process_field_by_field(
    ctx: &mut Ctx,
    data: &mut Data,
    src: usize,
    sget: InsnId,
) -> Result<Option<EnumField>, String> {
    let cfg = ctx.cfg(data.cls, data.ci, src).expect("cfg");
    let n = cfg.insn(sget);
    let InsnKind::Index(IndexValue::Field(f)) = &n.kind else {
        return Ok(None);
    };
    if n.ty != InsnType::Sget {
        return Ok(None);
    }
    let Some(field) = ctx.root.classes[data.cls]
        .fields
        .iter()
        .position(|x| x.info == *f)
    else {
        return Ok(None);
    };
    let sget_use_count = n
        .result
        .as_ref()
        .and_then(|r| r.svar)
        .map(|v| cfg.svars[v].use_count());
    let Some(sput) = search_field_put_insn(ctx, data, field) else {
        return Ok(None);
    };
    let Some(co) = get_constructor_insn(ctx.clinit, sput) else {
        return Ok(None);
    };
    if sget_use_count.is_none_or(|c| c == 1) {
        data.to_remove.push((src, sget));
    }
    data.to_remove.push((data.ci, sput));
    create_field_by_constructor(ctx, data, field, co)
}

fn process_field_by_register(
    ctx: &mut Ctx,
    data: &mut Data,
    src: usize,
    arg: &crate::model::insn::RegisterArg,
) -> Result<Option<EnumField>, String> {
    let Some(var) = arg.svar else {
        return Ok(None);
    };
    let cfg = ctx.cfg(data.cls, data.ci, src).expect("cfg");
    if let ArgRef::Result(assign) = cfg.svars[var].assign {
        if cfg.insn(assign).ty == InsnType::Sget {
            return process_field_by_field(ctx, data, src, assign);
        }
    }
    if cfg.svars[var].use_count() == 0 || src != data.ci {
        return Ok(None);
    }
    let Some(constr) = cfg.svars[var].assign.parent_insn() else {
        return Ok(None);
    };
    if cfg.insn(constr).ty != InsnType::Constructor {
        return Ok(None);
    }
    // search enum field by the first use
    let field = match cfg.svars[var].uses[0].parent_insn() {
        Some(sput) if cfg.insn(sput).ty == InsnType::Sput => {
            let InsnKind::Index(IndexValue::Field(f)) = &cfg.insn(sput).kind else {
                return Ok(None);
            };
            ctx.root.classes[data.cls]
                .fields
                .iter()
                .position(|x| x.info == *f)
                .inspect(|_| data.to_remove.push((data.ci, sput)))
        }
        _ => None,
    };
    let field = match field {
        Some(f) => f,
        None => create_fake_field(ctx.root, data.cls, &format!("EF{}", arg.reg_num)),
    };
    create_field_by_constructor(ctx, data, field, constr)
}

fn create_fake_field(root: &mut Root, cls: usize, name: &str) -> usize {
    let info = FieldInfo {
        decl_class: root.classes[cls].info.clone(),
        name: name.to_string(),
        ty: root.classes[cls].info.arg_type().clone(),
    };
    let ty = info.ty.clone();
    let mut f = root.classes[cls].fields[0].clone();
    f.info = info;
    f.ty = ty;
    f.access = ACC_SYNTHETIC;
    f.signature = None;
    f.const_value = None;
    f.alias = None;
    f.dont_generate = false;
    f.replace = None;
    f.init_insn = None;
    f.zero_init = false;
    f.use_in = Vec::new();
    f.annotation_refs = Vec::new();
    // `EnumVisitor.createFakeField`: marks the field as not present in the bytecode.
    f.info_comments = vec!["Fake field, exist only in values array".to_string()];
    root.classes[cls].fields.push(f);
    root.classes[cls].fields.len() - 1
}

fn search_field_put_insn(ctx: &Ctx, data: &Data, field: usize) -> Option<InsnId> {
    let target = &ctx.root.classes[data.cls].fields[field];
    for &b in &data.static_blocks {
        for &insn in &ctx.clinit.block(b).insns {
            let n = ctx.clinit.insn(insn);
            if n.ty == InsnType::Sput {
                if let InsnKind::Index(IndexValue::Field(f)) = &n.kind {
                    if *f == target.info {
                        return Some(insn);
                    }
                }
            }
        }
    }
    None
}

fn get_constructor_insn(cfg: &MethodCfg, insn: InsnId) -> Option<InsnId> {
    let n = cfg.insn(insn);
    if n.args.len() != 1 {
        return None;
    }
    let cand = match &n.args[0] {
        InsnArg::Wrapped(w) => Some(w.insn),
        InsnArg::Register(r) => match cfg.svars[r.svar?].assign {
            ArgRef::Result(p) => Some(p),
            _ => None,
        },
        _ => None,
    }?;
    (cfg.insn(cand).ty == InsnType::Constructor).then_some(cand)
}

fn const_string(root: &Root, cfg: &MethodCfg, arg: &InsnArg) -> Option<String> {
    let InsnArg::Wrapped(w) = arg else {
        return None;
    };
    let n = cfg.insn(w.insn);
    match (&n.ty, &n.kind) {
        (InsnType::ConstStr, InsnKind::ConstStr(s)) => Some(String::from_utf16_lossy(s)),
        (InsnType::Sget, InsnKind::Index(IndexValue::Field(f))) => {
            match root.resolve_field(f)?.const_value.as_ref()? {
                ConstValue::Str(s) => Some(String::from_utf16_lossy(s)),
                _ => None,
            }
        }
        _ => None,
    }
}

fn create_field_by_constructor(
    ctx: &mut Ctx,
    data: &mut Data,
    field: usize,
    mut co: InsnId,
) -> Result<Option<EnumField>, String> {
    // usually constructor signature is '<init>(Ljava/lang/String;I)V', sometimes one or both
    // args can be omitted
    let InsnKind::Constructor { call_mth, .. } = ctx.clinit.insn(co).kind.clone() else {
        return Ok(None);
    };
    let Some(constr_cls) = ctx.root.resolve_class(call_mth.decl_class.arg_type()) else {
        return Ok(None);
    };
    if constr_cls != data.cls && ctx.root.classes[constr_cls].anonymous.is_none() {
        return Ok(None);
    }
    let Some((cc, cm)) = ctx.root.resolve_method(&call_mth) else {
        return Ok(None);
    };
    let name;
    if ctx.clinit.insn(co).args.is_empty() {
        let ctr_cfg = ctx.methods.get(&(cc, cm)).and_then(|s| s.cfg.as_ref());
        name = ctr_cfg.and_then(|cfg| {
            let sup = search_enum_super_ctr_insn(cfg)?;
            cfg.insn(sup)
                .args
                .first()
                .and_then(|a| const_string(ctx.root, cfg, a))
        });
    } else {
        name = ctx
            .clinit
            .insn(co)
            .args
            .first()
            .and_then(|a| const_string(ctx.root, ctx.clinit, a));
        // verify and try to inline additional constructor args
        if !register_args(ctx.clinit, co).is_empty() {
            let Some(replaced) = inline_external_regs(ctx, data, co) else {
                return Err(format!(
                    "Init of enum field '{}' uses external variables",
                    ctx.root.classes[data.cls].fields[field].info.name
                ));
            };
            data.to_remove.push((data.ci, co));
            co = replaced;
        }
    }
    Ok(Some(EnumField {
        field,
        co,
        name,
        cls: None,
    }))
}

fn search_enum_super_ctr_insn(cfg: &MethodCfg) -> Option<InsnId> {
    for &b in &cfg.blocks {
        for &i in &cfg.block(b).insns {
            let n = cfg.insn(i);
            if let (
                InsnType::Constructor,
                InsnKind::Constructor {
                    call_mth,
                    call_type,
                },
            ) = (n.ty, &n.kind)
            {
                let id = format!("{}.{}", call_mth.decl_class.raw_name(), call_mth.short_id());
                if *call_type == CallType::Super
                    && !n.args.is_empty()
                    && id == ENUM_SUPER_CONSTRUCTOR_ID
                {
                    return Some(i);
                }
            }
        }
    }
    None
}

fn inline_external_regs(ctx: &mut Ctx, data: &mut Data, co: InsnId) -> Option<InsnId> {
    let res_co = ctx.clinit.copy_without_result(co);
    let regs = register_args(ctx.clinit, res_co);
    for reg in regs {
        let enum_field = check_external_reg_usage(ctx, data, reg)?;
        let mut sget = InsnNode::new(
            InsnType::Sget,
            InsnKind::Index(IndexValue::Field(enum_field)),
        );
        sget.result = None;
        let id = ctx.clinit.add_insn(sget);
        let arg = ctx.clinit.wrap_arg(id).ok()?;
        let ArgRef::Arg(parent, k) = reg else {
            return None;
        };
        if !matches!(
            ctx.clinit.insn(parent).args.get(k),
            Some(InsnArg::Register(_))
        ) {
            return None;
        }
        ctx.clinit.replace_arg(parent, k, arg);
    }
    Some(res_co)
}

fn check_external_reg_usage(ctx: &mut Ctx, data: &mut Data, reg: ArgRef) -> Option<FieldInfo> {
    let cfg = &*ctx.clinit;
    let var = cfg.try_reg(reg)?.svar?;
    let assign = match cfg.svars[var].assign {
        ArgRef::Result(p) if cfg.insn(p).ty == InsnType::Constructor => p,
        _ => return None,
    };
    let cls_info = &ctx.root.classes[data.cls].info;
    let cls_type = cls_info.arg_type();
    match &cfg.insn(assign).kind {
        InsnKind::Constructor { call_mth, .. } if call_mth.decl_class == *cls_info => {}
        _ => return None,
    }
    let mut enum_field: Option<FieldInfo> = None;
    for u in &cfg.svars[var].uses {
        let use_insn = u.parent_insn()?;
        let n = cfg.insn(use_insn);
        match n.ty {
            InsnType::Sput => {
                let InsnKind::Index(IndexValue::Field(f)) = &n.kind else {
                    return None;
                };
                if f.decl_class != *cls_info || f.ty != *cls_type {
                    return None;
                }
                enum_field = Some(f.clone());
            }
            InsnType::Constructor => match &n.kind {
                InsnKind::Constructor { call_mth, .. } if call_mth.decl_class == *cls_info => {}
                _ => return None,
            },
            InsnType::FilledNewArray => {
                // allow usage in values init instruction
                match cfg.insn(data.values_init).args.first() {
                    Some(InsnArg::Wrapped(w)) => {
                        if w.insn != use_insn {
                            return None;
                        }
                    }
                    Some(InsnArg::Register(r)) => {
                        let ArgRef::Result(a) = cfg.svars[r.svar?].assign else {
                            return None;
                        };
                        if a != use_insn {
                            return None;
                        }
                    }
                    _ => return None,
                }
            }
            _ => return None,
        }
    }
    if enum_field.is_some() {
        data.to_remove.push((data.ci, assign));
    }
    enum_field
}

fn process_constructor_insn(
    ctx: &mut Ctx,
    data: &mut Data,
    ef: &mut EnumField,
    co: InsnId,
) -> Result<(), String> {
    let InsnKind::Constructor { call_mth, .. } = ctx.clinit.insn(co).kind.clone() else {
        return Ok(());
    };
    let cls = data.cls;
    if call_mth.decl_class != ctx.root.classes[cls].info {
        if let Some(enum_cls) = ctx.root.resolve_class(call_mth.decl_class.arg_type()) {
            process_enum_cls(ctx.root, cls, ef, enum_cls);
        }
    }
    if let Some((c, m)) = ctx.root.resolve_method(&call_mth) {
        mark_args_for_skip(ctx.root, c, m);
    }
    let res_var = ctx.clinit.insn(co).result.as_ref().and_then(|r| r.svar);
    let uses = res_var.map(|v| ctx.clinit.svars[v].uses.clone());
    match uses {
        Some(uses) if uses.len() > 2 => {
            let found = uses.iter().any(|u| {
                !u.parent_insn()
                    .is_some_and(|p| data.to_remove.contains(&(data.ci, p)))
            });
            if found {
                // constructor result used in other places -> replace constructor with enum
                // field get (SGET)
                let info = ctx.root.classes[cls].fields[ef.field].info.clone();
                let mut get =
                    InsnNode::new(InsnType::Sget, InsnKind::Index(IndexValue::Field(info)));
                get.result = ctx.clinit.insn(co).result.clone();
                let id = ctx.clinit.add_insn(get);
                if let Some(block) = ctx.clinit.block_by_insn(co) {
                    replace_insn_by_ref(ctx.clinit, block, co, id)?;
                }
            }
        }
        _ => data.to_remove.push((data.ci, co)),
    }
    Ok(())
}

fn process_enum_cls(root: &mut Root, cls: usize, ef: &mut EnumField, inner: usize) {
    // remove constructor, because it is anonymous class
    for m in root.classes[inner].methods.iter_mut() {
        if m.access & ACC_CONSTRUCTOR != 0 {
            m.dont_generate = true;
        }
    }
    ef.cls = Some(inner);
    if root.parent_index(inner).unwrap_or(inner) != cls {
        // not inner
        root.classes[cls].inlined_classes.push(inner);
        root.classes[inner].dont_generate = true;
    }
}

fn mark_args_for_skip(root: &mut Root, c: usize, m: usize) {
    // skip first and second args
    let count = root.classes[c].methods[m].info.arg_types.len();
    let skip = root.classes[c].methods[m]
        .skip_args
        .get_or_insert_with(|| vec![false; count]);
    if !skip.is_empty() {
        skip[0] = true;
    }
    if count > 1 {
        skip[1] = true;
    }
}

fn remove_enum_methods(
    ctx: &mut Ctx,
    cls: usize,
    ci: usize,
    values_field: usize,
) -> Result<(), String> {
    let cls_type = ctx.root.classes[cls].info.arg_type().clone();
    let values_short_id = format!(
        "values(){}",
        crate::model::info::type_signature(&ArgType::array(cls_type.clone()))
    );
    let mut values_method: Option<usize> = None;
    // remove compiler generated methods
    for mi in 0..ctx.root.classes[cls].methods.len() {
        let info = ctx.root.classes[cls].methods[mi].info.clone();
        if info.is_class_init() || ctx.cfg(cls, ci, mi).is_none() {
            continue;
        }
        let short_id = info.short_id();
        if info.is_constructor() {
            mark_args_for_skip(ctx.root, cls, mi);
            // remove super constructor call
            let sup = ctx.cfg(cls, ci, mi).and_then(search_enum_super_ctr_insn);
            if let Some(sup) = sup {
                if let Some(cfg) = ctx.methods.get_mut(&(cls, mi)).and_then(|s| s.cfg.as_mut()) {
                    cfg.insns[sup].flags |= iflag::DONT_GENERATE;
                    cfg.remove_insn(sup)?;
                }
            }
            if is_default_constructor(ctx, cls, ci, mi, &short_id) {
                ctx.root.classes[cls].methods[mi].dont_generate = true;
            }
        } else if short_id == values_short_id {
            if is_values_method(ctx, cls, ci, mi, &cls_type) {
                values_method = Some(mi);
                ctx.root.classes[cls].methods[mi].dont_generate = true;
            } else {
                // custom values method => rename to resolve conflict with enum method
                let m = &mut ctx.root.classes[cls].methods[mi];
                m.alias = Some("valuesCustom".to_string());
                m.alias_reason = Some("to resolve conflict with enum method".to_string());
            }
        } else if is_values_method(ctx, cls, ci, mi, &cls_type) {
            let m = &mut ctx.root.classes[cls].methods[mi];
            if m.alias.as_deref().unwrap_or(&m.info.name) != "values" && !m.use_in.is_empty() {
                // rename to use default values method
                m.alias = Some("values".to_string());
                m.alias_reason = Some("to match enum method name".to_string());
                m.dont_rename = true;
            }
            values_method = Some(mi);
            m.dont_generate = true;
        } else if simple_value_of_mth(ctx, cls, ci, mi, &cls_type) {
            ctx.root.classes[cls].methods[mi].dont_generate = true;
        }
    }
    let values_info = ctx.root.classes[cls].fields[values_field].info.clone();
    for mi in 0..ctx.root.classes[cls].methods.len() {
        fix_values_access(ctx, cls, ci, mi, &values_info, &cls_type, values_method)?;
    }
    Ok(())
}

fn is_default_constructor(ctx: &Ctx, cls: usize, ci: usize, mi: usize, short_id: &str) -> bool {
    if short_id == "<init>(Ljava/lang/String;I)V" || short_id == "<init>(Ljava/lang/String;)V" {
        // check content
        return ctx.cfg(cls, ci, mi).is_some_and(|c| {
            c.blocks
                .iter()
                .map(|b| c.block(*b).insns.len())
                .sum::<usize>()
                == 0
        });
    }
    false
}

fn only_one_insn(cfg: &MethodCfg) -> Option<InsnId> {
    let mut found = None;
    for &b in &cfg.blocks {
        let insns = &cfg.block(b).insns;
        match insns.len() {
            0 => {}
            1 if found.is_none() => found = Some(insns[0]),
            _ => return None,
        }
    }
    found
}

fn is_values_method(ctx: &Ctx, cls: usize, ci: usize, mi: usize, cls_type: &ArgType) -> bool {
    let m = &ctx.root.classes[cls].methods[mi];
    if m.return_type.array_element() != Some(cls_type) {
        return false;
    }
    let Some(cfg) = ctx.cfg(cls, ci, mi) else {
        return false;
    };
    let Some(ret) = only_one_insn(cfg) else {
        return false;
    };
    let rn = cfg.insn(ret);
    if rn.ty != InsnType::Return || rn.args.len() != 1 {
        return false;
    }
    let Some(cast) = wrapped_of(cfg, &rn.args[0]) else {
        return false;
    };
    let cn = cfg.insn(cast);
    if cn.ty != InsnType::CheckCast
        || !matches!(&cn.kind, InsnKind::Index(IndexValue::Type(t)) if *t == ArgType::array(cls_type.clone()))
    {
        return false;
    }
    let Some(inv) = cn.args.first().and_then(|a| wrapped_of(cfg, a)) else {
        return false;
    };
    matches!(&cfg.insn(inv).kind, InsnKind::Invoke { mth, .. }
        if mth.name == "clone" && mth.decl_class.full_name() == "java.lang.Object"
            && mth.arg_types.is_empty() && mth.return_type == ArgType::object_type())
}

fn wrapped_of(_cfg: &MethodCfg, arg: &InsnArg) -> Option<InsnId> {
    match arg {
        InsnArg::Wrapped(w) => Some(w.insn),
        _ => None,
    }
}

fn simple_value_of_mth(ctx: &Ctx, cls: usize, ci: usize, mi: usize, cls_type: &ArgType) -> bool {
    let Some(cfg) = ctx.cfg(cls, ci, mi) else {
        return false;
    };
    if cfg.block(cfg.exit).preds.len() != 1 {
        return false;
    }
    let ret = cfg.blocks.iter().find_map(|b| {
        cfg.block(*b).insns.iter().copied().find(|i| {
            let n = cfg.insn(*i);
            n.ty == InsnType::Return && n.args.len() == 1
        })
    });
    let Some(ret) = ret else {
        return false;
    };
    let Some(cast) = wrapped_of(cfg, &cfg.insn(ret).args[0]) else {
        return false;
    };
    let cn = cfg.insn(cast);
    if cn.ty != InsnType::CheckCast
        || !matches!(&cn.kind, InsnKind::Index(IndexValue::Type(t)) if t == cls_type)
    {
        return false;
    }
    let Some(inv) = cn.args.first().and_then(|a| wrapped_of(cfg, a)) else {
        return false;
    };
    matches!(&cfg.insn(inv).kind, InsnKind::Invoke { mth, .. }
        if mth.name == "valueOf" && mth.decl_class.full_name() == "java.lang.Enum"
            && mth.arg_types == [ArgType::class(), ArgType::string()]
            && mth.return_type == ArgType::enum_type())
}

fn fix_values_access(
    ctx: &mut Ctx,
    cls: usize,
    ci: usize,
    mi: usize,
    values_info: &FieldInfo,
    cls_type: &ArgType,
    values_method: Option<usize>,
) -> Result<(), String> {
    let info = ctx.root.classes[cls].methods[mi].info.clone();
    if info.is_constructor() || info.is_class_init() || values_method == Some(mi) {
        return Ok(());
    }
    // search value field usage
    let found = {
        let Some(cfg) = ctx.cfg(cls, ci, mi) else {
            return Ok(());
        };
        collect_sgets(cfg, values_info)
    };
    if found.is_empty() {
        return Ok(());
    }
    let value_mth = match values_method {
        Some(vm) => ctx.root.classes[cls].methods[vm].info.clone(),
        None => MethodInfo {
            decl_class: ctx.root.classes[cls].info.clone(),
            name: "values".to_string(),
            arg_types: Vec::new(),
            return_type: ArgType::array(cls_type.clone()),
        },
    };
    let cfg: &mut MethodCfg = if mi == ci {
        &mut *ctx.clinit
    } else {
        match ctx.methods.get_mut(&(cls, mi)).and_then(|s| s.cfg.as_mut()) {
            Some(c) => c,
            None => return Ok(()),
        }
    };
    for (parent, block, sget) in found {
        let mut inv = InsnNode::new(
            InsnType::Invoke,
            InsnKind::Invoke {
                invoke_type: InvokeType::Static,
                mth: value_mth.clone(),
            },
        );
        if values_method.is_none() {
            // forcing enum method (can overlap and get renamed by custom method)
            inv.flags |= iflag::FORCE_RAW_NAME;
        }
        let id = cfg.add_insn(inv);
        let res = cfg.insn(sget).result.clone();
        cfg.set_result(id, res);
        match parent {
            Some((p, k)) => wrap_instruction_in_place(cfg, p, k, id)?,
            None => {
                if let Some(b) = block {
                    replace_insn_by_ref(cfg, b, sget, id)?;
                }
            }
        }
    }
    Ok(())
}

type SgetPlace = (Option<(InsnId, usize)>, Option<BlockId>, InsnId);

fn collect_sgets(cfg: &MethodCfg, field: &FieldInfo) -> Vec<SgetPlace> {
    fn rec(
        cfg: &MethodCfg,
        field: &FieldInfo,
        insn: InsnId,
        parent: Option<(InsnId, usize)>,
        block: Option<BlockId>,
        out: &mut Vec<SgetPlace>,
    ) {
        let n = cfg.insn(insn);
        if n.ty == InsnType::Sget
            && matches!(&n.kind, InsnKind::Index(IndexValue::Field(f)) if f == field)
        {
            out.push((parent, block, insn));
        }
        for (k, a) in n.args.iter().enumerate() {
            if let InsnArg::Wrapped(w) = a {
                rec(cfg, field, w.insn, Some((insn, k)), None, out);
            }
        }
    }
    let mut out = Vec::new();
    for &b in &cfg.blocks {
        for &i in &cfg.block(b).insns {
            rec(cfg, field, i, None, Some(b), &mut out);
        }
    }
    out
}
