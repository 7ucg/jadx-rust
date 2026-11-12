//! `TypeUpdate`: propagate a candidate type of an SSA variable through the instructions using it.
//!
//! The reference uses callback objects chained through request queues. Here the callbacks live in
//! an arena inside [`UpdateInfo`] and are addressed by id; their state is only touched through
//! short borrows so that a callback may be re-entered while it runs (as the reference does).

use std::collections::HashMap;

use crate::method_utils::{first_arg_offset, has_instance_arg, insn_method_details};
use crate::model::cfg::MethodCfg;
use crate::model::insn::{iflag, ArithOp, IndexValue, InsnKind, InsnType};
use crate::model::node::Root;
use crate::model::ssa::{ArgRef, SsaId, TypeBound};
use crate::model::types::{ArgType, PrimitiveType};
use crate::type_compare::{TypeCompare, TypeCompareEnum};
use crate::type_utils::{replace_type_variables_using_map, TypeMap, TypeUtils};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateResult {
    Reject,
    Same,
    Changed,
}
use UpdateResult::*;

/// `null` result of the reference: the update is waiting in the queue.
pub type Res = Result<Option<UpdateResult>, String>;

#[derive(Debug, Clone, Copy, Default)]
pub struct Flags {
    pub allow_wider: bool,
    pub ignore_same: bool,
    pub ignore_unknown: bool,
    pub keep_generics: bool,
}

impl Flags {
    pub const EMPTY: Flags = Flags {
        allow_wider: false,
        ignore_same: false,
        ignore_unknown: false,
        keep_generics: false,
    };
    pub const WIDER: Flags = Flags {
        allow_wider: true,
        ..Flags::EMPTY
    };
    pub const WIDER_IGNORE_SAME: Flags = Flags {
        allow_wider: true,
        ignore_same: true,
        ..Flags::EMPTY
    };
    pub const APPLY_DEBUG: Flags = Flags {
        allow_wider: true,
        keep_generics: true,
        ignore_unknown: true,
        ignore_same: false,
    };
}

type CbId = usize;

#[derive(Clone)]
struct Request {
    arg: ArgRef,
    candidate: ArgType,
    direct: bool,
    cb: Option<CbId>,
}

#[derive(Clone)]
struct Entry {
    seq: usize,
    ty: ArgType,
}

struct ArgsListCb {
    args: Vec<ArgRef>,
    next: usize,
    candidate: ArgType,
    direct: bool,
    /// `argsFilter`: skip this argument.
    skip: Option<ArgRef>,
    /// `finalResultCallback`: roll back the variable on reject.
    final_ssa: Option<SsaId>,
    ignore_reject: bool,
    all_same: bool,
    first_queue: bool,
}

struct InvokeCb {
    invoke: usize,
    args_count: usize,
    known_type_vars: Vec<ArgType>,
    return_type: ArgType,
    arg_types: Vec<ArgType>,
    type_vars_map: TypeMap,
    candidate: ArgType,
    is_assign: bool,
    all_same: bool,
    current_arg: i64,
    update_arg: Option<ArgRef>,
    update_type: Option<ArgType>,
    first_queue: bool,
}

enum Cb {
    ArgsList(ArgsListCb),
    Invoke(InvokeCb),
    Move {
        change_arg: ArgRef,
        candidate: ArgType,
        correct_type: bool,
    },
    CheckCastAssign,
    ArrayGetAssign {
        arr_arg: ArgRef,
        candidate: ArgType,
    },
    ArrayGetArr {
        result: ArgRef,
        element: ArgType,
    },
    ArrayPut {
        put_arg: ArgRef,
        element: ArgType,
    },
    If {
        update_arg: ArgRef,
        candidate: ArgType,
    },
}

/// `TypeUpdateInfo`
pub struct UpdateInfo {
    flags: Flags,
    update_map: HashMap<ArgRef, Entry>,
    queue: Vec<Request>,
    callback_queue: Vec<Request>,
    limit: usize,
    seq: usize,
    cbs: Vec<Cb>,
}

impl UpdateInfo {
    fn new(flags: Flags, limit: usize) -> Self {
        UpdateInfo {
            flags,
            update_map: HashMap::new(),
            queue: Vec::new(),
            callback_queue: Vec::new(),
            limit,
            seq: 0,
            cbs: Vec::new(),
        }
    }

    fn add_cb(&mut self, cb: Cb) -> CbId {
        self.cbs.push(cb);
        self.cbs.len() - 1
    }

    fn queue_request(&mut self, r: Request) {
        self.queue.push(r);
    }

    fn save_callback(&mut self, r: Request) {
        if r.cb.is_some() {
            self.callback_queue.push(r);
        }
    }

    fn request_update(&mut self, arg: ArgRef, ty: ArgType) -> Result<(), String> {
        let seq = self.seq;
        self.seq += 1;
        if self.update_map.insert(arg, Entry { seq, ty }).is_some() {
            return Err("Unexpected type update override for arg".into());
        }
        if self.seq > self.limit {
            return Err(format!(
                "Type inference error: updates count limit reached with updateSeq = {}",
                self.seq
            ));
        }
        Ok(())
    }

    fn rollback_update(&mut self, arg: ArgRef) {
        if let Some(removed) = self.update_map.remove(&arg) {
            self.update_map.retain(|_, e| e.seq <= removed.seq);
        }
    }

    fn is_processed(&self, arg: ArgRef) -> bool {
        self.update_map.contains_key(&arg)
    }

    fn has_update_with_type(&self, arg: ArgRef, ty: &ArgType) -> bool {
        self.update_map.get(&arg).is_some_and(|e| e.ty == *ty)
    }

    /// `getType(arg)`: pending update type or the current type.
    pub fn get_type(&self, cfg: &MethodCfg, arg: ArgRef) -> ArgType {
        match self.update_map.get(&arg) {
            Some(e) => e.ty.clone(),
            None => cfg.arg_type(arg),
        }
    }
}

pub struct TypeUpdate<'a> {
    pub root: &'a Root,
    pub cls: usize,
    pub mth: usize,
    pub compare: TypeCompare<'a>,
    /// Instruction units of the method (update limit base).
    pub insns_count: usize,
}

const TYPE_UPDATES_LIMIT: usize = 10;

impl<'a> TypeUpdate<'a> {
    pub fn new(root: &'a Root, cls: usize, mth: usize, insns_count: usize) -> Self {
        TypeUpdate {
            root,
            cls,
            mth,
            compare: TypeCompare::new(&root.clsp),
            insns_count,
        }
    }

    fn tu(&self) -> TypeUtils<'a> {
        TypeUtils::new(self.root)
    }

    pub fn apply(
        &self,
        cfg: &mut MethodCfg,
        var: SsaId,
        candidate: &ArgType,
    ) -> Result<UpdateResult, String> {
        self.apply_flags(cfg, var, candidate, Flags::EMPTY)
    }

    pub fn apply_with_wider_ign_same(
        &self,
        cfg: &mut MethodCfg,
        var: SsaId,
        candidate: &ArgType,
    ) -> Result<UpdateResult, String> {
        self.apply_flags(cfg, var, candidate, Flags::WIDER_IGNORE_SAME)
    }

    pub fn apply_with_wider_allow(
        &self,
        cfg: &mut MethodCfg,
        var: SsaId,
        candidate: &ArgType,
    ) -> Result<UpdateResult, String> {
        self.apply_flags(cfg, var, candidate, Flags::WIDER)
    }

    pub fn apply_debug_info(
        &self,
        cfg: &mut MethodCfg,
        var: SsaId,
        candidate: &ArgType,
    ) -> Result<UpdateResult, String> {
        self.apply_flags(cfg, var, candidate, Flags::APPLY_DEBUG)
    }

    fn apply_flags(
        &self,
        cfg: &mut MethodCfg,
        var: SsaId,
        candidate: &ArgType,
        flags: Flags,
    ) -> Result<UpdateResult, String> {
        if !candidate.is_type_known() {
            return Ok(Reject);
        }
        let mut info = UpdateInfo::new(flags, self.insns_count * TYPE_UPDATES_LIMIT);
        let assign = cfg.svars[var].assign;
        let mut result = self.queue_type_update(cfg, &mut info, assign, candidate.clone(), None)?;
        if result.is_none() {
            result = self.run_update(cfg, &mut info)?;
        }
        if result == Some(Reject) {
            return Ok(Reject);
        }
        if info.update_map.is_empty() {
            return Ok(Same);
        }
        let mut updates: Vec<(usize, ArgRef, ArgType)> = info
            .update_map
            .iter()
            .map(|(a, e)| (e.seq, *a, e.ty.clone()))
            .collect();
        updates.sort_by_key(|u| u.0);
        for (_, a, t) in updates {
            cfg.set_arg_type(a, t)?;
        }
        Ok(Changed)
    }

    fn run_update(&self, cfg: &mut MethodCfg, info: &mut UpdateInfo) -> Res {
        let mut result = Some(Reject);
        loop {
            let Some(request) = info.queue.pop() else {
                return Ok(result);
            };
            let new_result = if request.direct {
                self.request_update(cfg, info, request.arg, &request.candidate)?
            } else {
                self.update_type_for_arg(cfg, info, request.arg, &request.candidate)?
            };
            info.save_callback(request);
            if let Some(nr) = new_result {
                result = self.process_callbacks(cfg, info, nr)?;
            }
        }
    }

    fn process_callbacks(
        &self,
        cfg: &mut MethodCfg,
        info: &mut UpdateInfo,
        result: UpdateResult,
    ) -> Res {
        let mut current = result;
        loop {
            let Some(cb_req) = info.callback_queue.pop() else {
                return Ok(Some(current));
            };
            let cb = cb_req.cb.expect("saved callback request has a callback");
            match self.run_callback(cfg, info, cb, current)? {
                None => {
                    info.save_callback(cb_req);
                    return Ok(None);
                }
                Some(r) => {
                    current = r;
                    if current == Reject {
                        info.rollback_update(cb_req.arg);
                    }
                }
            }
        }
    }

    pub fn queue_type_update(
        &self,
        cfg: &mut MethodCfg,
        info: &mut UpdateInfo,
        arg: ArgRef,
        candidate: ArgType,
        cb: Option<CbId>,
    ) -> Res {
        let res = self.verify_type(cfg, info, arg, &candidate)?;
        if let Some(res) = res {
            let Some(cb) = cb else {
                return Ok(Some(res));
            };
            let result = self.run_callback(cfg, info, cb, res)?;
            if result.is_none() {
                info.save_callback(Request {
                    arg,
                    candidate,
                    direct: false,
                    cb: Some(cb),
                });
            }
            return Ok(result);
        }
        info.queue_request(Request {
            arg,
            candidate,
            direct: false,
            cb,
        });
        Ok(None)
    }

    fn queue_direct_type_update(
        &self,
        info: &mut UpdateInfo,
        arg: ArgRef,
        candidate: ArgType,
        cb: Option<CbId>,
    ) -> Res {
        info.queue_request(Request {
            arg,
            candidate,
            direct: true,
            cb,
        });
        Ok(None)
    }

    fn update_type_for_arg(
        &self,
        cfg: &mut MethodCfg,
        info: &mut UpdateInfo,
        arg: ArgRef,
        candidate: &ArgType,
    ) -> Res {
        if is_register_arg(cfg, arg) {
            let Some(var) = cfg.reg(arg).svar else {
                return Err("register without SSA variable in type update".into());
            };
            return self.update_type_for_ssa_var(cfg, info, var, candidate);
        }
        self.request_update(cfg, info, arg, candidate)
    }

    fn verify_type(
        &self,
        cfg: &MethodCfg,
        info: &UpdateInfo,
        arg: ArgRef,
        candidate: &ArgType,
    ) -> Result<Option<UpdateResult>, String> {
        if info.is_processed(arg) {
            return Ok(Some(Changed));
        }
        let current = cfg.arg_type(arg);
        let flags = info.flags;
        if current == *candidate {
            if !flags.ignore_same {
                return Ok(Some(Same));
            }
        } else {
            if candidate.is_wildcard() {
                return Ok(Some(Reject));
            }
            let cmp = self.compare.compare_types(candidate, &current);
            if cmp.is_conflict() {
                return Ok(Some(Reject));
            }
            if cmp == TypeCompareEnum::Unknown && flags.ignore_unknown {
                return Ok(Some(Reject));
            }
            if cfg.arg_type_immutable(arg) && current != ArgType::unknown_any() {
                if cmp == TypeCompareEnum::Equal {
                    return Ok(Some(Same));
                }
                return Ok(Some(Reject));
            }
            if cmp == TypeCompareEnum::WiderByGeneric && flags.keep_generics {
                return Ok(Some(Reject));
            }
            if cmp.is_wider() && !flags.allow_wider {
                return Ok(Some(Reject));
            }
            if candidate.contains_type_variable()
                && self
                    .tu()
                    .check_for_unknown_type_vars(self.cls, self.mth, candidate)
                    .is_some()
            {
                return Ok(Some(Reject));
            }
        }
        Ok(None)
    }

    fn update_type_for_ssa_var(
        &self,
        cfg: &mut MethodCfg,
        info: &mut UpdateInfo,
        var: SsaId,
        candidate: &ArgType,
    ) -> Res {
        if let Some(im) = cfg.svar_immutable_type(var) {
            if im != *candidate {
                return Ok(Some(Reject));
            }
        }
        let bounds = cfg.svars[var].bounds.clone();
        if !self.in_bounds(cfg, Some(info), &bounds, candidate) {
            return Ok(Some(Reject));
        }
        let cb = info.add_cb(Cb::ArgsList(ArgsListCb {
            args: cfg.svars[var].uses.clone(),
            next: 0,
            candidate: candidate.clone(),
            direct: true,
            skip: None,
            final_ssa: Some(var),
            ignore_reject: false,
            all_same: true,
            first_queue: false,
        }));
        let assign = cfg.svars[var].assign;
        self.queue_direct_type_update(info, assign, candidate.clone(), Some(cb))
    }

    fn request_update(
        &self,
        cfg: &mut MethodCfg,
        info: &mut UpdateInfo,
        arg: ArgRef,
        candidate: &ArgType,
    ) -> Res {
        if info.is_processed(arg) {
            return Ok(Some(Changed));
        }
        info.request_update(arg, candidate.clone())?;
        let Some(insn) = arg.parent_insn() else {
            return Ok(Some(Same));
        };
        match cfg.insn(insn).ty {
            InsnType::Const => self.same_first_arg_listener(cfg, info, insn, arg, candidate),
            InsnType::Move => self.move_listener(cfg, info, insn, arg, candidate),
            InsnType::Phi => self.all_same_listener(cfg, info, insn, arg, candidate),
            InsnType::Aget => self.array_get_listener(cfg, info, insn, arg, candidate),
            InsnType::Aput => self.array_put_listener(cfg, info, insn, arg, candidate),
            InsnType::If => self.if_listener(cfg, info, insn, arg, candidate),
            InsnType::Arith => self.arith_listener(cfg, info, insn, arg, candidate),
            InsnType::Neg | InsnType::Not => {
                self.suggest_all_same_listener(cfg, info, insn, arg, candidate)
            }
            InsnType::CheckCast => self.check_cast_listener(cfg, info, insn, arg, candidate),
            InsnType::Invoke | InsnType::Constructor => {
                self.invoke_listener(cfg, info, insn, arg, candidate)
            }
            _ => Ok(Some(Changed)),
        }
    }

    /// `ITypeBound.getType` (with `updateInfo` for dynamic bounds).
    pub fn bound_type(
        &self,
        cfg: &MethodCfg,
        info: Option<&UpdateInfo>,
        bound: &TypeBound,
    ) -> Option<ArgType> {
        let ty = |a: ArgRef| match info {
            Some(i) => i.get_type(cfg, a),
            None => cfg.arg_type(a),
        };
        match bound {
            TypeBound::Const { ty, .. } => Some(ty.clone()),
            TypeBound::CheckCastAssign { insn } => {
                let arg_type = ty(ArgRef::Arg(*insn, 0));
                let cast_type = match &cfg.insn(*insn).kind {
                    InsnKind::Index(IndexValue::Type(t)) => t.clone(),
                    _ => return None,
                };
                let r = self.compare.compare_types(&arg_type, &cast_type);
                Some(if r.is_narrow() { arg_type } else { cast_type })
            }
            TypeBound::FieldGetAssign { insn, init_type } => {
                let instance = ty(ArgRef::Arg(*insn, 0));
                match self.tu().replace_class_generics(&instance, init_type) {
                    Some(r) if !r.is_wildcard() => Some(r),
                    _ => Some(init_type.clone()),
                }
            }
            TypeBound::InvokeAssign {
                insn,
                generic_return,
            } => {
                let instance = ty(ArgRef::Arg(*insn, 0));
                let decl = match insn_method_details(self.root, cfg.insn(*insn)) {
                    Some(d) => d.decl_class_type(self.root),
                    None => instance.clone(),
                };
                let generic =
                    self.tu()
                        .replace_class_generics_from(&instance, &decl, generic_return);
                let result = match generic {
                    None => None,
                    Some(r) if !r.is_wildcard() => Some(r),
                    Some(r) => r.wildcard_type().cloned(),
                };
                match result {
                    Some(r) => Some(r),
                    None => crate::method_utils::call_method(cfg.insn(*insn))
                        .map(|m| m.return_type.clone()),
                }
            }
            TypeBound::InvokeUse {
                insn,
                arg,
                generic_arg,
            } => {
                let instance = ty(ArgRef::Arg(*insn, 0));
                let arg_type = ty(*arg);
                match self.tu().replace_class_generics(&instance, generic_arg) {
                    Some(r) => Some(r),
                    None => Some(arg_type),
                }
            }
        }
    }

    pub fn in_bounds(
        &self,
        cfg: &MethodCfg,
        info: Option<&UpdateInfo>,
        bounds: &[TypeBound],
        candidate: &ArgType,
    ) -> bool {
        for b in bounds {
            let bound_type = if b.is_dynamic() {
                self.bound_type(cfg, info, b)
            } else {
                self.bound_type(cfg, None, b)
            };
            if let Some(bt) = bound_type {
                if !self.check_bound(candidate, b, &bt) {
                    return false;
                }
            }
        }
        true
    }

    fn check_bound(&self, candidate: &ArgType, bound: &TypeBound, bound_type: &ArgType) -> bool {
        use crate::model::ssa::BoundKind;
        match self.compare.compare_types(candidate, bound_type) {
            TypeCompareEnum::Equal => true,
            TypeCompareEnum::Wider => bound.kind() != BoundKind::Use,
            TypeCompareEnum::Narrow => {
                if bound.kind() == BoundKind::Assign {
                    return !bound_type.is_type_known()
                        && check_assign_for_unknown(bound_type, candidate);
                }
                true
            }
            TypeCompareEnum::WiderByGeneric | TypeCompareEnum::NarrowByGeneric => true,
            TypeCompareEnum::Conflict | TypeCompareEnum::ConflictByGeneric => false,
            TypeCompareEnum::Unknown => true,
        }
    }

    // ---- callbacks ---------------------------------------------------------------------

    fn run_callback(
        &self,
        cfg: &mut MethodCfg,
        info: &mut UpdateInfo,
        id: CbId,
        result: UpdateResult,
    ) -> Res {
        enum Kind {
            Args,
            Invoke,
            Move(ArgRef, ArgType, bool),
            CheckCast,
            ArrayGetAssign(ArgRef, ArgType),
            ArrayGetArr(ArgRef, ArgType),
            ArrayPut(ArgRef, ArgType),
            If(ArgRef, ArgType),
        }
        let kind = match &info.cbs[id] {
            Cb::ArgsList(_) => Kind::Args,
            Cb::Invoke(_) => Kind::Invoke,
            Cb::Move {
                change_arg,
                candidate,
                correct_type,
            } => Kind::Move(*change_arg, candidate.clone(), *correct_type),
            Cb::CheckCastAssign => Kind::CheckCast,
            Cb::ArrayGetAssign { arr_arg, candidate } => {
                Kind::ArrayGetAssign(*arr_arg, candidate.clone())
            }
            Cb::ArrayGetArr { result, element } => Kind::ArrayGetArr(*result, element.clone()),
            Cb::ArrayPut { put_arg, element } => Kind::ArrayPut(*put_arg, element.clone()),
            Cb::If {
                update_arg,
                candidate,
            } => Kind::If(*update_arg, candidate.clone()),
        };
        match kind {
            Kind::Args => self.args_list_callback(cfg, info, id, result),
            Kind::Invoke => self.invoke_callback(cfg, info, id, result),
            Kind::Move(change_arg, candidate, correct) => {
                let _ = (change_arg, candidate);
                if result == Same && !correct {
                    return Ok(Some(Reject));
                }
                if result == Reject && correct {
                    return Ok(Some(Changed));
                }
                Ok(Some(result))
            }
            Kind::CheckCast => Ok(Some(if result == Reject { Same } else { result })),
            Kind::ArrayGetAssign(arr_arg, candidate) => {
                if result == Reject {
                    let arr_type = cfg.arg_type(arr_arg);
                    if arr_type.is_type_known()
                        && arr_type.is_array()
                        && arr_type.array_element().is_some_and(|e| e.is_primitive())
                    {
                        let el = arr_type
                            .array_element()
                            .cloned()
                            .unwrap_or_else(ArgType::object_type);
                        if self.compare.compare_types(&candidate, &el) == TypeCompareEnum::Wider {
                            return Ok(Some(Changed));
                        }
                    }
                }
                Ok(Some(result))
            }
            Kind::ArrayGetArr(res_arg, element) => {
                if result == Reject {
                    let res_type = cfg.arg_type(res_arg);
                    if res_type.is_type_known() && res_type.is_primitive() {
                        let c = self.compare.compare_types(&res_type, &element);
                        if c == TypeCompareEnum::Wider {
                            return Ok(Some(Changed));
                        }
                    }
                }
                Ok(Some(result))
            }
            Kind::ArrayPut(put_arg, element) => {
                if result == Reject {
                    let put_type = cfg.arg_type(put_arg);
                    if put_type.is_type_known() {
                        let c = self.compare.compare_types(&element, &put_type);
                        if c == TypeCompareEnum::Wider || c == TypeCompareEnum::WiderByGeneric {
                            return Ok(Some(Changed));
                        }
                    }
                }
                Ok(Some(result))
            }
            Kind::If(update_arg, candidate) => {
                if result == Reject {
                    let t = cfg.arg_type(update_arg);
                    if candidate.is_object() && t.can_be_object() {
                        return Ok(Some(Same));
                    }
                    if candidate.is_array() && t.can_be_array() {
                        return Ok(Some(Same));
                    }
                    if candidate.is_primitive() {
                        if let Some(p) = candidate.primitive_type() {
                            if t.can_be_primitive(p) {
                                return Ok(Some(Same));
                            }
                        }
                        if t.is_type_known() && candidate.reg_count() == t.reg_count() {
                            return Ok(Some(Same));
                        }
                    }
                }
                Ok(Some(result))
            }
        }
    }

    // ---- ArgsListUpdateCallback ---------------------------------------------------------

    fn al(info: &mut UpdateInfo, id: CbId) -> &mut ArgsListCb {
        match &mut info.cbs[id] {
            Cb::ArgsList(a) => a,
            _ => panic!("not an args list callback"),
        }
    }

    fn args_list_final(
        &self,
        cfg: &MethodCfg,
        info: &mut UpdateInfo,
        id: CbId,
        result: UpdateResult,
    ) -> Res {
        if let Some(var) = Self::al(info, id).final_ssa {
            if result == Reject {
                info.rollback_update(cfg.svars[var].assign);
                for u in cfg.svars[var].uses.clone() {
                    info.rollback_update(u);
                }
            }
        }
        Ok(Some(result))
    }

    fn args_list_callback(
        &self,
        cfg: &mut MethodCfg,
        info: &mut UpdateInfo,
        id: CbId,
        mut result: UpdateResult,
    ) -> Res {
        loop {
            if !Self::al(info, id).ignore_reject && result == Reject {
                return self.args_list_final(cfg, info, id, result);
            }
            if result != Same {
                Self::al(info, id).all_same = false;
            }
            let Some(next) = self.args_list_next(info, id) else {
                let all_same = Self::al(info, id).all_same;
                return self.args_list_final(cfg, info, id, if all_same { Same } else { Changed });
            };
            match self.args_list_queue(cfg, info, id, next)? {
                None => return Ok(None),
                Some(r) => result = r,
            }
        }
    }

    fn args_list_next(&self, info: &mut UpdateInfo, id: CbId) -> Option<ArgRef> {
        let a = Self::al(info, id);
        while a.next < a.args.len() {
            let n = a.args[a.next];
            a.next += 1;
            if a.skip != Some(n) {
                return Some(n);
            }
        }
        None
    }

    fn args_list_queue(
        &self,
        cfg: &mut MethodCfg,
        info: &mut UpdateInfo,
        id: CbId,
        next: ArgRef,
    ) -> Res {
        let (cb, direct, candidate) = {
            let a = Self::al(info, id);
            let cb = if a.first_queue {
                a.first_queue = false;
                Some(id)
            } else {
                None
            };
            (cb, a.direct, a.candidate.clone())
        };
        if direct {
            self.queue_direct_type_update(info, next, candidate, cb)
        } else {
            self.queue_type_update(cfg, info, next, candidate, cb)
        }
    }

    fn args_list_run_first_queue(
        &self,
        cfg: &mut MethodCfg,
        info: &mut UpdateInfo,
        id: CbId,
    ) -> Res {
        Self::al(info, id).first_queue = true;
        self.args_list_callback(cfg, info, id, Same)
    }

    // ---- InvokeUpdateCallback -----------------------------------------------------------

    fn ic(info: &mut UpdateInfo, id: CbId) -> &mut InvokeCb {
        match &mut info.cbs[id] {
            Cb::Invoke(a) => a,
            _ => panic!("not an invoke callback"),
        }
    }

    fn invoke_callback(
        &self,
        cfg: &mut MethodCfg,
        info: &mut UpdateInfo,
        id: CbId,
        mut result: UpdateResult,
    ) -> Res {
        loop {
            match result {
                Changed => Self::ic(info, id).all_same = false,
                Reject => {
                    let (ua, ut, is_assign) = {
                        let c = Self::ic(info, id);
                        (c.update_arg, c.update_type.clone(), c.is_assign)
                    };
                    if let (Some(ua), Some(ut)) = (ua, ut) {
                        let compare = self.compare.compare_types(&ut, &cfg.arg_type(ua));
                        if if is_assign {
                            compare.is_wider()
                        } else {
                            compare.is_narrow()
                        } {
                            return Ok(Some(Reject));
                        }
                    }
                }
                Same => {}
            }
            if !self.invoke_next_arg(cfg, info, id) {
                let all_same = Self::ic(info, id).all_same;
                return Ok(Some(if all_same { Same } else { Changed }));
            }
            let (ua, ut, cb) = {
                let c = Self::ic(info, id);
                let cb = if c.first_queue {
                    c.first_queue = false;
                    Some(id)
                } else {
                    None
                };
                (
                    c.update_arg.expect("update arg"),
                    c.update_type.clone().expect("update type"),
                    cb,
                )
            };
            match self.queue_type_update(cfg, info, ua, ut, cb)? {
                None => return Ok(None),
                Some(r) => result = r,
            }
        }
    }

    fn invoke_run_queue(&self, cfg: &mut MethodCfg, info: &mut UpdateInfo, id: CbId) -> Res {
        Self::ic(info, id).first_queue = true;
        let mut result = Same;
        let invoke = Self::ic(info, id).invoke;
        let has_result = cfg.insn(invoke).result.is_some();
        if has_result && !cfg.arg_type_immutable(ArgRef::Result(invoke)) {
            let rt = self.invoke_return_type(info, id);
            let known = Self::ic(info, id).known_type_vars.clone();
            if let Some(rt) = self.check_type(&known, rt) {
                let res_arg = ArgRef::Result(invoke);
                {
                    let c = Self::ic(info, id);
                    c.update_arg = Some(res_arg);
                    c.update_type = Some(rt.clone());
                    c.is_assign = true;
                    c.first_queue = false;
                }
                match self.queue_type_update(cfg, info, res_arg, rt, Some(id))? {
                    None => return Ok(None),
                    Some(r) => result = r,
                }
            }
        }
        self.invoke_callback(cfg, info, id, result)
    }

    fn invoke_return_type(&self, info: &mut UpdateInfo, id: CbId) -> Option<ArgType> {
        let c = Self::ic(info, id);
        if c.type_vars_map.is_empty() {
            Some(c.return_type.clone())
        } else {
            replace_type_variables_using_map(&c.return_type, &c.type_vars_map)
        }
    }

    fn invoke_arg_type(&self, info: &mut UpdateInfo, id: CbId, i: usize) -> Option<ArgType> {
        let (map_empty, arg_type, candidate) = {
            let c = Self::ic(info, id);
            (
                c.type_vars_map.is_empty(),
                c.arg_types.get(i).cloned(),
                c.candidate.clone(),
            )
        };
        let arg_type = arg_type?;
        if map_empty {
            Some(arg_type)
        } else {
            self.tu().replace_class_generics(&candidate, &arg_type)
        }
    }

    fn invoke_next_arg(&self, cfg: &MethodCfg, info: &mut UpdateInfo, id: CbId) -> bool {
        loop {
            let i = {
                let c = Self::ic(info, id);
                c.current_arg += 1;
                c.current_arg as usize
            };
            let (args_count, invoke) = {
                let c = Self::ic(info, id);
                (c.args_count, c.invoke)
            };
            if i >= args_count {
                return false;
            }
            let offset = first_arg_offset(cfg.insn(invoke));
            let invoke_arg = ArgRef::Arg(invoke, offset + i);
            if !cfg.arg_type_immutable(invoke_arg) {
                let at = self.invoke_arg_type(info, id, i);
                let known = Self::ic(info, id).known_type_vars.clone();
                if let Some(t) = self.check_type(&known, at) {
                    let c = Self::ic(info, id);
                    c.update_arg = Some(invoke_arg);
                    c.update_type = Some(t);
                    c.is_assign = false;
                    return true;
                }
            }
        }
    }

    fn check_type(&self, known: &[ArgType], ty: Option<ArgType>) -> Option<ArgType> {
        let ty = ty?;
        if ty.is_wildcard() {
            return None;
        }
        if ty.contains_type_variable() {
            if known.is_empty() {
                return None;
            }
            let has_unknown = ty.visit_types(&mut |t: &ArgType| {
                if t.is_generic_type() && !known.contains(t) {
                    Some(true)
                } else {
                    None
                }
            });
            if has_unknown.is_some() {
                return None;
            }
        }
        Some(ty)
    }

    // ---- listeners ---------------------------------------------------------------------

    fn invoke_listener(
        &self,
        cfg: &mut MethodCfg,
        info: &mut UpdateInfo,
        insn: usize,
        arg: ArgRef,
        candidate: &ArgType,
    ) -> Res {
        if arg == ArgRef::Result(insn) {
            return Ok(Some(Same));
        }
        if has_instance_arg(cfg.insn(insn)) && arg == ArgRef::Arg(insn, 0) {
            let Some(details) = insn_method_details(self.root, cfg.insn(insn)) else {
                return Ok(Some(Same));
            };
            let tu = self.tu();
            let known = tu.known_type_vars_at_method(self.cls, self.mth);
            let map = tu.type_variables_mapping(candidate);
            let return_type = details.return_type(self.root);
            let arg_types = details.arg_types(self.root);
            let args_count = arg_types.len();
            let id = info.add_cb(Cb::Invoke(InvokeCb {
                invoke: insn,
                args_count,
                known_type_vars: known,
                return_type,
                arg_types,
                type_vars_map: map,
                candidate: candidate.clone(),
                is_assign: false,
                all_same: true,
                current_arg: -1,
                update_arg: None,
                update_type: None,
                first_queue: false,
            }));
            return self.invoke_run_queue(cfg, info, id);
        }
        Ok(Some(Same))
    }

    fn same_first_arg_listener(
        &self,
        cfg: &mut MethodCfg,
        info: &mut UpdateInfo,
        insn: usize,
        arg: ArgRef,
        candidate: &ArgType,
    ) -> Res {
        let change_arg = if arg == ArgRef::Result(insn) {
            ArgRef::Arg(insn, 0)
        } else {
            ArgRef::Result(insn)
        };
        if info.has_update_with_type(change_arg, candidate) {
            return Ok(Some(Changed));
        }
        self.queue_type_update(cfg, info, change_arg, candidate.clone(), None)
    }

    fn move_listener(
        &self,
        cfg: &mut MethodCfg,
        info: &mut UpdateInfo,
        insn: usize,
        arg: ArgRef,
        candidate: &ArgType,
    ) -> Res {
        if cfg.insn(insn).result.is_none() {
            return Ok(Some(Changed));
        }
        let assign_changed = arg == ArgRef::Result(insn);
        let change_arg = if assign_changed {
            ArgRef::Arg(insn, 0)
        } else {
            ArgRef::Result(insn)
        };
        let cmp = self
            .compare
            .compare_types(candidate, &cfg.arg_type(change_arg));
        let correct = cmp.is_equal()
            || if assign_changed {
                cmp.is_wider()
            } else {
                cmp.is_narrow()
            };
        let cb = info.add_cb(Cb::Move {
            change_arg,
            candidate: candidate.clone(),
            correct_type: correct,
        });
        self.queue_type_update(cfg, info, change_arg, candidate.clone(), Some(cb))
    }

    fn all_same_listener(
        &self,
        cfg: &mut MethodCfg,
        info: &mut UpdateInfo,
        insn: usize,
        arg: ArgRef,
        candidate: &ArgType,
    ) -> Res {
        if arg != ArgRef::Result(insn) {
            return self.queue_type_update(
                cfg,
                info,
                ArgRef::Result(insn),
                candidate.clone(),
                None,
            );
        }
        let args: Vec<ArgRef> = (0..cfg.insn(insn).args.len())
            .map(|k| ArgRef::Arg(insn, k))
            .collect();
        let cb = info.add_cb(Cb::ArgsList(ArgsListCb {
            args,
            next: 0,
            candidate: candidate.clone(),
            direct: false,
            skip: Some(arg),
            final_ssa: None,
            ignore_reject: false,
            all_same: true,
            first_queue: false,
        }));
        self.args_list_run_first_queue(cfg, info, cb)
    }

    fn arith_listener(
        &self,
        cfg: &mut MethodCfg,
        info: &mut UpdateInfo,
        insn: usize,
        arg: ArgRef,
        candidate: &ArgType,
    ) -> Res {
        let is_bit_op = matches!(
            &cfg.insn(insn).kind,
            InsnKind::Arith(op) if op.is_bit_op()
        );
        let _ = ArithOp::Add;
        if *candidate == ArgType::boolean() && is_bit_op {
            return self.all_same_listener(cfg, info, insn, arg, candidate);
        }
        self.suggest_all_same_listener(cfg, info, insn, arg, candidate)
    }

    fn suggest_all_same_listener(
        &self,
        cfg: &mut MethodCfg,
        info: &mut UpdateInfo,
        insn: usize,
        arg: ArgRef,
        candidate: &ArgType,
    ) -> Res {
        let args: Vec<ArgRef> = (0..cfg.insn(insn).args.len())
            .map(|k| ArgRef::Arg(insn, k))
            .collect();
        let cb = info.add_cb(Cb::ArgsList(ArgsListCb {
            args,
            next: 0,
            candidate: candidate.clone(),
            direct: false,
            skip: Some(arg),
            final_ssa: None,
            ignore_reject: true,
            all_same: true,
            first_queue: false,
        }));
        if arg != ArgRef::Result(insn) && cfg.insn(insn).result.is_some() {
            return self.queue_type_update(
                cfg,
                info,
                ArgRef::Result(insn),
                candidate.clone(),
                Some(cb),
            );
        }
        self.args_list_run_first_queue(cfg, info, cb)
    }

    fn check_cast_listener(
        &self,
        cfg: &mut MethodCfg,
        info: &mut UpdateInfo,
        insn: usize,
        arg: ArgRef,
        candidate: &ArgType,
    ) -> Res {
        if arg == ArgRef::Result(insn) {
            let cb = info.add_cb(Cb::CheckCastAssign);
            return self.queue_type_update(
                cfg,
                info,
                ArgRef::Arg(insn, 0),
                candidate.clone(),
                Some(cb),
            );
        }
        let cast_type = match &cfg.insn(insn).kind {
            InsnKind::Index(IndexValue::Type(t)) => t.clone(),
            _ => return Ok(Some(Same)),
        };
        let res = self.compare.compare_types(candidate, &cast_type);
        if res == TypeCompareEnum::Conflict && !self.is_interfaces(candidate, &cast_type) {
            return Ok(Some(Reject));
        }
        let soft = cfg.insn(insn).contains_flag(iflag::SOFT_CAST);
        if res == TypeCompareEnum::ConflictByGeneric && !soft {
            return Ok(Some(Reject));
        }
        if res == TypeCompareEnum::NarrowByGeneric && candidate.contains_generic() {
            return self.queue_type_update(
                cfg,
                info,
                ArgRef::Result(insn),
                candidate.clone(),
                None,
            );
        }
        let current = cfg.arg_type(ArgRef::Arg(insn, 0));
        Ok(Some(if *candidate == current { Same } else { Changed }))
    }

    fn is_interfaces(&self, first: &ArgType, second: &ArgType) -> bool {
        if !first.is_object() || !second.is_object() {
            return false;
        }
        let details = |t: &ArgType| t.object_name().and_then(|n| self.root.clsp.cls_details(&n));
        let (f, s) = (details(first), details(second));
        let is_iface = |c: &crate::clsp::ClspClass| c.access_flags & 0x200 != 0;
        if f.is_some_and(|c| !is_iface(c)) {
            return false;
        }
        if s.is_some_and(|c| !is_iface(c)) {
            return false;
        }
        match (f, s) {
            (Some(f), Some(s)) => is_iface(s) && is_iface(f),
            _ => true,
        }
    }

    fn array_get_listener(
        &self,
        cfg: &mut MethodCfg,
        info: &mut UpdateInfo,
        insn: usize,
        arg: ArgRef,
        candidate: &ArgType,
    ) -> Res {
        let arr = ArgRef::Arg(insn, 0);
        if arg == ArgRef::Result(insn) {
            let cb = info.add_cb(Cb::ArrayGetAssign {
                arr_arg: arr,
                candidate: candidate.clone(),
            });
            return self.queue_type_update(
                cfg,
                info,
                arr,
                ArgType::array(candidate.clone()),
                Some(cb),
            );
        }
        if arr == arg {
            let Some(element) = candidate.array_element().cloned() else {
                return Ok(Some(Reject));
            };
            let cb = info.add_cb(Cb::ArrayGetArr {
                result: ArgRef::Result(insn),
                element: element.clone(),
            });
            return self.queue_type_update(cfg, info, ArgRef::Result(insn), element, Some(cb));
        }
        Ok(Some(Same))
    }

    fn array_put_listener(
        &self,
        cfg: &mut MethodCfg,
        info: &mut UpdateInfo,
        insn: usize,
        arg: ArgRef,
        candidate: &ArgType,
    ) -> Res {
        let arr = ArgRef::Arg(insn, 0);
        let put = ArgRef::Arg(insn, 2);
        if arr == arg {
            let Some(element) = candidate.array_element().cloned() else {
                return Ok(Some(Reject));
            };
            let cb = info.add_cb(Cb::ArrayPut {
                put_arg: put,
                element: element.clone(),
            });
            return self.queue_type_update(cfg, info, put, element, Some(cb));
        }
        if arr == put {
            return self.queue_type_update(cfg, info, arr, ArgType::array(candidate.clone()), None);
        }
        Ok(Some(Same))
    }

    fn if_listener(
        &self,
        cfg: &mut MethodCfg,
        info: &mut UpdateInfo,
        insn: usize,
        arg: ArgRef,
        candidate: &ArgType,
    ) -> Res {
        let first = ArgRef::Arg(insn, 0);
        let second = ArgRef::Arg(insn, 1);
        let update_arg = if first == arg { second } else { first };
        let cb = info.add_cb(Cb::If {
            update_arg,
            candidate: candidate.clone(),
        });
        self.queue_type_update(cfg, info, update_arg, candidate.clone(), Some(cb))
    }
}

fn is_register_arg(cfg: &MethodCfg, at: ArgRef) -> bool {
    match at {
        ArgRef::Arg(i, k) => matches!(
            cfg.insn(i).args[k],
            crate::model::insn::InsnArg::Register(_)
        ),
        _ => true,
    }
}

fn check_assign_for_unknown(bound_type: &ArgType, candidate: &ArgType) -> bool {
    if *bound_type == ArgType::unknown_any() {
        return true;
    }
    let candidate_array = candidate.is_array();
    if bound_type.is_array() && candidate_array {
        if let (Some(b), Some(c)) = (bound_type.array_element(), candidate.array_element()) {
            return check_assign_for_unknown(b, c);
        }
    }
    if candidate_array && bound_type.contains(PrimitiveType::Array) {
        return true;
    }
    if candidate.is_object() && bound_type.contains(PrimitiveType::Object) {
        return true;
    }
    if candidate.is_primitive() {
        if let Some(p) = candidate.primitive_type() {
            if bound_type.contains(p) {
                return true;
            }
        }
    }
    false
}
