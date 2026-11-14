//! Type model (`ArgType`): primitives, objects, generics, arrays and "unknown" types that are
//! narrowed by type inference.

use std::fmt;

/// Primitive kinds. The declaration order matters: it is the order of `ArgType::unknown()`
/// "all types" and of the printed possible-type lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum PrimitiveType {
    Boolean,
    Char,
    Byte,
    Short,
    Int,
    Float,
    Long,
    Double,
    Object,
    Array,
    Void,
}

impl PrimitiveType {
    pub const ALL: [PrimitiveType; 11] = [
        PrimitiveType::Boolean,
        PrimitiveType::Char,
        PrimitiveType::Byte,
        PrimitiveType::Short,
        PrimitiveType::Int,
        PrimitiveType::Float,
        PrimitiveType::Long,
        PrimitiveType::Double,
        PrimitiveType::Object,
        PrimitiveType::Array,
        PrimitiveType::Void,
    ];

    pub fn short_name(self) -> &'static str {
        match self {
            PrimitiveType::Boolean => "Z",
            PrimitiveType::Char => "C",
            PrimitiveType::Byte => "B",
            PrimitiveType::Short => "S",
            PrimitiveType::Int => "I",
            PrimitiveType::Float => "F",
            PrimitiveType::Long => "J",
            PrimitiveType::Double => "D",
            PrimitiveType::Object => "L",
            PrimitiveType::Array => "[",
            PrimitiveType::Void => "V",
        }
    }

    pub fn long_name(self) -> &'static str {
        match self {
            PrimitiveType::Boolean => "boolean",
            PrimitiveType::Char => "char",
            PrimitiveType::Byte => "byte",
            PrimitiveType::Short => "short",
            PrimitiveType::Int => "int",
            PrimitiveType::Float => "float",
            PrimitiveType::Long => "long",
            PrimitiveType::Double => "double",
            PrimitiveType::Object => "OBJECT",
            PrimitiveType::Array => "ARRAY",
            PrimitiveType::Void => "void",
        }
    }

    pub fn is_object_or_array(self) -> bool {
        matches!(self, PrimitiveType::Object | PrimitiveType::Array)
    }
}

impl fmt::Display for PrimitiveType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.long_name())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum WildcardBound {
    Extends,
    Unbound,
    Super,
}

impl WildcardBound {
    pub fn num(self) -> i32 {
        match self {
            WildcardBound::Extends => 1,
            WildcardBound::Unbound => 0,
            WildcardBound::Super => -1,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            WildcardBound::Extends => "? extends ",
            WildcardBound::Unbound => "?",
            WildcardBound::Super => "? super ",
        }
    }
}

pub const CLASS_OBJECT: &str = "java.lang.Object";
pub const CLASS_STRING: &str = "java.lang.String";
pub const CLASS_CLASS: &str = "java.lang.Class";
pub const CLASS_THROWABLE: &str = "java.lang.Throwable";
pub const CLASS_ERROR: &str = "java.lang.Error";
pub const CLASS_EXCEPTION: &str = "java.lang.Exception";
pub const CLASS_RUNTIME_EXCEPTION: &str = "java.lang.RuntimeException";
pub const CLASS_ENUM: &str = "java.lang.Enum";

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ArgType {
    Primitive(PrimitiveType),
    /// Plain class type, full name with dots (`java.lang.String`).
    Object(String),
    /// Type variable (`T`) with optional bounds.
    GenericType {
        name: String,
        extend_types: Vec<ArgType>,
    },
    Wildcard {
        ty: Box<ArgType>,
        bound: WildcardBound,
    },
    /// Parameterized class (`List<String>`).
    Generic {
        name: String,
        generics: Vec<ArgType>,
    },
    /// Inner class of a parameterized outer class (`Outer<A>$Inner<B>`).
    OuterGeneric {
        outer: Box<ArgType>,
        inner: Box<ArgType>,
    },
    Array(Box<ArgType>),
    /// Not yet inferred: any of the listed primitive kinds.
    Unknown(Vec<PrimitiveType>),
}

/// `Lcom/pkg/Name;` to `com.pkg.Name`; anything else is returned unchanged.
pub fn clean_object_name(obj: &str) -> String {
    if let Some(inner) = obj.strip_prefix('L').and_then(|s| s.strip_suffix(';')) {
        return inner.replace('/', ".");
    }
    obj.to_string()
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, serde::Serialize, serde::Deserialize)]
#[error("{0}")]
pub struct TypeParseError(pub String);

use PrimitiveType as P;

impl ArgType {
    pub fn primitive(p: PrimitiveType) -> ArgType {
        ArgType::Primitive(p)
    }
    pub fn int() -> ArgType {
        ArgType::Primitive(P::Int)
    }
    pub fn boolean() -> ArgType {
        ArgType::Primitive(P::Boolean)
    }
    pub fn byte() -> ArgType {
        ArgType::Primitive(P::Byte)
    }
    pub fn short() -> ArgType {
        ArgType::Primitive(P::Short)
    }
    pub fn char() -> ArgType {
        ArgType::Primitive(P::Char)
    }
    pub fn float() -> ArgType {
        ArgType::Primitive(P::Float)
    }
    pub fn double() -> ArgType {
        ArgType::Primitive(P::Double)
    }
    pub fn long() -> ArgType {
        ArgType::Primitive(P::Long)
    }
    pub fn void() -> ArgType {
        ArgType::Primitive(P::Void)
    }

    pub fn object_type() -> ArgType {
        ArgType::Object(CLASS_OBJECT.into())
    }
    pub fn string() -> ArgType {
        ArgType::Object(CLASS_STRING.into())
    }
    pub fn class() -> ArgType {
        ArgType::Object(CLASS_CLASS.into())
    }
    pub fn enum_type() -> ArgType {
        ArgType::Object(CLASS_ENUM.into())
    }
    pub fn throwable() -> ArgType {
        ArgType::Object(CLASS_THROWABLE.into())
    }
    pub fn exception() -> ArgType {
        ArgType::Object(CLASS_EXCEPTION.into())
    }
    pub fn error() -> ArgType {
        ArgType::Object(CLASS_ERROR.into())
    }
    pub fn runtime_exception() -> ArgType {
        ArgType::Object(CLASS_RUNTIME_EXCEPTION.into())
    }
    pub fn object_array() -> ArgType {
        ArgType::array(ArgType::object_type())
    }
    pub fn wildcard_unbound() -> ArgType {
        ArgType::Wildcard {
            ty: Box::new(ArgType::object_type()),
            bound: WildcardBound::Unbound,
        }
    }

    pub fn unknown(types: &[PrimitiveType]) -> ArgType {
        ArgType::Unknown(types.to_vec())
    }
    /// `??`
    pub fn unknown_any() -> ArgType {
        ArgType::unknown(&PrimitiveType::ALL)
    }
    pub fn unknown_object() -> ArgType {
        ArgType::unknown(&[P::Object, P::Array])
    }
    pub fn unknown_object_no_array() -> ArgType {
        ArgType::unknown(&[P::Object])
    }
    pub fn unknown_array() -> ArgType {
        ArgType::array(ArgType::unknown_any())
    }
    pub fn narrow() -> ArgType {
        ArgType::unknown(&[
            P::Int,
            P::Float,
            P::Boolean,
            P::Short,
            P::Byte,
            P::Char,
            P::Object,
            P::Array,
        ])
    }
    pub fn narrow_numbers() -> ArgType {
        ArgType::unknown(&[P::Boolean, P::Int, P::Float, P::Short, P::Byte, P::Char])
    }
    pub fn narrow_integral() -> ArgType {
        ArgType::unknown(&[P::Int, P::Short, P::Byte, P::Char])
    }
    pub fn narrow_numbers_no_bool() -> ArgType {
        ArgType::unknown(&[P::Int, P::Float, P::Short, P::Byte, P::Char])
    }
    pub fn narrow_neg_numbers() -> ArgType {
        ArgType::unknown(&[P::Int, P::Short, P::Byte, P::Float])
    }
    pub fn narrow_numbers_no_float() -> ArgType {
        ArgType::unknown(&[P::Int, P::Boolean, P::Short, P::Byte, P::Char])
    }
    pub fn wide() -> ArgType {
        ArgType::unknown(&[P::Long, P::Double])
    }
    pub fn int_float() -> ArgType {
        ArgType::unknown(&[P::Int, P::Float])
    }
    pub fn int_boolean() -> ArgType {
        ArgType::unknown(&[P::Int, P::Boolean])
    }
    pub fn byte_boolean() -> ArgType {
        ArgType::unknown(&[P::Byte, P::Boolean])
    }
    pub fn unknown_int() -> ArgType {
        ArgType::unknown(&[P::Int])
    }

    /// Class type from a descriptor (`Ljava/lang/String;`) or a dotted name.
    pub fn object(name: &str) -> ArgType {
        ArgType::Object(clean_object_name(name))
    }

    pub fn generic_type(name: &str) -> ArgType {
        ArgType::GenericType {
            name: name.to_string(),
            extend_types: Vec::new(),
        }
    }

    pub fn generic(obj: &ArgType, generics: Vec<ArgType>) -> ArgType {
        match obj.object_name() {
            Some(name) if obj.is_object() => ArgType::Generic {
                name: name.to_string(),
                generics,
            },
            _ => panic!("Expected Object as ArgType, got: {obj}"),
        }
    }

    pub fn generic_named(name: &str, generics: Vec<ArgType>) -> ArgType {
        ArgType::Generic {
            name: clean_object_name(name),
            generics,
        }
    }

    pub fn array(elem: ArgType) -> ArgType {
        ArgType::Array(Box::new(elem))
    }

    pub fn array_dim(ty: ArgType, dimension: u32) -> ArgType {
        let mut t = ty;
        for _ in 0..dimension {
            t = ArgType::array(t);
        }
        t
    }

    pub fn is_type_known(&self) -> bool {
        match self {
            ArgType::Unknown(_) => false,
            ArgType::Array(e) => e.is_type_known(),
            _ => true,
        }
    }

    pub fn primitive_type(&self) -> Option<PrimitiveType> {
        match self {
            ArgType::Primitive(p) => Some(*p),
            ArgType::Array(_) => Some(P::Array),
            ArgType::Unknown(_) => None,
            _ => Some(P::Object),
        }
    }

    pub fn is_primitive(&self) -> bool {
        matches!(self, ArgType::Primitive(_))
    }

    /// True for all class-like types (plain, type variable, wildcard, parameterized).
    pub fn is_object(&self) -> bool {
        matches!(
            self,
            ArgType::Object(_)
                | ArgType::GenericType { .. }
                | ArgType::Wildcard { .. }
                | ArgType::Generic { .. }
                | ArgType::OuterGeneric { .. }
        )
    }

    /// Class name of an object type; `None` for non-objects.
    pub fn object_name(&self) -> Option<String> {
        match self {
            ArgType::Object(n)
            | ArgType::GenericType { name: n, .. }
            | ArgType::Generic { name: n, .. } => Some(n.clone()),
            ArgType::Wildcard { .. } => Some(CLASS_OBJECT.to_string()),
            ArgType::OuterGeneric { outer, inner } => {
                Some(format!("{}${}", outer.object_name()?, inner.object_name()?))
            }
            _ => None,
        }
    }

    pub fn is_generic(&self) -> bool {
        matches!(
            self,
            ArgType::Wildcard { .. } | ArgType::Generic { .. } | ArgType::OuterGeneric { .. }
        )
    }

    pub fn is_generic_type(&self) -> bool {
        matches!(self, ArgType::GenericType { .. })
    }

    pub fn generic_types(&self) -> Option<&[ArgType]> {
        match self {
            ArgType::Generic { generics, .. } => Some(generics),
            ArgType::OuterGeneric { inner, .. } => inner.generic_types(),
            _ => None,
        }
    }

    pub fn extend_types(&self) -> &[ArgType] {
        match self {
            ArgType::GenericType { extend_types, .. } => extend_types,
            _ => &[],
        }
    }

    pub fn wildcard_type(&self) -> Option<&ArgType> {
        match self {
            ArgType::Wildcard { ty, .. } => Some(ty),
            _ => None,
        }
    }

    pub fn wildcard_bound(&self) -> Option<WildcardBound> {
        match self {
            ArgType::Wildcard { bound, .. } => Some(*bound),
            _ => None,
        }
    }

    pub fn is_wildcard(&self) -> bool {
        matches!(self, ArgType::Wildcard { .. })
    }

    pub fn outer_type(&self) -> Option<&ArgType> {
        match self {
            ArgType::OuterGeneric { outer, .. } => Some(outer),
            _ => None,
        }
    }

    pub fn inner_type(&self) -> Option<&ArgType> {
        match self {
            ArgType::OuterGeneric { inner, .. } => Some(inner),
            _ => None,
        }
    }

    pub fn is_array(&self) -> bool {
        matches!(self, ArgType::Array(_))
    }

    pub fn array_dimension(&self) -> u32 {
        match self {
            ArgType::Array(e) => 1 + e.array_dimension(),
            _ => 0,
        }
    }

    pub fn array_element(&self) -> Option<&ArgType> {
        match self {
            ArgType::Array(e) => Some(e),
            _ => None,
        }
    }

    pub fn array_root_element(&self) -> &ArgType {
        match self {
            ArgType::Array(e) => e.array_root_element(),
            _ => self,
        }
    }

    /// Primitive kinds an unknown type may still become (empty for known types, `[Array]` for arrays).
    pub fn possible_types(&self) -> Vec<PrimitiveType> {
        match self {
            ArgType::Unknown(t) => t.clone(),
            ArgType::Array(_) => vec![P::Array],
            _ => Vec::new(),
        }
    }

    pub fn contains(&self, ty: PrimitiveType) -> bool {
        match self {
            ArgType::Unknown(t) => t.contains(&ty),
            _ => self.primitive_type() == Some(ty),
        }
    }

    /// Pick the preferred concrete type of an unknown type (`None` for known types).
    pub fn select_first(&self) -> Option<ArgType> {
        match self {
            ArgType::Unknown(types) => {
                if self.contains(P::Object) {
                    Some(ArgType::object_type())
                } else if self.contains(P::Array) {
                    Some(ArgType::array(ArgType::object_type()))
                } else {
                    types.first().map(|p| ArgType::Primitive(*p))
                }
            }
            ArgType::Array(e) => e.select_first().map(ArgType::array),
            _ => None,
        }
    }

    pub fn can_be_object(&self) -> bool {
        self.is_object() || (!self.is_type_known() && self.contains(P::Object))
    }

    pub fn can_be_array(&self) -> bool {
        self.is_array() || (!self.is_type_known() && self.contains(P::Array))
    }

    pub fn can_be_primitive(&self, p: PrimitiveType) -> bool {
        (self.is_primitive() && self.primitive_type() == Some(p))
            || (!self.is_type_known() && self.contains(p))
    }

    pub fn can_be_any_number(&self) -> bool {
        if let Some(p) = self.as_primitive() {
            return !p.is_object_or_array();
        }
        self.possible_types()
            .iter()
            .any(|p| !p.is_object_or_array())
    }

    pub fn as_primitive(&self) -> Option<PrimitiveType> {
        match self {
            ArgType::Primitive(p) => Some(*p),
            _ => None,
        }
    }

    pub fn convert_from_primitive(p: PrimitiveType) -> ArgType {
        match p {
            P::Object => ArgType::object_type(),
            P::Array => ArgType::object_array(),
            other => ArgType::Primitive(other),
        }
    }

    /// Parse a descriptor: `I`, `Ljava/lang/String;`, `[I`, `TT;` (type variable).
    pub fn parse(s: &str) -> Result<ArgType, TypeParseError> {
        let mut chars = s.chars();
        let Some(f) = chars.next() else {
            return Err(TypeParseError(format!(
                "Failed to parse type string: {s:?}"
            )));
        };
        match f {
            'L' => Ok(ArgType::object(s)),
            'T' => Ok(ArgType::generic_type(
                &s[1..s.len().saturating_sub(1).max(1)],
            )),
            '[' => Ok(ArgType::array(ArgType::parse(&s[1..])?)),
            _ => {
                if s.chars().count() != 1 {
                    return Err(TypeParseError(format!("Unknown type string: \"{s}\"")));
                }
                ArgType::parse_char(f)
            }
        }
    }

    pub fn parse_char(f: char) -> Result<ArgType, TypeParseError> {
        Ok(match f {
            'Z' => ArgType::boolean(),
            'B' => ArgType::byte(),
            'C' => ArgType::char(),
            'S' => ArgType::short(),
            'I' => ArgType::int(),
            'J' => ArgType::long(),
            'F' => ArgType::float(),
            'D' => ArgType::double(),
            'V' => ArgType::void(),
            _ => {
                return Err(TypeParseError(format!(
                    "Unknown type char: '{f}' (0x{:x})",
                    f as u32
                )))
            }
        })
    }

    /// Number of registers a value of this type occupies (0 for unknown types).
    pub fn reg_count(&self) -> u32 {
        if let Some(p) = self.as_primitive() {
            return if matches!(p, P::Long | P::Double) {
                2
            } else {
                1
            };
        }
        if !self.is_type_known() {
            return 0;
        }
        1
    }

    pub fn contains_generic(&self) -> bool {
        if self.is_generic() || self.is_generic_type() {
            return true;
        }
        self.array_element()
            .map(ArgType::contains_generic)
            .unwrap_or(false)
    }

    pub fn contains_type_variable(&self) -> bool {
        if self.is_generic_type() {
            return true;
        }
        if let Some(w) = self.wildcard_type() {
            return w.contains_type_variable();
        }
        if self.is_generic() {
            if let Some(g) = self.generic_types() {
                if g.iter().any(ArgType::contains_type_variable) {
                    return true;
                }
            }
            if let Some(o) = self.outer_type() {
                return o.contains_type_variable();
            }
            return false;
        }
        self.array_element()
            .map(ArgType::contains_type_variable)
            .unwrap_or(false)
    }

    pub fn is_void(&self) -> bool {
        self.as_primitive() == Some(P::Void)
    }

    /// Visit this type and all sub types; stop at the first `Some`.
    pub fn visit_types<R>(&self, visitor: &mut impl FnMut(&ArgType) -> Option<R>) -> Option<R> {
        if let Some(r) = visitor(self) {
            return Some(r);
        }
        if let Some(e) = self.array_element() {
            return e.visit_types(visitor);
        }
        if let Some(w) = self.wildcard_type() {
            if let Some(r) = w.visit_types(visitor) {
                return Some(r);
            }
        }
        if self.is_generic() {
            if let Some(g) = self.generic_types() {
                for t in g {
                    if let Some(r) = t.visit_types(visitor) {
                        return Some(r);
                    }
                }
            }
        }
        None
    }
}

impl fmt::Display for ArgType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let join = |f: &mut fmt::Formatter<'_>, list: &[ArgType], sep: &str| -> fmt::Result {
            for (i, t) in list.iter().enumerate() {
                if i > 0 {
                    f.write_str(sep)?;
                }
                write!(f, "{t}")?;
            }
            Ok(())
        };
        match self {
            ArgType::Primitive(p) => write!(f, "{p}"),
            ArgType::Object(n) => f.write_str(n),
            ArgType::GenericType { name, extend_types } => {
                f.write_str(name)?;
                if !extend_types.is_empty() {
                    f.write_str(" extends ")?;
                    join(f, extend_types, " & ")?;
                }
                Ok(())
            }
            ArgType::Wildcard { ty, bound } => {
                if *bound == WildcardBound::Unbound {
                    f.write_str(bound.as_str())
                } else {
                    write!(f, "{}{}", bound.as_str(), ty)
                }
            }
            ArgType::Generic { name, generics } => {
                write!(f, "{name}<")?;
                join(f, generics, ", ")?;
                f.write_str(">")
            }
            ArgType::OuterGeneric { outer, inner } => write!(f, "{outer}${inner}"),
            ArgType::Array(e) => write!(f, "{e}[]"),
            ArgType::Unknown(types) => {
                if types.len() == PrimitiveType::ALL.len() {
                    f.write_str("??")
                } else {
                    f.write_str("??[")?;
                    for (i, t) in types.iter().enumerate() {
                        if i > 0 {
                            f.write_str(", ")?;
                        }
                        write!(f, "{t}")?;
                    }
                    f.write_str("]")
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_print() {
        assert_eq!(ArgType::parse("I").unwrap().to_string(), "int");
        assert_eq!(
            ArgType::parse("Ljava/lang/String;").unwrap().to_string(),
            "java.lang.String"
        );
        assert_eq!(ArgType::parse("[[J").unwrap().to_string(), "long[][]");
        assert_eq!(ArgType::parse("TT;").unwrap().to_string(), "T");
        assert!(ArgType::parse("").is_err());
        assert!(ArgType::parse("Q").is_err());
        assert!(ArgType::parse("II").is_err());
    }

    #[test]
    fn unknown_printing() {
        assert_eq!(ArgType::unknown_any().to_string(), "??");
        assert_eq!(ArgType::wide().to_string(), "??[long, double]");
        assert_eq!(
            ArgType::narrow_integral().to_string(),
            "??[int, short, byte, char]"
        );
    }

    #[test]
    fn reg_counts() {
        assert_eq!(ArgType::long().reg_count(), 2);
        assert_eq!(ArgType::int().reg_count(), 1);
        assert_eq!(ArgType::wide().reg_count(), 0);
        assert_eq!(ArgType::string().reg_count(), 1);
    }

    #[test]
    fn generics_print() {
        let list = ArgType::generic_named("java.util.List", vec![ArgType::string()]);
        assert_eq!(list.to_string(), "java.util.List<java.lang.String>");
        let w = ArgType::Wildcard {
            ty: Box::new(ArgType::string()),
            bound: WildcardBound::Extends,
        };
        assert_eq!(w.to_string(), "? extends java.lang.String");
        assert_eq!(ArgType::wildcard_unbound().to_string(), "?");
    }
}
