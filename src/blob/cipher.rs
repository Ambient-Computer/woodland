//! How a sealed group's blobs are encrypted.
//!
//! Each blob has a key of its own, which is in the references to it
//! and nowhere else, so replacing a group's secret re-encrypts
//! nothing. The cipher is a stream that can start anywhere, so any
//! range is read without the rest.
//!
//! It carries no check of its own. A blob's name is the hash of its
//! encrypted bytes, and that's what says they're the ones the author
//! wrote.

use chacha20::ChaCha20Legacy;
use chacha20::cipher::{KeyIvInit, StreamCipher, StreamCipherSeek};

use crate::encoding;

/// The key of a blob, made from randomness the application supplied.
pub(crate) fn key(randomness: [u8; 32]) -> [u8; 32] {
    encoding::hash("woodland/blob/key", &randomness)
}

/// Encrypts or decrypts `bytes`, which start `offset` bytes into a
/// blob. A key encrypts one blob, so there's nothing else to tell
/// two streams apart by. The counter is 64 bits, which is more than
/// any blob is long.
pub(crate) fn apply(key: &[u8; 32], offset: u64, bytes: &mut [u8]) {
    let mut cipher = ChaCha20Legacy::new(key.into(), &[0; 8].into());
    cipher.seek(offset);
    cipher.apply_keystream(bytes);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_range_is_decrypted_without_the_rest() {
        let key = key([7; 32]);
        let plain: Vec<u8> = (0..=250).cycle().take(200_000).collect();
        let mut sealed = plain.clone();
        apply(&key, 0, &mut sealed);
        assert_ne!(sealed, plain);

        for (start, end) in [(0, 10), (63, 65), (70_001, 133_333), (199_990, 200_000)] {
            let mut range = sealed[start..end].to_vec();
            apply(&key, start as u64, &mut range);

            assert_eq!(range, plain[start..end], "{start}..{end}");
        }
    }
}
