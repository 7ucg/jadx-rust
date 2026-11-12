//! Method code: sizes, instructions, exception tables and debug info.

use std::collections::HashSet;

use jadx_input_api::{CatchData, InsnData, TryData};

use crate::attrs::{parse_attrs, AttrFilter};
use crate::class::JavaClass;
use crate::error::Result;
use crate::insns::Decoder;
use crate::types::{CodeInfo, DebugInfo, LocalVar};

/// Entries of the exception table: `(start, end, handler, catch type index)`.
type ExcEntry = (u32, u32, u32, u32);

impl JavaClass {
    /// Sizes of the `Code` attribute whose data starts at `code_offset`.
    pub fn code_info(&self, code_offset: u32) -> Result<CodeInfo> {
        let mut r = self.reader_at(code_offset as usize);
        let max_stack = r.u2()?;
        let max_locals = r.u2()?;
        let code_length = r.u4()?;
        Ok(CodeInfo {
            max_stack,
            max_locals,
            code_length,
        })
    }

    fn exception_table(&self, code_offset: u32) -> Result<(Vec<ExcEntry>, usize)> {
        let info = self.code_info(code_offset)?;
        let mut r = self.reader_at(code_offset as usize + 8 + info.code_length as usize);
        let n = r.u2()?;
        let mut out = Vec::with_capacity(n as usize);
        for _ in 0..n {
            let start = r.u2()?;
            let end = r.u2()?;
            let handler = r.u2()?;
            let ty = r.u2()?;
            out.push((start, end, handler, ty));
        }
        Ok((out, r.pos()))
    }

    /// Decode the instructions of the method code into the normalised form.
    pub fn decode_insns(&self, code_offset: u32) -> Result<Vec<InsnData>> {
        let info = self.code_info(code_offset)?;
        let (table, attrs_pos) = self.exception_table(code_offset)?;
        let handlers: HashSet<u32> = table.iter().map(|e| e.2).collect();
        let mut ar = self.reader_at(attrs_pos);
        let attrs = parse_attrs(self, &mut ar, AttrFilter::Only(&["StackMapTable"]))?;
        let frames = attrs.stack_map.unwrap_or_default();
        let mut dec = Decoder::new(
            self,
            code_offset as usize + 8,
            info.max_stack,
            handlers,
            frames,
        );
        dec.decode_all(info.code_length)
    }

    /// Try blocks of the method (`end` is the exclusive end offset of the class file).
    pub fn tries(&self, code_offset: u32) -> Result<Vec<TryData>> {
        let (table, _) = self.exception_table(code_offset)?;
        if table.is_empty() {
            return Ok(Vec::new());
        }
        // group the entries with the same range in order of appearance
        let mut groups: Vec<((u32, u32), Vec<(u32, Option<String>)>)> = Vec::new();
        for (start, end, handler, ty) in &table {
            let ty = if *ty == 0 {
                None
            } else {
                Some(self.class_name(*ty)?)
            };
            match groups.iter_mut().find(|(k, _)| *k == (*start, *end)) {
                Some((_, list)) => list.push((*handler, ty)),
                None => groups.push(((*start, *end), vec![(*handler, ty)])),
            }
        }
        let entries: Vec<(u32, u32)> = table.iter().map(|e| (e.0, e.1)).collect();
        let order = hash_map_order(&entries);
        let mut out = Vec::with_capacity(groups.len());
        for gi in order {
            let ((start, end), list) = &groups[gi];
            let mut catch_all: i64 = -1;
            let mut handlers = Vec::new();
            let mut types = Vec::new();
            let mut all_taken = false;
            for (handler, ty) in list {
                match ty {
                    None if !all_taken => {
                        catch_all = *handler as i64;
                        all_taken = true;
                    }
                    // a second catch-all stays in the list like in the reference
                    // (as an entry without type)
                    None => {
                        handlers.push(*handler);
                        types.push(String::new());
                    }
                    Some(t) => {
                        handlers.push(*handler);
                        types.push(t.clone());
                    }
                }
            }
            out.push(TryData {
                start: *start,
                end: *end,
                catch: CatchData {
                    handlers,
                    types,
                    catch_all,
                },
            });
        }
        Ok(out)
    }

    /// Line numbers and local variables of the method.
    pub fn debug_info(&self, code_offset: u32) -> Result<Option<DebugInfo>> {
        let info = self.code_info(code_offset)?;
        let (_, attrs_pos) = self.exception_table(code_offset)?;
        let mut ar = self.reader_at(attrs_pos);
        let attrs = parse_attrs(
            self,
            &mut ar,
            AttrFilter::Only(&[
                "LineNumberTable",
                "LocalVariableTable",
                "LocalVariableTypeTable",
            ]),
        )?;
        if attrs.line_numbers.is_none() && attrs.local_vars.is_none() {
            return Ok(None);
        }
        let lines = attrs.line_numbers.unwrap_or_default();
        let mut vars: Vec<LocalVar> = Vec::new();
        if let Some(raw) = &attrs.local_vars {
            let mut list: Vec<LocalVar> = raw
                .iter()
                .map(|v| LocalVar {
                    reg: v.reg,
                    name: v.name.clone(),
                    ty: v.ty.clone(),
                    signature: None,
                    start: v.start,
                    end: v.end,
                })
                .collect();
            for typed in attrs.local_var_types.iter().flatten() {
                // the last variable with the same identity gets the signature
                if let Some(v) = list.iter_mut().rev().find(|v| {
                    v.reg == typed.reg
                        && v.start == typed.start
                        && v.end == typed.end
                        && v.name == typed.name
                }) {
                    v.signature = typed.ty.clone();
                }
            }
            let shift = info.args_start_reg();
            for v in &mut list {
                v.reg += shift;
            }
            vars = list;
        }
        Ok(Some(DebugInfo { lines, vars }))
    }
}

/// Iteration order of the reference `HashMap<JavaTryData, ...>` that is filled with
/// `computeIfAbsent` for every exception table entry (`entries`, the map is created with
/// `entries.len()` as initial capacity). Returns the indices of the distinct keys (in order of
/// first appearance).
///
/// `computeIfAbsent` resizes lazily (before an insertion when the size exceeds the threshold)
/// and inserts a new node at the head of its bucket, so within a bucket later keys come first.
fn hash_map_order(entries: &[(u32, u32)]) -> Vec<usize> {
    let hash = |k: &(u32, u32)| -> u32 {
        let h = k.0.wrapping_add(31u32.wrapping_mul(k.1));
        h ^ (h >> 16)
    };
    let mut cap = 0usize;
    let mut thr = entries.len().max(1).next_power_of_two();
    let resize = |cap: &mut usize, thr: &mut usize| {
        let old_cap = *cap;
        let old_thr = *thr;
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
        *cap = new_cap;
        *thr = new_thr;
    };
    let mut keys: Vec<(u32, u32)> = Vec::new();
    for e in entries {
        if keys.len() > thr || cap == 0 {
            resize(&mut cap, &mut thr);
        }
        if keys.contains(e) {
            continue;
        }
        let bucket = hash(e) as usize & (cap - 1);
        let chain = keys
            .iter()
            .filter(|k| hash(k) as usize & (cap - 1) == bucket)
            .count();
        keys.push(*e);
        if chain >= 8 && cap < 64 {
            resize(&mut cap, &mut thr);
        }
    }
    let cap = cap.max(1);
    let mut order: Vec<usize> = (0..keys.len()).collect();
    order.sort_by_key(|i| (hash(&keys[*i]) as usize & (cap - 1), std::cmp::Reverse(*i)));
    order
}

#[cfg(test)]
mod tests {
    use super::hash_map_order;

    #[test]
    fn hash_order_matches_reference() {
        // exception table of a method with nested try/finally: expected order taken from the
        // reference reader
        let entries = [(0, 4), (0, 12), (22, 33), (0, 12), (22, 37), (47, 49)];
        assert_eq!(hash_map_order(&entries), vec![3, 1, 0, 2, 4]);
    }
}
