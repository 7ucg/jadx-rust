//! Bounded static call graph around one method.
//!
//! Adapted from RDX (Apache-2.0, Ch0pin and RDX contributors, `src/call_graph.rs`): breadth
//! first expansion with a depth limit and node and edge caps, edges folded per caller/callee
//! pair with a site count, Android component classification by super type, and the list of
//! super type declarations a virtual call may dispatch to. Modifications: built on the
//! [`UsageIndex`] and the root model instead of the RDX engine, backward and forward expansion
//! stay on their own side of the root.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};

use jadx_core::model::node::{MethodId, Root, ACC_STATIC};
use jadx_input_api::MethodRefData;

use crate::error::{IndexError, Result};
use crate::resolve::{display_method, method_ref_text, parse_method_ref};
use crate::usage::{CallTarget, Query, UsageIndex};

/// Android component kind of a class.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Component {
    Activity,
    Service,
    Receiver,
    Provider,
}

impl Component {
    pub fn label(self) -> &'static str {
        match self {
            Component::Activity => "Activity",
            Component::Service => "Service",
            Component::Receiver => "Receiver",
            Component::Provider => "Provider",
        }
    }

    fn of_base(name: &str) -> Option<Component> {
        match name {
            "android.app.Activity" => Some(Component::Activity),
            "android.app.Service" => Some(Component::Service),
            "android.content.BroadcastReceiver" => Some(Component::Receiver),
            "android.content.ContentProvider" => Some(Component::Provider),
            _ => None,
        }
    }
}

/// Component kind of a class (walks the super classes, then the framework hierarchy of the
/// first class outside the root).
pub fn component_of(root: &Root, class: usize) -> Option<Component> {
    let mut cur = root.classes[class].info.raw_name();
    let mut idx = Some(class);
    for _ in 0..256 {
        if let Some(c) = Component::of_base(&cur) {
            return Some(c);
        }
        match idx {
            Some(i) => {
                let sup = root.classes[i].super_class.as_ref()?.object_name()?;
                idx = root.class_index(&sup);
                cur = sup;
            }
            None => {
                let supers = root.clsp.super_types(&cur)?;
                return supers.iter().find_map(|s| Component::of_base(s));
            }
        }
    }
    None
}

/// Direction of the expansion.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Direction {
    /// Methods that call the root, then their callers.
    Callers,
    /// Methods the root calls, then their callees.
    #[default]
    Callees,
    /// Both; each side expands only away from the root.
    Both,
}

impl Direction {
    pub fn parse(text: &str) -> Option<Direction> {
        match text {
            "callers" => Some(Direction::Callers),
            "callees" => Some(Direction::Callees),
            "both" => Some(Direction::Both),
            _ => None,
        }
    }
}

/// Bounds of a graph build.
#[derive(Clone, Debug)]
pub struct GraphLimits {
    /// Expansion depth, 1 to 100.
    pub depth: usize,
    pub max_nodes: usize,
    pub max_edges: usize,
    /// Maximum number of caller sites collected per method.
    pub max_caller_sites: usize,
}

impl Default for GraphLimits {
    fn default() -> Self {
        GraphLimits {
            depth: 3,
            max_nodes: 5000,
            max_edges: 20_000,
            max_caller_sites: 2000,
        }
    }
}

/// Identity of a node.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum NodeKey {
    Method(MethodId),
    /// Method outside the root, by reference text.
    External(String),
}

#[derive(Debug)]
pub struct Node {
    pub key: NodeKey,
    /// Display text of the method.
    pub text: String,
    /// Distance from the root method.
    pub depth: usize,
    pub component: Option<Component>,
    /// The method has code in the root.
    pub available: bool,
}

#[derive(Debug)]
pub struct Edge {
    /// Caller node index.
    pub from: usize,
    /// Callee node index.
    pub to: usize,
    /// Invoke sites folded into this edge.
    pub sites: usize,
}

#[derive(Debug, Default)]
pub struct CallGraph {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    /// A cap was hit; the graph is incomplete.
    pub truncated: bool,
    /// Declarations in super types that a virtual call of the root method may dispatch to.
    pub dispatch_targets: Vec<MethodId>,
    pub dispatch_targets_truncated: bool,
    pub direction: Direction,
}

/// Declarations of `method` in super classes and interfaces (same name and descriptor, not
/// static, private or a constructor): the methods a call to the super type may reach.
pub fn dispatch_targets(root: &Root, method: MethodId) -> (Vec<MethodId>, bool) {
    const MAX_VISITS: usize = 256;
    let m = &root.classes[method.0].methods[method.1];
    if m.info.name.starts_with('<') || m.access & (ACC_STATIC | 0x2) != 0 {
        return (Vec::new(), false);
    }
    let short = m.info.short_id();
    let mut queue: VecDeque<usize> = VecDeque::new();
    let parents = |c: usize| -> Vec<usize> {
        let cn = &root.classes[c];
        cn.interfaces
            .iter()
            .chain(cn.super_class.iter())
            .filter_map(|t| t.object_name())
            .filter_map(|n| root.class_index(&n))
            .collect()
    };
    queue.extend(parents(method.0));
    let mut seen = HashSet::from([method.0]);
    let mut out = Vec::new();
    let mut truncated = false;
    while let Some(c) = queue.pop_front() {
        if !seen.insert(c) {
            continue;
        }
        if seen.len() > MAX_VISITS {
            truncated = true;
            break;
        }
        let cn = &root.classes[c];
        if let Some((i, other)) = cn
            .methods
            .iter()
            .enumerate()
            .find(|(_, o)| o.info.short_id() == short)
        {
            if other.access & (ACC_STATIC | 0x2) == 0 {
                out.push((c, i));
            }
        }
        queue.extend(parents(c));
    }
    out.sort();
    out.dedup();
    (out, truncated)
}

struct Builder<'i, 'a> {
    index: &'i UsageIndex<'a>,
    limits: &'i GraphLimits,
    cancel: Option<&'i AtomicBool>,
    graph: CallGraph,
    ids: HashMap<NodeKey, usize>,
    edge_ids: HashMap<(usize, usize), usize>,
}

impl Builder<'_, '_> {
    fn check(&self) -> Result<()> {
        if self.cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            return Err(IndexError::Cancelled);
        }
        Ok(())
    }

    fn node(&mut self, key: NodeKey, depth: usize) -> Option<(usize, bool)> {
        if let Some(&i) = self.ids.get(&key) {
            return Some((i, false));
        }
        if self.graph.nodes.len() >= self.limits.max_nodes {
            self.graph.truncated = true;
            return None;
        }
        let root = self.index.root();
        let (text, component, available) = match &key {
            NodeKey::Method(id) => (
                display_method(root, *id),
                component_of(root, id.0),
                root.classes[id.0].methods[id.1].code_offset.is_some(),
            ),
            NodeKey::External(t) => (t.clone(), None, false),
        };
        let i = self.graph.nodes.len();
        self.graph.nodes.push(Node {
            key: key.clone(),
            text,
            depth,
            component,
            available,
        });
        self.ids.insert(key, i);
        Some((i, true))
    }

    fn edge(&mut self, from: usize, to: usize) -> bool {
        if let Some(&i) = self.edge_ids.get(&(from, to)) {
            self.graph.edges[i].sites += 1;
            return true;
        }
        if self.graph.edges.len() >= self.limits.max_edges {
            self.graph.truncated = true;
            return false;
        }
        self.edge_ids.insert((from, to), self.graph.edges.len());
        self.graph.edges.push(Edge { from, to, sites: 1 });
        true
    }

    /// Expand away from the root in one direction (`forward`: callees).
    fn expand(&mut self, forward: bool) -> Result<()> {
        let mut queue = VecDeque::from([0usize]);
        let mut visited: HashSet<usize> = HashSet::new();
        while let Some(cur) = queue.pop_front() {
            self.check()?;
            if !visited.insert(cur) || self.graph.nodes[cur].depth >= self.limits.depth {
                continue;
            }
            let depth = self.graph.nodes[cur].depth;
            let key = self.graph.nodes[cur].key.clone();
            // neighbours as (key, number of sites)
            let mut neighbours: Vec<NodeKey> = Vec::new();
            if forward {
                if let NodeKey::Method(id) = key {
                    for c in self.index.callees_of(id).unwrap_or_default() {
                        neighbours.push(match c.target {
                            CallTarget::Method(m) => NodeKey::Method(m),
                            CallTarget::External(r) => NodeKey::External(method_ref_text(&r)),
                        });
                    }
                }
            } else {
                let q = Query {
                    limit: self.limits.max_caller_sites,
                    ..Query::default()
                };
                let sites = match &key {
                    NodeKey::Method(id) => self.index.callers_of(*id, &q)?,
                    NodeKey::External(text) => match parse_method_ref(text) {
                        Some(r) => self.index.callers_of_ref(&r, &q)?,
                        None => Default::default(),
                    },
                };
                self.graph.truncated |= sites.truncated;
                neighbours.extend(sites.sites.iter().map(|s| NodeKey::Method(s.method_id())));
            }
            for nk in neighbours {
                self.check()?;
                let Some((ni, fresh)) = self.node(nk, depth + 1) else {
                    continue;
                };
                let (from, to) = if forward { (cur, ni) } else { (ni, cur) };
                if !self.edge(from, to) {
                    return Ok(());
                }
                if fresh {
                    queue.push_back(ni);
                }
            }
        }
        Ok(())
    }
}

/// Build the call graph around `root_method`.
pub fn build_call_graph(
    index: &UsageIndex<'_>,
    root_method: MethodId,
    direction: Direction,
    limits: &GraphLimits,
    cancel: Option<&AtomicBool>,
) -> Result<CallGraph> {
    if !(1..=100).contains(&limits.depth) {
        return Err(IndexError::Invalid(
            "call graph depth must be between 1 and 100".into(),
        ));
    }
    let root = index.root();
    if root_method.0 >= root.classes.len()
        || root_method.1 >= root.classes[root_method.0].methods.len()
    {
        return Err(IndexError::Invalid("unknown method".into()));
    }
    let (dispatch, dispatch_truncated) = dispatch_targets(root, root_method);
    let mut b = Builder {
        index,
        limits,
        cancel,
        graph: CallGraph {
            dispatch_targets: dispatch,
            dispatch_targets_truncated: dispatch_truncated,
            direction,
            ..CallGraph::default()
        },
        ids: HashMap::new(),
        edge_ids: HashMap::new(),
    };
    b.node(NodeKey::Method(root_method), 0);
    if direction != Direction::Callers {
        b.expand(true)?;
    }
    if direction != Direction::Callees {
        b.expand(false)?;
    }
    Ok(b.graph)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direction_parse() {
        assert_eq!(Direction::parse("both"), Some(Direction::Both));
        assert_eq!(Direction::parse("x"), None);
        assert_eq!(Direction::default(), Direction::Callees);
    }
}
