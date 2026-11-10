//! Bit-exact port of .NET's seeded `System.Random` (Knuth subtractive generator).
//!
//! Needed only to read key blobs produced by CodeWalker, which obfuscates them
//! with byte streams from `new Random(seed).NextBytes(...)`.

const MBIG: i32 = i32::MAX;
const MSEED: i32 = 161_803_398;

pub struct DotNetRandom {
    seed_array: [i32; 56],
    inext: usize,
    inextp: usize,
}

impl DotNetRandom {
    pub fn new(seed: i32) -> Self {
        let mut seed_array = [0i32; 56];
        let subtraction = if seed == i32::MIN { i32::MAX } else { seed.abs() };
        let mut mj = MSEED - subtraction;
        seed_array[55] = mj;
        let mut mk = 1i32;
        for i in 1..55 {
            let ii = (21 * i) % 55;
            seed_array[ii] = mk;
            mk = mj.wrapping_sub(mk);
            if mk < 0 {
                mk = mk.wrapping_add(MBIG);
            }
            mj = seed_array[ii];
        }
        for _ in 1..5 {
            for i in 1..56 {
                seed_array[i] = seed_array[i].wrapping_sub(seed_array[1 + (i + 30) % 55]);
                if seed_array[i] < 0 {
                    seed_array[i] = seed_array[i].wrapping_add(MBIG);
                }
            }
        }
        Self { seed_array, inext: 0, inextp: 21 }
    }

    fn internal_sample(&mut self) -> i32 {
        self.inext = if self.inext + 1 >= 56 { 1 } else { self.inext + 1 };
        self.inextp = if self.inextp + 1 >= 56 { 1 } else { self.inextp + 1 };
        let mut ret = self.seed_array[self.inext].wrapping_sub(self.seed_array[self.inextp]);
        if ret == MBIG {
            ret -= 1;
        }
        if ret < 0 {
            ret = ret.wrapping_add(MBIG);
        }
        self.seed_array[self.inext] = ret;
        ret
    }

    pub fn next_bytes(&mut self, buf: &mut [u8]) {
        for b in buf {
            *b = (self.internal_sample() % 256) as u8;
        }
    }

    #[cfg(test)]
    pub fn next(&mut self) -> i32 {
        self.internal_sample()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_dotnet_reference() {
        // new Random(0).Next() == 1559595546 and new Random(42).Next() == 1434747710
        // in every .NET runtime (legacy seeded algorithm).
        assert_eq!(DotNetRandom::new(0).next(), 1_559_595_546);
        assert_eq!(DotNetRandom::new(42).next(), 1_434_747_710);
    }
}
