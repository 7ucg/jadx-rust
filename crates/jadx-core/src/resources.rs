//! Android resource ids in the class tree: the app `R` class gets the resource id constants of
//! the resource table (`AndroidResourcesUtils.searchAppResClass`), and the resource id to name
//! map of the table is used to replace id literals by `R.<type>.<name>` references
//! (`ConstStorage.getResourceField`).
//!
//! The table is decoded outside of the core, the result arrives as a [`ResourceInput`]. The `R`
//! classes are added to the class data before the class tree is built
//! ([`add_resource_classes`]) and the plan is applied to the built tree
//! ([`Root::apply_resource_plan`]).

use std::collections::{BTreeMap, HashMap, HashSet};

use jadx_input_api::{ClassAttrs, ClassData, ConstValue, FieldAttrs, FieldData};

use crate::model::const_storage::FieldRef;
use crate::model::node::Root;

const ACC_PUBLIC: u32 = 0x1;
const ACC_STATIC: u32 = 0x8;
const ACC_FINAL: u32 = 0x10;

/// One resource id constant of the app `R` class.
#[derive(Debug, Clone)]
pub struct ResourceField {
    pub id: i32,
    /// Resource type (`id`, `attr`, ...), the name of the inner class.
    pub type_name: String,
    /// Field name.
    pub name: String,
}

/// What the core needs from the resource table.
#[derive(Debug, Clone, Default)]
pub struct ResourceInput {
    pub app_package: Option<String>,
    /// Resource id to `type/name`.
    pub names: HashMap<i32, String>,
    /// Constants of the `R` class in table order.
    pub fields: Vec<ResourceField>,
}

/// Result of [`add_resource_classes`]: what [`Root::apply_resource_plan`] sets on the built tree.
#[derive(Debug, Clone, Default)]
pub struct ResourcePlan {
    app_package: Option<String>,
    names: HashMap<i32, String>,
    /// Raw name of the app `R` class.
    app_res_class: Option<String>,
    /// Raw names of the classes created by the decompiler.
    synthetic: Vec<String>,
    /// `(raw class name, comment)`
    class_comments: Vec<(String, String)>,
    /// `(raw class name, field name, comment)`
    field_comments: Vec<(String, String, String)>,
}

fn raw_name(desc: &str) -> String {
    desc.strip_prefix('L')
        .and_then(|d| d.strip_suffix(';'))
        .unwrap_or(desc)
        .replace('/', ".")
}

fn desc_of(raw: &str) -> String {
    format!("L{};", raw.replace('.', "/"))
}

fn synthetic_class(raw: &str, access: u32, fields: Vec<FieldData>) -> ClassData {
    ClassData {
        type_desc: desc_of(raw),
        access_flags: access,
        super_type: None,
        interfaces: Vec::new(),
        signature: None,
        inner_class: None,
        annotation_refs: Vec::new(),
        fields,
        methods: Vec::new(),
        attrs: ClassAttrs::default(),
    }
}

fn resource_field(f: &ResourceField) -> FieldData {
    FieldData {
        name: f.name.clone(),
        ty: "I".to_string(),
        access_flags: ACC_PUBLIC | ACC_STATIC | ACC_FINAL,
        signature: None,
        const_value: Some(ConstValue::Int(f.id)),
        annotation_refs: Vec::new(),
        attrs: FieldAttrs::default(),
    }
}

/// Short name of a class as `ClassInfo` computes it.
fn short_name(raw: &str, move_inner_classes: bool, known: &HashSet<String>) -> String {
    let simple = raw.rsplit('.').next().unwrap_or(raw);
    if move_inner_classes {
        if let Some((outer, inner)) = raw.rsplit_once('$') {
            if known.contains(outer) {
                return inner.to_string();
            }
        }
    }
    simple.to_string()
}

/// `AndroidResourcesUtils.searchAppResClass` and `addResourceFields` on the class data: the
/// resource id fields are added to the app `R` class and its type classes; classes that do not
/// exist are created.
pub fn add_resource_classes(
    classes: &mut Vec<ClassData>,
    input: &ResourceInput,
    move_inner_classes: bool,
) -> ResourcePlan {
    let mut plan = ResourcePlan {
        app_package: input.app_package.clone(),
        names: input.names.clone(),
        ..ResourcePlan::default()
    };
    let app_name = match &input.app_package {
        Some(p) => format!("{p}.R"),
        None => "R".to_string(),
    };
    let index_of = |classes: &[ClassData], raw: &str| -> Option<usize> {
        let desc = desc_of(raw);
        classes.iter().position(|c| c.type_desc == desc)
    };
    let known: HashSet<String> = classes.iter().map(|c| raw_name(&c.type_desc)).collect();
    let (res_raw, r_cls_exists) = if index_of(classes, &app_name).is_some() {
        (app_name.clone(), true)
    } else {
        let candidates: Vec<String> = classes
            .iter()
            .map(|c| raw_name(&c.type_desc))
            .filter(|r| short_name(r, move_inner_classes, &known) == "R")
            .collect();
        if candidates.len() == 1 {
            (candidates[0].clone(), true)
        } else {
            classes.push(synthetic_class(
                &app_name,
                ACC_PUBLIC | ACC_FINAL,
                Vec::new(),
            ));
            plan.synthetic.push(app_name.clone());
            plan.class_comments
                .push((app_name.clone(), "This class is generated by JADX".into()));
            (app_name.clone(), false)
        }
    };
    plan.app_res_class = Some(res_raw.clone());

    // type classes of the `R` class by type name
    let mut type_classes: BTreeMap<String, String> = BTreeMap::new();
    if r_cls_exists {
        let prefix = format!("{res_raw}$");
        for c in classes.iter() {
            let raw = raw_name(&c.type_desc);
            if let Some(rest) = raw.strip_prefix(&prefix) {
                if !rest.contains('$') {
                    type_classes.insert(rest.to_string(), raw.clone());
                }
            }
        }
    }
    for f in &input.fields {
        let cls_raw = match type_classes.get(&f.type_name) {
            Some(r) => r.clone(),
            None => {
                let raw = format!("{res_raw}${}", f.type_name);
                if index_of(classes, &raw).is_none() {
                    classes.push(synthetic_class(
                        &raw,
                        ACC_PUBLIC | ACC_STATIC | ACC_FINAL,
                        Vec::new(),
                    ));
                    plan.synthetic.push(raw.clone());
                    if r_cls_exists {
                        plan.class_comments
                            .push((raw.clone(), "Added by JADX".into()));
                    }
                }
                type_classes.insert(f.type_name.clone(), raw.clone());
                raw
            }
        };
        let Some(idx) = index_of(classes, &cls_raw) else {
            continue;
        };
        if classes[idx].fields.iter().any(|x| x.name == f.name) {
            continue;
        }
        classes[idx].fields.push(resource_field(f));
        if r_cls_exists {
            plan.field_comments
                .push((cls_raw, f.name.clone(), "Added by JADX".into()));
        }
    }
    plan
}

impl Root {
    /// Apply the plan made by [`add_resource_classes`] to the built class tree: remember the
    /// app `R` class, mark the created classes, attach comments and resolve the resource ids
    /// to their `R` fields for the constant replacement.
    pub fn apply_resource_plan(&mut self, plan: ResourcePlan) {
        self.app_package = plan.app_package;
        for raw in &plan.synthetic {
            if let Some(i) = self.class_index(raw) {
                self.classes[i].synthetic = true;
            }
        }
        for (raw, comment) in plan.class_comments {
            if let Some(i) = self.class_index(&raw) {
                self.classes[i].info_comments.push(comment);
            }
        }
        for (raw, field, comment) in plan.field_comments {
            if let Some(i) = self.class_index(&raw) {
                if let Some(f) = self.classes[i]
                    .fields
                    .iter_mut()
                    .find(|f| f.info.name == field)
                {
                    f.info_comments.push(comment);
                }
            }
        }
        self.app_res_class = plan
            .app_res_class
            .as_deref()
            .and_then(|r| self.class_index(r));
        let mut resolved: HashMap<i32, FieldRef> = HashMap::new();
        if let Some(res_cls) = self.app_res_class {
            let mut type_classes: HashMap<String, usize> = HashMap::new();
            for ci in 0..self.classes.len() {
                if self.parent_index(ci) == Some(res_cls) {
                    type_classes
                        .entry(self.classes[ci].info.short_name().to_string())
                        .or_insert(ci);
                }
            }
            for (id, name) in &plan.names {
                let Some((type_name, field_name)) = name.split_once('/') else {
                    continue;
                };
                let Some(&ci) = type_classes.get(type_name) else {
                    continue;
                };
                if let Some(fi) = self.classes[ci]
                    .fields
                    .iter()
                    .position(|f| f.info.name == field_name)
                {
                    resolved.insert(*id, FieldRef::Node(ci, fi));
                }
            }
        }
        self.const_storage.set_resource_names(plan.names, resolved);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> ResourceInput {
        let field = |id, t: &str, n: &str| ResourceField {
            id,
            type_name: t.into(),
            name: n.into(),
        };
        ResourceInput {
            app_package: Some("a.b".into()),
            names: HashMap::from([(0x7f01_0000, "id/main".to_string())]),
            fields: vec![
                field(0x7f01_0000, "id", "main"),
                field(0x7f02_0000, "layout", "act"),
            ],
        }
    }

    #[test]
    fn creates_the_app_r_class_when_missing() {
        let mut classes = Vec::new();
        let plan = add_resource_classes(&mut classes, &input(), true);
        let names: Vec<_> = classes.iter().map(|c| c.type_desc.as_str()).collect();
        assert_eq!(names, ["La/b/R;", "La/b/R$id;", "La/b/R$layout;"]);
        assert_eq!(classes[1].fields[0].name, "main");
        assert_eq!(
            classes[1].fields[0].const_value,
            Some(ConstValue::Int(0x7f01_0000))
        );
        assert_eq!(plan.synthetic.len(), 3);
        // only the top class carries a comment, the new members are not "added"
        assert_eq!(plan.class_comments.len(), 1);
        assert!(plan.field_comments.is_empty());
    }

    #[test]
    fn extends_an_existing_r_class() {
        let mut classes = vec![synthetic_class("a.b.R", ACC_PUBLIC, Vec::new())];
        classes.push(synthetic_class(
            "a.b.R$id",
            ACC_PUBLIC | ACC_STATIC,
            vec![resource_field(&ResourceField {
                id: 0x7f01_0000,
                type_name: "id".into(),
                name: "main".into(),
            })],
        ));
        let plan = add_resource_classes(&mut classes, &input(), true);
        assert_eq!(classes.len(), 3, "only the layout class is created");
        assert_eq!(classes[1].fields.len(), 1, "existing field is kept");
        assert_eq!(plan.field_comments.len(), 1);
        assert_eq!(plan.field_comments[0].1, "act");
    }
}
