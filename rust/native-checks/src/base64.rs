//! Strict RFC 4648 standard Base64 used only to decode explicitly enumerated source bytes.
const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
fn value(b: u8) -> Option<u8> {
    match b {
        b'A'..=b'Z' => Some(b - b'A'),
        b'a'..=b'z' => Some(b - b'a' + 26),
        b'0'..=b'9' => Some(b - b'0' + 52),
        b'+' => Some(62), b'/' => Some(63), _ => None,
    }
}
pub fn decode(text: &str) -> Result<Vec<u8>, String> {
    let input = text.as_bytes();
    if input.len() % 4 != 0 { return Err("base64 requires four-byte framing".into()); }
    let mut out = Vec::with_capacity(input.len()/4*3);
    for (i, block) in input.chunks_exact(4).enumerate() {
        let final_block = i == input.len()/4-1;
        let a = value(block[0]).ok_or("base64 alphabet")?;
        let b = value(block[1]).ok_or("base64 alphabet")?;
        let c = if block[2] == b'=' { None } else { Some(value(block[2]).ok_or("base64 alphabet")?) };
        let d = if block[3] == b'=' { None } else { Some(value(block[3]).ok_or("base64 alphabet")?) };
        if (c.is_none() || d.is_none()) && !final_block { return Err("base64 padding before end".into()); }
        if c.is_none() && (d.is_some() || b&15 != 0) { return Err("invalid canonical base64 padding".into()); }
        if c.is_some() && d.is_none() && c.unwrap_or(0)&3 != 0 { return Err("invalid trailing base64 bits".into()); }
        out.push(a<<2 | b>>4);
        if let Some(c) = c { out.push((b&15)<<4 | c>>2); }
        if let (Some(c),Some(d)) = (c,d) { out.push((c&3)<<6 | d); }
    }
    Ok(out)
}
pub fn encode(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len().div_ceil(3)*4);
    for block in input.chunks(3) {
        let a = block[0];
        let b = block.get(1).copied().unwrap_or(0);
        let c = block.get(2).copied().unwrap_or(0);
        out.push(TABLE[(a>>2) as usize] as char);
        out.push(TABLE[(((a&3)<<4)|(b>>4)) as usize] as char);
        out.push(if block.len()>1 { TABLE[(((b&15)<<2)|(c>>6)) as usize] as char } else { '=' });
        out.push(if block.len()>2 { TABLE[(c&63) as usize] as char } else { '=' });
    }
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canonical_binary_and_padding() {
        for data in [b"".as_slice(),b"f",b"fo",b"foo",b"fool", &[0,255,187,253,123][..]] {
            assert_eq!(decode(&encode(data)).unwrap(),data);
        }
        for bad in ["a", "Zg==AAAA", "Zh==", "Zg=A", "Z===", "!AAA", "Zg=\n"] { assert!(decode(bad).is_err(),"{bad}"); }
    }
}
