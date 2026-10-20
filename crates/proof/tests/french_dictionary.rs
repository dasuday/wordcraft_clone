//! Explicit real-dictionary gate. The normal synthetic suite stays offline; CI installs the
//! pinned Dicollecte dictionary and runs this test with `--ignored`. Missing data is a failure,
//! never a silently successful test that checked nothing.

#![cfg(not(target_arch = "wasm32"))]

use wordcraft_proof::{ProofLang, fr};

#[test]
#[ignore = "requires a real French dictionary in WORDCRAFT_DICTIONARIES"]
fn real_french_dictionary_spelling_suggestions_and_dispatch() {
    let directory = std::env::var_os("WORDCRAFT_DICTIONARIES").expect("set WORDCRAFT_DICTIONARIES to the test dictionary folder");
    let installed = fr::installed_path().expect("the real French dictionary must be installed");
    assert_eq!(installed.parent(), Some(std::path::Path::new(&directory)), "do not silently use another system dictionary");
    let dictionary = fr::dictionary();
    assert!(dictionary.len() > 50_000, "expected Dicollecte, not the synthetic fixture: {}", dictionary.len());
    for word in [
        "maison",
        "maisons",
        "Paris",
        "mangeons",
        "finissaient",
        "étaient",
        "l'arbre",
        "l’arbre",
        "qu'elles",
        "école",
        "œuvre",
        "aujourd'hui",
        "chevaux",
        "heureuse",
        "va-t-il",
    ] {
        assert!(fr::is_correct(word), "correct French word: {word}");
    }
    for (typo, expected) in [("maisonn", "maison"), ("ecole", "école"), ("l’ecole", "l’école"), ("documant", "document")] {
        assert!(!fr::is_correct(typo), "misspelling: {typo}");
        let suggestions = fr::suggest(typo, 8);
        assert!(suggestions.iter().any(|s| s == expected), "{typo} -> {suggestions:?}, expected {expected}");
    }
    let text = "Une maisonn près de l’école.";
    for lang in [ProofLang::Fr, ProofLang::FrCa] {
        let issues = wordcraft_proof::check_text(text, &[(0..text.len(), lang)]);
        assert_eq!(issues.len(), 1, "{lang:?}: {issues:?}");
        let issue = &issues[0];
        assert_eq!(&text[issue.start..issue.end], "maisonn");
        assert_eq!(issue.lang, lang);
        assert_eq!(issue.message, "Faute d’orthographe possible");
    }
    assert!(wordcraft_proof::dictionary_load_error(ProofLang::Fr).is_none());
}
