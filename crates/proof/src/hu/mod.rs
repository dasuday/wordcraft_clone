//! Offline Hungarian spelling using the bundled Magyar Ispell 1.9 dictionary (MPL-2.0).
//! Source, upstream revision and redistribution terms: `assets/spelling/hu/` and `NOTICE`.
//! Spellbook handles the compound/affix rules missing from our smaller affix engine.

mod encoding;
pub mod grammar;

use spellbook::Dictionary;
use std::sync::OnceLock;

const AFF: &[u8] = include_bytes!("../../../../assets/spelling/hu/hu_HU.aff");
const DIC: &[u8] = include_bytes!("../../../../assets/spelling/hu/hu_HU.dic");
const MAX_WORD_BYTES: usize = 120;

/// Parse and validate a candidate dictionary before a maintainer replaces the bundled files.
/// The original bytes stay untouched on disk; only the parser's input is normalized.
pub fn parse_dictionary(aff: &[u8], dic: &[u8]) -> Result<Dictionary, String> {
    if aff.len() > 16 << 20 || dic.len() > 16 << 20 {
        return Err("Hungarian dictionary exceeds the 16 MiB per-file limit".into());
    }
    let aff = encoding::normalize_aff(aff)?;
    let dic = std::str::from_utf8(dic).map_err(|e| format!("Hungarian .dic: {e}"))?;
    Dictionary::new(&aff, dic).map_err(|e| format!("Hungarian dictionary: {e}"))
}

fn dictionary() -> Option<&'static Dictionary> {
    static DICT: OnceLock<Result<Dictionary, String>> = OnceLock::new();
    DICT.get_or_init(|| {
        let result = parse_dictionary(AFF, DIC);
        if let Err(e) = &result {
            log::error!("{e}");
        }
        result
    })
    .as_ref()
    .ok()
}

pub fn is_correct(word: &str) -> bool {
    let word = word.trim_matches(['\'', '’']);
    if word.is_empty() || crate::in_user_dictionary(&word.to_lowercase()) {
        return true;
    }
    // Other scripts are outside the Hungarian dictionary's scope.
    if !word.chars().any(|c| c.is_alphabetic() && c <= '\u{024f}') {
        return true;
    }
    if word.len() > MAX_WORD_BYTES {
        return false;
    }
    dictionary().is_none_or(|d| d.check(word))
}

pub fn suggest(word: &str, max: usize) -> Vec<String> {
    if max == 0 || word.is_empty() || word.len() > MAX_WORD_BYTES || word.chars().any(char::is_control) {
        return Vec::new();
    }
    let Some(d) = dictionary() else { return Vec::new() };
    let mut suggestions = Vec::new();
    d.suggest(word, &mut suggestions);
    suggestions.truncate(max.min(16));
    suggestions
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{IssueKind, Options, ProofLang, check_text, is_correct_in_with, suggest_in};

    #[test]
    fn bundled_dictionary_checks_real_hungarian_and_compounds_without_panicking() {
        assert!(dictionary().is_some());
        for w in ["magyar", "helyesírás", "ellenőrzés", "árvíztűrő", "tükörfúrógép", "ház", "házak", "házainkban", "könyvespolc", "Budapest"]
        {
            assert!(is_correct(w), "{w}");
        }
        for w in ["muszály", "hejesírás", "szépp", "konyv", "árvíztürő", "budapest"] {
            assert!(!is_correct(w), "{w}");
        }
    }

    #[test]
    fn suggestions_use_accents_and_hungarian_spelling() {
        for (wrong, right) in [("muszály", "muszáj"), ("hejesírás", "helyesírás"), ("konyv", "könyv"), ("árvíztürő", "árvíztűrő")] {
            let suggestions = suggest_in(wrong, 5, ProofLang::Hu);
            assert!(suggestions.contains(&right.to_string()), "{wrong}: {suggestions:?}");
            assert!(suggestions.len() <= 5);
        }
        let _ = suggest("A", 5); // Regression: title-case suggestions used an end index.
        assert!(suggest("konyv", 0).is_empty());
        assert!(suggest(&"ő".repeat(1000), 5).is_empty());
    }

    #[test]
    fn mixed_runs_ranges_options_and_user_dictionary() {
        let text = "Magyar hejesírás. English recieve.";
        let boundary = text.find("English").unwrap();
        let issues = check_text(text, &[(0..boundary, ProofLang::Hu), (boundary..text.len(), ProofLang::En)]);
        assert!(issues.iter().any(|i| i.lang == ProofLang::Hu
            && i.kind == IssueKind::Spelling
            && &text[i.start..i.end] == "hejesírás"
            && i.message == "Lehetséges helyesírási hiba"));
        assert!(issues.iter().any(|i| i.lang == ProofLang::En && &text[i.start..i.end] == "recieve"));
        let opts = Options { ignore_uppercase: false, ..Options::default() };
        assert!(!is_correct_in_with("HEJESÍRÁS", ProofLang::Hu, &opts));
        assert!(is_correct_in_with("HEJESÍRÁS", ProofLang::Hu, &Options::default()));
        crate::add_word("hungarianregressioncustomword");
        assert!(is_correct("hungarianregressioncustomword"));
        crate::remove_word("hungarianregressioncustomword");
        assert!(crate::missing_dictionary(Some("hu-HU")).is_none());
        assert!(crate::hyphen::hyphen_points_in("tükörfúrógép", &crate::hyphen::Limits::default(), ProofLang::Hu).is_empty());
    }

    proptest::proptest! {
        #![proptest_config(proptest::test_runner::Config { cases: 64, failure_persistence: None, ..Default::default() })]
        #[test]
        fn arbitrary_unicode_is_safe(s in "\\PC{0,48}") {
            let _ = is_correct(&s);
            let suggestions = suggest(&s, 3);
            proptest::prop_assert!(suggestions.len() <= 3);
        }
    }
}
