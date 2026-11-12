//! Emulation of the iteration order of `java.util.HashMap`.
//!
//! The reference input plugin groups the exception table of a method in a `HashMap` (filled with
//! `computeIfAbsent`) and hands out the try blocks in the iteration order of that map. To return
//! them in the same order the map is simulated, including lazy resizing, head insertion into
//! bins and tree bins (red-black trees are used for bins with more than 7 colliding keys once
//! the table has at least 64 buckets).
//!
//! Keys are `(start, end)` pairs with the hash code `start + 31 * end`.

const TREEIFY_THRESHOLD: usize = 8;
const UNTREEIFY_THRESHOLD: usize = 6;
const MIN_TREEIFY_CAPACITY: usize = 64;

#[derive(Clone)]
struct Node {
    hash: u32,
    key: usize,
    next: Option<usize>,
    prev: Option<usize>,
    parent: Option<usize>,
    left: Option<usize>,
    right: Option<usize>,
    red: bool,
    tree: bool,
}

struct Sim {
    nodes: Vec<Node>,
    table: Vec<Option<usize>>,
    size: usize,
    threshold: usize,
    keys: Vec<(u32, u32)>,
}

fn spread(key: &(u32, u32)) -> u32 {
    let h = key.0.wrapping_add(31u32.wrapping_mul(key.1));
    h ^ (h >> 16)
}

impl Sim {
    fn resize(&mut self) {
        let old_cap = self.table.len();
        let old_thr = self.threshold;
        let (new_cap, mut new_thr) = if old_cap > 0 {
            (old_cap << 1, if old_cap >= 16 { old_thr << 1 } else { 0 })
        } else if old_thr > 0 {
            (old_thr, 0)
        } else {
            (16, 12)
        };
        if new_thr == 0 {
            new_thr = (new_cap as f32 * 0.75) as usize;
        }
        self.threshold = new_thr;
        let old = std::mem::replace(&mut self.table, vec![None; new_cap]);
        for (j, slot) in old.iter().enumerate() {
            let Some(e) = *slot else { continue };
            if self.nodes[e].next.is_none() {
                let idx = self.nodes[e].hash as usize & (new_cap - 1);
                self.table[idx] = Some(e);
            } else if self.nodes[e].tree {
                self.split_tree(e, j, old_cap);
            } else {
                let (mut lo, mut lo_tail, mut hi, mut hi_tail) = (None, None, None, None);
                let mut cur = Some(e);
                while let Some(c) = cur {
                    let next = self.nodes[c].next;
                    if self.nodes[c].hash as usize & old_cap == 0 {
                        link_tail(&mut self.nodes, &mut lo, &mut lo_tail, c, false);
                    } else {
                        link_tail(&mut self.nodes, &mut hi, &mut hi_tail, c, false);
                    }
                    cur = next;
                }
                if let Some(t) = lo_tail {
                    self.nodes[t].next = None;
                }
                if let Some(t) = hi_tail {
                    self.nodes[t].next = None;
                }
                self.table[j] = lo;
                self.table[j + old_cap] = hi;
            }
        }
    }

    fn split_tree(&mut self, head: usize, index: usize, bit: usize) {
        let (mut lo, mut lo_tail, mut hi, mut hi_tail) = (None, None, None, None);
        let (mut lc, mut hc) = (0usize, 0usize);
        let mut cur = Some(head);
        while let Some(c) = cur {
            let next = self.nodes[c].next;
            self.nodes[c].next = None;
            if self.nodes[c].hash as usize & bit == 0 {
                link_tail(&mut self.nodes, &mut lo, &mut lo_tail, c, true);
                lc += 1;
            } else {
                link_tail(&mut self.nodes, &mut hi, &mut hi_tail, c, true);
                hc += 1;
            }
            cur = next;
        }
        for (list, count, other, idx) in [(lo, lc, hi, index), (hi, hc, lo, index + bit)] {
            let Some(h) = list else { continue };
            if count <= UNTREEIFY_THRESHOLD {
                self.untreeify(h);
                self.table[idx] = Some(h);
            } else {
                self.table[idx] = Some(h);
                if other.is_some() {
                    self.treeify(h);
                }
            }
        }
    }

    fn untreeify(&mut self, head: usize) {
        let mut cur = Some(head);
        while let Some(c) = cur {
            let n = &mut self.nodes[c];
            n.tree = false;
            n.parent = None;
            n.left = None;
            n.right = None;
            n.prev = None;
            cur = n.next;
        }
    }

    fn move_root_to_front(&mut self, root: usize) {
        let n = self.table.len();
        let index = self.nodes[root].hash as usize & (n - 1);
        let first = self.table[index];
        if first != Some(root) {
            self.table[index] = Some(root);
            let rp = self.nodes[root].prev;
            let rn = self.nodes[root].next;
            if let Some(rn) = rn {
                self.nodes[rn].prev = rp;
            }
            if let Some(rp) = rp {
                self.nodes[rp].next = rn;
            }
            if let Some(f) = first {
                self.nodes[f].prev = Some(root);
            }
            self.nodes[root].next = first;
            self.nodes[root].prev = None;
        }
    }

    fn treeify(&mut self, head: usize) {
        let mut root: Option<usize> = None;
        let mut cur = Some(head);
        while let Some(x) = cur {
            cur = self.nodes[x].next;
            self.nodes[x].left = None;
            self.nodes[x].right = None;
            let Some(r) = root else {
                self.nodes[x].parent = None;
                self.nodes[x].red = false;
                root = Some(x);
                continue;
            };
            let h = self.nodes[x].hash;
            let mut p = r;
            loop {
                let ph = self.nodes[p].hash;
                // equal hashes would be ordered by identity hash code in the reference
                let dir = if ph > h {
                    -1
                } else if ph < h {
                    1
                } else {
                    -1
                };
                let xp = p;
                let child = if dir <= 0 {
                    self.nodes[p].left
                } else {
                    self.nodes[p].right
                };
                match child {
                    Some(c) => p = c,
                    None => {
                        self.nodes[x].parent = Some(xp);
                        if dir <= 0 {
                            self.nodes[xp].left = Some(x);
                        } else {
                            self.nodes[xp].right = Some(x);
                        }
                        root = Some(self.balance_insertion(root.expect("root"), x));
                        break;
                    }
                }
            }
        }
        if let Some(r) = root {
            self.move_root_to_front(r);
        }
    }

    fn rotate_left(&mut self, mut root: usize, p: usize) -> usize {
        if let Some(r) = self.nodes[p].right {
            let rl = self.nodes[r].left;
            self.nodes[p].right = rl;
            if let Some(rl) = rl {
                self.nodes[rl].parent = Some(p);
            }
            let pp = self.nodes[p].parent;
            self.nodes[r].parent = pp;
            match pp {
                None => {
                    root = r;
                    self.nodes[r].red = false;
                }
                Some(pp) => {
                    if self.nodes[pp].left == Some(p) {
                        self.nodes[pp].left = Some(r);
                    } else {
                        self.nodes[pp].right = Some(r);
                    }
                }
            }
            self.nodes[r].left = Some(p);
            self.nodes[p].parent = Some(r);
        }
        root
    }

    fn rotate_right(&mut self, mut root: usize, p: usize) -> usize {
        if let Some(l) = self.nodes[p].left {
            let lr = self.nodes[l].right;
            self.nodes[p].left = lr;
            if let Some(lr) = lr {
                self.nodes[lr].parent = Some(p);
            }
            let pp = self.nodes[p].parent;
            self.nodes[l].parent = pp;
            match pp {
                None => {
                    root = l;
                    self.nodes[l].red = false;
                }
                Some(pp) => {
                    if self.nodes[pp].right == Some(p) {
                        self.nodes[pp].right = Some(l);
                    } else {
                        self.nodes[pp].left = Some(l);
                    }
                }
            }
            self.nodes[l].right = Some(p);
            self.nodes[p].parent = Some(l);
        }
        root
    }

    fn balance_insertion(&mut self, mut root: usize, mut x: usize) -> usize {
        self.nodes[x].red = true;
        loop {
            let Some(xp) = self.nodes[x].parent else {
                self.nodes[x].red = false;
                return x;
            };
            let Some(xpp) = self.nodes[xp].parent else {
                return root;
            };
            if !self.nodes[xp].red {
                return root;
            }
            let xppl = self.nodes[xpp].left;
            if Some(xp) == xppl {
                let xppr = self.nodes[xpp].right;
                if xppr.is_some_and(|r| self.nodes[r].red) {
                    self.nodes[xppr.expect("checked")].red = false;
                    self.nodes[xp].red = false;
                    self.nodes[xpp].red = true;
                    x = xpp;
                } else {
                    let mut xp_opt = Some(xp);
                    let mut xpp_opt = Some(xpp);
                    if Some(x) == self.nodes[xp].right {
                        x = xp;
                        root = self.rotate_left(root, x);
                        xp_opt = self.nodes[x].parent;
                        xpp_opt = xp_opt.and_then(|p| self.nodes[p].parent);
                    }
                    if let Some(p) = xp_opt {
                        self.nodes[p].red = false;
                        if let Some(pp) = xpp_opt {
                            self.nodes[pp].red = true;
                            root = self.rotate_right(root, pp);
                        }
                    }
                }
            } else if xppl.is_some_and(|l| self.nodes[l].red) {
                self.nodes[xppl.expect("checked")].red = false;
                self.nodes[xp].red = false;
                self.nodes[xpp].red = true;
                x = xpp;
            } else {
                let mut xp_opt = Some(xp);
                let mut xpp_opt = Some(xpp);
                if Some(x) == self.nodes[xp].left {
                    x = xp;
                    root = self.rotate_right(root, x);
                    xp_opt = self.nodes[x].parent;
                    xpp_opt = xp_opt.and_then(|p| self.nodes[p].parent);
                }
                if let Some(p) = xp_opt {
                    self.nodes[p].red = false;
                    if let Some(pp) = xpp_opt {
                        self.nodes[pp].red = true;
                        root = self.rotate_left(root, pp);
                    }
                }
            }
        }
    }

    fn treeify_bin(&mut self, hash: u32) {
        let n = self.table.len();
        if n < MIN_TREEIFY_CAPACITY {
            self.resize();
            return;
        }
        let index = hash as usize & (n - 1);
        let Some(first) = self.table[index] else {
            return;
        };
        let mut prev: Option<usize> = None;
        let mut cur = Some(first);
        while let Some(c) = cur {
            self.nodes[c].tree = true;
            self.nodes[c].prev = prev;
            prev = Some(c);
            cur = self.nodes[c].next;
        }
        self.treeify(first);
    }

    fn put_tree_val(&mut self, first: usize, new: usize) {
        let mut root = first;
        while let Some(p) = self.nodes[root].parent {
            root = p;
        }
        let h = self.nodes[new].hash;
        let mut p = root;
        loop {
            let ph = self.nodes[p].hash;
            let dir = if ph > h {
                -1
            } else if ph < h {
                1
            } else {
                -1
            };
            let xp = p;
            let child = if dir <= 0 {
                self.nodes[p].left
            } else {
                self.nodes[p].right
            };
            match child {
                Some(c) => p = c,
                None => {
                    let xpn = self.nodes[xp].next;
                    self.nodes[new].next = xpn;
                    self.nodes[new].tree = true;
                    if dir <= 0 {
                        self.nodes[xp].left = Some(new);
                    } else {
                        self.nodes[xp].right = Some(new);
                    }
                    self.nodes[xp].next = Some(new);
                    self.nodes[new].parent = Some(xp);
                    self.nodes[new].prev = Some(xp);
                    if let Some(xpn) = xpn {
                        self.nodes[xpn].prev = Some(new);
                    }
                    let r = self.balance_insertion(root, new);
                    self.move_root_to_front(r);
                    return;
                }
            }
        }
    }

    /// `computeIfAbsent` of the key with index `key` (a repeated key only checks the size).
    fn compute_if_absent(&mut self, key: usize) {
        let hash = spread(&self.keys[key]);
        if self.size > self.threshold || self.table.is_empty() {
            self.resize();
        }
        let n = self.table.len();
        let i = hash as usize & (n - 1);
        let first = self.table[i];
        let mut bin_count = 0usize;
        if let Some(f) = first {
            let mut cur = Some(f);
            while let Some(c) = cur {
                if self.keys[self.nodes[c].key] == self.keys[key] {
                    return;
                }
                bin_count += 1;
                cur = self.nodes[c].next;
            }
        }
        let new = self.nodes.len();
        self.nodes.push(Node {
            hash,
            key,
            next: first,
            prev: None,
            parent: None,
            left: None,
            right: None,
            red: false,
            tree: false,
        });
        match first {
            Some(f) if self.nodes[f].tree => {
                self.nodes[new].next = None;
                self.put_tree_val(f, new);
            }
            _ => {
                self.table[i] = Some(new);
                if bin_count >= TREEIFY_THRESHOLD - 1 {
                    self.treeify_bin(hash);
                }
            }
        }
        self.size += 1;
    }
}

/// Append `c` to the list `(head, tail)`.
fn link_tail(
    nodes: &mut [Node],
    head: &mut Option<usize>,
    tail: &mut Option<usize>,
    c: usize,
    set_prev: bool,
) {
    if set_prev {
        nodes[c].prev = *tail;
    }
    match *tail {
        None => *head = Some(c),
        Some(t) => nodes[t].next = Some(c),
    }
    *tail = Some(c);
}

/// Iteration order of a `HashMap` created with `entries.len()` as initial capacity after
/// `computeIfAbsent` was called for every `(start, end)` key of `entries` in order. Returns the
/// indices of the distinct keys in order of first appearance.
pub(crate) fn computeifabsent_order(entries: &[(u32, u32)]) -> Vec<usize> {
    let mut keys: Vec<(u32, u32)> = Vec::new();
    let mut entry_key: Vec<usize> = Vec::with_capacity(entries.len());
    for e in entries {
        match keys.iter().position(|k| k == e) {
            Some(i) => entry_key.push(i),
            None => {
                entry_key.push(keys.len());
                keys.push(*e);
            }
        }
    }
    let mut sim = Sim {
        nodes: Vec::new(),
        table: Vec::new(),
        size: 0,
        threshold: entries.len().max(1).next_power_of_two(),
        keys,
    };
    for k in entry_key {
        sim.compute_if_absent(k);
    }
    let mut order = Vec::with_capacity(sim.size);
    for slot in &sim.table {
        let mut cur = *slot;
        while let Some(c) = cur {
            order.push(sim.nodes[c].key);
            cur = sim.nodes[c].next;
        }
    }
    order
}

#[cfg(test)]
mod tests {
    use super::computeifabsent_order;

    #[test]
    fn small_map() {
        // exception table of a method with nested try/finally: expected order taken from the
        // reference reader
        let entries = [(0, 4), (0, 12), (22, 33), (0, 12), (22, 37), (47, 49)];
        assert_eq!(computeifabsent_order(&entries), vec![3, 1, 0, 2, 4]);
    }

    #[test]
    fn colliding_keys_use_tree_bins() {
        // 17 try blocks of an enum switch map: all keys collide, the order comes from the
        // reference reader
        let entries: Vec<(u32, u32)> = (0..17).map(|i| (9 + 15 * i, 20 + 15 * i)).collect();
        let order = computeifabsent_order(&entries);
        assert_eq!(
            order,
            vec![
                // (144,155) (234,245) (204,215) (174,185) (114,125) (84,95) (54,65) (24,35)
                // (129,140) (219,230) (249,260) (189,200) (159,170) (99,110) (69,80) (39,50)
                // (9,20) as indices of first appearance
                9, 15, 13, 11, 7, 5, 3, 1, 8, 14, 16, 12, 10, 6, 4, 2, 0
            ]
        );
    }
}
