//! Seeded System.Random's compatibility subtractive generator (.NET 10).
//! System.Random(int) uses this stream, unlike Random.Shared and unseeded Random.
const MBIG: i64 = i32::MAX as i64;

#[derive(Clone, Debug)]
pub(crate) struct CompatRandom {
    array: [i64; 56],
    inext: usize,
    inextp: usize,
}

impl CompatRandom {
    pub(crate) fn new(seed: i32) -> Self {
        let subtraction = if seed == i32::MIN {
            i32::MAX as i64
        } else {
            i64::from(seed.abs())
        };
        let mut array = [0_i64; 56];
        let mut mj = 161_803_398_i64 - subtraction;
        array[55] = mj;
        let mut mk = 1_i64;
        for i in 1..55 {
            let ii = (21 * i) % 55;
            array[ii] = mk;
            mk = mj - mk;
            if mk < 0 {
                mk += MBIG;
            }
            mj = array[ii];
        }
        for _ in 1..5 {
            for i in 1..56 {
                array[i] -= array[1 + (i + 30) % 55];
                if array[i] < 0 {
                    array[i] += MBIG;
                }
            }
        }
        Self {
            array,
            inext: 0,
            inextp: 21,
        }
    }

    fn sample(&mut self) -> f64 {
        self.inext = if self.inext == 55 { 1 } else { self.inext + 1 };
        self.inextp = if self.inextp == 55 {
            1
        } else {
            self.inextp + 1
        };
        let mut value = self.array[self.inext] - self.array[self.inextp];
        if value == MBIG {
            value -= 1;
        }
        if value < 0 {
            value += MBIG;
        }
        self.array[self.inext] = value;
        value as f64 * (1.0 / MBIG as f64)
    }

    pub(crate) fn next(&mut self, ceiling: usize) -> usize {
        debug_assert!(ceiling > 0 && ceiling <= i32::MAX as usize);
        (self.sample() * ceiling as f64) as usize
    }

    pub(crate) fn range(&mut self, lower: i32, upper: i32) -> i32 {
        debug_assert!(lower < upper && i64::from(upper) - i64::from(lower) <= MBIG);
        ((self.sample() * f64::from(upper - lower)) as i32) + lower
    }
}

#[cfg(test)]
mod tests {
    use super::CompatRandom;
    #[test]
    fn dotnet_10_seeded_vectors() {
        for (seed, values) in [
            (0, [72624, 81732, 76802, 55816, 20603, 55888]),
            (i32::MIN, [72624, 81732, 76802, 55816, 20603, 55888]),
            (1, [24866, 11074, 46701, 77160, 65751, 43278]),
            (-1, [24866, 11074, 46701, 77160, 65751, 43278]),
            (11, [47292, 4493, 45689, 90603, 17237, 17176]),
        ] {
            let mut random = CompatRandom::new(seed);
            for value in values {
                assert_eq!(random.next(100_000), value);
            }
        }
    }
}
