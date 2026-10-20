//! Complex scripts that reorder and stack (#604): Myanmar, Khmer, Devanagari. Cluster and line
//! breaking tests don't depend on installed fonts (a missing font draws .notdef boxes, but the
//! clusters and breaks are the same); shaping tests use a system font when one is installed.

use super::*;
use wordcraft_doc::{Align, Pos, StoryRef};

fn lay(doc: &Document) -> DocLayout {
    let mut c = LayoutCache::new();
    layout(doc, &mut c, &LayoutOptions::default())
}

/// Body paragraph `i`'s layout.
fn para_layout(l: &DocLayout, i: u32) -> &ParaLayout {
    for p in &l.pages {
        for it in &p.items {
            if let Placed::Lines { story: StoryRef::Body, path, para, .. } = it
                && path.0 == vec![i]
            {
                return para;
            }
        }
    }
    panic!("paragraph {i} not laid out");
}

/// "မြန်မာစာ": the syllables မြ | န် | မာ | စာ.
const MYANMAR: &str = "\u{1019}\u{103C}\u{1014}\u{103A}\u{1019}\u{102C}\u{1005}\u{102C}";

#[test]
fn myanmar_syllables_are_single_clusters() {
    let l = lay(&Document::from_text(MYANMAR));
    let pl = para_layout(&l, 0);
    let starts: Vec<usize> = pl.clusters.iter().map(|c| c.start).collect();
    // Grapheme clusters would also start at the aa signs (bytes 15 and 21), splitting မာ and စာ.
    assert_eq!(starts, vec![0, 6, 12, 18]);
    // The caret can't land inside a syllable: every caret offset is a cluster start or the end.
    let offs = pl.line_offsets(0);
    assert!(!offs.contains(&15) && !offs.contains(&21), "{offs:?}");
    // Stacked consonants (virama) and Khmer subscripts (coeng) too.
    let l = lay(&Document::from_text("\u{1019}\u{1039}\u{1018}\u{102C} \u{1781}\u{17D2}\u{1798}\u{17C2}\u{179A}"));
    let pl = para_layout(&l, 0);
    let starts: Vec<usize> = pl.clusters.iter().map(|c| c.start).collect();
    assert_eq!(starts, vec![0, 12, 13, 25]);
}

#[test]
fn myanmar_without_spaces_wraps_between_syllables() {
    let text = MYANMAR.repeat(80);
    let l = lay(&Document::from_text(&text));
    let pl = para_layout(&l, 0);
    assert!(pl.lines.len() > 1, "a long Myanmar paragraph wraps");
    let breaks = wordcraft_doc::cluster::myanmar_breaks(&text);
    // A line may end before each syllable: before မာ and စာ, never before န် (a final).
    for c in &pl.clusters {
        assert_eq!(c.break_after, breaks.contains(&c.end), "break after {}", c.end);
    }
    assert!(pl.clusters.iter().any(|c| c.break_after && c.end == 12) && !pl.clusters.iter().any(|c| c.break_after && c.end == 6));
    for line in pl.lines.iter().skip(1) {
        let start = pl.clusters[line.c0].start;
        assert!(breaks.contains(&start), "line starts at syllable {start}");
    }
    // Lines are full: a syllable break, not an emergency break after one cluster.
    assert!(pl.lines[0].c1 - pl.lines[0].c0 > 10);
}

/// A face with Myanmar letters (an installed system font), or `None` to skip.
fn myanmar_face() -> Option<wordcraft_fonts::FaceRef> {
    wordcraft_fonts::FontDb::global().fallback_for('\u{1000}', 0).map(|f| wordcraft_fonts::FaceRef::of(&f)).filter(|f| f.has_layout_tables())
}

#[test]
fn myanmar_prevowel_is_drawn_before_its_consonant() {
    let Some(face) = myanmar_face() else {
        eprintln!("skipped: no Myanmar font with OpenType tables installed");
        return;
    };
    // ကေ: in memory consonant first, on screen the vowel sign ေ first.
    let ka = face.glyph_for('\u{1000}');
    let g = wordcraft_fonts::shape(&face, "\u{1000}\u{1031}", &[], |c| c);
    assert_eq!(g.len(), 2, "{g:?}");
    assert_eq!(g[1].gid, ka, "the consonant is drawn second: {g:?}");
    assert!(g.iter().all(|x| x.cluster == 0), "one cluster: {g:?}");
    // Itemised by script: the same syllable after Latin text in the same face shapes the same
    // (shaped as one Latin run, the vowel sign would stay after the consonant).
    if face.covers('H') {
        let mixed = wordcraft_fonts::shape(&face, "Hi \u{1000}\u{1031}", &[], |c| c);
        let tail: Vec<u32> = mixed.iter().filter(|x| x.cluster >= 3).map(|x| x.gid).collect();
        assert_eq!(tail, g.iter().map(|x| x.gid).collect::<Vec<_>>());
    }
}

#[test]
fn myanmar_in_a_latin_font_falls_back_to_a_regular_myanmar_face() {
    if myanmar_face().is_none() {
        eprintln!("skipped: no Myanmar font with OpenType tables installed");
        return;
    }
    let l = lay(&Document::from_text(MYANMAR));
    let pl = para_layout(&l, 0);
    let faces: Vec<_> = pl.clusters.iter().map(|c| pl.styles[c.style as usize].face).collect();
    assert!(faces.windows(2).all(|w| w[0].id() == w[1].id()), "one face for the word");
    let f = faces[0];
    assert!(f.has_layout_tables(), "{} can reorder Myanmar", f.family);
    assert!(!f.italic && (f.weight - 400.0).abs() <= 100.0, "a regular weight, not {} {}", f.family, f.style);
    assert!(pl.glyphs.iter().all(|g| g.gid != 0), "no missing glyphs");
    // The caret after the first syllable sits after its glyphs.
    let x6 = l.caret(&Pos::body(0, 6)).map(|c| c.x).unwrap_or(0.0);
    let x0 = l.caret(&Pos::body(0, 0)).map(|c| c.x).unwrap_or(0.0);
    assert!(x6 > x0);
}

/// The issue's run (#1549): Thai letters and digits, Latin letters, digits and the baht sign.
const THAI_RUN: &str = "[D] น้ำ ค่ำ ปั้น ฝั่ง ปู่ ๑๒๓๔๕๖๗๘๙๐ ฿1,250.50";

/// Is a font with Thai letters installed? `false` skips the test.
fn thai_font_installed() -> bool {
    wordcraft_fonts::FontDb::global().fallback_for('\u{0E19}', 0).is_some()
}

/// Body paragraph 0 of `text`, its run set in `font` at 16 pt (`None` keeps the document's
/// default font), and the face of every cluster: (cluster start, face).
fn cluster_faces(text: &str, font: Option<&str>) -> Vec<(usize, wordcraft_fonts::FaceRef)> {
    let mut d = Document::from_text(text);
    let p = d.para_mut(StoryRef::Body, &Path::top(0)).unwrap();
    p.runs[0].props.font = font.map(Into::into);
    p.runs[0].props.size = Some(16.0);
    let l = lay(&d);
    let pl = para_layout(&l, 0);
    pl.clusters.iter().map(|c| (c.start, pl.styles[c.style as usize].face)).collect()
}

#[test]
fn a_missing_thai_font_draws_the_whole_run_in_one_thai_face() {
    if !thai_font_installed() {
        eprintln!("skipped: no Thai font installed");
        return;
    }
    // Word substitutes one font (Cordia New) for a missing Thai font, so the Latin letters, the
    // digits and the baht sign keep the Thai letters' face and size. A Thai name the lists don't
    // spell out ("TH Niramit AS") is matched by its "TH " prefix and substituted the same way.
    let db = wordcraft_fonts::FontDb::global();
    for font in ["TH Sarabun New", "Angsana New", "TH Niramit AS"] {
        if db.has_family(font) {
            continue;
        }
        let faces = cluster_faces(THAI_RUN, Some(font));
        let first = faces[0].1;
        let names: Vec<&str> = faces.iter().map(|(_, f)| f.family.as_str()).collect();
        assert!(faces.iter().all(|(_, f)| f.id() == first.id()), "{font}: one face for the run, not {names:?}");
        assert!(first.covers('\u{0E19}') && first.covers('D') && first.covers('1') && first.covers('\u{0E3F}'), "{font} → {}", first.family);
    }
}

#[test]
fn a_thai_substitute_without_latin_hands_the_run_to_one_face_covering_it() {
    if !thai_font_installed() {
        eprintln!("skipped: no Thai font installed");
        return;
    }
    // When the substitute lacks the run's Thai letters (no listed Thai font is installed, so the
    // Latin fallback family stands in) or its Latin ones, the run moves to one face covering all.
    let latin = wordcraft_fonts::word::resolve("Source Sans 3", false, false).face;
    let face = crate::para::thai_run_face("TH Sarabun New", true, latin, THAI_RUN, false, false).expect("one face covers the run");
    assert_ne!(face.id(), latin.id());
    for c in THAI_RUN.chars().filter(|c| !c.is_whitespace()) {
        assert!(face.covers(c), "{} covers {c}", face.family);
    }
    // Only for a Thai family that was substituted.
    assert!(crate::para::thai_run_face("Calibri", true, latin, THAI_RUN, false, false).is_none());
    assert!(crate::para::thai_run_face("TH Sarabun New", false, latin, THAI_RUN, false, false).is_none());
    // A substitute that already covers the run keeps it.
    assert!(crate::para::thai_run_face("TH Sarabun New", true, face, THAI_RUN, false, false).is_none());
    // Text that is neither Thai nor Latin doesn't make the run move.
    assert!(crate::para::thai_run_face("TH Sarabun New", true, latin, "abc \u{2603}", false, false).is_none());
}

#[test]
fn latin_text_in_a_missing_latin_font_stays_in_its_substitute_beside_thai() {
    if !thai_font_installed() {
        eprintln!("skipped: no Thai font installed");
        return;
    }
    // Word keeps Latin text in the Latin font's substitute; only the Thai (or Myanmar) letters
    // fall back. A missing Latin family (and the default body font, Aptos, wherever it is
    // missing) must not move "Hello" into a Thai face.
    let myanmar = myanmar_face().is_some();
    for font in [Some("No Such Latin Font 1549"), None] {
        let requested = font.unwrap_or(wordcraft_doc::styles::BODY_FONT);
        let latin = wordcraft_fonts::word::resolve(requested, false, false).face;
        for text in ["Hello \u{0E2A}\u{0E27}\u{0E31}\u{0E2A}\u{0E14}\u{0E35}", "Hello \u{1019}\u{103C}\u{1014}\u{103A}\u{1019}\u{102C}"] {
            if text.contains('\u{1019}') && !myanmar {
                continue;
            }
            let faces = cluster_faces(text, font);
            let hello: Vec<_> = faces.iter().filter(|(at, _)| *at < 5).collect();
            assert_eq!(hello.len(), 5, "{faces:?}");
            assert!(
                hello.iter().all(|(_, f)| f.id() == latin.id()),
                "{requested}: Hello in {}, not {:?}",
                latin.family,
                hello.iter().map(|(_, f)| f.family.as_str()).collect::<Vec<_>>()
            );
            assert!(faces.iter().any(|(at, f)| *at >= 6 && f.id() != latin.id()), "the other script falls back");
        }
    }
}

#[test]
fn thai_in_an_installed_latin_font_keeps_the_latin_face_for_latin() {
    if !thai_font_installed() {
        eprintln!("skipped: no Thai font installed");
        return;
    }
    // The family exists, so nothing is substituted: Latin stays in it and only the Thai letters
    // fall back, character by character, as before.
    let faces = cluster_faces(THAI_RUN, Some("Source Sans 3"));
    let latin = wordcraft_fonts::word::resolve("Source Sans 3", false, false).face;
    assert!(faces.iter().any(|(_, f)| f.id() == latin.id()), "Latin clusters stay in Source Sans 3");
    assert!(faces.iter().any(|(_, f)| f.id() != latin.id() && f.covers('\u{0E19}')), "Thai clusters fall back to a Thai face");
    assert_eq!(faces[0].1.id(), latin.id(), "[D] opens the run in Source Sans 3");
}

#[test]
fn a_missing_latin_font_with_a_thai_complex_script_font_keeps_both() {
    if !thai_font_installed() {
        eprintln!("skipped: no Thai font installed");
        return;
    }
    // A run with its own complex-script font (`w:cs`) draws the Thai letters in that font's
    // substitute and the Latin letters in the Latin font's substitute, as Word does.
    let mut d = Document::from_text(THAI_RUN);
    let p = d.para_mut(StoryRef::Body, &Path::top(0)).unwrap();
    p.runs[0].props.font = Some("No Such Latin Font 1549".into());
    p.runs[0].props.font_cs = Some("TH Sarabun New".into());
    let l = lay(&d);
    let pl = para_layout(&l, 0);
    let faces: Vec<_> = pl.clusters.iter().map(|c| pl.styles[c.style as usize].face).collect();
    let latin = wordcraft_fonts::word::resolve("No Such Latin Font 1549", false, false).face;
    let thai = wordcraft_fonts::word::resolve("TH Sarabun New", false, false).face;
    assert_eq!(faces[0].id(), latin.id(), "[D] in the Latin substitute");
    assert!(faces.iter().any(|f| f.id() == thai.id()), "Thai letters in the Thai substitute {}", thai.family);
}

#[test]
fn a_missing_roman_thai_font_is_not_drawn_in_cambria() {
    // Word draws a missing font the font table classes as roman in Cambria, but a missing Thai
    // font keeps its Thai substitute: the same face whether or not the table calls it roman.
    let db = wordcraft_fonts::FontDb::global();
    if db.has_family("TH Sarabun New") {
        eprintln!("skipped: TH Sarabun New is installed");
        return;
    }
    let face = |roman: bool| {
        let mut d = Document::from_text("\u{0E19}\u{0E49}\u{0E33} Thai");
        if roman {
            d.settings.roman_fonts = vec!["TH Sarabun New".into()];
        }
        let p = d.para_mut(StoryRef::Body, &Path::top(0)).unwrap();
        p.runs[0].props.font = Some("TH Sarabun New".into());
        let l = lay(&d);
        let pl = para_layout(&l, 0);
        pl.styles[pl.clusters.last().expect("a cluster").style as usize].face
    };
    let cambria = wordcraft_fonts::word::resolve("Cambria", false, false).face;
    assert_ne!(face(true).id(), cambria.id());
    assert_eq!(face(true).id(), face(false).id());
}

#[test]
fn khmer_line_wrapping_keeps_recognized_words_intact() {
    let p_text = "ខ្ញុំបាទ/នាងខ្ញុំ សូមសម្ដែងនូវកតញ្ញូតាធម៌គោរពថ្លែងអំណរគុណយ៉ាងជ្រាលជ្រៅបំផុតចំពោះ ឯកឧត្តម អ៊ាង សុផល្លែត រដ្ឋមន្ត្រីក្រសួងបរិស្ថាន ដែលបានផ្តល់ការគាំទ្រ ការណែនាំប្រកបដោយយកចិត្តទុកដាក់ និងផ្តល់ឱកាសដ៏ មានតម្លៃដល់រូបយើងខ្ញុំក្នុងការសិក្សា ស្រាវជ្រាវ និងអនុវត្តការងារជាក់ស្តែងនៅក្នុងវិស័យបរិស្ថាន។";
    let doc = Document::from_text(p_text);
    let l = lay(&doc);
    let pl = para_layout(&l, 0);

    // Multi-line wrap
    assert!(pl.lines.len() >= 3, "Khmer paragraph wraps across lines: {}", pl.lines.len());

    // Verify each line starts and ends on valid word or cluster boundaries, never splitting words
    for (idx, line) in pl.lines.iter().enumerate() {
        let line_text: String = (line.c0..line.c1)
            .filter_map(|ci| {
                let c = pl.clusters.get(ci)?;
                p_text.get(c.start..c.end)
            })
            .collect();
        assert!(!line_text.is_empty(), "line {idx} is non-empty");

        // Recognized words must not be sliced at line break
        for word in ["កតញ្ញូតាធម៌", "រដ្ឋមន្ត្រី", "ក្រសួងបរិស្ថាន", "ការណែនាំ", "ប្រកបដោយ", "ស្រាវជ្រាវ"]
        {
            let half: String = word.chars().take(word.chars().count() / 2).collect();
            if line_text.contains(&half) {
                assert!(line_text.contains(word), "word '{word}' must remain whole on line {idx}: '{line_text}'");
            }
        }
    }
}

#[test]
fn khmer_justification_avoids_excessive_spacing() {
    use crate::para::ClKind;
    use wordcraft_doc::Align;

    let _ = wordcraft_fonts::FontDb::global();
    for (name, p_text) in [
        (
            "P1",
            "ខ្ញុំបាទ/នាងខ្ញុំ សូមសម្ដែងនូវកតញ្ញូនិងគោរពថ្លែងអំណរគុណយ៉ាងជ្រាលជ្រៅបំផុត ជូនចំពោះ ឯកឧត្តម អ៊ាង សុផល្លែត រដ្ឋមន្ត្រីក្រសួងបរិស្ថាន ដែលបានផ្ដល់ការគាំទ្រ ការណែនាំ ប្រកបដោយយកចិត្តទុកដាក់ និងផ្ដល់ឱកាសដ៏ មានតម្លៃដល់រូបយើងខ្ញុំក្នុងការសិក្សា ស្រាវជ្រាវ និង អនុវត្តការងារជាក់ស្ដែងនៅក្នុងវិស័យបរិស្ថាន។",
        ),
        (
            "P2",
            "ជាពិសេស ខ្ញុំបាទ/នាងខ្ញុំ សូមថ្លែងអំណរគុណយ៉ាងជ្រាលជ្រៅចំពោះ អគ្គនាយកដ្ឋានគាំពារបរិស្ថាន និង ឯក ឧត្តម លោកជំទាវ ថ្នាក់ដឹកនាំ មន្ត្រីជំនាញ និងមន្ត្រីបុគ្គលិកទាំងអស់នៃក្រសួងបរិស្ថាន ដែលបានចំណាយពេលវេលាដ៏មាន តម្លៃ ក្នុងការចែករំលែកចំណេះដឹង បទពិសោធន៍ ព្រមទាំងផ្តល់កិច្ចសហការយ៉ាងល្អប្រសើរ ដែលជាការរួមចំណែកយ៉ាង សំខាន់ធ្វើឱ្យការសិក្សាស្រាវជ្រាវ និងការបំពេញការងាររបស់ខ្ញុំបាទ/នាងខ្ញុំ ទទួលបានជោគជ័យយ៉ាងត្រចះត្រចង់។",
        ),
        (
            "P3",
            "ជាទីបញ្ចប់ ខ្ញុំបាទ/នាងខ្ញុំ សូមគោរពជូនពរចំពោះ ឯកឧត្តម លោកជំទាវ លោក លោកស្រី និងមន្ត្រីរាជការទាំង អស់នៃក្រសួងបរិស្ថាន សូមមានសុខភាពល្អបរិបូរណ៍ កម្លាំងមាំមួន ប្រាជ្ញាឈ្លាសវៃ និងទទួលបានជោគជ័យគ្រប់ភារកិច្ច ដើម្បីបន្តដឹកនាំការការពារ និងអភិរក្សបរិស្ថានប្រកបដោយចីរភាពសម្រាប់ប្រទេសជាតិយើង។",
        ),
    ] {
        let mut doc = Document::from_text(p_text);
        doc.format_paragraphs(&Pos::body(0, 0), &Pos::body(0, 0), &|p| p.align = Some(Align::Justify)).unwrap();
        let l = lay(&doc);
        let pl = para_layout(&l, 0);
        for (idx, line) in pl.lines.iter().enumerate() {
            // 1. Check space widths: no space should exceed 16.0 pt (before fix, spaces expanded to 102pt-210pt!)
            // Preventing "some space big some space small"
            for ci in line.c0..line.c1 {
                if pl.clusters.get(ci).is_some_and(|c| c.kind == ClKind::Space) {
                    let next_x = line.xs.get(ci + 1 - line.c0).copied().unwrap_or(0.0);
                    let curr_x = line.xs.get(ci - line.c0).copied().unwrap_or(0.0);
                    let sp_w = next_x - curr_x;
                    assert!(
                        (2.0..=10.0).contains(&sp_w),
                        "{name} Line {idx}: space at cluster {ci} has width {sp_w} pt (expected natural width within [2.0, 10.0] pt, before fix was 100pt-210pt)"
                    );
                }
            }

            if line.end == LineEnd::Wrap {
                // 2. Inter-cluster letter-by-letter spacing: micro-spacing should be subtle (<= 1.5 pt),
                // making it imperceptible to the naked eye.
                for ci in line.c0..line.c1.saturating_sub(1) {
                    let c = &pl.clusters[ci];
                    let next_c = &pl.clusters[ci + 1];
                    if c.kind != ClKind::Space && next_c.kind != ClKind::Space {
                        let curr_x = line.xs[ci - line.c0];
                        let next_x = line.xs[ci + 1 - line.c0];
                        let letter_gap = next_x - (curr_x + c.adv);
                        assert!(
                            (-0.75..=8.0).contains(&letter_gap),
                            "{name} Line {idx}: cluster {ci} letter-spacing gap is {letter_gap} pt (must be within [-0.75, 8.0] pt)"
                        );
                    }
                }

                // 3. Wrapped lines must be flush with the column width (content_w == col_w within 0.05 pt).
                // This prevents "the end of the first line got big space".
                let trimmed_c1 = (line.c0..line.c1)
                    .rposition(|k| !matches!(pl.clusters.get(k).map(|c| c.kind), Some(ClKind::Space) | Some(ClKind::Marker)))
                    .map(|pos| line.c0 + pos + 1)
                    .unwrap_or(line.c1);
                let content_w = line.xs.get(trimmed_c1 - line.c0).copied().unwrap_or(0.0) - line.xs.first().copied().unwrap_or(0.0);
                let col_w = line.right - line.left;
                assert!(
                    (content_w - col_w).abs() <= 0.05,
                    "{name} Line {idx}: content {content_w} not flush with column width {col_w} (diff: {:.4})",
                    col_w - content_w
                );
            }
        }
    }
}

#[test]
fn khmer_character_sequences_coeng_and_vowels_remain_intact() {
    // ខ្មែរ (Ka + Coeng Mo + Ae + Ro) and ក្សត្រ (Ka + Coeng Sa + Coeng Tra...)
    let text = "ខ្មែរ ក្សត្រ ក្រសួងបរិស្ថាន";
    let doc = Document::from_text(text);
    let l = lay(&doc);
    let pl = para_layout(&l, 0);

    // No caret position or cluster boundary may split between coeng (U+17D2) and subscript consonant
    for c in &pl.clusters {
        let sub = text.get(c.start..c.end).unwrap_or("");
        assert!(!sub.starts_with('\u{17D2}'), "cluster must not start with coeng: {sub}");
        assert!(!sub.ends_with('\u{17D2}'), "cluster must not end with trailing coeng: {sub}");
    }
}

#[test]
fn english_justification_retains_existing_behavior() {
    use wordcraft_doc::Align;
    let text = "Typography is the art and technique of arranging type to make written language legible, readable and appealing when displayed.";
    let mut doc = Document::from_text(text);
    doc.format_paragraphs(&Pos::body(0, 0), &Pos::body(0, 0), &|p| p.align = Some(Align::Justify)).unwrap();
    let l = lay(&doc);
    let pl = para_layout(&l, 0);

    assert!(pl.lines.len() >= 2);
    // English words are never split mid-word
    for line in &pl.lines {
        let line_text: String = (line.c0..line.c1)
            .filter_map(|ci| {
                let c = pl.clusters.get(ci)?;
                text.get(c.start..c.end)
            })
            .collect();
        for word in ["Typography", "technique", "arranging", "language", "appealing", "displayed"] {
            if line_text.contains(&word[..4]) {
                assert!(line_text.contains(word), "English word '{word}' must remain whole");
            }
        }
    }
}

#[test]
fn khmer_dynamic_layout_adapts_to_margin_changes() {
    use wordcraft_doc::Align;
    let text = "ខ្ញុំបាទ/នាងខ្ញុំ សូមសម្ដែងនូវកតញ្ញូតាធម៌គោរពថ្លែងអំណរគុណយ៉ាងជ្រាលជ្រៅបំផុតចំពោះ ឯកឧត្តម អ៊ាង សុផល្លែត រដ្ឋមន្ត្រីក្រសួងបរិស្ថាន ដែលបានផ្តល់ការគាំទ្រ";

    for margin in [36.0, 72.0, 108.0] {
        let mut doc = Document::from_text(text);
        doc.format_paragraphs(&Pos::body(0, 0), &Pos::body(0, 0), &|p| p.align = Some(Align::Justify)).unwrap();
        doc.last_section.margin_left = margin;
        doc.last_section.margin_right = margin;
        let l = lay(&doc);
        let pl = para_layout(&l, 0);
        assert!(pl.lines.len() >= 2);
        for line in &pl.lines {
            let col_w = line.right - line.left;
            let expected_w = 612.0 - 2.0 * margin;
            assert!((col_w - expected_w).abs() <= 1.0, "margin {margin}: col_w {col_w} vs expected {expected_w}");
        }
    }
}

#[test]
fn khmer_tab_indented_paragraph_spaces_remain_uniform() {
    use crate::para::ClKind;
    use wordcraft_doc::Align;

    let p_text = "\tខ្ញុំបាទ/នាងខ្ញុំ សូមសម្ដែងនូវកតញ្ញូនិងគោរពថ្លែងអំណរគុណយ៉ាងជ្រាលជ្រៅបំផុត ជូនចំពោះ ឯកឧត្តម អ៊ាង សុផល្លែត រដ្ឋមន្ត្រីក្រសួងបរិស្ថាន ដែលបានផ្ដល់ការគាំទ្រ ការណែនាំ ប្រកបដោយយកចិត្តទុកដាក់ និងផ្ដល់ឱកាសដ៏ មានតម្លៃដល់រូបយើងខ្ញុំក្នុងការសិក្សា ស្រាវជ្រាវ និង អនុវត្តការងារជាក់ស្ដែងនៅក្នុងវិស័យបរិស្ថាន។";
    let mut doc = Document::from_text(p_text);
    doc.format_paragraphs(&Pos::body(0, 0), &Pos::body(0, 0), &|p| p.align = Some(Align::Justify)).unwrap();
    let l = lay(&doc);
    let pl = para_layout(&l, 0);

    assert!(pl.lines.len() >= 2);
    // Line 0 has the tab at the start
    assert_eq!(pl.clusters[pl.lines[0].c0].kind, ClKind::Tab);

    // Collect space widths for all wrapped lines
    let mut line_space_widths: Vec<Vec<f32>> = Vec::new();
    for line in &pl.lines {
        let mut sps = Vec::new();
        for ci in line.c0..line.c1 {
            if pl.clusters.get(ci).is_some_and(|c| c.kind == ClKind::Space) {
                let next_x = line.xs.get(ci + 1 - line.c0).copied().unwrap_or(0.0);
                let curr_x = line.xs.get(ci - line.c0).copied().unwrap_or(0.0);
                sps.push(next_x - curr_x);
            }
        }
        line_space_widths.push(sps);
    }

    // Line 0 has the single space between ខ្ញុំបាទ/នាងខ្ញុំ and សូម
    let l0_spaces = &line_space_widths[0];
    assert!(!l0_spaces.is_empty(), "Line 0 must have space after នាងខ្ញុំ");
    let l0_sp = l0_spaces[0];

    // Find other line with spaces
    let other_spaces: Vec<f32> = line_space_widths.iter().skip(1).flatten().copied().collect();
    assert!(!other_spaces.is_empty(), "Paragraph must have spaces on subsequent lines");

    // All spaces must stay standard and uniform: no big spaces (<= 10.0 pt) and no collapsed spaces (>= 2.0 pt)
    assert!((2.0..=10.0).contains(&l0_sp), "Line 0 (with tab) space is {l0_sp} pt (must be standard within [2.0, 10.0] pt, not blown up to 9 pt)");

    for (si, &sp) in other_spaces.iter().enumerate() {
        assert!((2.0..=10.0).contains(&sp), "Subsequent space {si} is {sp} pt (must be standard within [2.0, 10.0] pt)");
        // The difference between line 0's space and other spaces must be small (<= 2.0 pt)
        // Preventing "some big some smaller"
        let diff = (l0_sp - sp).abs();
        assert!(diff <= 2.0, "Space difference between line 0 ({l0_sp} pt) and other space {si} ({sp} pt) is {diff} pt (expected <= 2.0 pt)");
    }

    // Line 0 and Line 1 must both be flush with column width
    for (idx, line) in pl.lines.iter().take(2).enumerate() {
        let trimmed_c1 = (line.c0..line.c1)
            .rposition(|k| !matches!(pl.clusters.get(k).map(|c| c.kind), Some(ClKind::Space) | Some(ClKind::Marker)))
            .map(|pos| line.c0 + pos + 1)
            .unwrap_or(line.c1);
        let content_w = line.xs.get(trimmed_c1 - line.c0).copied().unwrap_or(0.0) - line.xs.first().copied().unwrap_or(0.0);
        let col_w = line.right - line.left;
        assert!((content_w - col_w).abs() <= 0.05, "Line {idx} content_w {content_w} not flush with col_w {col_w}");
    }
}

#[test]
fn thai_breaks_and_justifies_without_tearing_words_or_marks() {
    use wordcraft_doc::Align;

    const THAI: &str = "ผู้ซื้อสามารถตรวจสอบประวัติเครื่องได้ทุกขั้นตอนโดยร้านจะบันทึกผลการตรวจสอบแบตเตอรี่หน้าจอคีย์บอร์ดและสถานะการล็อกไว้อย่างละเอียดก่อนส่งมอบให้ลูกค้าทุกครั้ง";
    let mut doc = Document::from_text(THAI);
    doc.format_paragraphs(&Pos::body(0, 0), &Pos::body(0, 0), &|p| p.align = Some(Align::Justify)).unwrap();
    let l = lay(&doc);
    let pl = para_layout(&l, 0);

    assert!(pl.lines.len() >= 2, "Thai paragraph must wrap into multiple lines");

    // Check that each wrapped line ends at a valid word boundary (never before a mark)
    for line in &pl.lines[..pl.lines.len() - 1] {
        let last_c = &pl.clusters[line.c1 - 1];
        let end_byte = last_c.end;
        assert!(last_c.break_after, "Line must end at a break opportunity");
        if let Some(next_char) = THAI[end_byte..].chars().next() {
            assert!(!wordcraft_proof::sea::is_sea_mark(next_char), "Line must not break before a mark: {next_char}");
        }
    }

    // Verify key words stay whole (not broken mid-word across lines)
    for word in ["ขั้นตอน", "แบตเตอรี่", "อย่าง", "บันทึก"] {
        for (at, _) in THAI.match_indices(word) {
            for line in &pl.lines[..pl.lines.len() - 1] {
                let end = pl.clusters[line.c1 - 1].end;
                assert!(end <= at || end >= at + word.len(), "word {word} split across lines at byte {end}");
            }
        }
    }
}

#[test]
fn khmer_words_stay_tight_under_justification() {
    use wordcraft_doc::Align;

    let p_text = "ការគាំទ្រ ការណែនាំ ប្រកបដោយយកចិត្តទុកដាក់ និងផ្ដល់ឱកាសដ៏ មានតម្លៃដល់រូបយើងខ្ញុំ";
    let mut doc_just = Document::from_text(p_text);
    doc_just.format_paragraphs(&Pos::body(0, 0), &Pos::body(0, 0), &|p| p.align = Some(Align::Justify)).unwrap();
    let l_just = lay(&doc_just);
    let pl_just = para_layout(&l_just, 0);

    let doc_natural = Document::from_text(p_text);
    let l_nat = lay(&doc_natural);
    let pl_nat = para_layout(&l_nat, 0);

    // In the justified line, find clusters of "ប្រកបដោយ"
    let word = "ប្រកបដោយ";
    let Some((at, _)) = p_text.match_indices(word).next() else { panic!("word not found") };
    let word_range = at..at + word.len();

    let line_just = &pl_just.lines[0];
    let line_nat = &pl_nat.lines[0];

    // Find clusters belonging to this word
    let word_cis: Vec<usize> = (line_just.c0..line_just.c1)
        .filter(|&ci| pl_just.clusters[ci].start >= word_range.start && pl_just.clusters[ci].end <= word_range.end)
        .collect();

    assert!(word_cis.len() >= 3, "Word should have several clusters");

    // Distances between adjacent clusters inside the word increase only by a subtle micro-advance (at most 0.5 pt), never a full word-space
    for w in word_cis.windows(2) {
        let (c_a, c_b) = (w[0], w[1]);
        let d_just = line_just.xs[c_b - line_just.c0] - line_just.xs[c_a - line_just.c0];
        let d_nat = line_nat.xs[c_b - line_nat.c0] - line_nat.xs[c_a - line_nat.c0];
        let diff = d_just - d_nat;
        assert!((0.0..0.6).contains(&diff), "Cluster spacing inside {word} widened too much: {diff} pt (expected < 0.6 pt)");
    }
}

#[test]
fn thai_distributed_does_not_stretch_last_line_of_paragraph() {
    // Under thaiDistribute (Align::Distribute on SEA text), wrapped lines are justified
    // but the final line of the paragraph must stay natural/un-stretched.
    let text = "ខ្ញុំបាទ/នាងខ្ញុំ សូមគោរពជូនពរចំពោះ ឯកឧត្តម លោកជំទាវ លោក លោកស្រី និង មន្ត្រី រាជការ ទាំងអស់ នៃ ក្រសួងបរិស្ថាន។ ចប់។";
    let mut doc = Document::from_text(text);
    doc.format_paragraphs(&Pos::body(0, 0), &Pos::body(0, 0), &|p| p.align = Some(Align::Distribute)).unwrap();
    let l = lay(&doc);
    let pl = para_layout(&l, 0);

    assert!(pl.lines.len() >= 2, "Should wrap into multiple lines");
    let last_line = pl.lines.last().unwrap();
    // The last line should end at natural width, not stretched to right_edge
    let last_xs = &last_line.xs;
    let natural_w = last_xs.last().copied().unwrap_or(0.0) - last_xs.first().copied().unwrap_or(0.0);
    // Line width should be far less than the full column width (~468 pt)
    assert!(natural_w < 200.0, "Last line must not be stretched to column width; got {natural_w}");
}

#[test]
fn test_khmer_os_battambang_line_break() {
    let p_text = "\tខ្ញុំបាទ/នាងខ្ញុំ សូមសម្ដែងនូវកតញ្ញូតាធម៌ដ៏ជ្រាលជ្រៅ និងការគោរពដឹងគុណយ៉ាងខ្ពង់ខ្ពស់បំផុត \
        ជូនចំពោះ ឯកឧត្តមបណ្ឌិត អ៊ាង សុផល្លែត រដ្ឋមន្ត្រីក្រសួងបរិស្ថាន ដែលជានិច្ចកាលតែងតែផ្តល់ការ\
        គាំទ្រ លើកទឹកចិត្ត និងផ្តល់ឱកាសដ៏មានតម្លៃបំផុតដល់រូបយើងខ្ញុំ ក្នុងការចុះសិក្សាស្រាវជ្រាវ និងអនុវត្ត\
        ការងារជាក់ស្តែងក្នុងវិស័យបរិស្ថាន ប្រកបដោយការយកចិត្តទុកដាក់ និងការចង្អុលបង្ហាញដ៏ខ្ពង់ខ្ពស់។";
    let mut doc = Document::from_text(p_text);
    doc.last_section.page_w = 595.28; // A4
    doc.last_section.page_h = 841.89;
    doc.last_section.margin_left = 70.87; // 2.5 cm
    doc.last_section.margin_right = 70.87; // 2.5 cm
    doc.format_range(&Pos::body(0, 0), &Pos::body(0, p_text.len()), &|c| {
        c.font = Some("Khmer OS Battambang".into());
        c.font_cs = Some("Khmer OS Battambang".into());
        c.size = Some(12.0);
        c.size_cs = Some(12.0);
    })
    .unwrap();
    doc.format_paragraphs(&Pos::body(0, 0), &Pos::body(0, 0), &|p| {
        p.align = Some(Align::Justify);
    })
    .unwrap();
    let l = lay(&doc);
    let pl = para_layout(&l, 0);
    assert_eq!(pl.lines.len(), 4, "The paragraph should have exactly 4 lines, matching MS Word");
    let line0_text: String =
        (pl.lines[0].c0..pl.lines[0].c1).map(|ci| p_text.get(pl.clusters[ci].start..pl.clusters[ci].end).unwrap_or("")).collect();
    assert!(line0_text.trim_end().ends_with("បំផុត"), "Line 0 must end with 'បំផុត'; got {:?}", line0_text);
}

#[test]
fn khmer_coda_consonants_are_never_orphaned_at_line_breaks() {
    let p_text = "ការប្រើប្រាស់សារធាតុក្រូមីញ៉ូម ក្នុងគោលដៅបង្កើនការយល់ដឹងអំពីបរិស្ថាន";
    for margin in [100.0, 150.0, 200.0, 250.0, 300.0, 350.0] {
        let mut doc = Document::from_text(p_text);
        doc.last_section.page_w = 595.28;
        doc.last_section.margin_left = 70.87;
        doc.last_section.margin_right = margin;
        let l = lay(&doc);
        let pl = para_layout(&l, 0);

        for (idx, line) in pl.lines.iter().enumerate() {
            let line_text: String =
                (line.c0..line.c1).map(|ci| p_text.get(pl.clusters[ci].start..pl.clusters[ci].end).unwrap_or("")).collect();
            let trimmed = line_text.trim();

            // Line must never start with orphaned coda consonant ម or ង
            assert!(
                !trimmed.starts_with('ម') || trimmed.starts_with("មន្ត្រី") || trimmed.starts_with("មិន"),
                "Line {idx} at margin {margin} starts with orphaned coda 'ម': '{trimmed}'"
            );
            assert!(
                !trimmed.starts_with('ង'),
                "Line {idx} at margin {margin} starts with orphaned coda 'ង': '{trimmed}'"
            );
            // Line must never end with ញ៉ូ or ដឹ
            assert!(
                !trimmed.ends_with("ញ៉ូ"),
                "Line {idx} at margin {margin} ends with broken syllable 'ញ៉ូ': '{trimmed}'"
            );
            assert!(
                !trimmed.ends_with("ដឹ"),
                "Line {idx} at margin {margin} ends with broken syllable 'ដឹ': '{trimmed}'"
            );
        }
    }
}

#[test]
fn khmer_medial_clusters_are_never_severed_at_line_breaks() {
    let p_text = "បណ្តាឆ្នាំបន្តបន្ទាប់ ដើម្បីបន្តដឹកនាំ គ្រប់គ្រង ការពារ និងអភិរក្ស ធនធានធម្មជាតិ";
    for margin in [70.0, 100.0, 130.0, 160.0, 190.0, 220.0, 250.0, 280.0, 310.0, 340.0, 370.0] {
        let mut doc = Document::from_text(p_text);
        doc.last_section.page_w = 595.28;
        doc.last_section.margin_left = 70.87;
        doc.last_section.margin_right = margin;
        let l = lay(&doc);
        let pl = para_layout(&l, 0);

        for (idx, line) in pl.lines.iter().enumerate() {
            let line_text: String =
                (line.c0..line.c1).map(|ci| p_text.get(pl.clusters[ci].start..pl.clusters[ci].end).unwrap_or("")).collect();
            let trimmed = line_text.trim();

            // Line must never start with orphaned subscript ន្ត (e.g. ន្តដឹកនាំ)
            assert!(
                !trimmed.starts_with("ន្ត"),
                "Line {idx} at margin {margin} starts with severed subscript 'ន្ត': '{trimmed}'"
            );
            // Line must never end with ដើម្បីប
            assert!(
                !trimmed.ends_with("ដើម្បីប"),
                "Line {idx} at margin {margin} ends with 'ដើម្បីប' (severing បន្ត): '{trimmed}'"
            );
            // Line must never end with broken initial consonant ប before subscript ន្ត
            assert!(
                !trimmed.ends_with("ឆ្នាំប"),
                "Line {idx} at margin {margin} ends with 'ឆ្នាំប': '{trimmed}'"
            );
        }
    }
}

#[test]
fn khmer_user_document_line_breaks_never_tear_words() {
    let p1 = "ឯកឧត្តមរដ្ឋមន្ត្រី ឯកឧត្តម លោកជំទាវ លោក លោកស្រី និងមន្ត្រីរាជការទាំងអស់នៃក្រសួងបរិស្ថាន សូម\
        ទទួលបាននូវសុខភាពល្អបរិបូរណ៍ កម្លាំងពលំមាំមួន ប្រាជ្ញាឈ្លាសវៃ និងទទួលបានការណែនាំយ៉ាងយកចិត្តទុកដាក់ ដើម្បី\
        បន្តដឹកនាំ គ្រប់គ្រង ការពារ និងអភិរក្សធនធានធម្មជាតិ និងបរិស្ថានប្រកបដោយចីរភាព សម្រាប់ឧត្តមប្រយោជន៍ជាតិមាតុភូមិ។";

    let p2 = "ការអនុវត្តការពិសោធន៍កំណត់កំហាប់ក្រូមីញ៉ូមសរុប ( ) ក្នុងសំណាកទឹកដោយ\
        ប្រើវិធីសាស្រ្តស្ស្បិចត្រូម៉ាទ្រីជាមួយ ១,៥-ឌីផេនីលកាបាហ្ស៊ីត (DPC) ក្នុងមជ្ឈដ្ឋានអាស៊ីត និងវាស់កំហាប់\
        កម្រិតស្រូបពន្លឺតាមម៉ាស៊ីន UV-Vis Spectrophotometer នៅជំហានរលក ការបង្ហាញនូវការអនុវត្តការកំណត់កំហាប់\
        និងប្រសិទ្ធភាពខ្ពស់ក្នុងការកែលម្អគុណភាពទឹក... ការយល់ដឹងបន្ថែមបច្ចេកទេស Spectrophotometry ម៉ាស៊ីនៈទេ ម៉ាស៊ីនចាស់មានឧបករណ៍វាស់ច្រើនសែនៈ ដំណាក់ម្តង";

    for margin in [70.0, 90.0, 110.0, 130.0, 150.0, 170.0, 190.0, 210.0, 230.0, 250.0, 270.0, 290.0, 310.0] {
        for (pi, p_text) in [(1, p1), (2, p2)] {
            let mut doc = Document::from_text(p_text);
            doc.last_section.page_w = 595.28;
            doc.last_section.margin_left = 70.87;
            doc.last_section.margin_right = margin;
            let l = lay(&doc);
            let pl = para_layout(&l, 0);

            for (idx, line) in pl.lines.iter().enumerate() {
                let line_text: String =
                    (line.c0..line.c1).map(|ci| p_text.get(pl.clusters[ci].start..pl.clusters[ci].end).unwrap_or("")).collect();
                let trimmed = line_text.trim();

                // P1 checks:
                assert!(
                    !trimmed.ends_with("ប្រ") || trimmed.ends_with("ប្រើ") || trimmed.ends_with("ប្រកបដោយ"),
                    "P{pi} Line {idx} at margin {margin} ends with torn 'ប្រ': '{trimmed}'"
                );
                assert!(
                    !trimmed.starts_with("យោជន៍"),
                    "P{pi} Line {idx} at margin {margin} starts with torn 'យោជន៍': '{trimmed}'"
                );
                assert!(
                    !trimmed.starts_with("ន្ត"),
                    "P{pi} Line {idx} at margin {margin} starts with severed subscript 'ន្ត': '{trimmed}'"
                );
                assert!(
                    !trimmed.ends_with("ដើម្បីប"),
                    "P{pi} Line {idx} at margin {margin} ends with 'ដើម្បីប': '{trimmed}'"
                );

                // P2 checks:
                assert!(
                    !trimmed.starts_with("ដឹងបន្ថែម"),
                    "P{pi} Line {idx} at margin {margin} splits ការយល់ from ដឹង: '{trimmed}'"
                );
            }
        }
    }
}

#[test]
fn khmer_robos_is_never_severed_at_line_breaks() {
    let p_text = "ស្របតាមច្បាប់ Beer-Lambert Law ដោយមានមេគុណកំហាប់បន្ទាត់ត្រង់ (R² > 0.999)។ វិធីសាស្ត្រ\
នេះអនុញ្ញាតឱ្យមន្ទីរពិសោធន៍អាចរកឃើញកម្រិតកំហាប់ក្រូមីញ៉ូមទាបបំផុតរហូតដល់កម្រិតមីក្រូក្រាម\
ក្នុងមួយលីត្រ (µg/L) ដែលឆ្លើយតបយ៉ាងពេញលេញទៅនឹងបទដ្ឋានគុណភាពទឹកផឹក និងទឹកក្រោមដីរបស់ព្រះរាជាណាចក្រកម្ពុជា។";

    for margin in [70.0, 80.0, 90.0, 100.0, 110.0, 120.0, 130.0, 140.0, 150.0, 160.0, 170.0, 180.0, 190.0, 200.0] {
        let mut doc = Document::from_text(p_text);
        doc.last_section.page_w = 595.28;
        doc.last_section.margin_left = 70.87;
        doc.last_section.margin_right = margin;
        let l = lay(&doc);
        let pl = para_layout(&l, 0);

        for (idx, line) in pl.lines.iter().enumerate() {
            let line_text: String =
                (line.c0..line.c1).map(|ci| p_text.get(pl.clusters[ci].start..pl.clusters[ci].end).unwrap_or("")).collect();
            let trimmed = line_text.trim();

            assert!(
                !trimmed.ends_with("ដីរ"),
                "Line {idx} at margin {margin} ends with 'ដីរ' (tearing របស់): '{trimmed}'"
            );
            assert!(
                !trimmed.starts_with("បស់"),
                "Line {idx} at margin {margin} starts with torn 'បស់': '{trimmed}'"
            );
        }
    }
}

#[test]
fn khmer_chhlon_is_never_severed_at_line_breaks() {
    let p_text = "ប្រព័ន្ធបាគងបានតភ្ជាប់គ្រឹះស្ថានធនាគារ និងហិរញ្ញវត្ថុទាំងអស់នៅក្នុងប្រទេស \
តាមរយៈការប្រើប្រាស់កូដស្តង់ដារ KHQR តែមួយ ដែលអនុញ្ញាតឱ្យប្រជាពលរដ្ឋ និងអាជីវករអាចធ្វើប្រតិបត្តិការ\
ទូទាត់ប្រាក់ឆ្លងធនាគារបានភ្លាមៗដោយមិនគិតថ្លៃសេវា។ លើសពីនេះ ប្រព័ន្ធបាគងបានពង្រីកការទូទាត់ឆ្លងដែន\
ជាមួយប្រទេសជិតខាង ដូចជា ថៃ វៀតណាម ឡាវ និងបណ្តាប្រទេសដទៃទៀត ដែលជួយសម្រួលដល់ភ្ញៀវទេសចរ\
អន្តរជាតិ និងកាត់បន្ថយថ្លៃដើមប្រតិបត្តិការពាណិជ្ជកម្មយ៉ាងច្រើន។";

    for margin in (50..220).step_by(5) {
        let margin_f = margin as f32;
        let mut doc = Document::from_text(p_text);
        doc.last_section.page_w = 595.28;
        doc.last_section.margin_left = 70.87;
        doc.last_section.margin_right = margin_f;
        let l = lay(&doc);
        let pl = para_layout(&l, 0);

        for (idx, line) in pl.lines.iter().enumerate() {
            let line_text: String =
                (line.c0..line.c1).map(|ci| p_text.get(pl.clusters[ci].start..pl.clusters[ci].end).unwrap_or("")).collect();
            let trimmed = line_text.trim();

            assert!(
                !trimmed.ends_with("ឆ្ល"),
                "Line {idx} at margin {margin_f} ends with severed 'ឆ្ល': '{trimmed}'"
            );
            assert!(
                !trimmed.starts_with("ង"),
                "Line {idx} at margin {margin_f} starts with severed coda 'ង': '{trimmed}'"
            );
            assert!(
                !trimmed.starts_with("ងធនាគារ"),
                "Line {idx} at margin {margin_f} starts with severed 'ងធនាគារ': '{trimmed}'"
            );
        }
    }
}


