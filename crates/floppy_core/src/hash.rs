//! FNV-1a 64-bit hashing (SPEC §5): used to fingerprint framebuffers and sim
//! state for determinism verification. Plain, allocation-free byte folding —
//! no `HashMap`/iteration-order dependence anywhere near this.

const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const PRIME: u64 = 0x0000_0100_0000_01b3;

/// Allocation-free streaming FNV-1a writer for deterministic state digests.
#[derive(Clone, Copy, Debug)]
pub struct Hasher64(u64);

impl Default for Hasher64 {
    fn default() -> Self {
        Self(OFFSET_BASIS)
    }
}

impl Hasher64 {
    pub fn write_u8(&mut self, value: u8) {
        self.0 = fold_byte(self.0, value);
    }

    pub fn write_bool(&mut self, value: bool) {
        self.write_u8(value as u8);
    }

    pub fn write_u16(&mut self, value: u16) {
        self.write_bytes(&value.to_le_bytes());
    }

    pub fn write_u32(&mut self, value: u32) {
        self.write_bytes(&value.to_le_bytes());
    }

    pub fn write_u64(&mut self, value: u64) {
        self.write_bytes(&value.to_le_bytes());
    }

    pub fn write_f32(&mut self, value: f32) {
        self.write_u32(value.to_bits());
    }

    pub fn write_bytes(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 = fold_byte(self.0, byte);
        }
    }

    pub fn finish(self) -> u64 {
        self.0
    }
}

#[inline]
fn fold_byte(hash: u64, byte: u8) -> u64 {
    (hash ^ byte as u64).wrapping_mul(PRIME)
}

/// FNV-1a over raw bytes.
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = OFFSET_BASIS;
    for &b in bytes {
        hash = fold_byte(hash, b);
    }
    hash
}

/// FNV-1a over a slice of `u32`s, each folded in as 4 little-endian bytes,
/// without allocating an intermediate byte buffer. Equivalent to calling
/// [`fnv1a64`] on the concatenated little-endian byte representation.
pub fn hash_u32s(words: &[u32]) -> u64 {
    let mut hash = OFFSET_BASIS;
    for &w in words {
        for b in w.to_le_bytes() {
            hash = fold_byte(hash, b);
        }
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv1a64_known_vectors() {
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
    }

    #[test]
    fn hash_u32s_matches_fnv1a64_of_le_bytes() {
        let words = [1u32, 2u32, 3u32];
        let mut bytes = Vec::new();
        for w in words {
            bytes.extend_from_slice(&w.to_le_bytes());
        }
        assert_eq!(hash_u32s(&words), fnv1a64(&bytes));
    }
}
