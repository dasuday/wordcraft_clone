//! Greek proofing using the installed Hunspell `el_GR` dictionary (never bundled; Debian and
//! Ubuntu ship it as `hunspell-el`, LibreOffice as its `dict-el` extension). Greek needs a few
//! things of its own on top of the dictionary:
//! - words in capitals drop the accent (`ΚΑΛΗΜΕΡΑ` is `καλημέρα`), so a word in capitals is
//!   correct when the dictionary has it with an accent on one of its vowels;
//! - `ς` ends a word and `σ` never does, so suggestions fix the sigma first;
//! - a missing or misplaced accent (`καλημερα`, `καλήμερα`) is the commonest slip, so those
//!   corrections rank first;
//! - elided words keep an apostrophe (`απ' το`, `γι' αυτό`, `όλ' αυτά`).
//!
//! Text in Greek letters under the default English language is proofed as Greek
//! ([`is_greek_word`], [`looks_greek`]), as Word's "detect language automatically" does.
//! Suggestions also use the dictionary's own MAP/REP/TRY/KEY tables. Grammar: repeated words,
//! spacing and the final `-ν` of the feminine article ([`grammar`]). Hyphenation keeps the
//! English fallback (no Greek patterns are bundled).

pub mod grammar;
mod suggest;
#[cfg(test)]
pub(crate) mod test_dict;

use std::sync::{Arc, RwLock};

use crate::affix::AffixDict;

pub use suggest::suggest;

/// The Greek dictionary: `None` until first use, then the installed one (or an empty one).
static DICT: RwLock<Option<Arc<AffixDict>>> = RwLock::new(None);

/// File names of a Greek Hunspell dictionary.
const NAMES: &[&str] = &["el_GR", "el-GR", "el"];

/// Longest word checked, in characters (longer ones are accepted unchecked).
const MAX_CHARS: usize = 64;

/// Elisions with a rough breathing, where the consonant before the apostrophe changes
/// (`καθ' όλη`, `αφ' ότου`): no dictionary word is the elided one plus a vowel.
const ASPIRATED_ELISIONS: &[&str] = &["καθ", "μεθ", "αφ", "εφ", "υφ", "ανθ", "ουθ", "ουδ"];

/// The Greek dictionary: the installed Hunspell `el_GR` (read on first use), one given with
/// [`install_dictionary`], or an empty one when none is available (Greek spelling is off then;
/// see [`has_dictionary`]).
pub fn dictionary() -> Arc<AffixDict> {
    if let Some(d) = DICT.read().unwrap_or_else(|e| e.into_inner()).as_ref() {
        return d.clone();
    }
    let loaded = Arc::new(load_installed());
    let mut slot = DICT.write().unwrap_or_else(|e| e.into_inner());
    // Another thread (or an install) may have filled it meanwhile: that one wins.
    slot.get_or_insert(loaded).clone()
}

fn load_installed() -> AffixDict {
    let Some((aff, dic)) = crate::hunspell::find(NAMES) else {
        crate::hunspell::clear_load_error(NAMES[0]);
        log::info!("Greek spelling: no Hunspell el_GR dictionary installed");
        return AffixDict::default();
    };
    crate::hunspell::note_load(NAMES[0], &dic, crate::hunspell::load(&aff, &dic)).unwrap_or_else(|e| {
        log::warn!("Greek spelling: {e}");
        AffixDict::default()
    })
}

/// Use this Greek dictionary from now on (Hunspell `.aff` and `.dic` text, decoded to UTF-8),
/// for example one the user picked. On error the current dictionary stays.
pub fn install_dictionary(aff: &str, dic: &str) -> Result<(), String> {
    let d = AffixDict::from_hunspell(aff, dic, &[])?;
    *DICT.write().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(d));
    Ok(())
}

/// Forget the loaded dictionary, so the next check looks for an installed one again (after
/// [`crate::hunspell::set_dictionary_dir`] or a dictionary was installed).
pub fn reload_dictionary() {
    *DICT.write().unwrap_or_else(|e| e.into_inner()) = None;
}

/// The `.dic` file of the Hunspell `el_GR` dictionary found in the dictionaries folder or the
/// system locations now (File › Options › Proofing shows it); `None` when there is none. A
/// dictionary given with [`install_dictionary`] has no file and isn't reported.
pub fn installed_path() -> Option<std::path::PathBuf> {
    crate::hunspell::find(NAMES).map(|(_, dic)| dic)
}

/// Is a Greek dictionary available (spelling is checked)?
pub fn has_dictionary() -> bool {
    !dictionary().is_empty()
}

/// Is `c` a letter of the Greek script (monotonic, or polytonic from Greek Extended)?
pub fn is_greek_letter(c: char) -> bool {
    matches!(c, '\u{0370}'..='\u{03FF}' | '\u{1F00}'..='\u{1FFF}') && c.is_alphabetic()
}

/// Is `word` written in Greek letters (most of its letters; `ΑΦΜ-ID` counts)? Such a word is
/// never an English misspelling: it is proofed as Greek.
pub fn is_greek_word(word: &str) -> bool {
    let (mut greek, mut letters) = (0usize, 0usize);
    for c in word.chars().filter(|c| c.is_alphabetic()) {
        letters += 1;
        if is_greek_letter(c) {
            greek += 1;
        }
    }
    greek > 0 && greek * 2 > letters
}

/// Does a paragraph read as Greek? More than half of its letters are Greek ones. A Greek
/// paragraph under the default English language gets Greek grammar, and an English one quoting
/// a Greek word keeps the English rules.
pub fn looks_greek(text: &str) -> bool {
    let (mut greek, mut letters) = (0usize, 0usize);
    for c in text.chars().filter(|c| c.is_alphabetic()) {
        letters += 1;
        if is_greek_letter(c) {
            greek += 1;
        }
    }
    greek > 0 && greek * 2 > letters
}

/// `c` without its accent (tonos) and diaeresis (dialytika): `ά` → `α`, `ΐ` → `ι`, `Ώ` → `Ω`.
pub fn strip_accent(c: char) -> char {
    match c {
        'ά' => 'α',
        'έ' => 'ε',
        'ή' => 'η',
        'ί' | 'ϊ' | 'ΐ' => 'ι',
        'ό' => 'ο',
        'ύ' | 'ϋ' | 'ΰ' => 'υ',
        'ώ' => 'ω',
        'Ά' => 'Α',
        'Έ' => 'Ε',
        'Ή' => 'Η',
        'Ί' | 'Ϊ' => 'Ι',
        'Ό' => 'Ο',
        'Ύ' | 'Ϋ' => 'Υ',
        'Ώ' => 'Ω',
        c => c,
    }
}

/// A lower-case vowel with the accent (`α` → `ά`, `ϊ` → `ΐ`); `None` for anything else.
pub(crate) fn with_tonos(c: char) -> Option<char> {
    Some(match c {
        'α' => 'ά',
        'ε' => 'έ',
        'η' => 'ή',
        'ι' => 'ί',
        'ο' => 'ό',
        'υ' => 'ύ',
        'ω' => 'ώ',
        'ϊ' => 'ΐ',
        'ϋ' => 'ΰ',
        _ => return None,
    })
}

/// Is `c` a Greek vowel (any case, accented or not)?
pub fn is_vowel(c: char) -> bool {
    let base = strip_accent(c);
    matches!(base, 'α' | 'ε' | 'η' | 'ι' | 'ο' | 'υ' | 'ω' | 'Α' | 'Ε' | 'Η' | 'Ι' | 'Ο' | 'Υ' | 'Ω')
}

/// `lower` (a lower-case word) with every accent taken off and, one vowel at a time, put back:
/// the words a word in capitals or one with a slipped accent may stand for. Diaereses stay.
pub(crate) fn accent_variants(lower: &str) -> Vec<String> {
    let bare: Vec<char> = lower
        .chars()
        .map(|c| {
            if matches!(c, 'ϊ' | 'ΐ') {
                'ϊ'
            } else if matches!(c, 'ϋ' | 'ΰ') {
                'ϋ'
            } else {
                strip_accent(c)
            }
        })
        .collect();
    let mut out = vec![bare.iter().collect::<String>()];
    for (i, &c) in bare.iter().enumerate() {
        if let Some(t) = with_tonos(c) {
            let mut v = bare.clone();
            if let Some(slot) = v.get_mut(i) {
                *slot = t;
            }
            out.push(v.into_iter().collect());
        }
    }
    out
}

/// `w` with its sigmas in place: `σ` at the end of a word is `ς`, `ς` inside it `σ`.
pub(crate) fn fix_sigmas(w: &str) -> String {
    let cs: Vec<char> = w.chars().collect();
    cs.iter()
        .enumerate()
        .map(|(i, &c)| {
            let last = cs.get(i + 1).is_none_or(|n| !n.is_alphabetic());
            let after_letter = i > 0 && cs.get(i - 1).is_some_and(|p| p.is_alphabetic());
            match c {
                'σ' if last && after_letter => 'ς',
                'ς' if !last => 'σ',
                c => c,
            }
        })
        .collect()
}

/// Is `word` spelled correctly in Greek? Numbers, single letters, words with digits and words
/// in other scripts are accepted, and so is every word when no Greek dictionary is installed.
pub fn is_correct(word: &str) -> bool {
    is_correct_with(&dictionary(), word)
}

/// [`is_correct`] against a given dictionary (an empty one accepts everything).
pub fn is_correct_with(d: &AffixDict, word: &str) -> bool {
    if d.is_empty() {
        return true; // no Greek dictionary installed: no spelling marks
    }
    let w = word.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'' && c != '’');
    let w = w.trim_matches(['\'', '’']);
    let n = w.chars().count();
    if n <= 1 || n > MAX_CHARS || w.chars().any(|c| c.is_ascii_digit()) || !w.chars().any(char::is_alphabetic) {
        return true;
    }
    if !w.chars().any(is_greek_letter) {
        return true; // other scripts: no dictionary
    }
    if w.contains('-') {
        // Each part on its own; a short one in capitals is an abbreviation (`Αθήνα-ΕΕ`).
        return known(d, w)
            || w.split('-').all(|p| {
                let n = p.chars().count();
                n <= 1 || !p.chars().any(is_greek_letter) || (n <= 6 && p.chars().all(|c| !c.is_lowercase())) || known(d, p)
            });
    }
    known(d, w)
}

/// Is one word in the dictionary or the user dictionary? A word in capitals may have lost its
/// accent (`ΚΑΛΗΜΕΡΑ`).
fn known(d: &AffixDict, w: &str) -> bool {
    if d.check(w) || crate::in_user_dictionary(&w.to_lowercase()) {
        return true;
    }
    if crate::affix::casing(w) != crate::affix::Casing::Upper {
        return false;
    }
    let lower = w.to_lowercase();
    accent_variants(&lower).iter().any(|v| d.contains(v) || d.contains(&crate::affix::capitalize(v)))
}

/// Is the word at the end of `word`, with `after` the text right after it, an elision (`απ'`,
/// `γι'`, `όλ'`, `καθ'`)? It is when an apostrophe follows and the dictionary has the word
/// with a vowel added (`από`, `για`, `όλα`), or it is an aspirated one (`καθ`, `αφ`).
pub fn is_elided(word: &str, after: &str) -> bool {
    if !after.starts_with(['\'', '’']) {
        return false;
    }
    let lower = word.to_lowercase();
    if lower.chars().count() > 6 || !lower.chars().all(is_greek_letter) {
        return false;
    }
    if ASPIRATED_ELISIONS.contains(&lower.as_str()) {
        return true;
    }
    let d = dictionary();
    if d.is_empty() {
        return true;
    }
    ['α', 'ε', 'ι', 'ο', 'ά', 'έ', 'ί', 'ό'].iter().any(|v| d.check(&format!("{lower}{v}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dictionary_lookup_accepts_greek_file_names() {
        let dir = std::env::temp_dir().join(format!("wordcraft-el-aliases-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["el_GR", "el-GR", "el"] {
            std::fs::write(dir.join(format!("{name}.aff")), test_dict::AFF).unwrap();
            std::fs::write(dir.join(format!("{name}.dic")), test_dict::DIC).unwrap();
            let (aff, dic) = crate::hunspell::find_in(&dir, NAMES).unwrap();
            let d = crate::hunspell::load(&aff, &dic).unwrap();
            assert!(is_correct_with(&d, "σπίτι"));
            std::fs::remove_file(aff).unwrap();
            std::fs::remove_file(dic).unwrap();
        }
        std::fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn greek_words_affixes_case_and_capitals_without_accents() {
        test_dict::install();
        assert!(has_dictionary());
        for w in [
            "σπίτι",
            "σπίτια",
            "λόγος",
            "λόγοι",
            "καλημέρα",
            "Καλημέρα",
            "ΚΑΛΗΜΕΡΑ",
            "ΚΑΛΗΜΈΡΑ",
            "ΛΟΓΟΣ",
            "Αθήνα",
            "ΑΘΗΝΑ",
            "είναι",
            "και",
            "ελληνικά",
            "Αθήνα-ΕΕ",
            "ελληνικό-αγγλικό",
        ] {
            assert!(is_correct(w), "{w}");
        }
        for w in ["καλημερα", "καλήμερα", "λογοσ", "λόγοσ", "σπιτι", "αθήνα", "Ειναι", "κια", "ΚΑΛΗΜΕΡΕΣ"]
        {
            assert!(!is_correct(w), "{w}");
        }
    }

    #[test]
    fn missing_dictionary_and_ignored_inputs_are_quiet() {
        let empty = AffixDict::default();
        assert!(is_correct_with(&empty, "καλημερα"));
        assert!(install_dictionary("FLAG long\n", "1\nσπίτι\n").is_err());
        let d = AffixDict::from_hunspell(test_dict::AFF, test_dict::DIC, &[]).unwrap();
        for w in ["", "α", "2026", "3ος", "word", "日本語", "-", "--", &"α".repeat(70)] {
            assert!(is_correct_with(&d, w), "{w}");
        }
    }

    #[test]
    fn user_dictionary_applies() {
        test_dict::install();
        assert!(!is_correct("ζκλεξηδική"));
        crate::add_word("Ζκλεξηδική");
        assert!(is_correct("ζκλεξηδική"));
    }

    #[test]
    fn script_detection() {
        for w in ["λόγος", "ΑΘΗΝΑ", "Ελλάδα-EU", "ΐ"] {
            assert!(is_greek_word(w), "{w}");
        }
        for w in ["logos", "", "2026", "EU-Ελ", "日本"] {
            assert!(!is_greek_word(w), "{w}");
        }
        assert!(looks_greek("Καλημέρα, τι κάνεις; OK"));
        assert!(!looks_greek("The word λόγος means word."));
    }

    #[test]
    fn sigmas_and_accents() {
        assert_eq!(fix_sigmas("λογοσ"), "λογος");
        assert_eq!(fix_sigmas("ςτο σ λογοσ."), "στο σ λογος.");
        assert_eq!(fix_sigmas("ΛΟΓΟΣ"), "ΛΟΓΟΣ");
        assert_eq!(accent_variants("καλημερα"), ["καλημερα", "κάλημερα", "καλήμερα", "καλημέρα", "καλημερά"]);
        assert_eq!(accent_variants("προϊον"), ["προϊον", "πρόϊον", "προΐον", "προϊόν"]);
        assert_eq!(strip_accent('ΐ'), 'ι');
        assert!(is_vowel('Ώ') && is_vowel('ϋ') && !is_vowel('σ'));
    }

    #[test]
    fn elisions() {
        test_dict::install();
        assert!(is_elided("απ", "' το"));
        assert!(is_elided("γι", "’ αυτό"));
        assert!(is_elided("καθ", "' όλη"));
        assert!(!is_elided("απ", " το"));
        assert!(!is_elided("ξψζ", "' το"));
        assert!(!is_elided("ok", "'"));
    }

    #[test]
    fn greek_dispatch_messages_and_detection() {
        test_dict::install();
        assert!(crate::has_dictionary(Some("el-GR")));
        assert!(crate::is_correct_for("σπίτι", Some("ell")));
        let text = "Το σπιτι μου";
        let issues = crate::check_text(text, &[(0..text.len(), crate::ProofLang::El)]);
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert_eq!(&text[issues[0].start..issues[0].end], "σπιτι");
        assert_eq!(issues[0].message, "Πιθανό ορθογραφικό λάθος");
        assert_eq!(issues[0].lang, crate::ProofLang::El);
        assert_eq!(crate::suggest_in("σπιτι", 6, crate::ProofLang::El).first().map(String::as_str), Some("σπίτι"));
        // Greek typed under the default English language is proofed as Greek.
        let issues = crate::check_text("Hello καλημερα world", &[]);
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert_eq!(issues[0].lang, crate::ProofLang::El);
        let issues = crate::check_text("Το το σπίτι", &[]);
        assert!(issues.iter().any(|i| i.lang == crate::ProofLang::El && i.kind == crate::IssueKind::Grammar), "{issues:?}");
        // Elided words with an apostrophe aren't misspelled.
        assert!(crate::check_text("Ήρθε απ' το σπίτι γι' αυτό", &[]).is_empty());
        let opts = crate::Options { ignore_uppercase: false, ..Default::default() };
        assert!(!crate::is_correct_in_with("ΣΠΙΤΑΚΙΑ", crate::ProofLang::El, &opts));
        assert!(crate::is_correct_in_with("ΣΠΙΤΙΑ", crate::ProofLang::El, &opts));
    }
}
