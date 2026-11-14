//! Class, field and method descriptors (`ClassInfo`, `FieldInfo`, `MethodInfo`).

use std::fmt;
use std::sync::Arc;

use super::types::{ArgType, PrimitiveType};

/// Descriptor signature of a type (`I`, `[I`, `Ljava/lang/String;`).
pub fn type_signature(ty: &ArgType) -> String {
    match ty.primitive_type() {
        Some(PrimitiveType::Object) => format!(
            "L{};",
            ty.object_name().unwrap_or_default().replace('.', "/")
        ),
        Some(PrimitiveType::Array) => {
            format!(
                "[{}",
                ty.array_element().map(type_signature).unwrap_or_default()
            )
        }
        Some(p) => p.short_name().to_string(),
        None => String::from("?"),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
struct ClassInfoInner {
    ty: ArgType,
    name: String,
    /// `None` for inner classes.
    pkg: Option<String>,
    full_name: String,
    parent: Option<ClassInfo>,
}

/// Class descriptor with package/short name split and inner-class detection.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct ClassInfo(Arc<ClassInfoInner>);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, serde::Serialize, serde::Deserialize)]
#[error("{0}")]
pub struct InfoError(pub String);

impl ClassInfo {
    /// `can_be_inner`: treat `Outer$Inner` as inner class of `Outer` (jadx default: yes).
    pub fn from_type(ty: &ArgType, can_be_inner: bool) -> Result<ClassInfo, InfoError> {
        let cls_type = check_class_type(ty)?;
        Ok(ClassInfo(Arc::new(Self::split(cls_type, can_be_inner))))
    }

    pub fn from_name(name: &str, can_be_inner: bool) -> Result<ClassInfo, InfoError> {
        Self::from_type(&ArgType::object(name), can_be_inner)
    }

    /// Top level (or "not inner") class: the whole name after the last dot is the short name.
    pub fn new_top_level(ty: &ArgType) -> Result<ClassInfo, InfoError> {
        let ty = check_class_type(ty)?;
        let full = ty.object_name().unwrap_or_default();
        let (pkg, name) = match full.rfind('.') {
            None => (String::new(), full.clone()),
            Some(dot) => (full[..dot].to_string(), full[dot + 1..].to_string()),
        };
        let full_name = make_full_name(&pkg, &name, None, false);
        Ok(ClassInfo(Arc::new(ClassInfoInner {
            ty,
            name,
            pkg: Some(pkg),
            full_name,
            parent: None,
        })))
    }

    /// Inner class of `parent`; the short name is the part after the last `$`.
    pub fn new_inner(ty: &ArgType, parent: ClassInfo) -> Result<ClassInfo, InfoError> {
        let ty = check_class_type(ty)?;
        let full = ty.object_name().unwrap_or_default();
        let cls_name = full.rsplit('.').next().unwrap_or(&full).to_string();
        let name = match cls_name.rfind('$') {
            Some(sep) if sep > 0 && sep != cls_name.len() - 1 => cls_name[sep + 1..].to_string(),
            _ => cls_name,
        };
        let full_name = make_full_name("", &name, Some(&parent), false);
        Ok(ClassInfo(Arc::new(ClassInfoInner {
            ty,
            name,
            pkg: None,
            full_name,
            parent: Some(parent),
        })))
    }

    /// Class converted to an inner class of `parent` although its naive outer class does not
    /// exist (`Cls$mth$1` becomes inner of `Cls`). The full name keeps the naive structure.
    pub fn new_converted_inner(
        ty: &ArgType,
        parent: ClassInfo,
        can_be_inner: bool,
    ) -> Result<ClassInfo, InfoError> {
        let ty = check_class_type(ty)?;
        let naive = Self::split(ty.clone(), can_be_inner);
        Ok(ClassInfo(Arc::new(ClassInfoInner {
            ty,
            name: naive.name,
            pkg: None,
            full_name: naive.full_name,
            parent: Some(parent),
        })))
    }

    fn split(ty: ArgType, can_be_inner: bool) -> ClassInfoInner {
        let full_object_name = ty.object_name().unwrap_or_default();
        let (cls_pkg, mut cls_name) = match full_object_name.rfind('.') {
            None => (String::new(), full_object_name.clone()),
            Some(dot) => (
                full_object_name[..dot].to_string(),
                full_object_name[dot + 1..].to_string(),
            ),
        };
        let mut parent = None;
        let mut pkg = Some(cls_pkg.clone());
        if can_be_inner {
            if let Some(sep) = cls_name.rfind('$') {
                if sep > 0 && sep != cls_name.len() - 1 {
                    let par_name = if cls_pkg.is_empty() {
                        cls_name[..sep].to_string()
                    } else {
                        format!("{}.{}", cls_pkg, &cls_name[..sep])
                    };
                    if let Ok(p) = Self::from_name(&par_name, can_be_inner) {
                        parent = Some(p);
                        pkg = None;
                        cls_name = cls_name[sep + 1..].to_string();
                    }
                }
            }
        }
        let full_name = make_full_name(
            pkg.as_deref().unwrap_or(""),
            &cls_name,
            parent.as_ref(),
            false,
        );
        ClassInfoInner {
            ty,
            name: cls_name,
            pkg,
            full_name,
            parent,
        }
    }

    pub fn arg_type(&self) -> &ArgType {
        &self.0.ty
    }
    /// Full dotted name, inner classes joined with `.` (`com.a.Outer.Inner`).
    pub fn full_name(&self) -> &str {
        &self.0.full_name
    }
    pub fn short_name(&self) -> &str {
        &self.0.name
    }
    pub fn package(&self) -> &str {
        match &self.0.parent {
            Some(p) => p.package(),
            None => self.0.pkg.as_deref().unwrap_or(""),
        }
    }
    pub fn parent_class(&self) -> Option<&ClassInfo> {
        self.0.parent.as_ref()
    }
    pub fn is_inner(&self) -> bool {
        self.0.parent.is_some()
    }
    /// Full name with `$` as inner class separator (`com.a.Outer$Inner`).
    pub fn raw_full_name(&self) -> String {
        make_full_name(
            self.0.pkg.as_deref().unwrap_or(""),
            &self.0.name,
            self.0.parent.as_ref(),
            true,
        )
    }
    pub fn raw_name(&self) -> String {
        self.0.ty.object_name().unwrap_or_default()
    }
}

fn make_full_name(pkg: &str, short: &str, parent: Option<&ClassInfo>, raw: bool) -> String {
    if let Some(p) = parent {
        let parent_name = if raw {
            p.raw_full_name()
        } else {
            p.full_name().to_string()
        };
        let sep = if raw { '$' } else { '.' };
        return format!("{parent_name}{sep}{short}");
    }
    if pkg.is_empty() {
        short.to_string()
    } else {
        format!("{pkg}.{short}")
    }
}

fn check_class_type(ty: &ArgType) -> Result<ArgType, InfoError> {
    if ty.is_array() {
        return Ok(ArgType::object_type());
    }
    if !ty.is_object() || ty.is_generic_type() {
        return Err(InfoError(format!("Not class type: {ty}")));
    }
    if ty.is_generic() {
        return Ok(ArgType::object(&ty.object_name().unwrap_or_default()));
    }
    Ok(ty.clone())
}

impl fmt::Display for ClassInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.full_name())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct FieldInfo {
    pub decl_class: ClassInfo,
    pub name: String,
    pub ty: ArgType,
}

impl FieldInfo {
    pub fn short_id(&self) -> String {
        format!("{}:{}", self.name, type_signature(&self.ty))
    }
    pub fn full_id(&self) -> String {
        format!(
            "{}.{}:{}",
            self.decl_class.full_name(),
            self.name,
            type_signature(&self.ty)
        )
    }
}

impl fmt::Display for FieldInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{} {}", self.decl_class, self.name, self.ty)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct MethodInfo {
    pub decl_class: ClassInfo,
    pub name: String,
    pub arg_types: Vec<ArgType>,
    pub return_type: ArgType,
}

impl MethodInfo {
    pub fn short_id(&self) -> String {
        make_short_id(&self.name, &self.arg_types, Some(&self.return_type))
    }
    pub fn args_count(&self) -> usize {
        self.arg_types.len()
    }
    pub fn is_constructor(&self) -> bool {
        self.name == "<init>"
    }
    pub fn is_class_init(&self) -> bool {
        self.name == "<clinit>"
    }
    /// `isOverloadedBy`: same name and arguments count, different signature.
    pub fn is_overloaded_by(&self, other: &MethodInfo) -> bool {
        self.arg_types.len() == other.arg_types.len()
            && self.name == other.name
            && self.short_id() != other.short_id()
    }
}

pub fn make_short_id(name: &str, args: &[ArgType], ret: Option<&ArgType>) -> String {
    let mut s = String::from(name);
    s.push('(');
    for a in args {
        s.push_str(&type_signature(a));
    }
    s.push(')');
    if let Some(r) = ret {
        s.push_str(&type_signature(r));
    }
    s
}

impl fmt::Display for MethodInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}(", self.decl_class.full_name(), self.name)?;
        for (i, a) in self.arg_types.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            write!(f, "{a}")?;
        }
        write!(f, "):{}", self.return_type)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn class_names() {
        let c = ClassInfo::from_name("Lcom/a/Outer$Inner;", true).unwrap();
        assert_eq!(c.full_name(), "com.a.Outer.Inner");
        assert_eq!(c.raw_full_name(), "com.a.Outer$Inner");
        assert_eq!(c.package(), "com.a");
        assert!(c.is_inner());
        let c = ClassInfo::from_name("Lcom/a/Outer$Inner;", false).unwrap();
        assert_eq!(c.full_name(), "com.a.Outer$Inner");
        let c = ClassInfo::from_name("LA;", true).unwrap();
        assert_eq!(c.full_name(), "A");
        assert_eq!(
            ClassInfo::from_type(&ArgType::parse("[I").unwrap(), true)
                .unwrap()
                .full_name(),
            "java.lang.Object"
        );
    }

    #[test]
    fn method_string() {
        let m = MethodInfo {
            decl_class: ClassInfo::from_name("Ljava/lang/String;", true).unwrap(),
            name: "indexOf".into(),
            arg_types: vec![ArgType::int(), ArgType::string()],
            return_type: ArgType::int(),
        };
        assert_eq!(
            m.to_string(),
            "java.lang.String.indexOf(int, java.lang.String):int"
        );
        assert_eq!(m.short_id(), "indexOf(ILjava/lang/String;)I");
    }
}
