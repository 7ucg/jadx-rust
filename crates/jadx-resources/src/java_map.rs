//! Insertion map that iterates in the order of a `java.util.HashMap<String, String>`.
//!
//! The reference writes namespace declarations while iterating a hash map, so the output order
//! depends on `String.hashCode` and the table size. Entries are never removed.

#[derive(Debug, Clone)]
pub struct JavaOrderMap {
    entries: Vec<(String, String)>,
    capacity: usize,
}

impl Default for JavaOrderMap {
    fn default() -> Self {
        Self::new()
    }
}

/// `String.hashCode`
pub fn java_string_hash(s: &str) -> i32 {
    let mut h: i32 = 0;
    for u in s.encode_utf16() {
        h = h.wrapping_mul(31).wrapping_add(i32::from(u));
    }
    h
}

impl JavaOrderMap {
    pub fn new() -> Self {
        JavaOrderMap {
            entries: Vec::new(),
            capacity: 16,
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    pub fn contains_value(&self, value: &str) -> bool {
        self.entries.iter().any(|(_, v)| v == value)
    }

    pub fn put(&mut self, key: &str, value: &str) {
        if let Some(e) = self.entries.iter_mut().find(|(k, _)| k == key) {
            e.1 = value.to_string();
            return;
        }
        self.entries.push((key.to_string(), value.to_string()));
        if self.entries.len() > self.capacity / 4 * 3 {
            self.capacity *= 2;
        }
    }

    pub fn put_if_absent(&mut self, key: &str, value: &str) {
        if self.get(key).is_none() {
            self.put(key, value);
        }
    }

    /// Entries in `HashMap` iteration order.
    pub fn iter_java_order(&self) -> Vec<(String, String)> {
        let mask = self.capacity as u32 - 1;
        let mut v: Vec<(u32, &(String, String))> = self
            .entries
            .iter()
            .map(|e| {
                let h = java_string_hash(&e.0) as u32;
                ((h ^ (h >> 16)) & mask, e)
            })
            .collect();
        v.sort_by_key(|(bucket, _)| *bucket);
        v.into_iter().map(|(_, e)| e.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_hash() {
        assert_eq!(java_string_hash("android"), -861391249);
        assert_eq!(java_string_hash(""), 0);
    }

    #[test]
    fn order_follows_buckets() {
        let mut m = JavaOrderMap::new();
        m.put("b", "1");
        m.put("a", "2");
        let order: Vec<String> = m.iter_java_order().into_iter().map(|e| e.0).collect();
        // "a" = 97 -> bucket 1, "b" = 98 -> bucket 2
        assert_eq!(order, vec!["a", "b"]);
    }
}
