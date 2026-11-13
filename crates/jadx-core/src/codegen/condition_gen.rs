//! `ConditionGen`: code for the (merged) conditions of `if` and loop regions and of ternary
//! instructions.

use super::code_writer::CodeWriter;
use super::insn_gen::InsnGen;
use super::CodegenError;
use crate::model::condition::{CondMode, IfCondition};
use crate::model::insn::{ArithOp, IfOp, InsnArg, InsnId, InsnKind, InsnNode, InsnType};
use crate::model::types::ArgType;

type Res<T = ()> = Result<T, CodegenError>;

impl InsnGen<'_, '_> {
    /// `ConditionGen.add`
    pub fn cond_add(&self, code: &mut CodeWriter, cond: &IfCondition) -> Res {
        self.cond_add_at(code, cond, 1)
    }

    /// `ConditionGen.wrap`
    pub fn cond_wrap(&self, code: &mut CodeWriter, cond: &IfCondition) -> Res {
        self.cond_wrap_at(code, cond, 0)
    }

    /// `stack` is the number of conditions on the condition stack including `cond`.
    fn cond_add_at(&self, code: &mut CodeWriter, cond: &IfCondition, stack: usize) -> Res {
        match cond.mode {
            CondMode::Compare => {
                let insn = cond
                    .compare
                    .ok_or_else(|| CodegenError::new("Compare condition without instruction"))?;
                self.add_compare(code, insn, stack)
            }
            CondMode::Ternary => {
                self.cond_add_at(code, cond.first(), stack + 1)?;
                code.add(" ? ");
                self.cond_add_at(code, cond.second(), stack + 1)?;
                code.add(" : ");
                self.cond_add_at(code, cond.third(), stack + 1)
            }
            CondMode::Not => {
                code.add("!");
                self.cond_wrap_at(code, &cond.args[0], stack)
            }
            CondMode::And | CondMode::Or => {
                let op = if cond.mode == CondMode::And {
                    " && "
                } else {
                    " || "
                };
                let n = cond.args.len();
                for (i, a) in cond.args.iter().enumerate() {
                    self.cond_wrap_at(code, a, stack)?;
                    if i + 1 < n {
                        code.add(op);
                    }
                }
                Ok(())
            }
        }
    }

    /// `wrap(code, stack, condition)`: the condition is pushed on the stack by `add`.
    fn cond_wrap_at(&self, code: &mut CodeWriter, cond: &IfCondition, stack: usize) -> Res {
        let wrap = is_wrap_needed(cond);
        if wrap {
            code.add("(");
        }
        self.cond_add_at(code, cond, stack + 1)?;
        if wrap {
            code.add(")");
        }
        Ok(())
    }

    fn wrap_arg(&self, code: &mut CodeWriter, first: &InsnArg) -> Res {
        let wrap = self.is_arg_wrap_needed(first);
        if wrap {
            code.add("(");
        }
        self.add_arg(code, first, false)?;
        if wrap {
            code.add(")");
        }
        Ok(())
    }

    fn add_compare(&self, code: &mut CodeWriter, if_insn: InsnId, stack_size: usize) -> Res {
        let insn = self.insn(if_insn);
        let InsnKind::If { op, .. } = insn.kind else {
            return Err(CodegenError::new("Condition is not an if instruction"));
        };
        let mut op = op;
        let first = arg_of(insn, 0)?;
        let second = arg_of(insn, 1)?;
        if let (InsnArg::Wrapped(w), InsnArg::Literal(l)) = (first, second) {
            let wrap_insn = self.insn(w.insn);
            if l.literal == 0
                && matches!(wrap_insn.ty, InsnType::CmpL | InsnType::CmpG)
                && self.is_float_cmp(wrap_insn)?
            {
                return self.add_float_cmp_compare(code, op, wrap_insn);
            }
        }
        if self.arg_type(first) == ArgType::boolean() {
            if let InsnArg::Literal(l) = second {
                if l.ty == ArgType::boolean() {
                    if l.literal == 0 {
                        op = op.invert();
                    }
                    if op == IfOp::Eq {
                        // == true
                        if stack_size == 1 {
                            self.add_arg(code, first, false)?;
                        } else {
                            self.wrap_arg(code, first)?;
                        }
                        return Ok(());
                    } else if op == IfOp::Ne {
                        // != true
                        code.add("!");
                        return self.wrap_arg(code, first);
                    }
                }
            }
        }
        self.add_arg(code, first, self.is_arg_wrap_needed(first))?;
        code.add(" ").add(op.symbol()).add(" ");
        self.add_arg(code, second, self.is_arg_wrap_needed(second))
    }

    /// Condition on the result of a float comparison: `cmp(a, b) op 0`, as the plain operator on
    /// the compared values. 'cmpg' returns 1 and 'cmpl' returns -1 if any arg is NaN, but
    /// `a < b`, `a <= b`, `a > b` and `a >= b` are all false for NaN, so `!(a inverted_op b)` is
    /// used if the condition must be true for NaN.
    fn add_float_cmp_compare(&self, code: &mut CodeWriter, op: IfOp, cmp: &InsnNode) -> Res {
        let true_for_nan = if cmp.ty == InsnType::CmpG {
            matches!(op, IfOp::Gt | IfOp::Ge)
        } else {
            matches!(op, IfOp::Lt | IfOp::Le)
        };
        let result_op = if true_for_nan { op.invert() } else { op };
        if true_for_nan {
            code.add("!(");
        }
        let first = arg_of(cmp, 0)?;
        let second = arg_of(cmp, 1)?;
        self.add_arg(code, first, self.is_arg_wrap_needed(first))?;
        code.add(" ").add(result_op.symbol()).add(" ");
        self.add_arg(code, second, self.is_arg_wrap_needed(second))?;
        if true_for_nan {
            code.add(")");
        }
        Ok(())
    }

    /// `IfCondition.isFloatCmp`
    fn is_float_cmp(&self, cmp: &InsnNode) -> Res<bool> {
        let ty = self.arg_type(arg_of(cmp, 0)?);
        Ok(ty == ArgType::float() || ty == ArgType::double())
    }

    fn is_arg_wrap_needed(&self, arg: &InsnArg) -> bool {
        let InsnArg::Wrapped(w) = arg else {
            return false;
        };
        let insn = self.insn(w.insn);
        if insn.ty == InsnType::Arith {
            return !matches!(
                insn.kind,
                InsnKind::Arith(
                    ArithOp::Add | ArithOp::Sub | ArithOp::Mul | ArithOp::Div | ArithOp::Rem
                )
            );
        }
        !matches!(
            insn.ty,
            InsnType::Invoke
                | InsnType::Sget
                | InsnType::Iget
                | InsnType::Aget
                | InsnType::Const
                | InsnType::ArrayLength
        )
    }
}

fn arg_of(insn: &InsnNode, n: usize) -> Res<&InsnArg> {
    insn.args
        .get(n)
        .ok_or_else(|| CodegenError::new(format!("Missing argument {n} of {}", insn.ty)))
}

fn is_wrap_needed(cond: &IfCondition) -> bool {
    if cond.is_compare() || cond.dont_wrap {
        return false;
    }
    cond.mode != CondMode::Not
}
