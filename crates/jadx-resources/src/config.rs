//! Resource configuration (`ResTable_config`) and its qualifier string (`-land`, `-v21`, ...).

use std::sync::atomic::{AtomicU32, Ordering};

/// Counter of invalid configurations, used to keep their qualifier strings unique (`-ERR0`).
static ERR_COUNTER: AtomicU32 = AtomicU32::new(0);

const MNC_ZERO: i16 = -1;

const DENSITY_LOW: i32 = 120;
const DENSITY_MEDIUM: i32 = 160;
const DENSITY_TV: i32 = 213;
const DENSITY_HIGH: i32 = 240;
const DENSITY_XHIGH: i32 = 320;
const DENSITY_XXHIGH: i32 = 480;
const DENSITY_XXXHIGH: i32 = 640;
const DENSITY_ANY: i32 = 0xFFFE;
const DENSITY_NONE: i32 = 0xFFFF;

const SDK_HONEYCOMB_MR2: i16 = 13;
const SDK_FROYO: i16 = 8;
const SDK_DONUT: i16 = 4;
const SDK_LOLLIPOP: i16 = 21;
const SDK_MNC: i16 = 23;
const SDK_OREO: i16 = 26;

/// Fields of a `ResTable_config` as read from the file.
#[derive(Debug, Clone, Default)]
pub struct RawConfig {
    pub mcc: i16,
    pub mnc: i16,
    pub language: Vec<u16>,
    pub region: Vec<u16>,
    pub orientation: i8,
    pub touchscreen: i8,
    pub density: i32,
    pub keyboard: i8,
    pub navigation: i8,
    pub input_flags: i8,
    pub grammatical_inflection: i8,
    pub screen_width: i16,
    pub screen_height: i16,
    pub sdk_version: i16,
    pub screen_layout: i8,
    pub ui_mode: i8,
    pub smallest_screen_width_dp: i16,
    pub screen_width_dp: i16,
    pub screen_height_dp: i16,
    pub locale_script: Option<Vec<u16>>,
    pub locale_variant: Option<Vec<u16>>,
    pub screen_layout2: i8,
    pub color_mode: i8,
    /// Size of the config structure in the file.
    pub size: i32,
}

/// Validated configuration with its qualifier string.
#[derive(Debug, Clone)]
pub struct EntryConfig {
    pub is_invalid: bool,
    qualifiers: String,
}

impl EntryConfig {
    pub fn from_raw(mut c: RawConfig) -> Self {
        let mut invalid = false;
        if !(0..=3).contains(&c.orientation) {
            c.orientation = 0;
            invalid = true;
        }
        if !(0..=3).contains(&c.touchscreen) {
            c.touchscreen = 0;
            invalid = true;
        }
        if c.density < -1 {
            c.density = 0;
            invalid = true;
        }
        if !(0..=3).contains(&c.keyboard) {
            c.keyboard = 0;
            invalid = true;
        }
        if !(0..=4).contains(&c.navigation) {
            c.navigation = 0;
            invalid = true;
        }
        if c.locale_script
            .as_ref()
            .is_some_and(|s| s.is_empty() || s[0] == 0)
        {
            c.locale_script = None;
        }
        if c.locale_variant
            .as_ref()
            .is_some_and(|s| s.is_empty() || s[0] == 0)
        {
            c.locale_variant = None;
        }
        let qualifiers = generate_qualifiers(&c, invalid);
        EntryConfig {
            is_invalid: invalid,
            qualifiers,
        }
    }

    pub fn qualifiers(&self) -> &str {
        &self.qualifiers
    }
}

fn push_chars(sb: &mut String, chars: &[u16]) {
    sb.push_str(&String::from_utf16_lossy(chars));
}

fn generate_qualifiers(c: &RawConfig, is_invalid: bool) -> String {
    let mut ret = String::new();
    if c.mcc != 0 {
        ret.push_str(&format!("-mcc{:03}", c.mcc));
        if c.mnc != MNC_ZERO {
            if c.mnc != 0 {
                ret.push_str("-mnc");
                if c.size <= 32 {
                    if c.mnc > 0 && c.mnc < 10 {
                        ret.push_str(&format!("{:02}", c.mnc));
                    } else {
                        ret.push_str(&format!("{:03}", c.mnc));
                    }
                } else {
                    ret.push_str(&c.mnc.to_string());
                }
            }
        } else {
            ret.push_str("-mnc00");
        }
    } else if c.mnc != 0 {
        ret.push_str(&format!("-mnc{}", c.mnc));
    }
    ret.push_str(&locale_string(c));

    match c.grammatical_inflection {
        1 => ret.push_str("-neuter"),
        2 => ret.push_str("-feminine"),
        3 => ret.push_str("-masculine"),
        _ => {}
    }

    let screen_layout = i32::from(c.screen_layout);
    match screen_layout & 0xc0 {
        0x80 => ret.push_str("-ldrtl"),
        0x40 => ret.push_str("-ldltr"),
        _ => {}
    }
    if c.smallest_screen_width_dp != 0 {
        ret.push_str(&format!("-sw{}dp", c.smallest_screen_width_dp));
    }
    if c.screen_width_dp != 0 {
        ret.push_str(&format!("-w{}dp", c.screen_width_dp));
    }
    if c.screen_height_dp != 0 {
        ret.push_str(&format!("-h{}dp", c.screen_height_dp));
    }
    match screen_layout & 0x0f {
        0x01 => ret.push_str("-small"),
        0x02 => ret.push_str("-normal"),
        0x03 => ret.push_str("-large"),
        0x04 => ret.push_str("-xlarge"),
        _ => {}
    }
    match screen_layout & 0x30 {
        0x20 => ret.push_str("-long"),
        0x10 => ret.push_str("-notlong"),
        _ => {}
    }
    match i32::from(c.screen_layout2) & 0x03 {
        0x1 => ret.push_str("-notround"),
        0x2 => ret.push_str("-round"),
        _ => {}
    }
    let color_mode = i32::from(c.color_mode);
    match color_mode & 0xC {
        0x8 => ret.push_str("-highdr"),
        0x4 => ret.push_str("-lowdr"),
        _ => {}
    }
    match color_mode & 0x3 {
        0x2 => ret.push_str("-widecg"),
        0x1 => ret.push_str("-nowidecg"),
        _ => {}
    }
    match c.orientation {
        1 => ret.push_str("-port"),
        2 => ret.push_str("-land"),
        3 => ret.push_str("-square"),
        _ => {}
    }
    let ui_mode = i32::from(c.ui_mode);
    match ui_mode & 0x0f {
        0x03 => ret.push_str("-car"),
        0x02 => ret.push_str("-desk"),
        0x04 => ret.push_str("-television"),
        0x0c => ret.push_str("-smallui"),
        0x0d => ret.push_str("-mediumui"),
        0x0e => ret.push_str("-largeui"),
        0x0b => ret.push_str("-godzillaui"),
        0x0f => ret.push_str("-hugeui"),
        0x05 => ret.push_str("-appliance"),
        0x06 => ret.push_str("-watch"),
        0x07 => ret.push_str("-vrheadset"),
        _ => {}
    }
    match ui_mode & 0x30 {
        0x20 => ret.push_str("-night"),
        0x10 => ret.push_str("-notnight"),
        _ => {}
    }
    match c.density {
        0 => {}
        DENSITY_LOW => ret.push_str("-ldpi"),
        DENSITY_MEDIUM => ret.push_str("-mdpi"),
        DENSITY_HIGH => ret.push_str("-hdpi"),
        DENSITY_TV => ret.push_str("-tvdpi"),
        DENSITY_XHIGH => ret.push_str("-xhdpi"),
        DENSITY_XXHIGH => ret.push_str("-xxhdpi"),
        DENSITY_XXXHIGH => ret.push_str("-xxxhdpi"),
        DENSITY_ANY => ret.push_str("-anydpi"),
        DENSITY_NONE => ret.push_str("-nodpi"),
        d => ret.push_str(&format!("-{d}dpi")),
    }
    match c.touchscreen {
        1 => ret.push_str("-notouch"),
        2 => ret.push_str("-stylus"),
        3 => ret.push_str("-finger"),
        _ => {}
    }
    let input_flags = i32::from(c.input_flags);
    match input_flags & 0x3 {
        0x1 => ret.push_str("-keysexposed"),
        0x2 => ret.push_str("-keyshidden"),
        0x3 => ret.push_str("-keyssoft"),
        _ => {}
    }
    match c.keyboard {
        1 => ret.push_str("-nokeys"),
        2 => ret.push_str("-qwerty"),
        3 => ret.push_str("-12key"),
        _ => {}
    }
    match input_flags & 0xc {
        0x4 => ret.push_str("-navexposed"),
        0x8 => ret.push_str("-navhidden"),
        _ => {}
    }
    match c.navigation {
        1 => ret.push_str("-nonav"),
        2 => ret.push_str("-dpad"),
        3 => ret.push_str("-trackball"),
        4 => ret.push_str("-wheel"),
        _ => {}
    }
    if c.screen_width != 0 && c.screen_height != 0 {
        if c.screen_width > c.screen_height {
            ret.push_str(&format!("-{}x{}", c.screen_width, c.screen_height));
        } else {
            ret.push_str(&format!("-{}x{}", c.screen_height, c.screen_width));
        }
    }
    if c.sdk_version > 0 && c.sdk_version >= natural_sdk_version_requirement(c) {
        ret.push_str(&format!("-v{}", c.sdk_version));
    }
    if is_invalid {
        let n = ERR_COUNTER.fetch_add(1, Ordering::Relaxed);
        ret.push_str(&format!("-ERR{n}"));
    }
    ret
}

fn natural_sdk_version_requirement(c: &RawConfig) -> i16 {
    let ui_mode = i32::from(c.ui_mode);
    let color_mode = i32::from(c.color_mode);
    if (ui_mode & 0x0f) == 0x07 || (color_mode & 0x3) != 0 || (color_mode & 0xC) != 0 {
        return SDK_OREO;
    }
    if (i32::from(c.screen_layout2) & 0x03) != 0 {
        return SDK_MNC;
    }
    if c.density == DENSITY_ANY {
        return SDK_LOLLIPOP;
    }
    if c.smallest_screen_width_dp != 0 || c.screen_width_dp != 0 || c.screen_height_dp != 0 {
        return SDK_HONEYCOMB_MR2;
    }
    if (ui_mode & 0x3f) != 0 {
        return SDK_FROYO;
    }
    if (i32::from(c.screen_layout) & 0x3f) != 0 || c.density != 0 {
        return SDK_DONUT;
    }
    0
}

fn locale_string(c: &RawConfig) -> String {
    let mut sb = String::new();
    let lang0 = c.language.first().copied().unwrap_or(0);
    let region0 = c.region.first().copied().unwrap_or(0);
    // old style non BCP47 tags: values-xx-rXX, values-xx, values-xxx-rXX
    if c.locale_variant.is_none()
        && c.locale_script.is_none()
        && (region0 != 0 || lang0 != 0)
        && c.region.len() != 3
    {
        sb.push('-');
        push_chars(&mut sb, &c.language);
        if region0 != 0 {
            sb.push_str("-r");
            push_chars(&mut sb, &c.region);
        }
    } else {
        if lang0 == 0 && region0 == 0 {
            return sb;
        }
        sb.push_str("-b+");
        if lang0 != 0 {
            push_chars(&mut sb, &c.language);
        }
        if let Some(script) = &c.locale_script {
            if script.len() == 4 {
                sb.push('+');
                push_chars(&mut sb, script);
            }
        }
        if (c.region.len() == 2 || c.region.len() == 3) && region0 != 0 {
            sb.push('+');
            push_chars(&mut sb, &c.region);
        }
        if let Some(variant) = &c.locale_variant {
            if variant.len() >= 5 {
                sb.push('+');
                let s = String::from_utf16_lossy(variant);
                sb.push_str(&s.to_uppercase());
            }
        }
    }
    sb
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> RawConfig {
        RawConfig {
            language: vec![0, 0],
            region: vec![0, 0],
            size: 52,
            ..RawConfig::default()
        }
    }

    #[test]
    fn default_config_is_empty() {
        assert_eq!(EntryConfig::from_raw(base()).qualifiers(), "");
    }

    #[test]
    fn locale_density_sdk() {
        let mut c = base();
        c.language = vec![b'd' as u16, b'e' as u16];
        c.region = vec![b'A' as u16, b'T' as u16];
        c.density = DENSITY_XXHIGH;
        c.sdk_version = 21;
        assert_eq!(EntryConfig::from_raw(c).qualifiers(), "-de-rAT-xxhdpi-v21");
    }

    #[test]
    fn bcp47_locale() {
        let mut c = base();
        c.language = vec![b'e' as u16, b's' as u16];
        c.region = vec![b'4' as u16, b'1' as u16, b'9' as u16];
        assert_eq!(EntryConfig::from_raw(c).qualifiers(), "-b+es+419");
    }

    #[test]
    fn night_and_anydpi() {
        let mut c = base();
        c.ui_mode = 0x20;
        c.density = DENSITY_ANY;
        c.sdk_version = 26;
        assert_eq!(EntryConfig::from_raw(c).qualifiers(), "-night-anydpi-v26");
    }
}
