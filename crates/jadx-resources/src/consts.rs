//! Chunk types and value constants of the binary resource formats
//! (see `frameworks/base/libs/androidfw/include/androidfw/ResourceTypes.h`).

pub const ANDROID_NS_URL: &str = "http://schemas.android.com/apk/res/android";
pub const ANDROID_NS_VALUE: &str = "android";

pub const RES_NULL_TYPE: i32 = 0x0000;
pub const RES_STRING_POOL_TYPE: i32 = 0x0001;
pub const RES_TABLE_TYPE: i32 = 0x0002;

pub const RES_XML_START_NAMESPACE_TYPE: i32 = 0x0100;
pub const RES_XML_END_NAMESPACE_TYPE: i32 = 0x0101;
pub const RES_XML_START_ELEMENT_TYPE: i32 = 0x0102;
pub const RES_XML_END_ELEMENT_TYPE: i32 = 0x0103;
pub const RES_XML_CDATA_TYPE: i32 = 0x0104;
pub const RES_XML_RESOURCE_MAP_TYPE: i32 = 0x0180;

pub const RES_TABLE_PACKAGE_TYPE: i32 = 0x0200;
pub const RES_TABLE_TYPE_TYPE: i32 = 0x0201;
pub const RES_TABLE_TYPE_SPEC_TYPE: i32 = 0x0202;
pub const RES_TABLE_TYPE_LIBRARY: i32 = 0x0203;
pub const RES_TABLE_TYPE_OVERLAY: i32 = 0x0204;
pub const RES_TABLE_TYPE_OVERLAY_POLICY: i32 = 0x0205;
pub const RES_TABLE_TYPE_STAGED_ALIAS: i32 = 0x0206;

pub const TYPE_NULL: i32 = 0x00;
pub const TYPE_REFERENCE: i32 = 0x01;
pub const TYPE_ATTRIBUTE: i32 = 0x02;
pub const TYPE_STRING: i32 = 0x03;
pub const TYPE_FLOAT: i32 = 0x04;
pub const TYPE_DIMENSION: i32 = 0x05;
pub const TYPE_FRACTION: i32 = 0x06;
pub const TYPE_DYNAMIC_REFERENCE: i32 = 0x07;
pub const TYPE_DYNAMIC_ATTRIBUTE: i32 = 0x08;
pub const TYPE_INT_DEC: i32 = 0x10;
pub const TYPE_INT_HEX: i32 = 0x11;
pub const TYPE_INT_BOOLEAN: i32 = 0x12;
pub const TYPE_INT_COLOR_ARGB8: i32 = 0x1c;
pub const TYPE_INT_COLOR_RGB8: i32 = 0x1d;
pub const TYPE_INT_COLOR_ARGB4: i32 = 0x1e;
pub const TYPE_INT_COLOR_RGB4: i32 = 0x1f;

pub const COMPLEX_UNIT_MASK: i32 = 0xf;
pub const COMPLEX_UNIT_PX: i32 = 0;
pub const COMPLEX_UNIT_DIP: i32 = 1;
pub const COMPLEX_UNIT_SP: i32 = 2;
pub const COMPLEX_UNIT_PT: i32 = 3;
pub const COMPLEX_UNIT_IN: i32 = 4;
pub const COMPLEX_UNIT_MM: i32 = 5;
pub const COMPLEX_UNIT_FRACTION: i32 = 0;
pub const COMPLEX_UNIT_FRACTION_PARENT: i32 = 1;
pub const COMPLEX_RADIX_SHIFT: i32 = 4;
pub const COMPLEX_RADIX_MASK: i32 = 0x3;
pub const COMPLEX_MANTISSA_SHIFT: i32 = 8;
pub const COMPLEX_MANTISSA_MASK: i32 = 0xff_ffff;

/// Multipliers of the four fixed point radix representations (exact powers of two).
pub const RADIX_MULTS: [f64; 4] = [
    1.0 / 256.0,
    1.0 / 128.0 / 256.0,
    1.0 / 32768.0 / 256.0,
    1.0 / 8_388_608.0 / 256.0,
];

pub const UTF8_FLAG: i32 = 1 << 8;

pub const NO_ENTRY: i32 = -1;
pub const FLAG_SPARSE: i32 = 0x01;
pub const FLAG_OFFSET16: i32 = 0x02;
pub const FLAG_COMPLEX: i32 = 0x0001;
pub const FLAG_COMPACT: i32 = 0x0008;

const fn make_res_internal(entry: i32) -> i32 {
    0x0100_0000 | (entry & 0xFFFF)
}

pub const ATTR_TYPE: i32 = make_res_internal(0);
pub const ATTR_MIN: i32 = make_res_internal(1);
pub const ATTR_OTHER: i32 = make_res_internal(4);
pub const ATTR_ZERO: i32 = make_res_internal(5);
pub const ATTR_ONE: i32 = make_res_internal(6);
pub const ATTR_TWO: i32 = make_res_internal(7);
pub const ATTR_FEW: i32 = make_res_internal(8);
pub const ATTR_MANY: i32 = make_res_internal(9);

/// Name of a plural quantity attribute id.
pub fn plural_name(id: i32) -> Option<&'static str> {
    match id {
        ATTR_OTHER => Some("other"),
        ATTR_ZERO => Some("zero"),
        ATTR_ONE => Some("one"),
        ATTR_TWO => Some("two"),
        ATTR_FEW => Some("few"),
        ATTR_MANY => Some("many"),
        _ => None,
    }
}

pub fn is_res_internal_id(res_id: i32) -> bool {
    (res_id as u32 & 0xFFFF_0000) != 0 && (res_id as u32 & 0xFF_0000) == 0
}

pub const ATTR_TYPE_ANY: i32 = 0x0000_FFFF;
pub const ATTR_TYPE_REFERENCE: i32 = 1;
pub const ATTR_TYPE_STRING: i32 = 1 << 1;
pub const ATTR_TYPE_INTEGER: i32 = 1 << 2;
pub const ATTR_TYPE_BOOLEAN: i32 = 1 << 3;
pub const ATTR_TYPE_COLOR: i32 = 1 << 4;
pub const ATTR_TYPE_FLOAT: i32 = 1 << 5;
pub const ATTR_TYPE_DIMENSION: i32 = 1 << 6;
pub const ATTR_TYPE_FRACTION: i32 = 1 << 7;
pub const ATTR_TYPE_ENUM: i32 = 1 << 16;
pub const ATTR_TYPE_FLAGS: i32 = 1 << 17;
