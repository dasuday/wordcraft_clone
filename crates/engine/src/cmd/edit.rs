//! Undo, clipboard, find & replace, Go To, Format Painter.

use serde_json::{Value, json};
use wordcraft_doc::edit::Fragment;
use wordcraft_doc::para::InlineObject;
use wordcraft_doc::{Pos, StoryRef};
use wordcraft_layout::DocLayout;

use super::{delete_selection, find_pattern, pos_json, sel_result};
use crate::{CmdError, CmdResult, CommandSpec, Selection, Session, p};

/// Undo several steps at once: the Undo button's list (#701). `count` is validated here and
/// clamped to the undo depth, so hostile numbers never reach the stack.
fn undo_to(s: &mut Session, v: &Value) -> CmdResult {
    let n = p::u64(v, "count").ok_or_else(|| CmdError::Params("`count` (whole number, 1 or more) is required".into()))?;
    if n == 0 {
        return Err(CmdError::Params("`count` must be 1 or more".into()));
    }
    let n = usize::try_from(n).unwrap_or(usize::MAX).min(s.undo_depth());
    if s.step_lifts_protection(false) {
        return Err(CmdError::Disabled("edit.undoTo: use Stop Protection to lift the document's protection".into()));
    }
    let undone = s.undo_steps(n);
    let mut r = sel_result(s)?;
    if let Some(m) = r.as_object_mut() {
        m.insert("undone".into(), json!(undone));
        m.insert("depth".into(), json!(s.undo_depth()));
    }
    Ok(r)
}

fn has_sel(s: &Session) -> Option<&'static str> {
    if s.sel.is_collapsed() { Some("nothing is selected") } else { None }
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("edit.undo", "Undo", "Quick Access Toolbar", |s, _| {
            if s.step_lifts_protection(false) {
                return Err(CmdError::Disabled("edit.undo: use Stop Protection to lift the document's protection".into()));
            }
            s.undo();
            sel_result(s)
        })
        .key("Mod+Z")
        .pure(),
        CommandSpec::new("edit.undoTo", "Undo Actions", "Quick Access Toolbar", undo_to)
            .params(r#"{"count": n (1 = the newest step, like edit.undo; n undoes the newest n steps at once; at most the undo depth)}"#)
            .when(|s| if s.can_undo() { None } else { Some("there is nothing to undo") })
            .pure(),
        CommandSpec::new("edit.undoHistory", "Undo History", "Quick Access Toolbar", |s, _| {
            Ok(json!({"undo": s.undo_history(), "depth": s.undo_depth(), "redo": s.redo_label()}))
        })
        .pure(),
        CommandSpec::new("edit.redo", "Redo", "Quick Access Toolbar", |s, _| {
            if s.step_lifts_protection(true) {
                return Err(CmdError::Disabled("edit.redo: use Stop Protection to lift the document's protection".into()));
            }
            s.redo();
            sel_result(s)
        })
        .key("Mod+Y / Mod+Shift+Z")
        .pure(),
        CommandSpec::new("edit.cut", "Cut", "Home › Clipboard", cut).key("Mod+X").when(has_sel),
        CommandSpec::new("edit.copy", "Copy", "Home › Clipboard", copy).key("Mod+C / Mod+Insert").pure(),
        // Word's Spike (Ctrl+F3, Ctrl+Shift+F3; ⌘ on the Mac): the AutoText entry "Spike", so Quick
        // Parts and AutoText insert it without emptying it, as in Word.
        CommandSpec::new("edit.spike", "Spike", "Editing", spike).key("Mod+F3").when(has_sel),
        CommandSpec::new("edit.insertSpike", "Insert Spike", "Editing", insert_spike).key("Mod+Shift+F3").when(|s| {
            if s.math.is_some() {
                Some("leave the equation first")
            } else if s.building_blocks.contains_key(SPIKE) {
                None
            } else {
                Some("the Spike is empty")
            }
        }),
        CommandSpec::new("edit.paste", "Paste", "Home › Clipboard", paste).key("Mod+V / Shift+Insert").params(r#"{"text"?: string, "html"?: string, "rtf"?: string}"#),
        CommandSpec::new("edit.pasteText", "Paste: Keep Text Only", "Home › Clipboard › Paste", paste_text)
            .params(r#"{"text"?: string}"#)
            .key("Mod+Shift+Alt+V"),
        CommandSpec::new("edit.pasteMerge", "Paste: Merge Formatting", "Home › Clipboard › Paste", paste_merge)
            .params(r#"{"text"?: string, "html"?: string, "rtf"?: string}"#),
        CommandSpec::new("edit.clipboardPane", "Clipboard Pane", "Home › Clipboard", |s, v| {
            let on = p::bool(v, "value").unwrap_or(!s.view.clipboard_pane);
            s.view.clipboard_pane = on;
            Ok(json!({"value": on, "count": s.clip_history.len()}))
        })
        .params(r#"{"value"?: bool}"#)
        .pure(),
        CommandSpec::new("edit.clipboardItems", "Clipboard Items", "Home › Clipboard › Clipboard Pane", |s, _| Ok(s.clip_history.describe())).pure(),
        CommandSpec::new("edit.pasteClipboardItem", "Paste Clipboard Item", "Home › Clipboard › Clipboard Pane", paste_clip_item)
            .params(r#"{"index": n (0 = newest)}"#)
            .when(has_clips),
        CommandSpec::new("edit.pasteAllClipboard", "Paste All", "Home › Clipboard › Clipboard Pane", paste_all_clips).when(has_clips),
        CommandSpec::new("edit.deleteClipboardItem", "Delete Clipboard Item", "Home › Clipboard › Clipboard Pane", |s, v| {
            let i = clip_index(s, v)?;
            s.clip_history.items.remove(i);
            Ok(s.clip_history.describe())
        })
        .params(r#"{"index": n (0 = newest)}"#)
        .when(has_clips)
        .pure(),
        CommandSpec::new("edit.clearClipboard", "Clear All", "Home › Clipboard › Clipboard Pane", |s, _| {
            s.clip_history.items.clear();
            Ok(s.clip_history.describe())
        })
        .pure(),
        CommandSpec::new("edit.find", "Find", "Home › Editing", find)
            .key("Mod+F")
            .params(r#"{"text": string, "matchCase"?: bool, "wholeWord"?: bool, "matchPrefix"?: bool, "matchSuffix"?: bool, "ignorePunct"?: bool, "ignoreSpace"?: bool, "wildcards"?: bool, "special"?: bool (^p ^t ^# … codes, default true), "regex"?: bool}"#)
            .pure(),
        CommandSpec::new("edit.advancedFind", "Advanced Find", "Home › Editing › Find", advanced_find)
            .params(r#"{"text"?: string, …options of edit.find, "in"?: "main|selection", "highlight"?: bool}"#)
            .pure(),
        CommandSpec::new("edit.findNext", "Find Next", "Home › Editing › Find", |s, v| {
            read_opts(s, v);
            step(s, 1)
        })
        .key("Mod+G / F3 / Shift+F4 / Mod+PageDown")
        .params(r#"{"text"?: string, "matchCase"?: bool, "wholeWord"?: bool, "regex"?: bool}"#)
        .pure(),
        CommandSpec::new("edit.findPrevious", "Find Previous", "Home › Editing › Find", |s, v| {
            read_opts(s, v);
            step(s, -1)
        })
        // Elsewhere Ctrl+Shift+G is Word Count; Ctrl+Page Up/Down browse the find results.
        .key(super::mac_or("Mod+Shift+G / Mod+PageUp", "Mod+PageUp"))
        .params(r#"{"text"?: string, "matchCase"?: bool, "wholeWord"?: bool, "regex"?: bool}"#)
        .pure(),
        CommandSpec::new("edit.replace", "Replace", "Home › Editing", replace).params(r#"{"text": string, "with": string, …options of edit.replaceAll}"#),
        // Ctrl+H, Home › Replace: the Find and Replace dialog on its Replace tab (#712). Opening
        // it changes nothing, so it is its own command and never marks the document edited.
        CommandSpec::new("edit.findReplace", "Find and Replace", "Home › Editing", |s, v| {
            let tab = match p::str(v, "tab").unwrap_or("replace") {
                t @ ("find" | "replace" | "goto") => t,
                other => return Err(CmdError::Params(format!("`tab` is find, replace or goto, not `{other}`"))),
            };
            s.ui_requests.push(json!({"open": tab}));
            Ok(json!({"open": tab}))
        })
        // Word for Mac's own key is ⌃H.
        .key(super::mac_or("Mod+H / Ctrl+H", "Mod+H"))
        .params(r#"{"tab"?: "find|replace|goto" (default replace)}"#)
        .pure(),
        CommandSpec::new("edit.replaceAll", "Replace All", "Home › Editing › Replace", replace_all)
            .params(r#"{"text": string, "with": string (^p ^t ^& ^c …; \\1 with wildcards), …options of edit.find}"#),
        CommandSpec::new("edit.goto", "Go To", "Home › Editing › Find", goto)
            .key("Mod+Alt+G / F5")
            .params(r#"{"page"?: n, "pageBy"?: int (pages forward, negative back, from the caret's page), "bookmark"?: string (selects its text), "paragraph"?: n}; none opens Find and Replace on its Go To tab"#)
            .pure(),
        CommandSpec::new("edit.goBack", "Go Back", "Navigation", |s, _| {
            let p = s.go_back().ok_or_else(|| CmdError::Failed("no edits to go back to yet".into()))?;
            s.sel = Selection::caret(p);
            s.goal_x = None;
            sel_result(s)
        })
        .key("Shift+F5 / Mod+Alt+Z")
        .pure(),
        CommandSpec::new("edit.followLink", "Follow Link", "Navigation", follow_link)
            .params(r#"{"pos"?: Pos (beside the linked character)} | {"page": n (0-based, unlike edit.goto), "x": pt, "y": pt, "story"?: StoryRef (the story hit, default the caret's)} | {} (the selection); a link to a place in the document (a hyperlink to a bookmark, or a REF, PAGEREF or NOTEREF field with \h) puts the caret there; a web link only returns its "url" (the app opens web and email links it is asked to follow)"#)
            .pure(),
        CommandSpec::new("edit.formatPainter", "Format Painter", "Home › Clipboard", painter).params(r#"{"sticky"?: bool}"#).pure(),
        CommandSpec::new("edit.copyFormat", "Copy Formatting", "Home › Clipboard", |s, _| {
            painter_pick(s);
            sel_result(s)
        })
        .key("Mod+Shift+C")
        .pure(),
        CommandSpec::new("edit.pasteFormat", "Paste Formatting", "Home › Clipboard", paste_format).key("Mod+Shift+V"),
    ]
}

fn copy(s: &mut Session, _: &Value) -> CmdResult {
    let (a, b) = s.sel.ordered();
    if a == b {
        return Ok(json!({"text": ""}));
    }
    let f = s.doc.copy_range(&a, &b);
    s.clipboard_text = f.clipboard_text();
    s.clip_history.push(&f, &s.clipboard_text);
    s.clipboard = Some(f);
    Ok(copy_result(s))
}

/// What Copy and Cut return: the clipboard's text, and the copy as HTML (equations as MathML,
/// #816) for the host to put on the system clipboard too, so other programs keep the formatting;
/// and for a copy of one equation alone, its MathML (#1297).
pub(crate) fn copy_result(s: &mut Session) -> Value {
    s.clipboard_html = match &s.clipboard {
        Some(f) => wordcraft_formats::html::export_fragment(&f.to_document()),
        None => String::new(),
    };
    let mut r = json!({"text": s.clipboard_text});
    if !s.clipboard_html.is_empty() {
        r["html"] = json!(s.clipboard_html);
    }
    // A copy of one equation alone, as MathML for the clipboard formats equation editors read
    // ("MathML" on Windows, `public.mathml.presentation` on macOS; #1297). Word looks for them
    // before anything else when it pastes; a copy with text around the equation doesn't offer
    // them, so pasting it keeps the text.
    if let Some((linear, display)) = s.clipboard.as_ref().and_then(|f| f.sole_equation()) {
        r["mathml"] = json!(wordcraft_formats::mathml::linear_to_mathml(linear, display));
    }
    r
}

fn cut(s: &mut Session, v: &Value) -> CmdResult {
    let r = copy(s, v)?;
    // A tracked cut of a paragraph or sentence may become a move when pasted.
    if !super::moves::cut_tracked(s)? {
        delete_selection(s)?;
    }
    super::controls::restore_placeholder(s)?;
    Ok(r)
}

/// The AutoText entry the Spike collects into.
const SPIKE: &str = "Spike";

/// Ctrl+F3: cut the selection to the end of the Spike. The clipboard is left alone.
fn spike(s: &mut Session, _: &Value) -> CmdResult {
    let (a, b) = s.sel.ordered();
    // Column selection collapses the selection before the command runs.
    if a == b {
        return Err(CmdError::Disabled("edit.spike: nothing is selected".into()));
    }
    let piece = s.doc.copy_range(&a, &b);
    if piece.blocks.is_empty() {
        return Err(CmdError::Disabled("edit.spike: this selection can't be collected".into()));
    }
    delete_selection(s)?;
    super::controls::restore_placeholder(s)?;
    s.building_blocks.entry(SPIKE.to_string()).or_default().append_piece(piece);
    sel_result(s)
}

/// Ctrl+Shift+F3: insert everything the Spike collected, in order, and empty it.
fn insert_spike(s: &mut Session, _: &Value) -> CmdResult {
    let frag = s.building_blocks.get(SPIKE).cloned().ok_or_else(|| CmdError::Disabled("edit.insertSpike: the Spike is empty".into()))?;
    super::paste::insert(s, frag)?;
    s.building_blocks.remove(SPIKE);
    sel_result(s)
}

/// Paste keeps the source formatting: our own copy while the system clipboard still holds it,
/// else another program's HTML or RTF (#687). The system clipboard's text has none, so it pastes
/// as Keep Text Only does.
fn paste(s: &mut Session, v: &Value) -> CmdResult {
    match super::paste::paste_content(s, v)? {
        super::paste::Pasted::Fragment(frag) => super::paste::insert(s, frag)?,
        super::paste::Pasted::Text(text) => super::paste::insert_text(s, &text)?,
    }
    sel_result(s)
}

/// Merge Formatting: the pasted text takes the destination's formatting, keeping its emphasis.
fn paste_merge(s: &mut Session, v: &Value) -> CmdResult {
    let mut frag = super::paste::paste_fragment(s, v)?;
    super::paste::merge_formatting(s, &mut frag);
    super::paste::insert(s, frag)?;
    sel_result(s)
}

/// Most items the Clipboard pane keeps.
pub const CLIP_MAX_ITEMS: usize = 24;
/// A copy bigger than this (estimated) is not collected (it still reaches the clipboard).
const CLIP_ITEM_BYTES: usize = 4 << 20;
/// All collected items together stay under this; the oldest go first.
const CLIP_TOTAL_BYTES: usize = 24 << 20;
/// Characters of preview per item.
const CLIP_PREVIEW_CHARS: usize = 120;

/// One item collected by Copy or Cut.
#[derive(Clone, Debug)]
pub struct ClipItem {
    pub fragment: Fragment,
    /// Its plain text.
    pub text: String,
    /// Rough memory use.
    bytes: usize,
}

impl ClipItem {
    /// One line of the item's text for a list: whitespace runs become one space, objects drop out.
    pub fn preview(&self) -> String {
        let words = self.text.split(|c: char| c.is_whitespace() || c == wordcraft_doc::para::OBJ).filter(|w| !w.is_empty());
        let mut out = String::new();
        let mut n = 0;
        for w in words {
            if !out.is_empty() {
                out.push(' ');
                n += 1;
            }
            for c in w.chars() {
                if n >= CLIP_PREVIEW_CHARS {
                    out.push('…');
                    return out;
                }
                out.push(c);
                n += 1;
            }
        }
        out
    }
}

/// Items collected by Copy and Cut during the session, newest first: the Clipboard pane
/// (Home › Clipboard). At most [`CLIP_MAX_ITEMS`]; oversized copies are skipped.
#[derive(Clone, Debug, Default)]
pub struct ClipHistory {
    items: Vec<ClipItem>,
}

impl ClipHistory {
    pub fn items(&self) -> &[ClipItem] {
        &self.items
    }
    pub fn len(&self) -> usize {
        self.items.len()
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    fn push(&mut self, f: &Fragment, text: &str) {
        // Text plus per-paragraph overhead; pictures are references into the document.
        let paras = text.matches('\n').count().saturating_add(f.blocks.len()).saturating_add(f.parts.len()).saturating_add(1);
        let bytes = text.len().saturating_mul(2).saturating_add(paras.saturating_mul(256));
        if bytes > CLIP_ITEM_BYTES || f.blocks.is_empty() {
            return;
        }
        // Copying the same thing again doesn't add a second item.
        if self.items.first().is_some_and(|i| i.text == text && i.fragment == *f) {
            return;
        }
        self.items.insert(0, ClipItem { fragment: f.clone(), text: text.to_string(), bytes });
        self.items.truncate(CLIP_MAX_ITEMS);
        while self.items.len() > 1 && self.items.iter().map(|i| i.bytes).sum::<usize>() > CLIP_TOTAL_BYTES {
            self.items.pop();
        }
    }

    /// The items for agents and the pane, newest first.
    fn describe(&self) -> Value {
        Value::Array(
            self.items
                .iter()
                .enumerate()
                .map(|(i, it)| {
                    json!({
                        "index": i,
                        "preview": it.preview(),
                        "chars": it.text.chars().count(),
                        "objects": it.text.matches(wordcraft_doc::para::OBJ).count(),
                    })
                })
                .collect(),
        )
    }
}

fn has_clips(s: &Session) -> Option<&'static str> {
    if s.clip_history.is_empty() { Some("the Clipboard pane is empty") } else { None }
}

fn clip_index(s: &Session, v: &Value) -> Result<usize, CmdError> {
    let i = p::u64(v, "index").ok_or_else(|| CmdError::Params("`index` (number, 0 = newest) is required".into()))?;
    usize::try_from(i)
        .ok()
        .filter(|i| *i < s.clip_history.len())
        .ok_or_else(|| CmdError::Params(format!("no clipboard item {i} (there are {})", s.clip_history.len())))
}

/// Paste a collected item through the normal paste path. The clipboard itself stays as it was.
fn paste_clip(s: &mut Session, f: Fragment) -> CmdResult {
    let saved = s.clipboard.replace(f);
    let r = paste(s, &json!({}));
    s.clipboard = saved;
    r
}

fn paste_clip_item(s: &mut Session, v: &Value) -> CmdResult {
    let i = clip_index(s, v)?;
    let f = s.clip_history.items.get(i).map(|x| x.fragment.clone()).ok_or_else(|| CmdError::Params("no such clipboard item".into()))?;
    paste_clip(s, f)
}

/// Every collected item, in the order they were copied (one undo step).
fn paste_all_clips(s: &mut Session, _: &Value) -> CmdResult {
    let all: Vec<Fragment> = s.clip_history.items.iter().rev().map(|x| x.fragment.clone()).collect();
    let mut r = sel_result(s)?;
    for f in all {
        r = paste_clip(s, f)?;
    }
    Ok(r)
}

fn paste_text(s: &mut Session, v: &Value) -> CmdResult {
    let text = match p::str(v, "text") {
        Some(t) => t.to_string(),
        None => s.clipboard.as_ref().map(Fragment::plain_text).unwrap_or_default(),
    };
    if text.is_empty() {
        return Err(CmdError::Failed("the clipboard is empty".into()));
    }
    super::paste_text(s, &text.replace("\r\n", "\r").replace('\n', "\r"))?;
    sel_result(s)
}

/// One match: its range and, when asked for, the Replace with text filled in for it.
pub(crate) struct Found {
    pub a: Pos,
    pub b: Pos,
    pub rep: Option<String>,
}

/// A stretch of searched text: `len` bytes from `hay` are paragraph `para`'s bytes from `raw`.
/// A paragraph mark is a one-byte stretch at the paragraph's end (`mark`).
struct Seg {
    hay: usize,
    para: usize,
    raw: usize,
    len: usize,
    mark: bool,
}

/// All matches of the find state in the current story. Text marked deleted by
/// Track Changes is no longer part of the document, so the search reads each
/// paragraph without it: a match lies within one stretch of live text, and word
/// boundaries and anchors see the neighbouring live text, not the deleted text.
/// Otherwise Replace would find its own deleted text again.
pub(crate) fn search(s: &Session, story: StoryRef) -> Result<Vec<(Pos, Pos)>, CmdError> {
    Ok(search_full(s, story, false)?.into_iter().map(|f| (f.a, f.b)).collect())
}

fn find_opts(f: &crate::session::FindState) -> find_pattern::Opts {
    find_pattern::Opts {
        match_case: f.match_case,
        whole_word: f.whole_word,
        prefix: f.match_prefix,
        suffix: f.match_suffix,
        ignore_punct: f.ignore_punct,
        ignore_space: f.ignore_space,
        wildcards: f.wildcards,
        regex: f.regex,
        literal: f.literal,
    }
}

/// [`search`], with the Replace with text of each match when `with_rep` (special codes, `^&`,
/// wildcard groups). A pattern that can match a paragraph mark (`^p`) reads runs of sibling
/// paragraphs as one text with a mark between them; otherwise each paragraph is read alone.
pub(crate) fn search_full(s: &Session, story: StoryRef, with_rep: bool) -> Result<Vec<Found>, CmdError> {
    use regex_automata::{Input, meta, util::syntax};
    let f = &s.find;
    if f.query.is_empty() {
        return Ok(Vec::new());
    }
    let opts = find_opts(f);
    let c = find_pattern::compile(&f.query, &opts).map_err(CmdError::Params)?;
    let re = meta::Regex::builder()
        .syntax(syntax::Config::new().case_insensitive(c.case_insensitive))
        .configure(meta::Config::new().nfa_size_limit(Some(1 << 20)))
        .build(&c.pattern)
        .map_err(|e| CmdError::Params(format!("bad pattern: {e}")))?;
    let pieces = with_rep.then(|| find_pattern::parse_replace(&f.replace, &opts));
    let paths = s.doc.para_paths(story);
    // Paragraphs read as one text: consecutive siblings when the pattern spans paragraphs.
    let mut chunks: Vec<std::ops::Range<usize>> = Vec::new();
    for i in 0..paths.len() {
        let joins =
            c.spans_paras && i > 0 && paths.get(i - 1).zip(paths.get(i)).is_some_and(|(x, y)| x.parent() == y.parent() && x.last() + 1 == y.last());
        match chunks.last_mut() {
            Some(r) if joins => r.end = i + 1,
            _ => chunks.push(i..i + 1),
        }
    }
    let mut out = Vec::new();
    let mut caps = re.create_captures();
    for chunk in chunks {
        let mut hay = String::new();
        let mut segs: Vec<Seg> = Vec::new();
        // Where the live text breaks (tracked-deleted text, or a paragraph edge when not spanning).
        let mut cuts: Vec<usize> = Vec::new();
        let last = chunk.end.saturating_sub(1);
        for i in chunk {
            let Some(p) = paths.get(i).and_then(|path| s.doc.para(story, path)) else {
                cuts.push(hay.len());
                continue;
            };
            let mut expect = 0;
            for (r, ch) in p.run_ranges() {
                let Some(t) = p.text.get(r.clone()).filter(|_| ch.del.is_none()) else { continue };
                if t.is_empty() {
                    continue;
                }
                if r.start != expect {
                    cuts.push(hay.len());
                }
                match segs.last_mut() {
                    Some(sg) if !sg.mark && sg.para == i && sg.raw + sg.len == r.start => sg.len += t.len(),
                    _ => segs.push(Seg { hay: hay.len(), para: i, raw: r.start, len: t.len(), mark: false }),
                }
                hay.push_str(t);
                expect = r.end;
            }
            if i < last {
                if expect != p.text.len() {
                    cuts.push(hay.len());
                }
                segs.push(Seg { hay: hay.len(), para: i, raw: p.text.len(), len: 1, mark: true });
                hay.push(if p.section.is_some() { find_pattern::SECT_MARK } else { find_pattern::PARA_MARK });
            }
        }
        let pos_of = |x: usize, end: bool| -> Option<Pos> {
            let k = segs.partition_point(|sg| if end { sg.hay + sg.len < x } else { sg.hay + sg.len <= x });
            let sg = segs.get(k)?;
            if (end && sg.hay >= x) || (!end && sg.hay > x) {
                return None;
            }
            if sg.mark && x > sg.hay {
                let path = paths.get(sg.para)?;
                return Some(Pos { story, path: path.with_last(path.last() + 1), off: 0 });
            }
            Some(Pos { story, path: paths.get(sg.para)?.clone(), off: sg.raw + (x - sg.hay) })
        };
        cuts.push(hay.len());
        let mut from = 0;
        for cut in cuts {
            if cut <= from {
                continue;
            }
            let mut input = Input::new(&hay).range(from..cut);
            from = cut;
            loop {
                re.search_captures(&input, &mut caps);
                let Some(m) = caps.get_match() else { break };
                // Step past an empty match so the search moves on.
                let next = if m.is_empty() {
                    hay.get(m.end()..).and_then(|t| t.chars().next()).map_or(m.end() + 1, |c| m.end() + c.len_utf8())
                } else {
                    m.end()
                };
                if next > input.end() {
                    break;
                }
                input.set_start(next);
                if m.is_empty() {
                    continue;
                }
                let (Some(a), Some(b)) = (pos_of(m.start(), false), pos_of(m.end(), true)) else { continue };
                let rep = pieces.as_ref().map(|pc| {
                    let group = |n: usize| caps.get_group(n).and_then(|g| hay.get(g.range())).map(str::to_string);
                    find_pattern::expand(pc, hay.get(m.range()).unwrap_or(""), &group, &s.clipboard_text)
                });
                out.push(Found { a, b, rep });
                if out.len() >= 100_000 {
                    return Ok(out);
                }
            }
        }
    }
    Ok(out)
}

fn read_opts(s: &mut Session, v: &Value) {
    // A new search changes what Reading Highlight marks.
    s.find.highlights = None;
    if let Some(t) = p::str(v, "text") {
        s.find.query = t.to_string();
    }
    if let Some(b) = p::bool(v, "matchCase") {
        s.find.match_case = b;
    }
    if let Some(b) = p::bool(v, "wholeWord") {
        s.find.whole_word = b;
    }
    if let Some(b) = p::bool(v, "regex") {
        s.find.regex = b;
    }
    if let Some(b) = p::bool(v, "wildcards") {
        s.find.wildcards = b;
    }
    if let Some(b) = p::bool(v, "special") {
        s.find.literal = !b;
    }
    if let Some(b) = p::bool(v, "matchPrefix") {
        s.find.match_prefix = b;
    }
    if let Some(b) = p::bool(v, "matchSuffix") {
        s.find.match_suffix = b;
    }
    if let Some(b) = p::bool(v, "ignorePunct") {
        s.find.ignore_punct = b;
    }
    if let Some(b) = p::bool(v, "ignoreSpace") {
        s.find.ignore_space = b;
    }
    if let Some(t) = p::str(v, "with") {
        s.find.replace = t.to_string();
    }
}

fn find(s: &mut Session, v: &Value) -> CmdResult {
    read_opts(s, v);
    if p::str(v, "text").is_none() {
        // Word's Ctrl+F: the Navigation pane with its search box ready to type in (#712); the
        // Find and Replace dialog is Advanced Find.
        s.view.nav_pane = true;
        s.ui_requests.push(json!({"focus": "navSearch"}));
        return Ok(json!({"count": s.find.results.len()}));
    }
    let story = s.sel.focus.story;
    s.find.results = search(s, story)?;
    // Select the first match after the caret.
    let caret = s.sel.focus.clone();
    let idx = s.find.results.iter().position(|(a, _)| *a >= caret).unwrap_or(0);
    s.find.current = idx;
    if let Some((a, b)) = s.find.results.get(idx).cloned() {
        s.sel = Selection { anchor: a, focus: b };
    }
    Ok(json!({
        "count": s.find.results.len(),
        "matches": s.find.results.iter().take(200).map(|(a, b)| json!({"start": pos_json(a), "end": pos_json(b)})).collect::<Vec<_>>(),
    }))
}

/// Find in the classic dialog: every match in the body or the selection, optionally highlighted while you read.
fn advanced_find(s: &mut Session, v: &Value) -> CmdResult {
    if p::str(v, "text").is_none() && v.get("highlight").is_none() {
        s.ui_requests.push(json!({"open": "find"}));
        return Ok(json!({"count": s.find.results.len()}));
    }
    read_opts(s, v);
    let within = match p::str(v, "in").unwrap_or("main") {
        "main" => None,
        "selection" if !s.sel.is_collapsed() => Some(s.sel.ordered()),
        "selection" => return Err(CmdError::Params("`in: selection` needs a selection".into())),
        other => return Err(CmdError::Params(format!("`in` is main or selection, not `{other}`"))),
    };
    let story = within.as_ref().map(|(a, _)| a.story).unwrap_or(StoryRef::Body);
    let mut found = search(s, story)?;
    if let Some((a, b)) = &within {
        found.retain(|(x, y)| x >= a && y <= b);
    }
    if let Some(h) = p::bool(v, "highlight") {
        s.find.highlight = h;
        s.find.highlights = None;
    }
    let n = found.len();
    let matches: Vec<Value> = found.iter().take(200).map(|(a, b)| json!({"start": pos_json(a), "end": pos_json(b)})).collect();
    s.find.current = 0;
    s.find.results = found;
    Ok(json!({"count": n, "matches": matches, "highlight": s.find.highlight}))
}

fn step(s: &mut Session, dir: i64) -> CmdResult {
    let story = s.sel.focus.story;
    s.find.results = search(s, story)?;
    let n = s.find.results.len();
    if n == 0 {
        return Err(CmdError::Failed("no matches".into()));
    }
    let caret = if dir > 0 { s.sel.ordered().1 } else { s.sel.ordered().0 };
    let idx = if dir > 0 {
        s.find.results.iter().position(|(a, _)| *a >= caret).unwrap_or(0)
    } else {
        s.find.results.iter().rposition(|(_, b)| *b <= caret).unwrap_or(n - 1)
    };
    s.find.current = idx;
    if let Some((a, b)) = s.find.results.get(idx).cloned() {
        s.sel = Selection { anchor: a, focus: b };
    }
    Ok(json!({"index": idx, "count": n}))
}

fn replace(s: &mut Session, v: &Value) -> CmdResult {
    read_opts(s, v);
    let story = s.sel.focus.story;
    let results = search_full(s, story, true)?;
    let (a, b) = s.sel.ordered();
    // Replace the current match if selected, then move to the next.
    if let Some(f) = results.into_iter().find(|f| f.a == a && f.b == b) {
        super::paste_text(s, &f.rep.unwrap_or_default())?;
    }
    let _ = step(s, 1);
    Ok(json!({"remaining": search(s, story)?.len()}))
}

/// Every match replaced, last first so earlier positions stay valid (one undo step). A `\r` in
/// the replacement (`^p`) splits the paragraph.
fn replace_all(s: &mut Session, v: &Value) -> CmdResult {
    read_opts(s, v);
    let story = s.sel.focus.story;
    let results = search_full(s, story, true)?;
    let n = results.len();
    super::controls::check_deletions(s, story, results.iter().map(|f| (&f.a, &f.b)))?;
    for Found { a, b, rep } in results.into_iter().rev() {
        let with = rep.unwrap_or_default();
        if s.doc.settings.track_changes {
            // Same path as a single replace, so the change is a reviewable revision.
            s.sel = Selection { anchor: a, focus: b };
            super::paste_text(s, &with)?;
            continue;
        }
        // Replacing a symbol gives the new text the symbol's formatting, not its own font.
        let props = s.doc.para_at(&a).map(|p| p.props_of_char(a.off).clone().without_symbol()).unwrap_or_default();
        let mut at = s.doc.delete_range(&a, &b)?;
        for (k, part) in with.split(['\r', '\u{2029}']).enumerate() {
            if k > 0 {
                at = s.doc.split_paragraph(&at)?;
            }
            if !part.is_empty() {
                at = s.doc.insert_text(&at, part, &props)?;
            }
        }
    }
    s.status = format!("All done. We made {n} replacements.");
    Ok(json!({"replaced": n}))
}

fn goto(s: &mut Session, v: &Value) -> CmdResult {
    // Relative pages, Go To's "+2" / "-1": from the page the caret is on.
    let by = v.get("pageBy").and_then(Value::as_i64);
    let rel = by.map(|by| {
        let here = s.layout().caret_on(&s.sel.focus, s.page_hint).map_or(0, |c| c.page) as i64;
        (here + 1).saturating_add(by).max(1) as u64
    });
    if let Some(pg) = rel.or_else(|| p::u64(v, "page")) {
        let l = s.layout();
        let idx = (pg.max(1) as usize - 1).min(l.pages.len().saturating_sub(1));
        let target = l.pages.get(idx).and_then(|page| {
            page.items.iter().find_map(|it| match it {
                wordcraft_layout::Placed::Lines { story, path, para, l0, .. } if *story == StoryRef::Body => {
                    para.lines.get(*l0).map(|ln| Pos { story: *story, path: path.clone(), off: ln.start })
                }
                _ => None,
            })
        });
        if let Some(t) = target {
            s.sel = Selection::caret(t);
            s.page_hint = idx;
        }
        s.ui_requests.push(json!({"scrollTo": "caret"}));
        return sel_result(s);
    }
    if let Some(name) = p::str(v, "bookmark") {
        // Word's built-in bookmark: the top of the document.
        if name == "_top" {
            s.sel = Selection::caret(Pos::body(0, 0));
            return sel_result(s);
        }
        // As in Word, the bookmark's text is selected (a caret for an empty bookmark).
        let (from, to) = super::insert::bookmark_range(&s.doc, name).ok_or_else(|| CmdError::Failed(format!("no bookmark `{name}`")))?;
        s.sel = Selection { anchor: from, focus: to };
        return sel_result(s);
    }
    if let Some(n) = p::u64(v, "paragraph") {
        let paths = s.doc.para_paths(StoryRef::Body);
        if let Some(path) = paths.get((n.max(1) - 1) as usize) {
            s.sel = Selection::caret(Pos { story: StoryRef::Body, path: path.clone(), off: 0 });
        }
        return sel_result(s);
    }
    s.ui_requests.push(json!({"open": "goto"}));
    sel_result(s)
}

/// A link a Ctrl/⌘+click follows: a hyperlink, or a cross-reference field with `\h`.
#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    /// `#bookmark` for a place in the document, else the hyperlink's address.
    pub target: String,
    /// Made by a REF, PAGEREF or NOTEREF field's `\h` switch, not by a hyperlink.
    pub field: bool,
    /// Where the linked character starts.
    pub at: Pos,
}

/// The link on the character starting at `at`: its hyperlink, else the bookmark of a
/// cross-reference field with `\h` that shows its result (not its code; `field_codes` is
/// View › Field Codes).
pub fn link_at(doc: &wordcraft_doc::Document, field_codes: bool, at: &Pos) -> Option<Link> {
    let para = doc.para_at(at)?;
    if at.off >= para.len() {
        return None;
    }
    let link = |target: String, field: bool| Some(Link { target, field, at: at.clone() });
    if let Some(url) = para.props_of_char(at.off).link.clone().filter(|l| !l.is_empty()) {
        return link(url, false);
    }
    match para.object_at(at.off) {
        Some(InlineObject::Field { instr, code, .. }) if *code == field_codes => link(wordcraft_layout::fields::reference_link(instr)?, true),
        _ => None,
    }
}

/// The link of the character after the caret position `at`, else of the one before it.
fn link_beside(doc: &wordcraft_doc::Document, field_codes: bool, at: &Pos) -> Option<Link> {
    let para = doc.para_at(at)?;
    let before = Pos { off: para.prev_boundary(at.off), ..at.clone() };
    link_at(doc, field_codes, at).or_else(|| if before.off < at.off { link_at(doc, field_codes, &before) } else { None })
}

/// The link under the point (x, y) on `page`, where `pos` is the caret position the point hits.
/// The hit snaps to the nearest text, so the link only counts when the point is on the linked
/// character itself, not beside, above or below it (#413).
pub fn link_under(doc: &wordcraft_doc::Document, field_codes: bool, layout: &DocLayout, page: usize, x: f32, y: f32, pos: &Pos) -> Option<Link> {
    let para = doc.para_at(pos)?;
    let at = wordcraft_geom::Point::new(x, y);
    // The caret falls between characters: the one under the point is on either side of it.
    [(pos.off, para.next_boundary(pos.off)), (para.prev_boundary(pos.off), pos.off)].into_iter().filter(|(a, b)| a < b).find_map(|(a, b)| {
        let link = link_at(doc, field_codes, &Pos { off: a, ..pos.clone() })?;
        let (from, to) = (Pos { off: a, ..pos.clone() }, Pos { off: b, ..pos.clone() });
        layout.selection_rects(doc, &from, &to, page).into_iter().any(|(pi, r)| pi == page && r.contains(at)).then_some(link)
    })
}

/// Where a link to a place in the document (`#bookmark`) puts the caret: the bookmark's start, in
/// whichever story it is; `#_top`, Word's built-in bookmark, is the top of the document. `None` for
/// a web link or a bookmark that is gone.
pub fn link_destination(doc: &wordcraft_doc::Document, target: &str) -> Option<Pos> {
    match target.strip_prefix('#')? {
        "_top" => Some(doc.start_of(StoryRef::Body)),
        name => super::insert::bookmark_range(doc, name).map(|(from, _)| from),
    }
}

/// Follow a link (#2945): a link to a place in the document puts the caret at its bookmark's
/// start, scrolled into view, and Go Back returns to the link; a web or email link is the host's
/// to open, so a command only returns its address.
fn follow_link(s: &mut Session, v: &Value) -> CmdResult {
    let fc = s.view.field_codes;
    let link = if let Some(pv) = v.get("pos") {
        let at = s.doc.clamp(&super::parse_pos(pv).ok_or_else(|| CmdError::Params("bad `pos`".into()))?);
        link_beside(&s.doc, fc, &at)
    } else if let (Some(pg), Some(x), Some(y)) = (p::u64(v, "page"), p::f32(v, "x"), p::f32(v, "y")) {
        let story = super::story_param(s, v);
        let pg = usize::try_from(pg).unwrap_or(usize::MAX);
        let l = s.layout();
        l.hit(pg, x, y, story).and_then(|pos| link_under(&s.doc, fc, &l, pg, x, y, &pos))
    } else {
        let (a, b) = s.sel.ordered();
        if a == b { link_beside(&s.doc, fc, &a) } else { link_at(&s.doc, fc, &a) }
    };
    let link = link.ok_or_else(|| CmdError::Failed("there is no link here".into()))?;
    let Some(name) = link.target.strip_prefix('#') else { return Ok(json!({"url": link.target})) };
    let to = link_destination(&s.doc, &link.target).ok_or_else(|| CmdError::Failed(format!("no bookmark `{name}`")))?;
    s.note_place(link.at.clone());
    s.sel = Selection::caret(to);
    s.goal_x = None;
    let mut r = sel_result(s)?;
    if let Some(m) = r.as_object_mut() {
        m.insert("bookmark".into(), json!(name));
    }
    Ok(r)
}

fn painter_pick(s: &mut Session) {
    let f = s.sel.ordered().0;
    // A symbol's own font isn't formatting to paint onto text.
    let chr = s.doc.para_at(&f).map(|p| p.props_of_char(f.off).clone().without_symbol()).unwrap_or_default();
    let para = s.doc.para_at(&f).map(|p| p.props.clone()).unwrap_or_default();
    s.painter = Some((chr, para, false));
}

fn painter(s: &mut Session, v: &Value) -> CmdResult {
    if s.painter.is_some() {
        s.painter = None;
        return Ok(json!({"active": false}));
    }
    painter_pick(s);
    if let Some((_, _, sticky)) = s.painter.as_mut() {
        *sticky = p::bool(v, "sticky").unwrap_or(false);
    }
    Ok(json!({"active": true}))
}

/// Apply the format painter's formatting to the selection.
fn paste_format(s: &mut Session, _: &Value) -> CmdResult {
    let Some((chr, para, sticky)) = s.painter.clone() else { return Err(CmdError::Failed("no formatting copied".into())) };
    let (a, b) = s.sel.ordered();
    if a == b {
        // No selection: apply paragraph formatting (and word formatting like Word).
        let f = s.sel.focus.clone();
        if let Some(pp) = s.doc.para_at(&f) {
            let (x, y) = pp.word_at(f.off);
            s.doc.format_range(&Pos { off: x, ..f.clone() }, &Pos { off: y, ..f.clone() }, &|c| *c = chr.cleared().overlaid(&chr))?;
        }
        s.doc.format_paragraphs(&a, &b, &|p| *p = para.clone())?;
    } else {
        s.doc.format_range(&a, &b, &|c| {
            let link = c.link.clone();
            *c = c.cleared().overlaid(&chr);
            c.link = link;
        })?;
        if a.path != b.path || a.off == 0 {
            s.doc.format_paragraphs(&a, &b, &|p| *p = para.clone())?;
        }
    }
    if !sticky {
        s.painter = None;
    }
    sel_result(s)
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use wordcraft_doc::para::InlineObject;
    use wordcraft_doc::{Document, Pos, StoryRef};

    use super::{Link, link_at, link_under};
    use crate::Session;

    /// A photo with the caption "Figure 1: Kiln" under it, `n` paragraphs, then "See " and a
    /// cross-reference to the figure with `params` added to `insert.crossReference`'s.
    fn figure_and_reference(n: usize, params: serde_json::Value) -> Session {
        let mut text = vec!["A kiln photo".to_string()];
        text.extend((0..n).map(|i| format!("Paragraph {i} of the body text.")));
        text.push("See ".into());
        let mut s = Session::new(Document::from_text(&text.join("\n")));
        s.run("caret.docStart", &json!({})).unwrap();
        s.run("caret.end", &json!({})).unwrap();
        s.run("references.caption", &json!({"label": "Figure", "text": "Kiln"})).unwrap();
        s.run("caret.docEnd", &json!({})).unwrap();
        let mut p = json!({"to": "figure", "target": "Figure 1", "show": "number"});
        if let (Some(m), Some(extra)) = (p.as_object_mut(), params.as_object()) {
            m.extend(extra.clone());
        }
        s.run("insert.crossReference", &p).unwrap();
        s
    }

    /// Where the body's last field starts.
    fn last_field(s: &Session) -> Pos {
        let paths = s.doc.para_paths(StoryRef::Body);
        let path = paths.last().unwrap().clone();
        let para = s.doc.para(StoryRef::Body, &path).unwrap();
        let off = para.object_offsets().into_iter().rfind(|o| matches!(para.object_at(*o), Some(InlineObject::Field { .. }))).unwrap();
        Pos::new(StoryRef::Body, path, off)
    }

    /// A point in the middle of the field at `at`: (page, x, y).
    fn middle_of(s: &mut Session, at: &Pos) -> (usize, f32, f32) {
        let l = s.layout();
        let a = l.caret(at).unwrap();
        let b = l.caret(&Pos { off: at.off + wordcraft_doc::para::OBJ.len_utf8(), ..at.clone() }).unwrap();
        (a.page, (a.x + b.x) / 2.0, a.top + a.height / 2.0)
    }

    /// #2945: a point on a cross-reference with `\h` maps to its bookmark; following it puts the
    /// caret at the caption, and Go Back returns to the reference.
    #[test]
    fn a_point_on_a_hyperlinked_cross_reference_maps_to_its_bookmark() {
        let mut s = figure_and_reference(80, json!({}));
        let at = last_field(&s);
        let (page, x, y) = middle_of(&mut s, &at);
        assert!(page > 0, "the reference is on a later page than the caption");
        let l = s.layout();
        let hit = l.hit(page, x, y, StoryRef::Body).unwrap();
        let link = link_under(&s.doc, false, &l, page, x, y, &hit).unwrap();
        let (name, _) = s.doc.bookmarks().into_iter().find(|(n, _)| n.starts_with("_Ref")).unwrap();
        assert_eq!(link, Link { target: format!("#{name}"), field: true, at: at.clone() });
        // Beside the field, on the "See " text before it, there is no link.
        let left = l.caret(&Pos { off: 1, ..at.clone() }).unwrap();
        let plain = l.hit(page, left.x, y, StoryRef::Body).unwrap();
        assert_eq!(link_under(&s.doc, false, &l, page, left.x, y, &plain), None);
        // Field codes shown: the code isn't a link.
        assert_eq!(link_at(&s.doc, true, &at), None);

        let r = s.run("edit.followLink", &json!({"page": page, "x": x, "y": y})).unwrap();
        assert_eq!(r["bookmark"], json!(name));
        let (start, _) = crate::cmd::insert::bookmark_range(&s.doc, &name).unwrap();
        assert!(s.sel.is_collapsed());
        assert_eq!(s.sel.focus, start, "the caret is at the caption");
        assert_eq!(s.doc.para_at(&s.sel.focus).unwrap().plain_text(), "Figure 1: Kiln");
        s.run("edit.goBack", &json!({})).unwrap();
        assert_eq!(s.sel.focus.path, at.path, "Go Back returns to the reference");

        // By position, and from the caret beside the field.
        s.run("caret.docStart", &json!({})).unwrap();
        s.run("edit.followLink", &json!({"pos": at})).unwrap();
        assert_eq!(s.sel.focus, start);
        s.sel = crate::Selection::caret(Pos { off: at.off + wordcraft_doc::para::OBJ.len_utf8(), ..at.clone() });
        s.run("edit.followLink", &json!({})).unwrap();
        assert_eq!(s.sel.focus, start);
    }

    /// A cross-reference inserted without "Insert as hyperlink" is no link: following it fails
    /// and leaves the caret where it was.
    #[test]
    fn a_cross_reference_without_the_hyperlink_switch_is_no_link() {
        let mut s = figure_and_reference(2, json!({"hyperlink": false}));
        let at = last_field(&s);
        assert_eq!(link_at(&s.doc, false, &at), None);
        s.sel = crate::Selection::caret(at.clone());
        assert!(s.run("edit.followLink", &json!({})).is_err());
        assert_eq!(s.sel.focus, at);
    }

    /// A cross-reference to a bookmark in a footnote follows into the note, and its tip text and
    /// Go To find it there too.
    #[test]
    fn a_cross_reference_follows_into_a_note() {
        let mut s = Session::new(Document::from_text("Body text\nSee "));
        s.run("caret.docStart", &json!({})).unwrap();
        s.run("caret.end", &json!({})).unwrap();
        s.run("references.footnote", &json!({"text": "Kiln note"})).unwrap();
        let note = s.sel.focus.story;
        assert_ne!(note, StoryRef::Body);
        s.run("insert.bookmark", &json!({"name": "InNote"})).unwrap();
        s.run("caret.docEnd", &json!({})).unwrap();
        s.sel = crate::Selection::caret(s.doc.end_of(StoryRef::Body));
        s.run("insert.field", &json!({"instr": "REF InNote \\h", "result": "Kiln note"})).unwrap();
        let at = last_field(&s);
        assert!(link_at(&s.doc, false, &at).is_some());
        s.run("edit.followLink", &json!({"pos": at})).unwrap();
        assert_eq!(s.sel.focus.story, note, "the caret is in the note");
        assert!(crate::cmd::references::bookmark_text(&s.doc, "InNote").is_some(), "found for the tip and Update Field");
        s.sel = crate::Selection::caret(at);
        s.run("edit.goto", &json!({"bookmark": "InNote"})).unwrap();
        assert_eq!(s.sel.focus.story, note);
    }

    /// Hyperlinks to bookmarks follow the same way; a web link only returns its address.
    #[test]
    fn hyperlinks_follow_too() {
        let mut s = Session::new(Document::from_text("Alpha beta\nSee here and there"));
        s.run("select.text", &json!({"text": "beta"})).unwrap();
        s.run("insert.bookmark", &json!({"name": "Spot"})).unwrap();
        s.run("select.text", &json!({"text": "here"})).unwrap();
        s.run("insert.link", &json!({"url": "#Spot"})).unwrap();
        s.run("select.text", &json!({"text": "there"})).unwrap();
        s.run("insert.link", &json!({"url": "https://example.com/"})).unwrap();
        s.run("select.text", &json!({"text": "here"})).unwrap();
        let r = s.run("edit.followLink", &json!({})).unwrap();
        assert_eq!(r["bookmark"], json!("Spot"));
        assert_eq!(s.sel.focus, crate::cmd::insert::bookmark_range(&s.doc, "Spot").unwrap().0);
        s.run("select.text", &json!({"text": "there"})).unwrap();
        let before = s.sel.clone();
        assert_eq!(s.run("edit.followLink", &json!({})).unwrap(), json!({"url": "https://example.com/"}));
        assert_eq!(s.sel, before, "a web link doesn't move the caret");
        // Junk never panics.
        for junk in [json!({"pos": {"story": "nope"}}), json!({"page": 99, "x": 1e30, "y": -1}), json!({"pos": {"block": 9999, "off": 99999}})] {
            let _ = s.run("edit.followLink", &junk);
        }
    }
}
