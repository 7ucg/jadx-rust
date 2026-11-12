//! `AddAndroidConstants`: insert the constants of the Android resource id mapping as global
//! const fields (`android.R.*`), unless the input defines `android.R` itself.

use crate::model::const_storage::FieldRef;
use crate::model::info::FieldInfo;
use crate::model::node::Root;
use crate::model::types::ArgType;
use jadx_input_api::ConstValue;

/// `<resource id as 8 hex digits>=<type>/<name>` per line.
static RES_MAP: &str = include_str!("../../resources/android-res-map.txt");

const R_CLS: &str = "android.R";
const SPLIT_POS: usize = 8;

pub fn add_android_constants(root: &mut Root) {
    if !root.const_storage.is_replace_enabled() || root.class_index(R_CLS).is_some() {
        return;
    }
    for line in RES_MAP.lines() {
        let Some((id_str, path)) = line
            .split_at_checked(SPLIT_POS)
            .and_then(|(a, b)| Some((a, b.get(1..)?)))
        else {
            continue;
        };
        let (Ok(id), Some((res_type, res_name))) =
            (i32::from_str_radix(id_str, 16), path.split_once('/'))
        else {
            continue;
        };
        let Ok(decl_class) = root
            .ctx
            .class_info(&ArgType::object(&format!("{R_CLS}${res_type}")))
        else {
            continue;
        };
        let field = FieldInfo {
            decl_class,
            name: res_name.to_string(),
            ty: ArgType::int(),
        };
        root.const_storage
            .add_global_const_field(FieldRef::Info(field), ConstValue::Int(id));
    }
}
