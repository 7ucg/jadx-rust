//! Minimal growable bit set (positions of blocks).

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BitSet {
    words: Vec<u64>,
}

impl BitSet {
    pub fn new() -> Self {
        BitSet { words: Vec::new() }
    }

    pub fn get(&self, i: usize) -> bool {
        self.words
            .get(i / 64)
            .is_some_and(|w| w & (1 << (i % 64)) != 0)
    }

    pub fn set(&mut self, i: usize) {
        let w = i / 64;
        if self.words.len() <= w {
            self.words.resize(w + 1, 0);
        }
        self.words[w] |= 1 << (i % 64);
    }

    pub fn clear(&mut self, i: usize) {
        if let Some(w) = self.words.get_mut(i / 64) {
            *w &= !(1 << (i % 64));
        }
    }

    pub fn or(&mut self, other: &BitSet) {
        if self.words.len() < other.words.len() {
            self.words.resize(other.words.len(), 0);
        }
        for (a, b) in self.words.iter_mut().zip(&other.words) {
            *a |= *b;
        }
    }

    pub fn is_empty(&self) -> bool {
        self.words.iter().all(|w| *w == 0)
    }

    pub fn and(&mut self, other: &BitSet) {
        for (i, w) in self.words.iter_mut().enumerate() {
            *w &= other.words.get(i).copied().unwrap_or(0);
        }
    }

    pub fn and_not(&mut self, other: &BitSet) {
        for (i, w) in self.words.iter_mut().enumerate() {
            *w &= !other.words.get(i).copied().unwrap_or(0);
        }
    }

    pub fn clear_all(&mut self) {
        self.words.iter_mut().for_each(|w| *w = 0);
    }

    pub fn set_range(&mut self, from: usize, to: usize) {
        for i in from..to {
            self.set(i);
        }
    }

    pub fn cardinality(&self) -> usize {
        self.words.iter().map(|w| w.count_ones() as usize).sum()
    }

    pub fn next_set_bit(&self, from: usize) -> Option<usize> {
        self.iter().find(|i| *i >= from)
    }

    /// Set bits in ascending order.
    pub fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        self.words.iter().enumerate().flat_map(|(wi, w)| {
            (0..64)
                .filter(move |b| w & (1u64 << b) != 0)
                .map(move |b| wi * 64 + b)
        })
    }
}

/// Emulates the iteration order of a `java.util.HashSet` of small non-negative integers
/// (hash code = value): buckets in ascending index, insertion order inside a bucket.
#[derive(Debug, Clone)]
pub struct JavaIntSet {
    items: Vec<usize>,
    cap: usize,
}

impl Default for JavaIntSet {
    fn default() -> Self {
        JavaIntSet {
            items: Vec::new(),
            cap: 16,
        }
    }
}

impl JavaIntSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, v: usize) -> bool {
        if self.items.contains(&v) {
            return false;
        }
        self.items.push(v);
        if self.items.len() > self.cap * 3 / 4 {
            self.cap *= 2;
        }
        true
    }

    pub fn contains(&self, v: usize) -> bool {
        self.items.contains(&v)
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Elements in Java iteration order.
    pub fn iter(&self) -> Vec<usize> {
        let mut v = self.items.clone();
        let mask = self.cap - 1;
        // `HashMap.hash`: h ^ (h >>> 16)
        v.sort_by_key(|x| (*x ^ (*x >> 16)) & mask);
        v
    }

    pub fn contains_all(&self, other: &JavaIntSet) -> bool {
        other.items.iter().all(|x| self.contains(*x))
    }
}

/// `Arrays.sort` / `List.sort` emulation for short lists (binary insertion sort after run
/// detection), so that inconsistent comparators produce the same order as the reference.
pub fn java_sort<T: Clone>(a: &mut [T], cmp: impl Fn(&T, &T) -> i32) {
    let n = a.len();
    if n < 2 {
        return;
    }
    // countRunAndMakeAscending
    let mut run_hi = 1usize;
    if cmp(&a[run_hi], &a[0]) < 0 {
        run_hi += 1;
        while run_hi < n && cmp(&a[run_hi], &a[run_hi - 1]) < 0 {
            run_hi += 1;
        }
        a[..run_hi].reverse();
    } else {
        run_hi += 1;
        while run_hi < n && cmp(&a[run_hi], &a[run_hi - 1]) >= 0 {
            run_hi += 1;
        }
    }
    // binarySort
    let mut start = run_hi;
    if start == 0 {
        start = 1;
    }
    while start < n {
        let pivot = a[start].clone();
        let (mut left, mut right) = (0usize, start);
        while left < right {
            let mid = (left + right) >> 1;
            if cmp(&pivot, &a[mid]) < 0 {
                right = mid;
            } else {
                left = mid + 1;
            }
        }
        for i in (left..start).rev() {
            a[i + 1] = a[i].clone();
        }
        a[left] = pivot;
        start += 1;
    }
}

/// Java `String.hashCode`
pub fn java_string_hash(s: &str) -> i32 {
    s.encode_utf16()
        .fold(0i32, |h, c| h.wrapping_mul(31).wrapping_add(c as i32))
}

/// Iteration order of a `java.util.HashMap` filled via `computeIfAbsent` in the given order:
/// new keys are prepended to their bucket (unlike `put`) and the table is resized before an
/// insertion when `size > threshold`. Returns indexes into `hashes`.
pub fn java_hash_map_order_compute(hashes: &[i32]) -> Vec<usize> {
    let mut cap = 16usize;
    let mut table: Vec<Vec<usize>> = vec![Vec::new(); cap];
    let bucket = |h: i32, cap: usize| {
        let h = h as u32;
        ((h ^ (h >> 16)) as usize) & (cap - 1)
    };
    for (size, (i, h)) in hashes.iter().enumerate().enumerate() {
        if size > cap * 3 / 4 {
            let new_cap = cap * 2;
            let mut nt: Vec<Vec<usize>> = vec![Vec::new(); new_cap];
            for bin in &table {
                for &k in bin {
                    nt[bucket(hashes[k], new_cap)].push(k);
                }
            }
            table = nt;
            cap = new_cap;
        }
        table[bucket(*h, cap)].insert(0, i);
    }
    table.into_iter().flatten().collect()
}

/// Iteration order of a `java.util.HashMap` filled with `put` in the given order: indexes of `hashes`.
pub fn java_hash_map_order(hashes: &[i32]) -> Vec<usize> {
    let mut cap = 16usize;
    while hashes.len() > cap * 3 / 4 {
        cap *= 2;
    }
    let mask = cap - 1;
    let mut idx: Vec<usize> = (0..hashes.len()).collect();
    idx.sort_by_key(|i| {
        let h = hashes[*i] as u32;
        ((h ^ (h >> 16)) as usize) & mask
    });
    idx
}
