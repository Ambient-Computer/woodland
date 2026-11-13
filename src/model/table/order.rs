//! Bytes that sort the way the things they stand for do, and can be
//! followed by more bytes without that changing. It's what lets a range
//! of keys in storage be a range of rows, or of a column's values.

use super::value::Type;
use super::{ColumnId, Value};

/// Writes bytes so that they can be followed by more, and still sort
/// the way they did: a zero becomes a zero and `0xFF`, and two zeroes
/// end it. Nothing written this way is the start of anything else
/// written this way.
pub(crate) fn escape(bytes: &[u8]) -> Vec<u8> {
    let mut escaped = Vec::with_capacity(bytes.len() + 2);
    for &byte in bytes {
        escaped.push(byte);
        if byte == 0 {
            escaped.push(0xFF);
        }
    }

    escaped.extend([0, 0]);
    escaped
}

/// Reads back what [`escape`] wrote, and whatever follows it.
pub(crate) fn unescape(escaped: &[u8]) -> Option<(Vec<u8>, &[u8])> {
    let mut bytes = Vec::new();
    let mut rest = escaped;
    loop {
        match rest {
            [0, 0, after @ ..] => return Some((bytes, after)),
            [0, 0xFF, after @ ..] => {
                bytes.push(0);
                rest = after;
            }
            [0, ..] | [] => return None,
            [byte, after @ ..] => {
                bytes.push(*byte);
                rest = after;
            }
        }
    }
}

/// A value, as bytes that sort the way values of its type do. Whole
/// numbers sort by size, reals too, text and bytes by their bytes, and
/// false before true. Nothing has no bytes.
pub(crate) fn ordered(value: &Value) -> Vec<u8> {
    const SIGN: u64 = 1 << 63;
    match value {
        Value::Null => Vec::new(),
        // Flipping the sign bit puts negative numbers first.
        Value::Integer(number) => (number.cast_unsigned() ^ SIGN).to_be_bytes().to_vec(),
        // A real's bits sort by size once the sign bit is flipped, and
        // for negative numbers, the rest are too.
        Value::Real(number) => {
            let bits = number.to_bits();
            let bits = if bits & SIGN == 0 { bits | SIGN } else { !bits };

            bits.to_be_bytes().to_vec()
        }
        Value::Text(text) => escape(text.as_bytes()),
        Value::Boolean(flag) => vec![u8::from(*flag)],
        Value::Bytes(bytes) => escape(bytes),
    }
}

/// What follows an [`ordered`] value of a column's type.
pub(crate) fn after(column: ColumnId, bytes: &[u8]) -> Option<&[u8]> {
    match column.kind()? {
        Type::Integer | Type::Real => bytes.get(8..),
        Type::Boolean => bytes.get(1..),
        Type::Text | Type::Bytes => unescape(bytes).map(|(_, rest)| rest),
    }
}

/// The first key that comes after every key starting with `prefix`.
/// `None` if nothing does.
pub(crate) fn successor(prefix: &[u8]) -> Option<Vec<u8>> {
    let last = prefix.iter().rposition(|&byte| byte != 0xFF)?;
    let mut next = prefix[..=last].to_vec();
    next[last] += 1;

    Some(next)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escaped_bytes_come_back() {
        for bytes in [&b""[..], b"ada", b"\0", b"a\0\0b", b"\0\xFF", b"\xFF\0"] {
            let mut escaped = escape(bytes);
            escaped.extend(b"more");

            assert_eq!(unescape(&escaped), Some((bytes.to_vec(), &b"more"[..])));
        }
    }

    #[test]
    fn escaped_bytes_sort_like_the_bytes() {
        let keys = [
            &b""[..],
            b"\0",
            b"\0\0",
            b"\0a",
            b"a",
            b"a\0",
            b"a\0b",
            b"ab",
        ];
        let escaped: Vec<_> = keys.iter().map(|key| escape(key)).collect();

        assert!(escaped.is_sorted());
    }

    fn sorted(values: &[Value]) -> bool {
        let ordered: Vec<_> = values.iter().map(ordered).collect();

        ordered.is_sorted() && ordered.windows(2).all(|pair| pair[0] != pair[1])
    }

    #[test]
    fn whole_numbers_sort_by_size() {
        let numbers = [i64::MIN, -300, -1, 0, 1, 255, 256, i64::MAX];

        assert!(sorted(&numbers.map(Value::from)));
    }

    #[test]
    fn reals_sort_by_size() {
        let numbers = [
            f64::NEG_INFINITY,
            -2.5,
            -1e-9,
            0.0,
            1e-9,
            2.5,
            1e300,
            f64::INFINITY,
        ];

        assert!(sorted(&numbers.map(Value::from)));
    }

    #[test]
    fn text_sorts_by_its_bytes() {
        let text = ["", "a", "a\0", "aa", "ab", "b"];

        assert!(sorted(&text.map(Value::from)));
        assert!(sorted(&[false, true].map(Value::from)));
    }

    #[test]
    fn a_successor_is_past_everything_with_the_prefix() {
        assert_eq!(successor(b"ab"), Some(b"ac".to_vec()));
        assert_eq!(successor(b"a\xFF\xFF"), Some(b"b".to_vec()));
        assert_eq!(successor(b"\xFF"), None);
        assert_eq!(successor(b""), None);
    }
}
