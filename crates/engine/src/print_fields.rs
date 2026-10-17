//! Cross-reference results in a transient print copy.

use wordcraft_doc::{Document, InlineObject, Pos, StoryRef};
use wordcraft_layout::{DocLayout, LayoutCache, LayoutOptions};

use crate::cmd::references::FieldCode;

/// Bound page-number feedback separately from REF dependency depth.
const MAX_PAGE_REF_PASSES: usize = 16;
/// Bound generated text work, including cyclic fields that duplicate their own results.
const MAX_REF_WORK_BYTES: usize = 64 * 1024 * 1024;
const REF_NAME: &str = "REF";
const PAGE_REF_NAME: &str = "PAGEREF";
const MISSING_TARGET: &str = "Missing reference target.";

pub(crate) struct PrintCopy {
    pub doc: Document,
    pub layout: DocLayout,
}

#[derive(Clone, Copy)]
enum RefPass<'a> {
    Text,
    Pages(&'a DocLayout),
}

/// Evaluate a copy: exporting never changes editor caches, saved results or TOC structure.
pub(crate) fn prepare(doc: &Document, opts: &LayoutOptions) -> Result<PrintCopy, String> {
    let mut doc = doc.clone();
    let mut cache = LayoutCache::new();
    if doc.settings.protection.is_some() {
        let layout = wordcraft_layout::layout(&doc, &mut cache, opts);
        return Ok(PrintCopy { doc, layout });
    }
    let mut budget = MAX_REF_WORK_BYTES;
    settle_text(&mut doc, &mut budget)?;
    for _ in 0..MAX_PAGE_REF_PASSES {
        let layout = wordcraft_layout::layout(&doc, &mut cache, opts);
        if !refresh(&mut doc, RefPass::Pages(&layout), &mut budget)? {
            return Ok(PrintCopy { doc, layout });
        }
        settle_text(&mut doc, &mut budget)?;
    }
    Err("Print page-reference results did not converge".into())
}

fn settle_text(doc: &mut Document, budget: &mut usize) -> Result<(), String> {
    let count: usize = std::iter::once(StoryRef::Body)
        .chain(doc.parts.keys().copied().map(StoryRef::Part))
        .map(|story| {
            doc.para_paths(story)
                .iter()
                .filter_map(|path| doc.para(story, path))
                .map(|p| {
                    p.objects
                        .iter()
                        .filter(|o| {
                            matches!(o, InlineObject::Field { instr, locked: false, .. }
                if wordcraft_layout::fields::field_name(instr) == REF_NAME)
                        })
                        .count()
                })
                .sum::<usize>()
        })
        .sum();
    // An acyclic chain propagates through at most its number of REF fields. One extra
    // pass checks stability without laying out every intermediate dependency value.
    for _ in 0..=count {
        if !refresh(doc, RefPass::Text, budget)? {
            return Ok(());
        }
    }
    Err("Print REF dependencies did not converge".into())
}

fn refresh(doc: &mut Document, pass: RefPass<'_>, budget: &mut usize) -> Result<bool, String> {
    let bookmarks: std::collections::HashMap<_, _> = doc.bookmarks().into_iter().collect();
    let sections = doc.sections();
    let pages: std::collections::HashMap<_, _> = match pass {
        RefPass::Text => Default::default(),
        RefPass::Pages(layout) => {
            let mut ends = std::collections::HashMap::new();
            for path in doc.para_paths(StoryRef::Body) {
                let Some(p) = doc.para(StoryRef::Body, &path) else { continue };
                for (off, object) in p.object_offsets().into_iter().zip(&p.objects) {
                    if let InlineObject::BookmarkEnd { name } = object {
                        ends.entry(name.clone()).or_insert_with(|| Pos::new(StoryRef::Body, path.clone(), off));
                    }
                }
            }
            bookmarks
                .iter()
                .filter_map(|(name, pos)| {
                    let end = ends.get(name)?;
                    let first = layout.caret(pos)?;
                    let last = layout.caret(end)?;
                    if end < pos {
                        return None;
                    }
                    let anchor = if first.page == last.page { first } else { layout.caret(&range_page_pos(doc, pos, end)?)? };
                    let page = layout.pages.get(anchor.page)?;
                    let format = sections.get(page.section).map(|(_, s)| s.page_num_format).unwrap_or_default();
                    Some((name.clone(), (page.number, format)))
                })
                .collect()
        }
    };
    let wanted = match pass {
        RefPass::Text => REF_NAME,
        RefPass::Pages(_) => PAGE_REF_NAME,
    };
    let stories: Vec<_> = std::iter::once(StoryRef::Body).chain(doc.parts.keys().copied().map(StoryRef::Part)).collect();
    let mut changed = false;
    for story in stories {
        let locks: Vec<_> = doc.field_ranges(story).into_iter().filter(|r| r.locked).collect();
        for path in doc.para_paths(story) {
            let Some(para) = doc.para(story, &path) else { continue };
            let mut updates = Vec::new();
            for (index, (off, object)) in para.object_offsets().into_iter().zip(&para.objects).enumerate() {
                let InlineObject::Field { instr, result, locked: false, .. } = object else { continue };
                let pos = Pos::new(story, path.clone(), off);
                if locks.iter().any(|r| r.start < pos && pos < r.end) {
                    continue;
                }
                let code = FieldCode::parse(instr);
                if code.name != wanted || code.args.len() != 1 {
                    continue;
                }
                // Numbered, relative and note references need their own evaluators.
                if code.switches.iter().any(|(s, arg)| match s {
                    'h' => false,
                    '*' => !arg.as_deref().is_some_and(|a| {
                        a.eq_ignore_ascii_case("MERGEFORMAT")
                            || a.eq_ignore_ascii_case("CHARFORMAT")
                            || (code.name == PAGE_REF_NAME && wordcraft_layout::fields::format_switch(&format!(r"PAGEREF target \* {a}")).is_some())
                    }),
                    _ => true,
                }) {
                    continue;
                }
                let Some(target) = code.args.first() else { continue };
                let text = match bookmarks.get(target) {
                    None => Some(MISSING_TARGET.to_string()),
                    Some(start) if wanted == REF_NAME => inline_bookmark(doc, target, start, *budget)?,
                    Some(_) => {
                        pages.get(target).map(|(number, format)| wordcraft_layout::fields::format_switch(instr).unwrap_or(*format).format(*number))
                    }
                };
                if let Some(text) = text.filter(|text| text != result) {
                    *budget = budget.checked_sub(text.len()).ok_or("Print cross-reference text exceeds the work limit")?;
                    updates.push((index, text));
                }
            }
            if updates.is_empty() {
                continue;
            }
            let para = doc.para_mut(story, &path).map_err(|e| e.to_string())?;
            for (index, text) in updates {
                if let Some(InlineObject::Field { result, dirty, .. }) = para.objects.get_mut(index) {
                    *result = text;
                    *dirty = false;
                }
            }
            para.touch();
            changed = true;
        }
    }
    Ok(changed)
}

/// A spanning range labels its first text. Leading hard page breaks alone move that
/// anchor forward; blank paragraphs and unsupported objects retain the saved cache.
fn range_page_pos(doc: &Document, start: &Pos, end: &Pos) -> Option<Pos> {
    if start.story != end.story || end < start {
        return None;
    }
    let mut after_break = false;
    for path in doc.para_paths(start.story).into_iter().filter(|p| *p >= start.path && *p <= end.path) {
        let para = doc.para(start.story, &path)?;
        let from = if path == start.path { start.off } else { 0 };
        let to = if path == end.path { end.off } else { para.len() };
        let mut has_break = false;
        for (offset, ch) in para.text.get(from..to)?.char_indices() {
            let off = from + offset;
            if ch == wordcraft_doc::para::OBJ {
                if para.object_at(off).is_some_and(InlineObject::is_marker) {
                    continue;
                }
                return None;
            }
            if ch == '\u{c}' {
                has_break = true;
                after_break = true;
                continue;
            }
            if ch.is_control() || ch.is_whitespace() {
                return None;
            }
            return Some(if after_break { Pos::new(start.story, path, off) } else { start.clone() });
        }
        if !has_break {
            return None;
        }
    }
    None
}

/// Buffered fields cannot represent copied paragraph formatting. Preserve their complete
/// cache unless the bookmark ends in its starting paragraph; never truncate at that boundary.
fn inline_bookmark(doc: &Document, name: &str, start: &Pos, budget: usize) -> Result<Option<String>, String> {
    let Some(para) = doc.para(start.story, &start.path) else { return Ok(None) };
    let Some(end) = para
        .object_offsets()
        .into_iter()
        .find(|off| *off > start.off && matches!(para.object_at(*off), Some(InlineObject::BookmarkEnd { name: n }) if n == name))
    else {
        return Ok(None);
    };
    let bytes = para.objects.iter().try_fold(para.text.len(), |n, o| n.checked_add(o.plain_text().len()));
    if bytes.is_none_or(|n| n > budget) {
        return Err("Print cross-reference text exceeds the work limit".into());
    }
    Ok(Some(doc.copy_range(start, &Pos { off: end, ..start.clone() }).plain_text()))
}

#[cfg(test)]
mod tests {
    use wordcraft_doc::{CharProps, Document, InlineObject, Paragraph, Path, StoryRef, para_block};

    enum FieldFlags {
        Clean,
        Dirty,
        LockedDirty,
    }

    fn field(instr: &str, result: &str, flags: FieldFlags) -> InlineObject {
        InlineObject::Field {
            instr: instr.into(),
            result: result.into(),
            locked: matches!(flags, FieldFlags::LockedDirty),
            dirty: !matches!(flags, FieldFlags::Clean),
            code: false,
            form: String::new(),
        }
    }

    fn control() -> Document {
        let props = CharProps::default();
        let mut doc = Document::new();
        let mut toc = Paragraph::with_text("Cached old heading ", props.clone());
        toc.insert_object(0, InlineObject::FieldStart { instr: r#"TOC \o "1-1" \h"#.into(), locked: false, dirty: false }, &props).unwrap();
        toc.insert_object(toc.len(), field(r"PAGEREF PublicTarget \h", "99", FieldFlags::Clean), &props).unwrap();
        toc.insert_object(toc.len(), InlineObject::FieldEnd, &props).unwrap();
        doc.body = vec![para_block(toc)];
        for (instr, cache, flags) in [
            (r"REF PublicTarget \h", "Cached clean text", FieldFlags::Clean),
            (r"REF PublicTarget \h", "Cached dirty text", FieldFlags::Dirty),
            (r"REF PublicTarget \h", "Cached locked text", FieldFlags::LockedDirty),
            (r"PAGEREF PublicTarget \h", "99", FieldFlags::Clean),
        ] {
            let mut p = Paragraph::new();
            p.insert_object(0, field(instr, cache, flags), &props).unwrap();
            doc.body.push(para_block(p));
        }
        let mut target = Paragraph::with_text("Current target heading", props.clone());
        target.props.page_break_before = Some(true);
        target.insert_object(0, InlineObject::BookmarkStart { name: "PublicTarget".into() }, &props).unwrap();
        target.insert_object(target.len(), InlineObject::BookmarkEnd { name: "PublicTarget".into() }, &props).unwrap();
        doc.body.push(para_block(target));
        doc
    }

    fn shown(layout: &wordcraft_layout::DocLayout, index: usize) -> Vec<String> {
        layout
            .pages
            .iter()
            .flat_map(|p| &p.items)
            .filter_map(|i| match i {
                wordcraft_layout::Placed::Lines { story: StoryRef::Body, path, para, .. } if *path == Path::top(index) => {
                    Some(para.shown.iter().map(|(_, text, _)| text.as_str()).collect::<String>())
                }
                _ => None,
            })
            .collect()
    }

    fn pdf_text(bytes: &[u8]) -> Vec<String> {
        use hayro_interpret::font::Glyph;
        use hayro_interpret::{
            BlendMode, ClipPath, Context, Device, GlyphDrawMode, Image, InterpreterCache, InterpreterSettings, Paint, PathDrawMode, SoftMask,
            interpret_page,
        };
        use hayro_syntax::Pdf;
        struct Ex(String);
        impl Device<'_> for Ex {
            fn set_soft_mask(&mut self, _: Option<SoftMask<'_>>) {}
            fn set_blend_mode(&mut self, _: BlendMode) {}
            fn draw_path(&mut self, _: &kurbo::BezPath, _: kurbo::Affine, _: &Paint<'_>, _: &PathDrawMode) {}
            fn push_clip_path(&mut self, _: &ClipPath) {}
            fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'_>>, _: BlendMode) {}
            fn draw_glyph(&mut self, g: &Glyph<'_>, _: kurbo::Affine, _: kurbo::Affine, _: &Paint<'_>, mode: &GlyphDrawMode) {
                // Synthetic bold fills and then strokes each glyph: one character, two draws.
                if matches!(mode, GlyphDrawMode::Stroke(_)) {
                    return;
                }
                match g.as_unicode() {
                    Some(hayro_cmap::BfString::Char(c)) => self.0.push(c),
                    Some(hayro_cmap::BfString::String(s)) => self.0.push_str(&s),
                    None => self.0.push('\u{FFFD}'),
                }
            }
            fn draw_image(&mut self, _: Image<'_, '_>, _: kurbo::Affine) {}
            fn pop_clip_path(&mut self) {}
            fn pop_transparency_group(&mut self) {}
        }
        let pdf = Pdf::new(bytes.to_vec()).expect("parse");
        let cache = InterpreterCache::new();
        pdf.pages()
            .iter()
            .map(|page| {
                let mut ctx =
                    Context::new(kurbo::Affine::IDENTITY, kurbo::Rect::new(0.0, 0.0, 1.0, 1.0), &cache, pdf.xref(), InterpreterSettings::default());
                let mut ex = Ex(String::new());
                interpret_page(page, &mut ctx, &mut ex);
                ex.0
            })
            .collect()
    }

    #[test]
    fn print_refs_keep_toc_cache() {
        // Original model survives DOCX import/save; exporting changes only displayed results.
        let bytes = wordcraft_docx::write(&control()).unwrap();
        let doc = crate::io::open_bytes("control.docx", &bytes).unwrap();
        let before = serde_json::to_value(&doc).unwrap();
        let session = crate::Session::new(doc);
        let output = super::prepare(&session.doc, &Default::default()).unwrap();
        assert_eq!(output.doc.para(StoryRef::Body, &Path::top(0)).unwrap().plain_text(), "Cached old heading 2");
        let layout = output.layout;
        assert_eq!(layout.pages.len(), 2);
        assert_eq!(shown(&layout, 1), ["Current target heading"]);
        assert_eq!(shown(&layout, 2), ["Current target heading"]);
        assert_eq!(shown(&layout, 3), ["Cached locked text"]);
        assert_eq!(shown(&layout, 4), ["2"]);
        assert!(shown(&layout, 0).join("").contains("2"));
        assert!(!shown(&layout, 0).join("").contains("Current target heading"));
        assert_eq!(serde_json::to_value(&session.doc).unwrap(), before);
        let reopened = wordcraft_docx::read(&crate::io::save_bytes("again.docx", &session.doc).unwrap()).unwrap();
        assert_eq!(reopened.plain_text(StoryRef::Body), session.doc.plain_text(StoryRef::Body));
    }

    #[test]
    fn print_toc_replaces_field_cache() {
        let mut doc = control();
        let cached = "Cached nested title99";
        for object in &mut doc.para_mut(StoryRef::Body, &Path::top(0)).unwrap().objects {
            if let InlineObject::Field { result, .. } = object {
                *result = cached.into();
            }
        }
        let doc = crate::io::open_bytes("nested.docx", &wordcraft_docx::write(&doc).unwrap()).unwrap();
        let before = doc.para(StoryRef::Body, &Path::top(0)).unwrap().plain_text();
        assert_eq!(before, format!("Cached old heading {cached}"));
        let text = pdf_text(&crate::io::save_bytes("out.pdf", &doc).unwrap());
        assert!(text[0].contains("Cached old heading 2"));
        assert!(!text[0].contains("Cached nested title"));
        assert_eq!(doc.para(StoryRef::Body, &Path::top(0)).unwrap().plain_text(), before);
    }

    fn reflow_control() -> Document {
        let mut doc = control();
        doc.last_section.page_w = 220.0;
        doc.last_section.page_h = 220.0;
        doc.last_section.margin_top = 20.0;
        doc.last_section.margin_bottom = 20.0;
        doc.last_section.margin_left = 20.0;
        doc.last_section.margin_right = 20.0;
        let target = doc.para_mut(StoryRef::Body, &Path::top(5)).unwrap();
        target.props.page_break_before = Some(false);
        target.props.keep_lines = Some(true);
        target.insert_text(target.len() - wordcraft_doc::para::OBJ.len_utf8(), &" target detail".repeat(12), &CharProps::default()).unwrap();
        doc
    }

    #[test]
    fn print_crossrefs_follow_reflow() {
        let doc = reflow_control();
        let before = wordcraft_layout::layout(&doc, &mut Default::default(), &Default::default());
        let pos = doc.bookmarks().into_iter().find(|(name, _)| name == "PublicTarget").unwrap().1;
        let old_page = before.caret(&pos).unwrap().page;
        let print = super::prepare(&doc, &Default::default()).unwrap();
        let new_page = print.layout.caret(&pos).unwrap().page;
        assert!(new_page > old_page, "expanded REF must move the target");
        assert_eq!(shown(&print.layout, 4), [(new_page + 1).to_string()]);
        assert_eq!(doc.para(StoryRef::Body, &Path::top(4)).unwrap().plain_text(), "99");
    }

    #[test]
    fn print_ref_keeps_block_cache() {
        let props = CharProps::default();
        let mut doc = Document::new();
        let mut result = Paragraph::new();
        result.insert_object(0, field("REF span", "First paragraph\nSecond paragraph", FieldFlags::Clean), &props).unwrap();
        let mut first = Paragraph::with_text("First paragraph", props.clone());
        first.props.page_break_before = Some(true);
        first.insert_object(0, InlineObject::BookmarkStart { name: "span".into() }, &props).unwrap();
        let mut last = Paragraph::with_text("Second paragraph", props.clone());
        last.insert_object(last.len(), InlineObject::BookmarkEnd { name: "span".into() }, &props).unwrap();
        doc.body = vec![para_block(result), para_block(first), para_block(last)];
        let output = super::prepare(&doc, &Default::default()).unwrap();
        assert_eq!(output.doc.para(StoryRef::Body, &Path::top(0)).unwrap().plain_text(), "First paragraph\nSecond paragraph");
        let text = pdf_text(&crate::io::save_bytes("out.pdf", &doc).unwrap());
        assert!(text[0].contains("Second paragraph"));
    }

    #[derive(Clone, Copy)]
    enum RangeContext {
        Standalone,
        Toc,
    }

    fn break_range_control(context: RangeContext) -> Document {
        let props = CharProps::default();
        let mut doc = Document::new();
        doc.body.clear();
        for name in ["BreakFirst", "TextFirst"] {
            let mut p = Paragraph::with_text(&format!("{name} page "), props.clone());
            p.insert_object(p.len(), field(&format!(r"PAGEREF {name} \h"), "99", FieldFlags::Clean), &props).unwrap();
            doc.body.push(para_block(p));
        }
        if matches!(context, RangeContext::Toc) {
            for (index, name) in ["BreakFirst", "TextFirst"].into_iter().enumerate() {
                let mut p = Paragraph::with_text(&format!("Cached TOC {name} page "), props.clone());
                if index == 0 {
                    p.insert_object(0, InlineObject::FieldStart { instr: r#"TOC \o "1-3" \h"#.into(), locked: false, dirty: false }, &props).unwrap();
                }
                p.insert_object(p.len(), field(&format!(r"PAGEREF {name} \h"), "99", FieldFlags::Clean), &props).unwrap();
                if index == 1 {
                    p.insert_object(p.len(), InlineObject::FieldEnd, &props).unwrap();
                }
                doc.body.push(para_block(p));
            }
        }
        doc.body.push(para_block(Paragraph::with_text("\u{c}", props.clone())));
        let mut first = Paragraph::with_text("\u{c}", props.clone());
        first.insert_object(0, InlineObject::BookmarkStart { name: "BreakFirst".into() }, &props).unwrap();
        doc.body.push(para_block(first));
        let mut last = Paragraph::with_text("First visible range text", props.clone());
        last.insert_object(last.len(), InlineObject::BookmarkEnd { name: "BreakFirst".into() }, &props).unwrap();
        doc.body.push(para_block(last));
        doc.body.push(para_block(Paragraph::with_text("\u{c}", props.clone())));
        let mut text = Paragraph::with_text("Visible range prefix\u{c}Visible range suffix", props.clone());
        text.insert_object(0, InlineObject::BookmarkStart { name: "TextFirst".into() }, &props).unwrap();
        text.insert_object(text.len(), InlineObject::BookmarkEnd { name: "TextFirst".into() }, &props).unwrap();
        doc.body.push(para_block(text));
        doc
    }

    #[test]
    fn print_range_break_labels() {
        // Word's unchanged default-export controls show 3/4, including cached TOC entries.
        for context in [RangeContext::Standalone, RangeContext::Toc] {
            let doc = break_range_control(context);
            let before = serde_json::to_value(&doc).unwrap();
            let print = super::prepare(&doc, &Default::default()).unwrap();
            assert_eq!(shown(&print.layout, 0), ["3"]);
            assert_eq!(shown(&print.layout, 1), ["4"]);
            if matches!(context, RangeContext::Toc) {
                assert_eq!(shown(&print.layout, 2), ["3"]);
                assert_eq!(shown(&print.layout, 3), ["4"]);
            }
            let text = pdf_text(&crate::io::save_bytes("out.pdf", &doc).unwrap());
            assert_eq!(text.len(), 5);
            assert!(text[0].contains("BreakFirst page 3"));
            assert!(text[0].contains("TextFirst page 4"));
            assert!(text[2].contains("First visible range text"));
            assert!(text[3].contains("Visible range prefix"));
            assert!(text[4].contains("Visible range suffix"));
            assert_eq!(serde_json::to_value(&doc).unwrap(), before);
        }
    }

    #[test]
    fn print_page_span_keeps_cache() {
        let props = CharProps::default();
        let mut doc = Document::new();
        let mut lookup = Paragraph::new();
        lookup.insert_object(0, field("PAGEREF span", "Saved range label 99", FieldFlags::Clean), &props).unwrap();
        let mut first = Paragraph::new();
        first.props.page_break_before = Some(true);
        first.insert_object(0, InlineObject::BookmarkStart { name: "span".into() }, &props).unwrap();
        first.insert_text(first.len(), " \u{c}", &props).unwrap();
        let mut last = Paragraph::with_text("Range text", props.clone());
        last.insert_object(last.len(), InlineObject::BookmarkEnd { name: "span".into() }, &props).unwrap();
        doc.body = vec![para_block(lookup), para_block(first), para_block(last)];
        let output = super::prepare(&doc, &Default::default()).unwrap();
        assert_eq!(output.doc.para(StoryRef::Body, &Path::top(0)).unwrap().plain_text(), "Saved range label 99");
        let text = pdf_text(&crate::io::save_bytes("out.pdf", &doc).unwrap());
        assert_eq!(text[0], "Saved range label 99", "whitespace before a break retains the complete cache");
    }

    #[test]
    fn print_refs_follow_long_chain() {
        let props = CharProps::default();
        let mut doc = Document::new();
        doc.body.clear();
        for i in 0..=24 {
            let mut p = Paragraph::new();
            p.insert_object(0, InlineObject::BookmarkStart { name: format!("B{i}") }, &props).unwrap();
            if i == 24 {
                p.insert_text(p.len(), "Final value", &props).unwrap();
            } else {
                p.insert_object(p.len(), field(&format!("REF B{}", i + 1), "Cached", FieldFlags::Clean), &props).unwrap();
            }
            p.insert_object(p.len(), InlineObject::BookmarkEnd { name: format!("B{i}") }, &props).unwrap();
            doc.body.push(para_block(p));
        }
        let output = super::prepare(&doc, &Default::default()).unwrap();
        assert_eq!(output.doc.para(StoryRef::Body, &Path::top(0)).unwrap().plain_text(), "Final value");
        assert_eq!(doc.para(StoryRef::Body, &Path::top(0)).unwrap().plain_text(), "Cached");
    }

    #[test]
    fn print_refs_mark_missing() {
        let mut doc = control();
        for index in [1, 4] {
            let p = doc.para_mut(StoryRef::Body, &Path::top(index)).unwrap();
            let InlineObject::Field { instr, .. } = &mut p.objects[0] else { panic!("field") };
            *instr = if index == 1 { "REF absent" } else { "PAGEREF absent" }.into();
        }
        let output = super::prepare(&doc, &Default::default()).unwrap();
        for index in [1, 4] {
            assert_eq!(output.doc.para(StoryRef::Body, &Path::top(index)).unwrap().plain_text(), "Missing reference target.");
        }
    }

    #[test]
    fn print_refs_keep_range_locks() {
        let mut doc = control();
        let p = doc.para_mut(StoryRef::Body, &Path::top(0)).unwrap();
        let end = p.object_offsets().last().copied().unwrap();
        p.delete(end, p.len()).unwrap();
        if let InlineObject::FieldStart { locked, .. } = &mut p.objects[0] {
            *locked = true;
        }
        let p = doc.para_mut(StoryRef::Body, &Path::top(2)).unwrap();
        p.insert_object(p.len(), InlineObject::FieldEnd, &Default::default()).unwrap();
        let text = pdf_text(&crate::io::save_bytes("out.pdf", &doc).unwrap());
        assert!(text[0].contains("Cached clean text"));
        assert!(text[0].contains("Cached dirty text"));
        assert!(text[0].contains("Cached locked text"));
        let output = super::prepare(&doc, &Default::default()).unwrap();
        assert_eq!(shown(&output.layout, 4), ["2"], "unlocked field outside the range refreshes");
    }

    #[test]
    fn print_pdf_labels_and_targets() {
        use hayro_syntax::object::{Array, Dict};
        let mut doc = reflow_control();
        // Only the lookup entry links, so its annotation cannot be masked by another REF.
        for path in doc.para_paths(StoryRef::Body) {
            for object in &mut doc.para_mut(StoryRef::Body, &path).unwrap().objects {
                if let InlineObject::Field { instr, .. } = object {
                    *instr = instr.replace(r" \h", "");
                }
            }
        }
        let linked = CharProps { link: Some("#PublicTarget".into()), ..Default::default() };
        let mut p = Paragraph::with_text("Lookup marker ", linked.clone());
        p.insert_object(p.len(), field(r"PAGEREF PublicTarget \h", "99", FieldFlags::Clean), &linked).unwrap();
        doc.body[4] = para_block(p);
        let before = serde_json::to_value(&doc).unwrap();
        let pos = doc.bookmarks().into_iter().find(|(name, _)| name == "PublicTarget").unwrap().1;
        let output = super::prepare(&doc, &Default::default()).unwrap();
        let target = output.layout.caret(&pos).unwrap().page;
        assert!(
            output.layout.pages.iter().flat_map(|p| &p.items).any(|item| matches!(item,
                wordcraft_layout::Placed::Lines { path, para, .. } if *path == Path::top(4)
                    && para.styles.iter().any(|r| r.rc.link.as_deref() == Some("#PublicTarget"))
            )),
            "linked field style survived preparation: {:?}",
            doc.para(StoryRef::Body, &Path::top(4)).unwrap().runs
        );
        let session = crate::Session::new(doc.clone());
        for bytes in [crate::io::save_bytes("out.pdf", &doc).unwrap(), session.print_pdf(&serde_json::json!({})).unwrap().0] {
            let text = pdf_text(&bytes).concat();
            assert!(text.contains(&format!("Lookup marker {}", target + 1)), "updated visible page label");
            let pdf = hayro_syntax::Pdf::new(bytes).unwrap();
            let targets: Vec<_> = pdf
                .pages()
                .iter()
                .flat_map(|page| {
                    page.raw()
                        .get::<Array>(b"Annots")
                        .into_iter()
                        .flat_map(|a| a.iter::<Dict>().collect::<Vec<_>>())
                        .filter_map(|a| a.get::<Array>(b"Dest").and_then(|a| a.raw_iter().next()).and_then(|r| r.as_obj_ref()))
                        .collect::<Vec<_>>()
                })
                .collect();
            assert_eq!(targets, [pdf.pages()[target].raw().obj_id().unwrap().into()], "link follows the target moved by REF expansion");
        }
        assert_eq!(serde_json::to_value(&doc).unwrap(), before);
        assert_eq!(serde_json::to_value(&session.doc).unwrap(), before);
    }

    #[test]
    fn print_refs_section_format() {
        for (instruction, expected) in [(r#"PAGEREF "PublicTarget" \h"#, "x"), (r"PAGEREF PublicTarget \* Arabic", "10")] {
            let mut doc = control();
            doc.last_section.page_num_start = Some(9);
            doc.last_section.page_num_format = wordcraft_doc::section::NumFormat::LowerRoman;
            let p = doc.para_mut(StoryRef::Body, &Path::top(4)).unwrap();
            if let InlineObject::Field { instr, .. } = &mut p.objects[0] {
                *instr = instruction.into();
            }
            let output = super::prepare(&doc, &Default::default()).unwrap();
            assert_eq!(shown(&output.layout, 4), [expected]);
        }
    }

    #[test]
    fn print_crossrefs_respect_scope() {
        for (instruction, locked_toc, protected) in [
            (r"REF PublicTarget \n", false, false),
            (r"PAGEREF PublicTarget \p", false, false),
            (r"PAGEREF PublicTarget \* roman \* invalid", false, false),
            (r"INCLUDETEXT remote", false, false),
            (r"PAGEREF PublicTarget \h", true, false),
            (r"REF PublicTarget \h", false, true),
        ] {
            let mut doc = control();
            let toc = doc.para_mut(StoryRef::Body, &Path::top(0)).unwrap();
            for object in &mut toc.objects {
                match object {
                    InlineObject::FieldStart { locked, .. } => *locked = locked_toc,
                    InlineObject::Field { instr, .. } => *instr = instruction.into(),
                    _ => {}
                }
            }
            if protected {
                doc.settings.protection = Some("readOnly".into());
            }
            let print = super::prepare(&doc, &Default::default()).unwrap();
            assert!(print.doc.para(StoryRef::Body, &Path::top(0)).unwrap().plain_text().contains("99"), "{instruction}");
        }
    }

    #[test]
    fn print_crossrefs_reject_cycles() {
        let mut doc = control();
        let target = doc.para_mut(StoryRef::Body, &Path::top(5)).unwrap();
        target.insert_object(wordcraft_doc::para::OBJ.len_utf8(), field("REF PublicTarget", "loop", FieldFlags::Clean), &Default::default()).unwrap();
        let before = serde_json::to_value(&doc).unwrap();
        assert!(super::prepare(&doc, &Default::default()).err().unwrap().contains("did not converge"));
        for name in ["out.pdf", "out.png"] {
            assert!(crate::io::save_bytes(name, &doc).unwrap_err().contains("did not converge"));
        }
        let mut session = crate::Session::new(doc.clone());
        for command in ["file.exportPdf", "file.exportPng", "file.printPdf"] {
            let error = session.run(command, &serde_json::json!({"path": "/tmp/wordcraft-crossref-cycle"})).unwrap_err();
            assert!(error.to_string().contains("did not converge"), "{command}: {error}");
        }
        assert_eq!(serde_json::to_value(&doc).unwrap(), before);
        assert_eq!(serde_json::to_value(&session.doc).unwrap(), before);
    }
}
