//! Method level state of the code generator (`MethodGen`): the method's code, its variable
//! names and the owning class generator.

use std::cell::RefCell;

use super::class_gen::ClassGen;
use super::name_gen::NameGen;
use crate::model::cfg::MethodCfg;
use crate::model::node::MethodNode;

/// `MethodGen`
pub struct MethodGen<'a> {
    cls_gen: &'a ClassGen<'a>,
    cfg: &'a MethodCfg,
    mth: usize,
    name_gen: RefCell<NameGen>,
}

impl<'a> MethodGen<'a> {
    /// Generator for method `mth` of the class of `cls_gen` with the code state `cfg`.
    pub fn new(cls_gen: &'a ClassGen<'a>, cfg: &'a MethodCfg, mth: usize) -> Self {
        let name_gen = NameGen::new(cls_gen.env(), cls_gen.class_index(), None);
        MethodGen {
            cls_gen,
            cfg,
            mth,
            name_gen: RefCell::new(name_gen),
        }
    }

    pub fn class_gen(&self) -> &'a ClassGen<'a> {
        self.cls_gen
    }

    pub fn cfg(&self) -> &'a MethodCfg {
        self.cfg
    }

    pub fn method_index(&self) -> usize {
        self.mth
    }

    pub fn method_node(&self) -> &'a MethodNode {
        &self.cls_gen.env().root.classes[self.cls_gen.class_index()].methods[self.mth]
    }

    pub fn name_gen(&self) -> &RefCell<NameGen> {
        &self.name_gen
    }

    /// Assign names to the declared method arguments (what the method declaration does).
    pub fn assign_arg_names(&self) {
        for reg in &self.cfg.arg_regs {
            if let Some(cv) = reg.svar.and_then(|v| self.cfg.svars[v].code_var) {
                self.name_gen.borrow_mut().assign_arg(self.cfg, cv);
            }
        }
    }
}
