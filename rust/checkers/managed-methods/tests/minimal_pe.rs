//! A hand-constructed text-only ECMA-335 specimen builder. No compiled binary
//! is committed. Expected text was derived by reading the pinned C# source;
//! the owner will run pinned-source differential verification at integration.
use slophawk_managed_methods::{ErrorKind, compare, differences_json, integer_literals_for_names};

fn put_u16(v: &mut [u8], at: usize, x: u16) { v[at..at + 2].copy_from_slice(&x.to_le_bytes()); }
fn put_u32(v: &mut [u8], at: usize, x: u32) { v[at..at + 4].copy_from_slice(&x.to_le_bytes()); }
fn add_u16(v: &mut Vec<u8>, x: u16) { v.extend_from_slice(&x.to_le_bytes()); }
fn add_u32(v: &mut Vec<u8>, x: u32) { v.extend_from_slice(&x.to_le_bytes()); }
fn add_u64(v: &mut Vec<u8>, x: u64) { v.extend_from_slice(&x.to_le_bytes()); }
fn stream_name(name: &str) -> Vec<u8> {
    let mut bytes = name.as_bytes().to_vec();
    bytes.push(0);
    while !bytes.len().is_multiple_of(4) { bytes.push(0); }
    bytes
}
fn specimen(il: &[u8]) -> Vec<u8> {
    assert!(il.len() <= 63);
    let mut tables = Vec::new();
    add_u32(&mut tables, 0);
    tables.extend_from_slice(&[2, 0, 0, 1]); // metadata table major/minor/heap sizes/reserved
    add_u64(&mut tables, (1 << 0) | (1 << 2) | (1 << 6));
    add_u64(&mut tables, 0);
    for _ in 0..3 { add_u32(&mut tables, 1); }
    // Module: generation, name, MVID GUID, EncId, EncBaseId
    for i in [0, 7, 1, 0, 0] { add_u16(&mut tables, i); }
    // TypeDef: flags, name, namespace, extends, field-list, method-list
    add_u32(&mut tables, 1);
    for i in [1, 0, 0, 1, 1] { add_u16(&mut tables, i); }
    // MethodDef: RVA, ImplFlags, Flags, Name, Signature, ParamList
    add_u32(&mut tables, 0x2300);
    for i in [0, 0x96, 3, 1, 1] { add_u16(&mut tables, i); }
    let streams: [(&str, Vec<u8>); 5] = [
        ("#~", tables), ("#Strings", b"\0T\0Foo\0m\0".to_vec()),
        ("#Blob", b"\0\x03\x00\x00\x08".to_vec()),
        ("#GUID", (0u8..16).collect()), ("#US", vec![0]),
    ];
    let version = b"v4.0.30319\0\0";
    let mut root = Vec::new();
    add_u32(&mut root, 0x424a_5342); // BSJB
    add_u16(&mut root, 1); add_u16(&mut root, 1);
    add_u32(&mut root, 0); add_u32(&mut root, version.len() as u32);
    root.extend_from_slice(version);
    add_u16(&mut root, 0); add_u16(&mut root, streams.len() as u16);
    let mut offset = root.len() + streams.iter().map(|(n, _)| 8 + stream_name(n).len()).sum::<usize>();
    for (name, bytes) in &streams {
        add_u32(&mut root, offset as u32);
        add_u32(&mut root, bytes.len() as u32);
        root.extend_from_slice(&stream_name(name));
        offset += bytes.len();
    }
    for (_, bytes) in &streams { root.extend_from_slice(bytes); }
    assert!(root.len() < 0x100);
    let mut pe = vec![0; 0x2200];
    pe[0..2].copy_from_slice(b"MZ");
    put_u32(&mut pe, 0x3c, 0x80);
    pe[0x80..0x84].copy_from_slice(b"PE\0\0");
    put_u16(&mut pe, 0x84, 0x14c); // x86 PE32
    put_u16(&mut pe, 0x86, 1);      // one section
    put_u16(&mut pe, 0x94, 0xe0);  // SizeOfOptionalHeader
    put_u16(&mut pe, 0x96, 0x210e);
    let opt = 0x98;
    put_u16(&mut pe, opt, 0x10b); // PE32
    put_u32(&mut pe, opt + 92, 16); // 16 data directories
    put_u32(&mut pe, opt + 96 + 14 * 8, 0x2100); // CLI RVA
    put_u32(&mut pe, opt + 96 + 14 * 8 + 4, 0x48);
    let sec = opt + 0xe0;
    pe[sec..sec + 8].copy_from_slice(b".text\0\0\0");
    put_u32(&mut pe, sec + 8, 0x2000);
    put_u32(&mut pe, sec + 12, 0x2000);
    put_u32(&mut pe, sec + 16, 0x2000);
    put_u32(&mut pe, sec + 20, 0x200);
    put_u32(&mut pe, sec + 36, 0x6000_0020);
    put_u32(&mut pe, 0x300, 0x48); // CLI header
    put_u16(&mut pe, 0x304, 2); put_u16(&mut pe, 0x306, 5);
    put_u32(&mut pe, 0x308, 0x2200); put_u32(&mut pe, 0x30c, root.len() as u32);
    put_u32(&mut pe, 0x310, 1); // ILOnly
    pe[0x400..0x400 + root.len()].copy_from_slice(&root);
    pe[0x500] = (il.len() as u8) << 2 | 2; // tiny method
    pe[0x501..0x501 + il.len()].copy_from_slice(il);
    pe
}

#[test]
fn minimal_managed_pe_csharp_reference_parity() {
    let left = specimen(&[0x18, 0x2a]);
    let right = specimen(&[0x17, 0x2a]);
    let result = differences_json(&compare(&left, &right).unwrap()).unwrap();
    assert_eq!(result, r#"[{"Method":"T::Foo():Int32[00:0:0]","Name":"Foo","Change":"changed","CommonPrefixCount":0,"Candidate":["ldc.i4.2","ret"],"Reference":["ldc.i4.1","ret"],"IntegerLiterals":[1,2]}]"#);
    assert!(compare(&left, &left).unwrap().is_empty());
    assert_eq!(integer_literals_for_names(&left, &["Foo".into()]).unwrap()[0].literals, vec![2]);
}

#[test]
fn malformed_metadata_and_method_bodies_are_rejected() {
    let good = specimen(&[0x17, 0x2a]);
    let mut bad = good.clone();
    bad[0x400] ^= 0xff; // BSJB signature
    assert_eq!(compare(&bad, &good).unwrap_err().kind, ErrorKind::Malformed);
    let short = &good[..0x301]; // PE/metadata directory no longer backed
    assert_eq!(compare(short, &good).unwrap_err().kind, ErrorKind::Malformed);
    let mut truncated = specimen(&[0x20, 0x01]); // ldc.i4, insufficient inline 4-byte operand
    assert_eq!(compare(&truncated, &good).unwrap_err().kind, ErrorKind::Malformed);
    // Raw section stops exactly after the tiny header and two IL bytes.
    put_u32(&mut truncated, 0x98 + 0xe0 + 16, 0x303);
    truncated[0x500] = (3 << 2) | 2; // claims three bytes, section exposes only two
    assert_eq!(compare(&truncated, &good).unwrap_err().kind, ErrorKind::Malformed);
    assert_eq!(compare(b"not a PE", &good).unwrap_err().kind, ErrorKind::NonManaged);
}

#[test]
fn missing_method_body_is_not_treated_as_a_method_with_empty_il() {
    let real = specimen(&[0x17, 0x2a]);
    let mut no_body = real.clone();
    // RVA of first MethodDef in the fixed one-method metadata table.
    let metadata = &real[0x400..];
    let needle = 0x2300u32.to_le_bytes();
    let row_offset = metadata.windows(4).position(|s| s == needle).expect("MethodDef RVA");
    put_u32(&mut no_body, 0x400 + row_offset, 0);
    let differences = compare(&real, &no_body).unwrap();
    assert_eq!(differences.len(), 1);
    assert_eq!(differences[0].change, "added");
    assert_eq!(differences[0].reference, Vec::<String>::new());
}
