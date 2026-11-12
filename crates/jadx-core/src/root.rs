//! Root context: the set of classes defined in the input, used to resolve class names
//! (inner class detection depends on whether the outer class exists).

use std::collections::HashSet;

use crate::model::info::ClassInfo;
use crate::model::types::ArgType;

/// Known classes of the loaded input.
#[derive(Debug, Clone, Default)]
pub struct RootCtx {
    /// Dotted raw names of all defined classes (`com.a.Outer$Inner`).
    defined: HashSet<String>,
    /// Root option `moveInnerClasses` (default true).
    pub move_inner_classes: bool,
}

impl RootCtx {
    pub fn new(move_inner_classes: bool) -> Self {
        RootCtx {
            defined: HashSet::new(),
            move_inner_classes,
        }
    }

    pub fn add_class(&mut self, descriptor: &str) {
        self.defined
            .insert(crate::model::types::clean_object_name(descriptor));
    }

    pub fn is_defined(&self, raw_name: &str) -> bool {
        self.defined.contains(raw_name)
    }

    /// Class info for a class descriptor / type. A defined class is an inner class only if its
    /// outer class is defined as well; other (external) classes use plain `$` splitting.
    pub fn class_info(&self, ty: &ArgType) -> Result<ClassInfo, String> {
        let norm = ClassInfo::from_type(ty, false).map_err(|e| e.0)?;
        let raw = norm.raw_name();
        if !self.move_inner_classes {
            return Ok(norm);
        }
        let (pkg, short) = match raw.rfind('.') {
            None => ("", raw.as_str()),
            Some(d) => (&raw[..d], &raw[d + 1..]),
        };
        let sep = short.rfind('$');
        let Some(sep) = sep.filter(|s| *s > 0 && *s != short.len() - 1) else {
            return Ok(norm);
        };
        let parent_raw = if pkg.is_empty() {
            short[..sep].to_string()
        } else {
            format!("{pkg}.{}", &short[..sep])
        };
        if self.is_defined(&raw) && !self.is_defined(&parent_raw) {
            return Ok(norm);
        }
        let parent = self.class_info(&ArgType::object(&parent_raw))?;
        ClassInfo::new_inner(norm.arg_type(), parent).map_err(|e| e.0)
    }
}
