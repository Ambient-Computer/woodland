//! A blob's name: the BLAKE3 hash of its bytes, kept as a tree.
//!
//! BLAKE3 hashes its input as a binary tree. The tree is kept at two
//! heights, so that any 16 KiB can be checked against the name
//! without the rest:
//!
//! - the hash of each **group** of 16 KiB, a page of them at a time;
//! - the hash of each full **page** of groups.
//!
//! A page is a power of two groups, so a full one is a whole subtree,
//! and the tree over pages has the shape the tree over groups has.
//! A device that holds the page hashes checks a page of group hashes
//! against one of them, and then a group of bytes against one of
//! those.

use blake3::hazmat::{
    ChainingValue, HasherExt, Mode, merge_subtrees_non_root, merge_subtrees_root,
};

/// How many bytes are checked at a time.
pub(crate) const GROUP: u64 = 16 * 1024;

/// How many groups' hashes are kept together.
pub(crate) const PAGE: u64 = 2048;

/// How many groups a blob of `length` bytes is.
pub(crate) fn groups(length: u64) -> u64 {
    length.div_ceil(GROUP)
}

/// The hash of the group that starts at `offset`, which isn't all of
/// its blob.
pub(crate) fn group(offset: u64, bytes: &[u8]) -> ChainingValue {
    let mut hasher = blake3::Hasher::new();
    hasher.set_input_offset(offset);
    hasher.update(bytes);

    hasher.finalize_non_root()
}

/// How many of `count` hashes are under the left half of their tree:
/// the largest power of two that leaves some for the right.
fn left(count: usize) -> usize {
    1 << (count - 1).ilog2()
}

/// The hash of the subtree over `hashes`, which isn't the whole tree.
///
/// # Panics
///
/// If there are none.
pub(crate) fn subtree(hashes: &[ChainingValue]) -> ChainingValue {
    if let [only] = hashes {
        return *only;
    }

    let (first, rest) = hashes.split_at(left(hashes.len()));

    merge_subtrees_non_root(&subtree(first), &subtree(rest), Mode::Hash)
}

/// The name of the blob whose tree is over `hashes`: two or more of
/// its groups', or of its pages'.
///
/// # Panics
///
/// If there are fewer than two.
pub(crate) fn root(hashes: &[ChainingValue]) -> [u8; 32] {
    let (first, rest) = hashes.split_at(left(hashes.len()));

    *merge_subtrees_root(&subtree(first), &subtree(rest), Mode::Hash).as_bytes()
}

/// The name of a blob of one group or none.
pub(crate) fn small(bytes: &[u8]) -> [u8; 32] {
    *blake3::hash(bytes).as_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(length: usize) -> Vec<u8> {
        (0..=250).cycle().take(length).collect()
    }

    /// The name, by way of the hashes that are kept.
    fn name(bytes: &[u8], page: usize) -> [u8; 32] {
        let size = usize::try_from(GROUP).expect("fits");
        let mut offset = 0;
        let mut hashes = Vec::new();
        for piece in bytes.chunks(size) {
            hashes.push(group(offset, piece));
            offset += GROUP;
        }
        if hashes.len() < 2 {
            return small(bytes);
        }
        if hashes.len() <= page {
            return root(&hashes);
        }

        let pages: Vec<_> = hashes.chunks(page).map(subtree).collect();

        root(&pages)
    }

    #[test]
    fn a_name_is_the_hash_of_the_bytes() {
        let size = usize::try_from(GROUP).expect("fits");
        let lengths = [
            0,
            1,
            size - 1,
            size,
            size + 1,
            2 * size,
            3 * size - 7,
            8 * size,
            8 * size + 1,
            21 * size + 5,
        ];
        for length in lengths {
            let bytes = bytes(length);
            for page in [4, 8, 2048] {
                let expected = *blake3::hash(&bytes).as_bytes();
                assert_eq!(
                    name(&bytes, page),
                    expected,
                    "{length} bytes, pages of {page}"
                );
            }
        }
    }
}
