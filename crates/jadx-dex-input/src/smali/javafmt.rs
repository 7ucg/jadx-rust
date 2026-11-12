//! Java-compatible number formatting (`Float.toString` / `Double.toString`).

/// Format shortest round-trip digits the way `Double.toString` does (JDK 19+).
fn java_fp(neg: bool, digits: &str, exp10: i32) -> String {
    // value = 0.d1d2... * 10^(exp10+1)  where `digits` = d1d2...dn and exp10 is the exponent of d1
    let n = digits.len() as i32;
    let mut s = String::new();
    if neg {
        s.push('-');
    }
    if (-3..7).contains(&exp10) {
        if exp10 >= 0 {
            let int_len = exp10 + 1;
            if n <= int_len {
                s.push_str(digits);
                for _ in 0..(int_len - n) {
                    s.push('0');
                }
                s.push_str(".0");
            } else {
                s.push_str(&digits[..int_len as usize]);
                s.push('.');
                s.push_str(&digits[int_len as usize..]);
            }
        } else {
            s.push_str("0.");
            for _ in 0..(-exp10 - 1) {
                s.push('0');
            }
            s.push_str(digits);
        }
    } else {
        s.push_str(&digits[..1]);
        s.push('.');
        if n > 1 {
            s.push_str(&digits[1..]);
        } else {
            s.push('0');
        }
        s.push('E');
        s.push_str(&exp10.to_string());
    }
    s
}

fn split_sci(sci: &str) -> (String, i32) {
    // Rust `{:e}` gives e.g. "1.2345e3" or "5e-1"
    let (mant, exp) = sci.split_once('e').expect("exponent");
    let digits: String = mant.chars().filter(|c| *c != '.' && *c != '-').collect();
    (digits, exp.parse().expect("exp int"))
}

/// Digits as Java renders them: the shortest decimal that round-trips, but with at least two
/// digits, and the candidate closest to the exact value (ties to even) when several exist.
fn java_digits<T: std::str::FromStr + PartialEq + Copy + std::fmt::LowerExp>(
    a: T,
) -> (String, i32) {
    let (digits, exp) = split_sci(&format!("{a:e}"));
    let n = digits.len().max(2);
    let closest = format!("{a:.prec$e}", prec = n - 1);
    if closest.parse::<T>().map(|v| v == a).unwrap_or(false) {
        let (d, e) = split_sci(&closest);
        let d = d.trim_end_matches('0');
        if !d.is_empty() {
            return (d.to_string(), e);
        }
    }
    (digits, exp)
}

pub fn double_to_string(v: f64) -> String {
    if v.is_nan() {
        return "NaN".into();
    }
    if v.is_infinite() {
        return if v > 0.0 {
            "Infinity".into()
        } else {
            "-Infinity".into()
        };
    }
    if v == 0.0 {
        return if v.is_sign_negative() {
            "-0.0".into()
        } else {
            "0.0".into()
        };
    }
    let (digits, exp) = java_digits(v.abs());
    java_fp(v < 0.0, &digits, exp)
}

pub fn float_to_string(v: f32) -> String {
    if v.is_nan() {
        return "NaN".into();
    }
    if v.is_infinite() {
        return if v > 0.0 {
            "Infinity".into()
        } else {
            "-Infinity".into()
        };
    }
    if v == 0.0 {
        return if v.is_sign_negative() {
            "-0.0".into()
        } else {
            "0.0".into()
        };
    }
    let (digits, exp) = java_digits(v.abs());
    java_fp(v < 0.0, &digits, exp)
}

// ---- "likely float/double" heuristic of the reference disassembler ----------------------------

/// Emulates `new DecimalFormat("0.####################E0").format(double)` (English locale).
fn sci_double(x: f64) -> String {
    if x.is_infinite() {
        return if x > 0.0 { "∞".into() } else { "-∞".into() };
    }
    if x == 0.0 {
        return if x.is_sign_negative() {
            "-0E0".into()
        } else {
            "0E0".into()
        };
    }
    let (digits, exp) = split_sci(&format!("{:e}", x.abs()));
    sci_from_digits(x < 0.0, &digits, exp)
}

fn sci_long(v: i64) -> String {
    if v == 0 {
        return "0E0".into();
    }
    let s = (v as i128).unsigned_abs().to_string();
    let exp = s.len() as i32 - 1;
    sci_from_digits(v < 0, &s, exp)
}

fn sci_from_digits(neg: bool, digits: &str, exp: i32) -> String {
    let trimmed = digits.trim_end_matches('0');
    let trimmed = if trimmed.is_empty() { "0" } else { trimmed };
    let mut s = String::new();
    if neg {
        s.push('-');
    }
    s.push_str(&trimmed[..1]);
    if trimmed.len() > 1 {
        s.push('.');
        s.push_str(&trimmed[1..]);
    }
    s.push('E');
    s.push_str(&exp.to_string());
    s
}

fn java_index_of(s: &str, pat: &str) -> i32 {
    s.find(pat).map(|i| i as i32).unwrap_or(-1)
}

/// Shortens runs of `000` / `999` in the mantissa, then reports whether the float rendering is
/// shorter than the integer rendering.
fn shorter_than(as_value: &str, as_int: &str) -> bool {
    let mut v = as_value.to_string();
    let dec = java_index_of(&v, ".");
    let exp = java_index_of(&v, "E");
    let zeros = java_index_of(&v, "000");
    if zeros > dec && zeros < exp {
        v = format!("{}{}", &v[..zeros as usize], &v[exp as usize..]);
    } else {
        let nines = java_index_of(&v, "999");
        if nines > dec && nines < exp {
            v = format!("{}{}", &v[..nines as usize], &v[exp as usize..]);
        }
    }
    v.chars().count() < as_int.chars().count()
}

pub fn is_likely_float(value: i32) -> bool {
    let bits = value as u32;
    if bits == 0x7fc0_0000
        || bits == f32::MAX.to_bits()
        || bits == std::f32::consts::PI.to_bits()
        || bits == std::f32::consts::E.to_bits()
    {
        return true;
    }
    if value == i32::MAX || value == i32::MIN {
        return false;
    }
    let package_id = value >> 24;
    let resource_type = (value >> 16) & 255;
    let resource_id = value & 65535;
    if (package_id == 127 || package_id == 1) && resource_type < 31 && resource_id < 4095 {
        return false;
    }
    let f = f32::from_bits(bits);
    if f.is_nan() {
        return false;
    }
    shorter_than(&sci_double(f as f64), &sci_long(value as i64))
}

pub fn is_likely_double(value: i64) -> bool {
    let bits = value as u64;
    if bits == 0x7ff8_0000_0000_0000
        || bits == f64::MAX.to_bits()
        || bits == std::f64::consts::PI.to_bits()
        || bits == std::f64::consts::E.to_bits()
    {
        return true;
    }
    if value == i64::MAX || value == i64::MIN {
        return false;
    }
    let d = f64::from_bits(bits);
    if d.is_nan() {
        return false;
    }
    shorter_than(&sci_double(d), &sci_long(value))
}

/// Comment text (without the leading `    # `) for a literal that looks like a float.
pub fn float_comment(value: i32) -> Option<String> {
    if !is_likely_float(value) {
        return None;
    }
    let f = f32::from_bits(value as u32);
    Some(if f == f32::INFINITY {
        "Float.POSITIVE_INFINITY".into()
    } else if f == f32::NEG_INFINITY {
        "Float.NEGATIVE_INFINITY".into()
    } else if f.is_nan() {
        "Float.NaN".into()
    } else if f == f32::MAX {
        "Float.MAX_VALUE".into()
    } else if f == std::f32::consts::PI {
        "(float)Math.PI".into()
    } else if f == std::f32::consts::E {
        "(float)Math.E".into()
    } else {
        format!("{}f", float_to_string(f))
    })
}

pub fn double_comment(value: i64) -> Option<String> {
    if !is_likely_double(value) {
        return None;
    }
    let d = f64::from_bits(value as u64);
    Some(if d == f64::INFINITY {
        "Double.POSITIVE_INFINITY".into()
    } else if d == f64::NEG_INFINITY {
        "Double.NEGATIVE_INFINITY".into()
    } else if d.is_nan() {
        "Double.NaN".into()
    } else if d == f64::MAX {
        "Double.MAX_VALUE".into()
    } else if d == std::f64::consts::PI {
        "Math.PI".into()
    } else if d == std::f64::consts::E {
        "Math.E".into()
    } else {
        double_to_string(d)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floats() {
        assert_eq!(float_to_string(1.0), "1.0");
        assert_eq!(float_to_string(0.5), "0.5");
        assert_eq!(float_to_string(-2.25), "-2.25");
        assert_eq!(float_to_string(1.0e7), "1.0E7");
        assert_eq!(float_to_string(9999999.0), "9999999.0");
        assert_eq!(float_to_string(0.001), "0.001");
        assert_eq!(float_to_string(0.0001), "1.0E-4");
        assert_eq!(float_to_string(3.4028235e38), "3.4028235E38");
        assert_eq!(float_to_string(100.0), "100.0");
        assert_eq!(float_to_string(f32::NAN), "NaN");
        assert_eq!(float_to_string(f32::from_bits(1)), "1.4E-45");
        assert_eq!(float_to_string(2.0), "2.0");
        assert_eq!(float_to_string(f32::from_bits(0x473f6790)), "48999.562");
    }

    #[test]
    fn doubles() {
        assert_eq!(double_to_string(1.0), "1.0");
        assert_eq!(double_to_string(123456.789), "123456.789");
        assert_eq!(double_to_string(1.0e21), "1.0E21");
        assert_eq!(double_to_string(1.2345e-5), "1.2345E-5");
        assert_eq!(double_to_string(f64::INFINITY), "Infinity");
        assert_eq!(double_to_string(-0.0), "-0.0");
        assert_eq!(double_to_string(f64::from_bits(1)), "4.9E-324");
        assert_eq!(double_to_string(1.0e23), "1.0E23");
    }
}
