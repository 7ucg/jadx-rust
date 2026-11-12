//! `ConstStorage`: static final fields with a constant value, used to replace literals by
//! field references.

use std::collections::{HashMap, HashSet};

use jadx_input_api::ConstValue;

use super::info::FieldInfo;
use super::node::Root;
use super::types::{ArgType, PrimitiveType};

/// Reference to a constant field (`IFieldInfoRef`): a field of the input or a bare field info.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldRef {
    /// `(class index, field index)` into `Root::classes`.
    Node(usize, usize),
    Info(FieldInfo),
}

#[derive(Debug, Default, Clone)]
struct ValueStorage {
    values: HashMap<ConstValue, FieldRef>,
    duplicates: HashSet<ConstValue>,
}

impl ValueStorage {
    /// Returns true if the value is duplicated.
    fn put(&mut self, value: ConstValue, fld: FieldRef) -> bool {
        if self.duplicates.contains(&value) {
            self.values.remove(&value);
            return true;
        }
        if self.values.contains_key(&value) {
            self.values.remove(&value);
            self.duplicates.insert(value);
            return true;
        }
        self.values.insert(value, fld);
        false
    }

    fn contains(&self, value: &ConstValue) -> bool {
        self.duplicates.contains(value) || self.values.contains_key(value)
    }

    fn remove_for_class(&mut self, cls: usize) {
        self.values
            .retain(|_, f| !matches!(f, FieldRef::Node(c, _) if *c == cls));
    }
}

#[derive(Debug, Clone)]
pub struct ConstStorage {
    replace_enabled: bool,
    global_values: ValueStorage,
    classes: HashMap<usize, ValueStorage>,
}

impl ConstStorage {
    pub fn new(replace_enabled: bool) -> Self {
        ConstStorage {
            replace_enabled,
            global_values: ValueStorage::default(),
            classes: HashMap::new(),
        }
    }

    pub fn is_replace_enabled(&self) -> bool {
        self.replace_enabled
    }

    pub fn set_replace_enabled(&mut self, enabled: bool) {
        self.replace_enabled = enabled;
    }

    pub fn add_const_field(&mut self, cls: usize, fld: usize, value: ConstValue, is_public: bool) {
        if is_public {
            self.add_global_const_field(FieldRef::Node(cls, fld), value);
        } else {
            self.classes
                .entry(cls)
                .or_default()
                .put(value, FieldRef::Node(cls, fld));
        }
    }

    pub fn add_global_const_field(&mut self, fld: FieldRef, value: ConstValue) {
        self.global_values.put(value, fld);
    }

    pub fn remove_for_class(&mut self, cls: usize) {
        self.classes.remove(&cls);
        self.global_values.remove_for_class(cls);
    }

    /// Values (and their fields) visible from all classes.
    pub fn global_const_fields(&self) -> &HashMap<ConstValue, FieldRef> {
        &self.global_values.values
    }

    /// Class local values: `(class index, value, field)`.
    pub fn class_const_fields(&self) -> impl Iterator<Item = (usize, &ConstValue, &FieldRef)> {
        self.classes
            .iter()
            .flat_map(|(c, s)| s.values.iter().map(move |(v, f)| (*c, v, f)))
    }

    /// Field with the given constant value that is visible in class `cls` (own and outer classes,
    /// then global values if `search_global`).
    pub fn get_const_field(
        &self,
        root: &Root,
        cls: usize,
        value: &ConstValue,
        search_global: bool,
    ) -> Option<&FieldRef> {
        if !self.replace_enabled {
            return None;
        }
        let found_in_global = self.global_values.contains(value);
        if found_in_global && !search_global {
            return None;
        }
        let mut current = Some(cls);
        while let Some(c) = current {
            if let Some(f) = self.classes.get(&c).and_then(|s| s.values.get(value)) {
                if found_in_global {
                    return None;
                }
                return Some(f);
            }
            current = root.parent_index(c).filter(|p| *p != c);
        }
        if search_global {
            return self.global_values.values.get(value);
        }
        None
    }

    /// `getConstFieldByLiteralArg`: constant field for a literal of the given type.
    pub fn get_const_field_by_literal(
        &self,
        root: &Root,
        cls: usize,
        literal: i64,
        ty: &ArgType,
    ) -> Option<&FieldRef> {
        if !self.replace_enabled {
            return None;
        }
        let big = |limit: i64| literal.wrapping_abs() > limit;
        let (value, search_global) = match ty.primitive_type()? {
            PrimitiveType::Boolean => (ConstValue::Boolean(literal == 1), false),
            PrimitiveType::Char => (ConstValue::Char(literal as u16), big(10)),
            PrimitiveType::Byte => (ConstValue::Byte(literal as i8), big(10)),
            PrimitiveType::Short => (ConstValue::Short(literal as i16), big(100)),
            PrimitiveType::Int => (ConstValue::Int(literal as i32), big(100)),
            PrimitiveType::Long => (ConstValue::Long(literal), big(1000)),
            PrimitiveType::Float => {
                let f = f32::from_bits(literal as i32 as u32);
                (ConstValue::from_f32(f), f.to_bits() == 0)
            }
            PrimitiveType::Double => {
                let d = f64::from_bits(literal as u64);
                (ConstValue::from_f64(d), d.to_bits() == 0)
            }
            _ => return None,
        };
        self.get_const_field(root, cls, &value, search_global)
    }
}
