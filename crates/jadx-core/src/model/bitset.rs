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
