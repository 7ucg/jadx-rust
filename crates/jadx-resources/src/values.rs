//! Resource value decoding (`ValuesParser`, `XmlGenUtils`).

use std::collections::HashMap;

use crate::android_map::android_res_name;
use crate::consts::*;
use crate::strings::BinaryXmlStrings;
use crate::text::hex32;

/// Typed raw value of a resource entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawValue {
    pub data_type: i32,
    pub data: i32,
}

/// Name reference and value of one item of a complex resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawNamedValue {
    pub name_ref: i32,
    pub value: RawValue,
}

/// Decodes raw values to their text form, resolving references through the id to `type/name` map.
pub struct ValuesParser<'a> {
    strings: &'a BinaryXmlStrings,
    res_map: &'a HashMap<i32, String>,
}

impl<'a> ValuesParser<'a> {
    pub fn new(strings: &'a BinaryXmlStrings, res_map: &'a HashMap<i32, String>) -> Self {
        ValuesParser { strings, res_map }
    }

    pub fn decode_raw(&self, v: &RawValue) -> Option<String> {
        self.decode_value(v.data_type, v.data)
    }

    pub fn decode_value(&self, data_type: i32, data: i32) -> Option<String> {
        match data_type {
            TYPE_NULL => None,
            TYPE_STRING => Some(self.strings.get(data)),
            TYPE_INT_DEC => Some(data.to_string()),
            TYPE_INT_HEX => Some(format!("0x{}", hex32(data))),
            TYPE_INT_BOOLEAN => Some(if data == 0 { "false" } else { "true" }.to_string()),
            TYPE_FLOAT => Some(float_to_string(f32::from_bits(data as u32))),
            TYPE_INT_COLOR_ARGB8 => Some(format!("#{:08x}", data as u32)),
            TYPE_INT_COLOR_RGB8 => Some(format!("#{:06x}", data as u32 & 0xFF_FFFF)),
            TYPE_INT_COLOR_ARGB4 => Some(format!("#{:04x}", data as u32 & 0xFFFF)),
            TYPE_INT_COLOR_RGB4 => Some(format!("#{:03x}", data as u32 & 0xFFF)),
            TYPE_DYNAMIC_REFERENCE | TYPE_REFERENCE => {
                if let Some(ri) = self.res_map.get(&data) {
                    return Some(format!("@{ri}"));
                }
                if let Some(a) = android_res_name(data) {
                    return Some(format!("@android:{a}"));
                }
                if data == 0 {
                    return Some("0".to_string());
                }
                Some(format!("?unknown_ref: {}", hex32(data)))
            }
            TYPE_ATTRIBUTE => {
                if let Some(ri) = self.res_map.get(&data) {
                    return Some(format!("?{ri}"));
                }
                if let Some(a) = android_res_name(data) {
                    return Some(format!("?android:{a}"));
                }
                Some(format!("?unknown_attr_ref: {}", hex32(data)))
            }
            TYPE_DIMENSION => Some(decode_complex(data, false)),
            TYPE_FRACTION => Some(decode_complex(data, true)),
            TYPE_DYNAMIC_ATTRIBUTE => Some(format!("  TYPE_DYNAMIC_ATTRIBUTE: {data}")),
            _ => Some(format!("  ?0x{} {}", hex32(data_type), data)),
        }
    }

    /// Text of an item name reference (`attr.name`, `android:attr.name`, `?0x...`); `None` for the
    /// internal id with an empty entry part.
    pub fn decode_name_ref(&self, name_ref: i32) -> Option<String> {
        let mut r = name_ref;
        if is_res_internal_id(name_ref) {
            r = name_ref & ATTR_TYPE_ANY;
            if r == 0 {
                return None;
            }
        }
        if let Some(ri) = self.res_map.get(&r) {
            return Some(ri.replace('/', "."));
        }
        if let Some(a) = android_res_name(r) {
            return Some(format!("android:{}", a.replace('/', ".")));
        }
        Some(format!("?0x{}", hex32(name_ref)))
    }
}

/// `XmlGenUtils.decodeComplex`
pub fn decode_complex(data: i32, is_fraction: bool) -> String {
    let mantissa = data & ((COMPLEX_MANTISSA_MASK << COMPLEX_MANTISSA_SHIFT) as u32 as i32);
    let radix = ((data >> COMPLEX_RADIX_SHIFT) & COMPLEX_RADIX_MASK) as usize;
    let mut value = f64::from(mantissa) * RADIX_MULTS[radix];
    let unit_type = data & COMPLEX_UNIT_MASK;
    let unit = if is_fraction {
        value *= 100.0;
        match unit_type {
            COMPLEX_UNIT_FRACTION => "%".to_string(),
            COMPLEX_UNIT_FRACTION_PARENT => "%p".to_string(),
            _ => format!("?f{unit_type:x}"),
        }
    } else {
        match unit_type {
            COMPLEX_UNIT_PX => "px".to_string(),
            COMPLEX_UNIT_DIP => "dp".to_string(),
            COMPLEX_UNIT_SP => "sp".to_string(),
            COMPLEX_UNIT_PT => "pt".to_string(),
            COMPLEX_UNIT_IN => "in".to_string(),
            COMPLEX_UNIT_MM => "mm".to_string(),
            _ => format!("?d{unit_type:x}"),
        }
    };
    format!("{}{}", double_to_string(value), unit)
}

/// `XmlGenUtils.doubleToString`: integral values print as an `int`, others with at most four
/// fraction digits (half even), grouping separators and no trailing zeros.
pub fn double_to_string(value: f64) -> String {
    if (value.is_nan() || value == value.floor()) && !value.is_infinite() {
        return java_f64_to_i32(value).to_string();
    }
    if value.is_infinite() {
        return if value > 0.0 { "\u{221e}" } else { "-\u{221e}" }.to_string();
    }
    let s = format!("{value:.4}");
    let (neg, digits) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s.as_str()),
    };
    let (int_part, frac_part) = digits.split_once('.').unwrap_or((digits, ""));
    let frac = frac_part.trim_end_matches('0');
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    let len = int_part.len();
    for (i, c) in int_part.chars().enumerate() {
        if i > 0 && (len - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if !frac.is_empty() {
        out.push('.');
        out.push_str(frac);
    }
    out
}

fn java_f64_to_i32(v: f64) -> i32 {
    // Rust float to int casts saturate and map NaN to 0 like the Java narrowing conversion
    v as i32
}

pub fn float_to_string(value: f32) -> String {
    double_to_string(f64::from(value))
}

/// `XmlGenUtils.getAttrTypeAsString`
pub fn attr_type_as_string(ty: i32) -> Option<String> {
    let table = [
        (ATTR_TYPE_REFERENCE, "reference"),
        (ATTR_TYPE_STRING, "string"),
        (ATTR_TYPE_INTEGER, "integer"),
        (ATTR_TYPE_BOOLEAN, "boolean"),
        (ATTR_TYPE_COLOR, "color"),
        (ATTR_TYPE_FLOAT, "float"),
        (ATTR_TYPE_DIMENSION, "dimension"),
        (ATTR_TYPE_FRACTION, "fraction"),
    ];
    let parts: Vec<&str> = table
        .iter()
        .filter(|(bit, _)| ty & bit != 0)
        .map(|(_, n)| *n)
        .collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("|"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn number_formatting() {
        assert_eq!(double_to_string(16.0), "16");
        assert_eq!(double_to_string(0.5), "0.5");
        assert_eq!(double_to_string(1234.5), "1,234.5");
        assert_eq!(double_to_string(-0.00001), "-0");
        assert_eq!(double_to_string(0.03125), "0.0312");
        assert_eq!(double_to_string(0.09375), "0.0938");
        assert_eq!(double_to_string(f64::NAN), "0");
        assert_eq!(double_to_string(1e12), "2147483647");
        assert_eq!(float_to_string(0.1), "0.1");
        assert_eq!(float_to_string(33.333332), "33.3333");
    }

    #[test]
    fn complex_values() {
        // 16dp: mantissa 16 << 8, radix 23p0, unit dip
        assert_eq!(decode_complex(16 << 8 | 1, false), "16dp");
        // 50%: radix 0p23 ( 0.5 * 2^23 << 8 )
        assert_eq!(decode_complex((0x400000 << 8) | (3 << 4), true), "50%");
    }

    #[test]
    fn attr_types() {
        assert_eq!(
            attr_type_as_string(ATTR_TYPE_REFERENCE | ATTR_TYPE_COLOR).as_deref(),
            Some("reference|color")
        );
        assert_eq!(attr_type_as_string(ATTR_TYPE_ENUM), None);
    }
}
