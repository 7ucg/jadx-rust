//! `TypeUtils`: generic type variable mappings across class hierarchies.

use std::collections::HashMap;

use crate::model::node::Root;
use crate::model::types::ArgType;

pub type TypeMap = HashMap<ArgType, ArgType>;

/// `ClassTypeVarsAttr`: type variables of a class and their mapping in super types.
#[derive(Debug, Clone, Default)]
pub struct ClassTypeVars {
    pub type_vars: Vec<ArgType>,
    pub super_type_maps: HashMap<String, TypeMap>,
}

impl ClassTypeVars {
    pub fn type_vars_map_for(&self, ty: &ArgType) -> TypeMap {
        ty.object_name()
            .and_then(|n| self.super_type_maps.get(&n).cloned())
            .unwrap_or_default()
    }
}

pub struct TypeUtils<'a> {
    pub root: &'a Root,
}

impl<'a> TypeUtils<'a> {
    pub fn new(root: &'a Root) -> Self {
        TypeUtils { root }
    }

    /// `ClassNode.getType`: the class type with its type parameters.
    pub fn class_type(&self, cls: usize) -> ArgType {
        let c = &self.root.classes[cls];
        let raw = c.info.arg_type().clone();
        if c.generics.is_empty() {
            raw
        } else {
            ArgType::generic(&raw, c.generics.clone())
        }
    }

    /// `getKnownTypeVarsAtMethod`: type variables of the method, its class and outer classes.
    pub fn known_type_vars_at_method(&self, cls: usize, mth: usize) -> Vec<ArgType> {
        let mut vars: Vec<ArgType> = Vec::new();
        let mut add = |v: &ArgType| {
            if !vars.contains(v) {
                vars.push(v.clone());
            }
        };
        let c = &self.root.classes[cls];
        for g in &c.generics {
            add(g);
        }
        if c.is_inner() {
            let mut cur = self.root.parent_index(cls);
            while let Some(p) = cur {
                for g in &self.root.classes[p].generics {
                    add(g);
                }
                cur = self.root.parent_index(p);
            }
        }
        for g in &c.methods[mth].type_params {
            add(g);
        }
        vars
    }

    /// `expandTypeVariables(mth, type)`: type variables get the bounds of their declaration.
    pub fn expand_type_variables(&self, cls: usize, mth: usize, ty: &ArgType) -> ArgType {
        if !ty.contains_type_variable() {
            return ty.clone();
        }
        let vars = self.known_type_vars_at_method(cls, mth);
        if vars.is_empty() || vars.iter().all(|v| v.extend_types().is_empty()) {
            return ty.clone();
        }
        expand_vars(ty, &vars)
    }

    pub fn class_generics(&self, ty: &ArgType) -> Vec<ArgType> {
        if let Some(idx) = self.root.resolve_class(ty) {
            return self.root.classes[idx].generics.clone();
        }
        ty.object_name()
            .and_then(|n| self.root.clsp.cls_details(&n))
            .map(|c| c.type_params.clone())
            .unwrap_or_default()
    }

    /// Visit all (transitive) super types of the class; consumer gets `(child type, super type)`.
    pub fn visit_super_types_of_class(
        &self,
        idx: usize,
        consumer: &mut dyn FnMut(&ArgType, &ArgType),
    ) {
        let cls = &self.root.classes[idx];
        let this_type = cls.info.arg_type().clone();
        if let Some(sup) = &cls.super_class {
            if *sup != ArgType::object_type() {
                consumer(&this_type, sup);
                self.visit_super_types(sup, consumer);
            }
        }
        for iface in &cls.interfaces {
            consumer(&this_type, iface);
            self.visit_super_types(iface, consumer);
        }
    }

    pub fn visit_super_types(&self, ty: &ArgType, consumer: &mut dyn FnMut(&ArgType, &ArgType)) {
        if let Some(idx) = self.root.resolve_class(ty) {
            self.visit_super_types_of_class(idx, consumer);
            return;
        }
        let Some(details) = ty
            .object_name()
            .and_then(|n| self.root.clsp.cls_details(&n))
        else {
            return;
        };
        for sup in details.parents() {
            if *sup != ArgType::object_type() {
                consumer(ty, sup);
                self.visit_super_types(sup, consumer);
            }
        }
    }

    fn build_class_type_vars(&self, idx: usize) -> ClassTypeVars {
        let mut map: HashMap<String, TypeMap> = HashMap::new();
        let cls = &self.root.classes[idx];
        let current = cls.info.arg_type().clone();
        map.insert(
            current.object_name().unwrap_or_default(),
            self.type_variables_mapping(&current),
        );
        let mut pairs: Vec<(ArgType, ArgType)> = Vec::new();
        self.visit_super_types_of_class(idx, &mut |p, t| pairs.push((p.clone(), t.clone())));
        for (parent, ty) in pairs {
            let Some(current_vars) = ty
                .generic_types()
                .filter(|g| !g.is_empty())
                .map(|g| g.to_vec())
            else {
                continue;
            };
            let source_vars = self.class_generics(&ty);
            if current_vars.len() != source_vars.len() {
                continue;
            }
            let parent_map = map.get(&parent.object_name().unwrap_or_default()).cloned();
            let mut vars_map = TypeMap::new();
            for (i, cv) in current_vars.iter().enumerate() {
                let result = parent_map.as_ref().and_then(|m| m.get(cv).cloned());
                vars_map.insert(source_vars[i].clone(), result.unwrap_or_else(|| cv.clone()));
            }
            map.insert(ty.object_name().unwrap_or_default(), vars_map);
        }
        ClassTypeVars {
            type_vars: cls.generics.clone(),
            super_type_maps: map,
        }
    }

    pub fn class_type_vars(&self, ty: &ArgType) -> Option<ClassTypeVars> {
        let idx = self.root.resolve_class(ty)?;
        Some(self.build_class_type_vars(idx))
    }

    pub fn type_variables_mapping(&self, cls_type: &ArgType) -> TypeMap {
        if !cls_type.is_generic() {
            return TypeMap::new();
        }
        let params = self.class_generics(cls_type);
        if params.is_empty() {
            return TypeMap::new();
        }
        let actual = match cls_type.generic_types() {
            Some(a) if !a.is_empty() => a.to_vec(),
            _ => return TypeMap::new(),
        };
        if actual.len() != params.len() {
            return TypeMap::new();
        }
        let mut map = TypeMap::new();
        for (a, p) in actual.into_iter().zip(params) {
            let key = ArgType::generic_type(&p.object_name().unwrap_or_default());
            map.insert(key, a);
        }
        map
    }

    /// Replace class type variables in `type_with_generic` using the generics of `instance_type`.
    pub fn replace_class_generics(
        &self,
        instance_type: &ArgType,
        type_with_generic: &ArgType,
    ) -> Option<ArgType> {
        let mut map = TypeMap::new();
        if let Some(vars) = self.class_type_vars(instance_type) {
            map = merge_type_maps(map, vars.type_vars_map_for(instance_type));
        }
        map = merge_type_maps(map, self.type_variables_mapping(instance_type));
        let mut outer = instance_type.outer_type().cloned();
        while let Some(o) = outer {
            map = merge_type_maps(map, self.type_variables_mapping(&o));
            outer = o.outer_type().cloned();
        }
        replace_type_variables_using_map(type_with_generic, &map)
    }
}

fn merge_type_maps(base: TypeMap, mut addition: TypeMap) -> TypeMap {
    if base.is_empty() {
        return addition;
    }
    if addition.is_empty() {
        return base;
    }
    let mut map = TypeMap::with_capacity(base.len() + addition.len());
    for (k, v) in base {
        match addition.remove(&v) {
            Some(t) => map.insert(k, t),
            None => map.insert(k, v),
        };
    }
    map.extend(addition);
    map
}

pub fn replace_type_variables_using_map(replace_type: &ArgType, map: &TypeMap) -> Option<ArgType> {
    if map.is_empty() {
        return None;
    }
    if replace_type.is_generic_type() {
        return map.get(replace_type).cloned();
    }
    if let Some(elem) = replace_type.array_element() {
        return replace_type_variables_using_map(elem, map).map(ArgType::array);
    }
    if let Some(w) = replace_type.wildcard_type() {
        if w.contains_type_variable() {
            let nw = replace_type_variables_using_map(w, map)?;
            return Some(ArgType::Wildcard {
                ty: Box::new(nw),
                bound: replace_type.wildcard_bound()?,
            });
        }
    }
    if replace_type.is_generic() {
        if let Some(outer) = replace_type.outer_type() {
            let replaced_outer = replace_type_variables_using_map(outer, map)?;
            let inner = replace_type.inner_type()?;
            let replaced_inner = replace_type_variables_using_map(inner, map);
            return Some(ArgType::OuterGeneric {
                outer: Box::new(replaced_outer),
                inner: Box::new(replaced_inner.unwrap_or_else(|| inner.clone())),
            });
        }
        if let Some(generics) = replace_type.generic_types().filter(|g| !g.is_empty()) {
            let new_types: Vec<ArgType> = generics
                .iter()
                .map(|t| replace_type_variables_using_map(t, map).unwrap_or_else(|| t.clone()))
                .collect();
            return Some(ArgType::generic(replace_type, new_types));
        }
    }
    None
}

fn expand_vars(ty: &ArgType, vars: &[ArgType]) -> ArgType {
    match ty {
        ArgType::GenericType { name, .. } => {
            for v in vars {
                if let ArgType::GenericType {
                    name: vn,
                    extend_types,
                } = v
                {
                    if vn == name {
                        return ArgType::GenericType {
                            name: name.clone(),
                            extend_types: extend_types.clone(),
                        };
                    }
                }
            }
            ty.clone()
        }
        ArgType::Generic { name, generics } => ArgType::Generic {
            name: name.clone(),
            generics: generics.iter().map(|g| expand_vars(g, vars)).collect(),
        },
        ArgType::OuterGeneric { outer, inner } => ArgType::OuterGeneric {
            // the reference visits only the generics of the inner type
            outer: outer.clone(),
            inner: Box::new(expand_vars(inner, vars)),
        },
        ArgType::Array(e) => ArgType::Array(Box::new(expand_vars(e, vars))),
        ArgType::Wildcard { ty: inner, bound } => ArgType::Wildcard {
            ty: Box::new(expand_vars(inner, vars)),
            bound: *bound,
        },
        other => other.clone(),
    }
}
