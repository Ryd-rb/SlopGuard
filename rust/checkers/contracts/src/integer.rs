//! Bounded signed two's-complement arithmetic, without machine-width shortcuts.
//! A value is a normalized sign and little-endian base-2^32 magnitude.
use std::cmp::Ordering;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Integer {
    negative: bool,
    words: Vec<u32>,
}

impl Integer {
    pub(crate) fn zero() -> Self {
        Self {
            negative: false,
            words: Vec::new(),
        }
    }

    pub(crate) fn one() -> Self {
        Self::from_i64(1)
    }

    pub(crate) fn from_i64(value: i64) -> Self {
        let n = value.unsigned_abs();
        let mut words = vec![n as u32, (n >> 32) as u32];
        trim(&mut words);
        Self {
            negative: value < 0,
            words,
        }
    }

    pub(crate) fn decimal(text: &str) -> Result<Self, &'static str> {
        if text.is_empty() || text.len() > 39 || !text.bytes().all(|b| b.is_ascii_digit()) {
            return Err("invalid integer constant");
        }
        let mut result = Self::zero();
        for byte in text.bytes() {
            result = result.mul_small(10);
            result = result.add(&Self::from_i64((byte - b'0') as i64));
        }
        Ok(result)
    }

    fn normalized(negative: bool, mut words: Vec<u32>) -> Self {
        trim(&mut words);
        Self {
            negative: negative && !words.is_empty(),
            words,
        }
    }

    fn mul_small(&self, multiplier: u32) -> Self {
        let mut output = Vec::with_capacity(self.words.len() + 1);
        let mut carry = 0_u64;
        for &word in &self.words {
            let value = u64::from(word) * u64::from(multiplier) + carry;
            output.push(value as u32);
            carry = value >> 32;
        }
        if carry != 0 {
            output.push(carry as u32);
        }
        Self::normalized(self.negative, output)
    }

    pub(crate) fn is_zero(&self) -> bool {
        self.words.is_empty()
    }
    pub(crate) fn negative(&self) -> bool {
        self.negative
    }
    pub(crate) fn abs(&self) -> Self {
        let mut copy = self.clone();
        copy.negative = false;
        copy
    }
    pub(crate) fn negated(&self) -> Self {
        let mut copy = self.clone();
        if !copy.is_zero() {
            copy.negative = !copy.negative;
        }
        copy
    }

    pub(crate) fn bits(&self) -> usize {
        let Some(&last) = self.words.last() else {
            return 0;
        };
        (self.words.len() - 1) * 32 + (32 - last.leading_zeros() as usize)
    }

    // .NET GetBitLength excludes the sign bit. For -2^k it is k, for -1 it is 0.
    pub(crate) fn twos_complement_bits(&self) -> usize {
        if !self.negative {
            return self.bits();
        }
        self.abs().sub(&Self::one()).bits()
    }

    fn cmp_magnitude(&self, other: &Self) -> Ordering {
        self.words
            .len()
            .cmp(&other.words.len())
            .then_with(|| self.words.iter().rev().cmp(other.words.iter().rev()))
    }

    pub(crate) fn compare(&self, other: &Self) -> Ordering {
        match (self.negative, other.negative) {
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            (false, false) => self.cmp_magnitude(other),
            (true, true) => other.cmp_magnitude(self),
        }
    }

    pub(crate) fn add(&self, other: &Self) -> Self {
        if self.negative == other.negative {
            return Self::normalized(self.negative, add_abs(&self.words, &other.words));
        }
        match self.cmp_magnitude(other) {
            Ordering::Greater => {
                Self::normalized(self.negative, sub_abs(&self.words, &other.words))
            }
            Ordering::Less => Self::normalized(other.negative, sub_abs(&other.words, &self.words)),
            Ordering::Equal => Self::zero(),
        }
    }

    pub(crate) fn sub(&self, other: &Self) -> Self {
        self.add(&other.negated())
    }

    pub(crate) fn multiply(&self, other: &Self) -> Self {
        if self.is_zero() || other.is_zero() {
            return Self::zero();
        }
        let mut words = vec![0_u32; self.words.len() + other.words.len()];
        for (i, &left) in self.words.iter().enumerate() {
            let mut carry = 0_u64;
            for (j, &right) in other.words.iter().enumerate() {
                let at = i + j;
                let sum = u64::from(left) * u64::from(right) + u64::from(words[at]) + carry;
                words[at] = sum as u32;
                carry = sum >> 32;
            }
            words[i + other.words.len()] = carry as u32;
        }
        Self::normalized(self.negative ^ other.negative, words)
    }

    fn bit(&self, at: usize) -> bool {
        self.words
            .get(at / 32)
            .is_some_and(|value| (value & (1 << (at % 32))) != 0)
    }

    fn set_bit(&mut self, at: usize) {
        if self.words.len() <= at / 32 {
            self.words.resize(at / 32 + 1, 0);
        }
        self.words[at / 32] |= 1_u32 << (at % 32);
    }

    pub(crate) fn div_rem(&self, other: &Self) -> Result<(Self, Self), &'static str> {
        if other.is_zero() {
            return Err("division by zero");
        }
        let mut quotient = Self::zero();
        let mut remainder = Self::zero();
        let divisor = other.abs();
        for bit in (0..self.bits()).rev() {
            remainder = remainder.left(1);
            if self.bit(bit) {
                remainder.set_bit(0);
            }
            if remainder.cmp_magnitude(&divisor) != Ordering::Less {
                remainder = remainder.sub(&divisor);
                quotient.set_bit(bit);
            }
        }
        quotient.negative = (self.negative ^ other.negative) && !quotient.is_zero();
        remainder.negative = self.negative && !remainder.is_zero();
        Ok((quotient, remainder))
    }

    pub(crate) fn left(&self, amount: usize) -> Self {
        if self.is_zero() {
            return Self::zero();
        }
        let whole = amount / 32;
        let part = amount % 32;
        let mut result = vec![0; whole + self.words.len() + 1];
        for (index, &word) in self.words.iter().enumerate() {
            result[whole + index] |= word << part;
            if part != 0 {
                result[whole + index + 1] |= word >> (32 - part);
            }
        }
        Self::normalized(self.negative, result)
    }

    pub(crate) fn right(&self, amount: usize) -> Self {
        if self.is_zero() || amount == 0 {
            return self.clone();
        }
        let whole = amount / 32;
        let part = amount % 32;
        let dropped = (0..amount.min(self.bits())).any(|bit| self.bit(bit));
        let mut result = Vec::with_capacity(self.words.len().saturating_sub(whole));
        for index in whole..self.words.len() {
            let mut current = self.words[index] >> part;
            if part != 0 && index + 1 < self.words.len() {
                current |= self.words[index + 1] << (32 - part);
            }
            result.push(current);
        }
        let mut magnitude = Self::normalized(false, result);
        if self.negative && dropped {
            magnitude = magnitude.add(&Self::one());
        }
        if self.negative {
            magnitude = magnitude.negated();
        }
        magnitude
    }

    fn twos(&self, width: usize) -> Vec<u32> {
        let mut output = self.words.clone();
        output.resize(width, 0);
        if self.negative {
            for word in &mut output {
                *word = !*word;
            }
            let mut carry = true;
            for word in &mut output {
                if !carry {
                    break;
                }
                let (value, overflow) = word.overflowing_add(1);
                *word = value;
                carry = overflow;
            }
        }
        output
    }

    pub(crate) fn bit_op(&self, other: &Self, op: &str) -> Self {
        // One extra word preserves the infinite sign extension semantics.
        let width = self.words.len().max(other.words.len()) + 1;
        let left = self.twos(width);
        let right = other.twos(width);
        let mut words: Vec<u32> = left
            .iter()
            .zip(right.iter())
            .map(|(&a, &b)| match op {
                "&" => a & b,
                "|" => a | b,
                "^" => a ^ b,
                _ => unreachable!(),
            })
            .collect();
        Self::from_twos(&mut words)
    }

    pub(crate) fn not(&self) -> Self {
        self.negated().sub(&Self::one())
    }

    fn from_twos(words: &mut [u32]) -> Self {
        let negative = words.last().is_some_and(|word| word & 0x8000_0000 != 0);
        if negative {
            for word in words.iter_mut() {
                *word = !*word;
            }
            let mut carry = true;
            for word in words.iter_mut() {
                if !carry {
                    break;
                }
                let (new, overflow) = word.overflowing_add(1);
                *word = new;
                carry = overflow;
            }
        }
        Self::normalized(negative, words.to_vec())
    }

    pub(crate) fn to_i64(&self) -> Option<i64> {
        let magnitude = self.to_u64_magnitude()?;
        if self.negative {
            if magnitude == 1_u64 << 63 {
                Some(i64::MIN)
            } else if magnitude < 1_u64 << 63 {
                Some(-(magnitude as i64))
            } else {
                None
            }
        } else if magnitude <= i64::MAX as u64 {
            Some(magnitude as i64)
        } else {
            None
        }
    }

    fn to_u64_magnitude(&self) -> Option<u64> {
        if self.words.len() > 2 {
            return None;
        }
        Some(
            u64::from(*self.words.first().unwrap_or(&0))
                | (u64::from(*self.words.get(1).unwrap_or(&0)) << 32),
        )
    }

    pub(crate) fn to_u64(&self) -> Option<u64> {
        if self.negative {
            None
        } else {
            self.to_u64_magnitude()
        }
    }
}

fn trim(words: &mut Vec<u32>) {
    while words.last() == Some(&0) {
        words.pop();
    }
}

fn add_abs(left: &[u32], right: &[u32]) -> Vec<u32> {
    let mut result = Vec::with_capacity(left.len().max(right.len()) + 1);
    let mut carry = 0_u64;
    for index in 0..left.len().max(right.len()) {
        let value = u64::from(*left.get(index).unwrap_or(&0))
            + u64::from(*right.get(index).unwrap_or(&0))
            + carry;
        result.push(value as u32);
        carry = value >> 32;
    }
    if carry != 0 {
        result.push(carry as u32);
    }
    result
}

// Precondition: |left| >= |right|.
fn sub_abs(left: &[u32], right: &[u32]) -> Vec<u32> {
    let mut result = Vec::with_capacity(left.len());
    let mut borrow = 0_i64;
    for (index, &word) in left.iter().enumerate() {
        let diff = i64::from(word) - i64::from(*right.get(index).unwrap_or(&0)) - borrow;
        if diff < 0 {
            result.push((diff + (1_i64 << 32)) as u32);
            borrow = 1;
        } else {
            result.push(diff as u32);
            borrow = 0;
        }
    }
    trim(&mut result);
    result
}

#[cfg(test)]
mod tests {
    use super::Integer as I;
    #[test]
    fn signed_division_and_bitwise() {
        for a in [-123, -2, -1, 0, 1, 2, 123, i64::MIN, i64::MAX] {
            let n = I::from_i64(a);
            assert_eq!(n.to_i64(), Some(a));
            assert_eq!(n.not().to_i64(), Some(!a));
            assert_eq!(n.right(1).to_i64(), Some(a >> 1));
            for b in [-31, -1, 1, 31] {
                let m = I::from_i64(b);
                let (q, r) = n.div_rem(&m).unwrap();
                assert_eq!(q.multiply(&m).add(&r), n);
                assert_eq!(n.bit_op(&m, "&").to_i64(), Some(a & b));
                assert_eq!(n.bit_op(&m, "|").to_i64(), Some(a | b));
                assert_eq!(n.bit_op(&m, "^").to_i64(), Some(a ^ b));
            }
        }
    }

    #[test]
    fn big_integer_intermediates() {
        let limit = I::from_i64(i64::MAX);
        let square = limit.multiply(&limit);
        assert!(square.bits() > 64);
        assert_eq!(square.div_rem(&limit).unwrap().0, limit);
        assert_eq!(I::from_i64(-1).twos_complement_bits(), 0);
        assert_eq!(I::from_i64(-2).twos_complement_bits(), 1);
        assert_eq!(I::from_i64(-3).right(1).to_i64(), Some(-2));
        assert_eq!(I::from_i64(-3).twos_complement_bits(), 2);
        assert_eq!(I::from_i64(i64::MIN).twos_complement_bits(), 63);
        assert_eq!(I::one().left(1024).negated().twos_complement_bits(), 1024);
        assert_eq!(
            I::from_i64(i64::MIN)
                .div_rem(&I::from_i64(-1))
                .unwrap()
                .0
                .to_i64(),
            None
        );
        let (quotient, remainder) = I::from_i64(-5).div_rem(&I::from_i64(2)).unwrap();
        assert_eq!(
            (quotient.to_i64(), remainder.to_i64()),
            (Some(-2), Some(-1))
        );
        assert_eq!(
            I::decimal("999999999999999999999999999999999999999")
                .unwrap()
                .bits(),
            130
        );
    }
}
