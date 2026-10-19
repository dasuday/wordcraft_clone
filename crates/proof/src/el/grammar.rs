//! Greek grammar checks: repeated words, spacing around punctuation (with the Greek question
//! mark `;` and upper stop `·`) and the final `-ν` of the feminine article (`τη`/`την`,
//! `στη`/`στην`). The school-grammar rule: the `-ν` stays before a vowel and before `κ π τ ξ ψ`
//! and `μπ ντ γκ τσ τζ`, and is dropped before any other consonant (`την πόλη`, `τη μητέρα`).

use crate::{Issue, IssueKind, ProofLang, words};

use super::{is_greek_word, is_vowel, strip_accent};

fn issue(start: usize, end: usize, message: String, suggestions: Vec<String>) -> Issue {
    Issue { start, end, kind: IssueKind::Grammar, message, suggestions, lang: ProofLang::El }
}

pub fn check(text: &str) -> Vec<Issue> {
    let ws = words(text);
    let mut issues = Vec::new();
    repeated_words(text, &ws, &mut issues);
    final_nu(text, &ws, &mut issues);
    spaces(text, &mut issues);
    issues.sort_by_key(|i| (i.start, i.end));
    issues
}

fn repeated_words(text: &str, ws: &[(usize, usize)], v: &mut Vec<Issue>) {
    for pair in ws.windows(2) {
        let [(a0, b0), (a1, b1)] = pair else { continue };
        let (Some(w0), Some(w1), Some(gap)) = (text.get(*a0..*b0), text.get(*a1..*b1), text.get(*b0..*a1)) else { continue };
        if !gap.is_empty() && gap.chars().all(char::is_whitespace) && w0.chars().all(char::is_alphabetic) && w0.to_lowercase() == w1.to_lowercase() {
            v.push(issue(*a0, *b1, format!("Επανάληψη λέξης: «{w1}»"), vec![w0.to_string()]));
        }
    }
}

/// Does a word starting this way keep the article's `-ν` before it?
fn keeps_nu(next: &str) -> bool {
    let lower: Vec<char> = next.chars().take(2).flat_map(char::to_lowercase).map(strip_accent).collect();
    match lower.as_slice() {
        [c, ..] if is_vowel(*c) => true,
        ['κ' | 'π' | 'τ' | 'ξ' | 'ψ', ..] => true,
        // `τσ`, `τζ` start with `τ`, already kept above.
        ['μ', 'π'] | ['ν', 'τ'] | ['γ', 'κ'] => true,
        _ => false,
    }
}

fn final_nu(text: &str, ws: &[(usize, usize)], v: &mut Vec<Issue>) {
    for pair in ws.windows(2) {
        let [(a0, b0), (a1, b1)] = pair else { continue };
        let (Some(w0), Some(w1), Some(gap)) = (text.get(*a0..*b0), text.get(*a1..*b1), text.get(*b0..*a1)) else { continue };
        if gap != " " || !is_greek_word(w1) || !w1.chars().next().is_some_and(char::is_alphabetic) {
            continue;
        }
        let keep = keeps_nu(w1);
        match w0.to_lowercase().as_str() {
            "την" | "στην" if !keep => {
                let short = w0.get(..w0.len().saturating_sub('ν'.len_utf8())).unwrap_or(w0);
                v.push(issue(*a0, *b0, format!("Περιττό τελικό ν: «{short} {w1}»"), vec![short.to_string()]));
            }
            "τη" | "στη" if keep => {
                let nu = if w0.chars().all(char::is_uppercase) { 'Ν' } else { 'ν' };
                let long = format!("{w0}{nu}");
                v.push(issue(*a0, *b0, format!("Λείπει το τελικό ν: «{long} {w1}»"), vec![long]));
            }
            _ => {}
        }
    }
}

fn spaces(text: &str, v: &mut Vec<Issue>) {
    for (i, c) in text.char_indices() {
        if c != ' ' {
            continue;
        }
        let rest = text.get(i + 1..).unwrap_or("");
        let mut it = rest.chars();
        let Some(p) = it.next() else { continue };
        let after = it.next();
        // A ratio/score keeps the spaces in digit-space-colon-space-digit.
        let ratio = p == ':'
            && text.get(..i).and_then(|before| before.chars().next_back()).is_some_and(|c| c.is_ascii_digit())
            && after == Some(' ')
            && it.next().is_some_and(|c| c.is_ascii_digit());
        let flag = !ratio
            && match p {
                // The upper stop (ano teleia), either code point.
                ',' | '·' | '\u{0387}' => true,
                // `;` is the Greek question mark; `;)` and `;-)` are emoticons.
                ';' | '\u{037E}' => !matches!(after, Some(')' | '(' | '-' | 'P' | 'D')),
                // ` ...` is an ellipsis, ` .5` a number.
                '.' => !matches!(after, Some('.')) && !after.is_some_and(|a| a.is_ascii_digit()),
                // ` :)`, ` :-)`, ` :D` and similar are emoticons; ` ?!` is still a stray space.
                ':' | '!' | '?' => after.is_none_or(|a| a.is_whitespace() || matches!(a, '"' | '»' | '”' | '’' | '!' | '?')),
                _ => false,
            };
        if flag {
            v.push(issue(i, i + 1 + p.len_utf8(), "Κενό πριν από σημείο στίξης".into(), vec![p.to_string()]));
        }
    }
    for (i, _) in text.match_indices("  ") {
        if i > 0 && text.get(..i).is_some_and(|t| t.ends_with(' ')) {
            continue;
        }
        v.push(issue(i, i + 2, "Διπλό κενό".into(), vec![" ".into()]));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_words_and_spaces_have_greek_messages() {
        let text = "Το το σπίτι ,  τι κάνεις ;";
        let issues = check(text);
        assert_eq!(issues.len(), 4, "{issues:?}");
        assert_eq!(issues[0].message, "Επανάληψη λέξης: «το»");
        assert_eq!(issues[0].suggestions, ["Το"]);
        assert!(issues.iter().any(|i| i.message == "Κενό πριν από σημείο στίξης" && &text[i.start..i.end] == " ,"));
        assert!(issues.iter().any(|i| i.message == "Κενό πριν από σημείο στίξης" && &text[i.start..i.end] == " ;"));
        assert!(issues.iter().any(|i| i.message == "Διπλό κενό"));
        assert!(issues.iter().all(|i| i.lang == ProofLang::El && i.kind == IssueKind::Grammar));
        for text in ["Τι κάνεις; Καλά.", "Ναι· και όχι", "Το σπίτι, το δέντρο.", "Χαμόγελο ;) και αριθμός .5 και τελείες ...", "Σκορ 2 : 1."]
        {
            assert!(check(text).is_empty(), "{text}: {:?}", check(text));
        }
    }

    #[test]
    fn final_nu_of_the_feminine_article() {
        for (text, wrong, fix) in [
            ("Είδα την μητέρα.", "την", "τη"),
            ("Πήγα στην θάλασσα.", "στην", "στη"),
            ("ΣΤΗΝ ΘΑΛΑΣΣΑ", "ΣΤΗΝ", "ΣΤΗ"),
            ("Είδα τη πόλη.", "τη", "την"),
            ("Πήγα στη Αθήνα.", "στη", "στην"),
            ("Τη μπάλα", "Τη", "Την"),
            ("ΣΤΗ ΠΟΛΗ", "ΣΤΗ", "ΣΤΗΝ"),
        ] {
            let issues = check(text);
            assert_eq!(issues.len(), 1, "{text}: {issues:?}");
            assert_eq!(&text[issues[0].start..issues[0].end], wrong, "{text}");
            assert_eq!(issues[0].suggestions, [fix], "{text}");
        }
        for text in [
            "Είδα τη μητέρα.",
            "Είδα την πόλη.",
            "Την άλλη μέρα",
            "στην έξοδο",
            "την ντομάτα",
            "την ψυχή",
            "τη θάλασσα",
            "Την είδα",
            "την Ελλάδα",
            "τη βλέπω",
            "την 3η μέρα",
            "την VW",
            "την, μητέρα",
        ] {
            assert!(check(text).is_empty(), "{text}: {:?}", check(text));
        }
    }

    proptest::proptest! {
        #[test]
        fn never_panics_and_ranges_are_valid(s in "\\PC{0,80}") {
            for issue in check(&s) {
                proptest::prop_assert!(issue.start <= issue.end && issue.end <= s.len());
                proptest::prop_assert!(s.is_char_boundary(issue.start) && s.is_char_boundary(issue.end));
            }
        }

        #[test]
        fn greek_text_never_panics(s in "[α-ωάέήίόύώϊϋΐΰΑ-ΩΆΈΉΊΌΎΏ ;·,.]{0,60}") {
            for issue in check(&s) {
                proptest::prop_assert!(s.is_char_boundary(issue.start) && s.is_char_boundary(issue.end));
            }
        }
    }
}
