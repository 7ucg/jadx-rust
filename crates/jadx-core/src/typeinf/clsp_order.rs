//! Super types and implementations of classes with the iteration order of the reference.
//!
//! `ClspGraph.getSuperTypes` hands out `java.util.HashSet` instances whose iteration order is
//! observable (the multi-variable search and the wider object search take the first accepted
//! candidate). The order depends on the string hashes, the table size and the insertion history,
//! so the sets are rebuilt here the same way `fillSuperTypesCache` builds them:
//! a shared temporary set (its table only grows) collects the super types of a class, then a
//! new set is created from it.

use std::collections::{HashMap, HashSet};

use crate::clsp::ClspGraph;
use crate::model::bitset::java_string_hash;

/// Cached hierarchy of the class path.
#[derive(Debug, Default)]
pub struct OrderedClsp {
    super_types: HashMap<String, Vec<String>>,
    implementations: HashMap<String, Vec<String>>,
}

impl OrderedClsp {
    /// `ClspGraph.getSuperTypes` in iteration order.
    pub fn super_types(&self, name: &str) -> &[String] {
        self.super_types.get(name).map_or(&[], |v| v.as_slice())
    }

    /// `ClspGraph.getImplementations`
    pub fn implementations(&self, name: &str) -> &[String] {
        self.implementations.get(name).map_or(&[], |v| v.as_slice())
    }
}

/// The hierarchy caches of `clsp` (built on first use).
pub fn ordered_clsp(clsp: &ClspGraph) -> &OrderedClsp {
    clsp.ordered.get_or_init(|| build(clsp))
}

/// `HashMap.tableSizeFor`
fn table_size_for(cap: usize) -> usize {
    cap.max(1).next_power_of_two()
}

/// Bucket index of a key in a table of `cap` buckets (`HashMap.hash` and index mask).
fn bucket(hash: i32, cap: usize) -> usize {
    let h = hash as u32;
    ((h ^ (h >> 16)) as usize) & (cap - 1)
}

/// Table size reached by a `HashSet` (default capacity) that grew by single insertions.
fn grown_capacity(mut cap: usize, size: usize) -> usize {
    while size > cap * 3 / 4 {
        cap *= 2;
    }
    cap
}

/// Stable order of `order` entries by bucket (insertion order is kept inside a bucket).
fn order_by_bucket(order: &[usize], hashes: &[i32], cap: usize) -> Vec<usize> {
    let mut v = order.to_vec();
    v.sort_by_key(|i| bucket(hashes[*i], cap));
    v
}

/// `String.compareTo`
fn java_str_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

fn add_super_types(
    g: &ClspGraph,
    name: &str,
    result: &mut Vec<String>,
    seen: &mut HashSet<String>,
) {
    let Some(cls) = g.cls_details(name) else {
        return;
    };
    for parent in cls.parents() {
        let Some(pname) = parent.object_name() else {
            continue;
        };
        match g.cls_details(&pname) {
            Some(pc) => {
                if seen.insert(pc.name.clone()) {
                    result.push(pc.name.clone());
                    add_super_types(g, &pc.name, result, seen);
                }
            }
            None => {
                if seen.insert(pname.clone()) {
                    result.push(pname);
                }
            }
        }
    }
}

fn build(g: &ClspGraph) -> OrderedClsp {
    let (names, set_count) = g.insertion_order();
    // iteration order of the class map: sized by the class set, doubled when it overflows
    let mut map_cap = table_size_for(set_count);
    let mut size = 0usize;
    for _ in names {
        size += 1;
        if size > map_cap * 3 / 4 {
            map_cap *= 2;
        }
    }
    let name_hashes: Vec<i32> = names.iter().map(|n| java_string_hash(n)).collect();
    let all: Vec<usize> = (0..names.len()).collect();
    let iteration = order_by_bucket(&all, &name_hashes, map_cap);

    let mut super_types: HashMap<String, Vec<String>> = HashMap::with_capacity(names.len());
    let mut tmp_cap = 16usize;
    for idx in iteration {
        let name = &names[idx];
        let mut collected: Vec<String> = Vec::new();
        add_super_types(g, name, &mut collected, &mut HashSet::new());
        let n = collected.len();
        tmp_cap = tmp_cap.max(grown_capacity(16, n));
        let ordered = if n < 2 {
            collected
        } else {
            let hashes: Vec<i32> = collected.iter().map(|s| java_string_hash(s)).collect();
            let seq: Vec<usize> = (0..n).collect();
            let tmp_order = order_by_bucket(&seq, &hashes, tmp_cap);
            // `new HashSet<>(tmpSet)`: `HashMap.newHashMap(Math.max(size, 12))` since JDK 19
            let final_cap = table_size_for((n.max(12) * 4).div_ceil(3));
            order_by_bucket(&tmp_order, &hashes, final_cap)
                .into_iter()
                .map(|i| collected[i].clone())
                .collect()
        };
        super_types.insert(name.clone(), ordered);
    }

    // `fillImplementsCache`: classes sorted by name, grouped by every super type
    let mut sorted: Vec<&String> = names.iter().collect();
    sorted.sort_by(|a, b| java_str_cmp(a, b));
    let mut implementations: HashMap<String, Vec<String>> = HashMap::new();
    for cls in sorted {
        for st in &super_types[cls] {
            implementations
                .entry(st.clone())
                .or_default()
                .push(cls.clone());
        }
    }
    OrderedClsp {
        super_types,
        implementations,
    }
}
