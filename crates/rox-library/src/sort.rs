//! The file-manager sort order. Lives here rather than up with the other
//! formatters because the folder trie sorts its children with it, and the
//! library can't reach up into the app's crates.

use std::cmp::Ordering;

/// Compare two names the way a file manager lists them: runs of digits
/// compare as numbers, so "2" sorts before "10" and padded "02" reads the
/// same as "2", and the rest byte by byte. Inputs come lowercased.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    /// Drop leading zeros but keep one digit, so "007" compares as "7".
    fn magnitude(digits: &[u8]) -> &[u8] {
        let mut k = 0;
        while k + 1 < digits.len() && digits[k] == b'0' {
            k += 1;
        }
        &digits[k..]
    }

    let (a, b) = (a.as_bytes(), b.as_bytes());
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if a[i].is_ascii_digit() && b[j].is_ascii_digit() {
            let (si, sj) = (i, j);
            while i < a.len() && a[i].is_ascii_digit() {
                i += 1;
            }
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            let (da, db) = (magnitude(&a[si..i]), magnitude(&b[sj..j]));
            let ord = da.len().cmp(&db.len()).then_with(|| da.cmp(db));
            if ord != Ordering::Equal {
                return ord;
            }
            // Same value; the shorter run (fewer leading zeros) reads first.
            let ord = (i - si).cmp(&(j - sj));
            if ord != Ordering::Equal {
                return ord;
            }
        } else {
            let ord = a[i].cmp(&b[j]);
            if ord != Ordering::Equal {
                return ord;
            }
            i += 1;
            j += 1;
        }
    }
    (a.len() - i).cmp(&(b.len() - j))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Filenames sort the file-manager way: digit runs compare as numbers,
    /// so padded and unpadded track numbers both land 1, 2, ... 10, 11 and
    /// never 1, 10, 11, 2.
    #[test]
    fn natural_sort_orders_track_numbers() {
        let mut names = vec![
            "10 moonbeam.mp3",
            "2 never ever.mp3",
            "1 lost.mp3",
            "12 emerald.mp3",
        ];
        names.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(
            names,
            [
                "1 lost.mp3",
                "2 never ever.mp3",
                "10 moonbeam.mp3",
                "12 emerald.mp3"
            ]
        );
        // Zero-padding reads as the same value, so "02" and "2" tie on
        // magnitude and only the padding breaks it.
        assert_eq!(natural_cmp("02 x.mp3", "2 x.mp3"), Ordering::Greater);
        assert_eq!(natural_cmp("03 a.mp3", "10 a.mp3"), Ordering::Less);
    }
}
