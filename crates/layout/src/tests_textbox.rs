//! Text boxes' text direction, alignment and linked boxes (Shape Format › Text).

use super::*;
use wordcraft_doc::props::{TextVert, VAlign};
use wordcraft_doc::{PartKind, Pos, TextBody, para_block};

fn lay(doc: &Document) -> DocLayout {
    let mut c = LayoutCache::new();
    layout(doc, &mut c, &LayoutOptions::default())
}

/// A text box `w` × `h` showing `text`, inline at the end of body paragraph `para`; its story id.
fn text_box(d: &mut Document, para: usize, text: &str, w: f32, h: f32, body: TextBody) -> u32 {
    let id = d.add_part(PartKind::TextBox, vec![para_block(Paragraph::with_text(text, Default::default()))]);
    if let Some(p) = d.parts.get_mut(&id) {
        p.body = body;
    }
    let shape = InlineObject::Shape {
        kind: wordcraft_doc::para::ShapeKind::TextBox,
        w,
        h,
        fill: None,
        stroke: None,
        stroke_width: 1.0,
        float: Default::default(),
        story: Some(id),
        effects: Default::default(),
        freeform: None,
        extra: Default::default(),
    };
    let off = d.para(StoryRef::Body, &Path::top(para)).map_or(0, |p| p.len());
    d.insert_object(&Pos::body(para, off), shape, &Default::default()).unwrap();
    id
}

/// The lines of story `id` on the first page: (x, y, turn, l0, l1).
fn lines(l: &DocLayout, id: u32) -> Vec<(f32, f32, TextDirection, usize, usize)> {
    l.pages[0]
        .items
        .iter()
        .filter_map(|it| match it {
            Placed::Lines { story: StoryRef::Part(s), x, y, turn, l0, l1, .. } if *s == id => Some((*x, *y, *turn, *l0, *l1)),
            _ => None,
        })
        .collect()
}

fn frame_of(l: &DocLayout, id: u32) -> Rect {
    l.find_object(0, |o| o.text_box == Some(id)).unwrap().rect
}

#[test]
fn align_text_and_text_direction_place_the_text_in_the_box() {
    let at = |body: TextBody| {
        let mut d = Document::from_text("x");
        let id = text_box(&mut d, 0, "Hi", 144.0, 144.0, body);
        let l = lay(&d);
        (lines(&l, id)[0], frame_of(&l, id))
    };
    let ((_, top, turn, ..), r) = at(TextBody::default());
    assert_eq!(turn, TextDirection::Horizontal);
    assert!((top - (r.y + BOX_INSET_Y)).abs() < 1.0, "top-aligned text starts under the top inset: {top} in {r:?}");
    let ((_, mid, ..), _) = at(TextBody { anchor: VAlign::Center, ..Default::default() });
    let ((_, bottom, ..), _) = at(TextBody { anchor: VAlign::Bottom, ..Default::default() });
    assert!(mid > top + 40.0 && bottom > mid + 40.0, "{top} {mid} {bottom}");
    assert!(bottom < r.bottom() - BOX_INSET_Y, "the line stays inside the box");
    // Rotated 90°: lines run down from the top right corner; 270°: up from the bottom left.
    let ((x, y, turn, ..), r) = at(TextBody { vert: TextVert::Vert, ..Default::default() });
    assert_eq!(turn, TextDirection::Down);
    assert!((x - (r.right() - BOX_INSET_X)).abs() < 1.0 && (y - (r.y + BOX_INSET_Y)).abs() < 1.0, "{x},{y} in {r:?}");
    let ((x, y, turn, ..), r) = at(TextBody { vert: TextVert::Vert270, anchor: VAlign::Bottom, ..Default::default() });
    assert_eq!(turn, TextDirection::Up);
    assert!(x > r.x + 100.0 && (y - (r.bottom() - BOX_INSET_Y)).abs() < 1.0, "bottom-aligned turned text sits at the far side: {x},{y} in {r:?}");
    // Stacked: one letter to a line, upright.
    let mut d = Document::from_text("x");
    let id = text_box(&mut d, 0, "Hi", 144.0, 144.0, TextBody { vert: TextVert::Stacked, ..Default::default() });
    let l = lay(&d);
    let (.., l0, l1) = lines(&l, id)[0];
    assert_eq!(l1 - l0, 2, "two letters, two lines");
}

#[test]
fn linked_boxes_continue_the_text() {
    let mut d = Document::from_text("one\ntwo");
    let text = "Words that run on well past the first little box. ".repeat(6);
    let a = text_box(&mut d, 0, &text, 144.0, 48.0, TextBody::default());
    let b = text_box(&mut d, 1, "", 144.0, 400.0, TextBody::default());
    let unlinked = lay(&d);
    let shown = |l: &DocLayout| lines(l, a).iter().map(|(.., l0, l1)| l1 - l0).sum::<usize>();
    let in_a = shown(&unlinked);
    d.parts.get_mut(&a).unwrap().body.next = Some(b);
    let l = lay(&d);
    // Both boxes show (and edit) the first box's story.
    let frames: Vec<Rect> = l.pages[0]
        .items
        .iter()
        .filter_map(|it| match it {
            Placed::Object { rect, text_box: Some(s), .. } if *s == a => Some(*rect),
            _ => None,
        })
        .collect();
    assert_eq!(frames.len(), 2);
    let rb = frames[1];
    let ls = lines(&l, a);
    // The first box shows what fits; the second picks up at the next line, at its top.
    let (first, rest): (Vec<(f32, f32, TextDirection, usize, usize)>, Vec<_>) = ls.iter().partition(|(_, y, ..)| *y < rb.y);
    assert_eq!(first.iter().map(|(.., l0, l1)| l1 - l0).sum::<usize>(), in_a);
    let (_, y, _, l0, _) = rest[0];
    assert_eq!(l0, in_a, "continues with the first line the first box couldn't show");
    assert!((y - (rb.y + BOX_INSET_Y)).abs() < 1.0);
    // Every line shows once.
    let total = ls.iter().map(|(.., l0, l1)| l1 - l0).sum::<usize>();
    let all = l.pages[0].items.iter().find_map(|it| match it {
        Placed::Lines { story: StoryRef::Part(s), para, .. } if *s == a => Some(para.lines.len()),
        _ => None,
    });
    assert_eq!(Some(total), all);
}

#[test]
fn hostile_link_chains_stay_bounded() {
    // Boxes linking in a circle show their own text; a box linking to itself too.
    let mut d = Document::from_text("one\ntwo\nthree");
    let a = text_box(&mut d, 0, "A", 72.0, 36.0, TextBody::default());
    let b = text_box(&mut d, 1, "B", 72.0, 36.0, TextBody::default());
    let c = text_box(&mut d, 2, "C", 72.0, 36.0, TextBody::default());
    d.parts.get_mut(&a).unwrap().body.next = Some(b);
    d.parts.get_mut(&b).unwrap().body.next = Some(a);
    d.parts.get_mut(&c).unwrap().body.next = Some(c);
    assert!(d.text_box_chains().is_empty());
    let l = lay(&d);
    for id in [a, b, c] {
        assert_eq!(lines(&l, id).len(), 1, "box {id} shows its own text");
    }
}

#[test]
fn own_margins_and_resize_to_fit_keep_the_text_in_view() {
    // #743: a cover page's two-line box with no margins (`lIns="0"`…) is just tall enough for
    // its text; with Word's default margins its second line would be cut.
    let two = |h: f32, body: TextBody| {
        let mut d = Document::from_text("x");
        let id = text_box(&mut d, 0, "One", 144.0, h, body);
        d.parts.get_mut(&id).unwrap().blocks =
            vec![para_block(Paragraph::with_text("One", Default::default())), para_block(Paragraph::with_text("Two", Default::default()))];
        let l = lay(&d);
        let shown: usize = lines(&l, id).iter().map(|(.., l0, l1)| l1 - l0).sum();
        (shown, lines(&l, id).first().map(|(x, y, ..)| (*x, *y)), frame_of(&l, id))
    };
    // How deep the two lines reach without margins, from a box with room to spare.
    let zero = TextBody { inset: Some([0; 4]), ..Default::default() };
    let mut d = Document::from_text("x");
    let id = text_box(&mut d, 0, "One", 144.0, 400.0, zero);
    d.parts.get_mut(&id).unwrap().blocks =
        vec![para_block(Paragraph::with_text("One", Default::default())), para_block(Paragraph::with_text("Two", Default::default()))];
    let l = lay(&d);
    let r = frame_of(&l, id);
    let need = l.pages[0]
        .items
        .iter()
        .filter_map(|it| match it {
            Placed::Lines { story: StoryRef::Part(s), para, l0, l1, y, .. } if *s == id => {
                let (first, last) = (para.lines.get(*l0)?, para.lines.get(l1.checked_sub(1)?)?);
                Some(y + last.top + last.height - first.top - r.y)
            }
            _ => None,
        })
        .fold(0.0f32, f32::max);
    assert!(need > 10.0, "{need}");
    let (shown, at, r) = two(need + 1.0, zero);
    assert_eq!(shown, 2, "both lines fit without margins");
    let (x, y) = at.unwrap();
    assert!((x - r.x).abs() < 0.5 && (y - r.y).abs() < 0.5, "text starts at the box's corner: {x},{y} in {r:?}");
    assert_eq!(two(need + 1.0, TextBody::default()).0, 1, "Word's default margins cut the second line");
    let (shown, ..) = two(need / 2.0, TextBody { auto_fit: true, ..Default::default() });
    assert_eq!(shown, 2, "a box that resizes to fit its text shows it all");
    // Hostile margins wider than the box still lay out.
    let (shown, ..) = two(20.0, TextBody { inset: Some([u32::MAX; 4]), ..Default::default() });
    assert!(shown >= 1);
}

/// A 160 × 80 text box in front of the text at (100, 150) on the page, turned and flipped as
/// `rot`, `flip_h` and `flip_v` say, showing two paragraphs; its story id.
fn turned_box(d: &mut Document, rot: f32, flip_h: bool, flip_v: bool) -> u32 {
    let float =
        Float { wrap: Wrap::InFrontOfText, h_rel: Anchor::Page, v_rel: Anchor::Page, x: 100.0, y: 150.0, rot, flip_h, flip_v, ..Default::default() };
    let id = text_box(d, 0, "", 160.0, 80.0, TextBody::default());
    d.parts.get_mut(&id).unwrap().blocks =
        vec![para_block(Paragraph::with_text("First line", Default::default())), para_block(Paragraph::with_text("Second line", Default::default()))];
    let mut p = d.para(StoryRef::Body, &Path::top(0)).unwrap().clone();
    *p.objects.iter_mut().find_map(|o| o.float_mut()).unwrap() = float;
    p.touch();
    d.body[0] = para_block(p);
    id
}

/// The text drawn inside a turn: (centre, spin, text) per glyph run.
fn turned_text(draws: &[crate::display::Draw]) -> Vec<((f32, f32), Spin, String)> {
    use crate::display::Draw;
    let mut out = Vec::new();
    for d in draws {
        if let Draw::Rotated { cx, cy, spin, items } = d {
            for i in items {
                if let Draw::Glyphs { text, .. } = i {
                    out.push(((*cx, *cy), *spin, text.clone()));
                }
            }
        }
    }
    out
}

/// The box's own text drawn without a turn.
fn upright_text(draws: &[crate::display::Draw]) -> Vec<(String, f32, f32)> {
    draws
        .iter()
        .filter_map(|d| match d {
            crate::display::Draw::Glyphs { text, glyphs, .. } if text.contains("line") => glyphs.first().map(|g| (text.clone(), g.1, g.2)),
            _ => None,
        })
        .collect()
}

#[test]
fn a_rotated_text_box_turns_its_text_with_it() {
    // #2458: the text turns with the box about its centre. Flips never mirror it: a vertical
    // flip turns it upside down.
    for (rot, flip_h, flip_v, want) in [(30.0, false, false, 30.0), (90.0, false, false, 90.0), (0.0, false, true, 180.0), (30.0, true, true, 210.0)]
    {
        let mut d = Document::from_text("Body text.");
        let id = turned_box(&mut d, rot, flip_h, flip_v);
        let l = lay(&d);
        let r = frame_of(&l, id);
        let draws = crate::display::page_display(&d, &l.pages[0], &Default::default());
        let turned = turned_text(&draws);
        let texts: Vec<&str> = turned.iter().map(|(_, _, t)| t.as_str()).collect();
        assert!(texts.iter().any(|t| t.contains("First")) && texts.iter().any(|t| t.contains("Second")), "{rot}: {texts:?}");
        for ((cx, cy), spin, _) in &turned {
            assert!((cx - (r.x + r.w / 2.0)).abs() < 0.01 && (cy - (r.y + r.h / 2.0)).abs() < 0.01, "about the box's centre: {cx},{cy} {r:?}");
            assert_eq!((spin.deg, spin.flip_h, spin.flip_v), (want, false, false), "{rot} {flip_h} {flip_v}");
        }
        assert!(upright_text(&draws).is_empty(), "{rot}: nothing of the box's text drawn unturned");
    }
}

#[test]
fn an_unturned_or_mirrored_text_box_draws_its_text_as_before() {
    // 0° draws the text as it always did, with no turn around it; a horizontal flip mirrors the
    // box but leaves its text exactly where it is.
    let draws = |flip_h: bool| {
        let mut d = Document::from_text("Body text.");
        let id = turned_box(&mut d, 0.0, flip_h, false);
        let l = lay(&d);
        assert!(l.pages[0].items.iter().all(|i| !matches!(i, Placed::Lines { story: StoryRef::Part(s), spin: Some(_), .. } if *s == id)));
        crate::display::page_display(&d, &l.pages[0], &Default::default())
    };
    let plain = draws(false);
    assert!(turned_text(&plain).is_empty());
    assert!(!plain.iter().any(|d| matches!(d, crate::display::Draw::Rotated { .. })), "no turn at all at 0°");
    let text = upright_text(&plain);
    assert_eq!(text.len(), 2, "{text:?}");
    let mirrored = draws(true);
    assert!(turned_text(&mirrored).is_empty());
    assert_eq!(upright_text(&mirrored), text);
}

#[test]
fn do_not_rotate_text_keeps_a_rotated_box_s_text_level() {
    // `bodyPr upright="1"`: the box turns, its text stays level where it is.
    let mut d = Document::from_text("Body text.");
    let id = turned_box(&mut d, 30.0, false, false);
    d.parts.get_mut(&id).unwrap().body.upright = true;
    let l = lay(&d);
    let draws = crate::display::page_display(&d, &l.pages[0], &Default::default());
    assert!(turned_text(&draws).is_empty());
    assert_eq!(upright_text(&draws).len(), 2);
    let r = frame_of(&l, id);
    let lines = lines(&l, id);
    assert!(lines.iter().all(|(x, y, ..)| r.contains(wordcraft_geom::Point::new(*x, *y))), "{lines:?} in {r:?}");
}

#[test]
fn clicks_and_the_caret_in_a_rotated_text_box_follow_its_turned_text() {
    let mut d = Document::from_text("Body text.");
    let id = turned_box(&mut d, 30.0, false, false);
    let l = lay(&d);
    let story = StoryRef::Part(id);
    // The middle of "line" in the second paragraph, in the box's unturned frame, then on the page.
    let (x, y, para, spin) = l.pages[0]
        .items
        .iter()
        .find_map(|i| match i {
            Placed::Lines { story: s, path, x, y, para, spin, .. } if *s == story && *path == Path::top(1) => Some((*x, *y, para.clone(), *spin)),
            _ => None,
        })
        .unwrap();
    let spin = spin.expect("turned with the box");
    let line = &para.lines[0];
    let (fx, fy) = (x + para.x_of(0, 9).unwrap(), y + line.height / 2.0);
    let (px, py) = spin.apply(fx, fy);
    assert!((px - fx).hypot(py - fy) > 10.0, "the turn moves it: {fx},{fy} → {px},{py}");
    assert_eq!(l.story_at(0, px, py), Some(story), "inside the turned box");
    let pos = l.hit(0, px, py, story).unwrap();
    assert_eq!(pos.path, Path::top(1));
    assert!((7..=11).contains(&pos.off), "in the word \"line\": {pos:?}");
    // The caret leans with the text, 30° clockwise, through the page point clicked.
    let c = l.caret(&pos).unwrap();
    assert!(c.height > 0.0 && c.width < 0.0, "{c:?}");
    assert!(((-c.width).atan2(c.height).to_degrees() - 30.0).abs() < 0.5, "{c:?}");
    let (mx, my) = (c.x + c.width / 2.0, c.top + c.height / 2.0);
    assert!((mx - px).hypot(my - py) < 4.0, "caret at {mx},{my}, click at {px},{py}");
    // Up from there goes to the first line, the same way along it.
    let (up, _) = l.vertical(&pos, None, -1, 0).unwrap();
    assert_eq!(up.path, Path::top(0));
    assert!((up.off as i64 - pos.off as i64).abs() <= 2, "{up:?} from {pos:?}");
    // The selection is highlighted turned: its rectangles are in the box's frame, with its turn.
    let (a, b) = (Pos { story, path: Path::top(1), off: 0 }, Pos { story, path: Path::top(1), off: 11 });
    let areas = l.selection_areas(&d, &a, &b, 0);
    assert!(!areas.is_empty() && areas.iter().all(|(_, _, s)| *s == Some(spin)), "{areas:?}");
    let (_, r, _) = areas[0];
    assert!(r.contains(wordcraft_geom::Point::new(fx, fy)), "{r:?}");
    let bounds = l.selection_rects(&d, &a, &b, 0);
    assert!(bounds[0].1.contains(wordcraft_geom::Point::new(px, py)) && bounds[0].1.h > r.h, "the page area it covers: {bounds:?}");
}

#[test]
fn text_boxes_in_turned_groups_and_headers_turn_too() {
    use wordcraft_doc::para::GroupChild;
    // A group turned 90° holding a text box turned 10° of its own: the text turns 100°.
    let mut d = Document::from_text("Body text.");
    let id = d.add_part(PartKind::TextBox, vec![para_block(Paragraph::with_text("Grouped", Default::default()))]);
    let member = InlineObject::Shape {
        kind: wordcraft_doc::para::ShapeKind::TextBox,
        w: 100.0,
        h: 40.0,
        fill: None,
        stroke: None,
        stroke_width: 1.0,
        float: Float { rot: 10.0, ..Default::default() },
        story: Some(id),
        effects: Default::default(),
        freeform: None,
        extra: Default::default(),
    };
    let float = Float { wrap: Wrap::InFrontOfText, h_rel: Anchor::Page, v_rel: Anchor::Page, x: 100.0, y: 200.0, rot: 90.0, ..Default::default() };
    let group = InlineObject::Group {
        w: 200.0,
        h: 40.0,
        float,
        ch_w: 200.0,
        ch_h: 40.0,
        children: vec![GroupChild { x: 100.0, y: 0.0, obj: member }],
        canvas: None,
    };
    d.insert_object(&Pos::body(0, 0), group, &Default::default()).unwrap();
    let l = lay(&d);
    let spin = l.pages[0].items.iter().find_map(|i| match i {
        Placed::Lines { story: StoryRef::Part(s), spin, .. } if *s == id => Some(*spin),
        _ => None,
    });
    let b = spin.flatten().expect("turned with the group");
    assert_eq!(b.spin.deg, 100.0);
    // The member's centre, carried a quarter turn about the group's: below the group's centre.
    let g = l.object(&Pos::body(0, 0), 0).unwrap().rect;
    assert!((b.centre.x - (g.x + 100.0)).abs() < 0.01 && (b.centre.y - (g.y + 20.0 + 50.0)).abs() < 0.01, "{b:?} in {g:?}");
    // A turned text box in the header: drawn turned, and found where its turned shape is.
    let mut d = Document::from_text("Body text.");
    let hid = d.add_part(PartKind::Header, vec![para_block(Paragraph::with_text("Head", Default::default()))]);
    d.last_section.headers.default = Some(hid);
    let id = d.add_part(PartKind::TextBox, vec![para_block(Paragraph::with_text("Header box", Default::default()))]);
    let shape = InlineObject::Shape {
        kind: wordcraft_doc::para::ShapeKind::TextBox,
        w: 120.0,
        h: 30.0,
        fill: None,
        stroke: None,
        stroke_width: 1.0,
        float: Float { wrap: Wrap::InFrontOfText, h_rel: Anchor::Page, v_rel: Anchor::Page, x: 300.0, y: 10.0, rot: 90.0, ..Default::default() },
        story: Some(id),
        effects: Default::default(),
        freeform: None,
        extra: Default::default(),
    };
    let e = d.end_of(StoryRef::Part(hid));
    d.insert_object(&e, shape, &Default::default()).unwrap();
    let l = lay(&d);
    let draws = crate::display::page_display(&d, &l.pages[0], &Default::default());
    assert!(turned_text(&draws).iter().any(|(_, s, t)| s.deg == 90.0 && t.contains("Header")), "{:?}", turned_text(&draws));
    let r = l.text_box(id, 0).unwrap().rect;
    let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
    // Turned a quarter, the box stands upright: its frame's left end is empty, below its centre isn't.
    assert_eq!(l.header_footer_text_box_at(0, r.x + 5.0, cy), None);
    assert_eq!(l.header_footer_text_box_at(0, cx, cy + 50.0), Some(id));
}

#[test]
fn a_rotated_warped_text_box_bends_its_text_and_turns_it() {
    let mut d = Document::from_text("Body text.");
    turned_box(&mut d, 30.0, false, false);
    let mut p = d.para(StoryRef::Body, &Path::top(0)).unwrap().clone();
    for o in p.objects.iter_mut() {
        if let InlineObject::Shape { extra, .. } = o {
            extra.warp = wordcraft_doc::wordart::TextWarp::new("textArchUp");
        }
    }
    p.touch();
    d.body[0] = para_block(p);
    let l = lay(&d);
    let draws = crate::display::page_display(&d, &l.pages[0], &Default::default());
    let bent: Vec<f32> = draws
        .iter()
        .filter_map(|d| match d {
            crate::display::Draw::Rotated { spin, items, .. } if items.iter().any(|i| matches!(i, crate::display::Draw::Art { .. })) => {
                Some(spin.deg)
            }
            _ => None,
        })
        .collect();
    assert_eq!(bent, [30.0], "the bent letters turn with the box");
    assert!(!draws.iter().any(|d| matches!(d, crate::display::Draw::Art { .. })), "none drawn unturned");
}

#[test]
fn change_bars_beside_a_rotated_text_box_s_lines_cover_the_turned_lines() {
    // A paragraph with a tracked formatting change, in a box turned 90°: its bar spans the
    // height the turned (now upright) line covers on the page.
    let mut d = Document::from_text("Body text.");
    let id = turned_box(&mut d, 90.0, false, false);
    let mut p = Paragraph::with_text("Changed paragraph text", Default::default());
    p.props.fmt_change = wordcraft_doc::props::PropChange::boxed(0, Default::default());
    d.parts.get_mut(&id).unwrap().blocks = vec![para_block(p)];
    let l = lay(&d);
    let line = l.pages[0].items.iter().find_map(|i| match i {
        Placed::Lines { story: StoryRef::Part(s), x, y, para, spin, .. } if *s == id => {
            let ln = para.lines.first()?;
            Some(spin.unwrap().bounds(Rect::new(x + ln.left, *y, ln.right - ln.left, ln.height)))
        }
        _ => None,
    });
    let line = line.unwrap();
    assert!(line.h > 3.0 * line.w, "turned upright: {line:?}");
    let bars: Vec<(f32, f32)> = crate::display::page_display(&d, &l.pages[0], &Default::default())
        .iter()
        .filter_map(|d| match d {
            crate::display::Draw::Line { x0, y0, y1, color, .. } if *color == crate::display::CHANGE_BAR && *x0 < 72.0 => Some((*y0, *y1)),
            _ => None,
        })
        .collect();
    assert_eq!(bars.len(), 1, "{bars:?}");
    assert!((bars[0].0 - line.y).abs() < 0.01 && (bars[0].1 - line.bottom()).abs() < 0.01, "{bars:?} vs {line:?}");
}

#[test]
fn links_in_clipped_turned_text_get_no_link_area() {
    // Link areas are axis-aligned page rectangles: text drawn turned gets none, also inside a
    // clipped table cell (the PDF would put the area where the text is not).
    use crate::display::Draw;
    let glyphs = || Draw::Glyphs {
        face: wordcraft_fonts::word::resolve("Arial", false, false).face,
        size: 12.0,
        glyphs: vec![(1, 0.0, 0.0)],
        color: wordcraft_doc::props::Rgb::BLACK,
        alpha: 1.0,
        synth_bold: false,
        synth_italic: false,
        text: "a".into(),
        link: Some("https://example.com".into()),
        ranges: Vec::new(),
        text_bg: None,
    };
    let mut items = vec![Draw::Clip { rect: Rect::new(0.0, 0.0, 10.0, 10.0), items: vec![glyphs()] }, glyphs()];
    crate::display::unlink(&mut items);
    fn links(items: &[Draw]) -> usize {
        items
            .iter()
            .map(|d| match d {
                Draw::Glyphs { link, .. } => usize::from(link.is_some()),
                Draw::Clip { items, .. } => links(items),
                _ => 0,
            })
            .sum()
    }
    assert_eq!(links(&items), 0);
}
