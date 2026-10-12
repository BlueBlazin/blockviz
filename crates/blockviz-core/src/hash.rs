//! Maps and sets keyed by a fast hash. The standard library hashes with
//! SipHash, which resists keys chosen to collide at several times the cost
//! of a simple multiply-and-rotate; the analyzer's keys are names and ids
//! from code it reads for its own reader, so it uses FxHash, the hash rustc
//! uses for its own tables. It is also deterministic, run to run.

use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasherDefault, Hasher};

pub type Map<K, V> = HashMap<K, V, BuildHasherDefault<FxHasher>>;
pub type Set<K> = HashSet<K, BuildHasherDefault<FxHasher>>;

#[derive(Default, Clone, Copy)]
pub struct FxHasher {
    hash: u64,
}

const SEED: u64 = 0x51_7c_c1_b7_27_22_0a_95;

impl FxHasher {
    #[inline]
    fn add(&mut self, word: u64) {
        self.hash = (self.hash.rotate_left(5) ^ word).wrapping_mul(SEED);
    }
}

impl Hasher for FxHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let mut words = bytes.chunks_exact(8);
        for w in &mut words {
            self.add(u64::from_le_bytes(w.try_into().unwrap()));
        }
        // The last bytes, little-endian as if padded with zeros, read in
        // place: copying them out cost a call to memmove for every name.
        let tail = words.remainder();
        let mut rest = 0u64;
        for (i, &b) in tail.iter().enumerate() {
            rest |= (b as u64) << (8 * i);
        }
        self.add(rest ^ tail.len() as u64);
    }

    #[inline]
    fn write_u8(&mut self, n: u8) {
        self.add(n as u64);
    }

    #[inline]
    fn write_u32(&mut self, n: u32) {
        self.add(n as u64);
    }

    #[inline]
    fn write_u64(&mut self, n: u64) {
        self.add(n);
    }

    #[inline]
    fn write_usize(&mut self, n: usize) {
        self.add(n as u64);
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.hash
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_find_what_they_hold() {
        let mut m: Map<String, usize> = Map::default();
        for i in 0..10_000 {
            m.insert(format!("name_{i}"), i);
        }
        assert!((0..10_000).all(|i| m[&format!("name_{i}")] == i));
        // Strings that differ only in their last bytes, or their length, hash apart.
        let s: Set<&str> = ["a", "a\0", "ab", "abcdefgh", "abcdefghi"].into_iter().collect();
        assert_eq!(s.len(), 5);
    }
}
