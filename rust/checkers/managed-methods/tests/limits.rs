//! Structural, source-authored adversarial PE images. These are data fixtures,
//! never executable assembly tests. They are only evaluated by the owner.
use slophawk_managed_methods::{ErrorKind, compare, integer_literals_for_names};

fn put_u16(b: &mut [u8], at: usize, value: u16) {
    b[at..at + 2].copy_from_slice(&value.to_le_bytes());
}
fn put_u32(b: &mut [u8], at: usize, value: u32) {
    b[at..at + 4].copy_from_slice(&value.to_le_bytes());
}
fn add_u16(b: &mut Vec<u8>, value: u16) { b.extend_from_slice(&value.to_le_bytes()); }
fn add_u32(b: &mut Vec<u8>, value: u32) { b.extend_from_slice(&value.to_le_bytes()); }
fn add_u64(b: &mut Vec<u8>, value: u64) { b.extend_from_slice(&value.to_le_bytes()); }
fn align4(n: usize) -> usize { (n + 3) & !3 }
fn align512(n: usize) -> usize { (n + 511) & !511 }
fn compressed(value: u32) -> Vec<u8> {
    match value {
        0..=0x7f => vec![value as u8],
        0x80..=0x3fff => vec![(0x80 | (value >> 8)) as u8, value as u8],
        0x4000..=0x1fff_ffff => vec![(0xc0 | (value >> 24)) as u8,
            (value >> 16) as u8, (value >> 8) as u8, value as u8],
        _ => panic!("fixture compressed integer exceeds CLI range"),
    }
}
fn stream_name(name: &str) -> Vec<u8> {
    let mut bytes = name.as_bytes().to_vec();
    bytes.push(0);
    bytes.resize(align4(bytes.len()), 0);
    bytes
}
struct Layout { table: usize, type_row: usize, blob: usize }
fn specimen(il: &[u8], user_string: &str, type_specs: &[Vec<u8>]) -> (Vec<u8>, Layout) {
    assert!(il.len() < 0x700);
    assert!(type_specs.len() <= 64);
    let mut blobs = vec![0, 3, 0, 0, 8]; // index 1: method () -> Int32
    let mut type_indices = Vec::new();
    for signature in type_specs {
        type_indices.push(blobs.len() as u16);
        blobs.extend(compressed(signature.len() as u32));
        blobs.extend(signature);
    }
    let mut tables = Vec::new();
    add_u32(&mut tables, 0);
    tables.extend_from_slice(&[2, 0, 0, 1]);
    let valid = (1u64 << 0) | (1u64 << 2) | (1u64 << 6) |
        if type_specs.is_empty() { 0 } else { 1u64 << 27 };
    add_u64(&mut tables, valid);
    add_u64(&mut tables, 0);
    for _ in 0..3 { add_u32(&mut tables, 1); }
    if !type_specs.is_empty() { add_u32(&mut tables, type_specs.len() as u32); }
    let type_row = tables.len() + 10; // module row precedes type row
    for value in [0, 7, 1, 0, 0] { add_u16(&mut tables, value); }
    add_u32(&mut tables, 1); // TypeDef flags
    for value in [1, 0, 0, 1, 1] { add_u16(&mut tables, value); }
    add_u32(&mut tables, 0x2600); // method RVA (file offset 0x800)
    for value in [0, 0x96, 3, 1, 1] { add_u16(&mut tables, value); }
    for index in type_indices { add_u16(&mut tables, index); }

    let mut us = vec![0u8];
    if !user_string.is_empty() {
        let units: Vec<_> = user_string.encode_utf16().collect();
        us.extend(compressed((units.len() * 2 + 1) as u32));
        for u in units { add_u16(&mut us, u); }
        us.push(0); // user-string trailing flag byte
    }
    let streams: [(&str, Vec<u8>); 5] = [
        ("#~", tables), ("#Strings", b"\0T\0Foo\0".to_vec()),
        ("#Blob", blobs), ("#GUID", (0u8..16).collect()), ("#US", us),
    ];
    let version = b"v4.0.30319\0\0";
    let mut root = Vec::new();
    add_u32(&mut root, 0x424a_5342);
    add_u16(&mut root, 1); add_u16(&mut root, 1);
    add_u32(&mut root, 0); add_u32(&mut root, version.len() as u32);
    root.extend_from_slice(version);
    add_u16(&mut root, 0); add_u16(&mut root, streams.len() as u16);
    let mut offset = root.len() + streams.iter().map(|(n, _)| 8 + stream_name(n).len()).sum::<usize>();
    let mut stream_positions = Vec::new();
    for (name, content) in &streams {
        stream_positions.push(offset);
        add_u32(&mut root, offset as u32);
        add_u32(&mut root, content.len() as u32);
        root.extend_from_slice(&stream_name(name));
        offset += content.len();
    }
    for (_, content) in &streams { root.extend(content); }
    let metadata_at = 0x1000;
    let raw_size = align512(metadata_at + root.len()) - 0x200;
    let mut image = vec![0; 0x200 + raw_size];
    image[0..2].copy_from_slice(b"MZ");
    put_u32(&mut image, 0x3c, 0x80);
    image[0x80..0x84].copy_from_slice(b"PE\0\0");
    put_u16(&mut image, 0x84, 0x14c);
    put_u16(&mut image, 0x86, 1);
    put_u16(&mut image, 0x94, 0xe0);
    put_u16(&mut image, 0x96, 0x210e);
    let opt = 0x98;
    put_u16(&mut image, opt, 0x10b);
    put_u32(&mut image, opt + 92, 16);
    put_u32(&mut image, opt + 96 + 14 * 8, 0x2100);
    put_u32(&mut image, opt + 96 + 14 * 8 + 4, 0x48);
    let sec = opt + 0xe0;
    image[sec..sec + 8].copy_from_slice(b".text\0\0\0");
    put_u32(&mut image, sec + 8, raw_size as u32);
    put_u32(&mut image, sec + 12, 0x2000);
    put_u32(&mut image, sec + 16, raw_size as u32);
    put_u32(&mut image, sec + 20, 0x200);
    put_u32(&mut image, sec + 36, 0x6000_0020);
    put_u32(&mut image, 0x300, 0x48);
    put_u16(&mut image, 0x304, 2); put_u16(&mut image, 0x306, 5);
    put_u32(&mut image, 0x308, 0x2e00);
    put_u32(&mut image, 0x30c, root.len() as u32);
    put_u32(&mut image, 0x310, 1); // ILOnly
    put_u16(&mut image, 0x800, 0x3003); // fat method header (3 dwords)
    put_u16(&mut image, 0x802, 8);
    put_u32(&mut image, 0x804, il.len() as u32);
    image[0x80c..0x80c + il.len()].copy_from_slice(il);
    image[metadata_at..metadata_at + root.len()].copy_from_slice(&root);
    let layout = Layout {
        table: metadata_at + stream_positions[0],
        type_row: metadata_at + stream_positions[0] + type_row,
        blob: metadata_at + stream_positions[2],
    };
    (image, layout)
}

#[test]
fn repeated_large_ldstr_is_charged_during_each_decode() {
    let mut il = Vec::new();
    for _ in 0..160 { il.extend_from_slice(&[0x72, 1, 0, 0, 0x70]); }
    il.push(0x2a);
    let (image, _) = specimen(&il, &"q".repeat(96 * 1024), &[]);
    let err = compare(&image, &image).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Limit);
}

fn type_spec_dag() -> Vec<Vec<u8>> {
    let mut list = vec![vec![0x08]]; // first node is Int32
    for index in 1..31u32 {
        let previous = (index << 2) | 2; // TypeSpec index, TypeDefOrRef tag 2
        let mut signature = vec![0x15, 0x12, 4, 2, 0x12];
        signature.extend(compressed(previous));
        signature.push(0x12);
        signature.extend(compressed(previous));
        list.push(signature);
    }
    list
}
#[test]
fn acyclic_exponential_signature_fails_before_text_expansion() {
    let token = (27u32 << 24) | 31;
    let mut il = vec![0xd0]; il.extend_from_slice(&token.to_le_bytes()); il.push(0x2a);
    let (image, _) = specimen(&il, "", &type_spec_dag());
    assert_eq!(compare(&image, &image).unwrap_err().kind, ErrorKind::Limit);
}
#[test]
fn cyclic_type_spec_is_malformed_without_recursion_exhaustion() {
    let token = (27u32 << 24) | 1;
    let mut il = vec![0xd0]; il.extend_from_slice(&token.to_le_bytes()); il.push(0x2a);
    let (image, _) = specimen(&il, "", &[vec![0x12, 0x06]]);
    assert_eq!(compare(&image, &image).unwrap_err().kind, ErrorKind::Malformed);
}
#[test]
fn member_ranges_tables_and_overlong_compressed_integers_are_rejected() {
    let (image, layout) = specimen(&[0x17, 0x2a], "", &[]);
    assert!(compare(&image, &image).unwrap().is_empty());
    let mut bad_range = image.clone();
    put_u16(&mut bad_range, layout.type_row + 12, 0);
    assert_eq!(compare(&bad_range, &image).unwrap_err().kind, ErrorKind::Malformed);
    let mut bad_table = image.clone();
    put_u32(&mut bad_table, layout.table + 24 + 8, 0xffff_ffff);
    assert_eq!(compare(&bad_table, &image).unwrap_err().kind, ErrorKind::Limit);
    let mut bad_blob = image.clone();
    bad_blob[layout.blob + 1] = 0x80;
    bad_blob[layout.blob + 2] = 0x01; // noncanonical 2-byte length for 1
    assert_eq!(compare(&bad_blob, &image).unwrap_err().kind, ErrorKind::Malformed);
    assert!(integer_literals_for_names(&image, &["Foo".into()]).is_ok());
}
