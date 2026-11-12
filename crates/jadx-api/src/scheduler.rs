//! `DecompilerScheduler`: the order classes are decompiled in. Classes with few dependencies come
//! first, the dependencies of a class are put in front of it in one batch. The batches are run
//! one after the other, so the flattened batch list is the processing order of a single thread.

use std::collections::HashSet;

use jadx_core::model::node::Root;

const MERGED_BATCH_SIZE: usize = 16;

fn total_deps(root: &Root, cls: usize) -> usize {
    root.classes[cls].dependencies.len() + root.classes[cls].codegen_deps.len()
}

/// `DecompilerScheduler.internalBatches`
pub fn build_batches(root: &Root, classes: &[usize]) -> Vec<Vec<usize>> {
    let mut rank = vec![0usize; root.classes.len()];
    for (r, i) in root.sorted_class_order().into_iter().enumerate() {
        rank[i] = r;
    }
    let mut deps: Vec<(usize, usize)> = classes
        .iter()
        .map(|&cls| {
            let count: usize = root.classes[cls]
                .dependencies
                .iter()
                .map(|&d| 1 + total_deps(root, d))
                .sum();
            (count, cls)
        })
        .collect();
    deps.sort_by_key(|&(count, cls)| (count, rank[cls]));
    let mut added: HashSet<usize> = HashSet::with_capacity(classes.len());
    let mut result: Vec<Vec<usize>> = Vec::new();
    let mut merged: Vec<usize> = Vec::with_capacity(MERGED_BATCH_SIZE);
    for (_, cls) in deps {
        if !added.insert(cls) {
            continue;
        }
        if total_deps(root, cls) == 0 {
            merged.push(cls);
            if merged.len() >= MERGED_BATCH_SIZE {
                result.push(std::mem::take(&mut merged));
            }
        } else {
            let mut batch = Vec::new();
            for &dep in &root.classes[cls].dependencies {
                let top = root.top_parent_index(dep);
                if !added.contains(&top) {
                    batch.push(top);
                    added.insert(top);
                }
            }
            // stable sort by the number of dependencies
            batch.sort_by_key(|&c| total_deps(root, c));
            batch.push(cls);
            result.push(batch);
        }
    }
    if !merged.is_empty() {
        result.push(merged);
    }
    result
}

/// The classes in the order the batches are processed.
pub fn schedule(root: &Root, classes: &[usize]) -> Vec<usize> {
    build_batches(root, classes).into_iter().flatten().collect()
}
