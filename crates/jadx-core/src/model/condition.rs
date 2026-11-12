//! `IfCondition`: the (possibly merged) condition of an `if` or loop region.
//!
//! A comparison refers to an `IF` instruction kept in the instruction arena (the reference's
//! `Compare`); inverting a comparison changes that instruction in place, exactly like the
//! reference does.

use super::cfg::MethodCfg;
use super::insn::{
    iflag, ArithOp, IfOp, InsnArg, InsnId, InsnKind, InsnNode, InsnType, LiteralArg,
};
use super::ssa::ArgRef;
use super::types::ArgType;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CondMode {
    Compare,
    Ternary,
    Not,
    And,
    Or,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IfCondition {
    pub mode: CondMode,
    pub args: Vec<IfCondition>,
    /// `IF` instruction of a [`CondMode::Compare`].
    pub compare: Option<InsnId>,
    /// `AFlag.DONT_WRAP`
    pub dont_wrap: bool,
}

/// `IfNode.getOp`
pub fn if_op(cfg: &MethodCfg, insn: InsnId) -> IfOp {
    match &cfg.insn(insn).kind {
        InsnKind::If { op, .. } => *op,
        other => panic!("not an if instruction: {other:?}"),
    }
}

/// `IfNode.invertCondition`
pub fn if_invert(cfg: &mut MethodCfg, insn: InsnId) {
    if let InsnKind::If {
        op,
        then_block,
        else_block,
        ..
    } = &mut cfg.insns[insn].kind
    {
        *op = op.invert();
        std::mem::swap(then_block, else_block);
    }
}

/// `IfNode.changeCondition` for arguments that already sit at their final place.
fn if_set_op(cfg: &mut MethodCfg, insn: InsnId, new_op: IfOp) {
    if let InsnKind::If { op, .. } = &mut cfg.insns[insn].kind {
        *op = new_op;
    }
}

/// `IfNode.normalize`: change `a != false` to `a == true`.
pub fn if_normalize(cfg: &mut MethodCfg, insn: InsnId) {
    if if_op(cfg, insn) == IfOp::Ne && is_false(&cfg.insn(insn).args[1]) {
        if_set_op(cfg, insn, IfOp::Eq);
        cfg.replace_arg(
            insn,
            1,
            InsnArg::Literal(LiteralArg {
                literal: 1,
                ty: ArgType::boolean(),
                explicit_primitive: false,
            }),
        );
    }
}

pub fn is_false(a: &InsnArg) -> bool {
    matches!(a, InsnArg::Literal(l) if l.literal == 0 && l.ty == ArgType::boolean())
}

pub fn is_true(a: &InsnArg) -> bool {
    matches!(a, InsnArg::Literal(l) if l.literal == 1 && l.ty == ArgType::boolean())
}

impl IfCondition {
    fn compare_wrap(insn: InsnId) -> IfCondition {
        IfCondition {
            mode: CondMode::Compare,
            args: Vec::new(),
            compare: Some(insn),
            dont_wrap: false,
        }
    }

    fn with_args(mode: CondMode, args: Vec<IfCondition>) -> IfCondition {
        IfCondition {
            mode,
            args,
            compare: None,
            dont_wrap: false,
        }
    }

    /// `IfCondition.fromIfNode` (the instruction becomes hidden)
    pub fn from_if_node(cfg: &mut MethodCfg, insn: InsnId) -> IfCondition {
        cfg.insns[insn].flags |= iflag::HIDDEN;
        IfCondition::compare_wrap(insn)
    }

    /// `IfCondition.fromIfBlock`
    pub fn from_if_block(cfg: &mut MethodCfg, block: usize) -> Option<IfCondition> {
        let last = *cfg.block(block).insns.last()?;
        Some(IfCondition::from_if_node(cfg, last))
    }

    pub fn ternary(a: IfCondition, b: IfCondition, c: IfCondition) -> IfCondition {
        IfCondition::with_args(CondMode::Ternary, vec![a, b, c])
    }

    /// `IfCondition.merge`
    pub fn merge(mode: CondMode, a: &IfCondition, b: IfCondition) -> IfCondition {
        if a.mode == mode {
            let mut n = a.clone();
            n.args.push(b);
            return n;
        }
        IfCondition::with_args(mode, vec![a.clone(), b])
    }

    pub fn first(&self) -> &IfCondition {
        &self.args[0]
    }
    pub fn second(&self) -> &IfCondition {
        &self.args[1]
    }
    pub fn third(&self) -> &IfCondition {
        &self.args[2]
    }

    pub fn is_compare(&self) -> bool {
        self.mode == CondMode::Compare
    }

    /// `IfCondition.invert`
    pub fn invert(cfg: &mut MethodCfg, cond: &IfCondition) -> IfCondition {
        match cond.mode {
            CondMode::Compare => {
                let insn = cond.compare.expect("compare");
                if_invert(cfg, insn);
                IfCondition::compare_wrap(insn)
            }
            CondMode::Ternary => {
                let b = IfCondition::not(cfg, cond.second());
                let c = IfCondition::not(cfg, cond.third());
                IfCondition::ternary(cond.first().clone(), b, c)
            }
            CondMode::Not => cond.first().clone(),
            CondMode::And | CondMode::Or => {
                let mut new_args = Vec::with_capacity(cond.args.len());
                for a in &cond.args {
                    new_args.push(IfCondition::invert(cfg, a));
                }
                IfCondition::with_args(
                    if cond.mode == CondMode::And {
                        CondMode::Or
                    } else {
                        CondMode::And
                    },
                    new_args,
                )
            }
        }
    }

    /// `IfCondition.not`
    pub fn not(cfg: &mut MethodCfg, cond: &IfCondition) -> IfCondition {
        if cond.mode == CondMode::Not {
            return cond.first().clone();
        }
        if let Some(insn) = cond.compare {
            if_invert(cfg, insn);
            return IfCondition::compare_wrap(insn);
        }
        IfCondition::with_args(CondMode::Not, vec![cond.clone()])
    }

    /// `IfCondition.simplify`; the flag tells that the result is a different object than `cond`.
    pub fn simplify(cfg: &mut MethodCfg, cond: &IfCondition) -> (IfCondition, bool) {
        let mut cond = cond.clone();
        let mut renewed = false;
        if cond.is_compare() {
            let insn = cond.compare.expect("compare");
            if let Some(i) = simplify_cmp_op(cfg, insn) {
                return (i, true);
            }
            if if_op(cfg, insn) == IfOp::Eq && is_false(&cfg.insn(insn).args[1]) {
                if_invert(cfg, insn);
                cond = IfCondition::with_args(CondMode::Not, vec![IfCondition::compare_wrap(insn)]);
                renewed = true;
            } else {
                if_normalize(cfg, insn);
            }
        }
        let mut new_args: Option<Vec<IfCondition>> = None;
        for i in 0..cond.args.len() {
            let arg = cond.args[i].clone();
            let (simpl, changed) = IfCondition::simplify(cfg, &arg);
            if changed {
                new_args.get_or_insert_with(|| cond.args.clone())[i] = simpl;
            }
        }
        if let Some(args) = new_args {
            cond = IfCondition::with_args(cond.mode, args);
            renewed = true;
        }
        if cond.mode == CondMode::Not && cond.first().mode == CondMode::Not {
            let inner = cond.first().clone();
            cond = IfCondition::invert(cfg, &inner);
            renewed = true;
        }
        if cond.mode == CondMode::Ternary && cond.first().mode == CondMode::Not {
            cond = IfCondition::invert(cfg, &cond);
            renewed = true;
        }
        if matches!(cond.mode, CondMode::Or | CondMode::And) {
            let count = cond.args.len();
            if count > 1 {
                let mut neg_count = 0;
                for a in &cond.args {
                    if a.mode == CondMode::Not
                        || (a.is_compare() && if_op(cfg, a.compare.expect("compare")) == IfOp::Ne)
                    {
                        neg_count += 1;
                    }
                }
                if neg_count > count / 2 {
                    let inv = IfCondition::invert(cfg, &cond);
                    return (IfCondition::not(cfg, &inv), true);
                }
            }
        }
        (cond, renewed)
    }

    /// `getRegisterArgs`
    pub fn register_args(&self, cfg: &MethodCfg) -> Vec<ArgRef> {
        match self.compare {
            Some(insn) if self.mode == CondMode::Compare => {
                crate::passes::insn_utils::register_args(cfg, insn)
            }
            _ => self
                .args
                .iter()
                .flat_map(|a| a.register_args(cfg))
                .collect(),
        }
    }

    /// `visitInsns` / `collectInsns`: the compare instructions in order.
    pub fn collect_insns(&self) -> Vec<InsnId> {
        match self.compare {
            Some(insn) if self.mode == CondMode::Compare => vec![insn],
            _ => self.args.iter().flat_map(|a| a.collect_insns()).collect(),
        }
    }

    /// `getFirstInsn`
    pub fn first_insn(&self) -> Option<InsnId> {
        match self.compare {
            Some(insn) if self.mode == CondMode::Compare => Some(insn),
            _ => self.args.first().and_then(|a| a.first_insn()),
        }
    }
}

/// `IfCondition.isFloatCmp`
pub fn is_float_cmp(cfg: &MethodCfg, cmp: InsnId) -> bool {
    let ty = cfg.arg_type(ArgRef::Arg(cmp, 0));
    ty == ArgType::float() || ty == ArgType::double()
}

fn simplify_cmp_op(cfg: &mut MethodCfg, insn: InsnId) -> Option<IfCondition> {
    let (a, b) = {
        let n = cfg.insn(insn);
        (n.args[0].clone(), n.args[1].clone())
    };
    let InsnArg::Wrapped(w) = a else {
        return None;
    };
    let InsnArg::Literal(lit) = &b else {
        return None;
    };
    if lit.literal != 0 && lit.literal != 1 {
        return None;
    }
    let wrap = w.insn;
    match cfg.insn(wrap).ty {
        InsnType::CmpL | InsnType::CmpG => {
            if lit.literal == 0 && !is_float_cmp(cfg, wrap) {
                // IfNode.changeCondition(op, wrap.arg0, wrap.arg1)
                move_arg(cfg, wrap, 0, insn, 0);
                move_arg(cfg, wrap, 1, insn, 1);
            }
        }
        InsnType::Arith if lit.ty == ArgType::boolean() => {
            let InsnKind::Arith(arith_op) = cfg.insn(wrap).kind.clone() else {
                return None;
            };
            if arith_op == ArithOp::Or || arith_op == ArithOp::And {
                let if_op_v = if_op(cfg, insn);
                let is_true = (if_op_v == IfOp::Ne && lit.literal == 0)
                    || (if_op_v == IfOp::Eq && lit.literal == 1);
                let op = if is_true { IfOp::Ne } else { IfOp::Eq };
                let mode = if (is_true && arith_op == ArithOp::Or)
                    || (!is_true && arith_op == ArithOp::And)
                {
                    CondMode::Or
                } else {
                    CondMode::And
                };
                let if1 = new_if(cfg, op, wrap, 0);
                let if2 = new_if(cfg, op, wrap, 1);
                return Some(IfCondition::with_args(
                    mode,
                    vec![
                        IfCondition::from_if_node(cfg, if1),
                        IfCondition::from_if_node(cfg, if2),
                    ],
                ));
            }
        }
        _ => {}
    }
    None
}

/// `new IfNode(op, -1, wrap.arg(idx), LiteralArg.litFalse())`; the argument moves into the new
/// instruction.
fn new_if(cfg: &mut MethodCfg, op: IfOp, wrap: InsnId, idx: usize) -> InsnId {
    let mut n = InsnNode::new(
        InsnType::If,
        InsnKind::If {
            op,
            target: -1,
            then_block: None,
            else_block: None,
        },
    );
    n.args.push(InsnArg::Literal(LiteralArg {
        literal: 0,
        ty: ArgType::boolean(),
        explicit_primitive: false,
    }));
    n.args.push(InsnArg::Literal(LiteralArg {
        literal: 0,
        ty: ArgType::boolean(),
        explicit_primitive: false,
    }));
    let id = cfg.add_insn(n);
    move_arg(cfg, wrap, idx, id, 0);
    id
}

/// Move argument `from_idx` of `from` to position `to_idx` of `to` (keeping its place in the use
/// list of its SSA variable).
pub fn move_arg(cfg: &mut MethodCfg, from: InsnId, from_idx: usize, to: InsnId, to_idx: usize) {
    let arg = cfg.insns[from].args[from_idx].clone();
    if let InsnArg::Register(r) = &arg {
        if let Some(v) = r.svar {
            for u in cfg.svars[v].uses.iter_mut() {
                if *u == ArgRef::Arg(from, from_idx) {
                    *u = ArgRef::Arg(to, to_idx);
                }
            }
        }
    }
    cfg.insns[to].args[to_idx] = arg;
    // the argument object stays in the old instruction and its parent is the new one
    cfg.arg_alias.insert((from, from_idx), (to, to_idx));
}
