//! `FixTypesVisitor`: try various methods to fix unresolved types.

use super::search::TypeSearch;
use super::update::{TypeUpdate, UpdateResult};
use super::{init_type_bounds, is_overflow, run_type_propagation, select_best_type_from_bounds};
use crate::method_utils::{has_instance_arg, insn_method_details};
use crate::model::cfg::{bflag, MethodCfg};
use crate::model::insn::{
    iflag, ArithOp, BlockId, IfOp, IndexValue, InsnArg, InsnId, InsnKind, InsnNode, InsnType,
    LiteralArg, RegisterArg,
};
use crate::model::node::Root;
use crate::model::ssa::{ArgRef, BoundKind, SsaId, TypeBound};
use crate::model::types::{ArgType, PrimitiveType};
use crate::type_compare::TypeCompareEnum;
use crate::type_utils::TypeUtils;

/// `ModVisitor.DOUBLE_TO_BITS`: `Double.doubleToLongBits(1)`
const DOUBLE_TO_BITS: i64 = 0x3FF0_0000_0000_0000;
/// `ModVisitor.FLOAT_TO_BITS`: `Float.floatToIntBits(1)`
const FLOAT_TO_BITS: i64 = 0x3F80_0000;

const RESOLVERS_COUNT: usize = 10;

/// `FixTypesVisitor.visit`
pub fn run_fix_types(
    cfg: &mut MethodCfg,
    root: &Root,
    cls: usize,
    mth: usize,
) -> Result<(), String> {
    cfg.types_fixed = true;
    if check_types(cfg) {
        return Ok(());
    }
    let fix = FixTypes {
        root,
        cls,
        mth,
        upd: TypeUpdate::new(root, cls, mth, cfg.units_count),
    };
    let result = (|| -> Result<(), String> {
        for resolver in 0..RESOLVERS_COUNT {
            if fix.run_resolver(resolver, cfg)? && check_types(cfg) {
                break;
            }
        }
        Ok(())
    })();
    if let Err(e) = result {
        cfg.errors.push(format!("Types fix failed: {e}"));
    }
    Ok(())
}

/// Check if all types are resolved.
fn check_types(cfg: &MethodCfg) -> bool {
    cfg.svar_list
        .iter()
        .all(|v| cfg.svars[*v].ty.is_type_known())
}

struct FixTypes<'a> {
    root: &'a Root,
    cls: usize,
    mth: usize,
    upd: TypeUpdate<'a>,
}

impl FixTypes<'_> {
    /// The resolvers in the order of the reference.
    fn run_resolver(&self, index: usize, cfg: &mut MethodCfg) -> Result<bool, String> {
        match index {
            0 => self.apply_field_type(cfg),
            1 => self.try_restore_type_var_casts(cfg),
            2 => self.try_insert_casts(cfg),
            3 => self.try_deduce_types(cfg),
            4 => self.try_split_const_insns(cfg),
            5 => self.try_to_fix_incompatible_primitives(cfg),
            6 => self.try_to_force_immutable_types(cfg),
            7 => self.try_insert_additional_move(cfg),
            8 => self.run_multi_variable_search(cfg),
            _ => self.try_remove_generics(cfg),
        }
    }

    fn tu(&self) -> TypeUtils<'_> {
        TypeUtils::new(self.root)
    }

    /// `TypeUtils.replaceClassGenerics(instanceType, genericSourceType, typeWithGeneric)`:
    /// the reference fails on a source type that is not a class type while the instance
    /// type is a class of the input.
    fn replace_class_generics(
        &self,
        instance: &ArgType,
        source: &ArgType,
        with_generic: &ArgType,
    ) -> Result<Option<ArgType>, String> {
        let tu = self.tu();
        if source.object_name().is_none() && tu.class_type_vars(instance).is_some() {
            return Err("ArgType.getObject() is not supported for this type".into());
        }
        Ok(tu.replace_class_generics_from(instance, source, with_generic))
    }

    /// `typeInference.initTypeBounds(mth)`
    fn init_bounds(&self, cfg: &mut MethodCfg) -> Result<(), String> {
        init_type_bounds(cfg, self.root, self.cls, self.mth)
    }

    /// `typeInference.runTypePropagation(mth)` (always `true` in the reference)
    fn propagate(&self, cfg: &mut MethodCfg) -> Result<bool, String> {
        run_type_propagation(cfg, &self.upd)?;
        Ok(true)
    }

    /// Bound type without update info (`ITypeBound.getType()`).
    fn bound_type(&self, cfg: &MethodCfg, b: &TypeBound) -> Result<ArgType, String> {
        self.upd
            .bound_type(cfg, None, b)
            .ok_or_else(|| "bound without type".to_string())
    }

    fn run_multi_variable_search(&self, cfg: &mut MethodCfg) -> Result<bool, String> {
        let result = {
            let mut search = TypeSearch::new(cfg, &self.upd);
            search.run()
        };
        match result {
            Ok(ok) => {
                if !ok {
                    cfg.warnings
                        .push("Multi-variable type inference failed".to_string());
                }
                Ok(check_types(cfg))
            }
            Err(e) => {
                cfg.warnings
                    .push(format!("Multi-variable type inference failed. Error: {e}"));
                Ok(false)
            }
        }
    }

    fn set_best_type(&self, cfg: &mut MethodCfg, var: SsaId) -> Result<bool, String> {
        match self.calculate_from_bounds(cfg, var) {
            Ok(b) => Ok(b),
            Err(e) if is_overflow(&e) => Err(e),
            Err(e) => {
                cfg.warnings.push(format!(
                    "Failed to calculate best type for var: r{}v{}: {e}",
                    cfg.svars[var].reg_num, cfg.svars[var].version
                ));
                Ok(false)
            }
        }
    }

    fn calculate_from_bounds(&self, cfg: &mut MethodCfg, var: SsaId) -> Result<bool, String> {
        let Some(candidate) = select_best_type_from_bounds(cfg, &self.upd, var) else {
            return Ok(false);
        };
        let result = self.upd.apply(cfg, var, &candidate)?;
        Ok(result == UpdateResult::Changed)
    }

    fn try_possible_types(
        &self,
        cfg: &mut MethodCfg,
        var: SsaId,
        ty: &ArgType,
    ) -> Result<bool, String> {
        let types = self.make_possible_types_list(cfg, ty, Some(var))?;
        for candidate in types {
            if self.upd.apply(cfg, var, &candidate)? == UpdateResult::Changed {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn make_possible_types_list(
        &self,
        cfg: &MethodCfg,
        ty: &ArgType,
        var: Option<SsaId>,
    ) -> Result<Vec<ArgType>, String> {
        if let Some(elem) = ty.array_element() {
            let mut list = Vec::new();
            for e in self.make_possible_types_list(cfg, elem, None)? {
                list.push(ArgType::array(e));
            }
            return Ok(list);
        }
        if let Some(var) = var {
            for b in &cfg.svars[var].bounds {
                let bt = self.bound_type(cfg, b)?;
                if bt.is_object() || bt.is_array() {
                    // don't add primitive types
                    return Ok(Vec::new());
                }
            }
        }
        let mut list = Vec::new();
        for p in ty.possible_types() {
            if p == PrimitiveType::Void {
                continue;
            }
            list.push(ArgType::convert_from_primitive(p));
        }
        Ok(list)
    }

    fn try_deduce_types(&self, cfg: &mut MethodCfg) -> Result<bool, String> {
        let mut fixed = false;
        for var in cfg.svar_list.clone() {
            if self.deduce_type(cfg, var)? {
                fixed = true;
            }
        }
        Ok(fixed)
    }

    fn deduce_type(&self, cfg: &mut MethodCfg, var: SsaId) -> Result<bool, String> {
        if cfg.svar_type_immutable(var) {
            return Ok(false);
        }
        let ty = cfg.svars[var].ty.clone();
        if ty.is_type_known() {
            return Ok(false);
        }
        // try best type from bounds again
        if self.set_best_type(cfg, var)? {
            return Ok(true);
        }
        // try all possible types (useful for primitives)
        if self.try_possible_types(cfg, var, &ty)? {
            return Ok(true);
        }
        // for objects try super types
        self.try_wider_objects(cfg, var)
    }

    fn try_remove_generics(&self, cfg: &mut MethodCfg) -> Result<bool, String> {
        let mut resolved = true;
        for var in cfg.svar_list.clone() {
            if !cfg.svars[var].ty.is_type_known()
                && !cfg.svar_type_immutable(var)
                && !self.try_raw_type(cfg, var)?
            {
                resolved = false;
            }
        }
        Ok(resolved)
    }

    /// Known object types of the bounds (insertion ordered set).
    fn bound_object_types(&self, cfg: &MethodCfg, var: SsaId) -> Result<Vec<ArgType>, String> {
        let mut types: Vec<ArgType> = Vec::new();
        for b in &cfg.svars[var].bounds {
            let bt = self.bound_type(cfg, b)?;
            if bt.is_type_known() && bt.is_object() && !types.contains(&bt) {
                types.push(bt);
            }
        }
        Ok(types)
    }

    fn try_raw_type(&self, cfg: &mut MethodCfg, var: SsaId) -> Result<bool, String> {
        let obj_types = self.bound_object_types(cfg, var)?;
        if obj_types.is_empty() {
            return Ok(false);
        }
        for obj_type in &obj_types {
            if self.check_raw_type(cfg, var, obj_type)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn check_raw_type(
        &self,
        cfg: &mut MethodCfg,
        var: SsaId,
        obj_type: &ArgType,
    ) -> Result<bool, String> {
        if obj_type.is_object() && obj_type.contains_generic() {
            let raw_type = if obj_type.is_generic_type() {
                ArgType::object_type()
            } else {
                ArgType::object(&obj_type.object_name().unwrap_or_default())
            };
            let result = self.upd.apply_with_wider_allow(cfg, var, &raw_type)?;
            return Ok(result == UpdateResult::Changed);
        }
        Ok(false)
    }

    /// Use type for var assigned from field (IGET or SGET).
    /// Insert additional casts at var use places.
    fn apply_field_type(&self, cfg: &mut MethodCfg) -> Result<bool, String> {
        match self.apply_field_type_impl(cfg) {
            Ok(b) => Ok(b),
            Err(e) => {
                cfg.warnings.push(format!(
                    "Type inference fix 'apply assigned field type' failed: {e}"
                ));
                Ok(false)
            }
        }
    }

    fn apply_field_type_impl(&self, cfg: &mut MethodCfg) -> Result<bool, String> {
        let mut changed = false;
        // will add new SSA vars
        let vars_count = cfg.svar_list.len();
        for i in 0..vars_count {
            let var = cfg.svar_list[i];
            if self.try_field_type_with_new_casts(cfg, var, true)? {
                changed = true;
            }
        }
        if !changed {
            return Ok(false);
        }
        // rerun full type inference
        rerun_code_vars(cfg)?;
        self.init_bounds(cfg)?;
        self.propagate(cfg)?;

        // check if changed var types are fixed
        let mut success = true;
        for var in cfg.svar_list.clone() {
            if self.try_field_type_with_new_casts(cfg, var, false)? {
                success = false;
            }
        }
        if !success {
            self.init_bounds(cfg)?;
            self.propagate(cfg)?;
            cfg.warnings
                .push("Type inference incomplete: some casts might be missing".to_string());
        }
        Ok(success)
    }

    fn try_field_type_with_new_casts(
        &self,
        cfg: &mut MethodCfg,
        var: SsaId,
        insert_casts: bool,
    ) -> Result<bool, String> {
        if cfg.svars[var].ty.is_type_known() || cfg.svar_type_immutable(var) {
            return Ok(false);
        }
        let Some(assign_insn) = cfg.svar_assign_insn(var) else {
            return Ok(false);
        };
        let insn_type = cfg.insn(assign_insn).ty;
        if insn_type != InsnType::Iget && insn_type != InsnType::Sget {
            return Ok(false);
        }
        let field_type = cfg.reg(ArgRef::Result(assign_insn)).init_type.clone();
        // field type should be used
        if insert_casts {
            // try to find a use place and insert cast
            let mut inserted = false;
            // iteration over the live use list, a change of the list fails the next step
            let snapshot = cfg.svars[var].uses.clone();
            let mut cursor = 0usize;
            while cursor != cfg.svars[var].uses.len() {
                if cfg.svars[var].uses != snapshot {
                    return Err("ConcurrentModificationException".into());
                }
                let use_ref = cfg.svars[var].uses[cursor];
                cursor += 1;
                if self.insert_explicit_use_cast(cfg, var, use_ref, &field_type)? {
                    inserted = true;
                }
            }
            return Ok(inserted);
        }
        // force field type, will make type inference incomplete,
        // but it is better that completely unknown type
        cfg.svar_set_type(var, field_type)?;
        Ok(true)
    }

    fn insert_explicit_use_cast(
        &self,
        cfg: &mut MethodCfg,
        var: SsaId,
        use_ref: ArgRef,
        field_type: &ArgType,
    ) -> Result<bool, String> {
        let Some(invoke) = use_ref.parent_insn() else {
            return Ok(false);
        };
        if cfg.insn(invoke).ty != InsnType::Invoke || !has_instance_arg(cfg.insn(invoke)) {
            return Ok(false);
        }
        let same_var = matches!(
            &cfg.insn(invoke).args[0],
            InsnArg::Register(r) if r.svar == Some(var)
        );
        if !same_var {
            return Ok(false);
        }
        let Some(details) = insn_method_details(self.root, cfg.insn(invoke)) else {
            return Ok(false);
        };
        let arg_types = details.arg_types(self.root);
        let mut new_casts = 0;
        let mut k: i64 = -1;
        for idx in 0..cfg.insn(invoke).args.len() {
            if idx == 0 {
                continue;
            }
            k += 1;
            if !matches!(cfg.insn(invoke).args[idx], InsnArg::Register(_)) {
                continue;
            }
            let details_arg = arg_types
                .get(k as usize)
                .ok_or_else(|| format!("Index {k} out of bounds for length {}", arg_types.len()))?;
            let at = ArgRef::Arg(invoke, idx);
            let inv_arg_type = cfg.arg_type(at);
            let resolved = self.replace_class_generics(field_type, &inv_arg_type, details_arg)?;
            if let Some(resolved_type) = resolved {
                if resolved_type != inv_arg_type {
                    if let Some(cast) = insert_use_cast(cfg, at, &resolved_type) {
                        cfg.insns[cast].flags |= iflag::EXPLICIT_CAST;
                        new_casts += 1;
                    }
                }
            }
        }
        Ok(new_casts > 0)
    }

    /// Fix check casts to type var extend type:
    /// `<T extends Comparable> T var = (Comparable) obj; => T var = (T) obj;`
    fn try_restore_type_var_casts(&self, cfg: &mut MethodCfg) -> Result<bool, String> {
        let mut changed = 0;
        for var in cfg.svar_list.clone() {
            changed += self.restore_type_var_casts(cfg, var)?;
        }
        if changed == 0 {
            return Ok(false);
        }
        self.init_bounds(cfg)?;
        self.propagate(cfg)
    }

    fn restore_type_var_casts(&self, cfg: &mut MethodCfg, var: SsaId) -> Result<usize, String> {
        let bounds = cfg.svars[var].bounds.clone();
        let mut any_generic = false;
        for b in &bounds {
            if self.bound_type(cfg, b)?.is_generic_type() {
                any_generic = true;
                break;
            }
        }
        if !any_generic {
            return Ok(0);
        }
        let casts: Vec<&TypeBound> = bounds
            .iter()
            .filter(|b| matches!(b, TypeBound::CheckCastAssign { .. }))
            .collect();
        if casts.is_empty() {
            return Ok(0);
        }
        let best_type =
            select_best_type_from_bounds(cfg, &self.upd, var).unwrap_or_else(ArgType::unknown_any);
        if !best_type.is_generic_type() {
            return Ok(0);
        }
        let extend_types = best_type.extend_types();
        if extend_types.len() != 1 {
            return Ok(0);
        }
        let extend_type = extend_types[0].clone();
        let mut fixed = 0;
        for bound in casts {
            let TypeBound::CheckCastAssign { insn } = bound else {
                continue;
            };
            let cast_type = self.bound_type(cfg, bound)?;
            let result = self.upd.compare.compare_types(&extend_type, &cast_type);
            if result.is_equal() || result == TypeCompareEnum::NarrowByGeneric {
                cfg.insns[*insn].kind = InsnKind::Index(IndexValue::Type(best_type.clone()));
                fixed += 1;
            }
        }
        Ok(fixed)
    }

    fn try_insert_casts(&self, cfg: &mut MethodCfg) -> Result<bool, String> {
        let mut added = 0;
        let vars_count = cfg.svar_list.len();
        for i in 0..vars_count {
            let var = cfg.svar_list[i];
            if !cfg.svars[var].ty.is_type_known() && !cfg.svar_type_immutable(var) {
                added += self.try_insert_var_cast(cfg, var)?;
            }
        }
        if added != 0 {
            rerun_code_vars(cfg)?;
            self.init_bounds(cfg)?;
            return self.propagate(cfg);
        }
        Ok(false)
    }

    fn try_insert_var_cast(&self, cfg: &mut MethodCfg, var: SsaId) -> Result<usize, String> {
        for bound in cfg.svars[var].bounds.clone() {
            let bound_type = self.bound_type(cfg, &bound)?;
            if bound_type.is_type_known()
                && bound_type != cfg.svars[var].ty
                && bound_type.contains_type_variable()
                && self
                    .tu()
                    .check_for_unknown_type_vars(self.cls, self.mth, &bound_type)
                    .is_none()
            {
                if let Some(cast) = insert_assign_cast(cfg, var, &bound_type) {
                    cfg.insns[cast].flags |= iflag::SOFT_CAST;
                    return Ok(1);
                }
                return Ok(insert_use_casts(cfg, var));
            }
        }
        Ok(0)
    }

    fn try_split_const_insns(&self, cfg: &mut MethodCfg) -> Result<bool, String> {
        let mut const_split = false;
        for var in cfg.svar_list.clone() {
            if check_and_split_const_insn(cfg, var)? {
                const_split = true;
            }
        }
        if !const_split {
            return Ok(false);
        }
        rerun_code_vars(cfg)?;
        self.init_bounds(cfg)?;
        self.propagate(cfg)
    }

    fn try_insert_additional_move(&self, cfg: &mut MethodCfg) -> Result<bool, String> {
        let mut insns_added = 0;
        for block in cfg.blocks.clone() {
            if let Some(phis) = cfg.block(block).phi_list.clone() {
                for phi in phis {
                    insns_added += try_insert_additional_insn(cfg, phi);
                }
            }
        }
        if insns_added == 0 {
            return Ok(false);
        }
        rerun_code_vars(cfg)?;
        self.init_bounds(cfg)?;
        if self.propagate(cfg)? && check_types(cfg) {
            return Ok(true);
        }
        self.try_deduce_types(cfg)
    }

    fn try_wider_objects(&self, cfg: &mut MethodCfg, var: SsaId) -> Result<bool, String> {
        let obj_types = self.bound_object_types(cfg, var)?;
        if obj_types.is_empty() {
            return Ok(false);
        }
        let clsp = super::clsp_order::ordered_clsp(&self.root.clsp);
        for obj_type in &obj_types {
            let name = obj_type.object_name().unwrap_or_default();
            for ancestor in clsp.super_types(&name) {
                let ancestor_type = ArgType::object(ancestor);
                let result = self.upd.apply_with_wider_allow(cfg, var, &ancestor_type)?;
                if result == UpdateResult::Changed {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    fn try_to_fix_incompatible_primitives(&self, cfg: &mut MethodCfg) -> Result<bool, String> {
        let mut fixed = false;
        // new vars will be added at a list end if fix is applied
        let vars_count = cfg.svar_list.len();
        for i in 0..vars_count {
            let var = cfg.svar_list[i];
            if self.process_incompatible_primitives(cfg, var)? {
                fixed = true;
            }
        }
        if !fixed {
            return Ok(false);
        }
        rerun_code_vars(cfg)?;
        self.init_bounds(cfg)?;
        self.propagate(cfg)
    }

    fn process_incompatible_primitives(
        &self,
        cfg: &mut MethodCfg,
        var: SsaId,
    ) -> Result<bool, String> {
        if cfg.svars[var].ty.is_type_known() {
            return Ok(false);
        }
        let mut assigned = false;
        for b in &cfg.svars[var].bounds {
            let bound_type = self.bound_type(cfg, b)?;
            match b.kind() {
                BoundKind::Assign => {
                    if !bound_type.contains(PrimitiveType::Boolean) {
                        return Ok(false);
                    }
                    assigned = true;
                }
                BoundKind::Use => {
                    if !bound_type.can_be_any_number() {
                        return Ok(false);
                    }
                }
            }
        }
        if !assigned {
            return Ok(false);
        }
        let mut fixed = false;
        for arg in cfg.svars[var].uses.clone() {
            if fix_boolean_usage(cfg, arg)? {
                fixed = true;
            }
        }
        Ok(fixed)
    }

    fn try_to_force_immutable_types(&self, cfg: &mut MethodCfg) -> Result<bool, String> {
        let mut fixed = false;
        for var in cfg.svar_list.clone() {
            if !cfg.svars[var].ty.is_type_known()
                && cfg.svar_type_immutable(var)
                && force_immutable_type(cfg, var)?
            {
                fixed = true;
            }
        }
        if !fixed {
            return Ok(false);
        }
        self.propagate(cfg)
    }
}

/// `InitCodeVariables.rerun`
fn rerun_code_vars(cfg: &mut MethodCfg) -> Result<(), String> {
    for var in cfg.svar_list.clone() {
        // `SSAVar.resetTypeAndCodeVar`
        if !cfg.svar_type_immutable(var) {
            cfg.svars[var].ty = ArgType::unknown_any();
        }
        cfg.svars[var].bounds.clear();
        cfg.svars[var].code_var = None;
    }
    crate::passes::init_code_vars::init_code_vars(cfg)
}

fn force_immutable_type(cfg: &mut MethodCfg, var: SsaId) -> Result<bool, String> {
    for use_ref in cfg.svars[var].uses.clone() {
        if let Some(parent) = use_ref.parent_insn() {
            let t = cfg.insn(parent).ty;
            if t == InsnType::Aget || t == InsnType::Aput {
                let Some(im) = cfg.svar_immutable_type(var) else {
                    return Ok(false);
                };
                cfg.svar_set_type(var, im)?;
                return Ok(true);
            }
        }
    }
    Ok(false)
}

// ---- block and instruction helpers ------------------------------------------------------------

/// `foundWrappedInsn`: `insn` is wrapped (at any depth) into an argument of `container`.
fn found_wrapped_insn(cfg: &MethodCfg, container: InsnId, insn: InsnId) -> bool {
    for a in &cfg.insn(container).args {
        if let InsnArg::Wrapped(w) = a {
            if w.insn == insn || found_wrapped_insn(cfg, w.insn, insn) {
                return true;
            }
        }
    }
    if let InsnKind::Ternary { cond: condition } = cfg.insn(container).kind {
        return found_wrapped_insn(cfg, condition, insn);
    }
    false
}

/// `BlockUtils.getBlockByInsn`
fn get_block_by_insn(cfg: &MethodCfg, insn: InsnId) -> Option<BlockId> {
    if cfg.insn(insn).ty == InsnType::Phi {
        return cfg.blocks.iter().copied().find(|b| {
            cfg.block(*b)
                .phi_list
                .as_ref()
                .is_some_and(|l| l.contains(&insn))
        });
    }
    if cfg.insn(insn).contains_flag(iflag::WRAPPED) {
        return cfg.blocks.iter().copied().find(|b| {
            cfg.block(*b)
                .insns
                .iter()
                .any(|bi| *bi == insn || found_wrapped_insn(cfg, *bi, insn))
        });
    }
    cfg.blocks
        .iter()
        .copied()
        .find(|b| cfg.block(*b).insns.contains(&insn))
}

/// `BlockUtils.getInsnIndexInBlock`
fn insn_index_in_block(cfg: &MethodCfg, block: BlockId, insn: InsnId) -> Option<usize> {
    cfg.block(block).insns.iter().position(|i| *i == insn)
}

/// `BlockUtils.insertBeforeInsn`
fn insert_before_insn(cfg: &mut MethodCfg, block: BlockId, insn: InsnId, new: InsnId) -> bool {
    match insn_index_in_block(cfg, block, insn) {
        Some(i) => {
            cfg.arena[block].insns.insert(i, new);
            true
        }
        None => false,
    }
}

/// `BlockUtils.insertAfterInsn`
fn insert_after_insn(cfg: &mut MethodCfg, block: BlockId, insn: InsnId, new: InsnId) -> bool {
    match insn_index_in_block(cfg, block, insn) {
        Some(i) => {
            cfg.arena[block].insns.insert(i + 1, new);
            true
        }
        None => false,
    }
}

/// `BlockUtils.replaceInsn(mth, block, i, insn)`
fn replace_insn(
    cfg: &mut MethodCfg,
    block: BlockId,
    index: usize,
    new: InsnId,
) -> Result<(), String> {
    let prev = cfg.block(block).insns[index];
    let prev_copy = cfg.insns[prev].clone();
    cfg.insns[new].copy_attributes_from(&prev_copy);
    cfg.inherit_metadata(new, prev);
    cfg.insns[new].offset = prev_copy.offset;
    cfg.arena[block].insns[index] = new;
    let same_result = match (&cfg.insns[new].result, &prev_copy.result) {
        (Some(a), Some(b)) => a.reg_num == b.reg_num && a.svar == b.svar,
        _ => false,
    };
    if same_result {
        // don't unbind the result for the same register
        cfg.unbind_all_args(prev);
    } else {
        cfg.unbind_insn(prev)?;
    }
    cfg.rebind_args(new)?;
    if let InsnKind::Ternary { cond: condition } = cfg.insns[new].kind {
        cfg.rebind_args(condition)?;
    }
    Ok(())
}

/// `InsnNode.copyWithNewSsaVar`
fn copy_with_new_ssa_var(cfg: &mut MethodCfg, insn: InsnId) -> Result<InsnId, String> {
    let Some(result) = cfg.insns[insn].result.clone() else {
        return Err("Result in null".into());
    };
    let (floating, _var) = cfg.duplicate_with_new_svar(&result);
    let new = cfg.copy_without_result(insn);
    let reg = cfg.reg(floating).clone();
    cfg.set_result(new, Some(reg));
    Ok(new)
}

/// `makeCastInsn`: synthetic `CHECK_CAST` without result and argument yet.
fn new_cast_insn(cfg: &mut MethodCfg, cast_type: &ArgType) -> InsnId {
    let mut cast = InsnNode::new(
        InsnType::CheckCast,
        InsnKind::Index(IndexValue::Type(cast_type.clone())),
    );
    cast.flags |= iflag::SYNTHETIC;
    cfg.add_insn(cast)
}

/// Add `arg` (a register copy) to `cast` and bind it as a use of its variable.
fn add_cast_arg(cfg: &mut MethodCfg, cast: InsnId, arg: RegisterArg) {
    let var = arg.svar;
    cfg.insns[cast].args.push(InsnArg::Register(arg));
    if let Some(v) = var {
        let at = ArgRef::Arg(cast, cfg.insns[cast].args.len() - 1);
        cfg.svar_use(v, at);
    }
}

/// `insertAssignCast`
fn insert_assign_cast(cfg: &mut MethodCfg, var: SsaId, cast_type: &ArgType) -> Option<InsnId> {
    let assign = cfg.svars[var].assign;
    let assign_insn = assign.parent_insn()?;
    if cfg.insn(assign_insn).ty == InsnType::Phi {
        return None;
    }
    let block = get_block_by_insn(cfg, assign_insn)?;
    // the instruction gets a new variable, the old one is assigned by the cast
    let old_reg = cfg.reg(assign).clone();
    let (floating, new_var) = cfg.duplicate_with_new_svar(&old_reg);
    let new_reg = cfg.reg(floating).clone();
    cfg.set_result(assign_insn, Some(new_reg));

    let cast = new_cast_insn(cfg, cast_type);
    cfg.insns[cast].result = Some(old_reg);
    cfg.svars[var].assign = ArgRef::Result(cast);
    let arg = cfg.reg(ArgRef::Result(assign_insn)).clone();
    debug_assert_eq!(arg.svar, Some(new_var));
    add_cast_arg(cfg, cast, arg);
    if !insert_after_insn(cfg, block, assign_insn, cast) {
        return None;
    }
    Some(cast)
}

/// `IfNode`-argument check of the reference: `getArg(1).isZeroConst()`
fn is_zero_const(cfg: &MethodCfg, insn: InsnId, k: usize) -> bool {
    match cfg.insn(insn).args.get(k) {
        Some(InsnArg::Literal(l)) => l.literal == 0,
        Some(InsnArg::Wrapped(w)) => {
            let wi = cfg.insn(w.insn);
            wi.ty == InsnType::Const
                && matches!(wi.args.first(), Some(InsnArg::Literal(l)) if l.literal == 0)
        }
        _ => false,
    }
}

/// `insertUseCast`
fn insert_use_cast(cfg: &mut MethodCfg, use_ref: ArgRef, cast_type: &ArgType) -> Option<InsnId> {
    let use_insn = use_ref.parent_insn()?;
    let ArgRef::Arg(_, k) = use_ref else {
        return None;
    };
    let use_type = cfg.insn(use_insn).ty;
    if use_type == InsnType::Phi {
        return None;
    }
    if use_type == InsnType::If && is_zero_const(cfg, use_insn, 1) {
        // cast isn't needed if compare with null
        return None;
    }
    let block = get_block_by_insn(cfg, use_insn)?;
    let use_reg = cfg.reg(use_ref).clone();
    let (floating, _new_var) = cfg.duplicate_with_new_svar(&use_reg);
    let cast = new_cast_insn(cfg, cast_type);
    let res = cfg.reg(floating).clone();
    cfg.set_result(cast, Some(res));
    add_cast_arg(cfg, cast, use_reg);
    let new_use = cfg.reg(ArgRef::Result(cast)).clone();
    cfg.replace_arg(use_insn, k, InsnArg::Register(new_use));
    if !insert_before_insn(cfg, block, use_insn, cast) {
        return None;
    }
    Some(cast)
}

/// `insertUseCasts`
fn insert_use_casts(cfg: &mut MethodCfg, var: SsaId) -> usize {
    let use_list = cfg.svars[var].uses.clone();
    let mut use_casts = 0;
    for use_ref in use_list {
        let Some(init_type) = cfg.try_reg_mut(use_ref).map(|r| r.init_type.clone()) else {
            continue;
        };
        if let Some(cast) = insert_use_cast(cfg, use_ref, &init_type) {
            cfg.insns[cast].flags |= iflag::SOFT_CAST;
            use_casts += 1;
        }
    }
    use_casts
}

// ---- const instructions splitting -------------------------------------------------------------

fn check_and_split_const_insn(cfg: &mut MethodCfg, var: SsaId) -> Result<bool, String> {
    if cfg.svars[var].ty.is_type_known() || cfg.svar_type_immutable(var) {
        return Ok(false);
    }
    Ok(split_by_phi(cfg, var)? || dup_const(cfg, var)?)
}

fn dup_const(cfg: &mut MethodCfg, var: SsaId) -> Result<bool, String> {
    let Some(assign_insn) = cfg.svar_assign_insn(var) else {
        return Ok(false);
    };
    if cfg.insn(assign_insn).ty != InsnType::Const {
        return Ok(false);
    }
    if cfg.svars[var].uses.len() < 2 {
        return Ok(false);
    }
    let Some(assign_block) = get_block_by_insn(cfg, assign_insn) else {
        return Ok(false);
    };
    cfg.insns[assign_insn].flags &= !iflag::DONT_INLINE;
    let insert_index = match insn_index_in_block(cfg, assign_block, assign_insn) {
        Some(i) => i + 1,
        None => 0,
    };
    let use_list = cfg.svars[var].uses.clone();
    for (i, use_ref) in use_list.into_iter().enumerate() {
        if let Some(r) = cfg.try_reg_mut(use_ref) {
            r.dont_inline_const = false;
        }
        if i == 0 {
            continue;
        }
        let (Some(use_insn), ArgRef::Arg(_, k)) = (use_ref.parent_insn(), use_ref) else {
            continue;
        };
        let new_insn = copy_with_new_ssa_var(cfg, assign_insn)?;
        cfg.arena[assign_block].insns.insert(insert_index, new_insn);
        let new_use = cfg.reg(ArgRef::Result(new_insn)).clone();
        cfg.replace_arg(use_insn, k, InsnArg::Register(new_use));
    }
    Ok(true)
}

/// For every PHI make separate CONST insn.
fn split_by_phi(cfg: &mut MethodCfg, var: SsaId) -> Result<bool, String> {
    let used_in_phi = cfg.svars[var].used_in_phi.clone().unwrap_or_default();
    if used_in_phi.len() < 2 {
        return Ok(false);
    }
    let Some(const_insn) = cfg
        .svar_assign_insn(var)
        .filter(|i| cfg.insn(*i).ty == InsnType::Const)
    else {
        return Ok(false);
    };
    let Some(block) = get_block_by_insn(cfg, const_insn) else {
        return Ok(false);
    };
    for phi in used_in_phi.into_iter().skip(1) {
        let copy = copy_with_new_ssa_var(cfg, const_insn)?;
        cfg.insns[copy].flags |= iflag::SYNTHETIC;
        insert_after_insn(cfg, block, const_insn, copy);

        let phi_arg = cfg
            .insn(phi)
            .args
            .iter()
            .position(|a| matches!(a, InsnArg::Register(r) if r.svar == Some(var)));
        if let Some(k) = phi_arg {
            let new_arg = cfg.reg(ArgRef::Result(copy)).clone();
            cfg.replace_arg(phi, k, InsnArg::Register(new_arg));
        }
    }
    Ok(true)
}

// ---- additional moves before phi --------------------------------------------------------------

/// `BlockSplitter.isSeparate`
fn is_separate(t: InsnType) -> bool {
    matches!(
        t,
        InsnType::Return
            | InsnType::If
            | InsnType::Switch
            | InsnType::MonitorEnter
            | InsnType::MonitorExit
            | InsnType::Throw
            | InsnType::MoveException
    )
}

/// Add MOVE instruction before PHI in bound blocks to make 'soft' type link.
/// This allows using different types in blocks merged by PHI.
fn try_insert_additional_insn(cfg: &mut MethodCfg, phi: InsnId) -> usize {
    if let Some(phi_type) = common_type_for_phi_args(cfg, phi) {
        if phi_type.is_type_known() {
            // all args have the same known type => nothing to do here
            return 0;
        }
    }
    // check if instructions can be inserted
    if insert_moves_for_phi(cfg, phi, false) == 0 {
        return 0;
    }
    // check passed => apply
    insert_moves_for_phi(cfg, phi, true)
}

fn common_type_for_phi_args(cfg: &MethodCfg, phi: InsnId) -> Option<ArgType> {
    let mut phi_arg_type: Option<ArgType> = None;
    for k in 0..cfg.insn(phi).args.len() {
        let ty = cfg.arg_type(ArgRef::Arg(phi, k));
        match &phi_arg_type {
            None => phi_arg_type = Some(ty),
            Some(t) => {
                if *t != ty {
                    return None;
                }
            }
        }
    }
    phi_arg_type
}

fn insert_moves_for_phi(cfg: &mut MethodCfg, phi: InsnId, apply: bool) -> usize {
    let args_count = cfg.insn(phi).args.len();
    let mut count = 0;
    for arg_index in 0..args_count {
        let start_block = cfg.phi_binds(phi)[arg_index];
        let Some(block) = check_block_for_insn_insert(cfg, start_block) else {
            // the reference adds a debug comment here
            return 0;
        };
        let mut add = true;
        let var = match &cfg.insn(phi).args[arg_index] {
            InsnArg::Register(r) => r.svar,
            _ => None,
        };
        if let Some(var) = var {
            if let Some(assign_insn) = cfg.svar_assign_insn(var) {
                let assign_type = cfg.insn(assign_insn).ty;
                if assign_type == InsnType::Const
                    || (assign_type == InsnType::Move && cfg.svars[var].use_count() == 1)
                {
                    add = false;
                }
            }
        }
        if add {
            count += 1;
            if apply {
                insert_move(cfg, block, phi, arg_index);
            }
        }
    }
    count
}

fn insert_move(cfg: &mut MethodCfg, block: BlockId, phi: InsnId, arg_index: usize) {
    let reg = cfg.reg(ArgRef::Arg(phi, arg_index)).clone();
    let (floating, new_var) = cfg.duplicate_with_new_svar(&reg);
    let result = cfg.reg(floating).clone();

    let mut mv = InsnNode::plain(InsnType::Move);
    mv.flags |= iflag::SYNTHETIC;
    let move_id = cfg.add_insn(mv);
    cfg.set_result(move_id, Some(result));
    add_cast_arg(cfg, move_id, reg.clone());
    cfg.arena[block].insns.push(move_id);

    let mut replacement = reg;
    replacement.svar = Some(new_var);
    cfg.replace_arg(phi, arg_index, InsnArg::Register(replacement));
}

fn check_block_for_insn_insert(cfg: &MethodCfg, block: BlockId) -> Option<BlockId> {
    if cfg.block(block).contains(bflag::SYNTHETIC) {
        return None;
    }
    if let Some(last) = cfg.last_insn(block) {
        if is_separate(last.ty) {
            // can't insert move in a block with 'separate' instruction
            // => try previous block by simple path
            let preds = &cfg.block(block).preds;
            if preds.len() == 1 {
                return check_block_for_insn_insert(cfg, preds[0]);
            }
            return None;
        }
    }
    Some(block)
}

// ---- boolean usage fix ------------------------------------------------------------------------

/// `ModVisitor.makeBooleanConvertInsn`: `result = (cast_arg == true) ? 1 : 0`.
/// The result is stored as is, binding it to its variable is up to the caller.
fn make_boolean_convert_insn(
    cfg: &mut MethodCfg,
    result: RegisterArg,
    cast_arg: InsnArg,
    ty: &ArgType,
) -> InsnId {
    let literal = |value: i64| {
        InsnArg::Literal(LiteralArg {
            literal: value,
            ty: ty.clone(),
            explicit_primitive: false,
        })
    };
    let zero = literal(0);
    let one_value = if *ty == ArgType::double() {
        DOUBLE_TO_BITS
    } else if *ty == ArgType::float() {
        FLOAT_TO_BITS
    } else {
        1
    };
    let one = literal(one_value);

    let mut if_node = InsnNode::new(
        InsnType::If,
        InsnKind::If {
            op: IfOp::Eq,
            target: -1,
            then_block: None,
            else_block: None,
        },
    );
    if_node.args.push(cast_arg);
    if_node.args.push(InsnArg::Literal(LiteralArg {
        literal: 1,
        ty: ArgType::boolean(),
        explicit_primitive: false,
    }));
    let if_id = cfg.add_insn(if_node);
    let var = match &cfg.insn(if_id).args[0] {
        InsnArg::Register(r) => r.svar,
        _ => None,
    };
    if let Some(v) = var {
        cfg.svar_use(v, ArgRef::Arg(if_id, 0));
    }

    // the true value is never `false` here, so the condition is not inverted
    let mut ternary = InsnNode::new(InsnType::Ternary, InsnKind::Ternary { cond: if_id });
    ternary.args.push(one);
    ternary.args.push(zero);
    ternary.result = Some(result);
    cfg.add_insn(ternary)
}

fn prepare_boolean_convert_insn(
    cfg: &mut MethodCfg,
    result: RegisterArg,
    bound_ref: ArgRef,
    use_type: &ArgType,
) -> Result<InsnId, String> {
    let Some(var) = cfg.reg(bound_ref).svar else {
        return Err("No SSA var for argument".into());
    };
    let use_arg = cfg.reg(cfg.svars[var].assign).clone();
    let convert = make_boolean_convert_insn(cfg, result, InsnArg::Register(use_arg), use_type);
    cfg.insns[convert].flags |= iflag::SYNTHETIC;
    Ok(convert)
}

fn not_boolean_to_int(
    cfg: &mut MethodCfg,
    arith: InsnId,
    bound_ref: ArgRef,
) -> Result<InsnId, String> {
    let mut not_insn = InsnNode::plain(InsnType::Not);
    not_insn.flags |= iflag::SYNTHETIC;
    let not_id = cfg.add_insn(not_insn);
    let bound_reg = cfg.reg(bound_ref).clone();
    add_cast_arg(cfg, not_id, bound_reg);

    let res_type = cfg.reg_type(ArgRef::Result(arith));
    let arith_result = cfg.reg(ArgRef::Result(arith)).clone();
    if res_type.can_be_primitive(PrimitiveType::Boolean) {
        cfg.insns[not_id].result = Some(arith_result);
        return Ok(not_id);
    }
    let wrapped = cfg.wrap_arg(not_id)?;
    let InsnArg::Wrapped(mut w) = wrapped else {
        return Err("wrap failed".into());
    };
    w.ty = ArgType::boolean();
    let convert =
        make_boolean_convert_insn(cfg, arith_result, InsnArg::Wrapped(w), &ArgType::int());
    cfg.insns[convert].flags |= iflag::SYNTHETIC;
    Ok(convert)
}

fn fix_boolean_usage(cfg: &mut MethodCfg, bound_ref: ArgRef) -> Result<bool, String> {
    let bound_type = cfg.reg(bound_ref).init_type.clone();
    if bound_type == ArgType::boolean()
        || (bound_type.is_type_known() && !bound_type.is_primitive())
    {
        return Ok(false);
    }
    let Some(insn) = bound_ref.parent_insn() else {
        return Ok(false);
    };
    let ArgRef::Arg(_, k) = bound_ref else {
        return Ok(false);
    };
    if cfg.insn(insn).ty == InsnType::If {
        return Ok(false);
    }
    let Some(block) = get_block_by_insn(cfg, insn) else {
        return Ok(false);
    };
    let Some(insn_index) = insn_index_in_block(cfg, block, insn) else {
        return Ok(false);
    };
    let insn_type = cfg.insn(insn).ty;
    if insn_type == InsnType::Cast {
        // replace cast
        let InsnKind::Index(IndexValue::Type(ty)) = cfg.insn(insn).kind.clone() else {
            return Err("cast without type".into());
        };
        let result = cfg.insn(insn).result.clone().ok_or("cast without result")?;
        let convert = prepare_boolean_convert_insn(cfg, result, bound_ref, &ty)?;
        replace_insn(cfg, block, insn_index, convert)?;
        return Ok(true);
    }
    if insn_type == InsnType::Arith {
        let is_xor = matches!(cfg.insn(insn).kind, InsnKind::Arith(ArithOp::Xor));
        if is_xor && cfg.insn(insn).args.len() == 2 {
            // replace (boolean ^ 1) with (!boolean)
            if let InsnArg::Literal(l) = &cfg.insn(insn).args[1] {
                if l.literal == 1 {
                    let convert = not_boolean_to_int(cfg, insn, bound_ref)?;
                    replace_insn(cfg, block, insn_index, convert)?;
                    return Ok(true);
                }
            }
        }
    }

    // insert before insn
    let bound_reg = cfg.reg(bound_ref).clone();
    let (floating, _new_var) = cfg.duplicate_with_new_svar(&bound_reg);
    let result_reg = cfg.reg(floating).clone();
    let convert = prepare_boolean_convert_insn(cfg, result_reg.clone(), bound_ref, &bound_type)?;
    cfg.set_result(convert, Some(result_reg));
    cfg.arena[block].insns.insert(insn_index, convert);
    let new_use = cfg.reg(ArgRef::Result(convert)).clone();
    cfg.replace_arg(insn, k, InsnArg::Register(new_use));
    Ok(true)
}
