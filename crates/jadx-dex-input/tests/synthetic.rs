//! Tests on a hand-assembled minimal DEX (no binary fixtures needed).

use std::sync::Arc;

use jadx_dex_input::{load_bytes, DexFile, LoadOptions};

fn put32(b: &mut [u8], off: usize, v: u32) {
    b[off..off + 4].copy_from_slice(&v.to_le_bytes());
}

/// One class `LA;` extending `Ljava/lang/Object;`, no code.
fn minimal_dex() -> Vec<u8> {
    let mut b = vec![0u8; 0x108];
    b[..8].copy_from_slice(b"dex\n035\0");
    put32(&mut b, 32, 0x108); // file_size
    put32(&mut b, 36, 0x70); // header_size
    put32(&mut b, 40, 0x1234_5678);
    put32(&mut b, 52, 0xBC); // map_off
    put32(&mut b, 56, 2); // string_ids
    put32(&mut b, 60, 0x70);
    put32(&mut b, 64, 2); // type_ids
    put32(&mut b, 68, 0x78);
    put32(&mut b, 96, 1); // class_defs
    put32(&mut b, 100, 0x80);
    put32(&mut b, 104, 0x108 - 0xA0); // data_size
    put32(&mut b, 108, 0xA0);
    // string_ids
    put32(&mut b, 0x70, 0xA0);
    put32(&mut b, 0x74, 0xA5);
    // type_ids
    put32(&mut b, 0x78, 0);
    put32(&mut b, 0x7C, 1);
    // class_def: class=0, flags=1, super=1, no interfaces, no source file
    put32(&mut b, 0x80, 0);
    put32(&mut b, 0x84, 1);
    put32(&mut b, 0x88, 1);
    put32(&mut b, 0x8C, 0);
    put32(&mut b, 0x90, 0xFFFF_FFFF);
    // string data
    b[0xA0..0xA5].copy_from_slice(&[3, b'L', b'A', b';', 0]);
    let obj = b"Ljava/lang/Object;";
    b[0xA5] = obj.len() as u8;
    b[0xA6..0xA6 + obj.len()].copy_from_slice(obj);
    // map list
    put32(&mut b, 0xBC, 6);
    let items: [(u16, u32, u32); 6] = [
        (0x0000, 1, 0),
        (0x0001, 2, 0x70),
        (0x0002, 2, 0x78),
        (0x0006, 1, 0x80),
        (0x2002, 2, 0xA0),
        (0x1000, 1, 0xBC),
    ];
    for (i, (kind, size, off)) in items.iter().enumerate() {
        let at = 0xC0 + i * 12;
        b[at..at + 2].copy_from_slice(&kind.to_le_bytes());
        put32(&mut b, at + 4, *size);
        put32(&mut b, at + 8, *off);
    }
    let sum = adler2::adler32_slice(&b[12..]);
    put32(&mut b, 8, sum);
    b
}

#[test]
fn parses_minimal_dex() {
    let res = load_bytes("min.dex", minimal_dex(), &LoadOptions::default(), true).unwrap();
    assert_eq!(res.dex_files.len(), 1);
    let dex = &res.dex_files[0];
    assert_eq!(dex.class_count(), 1);
    let def = dex.class_def(0).unwrap();
    assert_eq!(dex.type_descriptor(def.class_idx).unwrap(), "LA;");
    assert_eq!(
        dex.opt_type_descriptor(def.superclass_idx)
            .unwrap()
            .unwrap(),
        "Ljava/lang/Object;"
    );
    assert_eq!(dex.opt_string(def.source_file_idx).unwrap(), None);
    assert!(dex.type_list(def.interfaces_off).unwrap().is_empty());
    assert_eq!(dex.header().map.len(), 6);
}

#[test]
fn checksum_is_verified() {
    let mut b = minimal_dex();
    b[0xA1] ^= 0xFF;
    assert!(load_bytes("bad.dex", b.clone(), &LoadOptions::default(), true).is_err());
    let relaxed = LoadOptions {
        verify_checksum: false,
    };
    assert!(load_bytes("bad.dex", b, &relaxed, true).is_ok());
}

/// Exercise every accessor; must never panic whatever the bytes are.
fn poke(data: Vec<u8>) {
    let opts = LoadOptions {
        verify_checksum: false,
    };
    let Ok(res) = load_bytes("fuzz.dex", data, &opts, true) else {
        return;
    };
    for dex in &res.dex_files {
        walk(dex);
    }
}

fn walk(dex: &DexFile) {
    for i in 0..dex.string_count().min(64) {
        let _ = dex.string(i);
    }
    for i in 0..dex.type_count().min(64) {
        let _ = dex.type_descriptor(i);
    }
    for i in 0..dex.method_count().min(64) {
        let _ = dex.method_ref(i);
    }
    for i in 0..dex.field_count().min(64) {
        let _ = dex.field_ref(i);
    }
    for def in dex.class_defs().take(64).flatten() {
        let _ = dex.type_list(def.interfaces_off);
    }
}

#[test]
fn truncation_never_panics() {
    let b = minimal_dex();
    for len in 0..b.len() {
        poke(b[..len].to_vec());
    }
}

#[test]
fn corruption_never_panics() {
    let base = minimal_dex();
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    for _ in 0..20_000 {
        let mut b = base.clone();
        for _ in 0..1 + (state % 4) {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let pos = (state >> 33) as usize % b.len();
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            b[pos] = (state >> 56) as u8;
        }
        poke(b);
    }
}

#[test]
fn parse_from_shared_bytes() {
    // DexFile::parse accepts shared bytes directly.
    let d = DexFile::parse("x", Arc::from(minimal_dex()), 0).unwrap();
    assert_eq!(d.string_count(), 2);
}
