//! Class path graph: hierarchy and method details of framework classes (loaded from the bundled
//! `core.jcst` set) plus the classes of the input.

use std::collections::{HashMap, HashSet};

use crate::model::node::Root;
use crate::model::types::{ArgType, WildcardBound, CLASS_OBJECT};

static CORE_SET: &[u8] = include_bytes!("../resources/core.jcst");

#[derive(Debug, Clone)]
pub struct ClspMethod {
    /// Raw name of the declaring class.
    pub decl_class: String,
    pub name: String,
    /// Erased (descriptor) argument types.
    pub raw_arg_types: Vec<ArgType>,
    pub raw_return_type: ArgType,
    /// Argument types with generics (same as raw when no generic info).
    pub arg_types: Vec<ArgType>,
    pub return_type: ArgType,
    pub type_params: Vec<ArgType>,
    pub access_flags: u32,
    pub throws: Vec<ArgType>,
}

impl ClspMethod {
    pub fn short_id(&self) -> String {
        crate::model::info::make_short_id(
            &self.name,
            &self.raw_arg_types,
            Some(&self.raw_return_type),
        )
    }
}

#[derive(Debug, Clone)]
pub struct ClspClass {
    pub name: String,
    pub access_flags: u32,
    /// `None` when the parents are unknown.
    pub parents: Option<Vec<ArgType>>,
    pub type_params: Vec<ArgType>,
    pub methods: Vec<ClspMethod>,
    methods_map: HashMap<String, usize>,
}

impl ClspClass {
    pub fn parents(&self) -> &[ArgType] {
        self.parents.as_deref().unwrap_or(&[])
    }

    pub fn method_by_short_id(&self, id: &str) -> Option<&ClspMethod> {
        self.methods_map.get(id).map(|i| &self.methods[*i])
    }

    /// Methods sorted by short id (as stored in the class set).
    pub fn methods_map_iter(&self) -> impl Iterator<Item = (&String, &ClspMethod)> {
        self.methods_map.iter().map(|(k, i)| (k, &self.methods[*i]))
    }

    fn set_methods(&mut self, methods: Vec<ClspMethod>) {
        self.methods_map = methods
            .iter()
            .enumerate()
            .map(|(i, m)| (m.short_id(), i))
            .collect();
        self.methods = methods;
    }
}

struct ByteReader<'a> {
    d: &'a [u8],
    p: usize,
}

impl<'a> ByteReader<'a> {
    fn u8(&mut self) -> Result<u8, String> {
        let b = *self.d.get(self.p).ok_or("class set truncated")?;
        self.p += 1;
        Ok(b)
    }
    fn i8(&mut self) -> Result<i8, String> {
        Ok(self.u8()? as i8)
    }
    fn u32(&mut self) -> Result<u32, String> {
        let s = self
            .d
            .get(self.p..self.p + 4)
            .ok_or("class set truncated")?;
        self.p += 4;
        Ok(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn i16(&mut self) -> Result<i16, String> {
        let s = self
            .d
            .get(self.p..self.p + 2)
            .ok_or("class set truncated")?;
        self.p += 2;
        Ok(i16::from_be_bytes([s[0], s[1]]))
    }
    fn string(&mut self) -> Result<String, String> {
        let len = self.u8()? as usize;
        let s = self
            .d
            .get(self.p..self.p + len)
            .ok_or("class set truncated")?;
        self.p += len;
        Ok(String::from_utf8_lossy(s).into_owned())
    }
}

fn read_type(r: &mut ByteReader<'_>, names: &[String]) -> Result<Option<ArgType>, String> {
    let ord = r.i8()?;
    if ord == -1 {
        return Ok(None);
    }
    Ok(Some(match ord {
        0 => {
            let bound = r.i8()?;
            if bound == 0 {
                ArgType::wildcard_unbound()
            } else {
                let obj = read_type(r, names)?.ok_or("wildcard type missing")?;
                let b = if bound == 1 {
                    WildcardBound::Extends
                } else {
                    WildcardBound::Super
                };
                ArgType::Wildcard {
                    ty: Box::new(obj),
                    bound: b,
                }
            }
        }
        1 => {
            let cls = names.get(r.u32()? as usize).ok_or("bad class id")?.clone();
            let generics = read_type_list(r, names)?;
            ArgType::Generic {
                name: cls,
                generics,
            }
        }
        2 => {
            let name = r.string()?;
            let extends = read_type_list(r, names)?;
            ArgType::GenericType {
                name,
                extend_types: extends,
            }
        }
        3 => {
            let outer = read_type(r, names)?.ok_or("outer type missing")?;
            let inner = read_type(r, names)?.ok_or("inner type missing")?;
            ArgType::OuterGeneric {
                outer: Box::new(outer),
                inner: Box::new(inner),
            }
        }
        4 => ArgType::Object(names.get(r.u32()? as usize).ok_or("bad class id")?.clone()),
        5 => ArgType::array(read_type(r, names)?.ok_or("array element missing")?),
        6 => ArgType::parse_char(r.u8()? as char).map_err(|e| e.0)?,
        o => return Err(format!("unsupported type tag {o}")),
    }))
}

fn read_type_list(r: &mut ByteReader<'_>, names: &[String]) -> Result<Vec<ArgType>, String> {
    let count = r.i8()?;
    let mut v = Vec::new();
    for _ in 0..count.max(0) {
        v.push(read_type(r, names)?.ok_or("null type in list")?);
    }
    Ok(v)
}

fn read_method(r: &mut ByteReader<'_>, names: &[String]) -> Result<ClspMethod, String> {
    let name = r.string()?;
    let raw_args = read_type_list(r, names)?;
    let raw_ret = read_type(r, names)?.ok_or("return type missing")?;
    let mut generic_args = read_type_list(r, names)?;
    if generic_args.is_empty() || generic_args == raw_args {
        generic_args = raw_args.clone();
    }
    let mut generic_ret = read_type(r, names)?.ok_or("generic return type missing")?;
    if generic_ret == raw_ret {
        generic_ret = raw_ret.clone();
    }
    let type_params = read_type_list(r, names)?;
    let access_flags = r.u32()?;
    let throws = read_type_list(r, names)?;
    Ok(ClspMethod {
        decl_class: String::new(),
        name,
        raw_arg_types: raw_args,
        raw_return_type: raw_ret,
        arg_types: generic_args,
        return_type: generic_ret,
        type_params,
        access_flags,
        throws,
    })
}

/// Classes known to the decompiler: framework set + input classes.
#[derive(Debug, Default)]
pub struct ClspGraph {
    name_map: HashMap<String, ClspClass>,
    super_types: HashMap<String, HashSet<String>>,
}

impl ClspGraph {
    /// Load the bundled framework class set.
    pub fn load_core() -> Result<ClspGraph, String> {
        let mut g = ClspGraph::default();
        g.load_set(CORE_SET)?;
        Ok(g)
    }

    fn load_set(&mut self, data: &[u8]) -> Result<(), String> {
        let mut r = ByteReader { d: data, p: 0 };
        if data.get(..8) != Some(b"jadx-cst") {
            return Err("Wrong jadx class set header".into());
        }
        r.p = 8;
        let version = r.i8()?;
        if version != 5 {
            return Err(format!("Wrong jadx class set version: {version}"));
        }
        let _api = r.u32()?;
        let count = r.u32()? as usize;
        let mut names = Vec::with_capacity(count);
        let mut flags = Vec::with_capacity(count);
        for _ in 0..count {
            flags.push(r.u32()?);
            let _source = r.u8()?;
            names.push(r.string()?);
        }
        for i in 0..count {
            let parents = {
                let c = r.i8()?;
                match c {
                    -1 => None,
                    -2 => Some(vec![ArgType::object_type()]),
                    0 => Some(Vec::new()),
                    n => {
                        let mut v = Vec::new();
                        for _ in 0..n {
                            v.push(read_type(&mut r, &names)?.ok_or("null parent")?);
                        }
                        Some(v)
                    }
                }
            };
            let type_params = read_type_list(&mut r, &names)?;
            let mcount = r.i16()?;
            let mut methods = Vec::new();
            for _ in 0..mcount.max(0) {
                let mut m = read_method(&mut r, &names)?;
                m.decl_class = names[i].clone();
                methods.push(m);
            }
            let mut cls = ClspClass {
                name: names[i].clone(),
                access_flags: flags[i],
                parents,
                type_params,
                methods: Vec::new(),
                methods_map: HashMap::new(),
            };
            cls.set_methods(methods);
            self.name_map.insert(names[i].clone(), cls);
        }
        Ok(())
    }

    /// Add the classes of the input (hierarchy only).
    pub fn add_app(&mut self, root: &Root) {
        for c in &root.classes {
            let name = c.info.raw_name();
            let parents = make_parents(c.super_class.as_ref(), &c.interfaces);
            self.name_map.insert(
                name.clone(),
                ClspClass {
                    name,
                    access_flags: c.access,
                    parents: Some(parents),
                    type_params: Vec::new(),
                    methods: Vec::new(),
                    methods_map: HashMap::new(),
                },
            );
        }
    }

    /// Build the super types cache (call after all classes are added).
    pub fn init_cache(&mut self) {
        let names: Vec<String> = self.name_map.keys().cloned().collect();
        let mut cache = HashMap::with_capacity(names.len());
        for n in names {
            let mut set = HashSet::new();
            self.add_super_types(&self.name_map[&n], &mut set);
            cache.insert(n, set);
        }
        self.super_types = cache;
    }

    fn add_super_types(&self, cls: &ClspClass, result: &mut HashSet<String>) {
        for p in cls.parents() {
            let Some(pname) = p.object_name() else {
                continue;
            };
            match self.name_map.get(&pname) {
                Some(pc) => {
                    if result.insert(pc.name.clone()) {
                        self.add_super_types(pc, result);
                    }
                }
                None => {
                    result.insert(pname);
                }
            }
        }
    }

    pub fn is_cls_known(&self, full_name: &str) -> bool {
        self.name_map.contains_key(full_name)
    }

    pub fn cls_details(&self, name: &str) -> Option<&ClspClass> {
        self.name_map.get(name)
    }

    pub fn super_types(&self, name: &str) -> Option<&HashSet<String>> {
        self.super_types.get(name)
    }

    /// `cls_name` instanceof `impl_cls_name`
    pub fn is_implements(&self, cls_name: &str, impl_cls_name: &str) -> bool {
        self.super_types
            .get(cls_name)
            .is_some_and(|s| s.contains(impl_cls_name))
    }
}

/// `ClsSet.makeParentsArray`
pub fn make_parents(super_class: Option<&ArgType>, interfaces: &[ArgType]) -> Vec<ArgType> {
    let Some(sup) = super_class else {
        return Vec::new();
    };
    let mut v = Vec::with_capacity(1 + interfaces.len());
    v.push(sup.clone());
    v.extend(interfaces.iter().cloned());
    v
}

pub const OBJECT_NAME: &str = CLASS_OBJECT;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_set_loads() {
        let mut g = ClspGraph::load_core().unwrap();
        g.init_cache();
        assert!(g.is_cls_known("java.lang.String"));
        assert!(g.is_implements("java.util.ArrayList", "java.util.List"));
        assert!(g.is_implements("java.util.ArrayList", "java.lang.Object"));
        assert!(!g.is_implements("java.lang.String", "java.util.List"));
        let list = g.cls_details("java.util.List").unwrap();
        assert!(!list.type_params.is_empty());
    }
}
