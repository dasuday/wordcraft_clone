//! Drag-and-drop editing (#2449): the selected text dragged with the mouse to another place in the
//! document is moved there, or copied with Ctrl (Option on a Mac) held. The canvas runs
//! `edit.moveText` when the button is released; agents can run it directly.

use std::collections::BTreeSet;

use serde_json::{Value, json};
use wordcraft_doc::edit::Fragment;
use wordcraft_doc::para::InlineObject;
use wordcraft_doc::{Document, PartKind, Pos, StoryRef};

use super::{delete_selection, parse_pos, sel_result};
use crate::{CmdError, CmdResult, CommandSpec, Selection, Session, p};

/// Nesting depth followed when looking for the stories dragged text carries.
const MAX_DEPTH: usize = 64;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("edit.moveText", "Drag and Drop", "Editing", move_text)
            .params(
                r#"{"to": Pos, "copy"?: bool} | {"page": n, "x": pt, "y": pt, "story"?: StoryRef, "copy"?: bool} (moves the selection to `to`, or copies it with `copy`, as dragging it with the mouse does; one undo step; the moved text is selected; a drop point inside the selection is refused)"#,
            )
            .when(why_not),
    ]
}

/// Why the selection can't be dragged, if it can't. A selection running from text into a table
/// (or out of one) can't: it takes whole rows, and moving it would leave their cells empty.
pub fn why_not(s: &Session) -> Option<&'static str> {
    let (a, b) = s.sel.ordered();
    if a == b {
        Some("nothing is selected")
    } else if s.cell_range().is_some() || s.column_segments().is_some() {
        Some("whole table cells and column selections can't be dragged")
    } else if a.story != b.story || a.path.parent() != b.path.parent() {
        Some("a selection running into or out of a table can't be dragged")
    } else {
        None
    }
}

/// The text boxes and notes the text `a..b` carries: dropping it into one of them is refused.
pub fn carried_stories(doc: &Document, a: &Pos, b: &Pos) -> BTreeSet<u32> {
    carried(&doc.copy_range(a, b))
}

/// The drop point: a position, or a point on a page hit-tested in the selection's story (or
/// `story`).
fn target(s: &mut Session, v: &Value) -> Result<Pos, CmdError> {
    let to = if let Some(pv) = v.get("to") {
        parse_pos(pv).ok_or_else(|| CmdError::Params("bad `to`".into()))?
    } else if let (Some(pg), Some(x), Some(y)) = (p::u64(v, "page"), p::f32(v, "x"), p::f32(v, "y")) {
        let story = super::story_param(s, v);
        let page = usize::try_from(pg).unwrap_or(usize::MAX);
        s.layout().hit(page, x, y, story).ok_or_else(|| CmdError::Params("no text there".into()))?
    } else {
        return Err(CmdError::Params("`to` or `page`/`x`/`y` required".into()));
    };
    if s.doc.para_at(&to).is_none() {
        return Err(CmdError::Params("`to` is not in the document".into()));
    }
    Ok(s.doc.clamp(&to))
}

/// The text boxes and notes `frag` carries (their stories go wherever the text goes).
fn carried(frag: &Fragment) -> BTreeSet<u32> {
    let mut out: BTreeSet<u32> = frag.parts.keys().copied().collect();
    let blocks = frag.blocks.iter().chain(frag.parts.values().flat_map(|p| p.blocks.iter().map(|b| &**b)));
    for b in blocks {
        notes_in(b, 0, &mut out);
    }
    out
}

fn notes_in(b: &wordcraft_doc::Block, depth: usize, out: &mut BTreeSet<u32>) {
    if depth > MAX_DEPTH {
        return;
    }
    match b {
        wordcraft_doc::Block::Para(p) => {
            out.extend(p.objects.iter().filter_map(|o| match o {
                InlineObject::NoteRef { id, .. } => Some(*id),
                _ => None,
            }));
        }
        wordcraft_doc::Block::Table(t) => {
            for cell in t.rows.iter().flat_map(|r| r.cells.iter()) {
                for cb in &cell.blocks {
                    notes_in(cb, depth.saturating_add(1), out);
                }
            }
        }
    }
}

/// Where `p` is counted from the end of its story: paragraphs after its own, and bytes from it to
/// its paragraph's end. Deleting text before `p` leaves both unchanged.
fn from_end(doc: &Document, p: &Pos) -> Option<(usize, usize)> {
    let paths = doc.para_paths(p.story);
    let i = paths.iter().position(|q| *q == p.path)?;
    let after = paths.len().checked_sub(i)?.checked_sub(1)?;
    let len = doc.para(p.story, &p.path)?.len();
    Some((after, len.checked_sub(p.off)?))
}

/// The position [`from_end`] describes.
fn at_from_end(doc: &Document, story: StoryRef, (after, tail): (usize, usize)) -> Option<Pos> {
    let paths = doc.para_paths(story);
    let i = paths.len().checked_sub(1)?.checked_sub(after)?;
    let path = paths.get(i)?.clone();
    let off = doc.para(story, &path)?.len().checked_sub(tail)?;
    Some(doc.clamp(&Pos { story, path, off }))
}

fn move_text(s: &mut Session, v: &Value) -> CmdResult {
    let copy = match v.get("copy") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(CmdError::Params("`copy` must be true or false".into())),
    };
    let to = target(s, v)?;
    let (a, b) = s.sel.ordered();
    if to.story == a.story && a <= to && to <= b {
        return Err(CmdError::Failed("the drop point is inside the selection".into()));
    }
    let frag = s.doc.copy_range(&a, &b);
    if frag.blocks.is_empty() {
        return Err(CmdError::Failed("nothing to move".into()));
    }
    if let StoryRef::Part(id) = to.story {
        if carried(&frag).contains(&id) {
            return Err(CmdError::Failed("text can't be dropped into a text box or note it carries".into()));
        }
        // A comment's text is edited in its balloon; its anchor may be in the dragged text.
        if to.story != a.story && s.doc.parts.get(&id).is_some_and(|p| p.kind == PartKind::Comment) {
            return Err(CmdError::Failed("text can't be dropped into a comment".into()));
        }
    }
    let r = drop_fragment(s, frag, a, b, to, copy);
    // A tracked move's source is only for this drop, never for a later paste.
    s.pending_move = None;
    r?;
    let mut out = sel_result(s)?;
    if let Some(m) = out.as_object_mut() {
        m.insert("copied".into(), json!(copy));
    }
    Ok(out)
}

/// Take `a..b` away (unless copying) and insert `frag` at `to`, selecting it there.
fn drop_fragment(s: &mut Session, frag: Fragment, a: Pos, b: Pos, to: Pos, copy: bool) -> Result<(), CmdError> {
    let to = if copy {
        to
    } else {
        // A drop point after the text moves up as the text leaves.
        let later = if to.story == a.story && to > b {
            Some(from_end(&s.doc, &to).ok_or_else(|| CmdError::Failed("the drop point is not in the document".into()))?)
        } else {
            None
        };
        s.sel = Selection { anchor: a.clone(), focus: b.clone() };
        super::controls::check_delete(s, &a, &b)?;
        // With Track Changes on, a sentence or paragraph is recorded as moved, as cut and paste do.
        if !super::moves::take_tracked(s, &a, &b, &frag.clipboard_text())? {
            delete_selection(s)?;
        }
        super::controls::restore_placeholder(s)?;
        match later {
            Some(k) => at_from_end(&s.doc, to.story, k).ok_or_else(|| CmdError::Failed("the drop point is not in the document".into()))?,
            None => to,
        }
    };
    if s.doc.para_at(&to).is_none() {
        return Err(CmdError::Failed("the drop point is not in the document".into()));
    }
    s.sel = Selection::caret(to);
    let start = super::paste::insert(s, frag)?;
    let end = s.sel.focus.clone();
    s.sel = Selection { anchor: start, focus: end };
    s.goal_x = None;
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};
    use wordcraft_doc::{Block, Document, Path, Pos, StoryRef, Table};

    use crate::{Selection, Session};

    fn session(text: &str) -> Session {
        let mut s = Session::new(Document::new());
        s.run("document.setText", &json!({"text": text})).unwrap();
        s
    }

    fn select(s: &mut Session, a: (usize, usize), b: (usize, usize)) {
        s.run("select.range", &json!({"anchor": {"block": a.0, "off": a.1}, "focus": {"block": b.0, "off": b.1}})).unwrap();
    }

    fn drop_at(s: &mut Session, block: usize, off: usize, copy: bool) -> Result<Value, crate::CmdError> {
        s.run("edit.moveText", &json!({"to": {"block": block, "off": off}, "copy": copy}))
    }

    fn texts(s: &Session) -> Vec<String> {
        s.doc.body.iter().filter_map(|b| b.as_para()).map(|p| p.text.clone()).collect()
    }

    fn sel(s: &Session) -> (Pos, Pos) {
        (s.sel.anchor.clone(), s.sel.focus.clone())
    }

    #[test]
    fn a_word_moves_forward_and_back_within_a_paragraph() {
        let mut s = session("The quick brown fox");
        select(&mut s, (0, 4), (0, 10));
        drop_at(&mut s, 0, 16, false).unwrap();
        assert_eq!(texts(&s), ["The brown quick fox"]);
        assert_eq!(sel(&s), (Pos::body(0, 10), Pos::body(0, 16)), "the moved text is selected at its new place");
        // And back: before the drop point, which stays where it is.
        drop_at(&mut s, 0, 4, false).unwrap();
        assert_eq!(texts(&s), ["The quick brown fox"]);
        assert_eq!(sel(&s), (Pos::body(0, 4), Pos::body(0, 10)));
    }

    #[test]
    fn text_moves_across_paragraphs_and_whole_paragraphs_move() {
        let mut s = session("Alpha beta\nGamma delta");
        select(&mut s, (0, 5), (0, 10));
        drop_at(&mut s, 1, 5, false).unwrap();
        assert_eq!(texts(&s), ["Alpha", "Gamma beta delta"]);
        assert_eq!(sel(&s), (Pos::body(1, 5), Pos::body(1, 10)));

        // The first paragraph, mark and all, dropped before the third.
        let mut s = session("First.\nSecond.\nThird.");
        select(&mut s, (0, 0), (1, 0));
        drop_at(&mut s, 2, 0, false).unwrap();
        assert_eq!(texts(&s), ["Second.", "First.", "Third."]);
        assert_eq!(sel(&s), (Pos::body(1, 0), Pos::body(2, 0)));
        // Two paragraphs dropped at the start of the document.
        let mut s = session("One.\nTwo.\nThree.\nFour.");
        select(&mut s, (2, 0), (3, 5));
        drop_at(&mut s, 0, 0, false).unwrap();
        assert_eq!(texts(&s), ["Three.", "Four.One.", "Two.", ""]);
        assert_eq!(sel(&s), (Pos::body(0, 0), Pos::body(1, 5)));
    }

    #[test]
    fn copying_leaves_the_original() {
        let mut s = session("The quick brown fox");
        select(&mut s, (0, 4), (0, 10));
        let r = drop_at(&mut s, 0, 16, true).unwrap();
        assert_eq!(r["copied"], true);
        assert_eq!(texts(&s), ["The quick brown quick fox"]);
        assert_eq!(sel(&s), (Pos::body(0, 16), Pos::body(0, 22)));
    }

    #[test]
    fn a_drop_inside_the_selection_changes_nothing() {
        let mut s = session("The quick brown fox");
        select(&mut s, (0, 4), (0, 10));
        let depth = s.undo_depth();
        for off in [4, 6, 10] {
            for copy in [false, true] {
                assert!(drop_at(&mut s, 0, off, copy).is_err(), "{off} {copy}");
            }
        }
        assert_eq!(texts(&s), ["The quick brown fox"]);
        assert_eq!(sel(&s), (Pos::body(0, 4), Pos::body(0, 10)), "the selection stays");
        assert_eq!(s.undo_depth(), depth, "no undo step");
        // Nothing selected: nothing to drag.
        s.run("caret.set", &json!({"pos": {"block": 0, "off": 2}})).unwrap();
        assert!(drop_at(&mut s, 0, 16, false).is_err());
    }

    #[test]
    fn one_undo_step_puts_it_back() {
        let mut s = session("One two three\nFour five");
        let depth = s.undo_depth();
        select(&mut s, (0, 4), (0, 8));
        drop_at(&mut s, 1, 5, false).unwrap();
        assert_eq!(texts(&s), ["One three", "Four two five"]);
        assert_eq!(s.undo_depth(), depth + 1);
        assert_eq!(s.undo_history().first().copied(), Some("Drag and Drop"));
        s.run("edit.undo", &json!({})).unwrap();
        assert_eq!(texts(&s), ["One two three", "Four five"]);
        s.run("edit.redo", &json!({})).unwrap();
        assert_eq!(texts(&s), ["One three", "Four two five"]);
    }

    #[test]
    fn formatting_goes_with_the_text() {
        let mut s = session("The quick brown fox");
        select(&mut s, (0, 4), (0, 9));
        s.run("format.bold", &json!({})).unwrap();
        select(&mut s, (0, 4), (0, 10));
        drop_at(&mut s, 0, 16, false).unwrap();
        let p = s.doc.para_at(&Pos::body(0, 0)).unwrap();
        assert_eq!(p.text, "The brown quick fox");
        assert_eq!(p.props_of_char(10).bold, Some(true), "quick is still bold");
        assert_ne!(p.props_of_char(4).bold, Some(true), "brown is not");
    }

    #[test]
    fn a_point_on_the_page_is_a_drop_point() {
        let mut s = session("The quick brown fox");
        let c = s.layout().caret(&Pos::body(0, 16)).unwrap();
        select(&mut s, (0, 4), (0, 10));
        s.run("edit.moveText", &json!({"page": c.page, "x": c.x + 0.5, "y": c.top + c.height / 2.0})).unwrap();
        assert_eq!(texts(&s), ["The brown quick fox"]);
    }

    #[test]
    fn track_changes_record_a_deletion_and_an_insertion_or_a_move() {
        let kinds = |s: &mut Session| -> Vec<String> {
            let ch = s.run("review.changes", &json!({})).unwrap();
            ch.as_array().unwrap().iter().map(|c| c["kind"].as_str().unwrap().to_string()).collect()
        };
        // A word: deleted where it was, inserted where it landed.
        let mut s = session("The quick brown fox");
        s.run("review.trackChanges", &json!({"value": true})).unwrap();
        select(&mut s, (0, 4), (0, 10));
        drop_at(&mut s, 0, 16, false).unwrap();
        assert_eq!(texts(&s), ["The quick brown quick fox"], "the deleted text stays, marked");
        assert_eq!(kinds(&mut s), ["delete", "insert"]);
        assert_eq!(sel(&s), (Pos::body(0, 16), Pos::body(0, 22)), "the inserted text is selected");
        // A paragraph: a move, both ends linked.
        let mut s = session("First one.\nSecond one.\nThird one.");
        s.run("review.trackChanges", &json!({"value": true})).unwrap();
        select(&mut s, (1, 0), (2, 0));
        drop_at(&mut s, 0, 0, false).unwrap();
        assert_eq!(texts(&s), ["Second one.", "First one.", "Second one.", "Third one."]);
        let k = kinds(&mut s);
        assert!(!k.is_empty() && k.iter().all(|k| k.starts_with("move")), "{k:?}");
        assert!(s.pending_move.is_none(), "a later paste is no move");
        // Rejecting it puts the paragraph back.
        s.run("caret.set", &json!({"pos": {"block": 0, "off": 3}})).unwrap();
        s.run("review.reject", &json!({})).unwrap();
        assert_eq!(texts(&s), ["First one.", "Second one.", "Third one."]);
    }

    #[test]
    fn text_drops_into_a_text_box_but_not_into_one_it_carries() {
        let mut s = session("");
        s.run("text.insert", &json!({"text": "Before "})).unwrap();
        let id = s.run("insert.textBox", &json!({"text": "Box"})).unwrap()["story"].as_u64().unwrap() as u32;
        let anchor = s.doc.text_box_anchor(id).unwrap();
        s.run("caret.set", &json!({"pos": anchor})).unwrap();
        s.run("text.insert", &json!({"text": " after"})).unwrap();
        let boxed = StoryRef::Part(id);
        let in_box = json!({"story": {"part": id}, "path": [0], "off": 0});
        // "after" goes into the box.
        let end = s.doc.end_of(StoryRef::Body);
        s.run("select.range", &json!({"anchor": Pos { off: end.off - 5, ..end.clone() }, "focus": end})).unwrap();
        s.run("edit.moveText", &json!({"to": in_box})).unwrap();
        assert_eq!(s.doc.plain_text(boxed), "afterBox");
        assert_eq!(s.sel.focus.story, boxed);
        // The box itself can't go inside its own text.
        let body = s.doc.para(StoryRef::Body, &Path::top(0)).unwrap().text.clone();
        s.run("select.range", &json!({"anchor": Pos::body(0, 0), "focus": s.doc.end_of(StoryRef::Body)})).unwrap();
        assert!(s.run("edit.moveText", &json!({"to": in_box})).is_err());
        assert_eq!(s.doc.para(StoryRef::Body, &Path::top(0)).unwrap().text, body);
        assert_eq!(s.doc.plain_text(boxed), "afterBox");
    }

    /// "Intro", a 2×2 table with "r0c0"… in its cells, "After", "End".
    fn with_table() -> Session {
        let mut d = Document::from_text("Intro\nAfter\nEnd");
        d.insert_block(StoryRef::Body, &Path::top(1), Block::Table(Table::new(2, 2, 400.0))).unwrap();
        let mut s = Session::new(d);
        for r in 0..2 {
            for c in 0..2 {
                s.sel = Selection::caret(cell(r, c, 0));
                s.run("text.insert", &json!({"text": format!("r{r}c{c}")})).unwrap();
            }
        }
        s
    }

    fn cell(r: u32, c: u32, off: usize) -> Pos {
        Pos { story: StoryRef::Body, path: Path(vec![1, r, c, 0]), off }
    }

    fn cell_text(s: &Session, r: u32, c: u32) -> String {
        s.doc.para(StoryRef::Body, &cell(r, c, 0).path).map(|p| p.text.clone()).unwrap_or_default()
    }

    #[test]
    fn a_selection_running_into_or_out_of_a_table_is_refused() {
        let mut s = with_table();
        let before = s.doc.clone();
        // From text into the second row, and from the first row out to the text after.
        for (a, b) in [(Pos::body(0, 2), cell(1, 1, 1)), (cell(0, 0, 0), Pos::body(2, 2))] {
            s.run("select.range", &json!({"anchor": a, "focus": b})).unwrap();
            let e = drop_at(&mut s, 3, 3, false).unwrap_err();
            assert!(e.to_string().contains("into or out of a table"), "{e}");
            assert_eq!(s.doc, before, "the table is untouched");
        }
    }

    #[test]
    fn a_whole_table_moves_with_the_text_around_it() {
        let mut s = with_table();
        select(&mut s, (0, 2), (2, 2));
        drop_at(&mut s, 3, 3, false).unwrap();
        let kinds: Vec<String> = s.doc.body.iter().map(|b| b.as_para().map_or("table".into(), |p| p.text.clone())).collect();
        assert_eq!(kinds, ["Inter", "Endtro", "table", "Af"]);
        let t = s.doc.body[2].as_table().unwrap();
        assert_eq!(t.rows.len(), 2);
        let moved = |r: u32, c: u32| s.doc.para(StoryRef::Body, &Path(vec![2, r, c, 0])).map(|p| p.text.clone());
        assert_eq!((moved(0, 0), moved(1, 1)), (Some("r0c0".into()), Some("r1c1".into())));
        assert_eq!(sel(&s), (Pos::body(1, 3), Pos::body(3, 2)));
    }

    #[test]
    fn text_drops_into_a_table_cell() {
        let mut s = with_table();
        select(&mut s, (0, 2), (0, 5));
        s.run("edit.moveText", &json!({"to": cell(1, 1, 2)})).unwrap();
        assert_eq!(cell_text(&s, 1, 1), "r1troc1");
        assert_eq!(texts(&s)[0], "In");
        assert_eq!(sel(&s), (cell(1, 1, 2), cell(1, 1, 5)));
    }

    #[test]
    fn locked_content_and_protected_documents_refuse_a_drop() {
        let mut s = Session::new(Document::from_text("cat dog"));
        s.run("edit.find", &json!({"text": "cat"})).unwrap();
        s.run("developer.richText", &json!({"tag": "locked"})).unwrap();
        s.run("developer.properties", &json!({"control": "locked", "lockEdit": true})).unwrap();
        let r = s.doc.control_ranges(StoryRef::Body)[0].clone();
        let before = s.doc.clone();
        // Out of the locked control.
        s.run("select.range", &json!({"anchor": r.content_start(), "focus": r.end.clone()})).unwrap();
        let end = s.doc.end_of(StoryRef::Body);
        assert!(s.run("edit.moveText", &json!({"to": end})).is_err());
        assert_eq!(s.doc, before);
        // Into it.
        s.run("select.range", &json!({"anchor": Pos { off: end.off - 3, ..end.clone() }, "focus": end})).unwrap();
        let inside = Pos { off: r.content_start().off + 1, ..r.content_start() };
        assert!(s.run("edit.moveText", &json!({"to": inside})).is_err());
        assert_eq!(s.doc, before);
        // A read-only document refuses any drop.
        let mut s = session("The quick brown fox");
        s.run("review.restrict", &json!({"mode": "readOnly"})).unwrap();
        select(&mut s, (0, 4), (0, 10));
        assert!(drop_at(&mut s, 0, 16, false).is_err());
        assert!(drop_at(&mut s, 0, 16, true).is_err());
        assert_eq!(texts(&s), ["The quick brown fox"]);
    }

    #[test]
    fn text_is_not_dropped_into_a_comment() {
        let mut s = session("The quick brown fox");
        select(&mut s, (0, 16), (0, 19));
        let id = s.run("review.newComment", &json!({"text": "Note"})).unwrap()["id"].as_u64().unwrap() as u32;
        let part = s.doc.comments[&id].part;
        let before = s.doc.clone();
        select(&mut s, (0, 0), (0, 19));
        let e = s.run("edit.moveText", &json!({"to": {"story": {"part": part}, "path": [0], "off": 0}})).unwrap_err();
        assert!(e.to_string().contains("comment"), "{e}");
        assert_eq!(s.doc, before);
    }

    #[test]
    fn repeat_does_not_drop_again() {
        let mut s = session("One.\nTwo.\nThree.");
        select(&mut s, (0, 0), (1, 0));
        drop_at(&mut s, 2, 3, false).unwrap();
        assert_eq!(texts(&s), ["Two.", "ThrOne.", "ee."]);
        assert!(s.run("edit.repeat", &json!({})).is_err(), "nothing to repeat");
        assert_eq!(texts(&s), ["Two.", "ThrOne.", "ee."]);
    }

    #[test]
    fn hostile_drop_points_never_panic() {
        let junk = [
            json!(null),
            json!({}),
            json!({"to": "here"}),
            json!({"to": {"block": 9999, "off": 99999}}),
            json!({"to": {"story": {"part": 77}, "path": [0], "off": 0}}),
            json!({"to": {"story": "body", "path": [0, 5, 5, 0], "off": 3}}),
            json!({"to": {"block": 0, "off": 18446744073709551615u64}}),
            json!({"to": {"block": 0, "off": 1}, "copy": "yes"}),
            json!({"page": 1e30, "x": -1e308, "y": 1e308}),
            json!({"page": -1, "x": 10, "y": 10}),
            json!({"page": 0, "x": 1e300, "y": -1e300, "story": {"part": 4294967295u64}}),
        ];
        for j in &junk {
            let mut s = session("Ünïcødé text here\nand more");
            select(&mut s, (0, 2), (1, 3));
            let _ = s.run("edit.moveText", j);
            s.clamp_selection();
            let _ = s.layout();
        }
        // An offset inside a character is taken to its start.
        let mut s = session("Ünïcødé text");
        select(&mut s, (0, 11), (0, 16));
        s.run("edit.moveText", &json!({"to": {"block": 0, "off": 1}})).unwrap();
        assert_eq!(texts(&s), [" textÜnïcødé"]);
    }
}
