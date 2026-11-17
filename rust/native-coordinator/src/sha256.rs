//! Dependency-free incremental SHA-256 copied from the pinned SlopHawk
//! `rust/checkers/clone-index/src/sha256.rs` (blob 2dbc94c0f2e3ed492c259fc049bb26ea39eb409c).
//! The native pilot uses the digest for source evidence and state integrity.

const INIT: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];
const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

pub(crate) struct Sha256 {
    state: [u32; 8],
    bytes: u64,
    buffer: [u8; 64],
    used: usize,
}

impl Sha256 {
    pub(crate) fn new() -> Self {
        Self { state: INIT, bytes: 0, buffer: [0; 64], used: 0 }
    }
    pub(crate) fn update(&mut self, mut data: &[u8]) {
        self.bytes = self.bytes.wrapping_add(data.len() as u64);
        if self.used != 0 {
            let count = data.len().min(64 - self.used);
            self.buffer[self.used..self.used + count].copy_from_slice(&data[..count]);
            self.used += count;
            data = &data[count..];
            if self.used == 64 {
                compress(&mut self.state, &self.buffer);
                self.used = 0;
            }
        }
        while data.len() >= 64 {
            let (block, rest) = data.split_at(64);
            compress(&mut self.state, block);
            data = rest;
        }
        if !data.is_empty() {
            self.buffer[..data.len()].copy_from_slice(data);
            self.used = data.len();
        }
    }
    pub(crate) fn finish(mut self) -> [u8; 32] {
        let bit_length = self.bytes.wrapping_mul(8).to_be_bytes();
        self.update(&[0x80]);
        if self.used > 56 {
            let padding = 64 - self.used;
            self.update(&[0; 64][..padding]);
        }
        let zeros = 56 - self.used;
        self.update(&[0; 64][..zeros]);
        self.update(&bit_length);
        let mut out = [0; 32];
        for (i, part) in self.state.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&part.to_be_bytes());
        }
        out
    }
}

fn compress(state: &mut [u32; 8], block: &[u8]) {
    let mut w = [0u32; 64];
    for (i, chunk) in block.as_chunks::<4>().0.iter().enumerate() {
        w[i] = u32::from_be_bytes(*chunk);
    }
    for i in 16..64 {
        let a = w[i - 15];
        let b = w[i - 2];
        let s0 = a.rotate_right(7) ^ a.rotate_right(18) ^ (a >> 3);
        let s1 = b.rotate_right(17) ^ b.rotate_right(19) ^ (b >> 10);
        w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
    }
    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *state;
    for (word, k) in w.into_iter().zip(K) {
        let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let choose = (e & f) ^ (!e & g);
        let t1 = h.wrapping_add(s1).wrapping_add(choose).wrapping_add(k).wrapping_add(word);
        let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let majority = (a & b) ^ (a & c) ^ (b & c);
        let t2 = s0.wrapping_add(majority);
        h = g;
        g = f;
        f = e; // Required SHA-256 working-state shift (omitted in the rejected proposal).
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }
    for (dst, word) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
        *dst = dst.wrapping_add(word);
    }
}

pub(crate) fn digest(bytes: &[u8]) -> String {
    let mut hash = Sha256::new();
    hash.update(bytes);
    to_hex(&hash.finish())
}

pub(crate) fn to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        result.push(HEX[(b >> 4) as usize] as char);
        result.push(HEX[(b & 15) as usize] as char);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn vectors() {
        assert_eq!(digest(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(digest(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        // The 56-byte message crosses the padding-block boundary.
        assert_eq!(digest(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1");
        let mut fragmented = Sha256::new();
        for chunk in b"abc".chunks(1) { fragmented.update(chunk); }
        assert_eq!(to_hex(&fragmented.finish()),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(digest(&[b'a'; 1_000_000]), "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0");
    }
}
