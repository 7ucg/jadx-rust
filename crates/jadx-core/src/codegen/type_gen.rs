//! `TypeGen`: type signatures and literal values as source text.

use super::string_utils::{format_double, format_float, java_double_to_string, StringUtils};
use super::CodegenError;
use crate::model::types::{ArgType, PrimitiveType};

/// `TypeGen.signature`
pub fn signature(ty: &ArgType) -> String {
    match ty.primitive_type() {
        Some(PrimitiveType::Object) => format!(
            "L{};",
            ty.object_name().unwrap_or_default().replace('.', "/")
        ),
        Some(PrimitiveType::Array) => {
            format!("[{}", ty.array_element().map(signature).unwrap_or_default())
        }
        Some(p) => p.short_name().to_string(),
        None => String::from("?"),
    }
}

/// `TypeGen.signatures`
pub fn signatures(types: &[ArgType]) -> Vec<String> {
    types.iter().map(signature).collect()
}

/// `TypeGen.literalToString`: literal value as a source string according to its type.
///
/// `fallback` selects the debug output of unknown types, `cast` forces explicit casts of
/// numbers and chars.
pub fn literal_to_string(
    lit: i64,
    ty: &ArgType,
    su: &StringUtils,
    fallback: bool,
    cast: bool,
) -> Result<String, CodegenError> {
    if !ty.is_type_known() {
        let n = lit.to_string();
        if fallback && lit.checked_abs().is_some_and(|a| a > 100) {
            let mut sb = format!("{n}(0x{lit:x}");
            if ty.contains(PrimitiveType::Float) {
                sb.push_str(&format!(
                    ", float:{}",
                    super::string_utils::java_float_to_string(f32::from_bits(lit as u32))
                ));
            }
            if ty.contains(PrimitiveType::Double) {
                sb.push_str(&format!(
                    ", double:{}",
                    java_double_to_string(f64::from_bits(lit as u64))
                ));
            }
            sb.push(')');
            return Ok(sb);
        }
        return Ok(n);
    }
    match ty.primitive_type() {
        Some(PrimitiveType::Boolean) => Ok(if lit == 0 { "false" } else { "true" }.to_string()),
        Some(PrimitiveType::Char) => Ok(su.unescape_char(lit as u16, cast)),
        Some(PrimitiveType::Byte) => Ok(su.format_byte(lit, cast)),
        Some(PrimitiveType::Short) => Ok(su.format_short(lit, cast)),
        Some(PrimitiveType::Int) => Ok(su.format_integer(lit, cast)),
        Some(PrimitiveType::Long) => Ok(su.format_long(lit, cast)),
        Some(PrimitiveType::Float) => Ok(format_float(f32::from_bits(lit as u32))),
        Some(PrimitiveType::Double) => Ok(format_double(f64::from_bits(lit as u64))),
        Some(PrimitiveType::Object | PrimitiveType::Array) => {
            if lit != 0 {
                // wrong object literal
                return Ok(lit.to_string());
            }
            Ok("null".to_string())
        }
        _ => Err(CodegenError::new(format!(
            "Unknown type in literalToString: {ty}"
        ))),
    }
}

/// `TypeGen.literalToRawString`
pub fn literal_to_raw_string(lit: i64, ty: &ArgType) -> Option<String> {
    match ty.primitive_type()? {
        PrimitiveType::Boolean => Some(if lit == 0 { "false" } else { "true" }.to_string()),
        PrimitiveType::Char => Some(
            char::from_u32(u32::from(lit as u16))
                .map(String::from)
                .unwrap_or_default(),
        ),
        PrimitiveType::Byte | PrimitiveType::Short | PrimitiveType::Int | PrimitiveType::Long => {
            Some(lit.to_string())
        }
        PrimitiveType::Float => Some(super::string_utils::java_float_to_string(f32::from_bits(
            lit as u32,
        ))),
        PrimitiveType::Double => Some(java_double_to_string(f64::from_bits(lit as u64))),
        PrimitiveType::Object | PrimitiveType::Array => Some(if lit != 0 {
            lit.to_string()
        } else {
            "null".to_string()
        }),
        PrimitiveType::Void => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signatures_of_types() {
        assert_eq!(signature(&ArgType::int()), "I");
        assert_eq!(signature(&ArgType::string()), "Ljava/lang/String;");
        assert_eq!(signature(&ArgType::array(ArgType::long())), "[J");
    }

    #[test]
    fn literals() {
        let su = StringUtils::default();
        let s = |l: i64, t: ArgType| literal_to_string(l, &t, &su, false, false).unwrap();
        assert_eq!(s(1, ArgType::boolean()), "true");
        assert_eq!(s(0, ArgType::object_type()), "null");
        assert_eq!(s(65, ArgType::char()), "'A'");
        assert_eq!(s(0x3fc00000, ArgType::float()), "1.5f");
        assert_eq!(s(0x3ff8000000000000, ArgType::double()), "1.5d");
        assert_eq!(s(-5, ArgType::int()), "-5");
        assert_eq!(s(5, ArgType::narrow_numbers()), "5");
        assert!(literal_to_string(0, &ArgType::void(), &su, false, false).is_err());
        let dbg = literal_to_string(1000, &ArgType::narrow_numbers(), &su, true, false).unwrap();
        assert!(dbg.starts_with("1000(0x3e8, float:1.4"), "{dbg}");
        assert_eq!(
            literal_to_string(5, &ArgType::narrow_numbers(), &su, true, false).unwrap(),
            "5"
        );
    }
}
