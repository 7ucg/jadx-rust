//! `TypeCompare`: relation between two types (narrower, wider, conflicting).

use crate::clsp::ClspGraph;
use crate::model::types::{ArgType, PrimitiveType, WildcardBound};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeCompareEnum {
    Equal,
    Narrow,
    /// same basic type with generic
    NarrowByGeneric,
    Wider,
    /// same basic type without generic
    WiderByGeneric,
    Conflict,
    /// same basic type, conflict in generics
    ConflictByGeneric,
    Unknown,
}

use TypeCompareEnum::*;

impl TypeCompareEnum {
    pub fn invert(self) -> Self {
        match self {
            Narrow => Wider,
            NarrowByGeneric => WiderByGeneric,
            Wider => Narrow,
            WiderByGeneric => NarrowByGeneric,
            o => o,
        }
    }
    pub fn is_equal(self) -> bool {
        self == Equal
    }
    pub fn is_wider(self) -> bool {
        matches!(self, Wider | WiderByGeneric)
    }
    pub fn is_wider_or_equal(self) -> bool {
        self.is_equal() || self.is_wider()
    }
    pub fn is_narrow(self) -> bool {
        matches!(self, Narrow | NarrowByGeneric)
    }
    pub fn is_narrow_or_equal(self) -> bool {
        self.is_equal() || self.is_narrow()
    }
    pub fn is_conflict(self) -> bool {
        matches!(self, Conflict | ConflictByGeneric)
    }
}

pub struct TypeCompare<'a> {
    clsp: &'a ClspGraph,
}

impl<'a> TypeCompare<'a> {
    pub fn new(clsp: &'a ClspGraph) -> Self {
        TypeCompare { clsp }
    }

    /// Compare two types and return the result for the first argument.
    pub fn compare_types(&self, first: &ArgType, second: &ArgType) -> TypeCompareEnum {
        if first == second {
            return Equal;
        }
        let first_known = first.is_type_known();
        let second_known = second.is_type_known();
        if first_known != second_known {
            return if first_known {
                self.compare_with_unknown(first, second)
            } else {
                self.compare_with_unknown(second, first).invert()
            };
        }
        let first_array = first.is_array();
        let second_array = second.is_array();
        if first_array != second_array {
            return if first_array {
                self.compare_array_with_other(second)
            } else {
                self.compare_array_with_other(first).invert()
            };
        }
        if first_array {
            return self.compare_types(
                first.array_element().expect("array"),
                second.array_element().expect("array"),
            );
        }
        if !first_known {
            let a = first.possible_types().len();
            let b = second.possible_types().len();
            return if a > b { Wider } else { Narrow };
        }
        let first_primitive = first.is_primitive();
        let second_primitive = second.is_primitive();
        let first_obj = first.is_object();
        let second_obj = second.is_object();
        if first_obj && second_obj {
            return self.compare_objects_no_pre_check(first, second);
        }
        if (first_obj && second_primitive) || (first_primitive && second_obj) {
            return Conflict;
        }
        if first_primitive && second_primitive {
            return compare_primitives(
                first.as_primitive().expect("primitive"),
                second.as_primitive().expect("primitive"),
            );
        }
        Conflict
    }

    pub fn compare_objects(&self, first: &ArgType, second: &ArgType) -> TypeCompareEnum {
        if first == second {
            return Equal;
        }
        self.compare_objects_no_pre_check(first, second)
    }

    fn compare_array_with_other(&self, other: &ArgType) -> TypeCompareEnum {
        if !other.is_type_known() {
            return if other.contains(PrimitiveType::Array) {
                Narrow
            } else {
                Conflict
            };
        }
        if other.is_object() {
            return if *other == ArgType::object_type() {
                Narrow
            } else {
                Conflict
            };
        }
        Conflict
    }

    fn compare_with_unknown(&self, known: &ArgType, unknown: &ArgType) -> TypeCompareEnum {
        if *unknown == ArgType::unknown_any() {
            return Narrow;
        }
        if *unknown == ArgType::unknown_object() && (known.is_object() || known.is_array()) {
            return Narrow;
        }
        if *known == ArgType::object_type() && unknown.is_array() {
            return Wider;
        }
        let known_primitive = if let Some(p) = known.as_primitive() {
            p
        } else if known.is_array() {
            PrimitiveType::Array
        } else {
            PrimitiveType::Object
        };
        if unknown.possible_types().contains(&known_primitive) {
            Narrow
        } else {
            Conflict
        }
    }

    fn is_instance_of(&self, ty: &ArgType, of: &ArgType) -> bool {
        if ty == of {
            return true;
        }
        if !ty.is_object() || !of.is_object() {
            return false;
        }
        match (ty.object_name(), of.object_name()) {
            (Some(a), Some(b)) => self.clsp.is_implements(&a, &b),
            _ => false,
        }
    }

    fn is_cls_known(&self, ty: &ArgType) -> bool {
        ty.is_object() && ty.object_name().is_some_and(|n| self.clsp.is_cls_known(&n))
    }

    fn compare_objects_no_pre_check(&self, first: &ArgType, second: &ArgType) -> TypeCompareEnum {
        let objects_equals = first.object_name() == second.object_name();
        let first_generic_type = first.is_generic_type();
        let second_generic_type = second.is_generic_type();
        if first_generic_type && second_generic_type && !objects_equals {
            return Conflict;
        }
        let first_generic = first.is_generic();
        let second_generic = second.is_generic();
        if first_generic_type || second_generic_type {
            let fw = first.wildcard_type();
            let sw = second.wildcard_type();
            if fw.is_some() || sw.is_some() {
                if fw.is_some()
                    && second_generic_type
                    && first.wildcard_bound() == Some(WildcardBound::Unbound)
                {
                    return Conflict;
                }
                if first_generic_type
                    && sw.is_some()
                    && second.wildcard_bound() == Some(WildcardBound::Unbound)
                {
                    return Conflict;
                }
            }
            return if first_generic_type {
                self.compare_generic_type_with_object(first, second)
            } else {
                self.compare_generic_type_with_object(second, first)
                    .invert()
            };
        }
        if objects_equals {
            if first_generic != second_generic {
                return if first_generic {
                    NarrowByGeneric
                } else {
                    WiderByGeneric
                };
            }
            if first.wildcard_bound().is_some() && second.wildcard_bound().is_some() {
                return self.compare_wildcard_types(first, second);
            }
            let fg = first.generic_types().unwrap_or(&[]);
            let sg = second.generic_types().unwrap_or(&[]);
            if fg.is_empty() || sg.is_empty() {
                if let (Some(fo), Some(so)) = (first.outer_type(), second.outer_type()) {
                    return self.compare_types(fo, so);
                }
            } else if fg.len() == sg.len() {
                for (a, b) in fg.iter().zip(sg) {
                    let res = self.compare_types(a, b);
                    if res != Equal {
                        return res;
                    }
                }
            }
        }
        let first_is_obj = *first == ArgType::object_type();
        if first_is_obj || *second == ArgType::object_type() {
            return if first_is_obj { Wider } else { Narrow };
        }
        if self.is_instance_of(first, second) {
            return Narrow;
        }
        if self.is_instance_of(second, first) {
            return Wider;
        }
        if !self.is_cls_known(first) || !self.is_cls_known(second) {
            return Unknown;
        }
        Conflict
    }

    fn compare_wildcard_types(&self, first: &ArgType, second: &ArgType) -> TypeCompareEnum {
        let fb = first.wildcard_bound();
        let sb = second.wildcard_bound();
        if fb == Some(WildcardBound::Unbound) {
            return Wider;
        }
        if sb == Some(WildcardBound::Unbound) {
            return Narrow;
        }
        let cmp = self.compare_types(
            first.wildcard_type().expect("wildcard"),
            second.wildcard_type().expect("wildcard"),
        );
        if fb == sb {
            cmp
        } else {
            Conflict
        }
    }

    fn compare_generic_type_with_object(
        &self,
        generic_type: &ArgType,
        obj: &ArgType,
    ) -> TypeCompareEnum {
        if obj.is_generic_type() {
            return self.compare_type_variables(generic_type, obj);
        }
        if obj.is_wildcard() {
            return ConflictByGeneric;
        }
        let root_object = *obj == ArgType::object_type();
        let extends = generic_type.extend_types();
        if extends.is_empty() {
            return if root_object { Narrow } else { Conflict };
        }
        if extends.contains(obj) || root_object {
            return Narrow;
        }
        for e in extends {
            let res = self.compare_objects_no_pre_check(e, obj);
            if !res.is_narrow() {
                return res;
            }
        }
        Narrow
    }

    fn compare_type_variables(&self, first: &ArgType, second: &ArgType) -> TypeCompareEnum {
        if first.object_name() == second.object_name() {
            let fe = remove_object(first.extend_types());
            let se = remove_object(second.extend_types());
            if fe == se {
                return Equal;
            }
            if fe.is_empty() {
                return Wider;
            }
            if se.is_empty() {
                return Narrow;
            }
            if fe.len() == 1 && se.len() == 1 {
                return self.compare_types(&fe[0], &se[0]);
            }
        }
        Conflict
    }
}

fn remove_object(types: &[ArgType]) -> Vec<ArgType> {
    types
        .iter()
        .filter(|t| **t != ArgType::object_type())
        .cloned()
        .collect()
}

fn width(t: PrimitiveType) -> i8 {
    match t {
        PrimitiveType::Byte => 0,
        PrimitiveType::Short => 1,
        PrimitiveType::Char => 2,
        PrimitiveType::Int => 3,
        PrimitiveType::Long => 4,
        PrimitiveType::Float => 5,
        PrimitiveType::Double => 6,
        _ => -1,
    }
}

fn compare_primitives(t1: PrimitiveType, t2: PrimitiveType) -> TypeCompareEnum {
    use PrimitiveType::*;
    if t1 == Boolean || t2 == Boolean || t1 == Void || t2 == Void {
        return if t1 == t2 { Equal } else { Conflict };
    }
    if (t1 == Byte || t1 == Short) && t2 == Char {
        return Wider;
    }
    let (w1, w2) = (width(t1), width(t2));
    if w1 > w2 {
        TypeCompareEnum::Wider
    } else if w1 < w2 {
        TypeCompareEnum::Narrow
    } else {
        Equal
    }
}
