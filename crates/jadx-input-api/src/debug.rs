//! Debug information of a method (`IDebugInfo`): source lines and local variables.

/// `ILocalVar`: a named register range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalVar {
    pub reg: u32,
    pub name: String,
    /// Type descriptor.
    pub ty: String,
    pub signature: Option<String>,
    /// First code offset of the range, `-1` for the parameters of the method.
    pub start: i32,
    pub end: i32,
}

/// `IDebugInfo`
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DebugInfo {
    /// Code offset and source line, in the order the positions were found (a later entry of the
    /// same offset replaces an earlier one).
    pub lines: Vec<(u32, u32)>,
    /// Local variables in the order their ranges ended.
    pub locals: Vec<LocalVar>,
}
