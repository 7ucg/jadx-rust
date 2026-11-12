//! `InsnDecoder`: converts the normalised input instructions into [`InsnNode`]s.

use jadx_input_api::{InsnData, InsnResolver, Opcode, Payload};

use crate::model::info::{ClassInfo, FieldInfo, MethodInfo};
use crate::model::insn::{
    ArithOp, FillArrayData, IfOp, IndexValue, InsnArg, InsnKind, InsnNode, InsnType, InvokeType,
    RegisterArg, SwitchData,
};
use crate::model::types::{ArgType, PrimitiveType, CLASS_CLASS};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DecodeError {
    #[error("{0}")]
    Msg(String),
}

fn err<T>(msg: impl Into<String>) -> Result<T, DecodeError> {
    Err(DecodeError::Msg(msg.into()))
}

/// What the decoder needs to know about the method being decoded.
#[derive(Debug, Clone)]
pub struct MethodCtx {
    pub return_type: ArgType,
    pub parent_class: ClassInfo,
    pub root: std::sync::Arc<crate::model::node::Root>,
}

/// Decoded instruction array indexed by code-unit offset (`None` at non-instruction offsets).
pub struct DecodedMethod {
    pub insns: Vec<Option<InsnNode>>,
    /// Non-fatal decode errors (the failed instruction is replaced by a `NOP`).
    pub errors: Vec<String>,
}

pub struct InsnDecoder<'a> {
    resolver: &'a dyn InsnResolver,
    ctx: &'a MethodCtx,
}

impl<'a> InsnDecoder<'a> {
    pub fn new(resolver: &'a dyn InsnResolver, ctx: &'a MethodCtx) -> Self {
        InsnDecoder { resolver, ctx }
    }

    /// Decode all instructions. A second failing instruction aborts the method.
    pub fn process(
        &self,
        raw: &[InsnData],
        units_count: usize,
    ) -> Result<DecodedMethod, DecodeError> {
        let mut insns: Vec<Option<InsnNode>> = vec![None; units_count];
        let mut errors: Vec<String> = Vec::new();
        for r in raw {
            let node = match self.decode(r) {
                Ok(n) => n,
                Err(e) => {
                    if !errors.is_empty() {
                        return err(format!("Failed to decode insn: {:?} ({e})", r.opcode));
                    }
                    errors.push(format!("Failed to decode insn: {:?}: {e}", r.opcode));
                    let mut nop = InsnNode::plain(InsnType::Nop);
                    nop.error = Some(format!("decode failed: {e}"));
                    nop
                }
            };
            let mut node = node;
            node.offset = r.offset as i32;
            if let Some(slot) = insns.get_mut(r.offset as usize) {
                *slot = Some(node);
            }
        }
        Ok(DecodedMethod { insns, errors })
    }

    fn class_info(&self, desc: &str) -> Result<ClassInfo, DecodeError> {
        let ty = ArgType::parse(desc).map_err(|e| DecodeError::Msg(e.to_string()))?;
        self.ctx.root.ctx.class_info(&ty).map_err(DecodeError::Msg)
    }

    fn parse_type(&self, desc: &str) -> Result<ArgType, DecodeError> {
        ArgType::parse(desc).map_err(|e| DecodeError::Msg(e.to_string()))
    }

    fn field_info(&self, idx: u32) -> Result<FieldInfo, DecodeError> {
        let f = self.resolver.field(idx).map_err(DecodeError::Msg)?;
        Ok(FieldInfo {
            decl_class: self.class_info(&f.parent_class)?,
            name: f.name,
            ty: self.parse_type(&f.ty)?,
        })
    }

    /// Declared type of the field if it is defined in the input (may carry generics).
    fn resolved_field_type(&self, fld: &FieldInfo) -> ArgType {
        match self.ctx.root.resolve_field(fld) {
            Some(node) => node.ty.clone(),
            None => fld.ty.clone(),
        }
    }

    fn method_info(&self, idx: u32) -> Result<MethodInfo, DecodeError> {
        let m = self.resolver.method(idx).map_err(DecodeError::Msg)?;
        let mut args = Vec::with_capacity(m.arg_types.len());
        for a in &m.arg_types {
            args.push(self.parse_type(a)?);
        }
        Ok(MethodInfo {
            decl_class: self.class_info(&m.parent_class)?,
            name: m.name,
            arg_types: args,
            return_type: self.parse_type(&m.return_type)?,
        })
    }

    fn index_type(&self, insn: &InsnData) -> Result<ArgType, DecodeError> {
        let d = self
            .resolver
            .type_desc(insn.index)
            .map_err(DecodeError::Msg)?;
        self.parse_type(&d)
    }

    fn decode(&self, insn: &InsnData) -> Result<InsnNode, DecodeError> {
        use Opcode as O;
        let reg = |i: usize| -> Result<u32, DecodeError> {
            insn.regs.get(i).copied().ok_or_else(|| {
                DecodeError::Msg(format!("missing register #{i} in {:?}", insn.opcode))
            })
        };
        let reg_arg = |i: usize, ty: ArgType| -> Result<RegisterArg, DecodeError> {
            Ok(RegisterArg::new(reg(i)?, ty))
        };
        let arg = |i: usize, ty: ArgType| -> Result<InsnArg, DecodeError> {
            Ok(InsnArg::reg(reg(i)?, ty))
        };
        let imm_arg = |i: usize, ty: ArgType| -> Result<InsnArg, DecodeError> {
            Ok(InsnArg::immutable_reg(reg(i)?, ty))
        };
        let one = |ty: InsnType, res: Option<RegisterArg>, a: InsnArg| {
            let mut n = InsnNode::plain(ty).with_arg(a);
            n.result = res;
            n
        };
        let node = match insn.opcode {
            O::Nop => InsnNode::plain(InsnType::Nop),
            O::MoveResult => {
                let mut n = InsnNode::plain(InsnType::MoveResult);
                n.result = Some(reg_arg(0, ArgType::unknown_any())?);
                n
            }
            O::Const => {
                let lit = InsnArg::lit(insn.literal, ArgType::narrow());
                let ty = lit_type(&lit);
                one(InsnType::Const, Some(reg_arg(0, ty)?), lit)
            }
            O::ConstWide => {
                let lit = InsnArg::lit(insn.literal, ArgType::wide());
                let ty = lit_type(&lit);
                one(InsnType::Const, Some(reg_arg(0, ty)?), lit)
            }
            O::ConstString => {
                let s = self
                    .resolver
                    .string_utf16(insn.index)
                    .map_err(DecodeError::Msg)?;
                let mut n = InsnNode::new(InsnType::ConstStr, InsnKind::ConstStr(s));
                n.result = Some(reg_arg(0, ArgType::string())?);
                n
            }
            O::ConstClass => {
                let cls_type = self.index_type(insn)?;
                let mut n =
                    InsnNode::new(InsnType::ConstClass, InsnKind::ConstClass(cls_type.clone()));
                n.result = Some(reg_arg(
                    0,
                    ArgType::generic_named(CLASS_CLASS, vec![cls_type]),
                )?);
                n
            }
            O::Move => {
                let mut n = InsnNode::plain(InsnType::Move);
                n.result = Some(reg_arg(0, ArgType::narrow())?);
                n.args.push(arg(1, ArgType::narrow())?);
                n
            }
            O::MoveMulti => {
                let mut n = InsnNode::plain(InsnType::MoveMulti);
                for i in 0..insn.regs.len() {
                    n.args.push(arg(i, ArgType::unknown_any())?);
                }
                n
            }
            O::MoveWide => {
                let mut n = InsnNode::plain(InsnType::Move);
                n.result = Some(reg_arg(0, ArgType::wide())?);
                n.args.push(arg(1, ArgType::wide())?);
                n
            }
            O::MoveObject => {
                let mut n = InsnNode::plain(InsnType::Move);
                n.result = Some(reg_arg(0, ArgType::unknown_object())?);
                n.args.push(arg(1, ArgType::unknown_object())?);
                n
            }
            O::AddInt => self.arith(insn, ArithOp::Add, ArgType::int())?,
            O::AddDouble => self.arith(insn, ArithOp::Add, ArgType::double())?,
            O::AddFloat => self.arith(insn, ArithOp::Add, ArgType::float())?,
            O::AddLong => self.arith(insn, ArithOp::Add, ArgType::long())?,
            O::AddIntLit => self.arith_lit(insn, ArithOp::Add, ArgType::int())?,
            O::SubInt => self.arith(insn, ArithOp::Sub, ArgType::int())?,
            O::RsubInt => {
                let mut n = InsnNode::new(InsnType::Arith, InsnKind::Arith(ArithOp::Sub));
                n.result = Some(reg_arg(0, ArgType::int())?);
                n.args.push(InsnArg::lit(insn.literal, ArgType::int()));
                n.args.push(arg(1, ArgType::int())?);
                n
            }
            O::SubLong => self.arith(insn, ArithOp::Sub, ArgType::long())?,
            O::SubFloat => self.arith(insn, ArithOp::Sub, ArgType::float())?,
            O::SubDouble => self.arith(insn, ArithOp::Sub, ArgType::double())?,
            O::MulInt => self.arith(insn, ArithOp::Mul, ArgType::int())?,
            O::MulDouble => self.arith(insn, ArithOp::Mul, ArgType::double())?,
            O::MulFloat => self.arith(insn, ArithOp::Mul, ArgType::float())?,
            O::MulLong => self.arith(insn, ArithOp::Mul, ArgType::long())?,
            O::MulIntLit => self.arith_lit(insn, ArithOp::Mul, ArgType::int())?,
            O::DivInt => self.arith(insn, ArithOp::Div, ArgType::int())?,
            O::RemInt => self.arith(insn, ArithOp::Rem, ArgType::int())?,
            O::RemLong => self.arith(insn, ArithOp::Rem, ArgType::long())?,
            O::RemFloat => self.arith(insn, ArithOp::Rem, ArgType::float())?,
            O::RemDouble => self.arith(insn, ArithOp::Rem, ArgType::double())?,
            O::DivDouble => self.arith(insn, ArithOp::Div, ArgType::double())?,
            O::DivFloat => self.arith(insn, ArithOp::Div, ArgType::float())?,
            O::DivLong => self.arith(insn, ArithOp::Div, ArgType::long())?,
            O::DivIntLit => self.arith_lit(insn, ArithOp::Div, ArgType::int())?,
            O::RemIntLit => self.arith_lit(insn, ArithOp::Rem, ArgType::int())?,
            O::AndInt => self.arith(insn, ArithOp::And, ArgType::int())?,
            O::AndIntLit => self.arith_lit(insn, ArithOp::And, ArgType::int())?,
            O::XorIntLit => self.arith_lit(insn, ArithOp::Xor, ArgType::int())?,
            O::AndLong => self.arith(insn, ArithOp::And, ArgType::long())?,
            O::OrInt => self.arith(insn, ArithOp::Or, ArgType::int())?,
            O::OrIntLit => self.arith_lit(insn, ArithOp::Or, ArgType::int())?,
            O::XorInt => self.arith(insn, ArithOp::Xor, ArgType::int())?,
            O::OrLong => self.arith(insn, ArithOp::Or, ArgType::long())?,
            O::XorLong => self.arith(insn, ArithOp::Xor, ArgType::long())?,
            O::UshrInt => self.arith(insn, ArithOp::Ushr, ArgType::int())?,
            O::UshrLong => self.arith(insn, ArithOp::Ushr, ArgType::long())?,
            O::ShlInt => self.arith(insn, ArithOp::Shl, ArgType::int())?,
            O::ShlLong => self.arith(insn, ArithOp::Shl, ArgType::long())?,
            O::ShrInt => self.arith(insn, ArithOp::Shr, ArgType::int())?,
            O::ShrLong => self.arith(insn, ArithOp::Shr, ArgType::long())?,
            O::ShlIntLit => self.arith_lit(insn, ArithOp::Shl, ArgType::int())?,
            O::ShrIntLit => self.arith_lit(insn, ArithOp::Shr, ArgType::int())?,
            O::UshrIntLit => self.arith_lit(insn, ArithOp::Ushr, ArgType::int())?,
            O::NegInt => self.unary(insn, InsnType::Neg, ArgType::int())?,
            O::NegLong => self.unary(insn, InsnType::Neg, ArgType::long())?,
            O::NegFloat => self.unary(insn, InsnType::Neg, ArgType::float())?,
            O::NegDouble => self.unary(insn, InsnType::Neg, ArgType::double())?,
            O::NotInt => self.unary(insn, InsnType::Not, ArgType::int())?,
            O::NotLong => self.unary(insn, InsnType::Not, ArgType::long())?,
            O::IntToByte => self.cast(insn, ArgType::int(), ArgType::byte())?,
            O::IntToChar => self.cast(insn, ArgType::int(), ArgType::char())?,
            O::IntToShort => self.cast(insn, ArgType::int(), ArgType::short())?,
            O::IntToFloat => self.cast(insn, ArgType::int(), ArgType::float())?,
            O::IntToDouble => self.cast(insn, ArgType::int(), ArgType::double())?,
            O::IntToLong => self.cast(insn, ArgType::int(), ArgType::long())?,
            O::FloatToInt => self.cast(insn, ArgType::float(), ArgType::int())?,
            O::FloatToDouble => self.cast(insn, ArgType::float(), ArgType::double())?,
            O::FloatToLong => self.cast(insn, ArgType::float(), ArgType::long())?,
            O::DoubleToInt => self.cast(insn, ArgType::double(), ArgType::int())?,
            O::DoubleToFloat => self.cast(insn, ArgType::double(), ArgType::float())?,
            O::DoubleToLong => self.cast(insn, ArgType::double(), ArgType::long())?,
            O::LongToInt => self.cast(insn, ArgType::long(), ArgType::int())?,
            O::LongToFloat => self.cast(insn, ArgType::long(), ArgType::float())?,
            O::LongToDouble => self.cast(insn, ArgType::long(), ArgType::double())?,
            O::IfEq | O::IfEqz => self.if_node(insn, IfOp::Eq)?,
            O::IfNe | O::IfNez => self.if_node(insn, IfOp::Ne)?,
            O::IfGt | O::IfGtz => self.if_node(insn, IfOp::Gt)?,
            O::IfGe | O::IfGez => self.if_node(insn, IfOp::Ge)?,
            O::IfLt | O::IfLtz => self.if_node(insn, IfOp::Lt)?,
            O::IfLe | O::IfLez => self.if_node(insn, IfOp::Le)?,
            O::CmpLong => self.cmp(insn, InsnType::CmpL, ArgType::long())?,
            O::CmplFloat => self.cmp(insn, InsnType::CmpL, ArgType::float())?,
            O::CmplDouble => self.cmp(insn, InsnType::CmpL, ArgType::double())?,
            O::CmpgFloat => self.cmp(insn, InsnType::CmpG, ArgType::float())?,
            O::CmpgDouble => self.cmp(insn, InsnType::CmpG, ArgType::double())?,
            O::Goto => InsnNode::new(
                InsnType::Goto,
                InsnKind::Goto {
                    target: insn.target as i32,
                },
            ),
            O::Throw => one(InsnType::Throw, None, arg(0, ArgType::throwable())?),
            O::MoveException => {
                let mut n = InsnNode::plain(InsnType::MoveException);
                n.result = Some(reg_arg(0, ArgType::unknown_object_no_array())?);
                n
            }
            O::ReturnVoid => InsnNode::plain(InsnType::Return),
            O::Return => one(
                InsnType::Return,
                None,
                arg(0, self.ctx.return_type.clone())?,
            ),
            O::InstanceOf => {
                let ty = self.index_type(insn)?;
                let mut n =
                    InsnNode::new(InsnType::InstanceOf, InsnKind::Index(IndexValue::Type(ty)));
                n.result = Some(reg_arg(0, ArgType::boolean())?);
                n.args.push(arg(1, ArgType::unknown_object())?);
                n
            }
            O::CheckCast => {
                let ty = self.index_type(insn)?;
                let mut n = InsnNode::new(
                    InsnType::CheckCast,
                    InsnKind::Index(IndexValue::Type(ty.clone())),
                );
                n.result = Some(reg_arg(0, ty)?);
                n.args.push(arg(
                    if insn.regs.len() == 2 { 1 } else { 0 },
                    ArgType::unknown_object(),
                )?);
                n
            }
            O::Iget => {
                let fld = self.field_info(insn.index)?;
                let decl_type = fld.decl_class.arg_type().clone();
                let ty = self.resolved_field_type(&fld);
                let mut n = InsnNode::new(InsnType::Iget, InsnKind::Index(IndexValue::Field(fld)));
                n.result = Some(reg_arg(0, ty)?);
                n.args.push(arg(1, decl_type)?);
                n
            }
            O::Iput => {
                let fld = self.field_info(insn.index)?;
                let decl_type = fld.decl_class.arg_type().clone();
                let ty = self.resolved_field_type(&fld);
                let mut n = InsnNode::new(InsnType::Iput, InsnKind::Index(IndexValue::Field(fld)));
                n.args.push(arg(0, ty)?);
                n.args.push(arg(1, decl_type)?);
                n
            }
            O::Sget => {
                let fld = self.field_info(insn.index)?;
                let ty = self.resolved_field_type(&fld);
                let mut n = InsnNode::new(InsnType::Sget, InsnKind::Index(IndexValue::Field(fld)));
                n.result = Some(reg_arg(0, ty)?);
                n
            }
            O::Sput => {
                let fld = self.field_info(insn.index)?;
                let ty = self.resolved_field_type(&fld);
                let mut n = InsnNode::new(InsnType::Sput, InsnKind::Index(IndexValue::Field(fld)));
                n.args.push(arg(0, ty)?);
                n
            }
            O::ArrayLength => {
                let mut n = InsnNode::plain(InsnType::ArrayLength);
                n.result = Some(reg_arg(0, ArgType::int())?);
                n.args.push(arg(1, ArgType::array(ArgType::unknown_any()))?);
                n
            }
            O::Aget => self.array_get(
                insn,
                ArgType::int_float(),
                ArgType::narrow_numbers_no_bool(),
            )?,
            O::AgetBoolean => self.array_get(insn, ArgType::boolean(), ArgType::boolean())?,
            O::AgetByte => self.array_get(insn, ArgType::byte(), ArgType::narrow_integral())?,
            O::AgetByteBoolean => {
                self.array_get(insn, ArgType::byte_boolean(), ArgType::byte_boolean())?
            }
            O::AgetChar => self.array_get(insn, ArgType::char(), ArgType::char())?,
            O::AgetShort => self.array_get(insn, ArgType::short(), ArgType::short())?,
            O::AgetWide => self.array_get(insn, ArgType::wide(), ArgType::wide())?,
            O::AgetObject => {
                self.array_get(insn, ArgType::unknown_object(), ArgType::unknown_object())?
            }
            O::Aput => self.array_put(
                insn,
                ArgType::int_float(),
                ArgType::narrow_numbers_no_bool(),
            )?,
            O::AputBoolean => self.array_put(insn, ArgType::boolean(), ArgType::boolean())?,
            O::AputByte => self.array_put(insn, ArgType::byte(), ArgType::byte())?,
            O::AputByteBoolean => {
                self.array_put(insn, ArgType::byte_boolean(), ArgType::byte_boolean())?
            }
            O::AputChar => self.array_put(insn, ArgType::char(), ArgType::char())?,
            O::AputShort => self.array_put(insn, ArgType::short(), ArgType::short())?,
            O::AputWide => self.array_put(insn, ArgType::wide(), ArgType::wide())?,
            O::AputObject => {
                self.array_put(insn, ArgType::unknown_object(), ArgType::unknown_object())?
            }
            O::InvokeStatic => self.invoke(insn, InvokeType::Static, false)?,
            O::InvokeStaticRange => self.invoke(insn, InvokeType::Static, true)?,
            O::InvokeDirect => self.invoke(insn, InvokeType::Direct, false)?,
            O::InvokeInterface => self.invoke(insn, InvokeType::Interface, false)?,
            O::InvokeSuper => self.invoke(insn, InvokeType::Super, false)?,
            O::InvokeVirtual => self.invoke(insn, InvokeType::Virtual, false)?,
            O::InvokeDirectRange => self.invoke(insn, InvokeType::Direct, true)?,
            O::InvokeInterfaceRange => self.invoke(insn, InvokeType::Interface, true)?,
            O::InvokeSuperRange => self.invoke(insn, InvokeType::Super, true)?,
            O::InvokeVirtualRange => self.invoke(insn, InvokeType::Virtual, true)?,
            O::NewInstance => {
                let ty = self.index_type(insn)?;
                let mut n = InsnNode::new(
                    InsnType::NewInstance,
                    InsnKind::Index(IndexValue::Type(ty.clone())),
                );
                n.result = Some(reg_arg(0, ty)?);
                n
            }
            O::NewArray => {
                let index_type = self.index_type(insn)?;
                let dim = insn.literal as i32;
                let arr_type = if dim == 0 {
                    index_type
                } else {
                    ArgType::array_dim(index_type, dim as u32)
                };
                let mut n = InsnNode::new(InsnType::NewArray, InsnKind::NewArray(arr_type.clone()));
                n.result = Some(reg_arg(0, arr_type)?);
                for i in 1..insn.regs.len() {
                    n.args.push(imm_arg(i, ArgType::int())?);
                }
                n
            }
            O::FillArrayData => {
                let mut n = InsnNode::new(
                    InsnType::FillArray,
                    InsnKind::FillArray {
                        target: insn.target as i32,
                        data: None,
                    },
                );
                n.args.push(arg(0, ArgType::unknown_array())?);
                n
            }
            O::FillArrayDataPayload => {
                let Some(Payload::Array(p)) = &insn.payload else {
                    return err("array payload expected");
                };
                let elem_type = match p.element_size {
                    0 | 1 => ArgType::unknown(&[PrimitiveType::Byte, PrimitiveType::Boolean]),
                    2 => ArgType::unknown(&[PrimitiveType::Short, PrimitiveType::Char]),
                    4 => ArgType::unknown(&[PrimitiveType::Int, PrimitiveType::Float]),
                    8 => ArgType::unknown(&[PrimitiveType::Long, PrimitiveType::Double]),
                    w => return err(format!("Unknown array element width: {w}")),
                };
                InsnNode::new(
                    InsnType::FillArrayData,
                    InsnKind::FillArrayData(FillArrayData {
                        size: p.size,
                        element_size: p.element_size,
                        elem_type,
                        data: p.data.clone(),
                    }),
                )
            }
            O::FilledNewArray => self.filled_new_array(insn, false)?,
            O::FilledNewArrayRange => self.filled_new_array(insn, true)?,
            O::PackedSwitch => self.switch(insn, true)?,
            O::SparseSwitch => self.switch(insn, false)?,
            O::PackedSwitchPayload | O::SparseSwitchPayload => {
                let Some(Payload::Switch(p)) = &insn.payload else {
                    return err("switch payload expected");
                };
                InsnNode::new(
                    InsnType::SwitchData,
                    InsnKind::SwitchData(SwitchData {
                        keys: p.keys.clone(),
                        targets: p.targets.clone(),
                    }),
                )
            }
            O::MonitorEnter => one(
                InsnType::MonitorEnter,
                None,
                arg(0, ArgType::unknown_object())?,
            ),
            O::MonitorExit => one(
                InsnType::MonitorExit,
                None,
                arg(0, ArgType::unknown_object())?,
            ),
            O::InvokeCustom
            | O::InvokeCustomRange
            | O::InvokePolymorphic
            | O::InvokePolymorphicRange
            | O::ConstMethodHandle
            | O::ConstMethodType
            | O::InvokeSpecial
            | O::JavaJsr
            | O::JavaRet => {
                return err(format!(
                    "Unsupported instruction (not yet ported): {:?}",
                    insn.opcode
                ))
            }
            O::Unknown | O::Arith | O::Cast | O::If | O::Neg => {
                return err(format!("Unknown instruction: '{:?}'", insn.opcode))
            }
        };
        Ok(node)
    }

    fn arith(&self, insn: &InsnData, op: ArithOp, ty: ArgType) -> Result<InsnNode, DecodeError> {
        let reg = |i: usize| {
            insn.regs
                .get(i)
                .copied()
                .ok_or_else(|| DecodeError::Msg("missing register".into()))
        };
        let res = RegisterArg::new(reg(0)?, fix_result_type(op, &ty));
        let arg_ty = fix_arg_type(op, &ty);
        let (a, b) = match insn.regs.len() {
            2 => (reg(0)?, reg(1)?),
            3 => (reg(1)?, reg(2)?),
            _ => return err(format!("Unexpected registers count in {:?}", insn.opcode)),
        };
        let mut n = InsnNode::new(InsnType::Arith, InsnKind::Arith(op));
        n.result = Some(res);
        n.args.push(InsnArg::reg(a, arg_ty.clone()));
        n.args.push(InsnArg::reg(b, arg_ty));
        Ok(n)
    }

    fn arith_lit(
        &self,
        insn: &InsnData,
        op: ArithOp,
        ty: ArgType,
    ) -> Result<InsnNode, DecodeError> {
        let reg = |i: usize| {
            insn.regs
                .get(i)
                .copied()
                .ok_or_else(|| DecodeError::Msg("missing register".into()))
        };
        let res = RegisterArg::new(reg(0)?, fix_result_type(op, &ty));
        let arg_ty = fix_arg_type(op, &ty);
        let lit = InsnArg::lit(insn.literal, arg_ty.clone());
        let a = match insn.regs.len() {
            1 => reg(0)?,
            2 => reg(1)?,
            _ => return err(format!("Unexpected registers count in {:?}", insn.opcode)),
        };
        let mut n = InsnNode::new(InsnType::Arith, InsnKind::Arith(op));
        n.result = Some(res);
        n.args.push(InsnArg::reg(a, arg_ty));
        n.args.push(lit);
        Ok(n)
    }

    fn unary(&self, insn: &InsnData, ty: InsnType, t: ArgType) -> Result<InsnNode, DecodeError> {
        let (r0, r1) = two_regs(insn)?;
        let mut n = InsnNode::plain(ty);
        n.result = Some(RegisterArg::new(r0, t.clone()));
        n.args.push(InsnArg::reg(r1, t));
        Ok(n)
    }

    fn cast(&self, insn: &InsnData, from: ArgType, to: ArgType) -> Result<InsnNode, DecodeError> {
        let (r0, r1) = two_regs(insn)?;
        let mut n = InsnNode::new(
            InsnType::Cast,
            InsnKind::Index(IndexValue::Type(to.clone())),
        );
        n.result = Some(RegisterArg::new(r0, to));
        n.args.push(InsnArg::reg(r1, from));
        Ok(n)
    }

    fn cmp(&self, insn: &InsnData, ty: InsnType, t: ArgType) -> Result<InsnNode, DecodeError> {
        if insn.regs.len() < 3 {
            return err("missing registers");
        }
        let mut n = InsnNode::plain(ty);
        n.result = Some(RegisterArg::new(insn.regs[0], ArgType::int()));
        n.args.push(InsnArg::reg(insn.regs[1], t.clone()));
        n.args.push(InsnArg::reg(insn.regs[2], t));
        Ok(n)
    }

    fn if_node(&self, insn: &InsnData, op: IfOp) -> Result<InsnNode, DecodeError> {
        let arg_ty = if matches!(op, IfOp::Eq | IfOp::Ne) {
            ArgType::unknown(&[
                PrimitiveType::Int,
                PrimitiveType::Boolean,
                PrimitiveType::Object,
                PrimitiveType::Array,
                PrimitiveType::Byte,
                PrimitiveType::Short,
                PrimitiveType::Char,
            ])
        } else {
            ArgType::unknown(&[
                PrimitiveType::Int,
                PrimitiveType::Byte,
                PrimitiveType::Short,
                PrimitiveType::Char,
            ])
        };
        let r0 = *insn
            .regs
            .first()
            .ok_or_else(|| DecodeError::Msg("missing register".into()))?;
        let mut n = InsnNode::new(
            InsnType::If,
            InsnKind::If {
                op,
                target: insn.target as i32,
                then_block: None,
                else_block: None,
            },
        );
        n.args.push(InsnArg::reg(r0, arg_ty.clone()));
        if insn.regs.len() == 1 {
            n.args.push(InsnArg::lit(0, arg_ty));
        } else {
            n.args.push(InsnArg::reg(insn.regs[1], arg_ty));
        }
        Ok(n)
    }

    fn array_get(
        &self,
        insn: &InsnData,
        elem: ArgType,
        res: ArgType,
    ) -> Result<InsnNode, DecodeError> {
        if insn.regs.len() < 3 {
            return err("missing registers");
        }
        let mut n = InsnNode::plain(InsnType::Aget);
        n.result = Some(match InsnArg::immutable_if_known_reg(insn.regs[0], res) {
            InsnArg::Register(r) => r,
            _ => unreachable!(),
        });
        n.args.push(InsnArg::immutable_if_known_reg(
            insn.regs[1],
            ArgType::array(elem),
        ));
        n.args
            .push(InsnArg::reg(insn.regs[2], ArgType::narrow_integral()));
        Ok(n)
    }

    fn array_put(
        &self,
        insn: &InsnData,
        elem: ArgType,
        arg_ty: ArgType,
    ) -> Result<InsnNode, DecodeError> {
        if insn.regs.len() < 3 {
            return err("missing registers");
        }
        let mut n = InsnNode::plain(InsnType::Aput);
        n.args.push(InsnArg::immutable_if_known_reg(
            insn.regs[1],
            ArgType::array(elem),
        ));
        n.args
            .push(InsnArg::reg(insn.regs[2], ArgType::narrow_integral()));
        n.args
            .push(InsnArg::immutable_if_known_reg(insn.regs[0], arg_ty));
        Ok(n)
    }

    fn invoke(
        &self,
        insn: &InsnData,
        ty: InvokeType,
        is_range: bool,
    ) -> Result<InsnNode, DecodeError> {
        let mth = self.method_info(insn.index)?;
        let instance_call = ty != InvokeType::Static;
        let mut n = InsnNode::new(
            InsnType::Invoke,
            InsnKind::Invoke {
                invoke_type: ty,
                mth: mth.clone(),
            },
        );
        let reg_at = |i: usize| -> Result<u32, DecodeError> {
            insn.regs
                .get(i)
                .copied()
                .ok_or_else(|| DecodeError::Msg("missing invoke register".into()))
        };
        let mut k: u32 = if is_range { reg_at(0)? } else { 0 };
        if instance_call {
            let r = if is_range { k } else { reg_at(k as usize)? };
            n.args
                .push(InsnArg::reg(r, mth.decl_class.arg_type().clone()));
            k += 1;
        }
        for a in &mth.arg_types {
            let r = if is_range { k } else { reg_at(k as usize)? };
            n.args.push(InsnArg::reg(r, a.clone()));
            k += a.reg_count();
        }
        Ok(n)
    }

    fn filled_new_array(&self, insn: &InsnData, is_range: bool) -> Result<InsnNode, DecodeError> {
        let arr_type = self.index_type(insn)?;
        let elem_type = arr_type.array_element().cloned().ok_or_else(|| {
            DecodeError::Msg(format!("filled-new-array with non array type: {arr_type}"))
        })?;
        let immutable = elem_type.is_primitive();
        let mut n = InsnNode::new(
            InsnType::FilledNewArray,
            InsnKind::FilledNewArray(elem_type.clone()),
        );
        let count = insn.regs.len();
        if is_range {
            let start = *insn.regs.first().unwrap_or(&0);
            for r in start..start + count as u32 {
                n.args.push(InsnArg::Register(RegisterArg {
                    immutable_type: immutable,
                    ..RegisterArg::new(r, elem_type.clone())
                }));
            }
        } else {
            for &r in &insn.regs {
                n.args.push(InsnArg::Register(RegisterArg {
                    immutable_type: immutable,
                    ..RegisterArg::new(r, elem_type.clone())
                }));
            }
        }
        Ok(n)
    }

    fn switch(&self, insn: &InsnData, packed: bool) -> Result<InsnNode, DecodeError> {
        let r0 = *insn
            .regs
            .first()
            .ok_or_else(|| DecodeError::Msg("missing register".into()))?;
        let mut n = InsnNode::new(
            InsnType::Switch,
            InsnKind::Switch {
                data_target: insn.target as i32,
                packed,
                data: None,
                def: -1,
                target_blocks: None,
                def_block: None,
            },
        );
        n.args.push(InsnArg::reg(r0, ArgType::narrow_integral()));
        Ok(n)
    }
}

fn two_regs(insn: &InsnData) -> Result<(u32, u32), DecodeError> {
    match insn.regs.as_slice() {
        [a, b, ..] => Ok((*a, *b)),
        _ => err(format!("missing registers in {:?}", insn.opcode)),
    }
}

fn lit_type(arg: &InsnArg) -> ArgType {
    match arg {
        InsnArg::Literal(l) => l.ty.clone(),
        InsnArg::Register(r) => r.init_type.clone(),
    }
}

fn fix_result_type(op: ArithOp, ty: &ArgType) -> ArgType {
    if *ty == ArgType::int() && op.is_bit_op() {
        return ArgType::int_boolean();
    }
    ty.clone()
}

fn fix_arg_type(op: ArithOp, ty: &ArgType) -> ArgType {
    if *ty == ArgType::int() && op.is_bit_op() {
        return ArgType::narrow_numbers_no_float();
    }
    ty.clone()
}
