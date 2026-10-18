//! Greek suggestions. The commonest Greek slips come first: the final sigma (`λογοσ` →
//! `λόγος`) and a missing or misplaced accent (`καλημερα`, `καλήμερα` → `καλημέρα`). Then the
//! dictionary-driven ranking Czech uses ([`crate::cs::suggest::suggest_in`]: case, MAP, REP,
//! missing spaces, KEY, TRY edits and a stem scan), with every table from the installed `el_GR`
//! dictionary.

use crate::affix::{AffixDict, Casing, capitalize, casing};

use super::{accent_variants, fix_sigmas};

/// Longest word given suggestions, in characters.
const MAX_LETTERS: usize = 40;

/// Spelling suggestions for a Greek word, best first, at most `max`, in the word's case.
pub fn suggest(word: &str, max: usize) -> Vec<String> {
    suggest_in(&super::dictionary(), word, max)
}

/// [`suggest`] against a given dictionary.
pub fn suggest_in(d: &AffixDict, word: &str, max: usize) -> Vec<String> {
    let n = word.chars().count();
    if max == 0 || d.is_empty() || n == 0 || n > MAX_LETTERS || word.contains(char::is_whitespace) {
        return Vec::new();
    }
    let case = casing(word);
    let lower = fix_sigmas(&word.to_lowercase());
    let mut out: Vec<String> = Vec::new();
    // A word differing from one already offered only in case (`Σωστός` after `σωστός`) isn't
    // offered again.
    let push = |s: String, out: &mut Vec<String>| {
        let lower = s.to_lowercase();
        if s != word && !out.iter().any(|o| o.to_lowercase() == lower) {
            out.push(s);
        }
    };
    // Sigmas, then accents: as typed, then each vowel accented in turn, then with a diaeresis
    // splitting a vowel pair (`προιον` → `προϊόν`).
    let split = diaeresis_variants(&lower);
    let accented = by_stress(accent_variants(&lower)).into_iter().chain(split.iter().flat_map(|v| by_stress(accent_variants(v))));
    for cand in std::iter::once(lower.clone()).chain(accented) {
        // In the word's case; capitalized only when that's how the dictionary has it (`Αθήνα`).
        let shown = in_case(&cand, case);
        if d.can_suggest(&shown) {
            push(shown, &mut out);
        } else if case == Casing::Lower && d.can_suggest(&capitalize(&cand)) {
            push(capitalize(&cand), &mut out);
        }
    }
    // Already checked by the ranking itself (it keeps the case of mixed-case stems).
    for s in crate::cs::suggest::suggest_in(d, word, max) {
        push(s, &mut out);
    }
    out.truncate(max);
    out
}

/// [`accent_variants`] (the bare word, then the accent on each vowel from the left) with the
/// accented ones in the order Greek stress is likeliest: the second-to-last vowel, the last,
/// then further back (`καλημερα` → `καλημέρα` before the rarer `καλήμερα`).
fn by_stress(mut variants: Vec<String>) -> Vec<String> {
    if variants.len() < 2 {
        return variants;
    }
    let accented = variants.split_off(1);
    let n = accented.len();
    let mut ranked: Vec<(usize, String)> = accented
        .into_iter()
        .enumerate()
        .map(|(k, v)| {
            let from_end = n - 1 - k;
            (
                if from_end == 1 {
                    0
                } else if from_end == 0 {
                    1
                } else {
                    from_end
                },
                v,
            )
        })
        .collect();
    ranked.sort_by_key(|(rank, _)| *rank);
    variants.extend(ranked.into_iter().map(|(_, v)| v));
    variants
}

/// `lower` with one `ι` or `υ` after a vowel given a diaeresis (`προιον` → `προϊον`), one
/// variant per such letter.
fn diaeresis_variants(lower: &str) -> Vec<String> {
    let cs: Vec<char> = lower.chars().map(super::strip_accent).collect();
    (1..cs.len())
        .filter_map(|i| {
            let mark = match cs.get(i)? {
                'ι' => 'ϊ',
                'υ' => 'ϋ',
                _ => return None,
            };
            if !super::is_vowel(*cs.get(i - 1)?) {
                return None;
            }
            let mut v = cs.clone();
            *v.get_mut(i)? = mark;
            Some(v.into_iter().collect())
        })
        .collect()
}

/// A lower-case suggestion in the case of the word it replaces.
fn in_case(lower: &str, case: Casing) -> String {
    match case {
        Casing::Initial => capitalize(lower),
        Casing::Upper => lower.to_uppercase(),
        Casing::Lower | Casing::Mixed => lower.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sigma_and_accent_slips_come_first() {
        crate::el::test_dict::install();
        for (word, want) in [
            ("καλημερα", "καλημέρα"),
            ("ομορφο", "όμορφο"),
            ("πολυ", "πολύ"),
            ("καλήμερα", "καλημέρα"),
            ("Καλημερα", "Καλημέρα"),
            ("λογοσ", "λόγος"),
            ("λόγοσ", "λόγος"),
            ("σπιτι", "σπίτι"),
            ("σπιτια", "σπίτια"),
            ("Σπιτι", "Σπίτι"),
            ("αθηνα", "Αθήνα"),
            ("ειναι", "είναι"),
            ("προιον", "προϊόν"),
        ] {
            assert_eq!(suggest(word, 6).first().map(String::as_str), Some(want), "{word}: {:?}", suggest(word, 6));
        }
    }

    #[test]
    fn the_likelier_stress_comes_first() {
        // A real dictionary may list a rare form with the accent further back (`καλήμερα`).
        let d = AffixDict::from_hunspell("SET UTF-8\n", "2\nκαλήμερα\nκαλημέρα\n", &[]).unwrap();
        assert_eq!(suggest_in(&d, "Καλημερα", 4), ["Καλημέρα", "Καλήμερα"]);
        assert_eq!(by_stress(vec!["αβγα".into()]), ["αβγα"]);
    }

    #[test]
    fn other_slips_use_the_dictionary_tables() {
        crate::el::test_dict::install();
        for (word, want) in [("κια", "και"), ("σπίτ", "σπίτι"), ("μητερα", "μητέρα"), ("ελληνικα", "ελληνικά"), ("καλημέραα", "καλημέρα")]
        {
            assert!(suggest(word, 8).contains(&want.into()), "{word}: {:?}", suggest(word, 8));
        }
    }

    #[test]
    fn suggestions_stay_bounded_and_skip_forbidden_words() {
        crate::el::test_dict::install();
        for w in ["", "δύο λέξεις", &"α".repeat(41)] {
            assert!(suggest(w, 4).is_empty());
        }
        assert!(suggest("σπιτι", 0).is_empty());
        assert!(suggest_in(&AffixDict::default(), "σπιτι", 4).is_empty());
        assert!(suggest("σπίτι", 4).len() <= 4);
        assert!(!suggest("σπίτι", 4).contains(&"σπίτι".into()));
        assert!(!suggest("ζξψαπαγορευμενο", 8).contains(&"ζξψαπαγορευμένο".into()));
    }

    #[test]
    fn nosuggest_stems_and_forms_are_never_offered() {
        crate::affix::tests::assert_nosuggest_is_honoured(suggest_in);
    }
}
