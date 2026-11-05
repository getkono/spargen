/// FNV-1a 64 over `bytes`: the one stable hash behind the omit-profile fingerprint in every
/// provenance header and the pointer-seeded suffix of every disambiguated generated name.
///
/// Deliberately a function over explicit bytes rather than a [`std::hash::Hasher`], so derived
/// `Hash`, whose byte stream std documents as unstable across platforms and compiler releases,
/// cannot be fed into it.
pub(crate) fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::fnv1a64;

    #[test]
    fn fnv1a64_matches_the_reference_vectors() {
        // The empty input is the offset basis; `a` and `foobar` are the published FNV-1a 64
        // test vectors.
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a64(b"foobar"), 0x8594_4171_f739_67e8);
    }
}
