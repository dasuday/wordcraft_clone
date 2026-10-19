//! Thai text structure for Thai Distributed alignment.
//!
//! Like Khmer, Thai has no spaces between words. Extra space may go between character clusters,
//! but not before a combining vowel or tone mark, nor after a leading vowel (เ แ โ ใ ไ, written
//! before the consonant they follow in memory).

use crate::khmer::Gap;

/// Is `c` in the Thai block (U+0E00–0E7F)?
pub fn is_thai(c: char) -> bool {
    matches!(u32::from(c), 0x0E00..=0x0E7F)
}

/// A Thai character that combines with the one before it: mai han-akat, above and below vowels,
/// phinthu, thanthakhat, tone marks, nikhahit, yamakkan.
fn is_combining(c: char) -> bool {
    matches!(u32::from(c), 0x0E31 | 0x0E34..=0x0E3A | 0x0E47..=0x0E4E)
}

/// A leading vowel: it is drawn before the consonant it follows in the text.
fn is_leading_vowel(c: char) -> bool {
    matches!(u32::from(c), 0x0E40..=0x0E44)
}

/// The gap between `before` and `after`: a space, or a cluster boundary inside Thai text.
pub fn gap_between(before: char, after: char) -> Gap {
    if before == ' ' || before == '\u{3000}' {
        return Gap::Space;
    }
    if !is_thai(before) && !is_thai(after) {
        return Gap::None;
    }
    if is_combining(after) || is_leading_vowel(before) || before == '\u{200D}' || after == '\u{200D}' || after == '\u{200C}' {
        return Gap::None;
    }
    // Thai next to Latin or digits is a word boundary; between Thai letters, a cluster boundary.
    if is_thai(before) && is_thai(after) {
        // The repetition mark ๆ and the paiyannoi/maiyamok signs stay with their word.
        if matches!(u32::from(after), 0x0E2F | 0x0E46) {
            return Gap::None;
        }
        Gap::Syllable
    } else if before.is_alphanumeric() && after.is_alphanumeric() {
        Gap::Word
    } else {
        Gap::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_and_leading_vowels_stay_attached() {
        assert_eq!(gap_between('ก', 'ข'), Gap::Syllable);
        assert_eq!(gap_between('ก', 'ิ'), Gap::None, "vowel above");
        assert_eq!(gap_between('ก', '้'), Gap::None, "tone mark");
        assert_eq!(gap_between('เ', 'ก'), Gap::None, "leading vowel");
        assert_eq!(gap_between('ก', 'ๆ'), Gap::None, "repetition mark");
        assert_eq!(gap_between(' ', 'ก'), Gap::Space);
        assert_eq!(gap_between('ก', 'a'), Gap::Word);
        assert_eq!(gap_between('a', 'b'), Gap::None);
    }
}
