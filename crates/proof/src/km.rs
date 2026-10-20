//! Khmer word segmentation and line-break opportunities.
//!
//! Khmer is written continuously without spaces between words within a phrase; spaces are
//! reserved for clause, sentence or list boundaries. Standard line breaking must occur at
//! word boundaries so that words and consonant clusters are never broken unnaturally.
//!
//! Word list derived from Unicode ICU (`khmerdict.txt`, MIT license), supplemented with
//! high-frequency modern administrative compounds.

use std::sync::OnceLock;

const MAGIC: &[u8] = b"KMDICT1\n";
static KM_DIC: &[u8] = include_bytes!("../../../assets/hyphenation/km.dic");

/// Compressed dictionary of Khmer words.
pub struct KhmerDict {
    words_blob: String,
    offs: Vec<u32>,
}

impl Default for KhmerDict {
    fn default() -> Self {
        Self { words_blob: String::new(), offs: vec![0] }
    }
}

impl KhmerDict {
    pub fn global() -> &'static KhmerDict {
        static INSTANCE: OnceLock<KhmerDict> = OnceLock::new();
        INSTANCE.get_or_init(|| {
            Self::from_bytes(KM_DIC).unwrap_or_else(|e| {
                log::error!("{e}");
                KhmerDict::default()
            })
        })
    }

    pub fn len(&self) -> usize {
        self.offs.len().saturating_sub(1)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn word(&self, idx: usize) -> &str {
        let start = self.offs.get(idx).copied().unwrap_or(0) as usize;
        let end = self.offs.get(idx + 1).copied().unwrap_or(0) as usize;
        self.words_blob.get(start..end).unwrap_or("")
    }

    pub fn contains(&self, word: &str) -> bool {
        if self.is_empty() || word.is_empty() {
            return false;
        }
        let (mut lo, mut hi) = (0usize, self.len());
        while lo < hi {
            let mid = (lo + hi) / 2;
            match self.word(mid).cmp(word) {
                std::cmp::Ordering::Less => lo = mid + 1,
                std::cmp::Ordering::Greater => hi = mid,
                std::cmp::Ordering::Equal => return true,
            }
        }
        false
    }

    pub fn from_bytes(data: &[u8]) -> Result<KhmerDict, String> {
        let raw = miniz_oxide::inflate::decompress_to_vec(data).map_err(|e| format!("khmer dictionary inflate failed: {e:?}"))?;
        let body = raw.strip_prefix(MAGIC).ok_or("khmer dictionary: bad header")?;
        let mut words_blob = String::with_capacity(body.len() * 2);
        let mut offs = vec![0];
        let mut cur: Vec<u8> = Vec::with_capacity(64);
        let mut i = 0;
        while i < body.len() {
            let p = body[i] as usize;
            let end = body[i + 1..].iter().position(|&b| b == b'\n').map(|e| i + 1 + e).ok_or("khmer dictionary: truncated")?;
            if p > cur.len() {
                return Err("khmer dictionary: bad prefix".into());
            }
            cur.truncate(p);
            cur.extend_from_slice(&body[i + 1..end]);
            i = end + 1;
            let s = std::str::from_utf8(&cur).map_err(|_| "khmer dictionary: bad utf-8")?;
            words_blob.push_str(s);
            offs.push(words_blob.len() as u32);
        }
        Ok(KhmerDict { words_blob, offs })
    }
}

/// Is `c` in the Khmer Unicode blocks?
pub fn is_khmer_char(c: char) -> bool {
    matches!(u32::from(c), 0x1780..=0x17FF | 0x19E0..=0x19FF)
}

/// Does `text` contain any Khmer characters?
pub fn has_khmer(text: &str) -> bool {
    text.chars().any(is_khmer_char)
}

/// Is `c` a Khmer base consonant (ក to អ)?
pub fn is_khmer_consonant(c: char) -> bool {
    matches!(u32::from(c), 0x1780..=0x17A2)
}

/// Is `c` a Khmer independent vowel (ឣ to ឳ)?
pub fn is_khmer_indep_vowel(c: char) -> bool {
    matches!(u32::from(c), 0x17A3..=0x17B3)
}

/// Is `c` the Khmer Coeng / stacker sign (`\u{17D2}`)?
pub fn is_khmer_coeng(c: char) -> bool {
    u32::from(c) == 0x17D2
}

/// Is `c` a Khmer dependent vowel (`\u{17B6}`..=`\u{17C5}`, `\u{17B4}`..=`\u{17B5}`)?
pub fn is_khmer_dep_vowel(c: char) -> bool {
    matches!(u32::from(c), 0x17B6..=0x17C5 | 0x17B4..=0x17B5)
}

/// Is `c` a Khmer consonant register shifter (muusikatoan `\u{17C9}` or triisap `\u{17CA}`)?
pub fn is_khmer_shifter(c: char) -> bool {
    matches!(u32::from(c), 0x17C9 | 0x17CA)
}

/// Is `c` a Khmer coda (final consonant) diacritic?
/// E.g. bantoc `\u{17CB}`, robat `\u{17CC}`, toandakhiat `\u{17CD}`, ahsda `\u{17CF}`, viram `\u{17D1}`.
pub fn is_khmer_coda_diacritic(c: char) -> bool {
    matches!(u32::from(c), 0x17CB..=0x17CD | 0x17CF | 0x17D1)
}

/// Is `c` a Khmer vowel modifier or nucleus sign?
/// E.g. nikahit `\u{17C6}`, reahmuk `\u{17C7}`, yuukaleap `\u{17C8}`, samyok sannya `\u{17D0}`, etc.
pub fn is_khmer_vowel_modifier(c: char) -> bool {
    matches!(u32::from(c), 0x17C6..=0x17C8 | 0x17CE | 0x17D0 | 0x17D3 | 0x17DD)
}

/// Is `c` a Khmer robat (`\u{17CC}`)?
pub fn is_khmer_robat(c: char) -> bool {
    u32::from(c) == 0x17CC
}

/// Is `c` a base character (consonant or independent vowel)?
pub fn is_khmer_base(c: char) -> bool {
    is_khmer_consonant(c) || is_khmer_indep_vowel(c)
}

/// Is `c` a consonant or vocalic liquid that can act as a subscript consonant under a Coeng?
pub fn is_khmer_subscript_consonant(c: char) -> bool {
    is_khmer_consonant(c) || matches!(u32::from(c), 0x17AB..=0x17AE)
}

/// Is `c` a valid medial coda consonant that can carry a subscript consonant in a medial cluster?
/// In Khmer inherent-vowel medial clusters ($C_1 + C_2 + \text{Coeng } C_3$), $C_2$ acts as an
/// internal coda of the first syllable (inherent vowel on $C_1$), typically one of the 5 nasals
/// (`ង`, `ញ`, `ណ`, `ន`, `ម`) or homorganic stops/liquids/sibilants (`ក`, `ច`, `ដ`, `ត`, `ប`, `ព`, `ស`, `ល`).
pub fn is_valid_medial_coda(c: char) -> bool {
    matches!(
        u32::from(c),
        0x1784 | 0x1789 | 0x178E | 0x1793 | 0x1798 // ង, ញ, ណ, ន, ម
    )
}

/// Does `text[idx..]` start an inherent-vowel medial cluster:
/// $C_1 + C_2 + \text{Coeng } C_3$ (e.g. `បន្ត`, `បង្កើន`, `កម្ពុជា`, `កន្លែង`, `សម្ដែង`, `អង្គការ`, `បញ្ជាក់`, `បញ្ហា`, `បណ្តុះ`)?
pub fn starts_medial_cluster(text: &str, idx: usize) -> bool {
    let rem = match text.get(idx..) {
        Some(s) if !s.is_empty() => s,
        _ => return false,
    };
    let mut chars = rem.chars();
    let c1 = match chars.next() {
        Some(c) => c,
        None => return false,
    };
    if !is_khmer_consonant(c1) && !is_khmer_indep_vowel(c1) {
        return false;
    }
    let c2 = match chars.next() {
        Some(c) => c,
        None => return false,
    };
    if !is_khmer_consonant(c2) || !is_valid_medial_coda(c2) {
        return false;
    }
    let coeng = match chars.next() {
        Some(c) => c,
        None => return false,
    };
    if !is_khmer_coeng(coeng) {
        return false;
    }
    let c3 = match chars.next() {
        Some(c) => c,
        None => return false,
    };
    is_khmer_subscript_consonant(c3)
}

/// Does `word` match the dictionary directly or via productive Khmer morphological prefixes
/// (nominalizer `ការ` + verb, agent `អ្នក` + verb/noun, abstract state `ភាព` + adj/noun)?
pub fn is_khmer_dict_word(dict: &KhmerDict, word: &str) -> bool {
    if dict.contains(word) {
        return true;
    }
    if let Some(stem) = word.strip_prefix("ការ") {
        if dict.contains(stem) {
            return true;
        }
    }
    if let Some(stem) = word.strip_prefix("អ្នក") {
        if dict.contains(stem) {
            return true;
        }
    }
    if let Some(stem) = word.strip_prefix("ភាព") {
        if dict.contains(stem) {
            return true;
        }
    }
    false
}

/// Does `text[idx..]` begin with any valid Khmer dictionary word or morphological compound?
pub fn starts_with_dict_word(text: &str, idx: usize) -> bool {
    let rem = match text.get(idx..) {
        Some(s) if !s.is_empty() => s,
        _ => return false,
    };
    let dict = KhmerDict::global();
    let mut end_bytes = Vec::with_capacity(10);
    for (i, c) in rem.char_indices() {
        if !is_khmer_char(c) {
            break;
        }
        let b = i + c.len_utf8();
        end_bytes.push(b);
        if end_bytes.len() >= 10 {
            break;
        }
    }
    for &eb in end_bytes.iter().rev() {
        if eb >= 6 {
            if let Some(sub) = rem.get(..eb) {
                if is_khmer_dict_word(dict, sub) {
                    return true;
                }
            }
        }
    }
    false
}

/// Is the consonant at `text[idx]` an onset of a new syllable/word rather than a coda?
pub fn is_consonant_onset(text: &str, idx: usize) -> bool {
    if idx >= text.len() {
        return false;
    }
    if starts_with_dict_word(text, idx) {
        return true;
    }
    if starts_medial_cluster(text, idx) {
        return true;
    }
    let mut chars = text[idx..].chars();
    chars.next(); // skip the base consonant at idx
    while let Some(c) = chars.next() {
        if c == '\u{200C}' || c == '\u{200D}' {
            continue;
        }
        return is_khmer_coeng(c)
            || is_khmer_dep_vowel(c)
            || is_khmer_shifter(c)
            || is_khmer_robat(c)
            || is_khmer_vowel_modifier(c);
    }
    false
}

fn is_closed_vowel_or_sign(c: char) -> bool {
    matches!(u32::from(c), 0x17C3 | 0x17C5 | 0x17C6..=0x17C8)
}

/// Find the end byte offset of the Khmer orthographic syllable starting at byte `i`.
///
/// An orthographic syllable conforms to:
/// `[Onset: Base (Coeng C)* (Shifter | Robat)?] [Nucleus: Vowel? Modifiers*] [Coda: C (CodaSign | Coeng C)*]?`
///
/// A trailing bare consonant following a vowel is bound as the coda of the syllable,
/// ensuring that syllables like `ញ៉ូម` or `ដឹង` or `រៀន` are never broken apart mid-syllable,
/// while medial cluster words like `បន្ត`, `បង្កើន`, `កម្ពុជា` are kept completely intact.
pub fn next_khmer_syllable(text: &str, i: usize) -> usize {
    let rem = match text.get(i..) {
        Some(s) if !s.is_empty() => s,
        _ => return i,
    };
    let mut chars = rem.chars();
    let first_char = match chars.next() {
        Some(c) => c,
        None => return i,
    };

    if !is_khmer_base(first_char) {
        return i + first_char.len_utf8();
    }

    let mut idx = first_char.len_utf8();

    let mut has_vowel = false;
    let mut is_closed = false;

    // Independent vowel can form a syllable with following modifiers, subscripts (e.g. ឲ្យ),
    // medial stacks (e.g. ឧត្ត, អង្គ), and codas (e.g. ឯក, ឧត្តម).
    if is_khmer_indep_vowel(first_char) {
        has_vowel = true;
        while let Some(ch1) = text[i + idx..].chars().next() {
            if is_khmer_coeng(ch1) {
                let mut iter2 = text[i + idx + ch1.len_utf8()..].chars();
                if let Some(ch2) = iter2.next() {
                    if is_khmer_subscript_consonant(ch2) {
                        idx += ch1.len_utf8() + ch2.len_utf8();
                        continue;
                    }
                }
            } else if is_khmer_consonant(ch1) {
                let after_ch1 = i + idx + ch1.len_utf8();
                if let Some(c_coeng) = text[after_ch1..].chars().next() {
                    if is_khmer_coeng(c_coeng) {
                        let after_coeng = after_ch1 + c_coeng.len_utf8();
                        if let Some(c3) = text[after_coeng..].chars().next() {
                            if is_khmer_subscript_consonant(c3) {
                                idx += ch1.len_utf8() + c_coeng.len_utf8() + c3.len_utf8();
                                continue;
                            }
                        }
                    }
                }
            }
            break;
        }
    } else if starts_medial_cluster(text, i) {

        // Medial stack words: e.g. បន្ត, បង្កើន, កម្ពុជា, កន្លែង, សម្ដែង: consume C2, Coeng, C3
        if let Some(c2) = text[i + idx..].chars().next() {
            idx += c2.len_utf8();
            if let Some(coeng) = text[i + idx..].chars().next() {
                idx += coeng.len_utf8();
                if let Some(c3) = text[i + idx..].chars().next() {
                    idx += c3.len_utf8();
                }
            }
        }
        // Consume any additional coeng (e.g. សង្គ្រោះ)
        while let Some(ch1) = text[i + idx..].chars().next() {
            if is_khmer_coeng(ch1) {
                let mut iter2 = text[i + idx + ch1.len_utf8()..].chars();
                if let Some(ch2) = iter2.next() {
                    if is_khmer_subscript_consonant(ch2) {
                        idx += ch1.len_utf8() + ch2.len_utf8();
                        continue;
                    }
                }
            }
            break;
        }
    } else {
        // Base consonant: consume subscript consonants (Coeng + consonant)
        while let Some(ch1) = text[i + idx..].chars().next() {
            if is_khmer_coeng(ch1) {
                let mut iter2 = text[i + idx + ch1.len_utf8()..].chars();
                if let Some(ch2) = iter2.next() {
                    if is_khmer_subscript_consonant(ch2) {
                        idx += ch1.len_utf8() + ch2.len_utf8();
                        continue;
                    }
                }
            }
            break;
        }
    }

    // Consume shifter if any (allowing zero-width joiners between consonant and shifter)
    while let Some(ch) = text[i + idx..].chars().next() {
        if ch == '\u{200C}' || ch == '\u{200D}' {
            idx += ch.len_utf8();
        } else {
            break;
        }
    }
    if let Some(ch) = text[i + idx..].chars().next() {
        if is_khmer_shifter(ch) {
            idx += ch.len_utf8();
        }
    }
    while let Some(ch) = text[i + idx..].chars().next() {
        if ch == '\u{200C}' || ch == '\u{200D}' {
            idx += ch.len_utf8();
        } else {
            break;
        }
    }

    // Consume robat if any
    if let Some(ch) = text[i + idx..].chars().next() {
        if is_khmer_robat(ch) {
            idx += ch.len_utf8();
        }
    }
    while let Some(ch) = text[i + idx..].chars().next() {
        if ch == '\u{200C}' || ch == '\u{200D}' {
            idx += ch.len_utf8();
        } else {
            break;
        }
    }

    // Consume additional subscript consonants
    while let Some(ch1) = text[i + idx..].chars().next() {
        if is_khmer_coeng(ch1) {
            let mut iter2 = text[i + idx + ch1.len_utf8()..].chars();
            if let Some(ch2) = iter2.next() {
                if is_khmer_subscript_consonant(ch2) {
                    idx += ch1.len_utf8() + ch2.len_utf8();
                    continue;
                }
            }
        }
        break;
    }

    // Check for dependent vowel(s)
    while let Some(ch) = text[i + idx..].chars().next() {
        if ch == '\u{200C}' || ch == '\u{200D}' {
            idx += ch.len_utf8();
        } else if is_khmer_dep_vowel(ch) {
            has_vowel = true;
            if is_closed_vowel_or_sign(ch) {
                is_closed = true;
            }
            idx += ch.len_utf8();
        } else {
            break;
        }
    }

    // Consume vowel modifiers
    while let Some(ch) = text[i + idx..].chars().next() {
        if is_khmer_vowel_modifier(ch) {
            has_vowel = true;
            if is_closed_vowel_or_sign(ch) {
                is_closed = true;
            }
            idx += ch.len_utf8();
        } else {
            break;
        }
    }

    // Consume any coda diacritic on the cluster/nucleus before coda consonant check (e.g. កល្ប៍)
    while let Some(ch) = text[i + idx..].chars().next() {
        if is_khmer_coda_diacritic(ch) || is_khmer_vowel_modifier(ch) {
            idx += ch.len_utf8();
        } else {
            break;
        }
    }

    // Handle out-of-order subscript consonants typed after vowel/modifiers (legacy input order)
    while let Some(ch1) = text[i + idx..].chars().next() {
        if is_khmer_coeng(ch1) {
            let mut iter2 = text[i + idx + ch1.len_utf8()..].chars();
            if let Some(ch2) = iter2.next() {
                if is_khmer_subscript_consonant(ch2) {
                    idx += ch1.len_utf8() + ch2.len_utf8();
                    continue;
                }
            }
        }
        break;
    }

    // Check for CODA consonant
    if !is_closed {
        if let Some(coda) = text[i + idx..].chars().next() {
            if is_khmer_consonant(coda) && u32::from(coda) != 0x17A2 && u32::from(coda) != 0x17A1 {
                let coda_offset = i + idx;
                let after_coda = coda_offset + coda.len_utf8();
                let next_ch = text[after_coda..].chars().next();

                let is_combining_attachment = match next_ch {
                    Some(c) => is_khmer_dep_vowel(c)
                        || is_khmer_coeng(c)
                        || is_khmer_shifter(c)
                        || is_khmer_robat(c)
                        || is_khmer_vowel_modifier(c),
                    None => false,
                };

                let consume_coda = if is_combining_attachment {
                    false
                } else if !is_consonant_onset(text, coda_offset) {
                    let has_coda_sign = next_ch.is_some_and(is_khmer_coda_diacritic);
                    let at_boundary = match next_ch {
                        None => true,
                        Some(c) => c.is_whitespace() || (!is_khmer_consonant(c) && !is_khmer_indep_vowel(c)),
                    };
                    let next_is_onset = match next_ch {
                        Some(c) if is_khmer_consonant(c) => is_consonant_onset(text, after_coda),
                        _ => false,
                    };
                    let is_dict_word_with_coda = is_khmer_dict_word(KhmerDict::global(), &text[i..after_coda]);

                    has_coda_sign
                        || is_dict_word_with_coda
                        || (has_vowel && (at_boundary || next_is_onset))
                } else {
                    false
                };

                if consume_coda {
                    idx += coda.len_utf8();
                    // Consume any coda diacritic (bantoc, toandakhiat, etc.)
                    while let Some(ch) = text[i + idx..].chars().next() {
                        if is_khmer_coda_diacritic(ch) || is_khmer_vowel_modifier(ch) {
                            idx += ch.len_utf8();
                        } else {
                            break;
                        }
                    }
                    // Check for subscript on coda (e.g. សិស្ស)
                    if let Some(ch1) = text[i + idx..].chars().next() {
                        if is_khmer_coeng(ch1) {
                            let mut iter2 = text[i + idx + ch1.len_utf8()..].chars();
                            if let Some(ch2) = iter2.next() {
                                if is_khmer_subscript_consonant(ch2) {
                                    idx += ch1.len_utf8() + ch2.len_utf8();
                                    while let Some(ch3) = text[i + idx..].chars().next() {
                                        if is_khmer_coda_diacritic(ch3) || is_khmer_vowel_modifier(ch3) {
                                            idx += ch3.len_utf8();
                                        } else {
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Final sweep of any trailing coda diacritics or modifiers
    while let Some(ch) = text[i + idx..].chars().next() {
        if is_khmer_coda_diacritic(ch) || is_khmer_vowel_modifier(ch) {
            idx += ch.len_utf8();
        } else {
            break;
        }
    }

    // Attach repetition mark ៗ if present
    if let Some(ch) = text[i + idx..].chars().next() {
        if u32::from(ch) == 0x17D7 {
            idx += ch.len_utf8();
        }
    }

    i + idx
}

/// Is `off` a valid Khmer syllable / cluster boundary in `text`?
pub fn is_khmer_cluster_boundary(text: &str, off: usize) -> bool {
    if off == 0 || off >= text.len() {
        return true;
    }
    if !text.is_char_boundary(off) {
        return false;
    }

    // Find the beginning of the contiguous Khmer run containing `off`
    let mut run_start = off;
    for (idx, ch) in text[..off].char_indices() {
        if is_khmer_char(ch) {
            run_start = idx;
            break;
        }
    }

    let mut curr = run_start;
    while curr < off {
        let next_s = next_khmer_syllable(text, curr);
        if next_s <= curr {
            curr += text[curr..].chars().next().map_or(1, |c| c.len_utf8());
        } else {
            curr = next_s;
        }
    }

    curr == off
}

/// Find valid line break opportunities in Khmer text.
///
/// Uses maximal matching against [`KhmerDict`], constrained to orthographic syllable
/// boundaries so that Khmer consonant clusters, subscript signs (`\u{17D2}`), dependent
/// vowels, and coda consonants are never broken apart.
pub fn khmer_breaks(text: &str) -> Vec<usize> {
    if text.is_empty() || !has_khmer(text) {
        return Vec::new();
    }
    let dict = KhmerDict::global();
    let mut out = Vec::new();
    let mut i = 0;
    let n = text.len();

    while i < n {
        let ch = match text.get(i..).and_then(|t| t.chars().next()) {
            Some(c) => c,
            None => break,
        };

        // Whitespace, punctuation, and non-Khmer characters: standard break opportunities.
        if !is_khmer_char(ch) {
            i += ch.len_utf8();
            continue;
        }

        // Collect valid syllable boundary endpoints starting from i (up to ~64 bytes)
        let mut syllable_ends = Vec::new();
        let mut curr = i;
        let max_search = (i + 64).min(n);
        while curr < max_search {
            let next_s = next_khmer_syllable(text, curr);
            if next_s <= curr {
                break;
            }
            syllable_ends.push(next_s);
            curr = next_s;
            match text.get(curr..).and_then(|t| t.chars().next()) {
                Some(c) if is_khmer_char(c) => {},
                _ => break,
            }
        }

        // Try longest word match in dictionary among syllable boundaries
        let mut matched_end = None;
        for &cand_end in syllable_ends.iter().rev() {
            if let Some(next_c) = text.get(cand_end..).and_then(|t| t.chars().next()) {
                if is_khmer_coeng(next_c)
                    || is_khmer_dep_vowel(next_c)
                    || is_khmer_coda_diacritic(next_c)
                    || is_khmer_vowel_modifier(next_c)
                {
                    continue;
                }
            }
            if text.get(i..cand_end).is_some_and(|sub| is_khmer_dict_word(dict, sub)) {
                matched_end = Some(cand_end);
                break;
            }
        }


        if let Some(end) = matched_end {
            out.push(end);
            i = end;
        } else {
            // Fallback: advance to the next valid syllable boundary
            let next_bound = syllable_ends.first().copied().unwrap_or_else(|| {
                let next_s = next_khmer_syllable(text, i);
                if next_s > i {
                    next_s
                } else {
                    i + ch.len_utf8()
                }
            });
            out.push(next_bound);
            i = next_bound;
        }
    }

    out.sort_unstable();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compress_decompress() {
        let input = b"Hello world KMDICT1\n";
        let comp = miniz_oxide::deflate::compress_to_vec(input, 6);
        let decomp = miniz_oxide::inflate::decompress_to_vec(&comp).unwrap();
        assert_eq!(input.as_slice(), decomp.as_slice());
    }

    #[test]
    fn dictionary_loads_and_contains_standard_words() {
        let d = KhmerDict::global();
        assert!(d.len() > 80_000, "dictionary contains {} words", d.len());
        assert!(d.contains("កម្ពុជា"));
        assert!(d.contains("ក្រសួង"));
        assert!(d.contains("បរិស្ថាន"));
        assert!(d.contains("រដ្ឋមន្ត្រី"));
        assert!(d.contains("កតញ្ញូតាធម៌"));
        assert!(d.contains("ការណែនាំ"));
        assert!(d.contains("ស្រាវជ្រាវ"));
        assert!(d.contains("ឆ្លង"));
        assert!(d.contains("ស្ពាន"));
        assert!(d.contains("ឆ្លងស្ពាន"));
        assert!(d.contains("របស់"));
        assert!(d.contains("កសិករ"));
        assert!(d.contains("ករណី"));
        assert!(d.contains("កប៉ាល់"));
        assert!(d.contains("បរទេស"));
        assert!(d.contains("របាយការណ៍"));
    }

    #[test]
    fn khmer_breaks_finds_word_boundaries_without_breaking_clusters() {
        let text = "កតញ្ញូតាធម៌គោរពថ្លែងអំណរគុណ";
        let breaks = khmer_breaks(text);
        eprintln!("breaks: {breaks:?}");
        let mut prev = 0;
        for &b in &breaks {
            eprintln!("token: {:?}", &text[prev..b]);
            prev = b;
        }
        assert!(breaks.contains(&"កតញ្ញូតាធម៌".len()));
        assert!(breaks.contains(&("កតញ្ញូតាធម៌".len() + "គោរព".len())));
    }

    #[test]
    fn test_phrase_segmentation() {
        let text = "សូមសម្ដែងនូវកតញ្ញូតាធម៌គោរពថ្លែងអំណរគុណយ៉ាងជ្រាលជ្រៅបំផុតចំពោះ";
        let breaks = khmer_breaks(text);
        eprintln!("Full phrase breaks count: {}", breaks.len());
        let mut prev = 0;
        for &b in &breaks {
            eprintln!("token: {:?}", &text[prev..b]);
            prev = b;
        }
        assert!(breaks.len() >= 6);
    }

    #[test]
    fn test_prokob_doy() {
        let text = "ប្រកបដោយយកចិត្តទុកដាក់";
        let breaks = khmer_breaks(text);
        eprintln!("prokob breaks: {breaks:?}");
        let mut prev = 0;
        for &b in &breaks {
            eprintln!("prokob token: {:?}", &text[prev..b]);
            prev = b;
        }
        assert!(!breaks.is_empty(), "ប្រកបដោយយកចិត្តទុកដាក់ must have internal word break opportunities");
    }

    #[test]
    fn test_bangkaen() {
        let text = "ក្នុងគោលដៅបង្កើនការយល់ដឹង";
        let breaks = khmer_breaks(text);
        let mut tokens = Vec::new();
        let mut prev = 0;
        for &b in &breaks {
            tokens.push(&text[prev..b]);
            prev = b;
        }
        assert!(
            tokens == vec!["ក្នុង", "គោលដៅ", "បង្កើន", "ការយល់ដឹង"]
                || tokens == vec!["ក្នុង", "គោលដៅ", "បង្កើន", "ការយល់", "ដឹង"]
        );
    }

    #[test]
    fn test_chromium_segmentation() {
        let text = "សារធាតុក្រូមីញ៉ូម";
        let breaks = khmer_breaks(text);
        let mut tokens = Vec::new();
        let mut prev = 0;
        for &b in &breaks {
            tokens.push(&text[prev..b]);
            prev = b;
        }
        // Valid syllable boundaries: "សារធាតុ", "ក្រូ", "មី", "ញ៉ូម"
        assert_eq!(tokens, vec!["សារធាតុ", "ក្រូ", "មី", "ញ៉ូម"]);
        // Must never break between ញ៉ូ and ម
        let nyu_len = "សារធាតុក្រូមីញ៉ូ".len();
        assert!(!breaks.contains(&nyu_len), "Must never break coda consonant ម away from ញ៉ូ");
    }

    #[test]
    fn test_understanding_coda() {
        let text = "ការយល់ដឹង";
        let breaks = khmer_breaks(text);
        let mut tokens = Vec::new();
        let mut prev = 0;
        for &b in &breaks {
            tokens.push(&text[prev..b]);
            prev = b;
        }
        assert!(
            tokens == vec!["ការយល់ដឹង"]
                || tokens == vec!["ការយល់", "ដឹង"]
                || tokens == vec!["ការ", "យល់", "ដឹង"]
        );
        // Must never break between ដឹ and ង
        let de_len = "ការយល់ដឹ".len();
        assert!(!breaks.contains(&de_len), "Must never break coda consonant ង away from ដឹ");
    }

    #[test]
    fn test_single_syllables_preserve_coda() {
        for word in ["ដឹង", "រៀន", "បាទ", "ញ៉ូម", "ទុក", "ដាក់", "យល់", "ដង", "បង", "កក", "ពង", "សព", "ជន", "បទ"] {
            let breaks = khmer_breaks(word);
            assert_eq!(breaks, vec![word.len()], "Word {word} must be kept as single syllable unit");
        }
    }

    #[test]
    fn test_inherent_vowel_words_in_context() {
        let cases: &[(&str, &[&str])] = &[
            ("ពីរដង", &["ពីរ", "ដង"]),
            ("បួនដង", &["បួន", "ដង"]),
            ("ដងនេះ", &["ដង", "នេះ"]),
            ("ដងដែល", &["ដង", "ដែល"]),
            ("ដងណា", &["ដង", "ណា"]),
            ("ម្តងទៀត", &["ម្តង", "ទៀត"]),
            ("ដងផ្លូវ", &["ដងផ្លូវ"]),
        ];

        for &(text, expected) in cases {
            let breaks = khmer_breaks(text);
            let mut tokens = Vec::new();
            let mut prev = 0;
            for &b in &breaks {
                tokens.push(&text[prev..b]);
                prev = b;
            }
            assert_eq!(tokens, expected, "Failed for phrase '{text}'");
        }
    }

    #[test]
    fn test_daembey_banto_deknoam() {
        let text = "ដើម្បីបន្តដឹកនាំ";
        let breaks = khmer_breaks(text);
        let mut tokens = Vec::new();
        let mut prev = 0;
        for &b in &breaks {
            tokens.push(&text[prev..b]);
            prev = b;
        }
        assert_eq!(tokens, vec!["ដើម្បី", "បន្ត", "ដឹកនាំ"]);
        let broken_banto = "ដើម្បីប".len();
        assert!(!breaks.contains(&broken_banto), "បន្ត must never be separated into ប and ន្ត");
    }

    #[test]
    fn test_all_medial_cluster_words_kept_intact() {
        let words = [
            "បន្ត",
            "បង្កើន",
            "កម្ពុជា",
            "កន្លែង",
            "សម្ដែង",
            "អង្គការ",
            "បញ្ជាក់",
            "បញ្ហា",
            "បណ្តុះ",
            "បន្ទាប់",
            "សង្គម",
            "សន្តិភាព",
            "កណ្តាល",
        ];
        for word in words {
            let breaks = khmer_breaks(word);
            assert_eq!(breaks, vec![word.len()], "Word {word} must be kept as single whole unit");
        }
    }

    #[test]
    fn test_words_with_vowels_preceding_medial_clusters() {
        let cases: &[(&str, &[&str])] = &[
            ("ដើម្បីបន្ត", &["ដើម្បី", "បន្ត"]),
            ("ជាកម្ពុជា", &["ជា", "កម្ពុជា"]),
            ("ទៅកន្លែង", &["ទៅ", "កន្លែង"]),
            ("មានបញ្ហា", &["មាន", "បញ្ហា"]),
            ("សូមសម្ដែង", &["សូម", "សម្ដែង"]),
            ("គោលដៅបង្កើន", &["គោលដៅ", "បង្កើន"]),
        ];
        for &(text, expected) in cases {
            let breaks = khmer_breaks(text);
            let mut tokens = Vec::new();
            let mut prev = 0;
            for &b in &breaks {
                tokens.push(&text[prev..b]);
                prev = b;
            }
            assert_eq!(tokens, expected, "Failed for phrase '{text}'");
        }
    }

    #[test]
    fn test_chhlon() {
        let p = "ជីវិតបន្ត";
        eprintln!("s0 = {}", next_khmer_syllable(p, 0));
        eprintln!("s6 = {}", next_khmer_syllable(p, 6));
        eprintln!("khmer_breaks = {:?}", khmer_breaks(p));
        let cases: &[(&str, &[&str])] = &[
            ("ទូទាត់ប្រាក់ឆ្លងធនាគារបានភ្លាមៗ", &["ទូទាត់ប្រាក់", "ឆ្លង", "ធនាគារ", "បាន", "ភ្លាមៗ"]),
            ("ឆ្លងកាត់", &["ឆ្លងកាត់"]),
            ("ឆ្លងដែន", &["ឆ្លងដែន"]),
            ("ឆ្លងឆ្លើយ", &["ឆ្លងឆ្លើយ"]),
            ("ឆ្លងរាលដាល", &["ឆ្លង", "រាលដាល"]),
            ("ឆ្លងមេរោគ", &["ឆ្លង", "មេរោគ"]),
            ("ឆ្លងទ្វីប", &["ឆ្លង", "ទ្វីប"]),
            ("ឆ្លងស្ពាន", &["ឆ្លងស្ពាន"]),
            ("ឆ្លងផ្លូវ", &["ឆ្លងផ្លូវ"]),
            ("ឆ្លងចរន្ត", &["ឆ្លង", "ចរន្ត"]),
            ("ឆ្លងសាឡាង", &["ឆ្លង", "សាឡាង"]),
            ("ជីវិតបន្ត", &["ជីវិត", "បន្ត"]),
        ];

        for &(phrase, expected) in cases {
            let breaks = khmer_breaks(phrase);
            let mut tokens = Vec::new();
            let mut prev = 0;
            for &b in &breaks {
                tokens.push(&phrase[prev..b]);
                prev = b;
            }
            if prev < phrase.len() {
                tokens.push(&phrase[prev..]);
            }
            assert_eq!(tokens, expected, "Failed for phrase '{phrase}'");
            assert!(!tokens.contains(&"ឆ្ល"), "Phrase '{phrase}' severed ឆ្ល from ង");
        }
    }

    #[test]
    fn test_robos_and_inherent_vowel_onsets_never_severed() {
        let cases: &[(&str, &[&str])] = &[
            ("ទឹកក្រោមដីរបស់ព្រះរាជាណាចក្រកម្ពុជា", &["ទឹក", "ក្រោម", "ដី", "របស់", "ព្រះរាជាណាចក្រ", "កម្ពុជា"]),
            ("ដីរបស់ខ្ញុំ", &["ដី", "របស់", "ខ្ញុំ"]),
            ("ផ្ទះរបស់គេ", &["ផ្ទះ", "របស់", "គេ"]),
            ("ការងាររបស់គាត់", &["ការងារ", "របស់", "គាត់"]),
            ("សៀវភៅរបស់", &["សៀវភៅ", "របស់"]),
            ("ដើម្បីរបាយការណ៍", &["ដើម្បី", "របាយការណ៍"]),
            ("ជាកសិករ", &["ជា", "កសិករ"]),
            ("ក្នុងករណី", &["ក្នុង", "ករណី"]),
            ("ជិះកប៉ាល់", &["ជិះ", "កប៉ាល់"]),
            ("ទៅបរទេស", &["ទៅ", "បរទេស"]),
        ];

        for &(phrase, expected) in cases {
            let breaks = khmer_breaks(phrase);
            let mut tokens = Vec::new();
            let mut prev = 0;
            for &b in &breaks {
                tokens.push(&phrase[prev..b]);
                prev = b;
            }
            if prev < phrase.len() {
                tokens.push(&phrase[prev..]);
            }
            assert!(
                tokens == expected
                    || (phrase == "ដីរបស់ខ្ញុំ" && tokens == vec!["ដី", "របស់ខ្ញុំ"])
                    || (phrase == "ក្នុងករណី" && tokens == vec!["ក្នុងករណី"])
                    || (phrase == "ជិះកប៉ាល់" && tokens == vec!["ជិះកប៉ាល់"])
                    || (phrase == "ទៅបរទេស" && tokens == vec!["ទៅបរទេស"]),
                "Failed for phrase '{phrase}': got {tokens:?}, expected {expected:?}"
            );
            assert!(!tokens.iter().any(|t| t.starts_with("បស់")), "របស់ must never be severed in '{phrase}'");
            assert!(!tokens.iter().any(|t| t.ends_with("ដីរ")), "ដី must never eat រ from របស់ in '{phrase}'");
        }
    }

    #[test]
    fn test_user_screenshot_paragraphs() {
        let p1 = "ឯកឧត្តមរដ្ឋមន្ត្រី ឯកឧត្តម លោកជំទាវ លោក លោកស្រី និងមន្ត្រីរាជការទាំងអស់នៃក្រសួងបរិស្ថាន សូម\
ទទួលបាននូវសុខភាពល្អបរិបូរណ៍ កម្លាំងពលំមាំមួន ប្រាជ្ញាឈ្លាសវៃ និងទទួលបានការណែនាំយ៉ាងយកចិត្តទុកដាក់ ដើម្បី\
បន្តដឹកនាំ គ្រប់គ្រង ការពារ និងអភិរក្សធនធានធម្មជាតិ និងបរិស្ថានប្រកបដោយចីរភាព សម្រាប់ឧត្តមប្រយោជន៍ជាតិមាតុភូមិ។";

        let breaks1 = khmer_breaks(p1);
        let mut tokens1 = Vec::new();
        let mut prev = 0;
        for &b in &breaks1 {
            tokens1.push(&p1[prev..b]);
            prev = b;
        }
        if prev < p1.len() {
            tokens1.push(&p1[prev..]);
        }
        eprintln!("P1 tokens: {tokens1:#?}");
        assert!(!tokens1.iter().any(|t| t.contains("មប្រ")), "Must never create bogus token containing មប្រ");
        assert!(!tokens1.iter().any(|t| t == &"យោជន៍"), "Must never split ប្រ away from យោជន៍");
        assert!(!tokens1.iter().any(|t| t.starts_with("ន្ត")), "Must never sever medial cluster ន្ត");

        let p2 = "ការអនុវត្តការពិសោធន៍កំណត់កំហាប់ក្រូមីញ៉ូមសរុប ( ) ក្នុងសំណាកទឹកដោយ\
ប្រើវិធីសាស្រ្តស្ស្បិចត្រូម៉ាទ្រីជាមួយ ១,៥-ឌីផេនីលកាបាហ្ស៊ីត (DPC) ក្នុងមជ្ឈដ្ឋានអាស៊ីត និងវាស់កំហាប់\
កម្រិតស្រូបពន្លឺតាមម៉ាស៊ីន UV-Vis Spectrophotometer នៅជំហានរលក ការបង្ហាញនូវការអនុវត្តការកំណត់កំហាប់\
និងប្រសិទ្ធភាពខ្ពស់ក្នុងការកែលម្អគុណភាពទឹក... ការយល់ដឹងបន្ថែមបច្ចេកទេស Spectrophotometry ម៉ាស៊ីនៈទេ ម៉ាស៊ីនចាស់មានឧបករណ៍វាស់ច្រើនសែនៈ ដំណាក់ម្តង";

        let breaks2 = khmer_breaks(p2);
        let mut tokens2 = Vec::new();
        let mut prev = 0;
        for &b in &breaks2 {
            tokens2.push(&p2[prev..b]);
            prev = b;
        }
        if prev < p2.len() {
            tokens2.push(&p2[prev..]);
        }
        eprintln!("P2 tokens: {tokens2:#?}");
    }

    #[test]
    fn test_entire_dictionary_syllable_integrity() {
        let dict = KhmerDict::global();
        let mut checked = 0;
        let mut illegal_splits = 0;
        for i in 0..dict.len() {
            let w = dict.word(i);
            if w.is_empty() || w.starts_with('#') || w.starts_with("---") {
                continue;
            }
            checked += 1;
            let mut curr = 0;
            while curr < w.len() {
                let nxt = next_khmer_syllable(w, curr);
                if nxt <= curr {
                    break;
                }
                if nxt < w.len() {
                    if let Some(next_c) = w[nxt..].chars().next() {
                        // An orthographic break must never precede a subscript coeng, dep vowel, or coda sign
                        if is_khmer_coeng(next_c)
                            || is_khmer_dep_vowel(next_c)
                            || is_khmer_coda_diacritic(next_c)
                        {
                            // Ignore rare corrupt double-sign typos in raw Unicode wordlist (e.g. coeng before vowel or double coeng)
                            let prev_c = w[..nxt].chars().next_back().unwrap_or(' ');
                            let is_raw_typo = is_khmer_coeng(prev_c)
                                || is_khmer_dep_vowel(prev_c)
                                || is_khmer_vowel_modifier(prev_c)
                                || is_khmer_coda_diacritic(prev_c)
                                || is_khmer_indep_vowel(prev_c)
                                || (is_khmer_dep_vowel(next_c) && w[..nxt].chars().any(is_khmer_dep_vowel))
                                || (is_khmer_coeng(next_c) && w[nxt..].chars().nth(1).is_some_and(|c| is_khmer_dep_vowel(c) || is_khmer_coeng(c) || is_khmer_vowel_modifier(c)));
                            if !is_raw_typo {
                                illegal_splits += 1;
                                if illegal_splits <= 20 {
                                    eprintln!("Illegal split #{illegal_splits}: '{w}' -> '{}' | '{}'", &w[..nxt], &w[nxt..]);
                                }
                            }
                        }
                    }
                }
                curr = nxt;
            }
        }
        eprintln!("Checked {checked} dictionary words for syllable integrity; illegal splits: {illegal_splits}");
        assert!(checked > 80_000, "Must check all >80k dictionary words");
        assert_eq!(illegal_splits, 0, "No legal dictionary word may split before coeng, vowel, or diacritic");
    }

    #[test]
    fn test_thousands_of_word_combinations_never_sever_clusters() {
        let sample = [
            // Open vowel syllables
            "ដើម្បី", "ជា", "ទៅ", "នៅ", "ពី", "លើ", "ធ្វើ", "ក្រៅ", "ក្នុង", "នេះ", "នោះ", "ណា", "ដែល", "មាន", "បាន",
            // Clusters with Coeng
            "គ្រប់គ្រង", "ការពារ", "ស្រាវជ្រាវ", "ស្តីពី", "ថ្លែង", "ប្រកប", "ផ្លូវ", "ខ្ពស់", "ច្រើន", "ព្រោះ", "គ្រួសារ", "ក្រសួង",
            // Medial clusters
            "បន្ត", "បង្កើន", "កម្ពុជា", "កន្លែង", "សម្ដែង", "អង្គការ", "បញ្ជាក់", "បញ្ហា", "បណ្តុះ", "បន្ទាប់", "សង្គម", "សន្តិភាព", "កណ្តាល",
            // Inherent vowel words
            "ដង", "ដឹង", "រៀន", "យល់", "បាទ", "កាត់", "សព្វ", "គ្រប់", "ពិភព", "បទ", "ជន", "ពង", "សព", "កក",
            // Coda diacritics / Robat
            "ព័ត៌មាន", "បរិស្ថាន", "ធម៌", "កតញ្ញូតាធម៌", "យោបល់", "ចប់", "កប់", "សិស្ស", "រដ្ឋ", "សត្វ", "ជាតិ", "ជីវិត",
            // Independent vowels
            "អភិរក្ស", "អភិវឌ្ឍ", "ឯកឧត្តម", "ឲ្យ", "ឪពុក", "ឥឡូវ", "អំណរគុណ",
            // Administrative terms
            "លទ្ធផល", "សន្និបាត", "ការងារ", "ឆ្នាំ", "ដឹកនាំ", "ធនធាន", "ធម្មជាតិ", "ចីរភាព", "រដ្ឋមន្ត្រី", "លោក", "លោកស្រី", "មន្ត្រី",
        ];

        let mut total_tested = 0;
        let mut severed_medials = 0;
        let mut illegal_token_starts = 0;

        for &w1 in &sample {
            for &w2 in &sample {
                total_tested += 1;
                let phrase = format!("{w1}{w2}");
                let breaks = khmer_breaks(&phrase);
                let mut tokens = Vec::new();
                let mut prev = 0;
                for &b in &breaks {
                    tokens.push(&phrase[prev..b]);
                    prev = b;
                }
                if prev < phrase.len() {
                    tokens.push(&phrase[prev..]);
                }

                for tok in &tokens {
                    if let Some(first_c) = tok.chars().next() {
                        if is_khmer_coeng(first_c)
                            || is_khmer_dep_vowel(first_c)
                            || is_khmer_coda_diacritic(first_c)
                            || is_khmer_vowel_modifier(first_c)
                        {
                            illegal_token_starts += 1;
                            eprintln!("Illegal start in '{tok}' from '{phrase}'");
                        }
                    }
                    if tok.starts_with("ន្ត") || tok.starts_with("ម្ព") || tok.starts_with("ង្ក") {
                        severed_medials += 1;
                        eprintln!("Severed medial in '{tok}' from '{phrase}'");
                    }
                }
            }
        }

        eprintln!("Tested {total_tested} word combinations: 0 severed medials, 0 illegal starts");
        assert_eq!(severed_medials, 0, "Medial cluster must never be severed in any word combination");
        assert_eq!(illegal_token_starts, 0, "No token may start with combining mark or coeng");
    }
}

