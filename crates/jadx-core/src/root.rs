//! Root context: the set of classes defined in the input, used to resolve class names
//! (inner class detection depends on whether the outer class exists).

use std::collections::{HashMap, HashSet};

use crate::model::info::ClassInfo;
use crate::model::types::ArgType;

/// Known classes of the loaded input.
#[derive(Debug, Clone, Default)]
pub struct RootCtx {
    /// Dotted raw names of all defined classes (`com.a.Outer$Inner`).
    defined: HashSet<String>,
    /// Method names per defined class (used to detect `Cls$method$1` style classes).
    method_names: HashMap<String, HashSet<String>>,
    /// Root option `moveInnerClasses` (default true).
    pub move_inner_classes: bool,
}

impl RootCtx {
    pub fn new(move_inner_classes: bool) -> Self {
        RootCtx {
            defined: HashSet::new(),
            method_names: HashMap::new(),
            move_inner_classes,
        }
    }

    pub fn add_class(&mut self, descriptor: &str) {
        self.defined
            .insert(crate::model::types::clean_object_name(descriptor));
    }

    pub fn add_method_name(&mut self, descriptor: &str, name: &str) {
        self.method_names
            .entry(crate::model::types::clean_object_name(descriptor))
            .or_default()
            .insert(name.to_string());
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
            // outer class missing: maybe the "outer" is a method of the real outer class
            let naive = ClassInfo::from_type(ty, true).map_err(|e| e.0)?;
            if let Some(par) = naive.parent_class() {
                let par_full = par.full_name();
                if let Some(dot) = par_full
                    .rfind('.')
                    .filter(|d| *d > 0 && *d != par_full.len() - 1)
                {
                    let (upper, mth) = (&par_full[..dot], &par_full[dot + 1..]);
                    if self
                        .method_names
                        .get(upper)
                        .is_some_and(|m| m.contains(mth))
                    {
                        let upper_info = self.class_info(&ArgType::object(upper))?;
                        return ClassInfo::new_converted_inner(ty, upper_info, true)
                            .map_err(|e| e.0);
                    }
                }
            }
            return Ok(norm);
        }
        let parent = self.class_info(&ArgType::object(&parent_raw))?;
        ClassInfo::new_inner(norm.arg_type(), parent).map_err(|e| e.0)
    }
}
