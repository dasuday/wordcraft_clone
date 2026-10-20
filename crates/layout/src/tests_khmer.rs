//! Thai Distributed and Khmer Justify: lines fill the measure, space goes only between syllables
//! and words, never inside a syllable, and a zero-width space is invisible whatever the font has.
//! None of these need an installed Khmer font: a missing font draws .notdef boxes, but the
//! clusters, gaps and distribution are the same.

use super::*;
use crate::para::ClKind;
use wordcraft_doc::StoryRef;
use wordcraft_doc::khmer::Gap;
use wordcraft_doc::props::Align;

fn lay(doc: &Document) -> DocLayout {
    let mut c = LayoutCache::new();
    layout(doc, &mut c, &LayoutOptions::default())
}

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

/// A paragraph of `text` with the alignment `align` at 14 pt.
fn doc(text: &str, align: Align) -> Document {
    let mut d = Document::from_text(text);
    let p = d.para_mut(StoryRef::Body, &Path::top(0)).unwrap();
    p.props.align = Some(align);
    p.runs[0].props.size = Some(14.0);
    d
}

/// "កម្ពុជា ជាប្រទេស" style phrases: Khmer with a space between phrases, repeated until it wraps.
fn khmer_text(sep: &str) -> String {
    let phrases = ["ព្រះរាជាណាចក្រកម្ពុជា", "ជាប្រទេសមួយនៅអាស៊ីអាគ្នេយ៍", "មានប្រវត្តិសាស្ត្រដ៏យូរលង់", "និងវប្បធម៌ដ៏សម្បូរបែប"];
    (0..12).map(|i| phrases[i % phrases.len()]).collect::<Vec<_>>().join(sep)
}

/// The extra room before each cluster boundary of line `k`, over the cluster's own advance.
fn extras(pl: &ParaLayout, k: usize) -> Vec<f32> {
    let line = &pl.lines[k];
    (line.c0..line.c1.saturating_sub(1)).map(|c| line.xs[c + 1 - line.c0] - line.xs[c - line.c0] - pl.clusters[c].adv).collect()
}

#[test]
fn khmer_justify_fills_every_wrapped_line() {
    let l = lay(&doc(&khmer_text(" "), Align::KhmerJustify));
    let pl = para_layout(&l, 0);
    assert!(pl.lines.len() > 2, "wraps: {} lines", pl.lines.len());
    for (k, line) in pl.lines.iter().enumerate() {
        let end = *line.xs.last().unwrap();
        if k + 1 < pl.lines.len() {
            // A wrapped line ends at the right margin (a hanging space is not counted).
            let last_real = (line.c0..line.c1).rev().find(|c| pl.clusters[*c].kind != ClKind::Space).unwrap();
            let right = line.xs[last_real - line.c0] + pl.clusters[last_real].adv;
            assert!((right - line.right).abs() < 0.05 || (end - line.right).abs() < 0.05, "line {k}: {right} / {end} vs {}", line.right);
        } else {
            // The last line stays at the start edge, unstretched.
            assert!(extras(pl, k).iter().all(|e| e.abs() < 1e-3), "last line {k}");
        }
    }
}

#[test]
fn no_room_is_added_inside_a_syllable() {
    for align in [Align::KhmerJustify, Align::ThaiDistribute] {
        let l = lay(&doc(&khmer_text(" "), align));
        let pl = para_layout(&l, 0);
        assert_eq!(pl.gaps.len(), pl.clusters.len());
        for k in 0..pl.lines.len() - 1 {
            let line = &pl.lines[k];
            for (i, e) in extras(pl, k).into_iter().enumerate() {
                let c = line.c0 + i;
                if pl.gaps[c] == Gap::None {
                    assert!(e.abs() < 1e-3, "{align:?} line {k}: {e} inside a syllable after cluster {c}");
                }
            }
        }
    }
}

#[test]
fn khmer_justify_opens_phrase_spaces_before_syllables() {
    // Short phrases: every line holds several spaces whatever face the machine draws with.
    let l = lay(&doc(&"កម្ពុជា ខ្មែរ ភាសា ".repeat(40), Align::KhmerJustify));
    let pl = para_layout(&l, 0);
    let mut checked = 0;
    for k in 0..pl.lines.len() - 1 {
        let ex = extras(pl, k);
        let line = &pl.lines[k];
        let space = |i: usize| pl.gaps[line.c0 + i] == Gap::Space;
        let max_syl = ex.iter().enumerate().filter(|(i, _)| pl.gaps[line.c0 + i] == Gap::Syllable).map(|(_, e)| *e).fold(0.0f32, f32::max);
        let min_space = ex.iter().enumerate().filter(|(i, _)| space(*i)).map(|(_, e)| *e).fold(f32::MAX, f32::min);
        if min_space != f32::MAX {
            // Syllables only share in what the spaces (up to half an em each) could not take.
            assert!(min_space + 1e-3 >= max_syl.min(0.5 * 14.0), "line {k}: space {min_space} < syllable {max_syl}");
            checked += 1;
        }
    }
    assert!(checked > 0, "some line has a phrase space");
}

#[test]
fn thai_distributed_spreads_evenly_between_khmer_clusters() {
    let l = lay(&doc(&khmer_text(""), Align::ThaiDistribute));
    let pl = para_layout(&l, 0);
    assert!(pl.lines.len() > 1, "wraps at clusters when there are no spaces");
    let line = &pl.lines[0];
    let ex = extras(pl, 0);
    let live: Vec<f32> = ex.iter().enumerate().filter(|(i, _)| pl.gaps[line.c0 + i] != Gap::None).map(|(_, e)| *e).collect();
    assert!(!live.is_empty());
    let first = live[0];
    assert!(live.iter().all(|e| (e - first).abs() < 1e-3), "even: {live:?}");
}

#[test]
fn zero_width_space_is_an_invisible_word_boundary() {
    let text = khmer_text("\u{200B}");
    let l = lay(&doc(&text, Align::KhmerJustify));
    let pl = para_layout(&l, 0);
    let zw: Vec<usize> = pl.clusters.iter().enumerate().filter(|(_, c)| text[c.start..c.end] == *"\u{200B}").map(|(i, _)| i).collect();
    assert_eq!(zw.len(), 11);
    for &i in &zw {
        assert_eq!(pl.clusters[i].kind, ClKind::Marker, "no glyph is drawn for it");
        assert_eq!(pl.clusters[i].adv, 0.0);
        assert_eq!(pl.gaps[i], Gap::Word, "the gap after it is a word boundary");
        assert_eq!(pl.gaps[i - 1], Gap::None, "none before it");
        assert!(pl.clusters[i].break_after, "a line may end after it");
    }
}

#[test]
fn latin_text_in_a_khmer_justify_paragraph_is_stretched_at_its_spaces() {
    let text = "The quick brown fox jumps over the lazy dog ".repeat(8);
    let l = lay(&doc(&text, Align::KhmerJustify));
    let pl = para_layout(&l, 0);
    assert!(pl.lines.len() > 1);
    let line = &pl.lines[0];
    for (i, e) in extras(pl, 0).into_iter().enumerate() {
        let c = line.c0 + i;
        assert!(pl.gaps[c] == Gap::Space || e.abs() < 1e-3, "letters are not spaced: cluster {c} gets {e}");
    }
    let end = (line.c0..line.c1).rev().find(|c| pl.clusters[*c].kind != ClKind::Space).unwrap();
    assert!((line.xs[end - line.c0] + pl.clusters[end].adv - line.right).abs() < 0.05);
}

#[test]
fn plain_justify_and_left_leave_khmer_alone() {
    for align in [Align::Left, Align::Justify] {
        let l = lay(&doc(&khmer_text(""), align));
        let pl = para_layout(&l, 0);
        assert!(pl.gaps.is_empty(), "{align:?}");
    }
}
