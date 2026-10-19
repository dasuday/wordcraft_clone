use super::*;
use wordcraft_doc::para::{Anchor, Float, ShapeKind, Wrap, WrapText};
use wordcraft_doc::props::LineSpacing;
use wordcraft_doc::{PartKind, Pos, TextBody, para_block};

const TEST_WIDTH: f32 = 200.0;
const WRAP_END: f32 = 60.0;

fn area(left: f32, right: f32) -> para::Exclusion {
    para::Exclusion { top: 0.0, bottom: WRAP_END, left, right, top_bottom: false, contour: None, side: WrapText::BothSides }
}

fn word_doc() -> Document {
    let mut d = Document::from_text("Independent");
    d.styles.default_chr.font = Some("Source Sans 3".into());
    d.styles.default_chr.size = Some(12.0);
    d.para_mut(StoryRef::Body, &Path::top(0)).unwrap().props.line_spacing = Some(LineSpacing::Exactly(12.0));
    d
}

fn with_ctx<T>(d: &Document, f: impl FnOnce(&mut Ctx) -> T) -> T {
    let mut cache = LayoutCache::new();
    let opts = LayoutOptions::default();
    let prior_wraps = PriorBodyWraps::new();
    let mut back_wraps = BackBodyWraps::new();
    let mut ctx = Ctx {
        doc: d,
        cache: &mut cache,
        opts: &opts,
        counters: Default::default(),
        fields: Default::default(),
        notes_hash: 0,
        numbers: HashMap::new(),
        eq_count: 0,
        boxes: Default::default(),
        chains: Default::default(),
        grid: Default::default(),
        hide_deleted: opts.hides_deleted(StoryRef::Body),
        uses_totals: false,
        track_totals: true,
        pct_base: None,
        quiet_marks: Vec::new(),
        mirror: false,
        table_keys: Vec::new(),
        prior_wraps: &prior_wraps,
        back_wraps: &mut back_wraps,
        pin: None,
        footer_wraps: HashMap::new(),
        header_tables: HashMap::new(),
        fitted: HashMap::new(),
        fitted_order: Vec::new(),
        measure_left: 0,
        measure_depth: 0,
        measure_exhausted: false,
    };
    f(&mut ctx)
}

fn line_signature(p: &ParaLayout) -> Vec<(usize, usize, u32, u32, u32)> {
    p.lines.iter().map(|l| (l.start, l.stop, l.top.to_bits(), l.left.to_bits(), l.right.to_bits())).collect()
}

#[test]
fn policy_is_exact_in_warm_cache() {
    let d = word_doc();
    let p = d.para(StoryRef::Body, &Path::top(0)).unwrap();
    let areas = [area(0.0, 180.0)];
    let cold = with_ctx(&d, |ctx| ctx.para_with_wrap(p, TEST_WIDTH, None, &areas, None, &[0]));
    with_ctx(&d, |ctx| {
        let wait = ctx.para_labelled(p, TEST_WIDTH, None, &areas, None);
        let split = ctx.para_with_wrap(p, TEST_WIDTH, None, &areas, None, &[0]);
        assert!(wait.lines[0].top >= WRAP_END);
        assert!(split.lines[0].top < WRAP_END);
        assert!(!Arc::ptr_eq(&wait, &split));
        assert_eq!(line_signature(&split), line_signature(&cold));
        assert_eq!((ctx.cache.misses, ctx.cache.hits), (2, 0));
        let wait_again = ctx.para_labelled(p, TEST_WIDTH, None, &areas, None);
        let split_again = ctx.para_with_wrap(p, TEST_WIDTH, None, &areas, None, &[0]);
        assert!(Arc::ptr_eq(&wait, &wait_again));
        assert!(Arc::ptr_eq(&split, &split_again));
        assert_eq!((ctx.cache.misses, ctx.cache.hits), (2, 2));
        assert_eq!(ctx.cache.used.len(), 2);
    });
}

#[test]
fn rtl_keeps_policy_with_area() {
    for bidi in [false, true] {
        let mut d = word_doc();
        d.para_mut(StoryRef::Body, &Path::top(0)).unwrap().props.bidi = Some(bidi);
        let p = d.para(StoryRef::Body, &Path::top(0)).unwrap();
        let areas = [area(0.0, 180.0), area(400.0, 500.0)];
        with_ctx(&d, |ctx| {
            let waiting = ctx.para_with_wrap(p, TEST_WIDTH, None, &areas, None, &[1]);
            let split = ctx.para_with_wrap(p, TEST_WIDTH, None, &areas, None, &[0]);
            let invalid = ctx.para_with_wrap(p, TEST_WIDTH, None, &areas, None, &[usize::MAX]);
            let default = ctx.para_labelled(p, TEST_WIDTH, None, &areas, None);
            assert!(waiting.lines[0].top >= WRAP_END, "an unrelated Split area cannot change Wait, rtl={bidi}");
            assert!(split.lines[0].top < WRAP_END, "the active area keeps Split when mirrored, rtl={bidi}");
            assert_eq!(line_signature(&invalid), line_signature(&default));
        });
    }
}

#[test]
fn continuation_reindexes_policy() {
    let mut rel = BodyExclusions::default();
    let mut removed = area(300.0, 350.0);
    removed.top = 80.0;
    removed.bottom = 120.0;
    rel.append_split([removed]);
    rel.append_wait([area(0.0, 40.0)]);
    rel.append_split([area(60.0, 90.0)]);
    assert_eq!(rel.split, [0, 2]);
    let ordinary = (Rect::new(120.0, 0.0, 20.0, 1000.0), false, None, WrapText::BothSides);
    let filtered = (Rect::new(900.0, 0.0, 20.0, 1000.0), false, None, WrapText::BothSides);
    let next = continued_exclusions(&rel, &[ordinary, filtered], 0.0, 0.0, 30.0, TEST_WIDTH);
    assert_eq!(next.areas.len(), 3);
    assert_eq!(next.split, [1], "retained Split is reindexed; even a tall new area starts Wait");
    assert_eq!((next.areas[0].bottom, next.areas[1].bottom, next.areas[2].top), (30.0, 30.0, 30.0));
    let waiting = BodyExclusions::wait(next.areas.clone());
    assert_ne!(next, waiting, "policy-only changes must trigger the relay guard");
}

#[test]
fn fallback_keeps_pair() {
    let d = word_doc();
    let p = d.para(StoryRef::Body, &Path::top(0)).unwrap();
    let areas = vec![area(0.0, 180.0)];
    with_ctx(&d, |ctx| {
        let wait = ctx.para_labelled(p, TEST_WIDTH, None, &areas, None);
        let split = ctx.para_with_wrap(p, TEST_WIDTH, None, &areas, None, &[0]);
        let mut pl = wait.clone();
        let mut policy = BodyExclusions::wait(areas.clone());
        let mut changed = BodyExclusions::default();
        changed.append_split(areas);
        assert!(accept_wrap_relay(&mut pl, &mut policy, split.clone(), changed.clone(), 0, wait.lines[0].top).is_none());
        assert!(Arc::ptr_eq(&pl, &wait));
        assert!(policy.split.is_empty());
        assert!(accept_wrap_relay(&mut pl, &mut policy, split.clone(), changed.clone(), usize::MAX, 0.0).is_none());
        assert!(Arc::ptr_eq(&pl, &wait));
        assert!(policy.split.is_empty());
        assert_eq!(accept_wrap_relay(&mut pl, &mut policy, split.clone(), changed, 0, split.lines[0].top), Some(0));
        assert!(Arc::ptr_eq(&pl, &split));
        assert_eq!(policy.split, [0]);
    });
}

fn shape(story: Option<u32>, wrap: Wrap, w: f32, h: f32) -> InlineObject {
    InlineObject::Shape {
        kind: ShapeKind::TextBox,
        w,
        h,
        fill: None,
        stroke: None,
        stroke_width: 0.0,
        float: Float { wrap, h_rel: Anchor::Column, v_rel: Anchor::Paragraph, ..Default::default() },
        story,
        effects: Default::default(),
        freeform: None,
        extra: Default::default(),
    }
}

#[test]
fn default_nested_stays_wait() {
    let mut d = word_doc();
    let id = d.add_part(PartKind::TextBox, vec![para_block(Paragraph::with_text("Independent", Default::default()))]);
    d.parts.get_mut(&id).unwrap().body = TextBody { auto_fit: true, inset: Some([0; 4]), ..Default::default() };
    let mut obstacle = shape(None, Wrap::Square, 180.0, WRAP_END);
    if let InlineObject::Shape { kind, .. } = &mut obstacle {
        *kind = ShapeKind::Rectangle;
    }
    d.insert_object(&Pos::new(StoryRef::Part(id), Path::top(0), 0), obstacle, &Default::default()).unwrap();
    d.insert_object(&Pos::body(0, 0), shape(Some(id), Wrap::Inline, TEST_WIDTH, 10.0), &Default::default()).unwrap();
    let p = d.para(StoryRef::Body, &Path::top(0)).unwrap();
    let expected = with_ctx(&d, |ctx| ctx.fitted_boxes(p));
    assert_eq!(expected.len(), 1);
    assert!(expected[0].1 > WRAP_END, "nested default layout waits below its own obstruction");
    with_ctx(&d, |ctx| {
        let split = ctx.para_with_wrap(p, TEST_WIDTH, None, &[area(0.0, 180.0)], None, &[0]);
        assert!(!split.lines.is_empty());
        assert_eq!(ctx.fitted[&(id, TEST_WIDTH.to_bits())], Some(expected[0].1));
        let ordinary = ctx.para_labelled(p, TEST_WIDTH, None, &[area(0.0, 180.0)], None);
        let cold = with_ctx(&d, |fresh| fresh.para_labelled(p, TEST_WIDTH, None, &[area(0.0, 180.0)], None));
        assert_eq!(line_signature(&ordinary), line_signature(&cold));
    });
}

#[derive(Clone, Copy, Debug)]
enum ReplayCase {
    Plain,
    Totals,
    Notes,
    Columns,
    Footer,
    Combined,
}

fn field(instr: &str) -> InlineObject {
    InlineObject::Field { instr: instr.into(), result: "9".into(), locked: false, dirty: false, code: false, form: String::new() }
}

fn replay_doc(case: ReplayCase) -> (Document, Vec<u32>) {
    let text = std::iter::once("Earlier".to_string())
        .chain(std::iter::once("Boundary anchor".to_string()))
        .chain((0..80).map(|i| format!("Body{i:03} original row")))
        .collect::<Vec<_>>()
        .join("\n");
    let mut d = Document::from_text(&text);
    d.styles.default_chr.font = Some("Source Sans 3".into());
    d.styles.default_chr.size = Some(12.0);
    d.last_section.page_w = 595.3;
    d.last_section.page_h = 841.9;
    d.last_section.margin_top = 94.5;
    d.last_section.margin_bottom = 72.0;
    let list = d.numbering.add_list(wordcraft_doc::ListKind::Numbered);
    for i in 0..d.body.len() {
        let p = d.para_mut(StoryRef::Body, &Path::top(i)).unwrap();
        p.props.space_before = Some(0.0);
        p.props.space_after = Some(0.0);
        p.props.line_spacing = Some(LineSpacing::Exactly(if i == 0 { 80.0 } else { 18.0 }));
        if i >= 2 {
            p.props.numbering = Some(wordcraft_doc::props::NumRef { num: list, level: 0 });
        }
    }
    let id = d.add_part(PartKind::TextBox, vec![para_block(Paragraph::with_text("Boundary story", Default::default()))]);
    let mut obj = shape(Some(id), Wrap::Square, 400.0, 169.15);
    obj.float_mut().unwrap().y = -300.0;
    d.insert_object(&Pos::body(1, 0), obj, &Default::default()).unwrap();
    if matches!(case, ReplayCase::Totals | ReplayCase::Combined) {
        let p = d.para_mut(StoryRef::Body, &Path::top(4)).unwrap();
        for instr in ["NUMPAGES", "SECTIONPAGES"] {
            p.insert_object(p.len(), field(instr), &Default::default()).unwrap();
        }
    }
    let mut notes = Vec::new();
    if matches!(case, ReplayCase::Notes | ReplayCase::Combined) {
        d.settings.footnote_pr =
            wordcraft_doc::section::NoteProps { num_restart: Some(wordcraft_doc::section::NoteRestart::EachPage), ..Default::default() };
        d.para_mut(StoryRef::Body, &Path::top(70)).unwrap().props.page_break_before = Some(true);
        for (i, text) in [(0, "First note payload."), (70, "Later note payload.")] {
            let id = d.add_part(PartKind::Footnote, vec![para_block(Paragraph::with_text(text, Default::default()))]);
            let off = d.para(StoryRef::Body, &Path::top(i)).unwrap().len();
            d.insert_object(
                &Pos::body(i, off),
                InlineObject::NoteRef { kind: wordcraft_doc::para::NoteKind::Footnote, id, custom: String::new() },
                &Default::default(),
            )
            .unwrap();
            notes.push(id);
        }
    }
    if matches!(case, ReplayCase::Columns | ReplayCase::Combined) {
        d.last_section.columns = wordcraft_doc::section::Columns { count: 2, space: 36.0, separator: false, widths: Vec::new() };
    }
    if matches!(case, ReplayCase::Footer | ReplayCase::Combined) {
        let mut table = wordcraft_doc::Table::new(1, 1, 90.0);
        table.props.cell_margins = Some([0.0; 4]);
        table.props.float =
            Some(wordcraft_doc::props::TableFloat { y: 140.0, x: 72.0, h_rel: Anchor::Page, v_rel: Anchor::Page, ..Default::default() });
        table.rows[0].props.height = Some(60.0);
        table.rows[0].props.height_rule = wordcraft_doc::props::HeightRule::Exact;
        let id = d.add_part(PartKind::Footer, vec![Arc::new(Block::Table(table))]);
        d.last_section.footers.default = Some(id);
    }
    (d, notes)
}

fn counted_layout(d: &Document, cache: &mut LayoutCache) -> (DocLayout, ReplayStats) {
    REPLAY_STATS.with(|s| *s.borrow_mut() = ReplayStats::default());
    let laid = layout(d, cache, &LayoutOptions::default());
    let stats = REPLAY_STATS.with(|s| s.borrow().clone());
    assert_eq!(stats.passes, 1 + stats.back_retries + stats.total_retries + stats.note_retries);
    assert!(stats.back_retries <= MAX_BACK_WRAP_PASSES);
    assert!(stats.total_retries + stats.totals_with_back <= MAX_TOTAL_PASSES);
    assert!(stats.note_retries < MAX_NOTE_PASSES);
    assert!(stats.passes <= MAX_BACK_WRAP_PASSES + MAX_TOTAL_PASSES + MAX_NOTE_PASSES);
    assert_eq!(stats.wrap_attempts, stats.wrap_accepted + stats.wrap_rejected);
    (laid, stats)
}

fn body_signature(l: &DocLayout) -> Vec<(usize, Path, usize, usize, u32, u32)> {
    l.pages
        .iter()
        .enumerate()
        .flat_map(|(page, p)| {
            p.items.iter().filter_map(move |it| match it {
                Placed::Lines { story: StoryRef::Body, path, l0, l1, x, y, .. } => Some((page, path.clone(), *l0, *l1, x.to_bits(), y.to_bits())),
                _ => None,
            })
        })
        .collect()
}

fn assert_visible(d: &Document, l: &DocLayout) {
    let mut labels = Vec::new();
    let mut bytes: HashMap<usize, Vec<u8>> = d
        .body
        .iter()
        .enumerate()
        .filter_map(|(i, b)| match b.as_ref() {
            Block::Para(p) => Some((i, vec![0; p.text.len()])),
            _ => None,
        })
        .collect();
    for it in l.pages.iter().flat_map(|p| &p.items) {
        let Placed::Lines { story: StoryRef::Body, path, para, l0, l1, .. } = it else { continue };
        let index = path.0[0] as usize;
        for line in &para.lines[*l0..*l1] {
            for count in &mut bytes.get_mut(&index).unwrap()[line.start..line.stop] {
                *count += 1;
            }
        }
        if *l0 == 0
            && let Some(label) = &para.label
        {
            labels.push(label.text.trim().to_string());
        }
    }
    assert!(bytes.values().all(|row| row.iter().all(|n| *n == 1)), "each body byte remains on one physical line");
    assert_eq!(labels, (1..=80).map(|n| format!("{n}.")).collect::<Vec<_>>(), "replays do not lose or duplicate numbering");
}

#[test]
fn replay_counts_and_state_hold() {
    for case in [ReplayCase::Plain, ReplayCase::Totals, ReplayCase::Notes, ReplayCase::Columns, ReplayCase::Footer, ReplayCase::Combined] {
        let (d, notes) = replay_doc(case);
        let mut cache = LayoutCache::new();
        let (cold, first) = counted_layout(&d, &mut cache);
        let (warm, second) = counted_layout(&d, &mut cache);
        eprintln!("REPLAY_WORK case={case:?} cold={first:?} warm={second:?} pages={}", cold.pages.len());
        assert!(first.back_retries > 0 && second.back_retries > 0);
        assert_eq!(cold.pages.len(), warm.pages.len());
        assert_eq!(body_signature(&cold), body_signature(&warm));
        assert_visible(&d, &cold);
        assert_visible(&d, &warm);
        if matches!(case, ReplayCase::Totals | ReplayCase::Combined) {
            assert!(first.total_retries + first.totals_with_back > 0);
            let shown: Vec<_> = cold
                .pages
                .iter()
                .flat_map(|p| &p.items)
                .filter_map(|it| match it {
                    Placed::Lines { story: StoryRef::Body, path, para, .. } if *path == Path::top(4) => {
                        Some(para.shown.iter().map(|s| s.1.clone()).collect::<Vec<_>>())
                    }
                    _ => None,
                })
                .flatten()
                .collect();
            assert_eq!(shown, vec![cold.pages.len().to_string(); 2]);
        }
        if !notes.is_empty() {
            assert!(first.note_retries > 0);
            let pages: Vec<_> = notes.iter().map(|id| cold.caret(&d.start_of(StoryRef::Part(*id))).unwrap().page).collect();
            assert!(pages[1] > pages[0]);
            let mut marks = HashMap::new();
            for it in cold.pages.iter().flat_map(|p| &p.items) {
                if let Placed::Lines { story: StoryRef::Body, para, .. } = it {
                    for (ci, id) in &para.notes {
                        marks.insert(*id, para.shown.iter().find(|s| s.0 == *ci).map(|s| s.1.clone()));
                    }
                }
            }
            assert_eq!(marks[&notes[0]], Some("1".into()));
            assert_eq!(marks[&notes[1]], Some("1".into()));
        }
        if matches!(case, ReplayCase::Columns | ReplayCase::Combined) {
            let columns: std::collections::HashSet<_> = cold.pages[0]
                .items
                .iter()
                .filter_map(|it| match it {
                    Placed::Lines { story: StoryRef::Body, x, .. } => Some(x.to_bits()),
                    _ => None,
                })
                .collect();
            assert!(columns.len() >= 2, "the fixture traverses both columns");
        }
        if matches!(case, ReplayCase::Footer | ReplayCase::Combined) {
            assert!(cold.pages.iter().all(|p| p.footer.iter().any(|it| matches!(it, Placed::Cell { .. }))));
        }
    }
}
