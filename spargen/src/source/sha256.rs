//! A dependency-free SHA-256, used to hash-pin vendored remote `$ref` documents (and to derive
//! their vendor paths) and, in `cache.rs`, to fingerprint build inputs and generated output.
//!
//! Digests must be reproducible and add no supply-chain surface to the published crate, so they
//! are computed in-tree rather than via a hashing dependency. The implementation follows
//! FIPS 180-4, buffers at most one partial 64-byte block of its input, and is covered by the
//! standard NIST test vectors.

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

/// The lowercase hex SHA-256 digest of `bytes`: a [`Sha256`] given `bytes` in one update.
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher.finalize()
}

/// An incremental SHA-256. [`Sha256::update`] takes the message in pieces of any length, and
/// [`Sha256::finalize`] gives the digest of their concatenation, the one [`sha256_hex`] gives for
/// the whole message at once. Only a partial block is buffered, so hashing borrowed bytes copies
/// at most 63 of them.
pub(crate) struct Sha256 {
    state: [u32; 8],
    /// The current block, of which the first `buffered` bytes are filled.
    block: [u8; 64],
    buffered: usize,
    /// The message length so far, in bytes.
    length: u64,
}

impl Sha256 {
    pub(crate) fn new() -> Self {
        Self {
            state: [
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
                0x5be0cd19,
            ],
            block: [0; 64],
            buffered: 0,
            length: 0,
        }
    }

    /// Append `bytes` to the message.
    pub(crate) fn update(&mut self, mut bytes: &[u8]) {
        self.length = self.length.wrapping_add(bytes.len() as u64);
        // Mutation testing: `>` to `>=` is an equivalent mutant, declared in
        // `.cargo/mutants.toml`: from an empty block the branch buffers what the tail copy below
        // would, or fills and compresses the block the loop below would compress first. Only how
        // many bytes are copied differs.
        if self.buffered > 0 {
            let take = bytes.len().min(64 - self.buffered);
            self.block[self.buffered..self.buffered + take].copy_from_slice(&bytes[..take]);
            self.buffered += take;
            bytes = &bytes[take..];
            if self.buffered < 64 {
                return;
            }
            compress(&mut self.state, &self.block);
            self.buffered = 0;
        }
        let (blocks, rest) = bytes.as_chunks::<64>();
        for block in blocks {
            compress(&mut self.state, block);
        }
        self.block[..rest.len()].copy_from_slice(rest);
        self.buffered = rest.len();
    }

    /// The lowercase hex digest of everything [`Self::update`] was given.
    pub(crate) fn finalize(mut self) -> String {
        // Padding: 0x80, zeros to 56 mod 64, then the message's bit length as 64 big-endian bits.
        let bit_len = self.length.wrapping_mul(8);
        self.block[self.buffered] = 0x80;
        self.block[self.buffered + 1..].fill(0);
        if self.buffered >= 56 {
            compress(&mut self.state, &self.block);
            self.block = [0; 64];
        }
        self.block[56..].copy_from_slice(&bit_len.to_be_bytes());
        compress(&mut self.state, &self.block);

        let mut out = String::with_capacity(64);
        for word in self.state {
            out.push_str(&format!("{word:08x}"));
        }
        out
    }
}

/// One application of the FIPS 180-4 compression function to `state`.
fn compress(state: &mut [u32; 8], chunk: &[u8; 64]) {
    let mut w = [0u32; 64];
    for (i, word) in w.iter_mut().take(16).enumerate() {
        let j = i * 4;
        *word = u32::from_be_bytes([chunk[j], chunk[j + 1], chunk[j + 2], chunk[j + 3]]);
    }
    for i in 16..64 {
        let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
        let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
        w[i] = w[i - 16]
            .wrapping_add(s0)
            .wrapping_add(w[i - 7])
            .wrapping_add(s1);
    }

    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = *state;
    for i in 0..64 {
        let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        // Mutation testing: `^` to `|` here is an equivalent mutant, declared in
        // `.cargo/mutants.toml`: `e & f` and `!e & g` share no set bit, so their xor is their or.
        let ch = (e & f) ^ ((!e) & g);
        let t1 = hh
            .wrapping_add(s1)
            .wrapping_add(ch)
            .wrapping_add(K[i])
            .wrapping_add(w[i]);
        let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        // Mutation testing: either `^` to `|` is an equivalent mutant, declared in
        // `.cargo/mutants.toml`: each spelling it leaves is another form of the bitwise majority.
        // `(a & b) | (a & c) ^ (b & c)` parses as `(a & b) | (c & (a ^ b))`; and
        // `(a & b) ^ (a & c) | (b & c)` differs from this only where `b & c` is set, where
        // `a & b` equals `a & c` and their xor is clear.
        let maj = (a & b) ^ (a & c) ^ (b & c);
        let t2 = s0.wrapping_add(maj);
        hh = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }
    for (word, value) in state.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
        *word = word.wrapping_add(value);
    }
}

#[cfg(test)]
mod tests {
    use super::{sha256_hex, Sha256};
    use crate::test_support::{read_repo_file, repo_root};

    /// The digest of `message` fed to one hasher in pieces of `piece` bytes.
    fn in_pieces(message: &[u8], piece: usize) -> String {
        let mut hasher = Sha256::new();
        for chunk in message.chunks(piece) {
            hasher.update(chunk);
        }
        hasher.finalize()
    }

    #[test]
    fn nist_vectors() {
        let million_a = vec![b'a'; 1_000_000];
        let vectors: [(&[u8], &str); 5] = [
            (
                b"",
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            ),
            (
                b"abc",
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            ),
            (
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq",
                "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1",
            ),
            (
                b"abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmnoijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu",
                "cf5b16a778af8380036ce59e7b0492370b249b11e8f07a51afac45037afee9d1",
            ),
            (
                &million_a,
                "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0",
            ),
        ];
        for (message, digest) in vectors {
            assert_eq!(sha256_hex(message), digest, "{} bytes", message.len());
            // The same vectors fed incrementally, one byte and an odd-sized piece at a time.
            for piece in [1, 7] {
                assert_eq!(in_pieces(message, piece), digest, "{piece}-byte pieces");
            }
        }
    }

    /// Splitting a message anywhere leaves its digest alone: every length from empty through
    /// three blocks, around each padding edge (55, 56 and 64 bytes buffered), split at every
    /// point and fed in every piece size up to just past a block, agrees with the one-shot digest
    /// and with the `sha2` crate's.
    #[test]
    fn incremental_hashing_agrees_with_one_shot_across_every_block_boundary() {
        use sha2::{Digest, Sha256 as Reference};

        let message = (0..=192u8)
            .map(|byte| byte.wrapping_mul(31))
            .collect::<Vec<_>>();
        for len in 0..=message.len() {
            let message = &message[..len];
            let expected = format!("{:x}", Reference::digest(message));
            assert_eq!(sha256_hex(message), expected, "{len} bytes");
            for split in 0..=len {
                let mut hasher = Sha256::new();
                hasher.update(&message[..split]);
                hasher.update(&message[split..]);
                assert_eq!(hasher.finalize(), expected, "{len} bytes split at {split}");
            }
            for piece in 1..=65 {
                assert_eq!(
                    in_pieces(message, piece),
                    expected,
                    "{len} bytes in {piece}-byte pieces"
                );
            }
        }
    }

    #[test]
    fn multi_block_input() {
        // 1,000 'a' bytes spans several 64-byte blocks and exercises the length padding.
        let input = vec![b'a'; 1000];
        assert_eq!(
            sha256_hex(&input),
            "41edece42d63e8d9bf515a9ba6932e1c20cbc9f5a5d134645adb5db1b9737ea3"
        );
    }

    /// The hand-rolled hasher agrees with the `sha2` crate on every pinned corpus file: real-world
    /// inputs, the largest over ten megabytes, far longer than the vectors above. That each file
    /// still hashes to the `sha256` its manifest case pins is
    /// `every_pinned_spec_matches_the_hash_the_manifest_declares` in
    /// `spargen/tests/corpus_manifest.rs`; this test lives here only because `sha256_hex` is
    /// crate-private.
    #[test]
    fn the_hasher_agrees_with_sha2_on_every_pinned_corpus_file() {
        use sha2::{Digest, Sha256};

        // A packaged `.crate` ships no corpus, so the check is skipped there — gated on the
        // workspace marker, not on the file, so that inside the repository a missing or renamed
        // manifest fails instead of passing with a line on stderr nobody reads.
        let Some(root) = repo_root() else {
            eprintln!("skipping: not tested from the workspace");
            return;
        };
        let root = root.as_path();
        let manifest = read_repo_file(root, "corpus/manifest.toml");

        // The manifest is a list of `[[case]]` tables; `path` is the one key read here.
        let mut checked = 0usize;
        for line in manifest.lines() {
            let Some(relative) = line.strip_prefix("path = ") else {
                continue;
            };
            let relative = relative.trim().trim_matches('"');
            let file = root.join("corpus").join(relative);
            let bytes = std::fs::read(&file)
                .unwrap_or_else(|error| panic!("{} is missing: {error}", file.display()));
            assert_eq!(
                sha256_hex(&bytes),
                format!("{:x}", Sha256::digest(&bytes)),
                "the hasher disagrees with `sha2` on `{relative}` ({} bytes)",
                bytes.len()
            );
            checked += 1;
        }
        assert!(
            checked >= 9,
            "expected every manifest case to name its pinned file; checked {checked}"
        );
    }
}
