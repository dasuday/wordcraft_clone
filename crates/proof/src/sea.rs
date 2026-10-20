//! Southeast Asian continuous scripts (Thai, Lao, Khmer, Myanmar):
//! word boundaries, character classifications, and justification rules.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use icu_segmenter::{WordSegmenter, WordSegmenterBorrowed};

/// A letter, vowel sign or mark of Southeast Asian scripts: Thai, Lao, Myanmar, Khmer.
pub fn is_sea(c: char) -> bool {
    matches!(u32::from(c),
        0x0E01..=0x0E3A | 0x0E40..=0x0E4E // Thai
        | 0x0E81..=0x0ECE | 0x0EDC..=0x0EDF // Lao
        | 0x1000..=0x103F | 0x1050..=0x108F | 0x109A..=0x109F | 0xA9E0..=0xA9FF | 0xAA60..=0xAA7F // Myanmar
        | 0x1780..=0x17D3 | 0x17D7 | 0x17DC..=0x17DD | 0x19E0..=0x19FF) // Khmer
}

/// Does `text` contain any Southeast Asian characters?
pub fn has_sea(text: &str) -> bool {
    text.chars().any(is_sea)
}

/// A combining mark of Southeast Asian scripts, set above, below, or around its base letter.
pub fn is_sea_mark(c: char) -> bool {
    match u32::from(c) {
        0x0E31 | 0x0E34..=0x0E3A | 0x0E47..=0x0E4E // Thai marks
        | 0x0EB1 | 0x0EB4..=0x0EBC | 0x0EC8..=0x0ECE => true, // Lao marks
        // Myanmar and Khmer nonspacing marks and signs
        0x1000..=0x109F | 0xA9E0..=0xA9FF | 0xAA60..=0xAA7F | 0x1780..=0x17FF | 0x19E0..=0x19FF => {
            unicode_bidi::bidi_class(c) == unicode_bidi::BidiClass::NSM
                || matches!(u32::from(c), 0x17C6 | 0x17C7 | 0x17C8 | 0x17CB | 0x17CC | 0x17CD | 0x17CE | 0x17CF | 0x17D0 | 0x17D2 | 0x17DD)
        }
        _ => false,
    }
}

/// No line starts with `c`: a vowel sign written after its consonant, repetition or abbreviation sign.
pub fn stays_with_previous(c: char) -> bool {
    matches!(c, 'ะ' | 'า' | 'ำ' | 'ๅ' | 'ๆ' | 'ฯ' | 'ະ' | 'າ' | 'ຳ' | 'ໆ' | 'ຯ' | 'ៗ')
}

/// No line ends with `c`: a vowel written before its consonant (Thai เ แ โ ใ ไ, Lao ເ ແ ໂ ໃ ໄ).
pub fn stays_with_next(c: char) -> bool {
    matches!(u32::from(c), 0x0E40..=0x0E44 | 0x0EC0..=0x0EC4)
}

static DICTIONARY: LazyLock<WordSegmenterBorrowed<'static>> = LazyLock::new(|| WordSegmenter::new_dictionary(Default::default()));
static MODEL: LazyLock<WordSegmenterBorrowed<'static>> = LazyLock::new(|| WordSegmenter::new_lstm(Default::default()));

const CACHE_BYTES: usize = 4 << 20;

/// Word boundaries of `span` as sorted byte offsets: the ones ICU's dictionary and its LSTM model
/// agree on.
pub fn word_boundaries(span: &str) -> Arc<[usize]> {
    static CACHE: LazyLock<Mutex<(HashMap<Box<str>, Arc<[usize]>>, usize)>> = LazyLock::new(Default::default);
    let guard_res = CACHE.lock();
    let mut guard = match guard_res {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    if let Some(known) = guard.0.get(span) {
        return known.clone();
    }
    let model: Vec<usize> = MODEL.segment_str(span).collect();
    let out: Arc<[usize]> = DICTIONARY.segment_str(span).filter(|b| model.binary_search(b).is_ok()).collect();
    if guard.1 > CACHE_BYTES {
        *guard = Default::default();
    }
    guard.1 = guard.1.saturating_add(span.len());
    guard.0.insert(span.into(), out.clone());
    out
}

/// Word-break opportunities for Thai and Lao text.
pub fn thai_lao_breaks(span: &str) -> Vec<usize> {
    if !span.chars().any(|c| matches!(u32::from(c), 0x0E00..=0x0EFF)) {
        return Vec::new();
    }
    let words = word_boundaries(span);
    let mut out = Vec::new();
    for &b in words.iter() {
        if b == 0 || b >= span.len() {
            continue;
        }
        let Some(prev) = span.get(..b).and_then(|t| t.chars().next_back()) else { continue };
        let Some(next) = span.get(b..).and_then(|t| t.chars().next()) else { continue };
        if is_sea_mark(next) || stays_with_previous(next) || stays_with_next(prev) {
            continue;
        }
        out.push(b);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_thai_word_breaks() {
        let text = "ผู้ซื้อสามารถตรวจสอบประวัติเครื่องได้ทุกขั้นตอนโดยร้านจะบันทึกผลการตรวจสอบแบตเตอรี่หน้าจอคีย์บอร์ดและสถานะการล็อกไว้อย่างละเอียดก่อนส่งมอบให้ลูกค้าทุกครั้ง";
        let breaks = thai_lao_breaks(text);
        assert!(!breaks.is_empty(), "should find Thai word boundaries");
        // Verify no mark is detached
        for &b in &breaks {
            let Some(next) = text[b..].chars().next() else { continue };
            let Some(prev) = text[..b].chars().next_back() else { continue };
            assert!(!is_sea_mark(next), "break before mark at {b}");
            assert!(!stays_with_previous(next), "break before stays_with_previous at {b}");
            assert!(!stays_with_next(prev), "break after stays_with_next at {b}");
        }
    }

    #[test]
    fn test_sea_classification() {
        assert!(is_sea('ก')); // Thai Ko Kai
        assert!(is_sea('ກ')); // Lao Ko
        assert!(is_sea('ក')); // Khmer Ka
        assert!(is_sea('က')); // Myanmar Ka
        assert!(!is_sea('A'));
        assert!(!is_sea('1'));

        assert!(is_sea_mark('\u{0E31}')); // Thai Mai Han-Akat
        assert!(is_sea_mark('\u{17C6}')); // Khmer Nikahit
        assert!(!is_sea_mark('ก'));
    }
}
