//! Instruction nodes (`InsnNode`) and their arguments.

use std::fmt;

use super::info::{FieldInfo, MethodInfo};
use super::types::{ArgType, PrimitiveType};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InsnType {
    Const,
    ConstStr,
    ConstClass,
    Arith,
    Neg,
    Not,
    Move,
    MoveMulti,
    Cast,
    Return,
    Goto,
    Throw,
    MoveException,
    CmpL,
    CmpG,
    If,
    Switch,
    SwitchData,
    MonitorEnter,
    MonitorExit,
    CheckCast,
    InstanceOf,
    ArrayLength,
    FillArray,
    FillArrayData,
    FilledNewArray,
    Aget,
    Aput,
    NewArray,
    NewInstance,
    Iget,
    Iput,
    Sget,
    Sput,
    Invoke,
    MoveResult,
    Nop,
    Ternary,
    Constructor,
    Break,
    Continue,
    StrConcat,
    OneArg,
    Phi,
    RegionArg,
    JavaJsr,
    JavaRet,
}

impl InsnType {
    /// Name as printed by the reference (`CONST_STR`).
    pub fn name(self) -> &'static str {
        use InsnType::*;
        match self {
            Const => "CONST",
            ConstStr => "CONST_STR",
            ConstClass => "CONST_CLASS",
            Arith => "ARITH",
            Neg => "NEG",
            Not => "NOT",
            Move => "MOVE",
            MoveMulti => "MOVE_MULTI",
            Cast => "CAST",
            Return => "RETURN",
            Goto => "GOTO",
            Throw => "THROW",
            MoveException => "MOVE_EXCEPTION",
            CmpL => "CMP_L",
            CmpG => "CMP_G",
            If => "IF",
            Switch => "SWITCH",
            SwitchData => "SWITCH_DATA",
            MonitorEnter => "MONITOR_ENTER",
            MonitorExit => "MONITOR_EXIT",
            CheckCast => "CHECK_CAST",
            InstanceOf => "INSTANCE_OF",
            ArrayLength => "ARRAY_LENGTH",
            FillArray => "FILL_ARRAY",
            FillArrayData => "FILL_ARRAY_DATA",
            FilledNewArray => "FILLED_NEW_ARRAY",
            Aget => "AGET",
            Aput => "APUT",
            NewArray => "NEW_ARRAY",
            NewInstance => "NEW_INSTANCE",
            Iget => "IGET",
            Iput => "IPUT",
            Sget => "SGET",
            Sput => "SPUT",
            Invoke => "INVOKE",
            MoveResult => "MOVE_RESULT",
            Nop => "NOP",
            Ternary => "TERNARY",
            Constructor => "CONSTRUCTOR",
            Break => "BREAK",
            Continue => "CONTINUE",
            StrConcat => "STR_CONCAT",
            OneArg => "ONE_ARG",
            Phi => "PHI",
            RegionArg => "REGION_ARG",
            JavaJsr => "JAVA_JSR",
            JavaRet => "JAVA_RET",
        }
    }
}

impl fmt::Display for InsnType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IfOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl IfOp {
    pub fn symbol(self) -> &'static str {
        match self {
            IfOp::Eq => "==",
            IfOp::Ne => "!=",
            IfOp::Lt => "<",
            IfOp::Le => "<=",
            IfOp::Gt => ">",
            IfOp::Ge => ">=",
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            IfOp::Eq => "EQ",
            IfOp::Ne => "NE",
            IfOp::Lt => "LT",
            IfOp::Le => "LE",
            IfOp::Gt => "GT",
            IfOp::Ge => "GE",
        }
    }
    pub fn invert(self) -> IfOp {
        match self {
            IfOp::Eq => IfOp::Ne,
            IfOp::Ne => IfOp::Eq,
            IfOp::Lt => IfOp::Ge,
            IfOp::Le => IfOp::Gt,
            IfOp::Gt => IfOp::Le,
            IfOp::Ge => IfOp::Lt,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArithOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    And,
    Or,
    Xor,
    Shl,
    Shr,
    Ushr,
}

impl ArithOp {
    pub fn symbol(self) -> &'static str {
        match self {
            ArithOp::Add => "+",
            ArithOp::Sub => "-",
            ArithOp::Mul => "*",
            ArithOp::Div => "/",
            ArithOp::Rem => "%",
            ArithOp::And => "&",
            ArithOp::Or => "|",
            ArithOp::Xor => "^",
            ArithOp::Shl => "<<",
            ArithOp::Shr => ">>",
            ArithOp::Ushr => ">>>",
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            ArithOp::Add => "ADD",
            ArithOp::Sub => "SUB",
            ArithOp::Mul => "MUL",
            ArithOp::Div => "DIV",
            ArithOp::Rem => "REM",
            ArithOp::And => "AND",
            ArithOp::Or => "OR",
            ArithOp::Xor => "XOR",
            ArithOp::Shl => "SHL",
            ArithOp::Shr => "SHR",
            ArithOp::Ushr => "USHR",
        }
    }
    pub fn is_bit_op(self) -> bool {
        matches!(self, ArithOp::And | ArithOp::Or | ArithOp::Xor)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InvokeType {
    Static,
    Direct,
    Virtual,
    Interface,
    Super,
    Polymorphic,
    Custom,
    CustomRaw,
}

impl InvokeType {
    pub fn name(self) -> &'static str {
        match self {
            InvokeType::Static => "STATIC",
            InvokeType::Direct => "DIRECT",
            InvokeType::Virtual => "VIRTUAL",
            InvokeType::Interface => "INTERFACE",
            InvokeType::Super => "SUPER",
            InvokeType::Polymorphic => "POLYMORPHIC",
            InvokeType::Custom => "CUSTOM",
            InvokeType::CustomRaw => "CUSTOM_RAW",
        }
    }
}

/// Register argument. `init_type` is the type given at decode time (may be unknown).
#[derive(Debug, Clone, PartialEq)]
pub struct RegisterArg {
    pub reg_num: u32,
    pub init_type: ArgType,
    /// `AFlag.IMMUTABLE_TYPE`: type must not be changed by type inference.
    pub immutable_type: bool,
}

impl RegisterArg {
    pub fn new(reg_num: u32, init_type: ArgType) -> Self {
        RegisterArg {
            reg_num,
            init_type,
            immutable_type: false,
        }
    }
    pub fn immutable(reg_num: u32, init_type: ArgType) -> Self {
        RegisterArg {
            reg_num,
            init_type,
            immutable_type: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LiteralArg {
    pub literal: i64,
    pub ty: ArgType,
}

impl LiteralArg {
    /// `LiteralArg.makeWithFixedType`: narrow an unknown type by the literal's value.
    pub fn with_fixed_type(literal: i64, ty: ArgType) -> Self {
        let ty = fix_literal_type(literal, ty);
        LiteralArg { literal, ty }
    }
}

fn fix_literal_type(value: i64, ty: ArgType) -> ArgType {
    if value == 0
        || ty.is_type_known()
        || ty.contains(PrimitiveType::Long)
        || ty.contains(PrimitiveType::Double)
    {
        return ty;
    }
    if value == 1 {
        return ArgType::narrow_numbers();
    }
    if value < 0 {
        return ArgType::narrow_neg_numbers();
    }
    ArgType::narrow_numbers_no_bool()
}

#[derive(Debug, Clone, PartialEq)]
pub enum InsnArg {
    Register(RegisterArg),
    Literal(LiteralArg),
}

impl InsnArg {
    pub fn reg(reg_num: u32, ty: ArgType) -> InsnArg {
        InsnArg::Register(RegisterArg::new(reg_num, ty))
    }
    pub fn immutable_reg(reg_num: u32, ty: ArgType) -> InsnArg {
        InsnArg::Register(RegisterArg::immutable(reg_num, ty))
    }
    /// `typeImmutableIfKnownReg`
    pub fn immutable_if_known_reg(reg_num: u32, ty: ArgType) -> InsnArg {
        if ty.is_type_known() {
            InsnArg::immutable_reg(reg_num, ty)
        } else {
            InsnArg::reg(reg_num, ty)
        }
    }
    pub fn lit(literal: i64, ty: ArgType) -> InsnArg {
        InsnArg::Literal(LiteralArg::with_fixed_type(literal, ty))
    }
    pub fn as_register(&self) -> Option<&RegisterArg> {
        match self {
            InsnArg::Register(r) => Some(r),
            _ => None,
        }
    }
}

/// Data of a `fill-array-data` payload.
#[derive(Debug, Clone, PartialEq)]
pub struct FillArrayData {
    pub size: u32,
    pub element_size: u16,
    pub elem_type: ArgType,
    pub data: jadx_input_api::ArrayData,
}

/// Case table of a switch, targets are absolute offsets once attached.
#[derive(Debug, Clone, PartialEq)]
pub struct SwitchData {
    pub keys: Vec<i32>,
    pub targets: Vec<i32>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum IndexValue {
    Type(ArgType),
    Field(FieldInfo),
}

/// Instruction specific data (the Java subclasses of `InsnNode`).
#[derive(Debug, Clone, PartialEq)]
pub enum InsnKind {
    None,
    Goto {
        target: i32,
    },
    If {
        op: IfOp,
        target: i32,
        then_block: Option<BlockId>,
        else_block: Option<BlockId>,
    },
    Switch {
        data_target: i32,
        packed: bool,
        data: Option<SwitchData>,
        def: i32,
        target_blocks: Option<Vec<BlockId>>,
        def_block: Option<BlockId>,
    },
    SwitchData(SwitchData),
    /// String as UTF-16 code units.
    ConstStr(Vec<u16>),
    ConstClass(ArgType),
    Index(IndexValue),
    Invoke {
        invoke_type: InvokeType,
        mth: MethodInfo,
    },
    Arith(ArithOp),
    NewArray(ArgType),
    FilledNewArray(ArgType),
    FillArray {
        target: i32,
        data: Option<FillArrayData>,
    },
    FillArrayData(FillArrayData),
}

/// Block identifier: index into the method's block arena (the block's `cid`).
pub type BlockId = usize;
/// Exception handler identifier: index into the method's handler list.
pub type HandlerId = usize;

/// Instruction flags (subset of the reference `AFlag`).
pub mod iflag {
    pub const TRY_ENTER: u64 = 1 << 0;
    pub const TRY_LEAVE: u64 = 1 << 1;
    pub const SYNTHETIC: u64 = 1 << 2;
    pub const DONT_GENERATE: u64 = 1 << 3;
    pub const HIDDEN: u64 = 1 << 4;
    pub const REMOVE: u64 = 1 << 5;
    pub const WRAPPED: u64 = 1 << 6;
    pub const FALL_THROUGH: u64 = 1 << 7;

    pub const NAMES: &[(u64, &str)] = &[
        (TRY_ENTER, "TRY_ENTER"),
        (TRY_LEAVE, "TRY_LEAVE"),
        (SYNTHETIC, "SYNTHETIC"),
        (DONT_GENERATE, "DONT_GENERATE"),
        (HIDDEN, "HIDDEN"),
        (REMOVE, "REMOVE"),
        (WRAPPED, "WRAPPED"),
        (FALL_THROUGH, "FALL_THROUGH"),
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JumpInfo {
    pub src: i32,
    pub dest: i32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InsnNode {
    pub ty: InsnType,
    /// Offset in code units, `-1` when not set.
    pub offset: i32,
    pub result: Option<RegisterArg>,
    pub args: Vec<InsnArg>,
    pub kind: InsnKind,
    /// Jump sources/targets attached by `ProcessInstructions` (`AType.JUMP`).
    pub jumps: Vec<JumpInfo>,
    /// Decode error recorded on a replacement `NOP` (`AType.JADX_ERROR`).
    pub error: Option<String>,
    /// `iflag` bits.
    pub flags: u64,
    /// `AType.EXC_CATCH`: handlers protecting this instruction.
    pub catch: Option<Vec<HandlerId>>,
    /// `AType.EXC_HANDLER`: this instruction starts the given handler.
    pub exc_handler: Option<HandlerId>,
}

impl InsnNode {
    pub fn new(ty: InsnType, kind: InsnKind) -> Self {
        InsnNode {
            ty,
            offset: -1,
            result: None,
            args: Vec::new(),
            kind,
            jumps: Vec::new(),
            error: None,
            flags: 0,
            catch: None,
            exc_handler: None,
        }
    }
    pub fn plain(ty: InsnType) -> Self {
        InsnNode::new(ty, InsnKind::None)
    }
    pub fn contains_flag(&self, flag: u64) -> bool {
        self.flags & flag != 0
    }

    /// `isAttrStorageEmpty`: no flags and no attributes at all.
    pub fn attrs_empty(&self) -> bool {
        self.flags == 0
            && self.jumps.is_empty()
            && self.error.is_none()
            && self.catch.is_none()
            && self.exc_handler.is_none()
    }

    /// `copyAttributesFrom`: flags are merged, attributes of the other node replace ours.
    pub fn copy_attributes_from(&mut self, other: &InsnNode) {
        self.flags |= other.flags;
        if !other.jumps.is_empty() {
            self.jumps = other.jumps.clone();
        }
        if other.error.is_some() {
            self.error = other.error.clone();
        }
        if other.catch.is_some() {
            self.catch = other.catch.clone();
        }
        if other.exc_handler.is_some() {
            self.exc_handler = other.exc_handler;
        }
    }

    pub fn with_result(mut self, res: RegisterArg) -> Self {
        self.result = Some(res);
        self
    }
    pub fn with_arg(mut self, arg: InsnArg) -> Self {
        self.args.push(arg);
        self
    }
}
