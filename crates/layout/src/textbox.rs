//! A text box's text (Shape Format › Text): which way it runs (Text Direction), where it sits
//! between the box's edges (Align Text), and linked boxes, whose text continues from one box in
//! the next (Create Link).
//!
//! The text is laid out in the box's own frame (lines `len` long, stacking `across` deep), cut to
//! what fits, aligned, then turned onto the page as a table cell's turned text is. A chain of
//! linked boxes lays out the first box's story and gives each box the next stretch of it: the
//! sizes come from the boxes' shapes, so a box shows the same text wherever it is laid out.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use wordcraft_doc::props::{TextDirection, TextVert, VAlign};
use wordcraft_doc::{Block, Blocks, Path, StoryRef, TextBody};
use wordcraft_geom::Rect;

use crate::{BoxContext, Ctx, Placed, fit_box, layout_box};

/// Linked text boxes for one layout, worked out when a linked box is first laid out.
#[derive(Default)]
pub(crate) struct Chains {
    built: bool,
    /// Box → (chain, place in it).
    member: HashMap<u32, (usize, usize)>,
    chains: Vec<Vec<u32>>,
    /// Each chain's text split over its boxes: per box, items in its unturned frame.
    flows: HashMap<usize, Vec<Vec<Placed>>>,
    /// New entries can be discarded after a sizing probe without cloning placed items.
    flow_order: Vec<usize>,
}

impl Chains {
    fn build(&mut self, doc: &wordcraft_doc::Document) {
        if self.built {
            return;
        }
        self.built = true;
        if !doc.parts.values().any(|p| p.body.next.is_some()) {
            return;
        }
        self.chains = doc.text_box_chains();
        for (ci, c) in self.chains.iter().enumerate() {
            for (k, id) in c.iter().enumerate() {
                self.member.insert(*id, (ci, k));
            }
        }
    }
}

/// The story text box `id` shows: the first box's, for a box linked from another.
pub(crate) fn shown_story(ctx: &mut Ctx, id: u32) -> u32 {
    ctx.chains.build(ctx.doc);
    match ctx.chains.member.get(&id) {
        Some((ci, _)) => ctx.chains.chains.get(*ci).and_then(|c| c.first()).copied().unwrap_or(id),
        None => id,
    }
}

/// Line length and the depth lines stack in, for a `w` × `h` box whose text runs as `body`
/// says, inside its internal margins.
fn frame(body: &TextBody, w: f32, h: f32) -> (f32, f32) {
    let (w, h) = (wordcraft_geom::finite(w), wordcraft_geom::finite(h));
    let [l, t, r, b] = body.insets_pt();
    let (along, across) = match body.vert.turn() {
        TextDirection::Horizontal => (w - l - r, h - t - b),
        _ => (h - t - b, w - l - r),
    };
    (along.max(12.0), across)
}

/// The width stacked text is laid out in (a paragraph's narrowest).
const STACK_WIDTH: f32 = 12.0;

/// Lay `blocks` of story `id` out in lines `len` long; stacked text one letter to a line.
fn lay(ctx: &mut Ctx, id: u32, blocks: &Blocks, len: f32, vert: TextVert, depth: usize) -> Vec<Placed> {
    let story = StoryRef::Part(id);
    if vert == TextVert::Stacked {
        // One letter to a line: words may break anywhere, and each letter takes the narrowest
        // line a paragraph gets (the spacing after it doesn't show).
        let stacked: Blocks = blocks
            .iter()
            .map(|b| match &**b {
                Block::Para(p) => {
                    let mut p = p.clone();
                    p.props.word_wrap = Some(false);
                    for r in &mut p.runs {
                        r.props.spacing = Some(STACK_WIDTH);
                    }
                    p.touch();
                    Arc::new(Block::Para(p))
                }
                _ => b.clone(),
            })
            .collect();
        return layout_box(ctx, story, &stacked, &[], STACK_WIDTH, BoxContext::Other, depth + 1, None).items;
    }
    layout_box(ctx, story, blocks, &[], len, BoxContext::Other, depth + 1, None).items
}

// Allow repeated probe/placement visits, but cap shared graphs separately from the
// 65 actual expansions. Exhaustion keeps the saved frame; this is not a parity bound.
const MAX_MEASURE_BOXES: usize = (wordcraft_doc::BoxBudget::MAX_NESTED + 1) * (1 << wordcraft_doc::MAX_TEXT_BOX_DEPTH);

#[cfg(test)]
thread_local! {
    static MEASURE_COUNTS: std::cell::RefCell<Vec<usize>> = const { std::cell::RefCell::new(Vec::new()) };
    static COPIED_BLOCKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn reset_measure(ctx: &mut Ctx) {
    ctx.measure_left = MAX_MEASURE_BOXES;
    ctx.measure_exhausted = false;
    #[cfg(test)]
    MEASURE_COUNTS.with(|counts| counts.borrow_mut().push(0));
}

/// Charge before preparation or reuse. This allowance is never part of a semantic snapshot.
fn take_measure(ctx: &mut Ctx) -> bool {
    if ctx.measure_depth == 0 {
        return true;
    }
    if ctx.measure_left == 0 {
        ctx.measure_exhausted = true;
        return false;
    }
    ctx.measure_left -= 1;
    #[cfg(test)]
    MEASURE_COUNTS.with(|counts| {
        if let Some(last) = counts.borrow_mut().last_mut() {
            *last = last.saturating_add(1);
        }
    });
    true
}

fn enter_box(ctx: &mut Ctx, id: u32) -> bool {
    if ctx.measure_depth == 0 && !ctx.boxes.inside() {
        reset_measure(ctx);
    }
    take_measure(ctx) && ctx.boxes.enter(id)
}

/// Called only after admission, so rejected probes never copy a long story's block vector.
fn box_blocks(ctx: &Ctx, id: u32) -> Blocks {
    let Some(part) = ctx.doc.parts.get(&id) else { return Vec::new() };
    #[cfg(test)]
    if ctx.measure_depth > 0 {
        COPIED_BLOCKS.with(|count| count.set(count.get().saturating_add(part.blocks.len())));
    }
    part.blocks.clone()
}

/// Speculative layout keeps the active cycle/depth guards, but consumes no expansion or numbering.
/// Only cache entries created by this probe are removed; existing linked flows remain counted.
fn measure(ctx: &mut Ctx, id: u32, len: f32, vert: TextVert, depth: usize) -> Option<Vec<Placed>> {
    if ctx.measure_depth == 0 && !ctx.boxes.inside() {
        reset_measure(ctx);
    }
    let budget = ctx.boxes.clone();
    ctx.measure_depth += 1;
    if !enter_box(ctx, id) {
        ctx.measure_depth -= 1;
        return None;
    }
    let snap = (ctx.counters.clone(), ctx.eq_count);
    let flows = ctx.chains.flow_order.len();
    let fitted = ctx.fitted_order.len();
    let blocks = box_blocks(ctx, id);
    let items = lay(ctx, id, &blocks, len, vert, depth);
    (ctx.counters, ctx.eq_count, ctx.boxes) = (snap.0, snap.1, budget);
    while ctx.chains.flow_order.len() > flows {
        if let Some(key) = ctx.chains.flow_order.pop() {
            ctx.chains.flows.remove(&key);
        }
    }
    // A nested inline probe may have exhausted a temporary budget. Never retain that height.
    while ctx.fitted_order.len() > fitted {
        if let Some(key) = ctx.fitted_order.pop() {
            ctx.fitted.remove(&key);
        }
    }
    ctx.measure_depth -= 1;
    (!ctx.measure_exhausted).then_some(items)
}

/// Measure an unlinked horizontal auto-fit shape before positioning and wrapping it.
pub(super) fn fit_size(ctx: &mut Ctx, obj: &wordcraft_doc::para::InlineObject, w: f32, h: f32, depth: usize) -> (f32, f32) {
    let Some(id) = obj.text_box() else { return (w, h) };
    let Some(part) = ctx.doc.parts.get(&id) else { return (w, h) };
    let body = part.body;
    if !body.auto_fit || body.vert != TextVert::Horz {
        return (w, h);
    }
    ctx.chains.build(ctx.doc);
    if ctx.chains.member.contains_key(&id) {
        return (w, h);
    }
    let (len, _) = frame(&body, w, h);
    let Some(items) = measure(ctx, id, len, body.vert, depth) else { return (w, h) };
    let [_, top, _, bottom] = body.insets_pt();
    // Word fits the frame to the text and both margins, including a visible outline's width.
    let outline = match obj {
        wordcraft_doc::para::InlineObject::Shape { stroke: Some(_), stroke_width, .. } => wordcraft_geom::finite(*stroke_width).max(0.0),
        _ => 0.0,
    };
    (w, extent(&items) + top + bottom + outline)
}

/// The items showing text box `id` in its frame `rect` (page coordinates): its text, or its
/// share of a linked chain's, cut to what fits, aligned and turned as the box says.
pub(crate) fn box_items(ctx: &mut Ctx, id: u32, rect: Rect, depth: usize) -> Vec<Placed> {
    let body = ctx.doc.parts.get(&id).map(|p| p.body).unwrap_or_default();
    let (len, across) = frame(&body, rect.w, rect.h);
    let items = match chain_share(ctx, id, depth) {
        Some(items) => items,
        None => {
            if !enter_box(ctx, id) {
                return Vec::new();
            }
            let blocks = box_blocks(ctx, id);
            let inner = lay(ctx, id, &blocks, len, body.vert, depth);
            ctx.boxes.leave();
            // Text that doesn't fit inside the margins is hidden, as in Word; a box that resizes
            // to fit its text shows it all (#743).
            if body.auto_fit { inner } else { fit_box(inner, across) }
        }
    };
    place(items, &body, rect, across)
}

/// The height of a `w` wide text box `id` that resizes to fit its text (`a:spAutoFit`): its text's
/// depth plus its top and bottom margins, as Word fits it when laying the document out. `None`
/// for a box that keeps its own height: one that doesn't resize, with turned text, or linked.
pub(crate) fn fitted_height(ctx: &mut Ctx, id: u32, w: f32) -> Option<f32> {
    let body = ctx.doc.parts.get(&id).map(|p| p.body)?;
    if !body.auto_fit || body.vert.turn() != TextDirection::Horizontal || body.next.is_some() {
        return None;
    }
    ctx.chains.build(ctx.doc);
    if ctx.chains.member.contains_key(&id) {
        return None;
    }
    let (len, _) = frame(&body, w, 0.0);
    let items = measure(ctx, id, len, body.vert, 0)?;
    let [_, t, _, b] = body.insets_pt();
    let h = extent(&items) + t + b;
    h.is_finite().then_some(h)
}

/// Move items laid out in a box's frame onto the page: aligned in the `across` room (top,
/// middle, bottom; for turned text, from the side its lines start on) and turned.
fn place(mut items: Vec<Placed>, body: &TextBody, rect: Rect, across: f32) -> Vec<Placed> {
    let used = extent(&items);
    let off = match body.anchor {
        VAlign::Top => 0.0,
        VAlign::Center => ((across - used) / 2.0).max(0.0),
        VAlign::Bottom => (across - used).max(0.0),
    };
    let turn = body.vert.turn();
    let [l, t, r, b] = body.insets_pt();
    let (ox, oy) = match turn {
        TextDirection::Horizontal => (rect.x + l, rect.y + t + off),
        // Top-to-bottom lines stack from the right edge, bottom-to-top ones from the left.
        TextDirection::Down => (rect.right() - r - off, rect.y + t),
        TextDirection::Up => (rect.x + l + off, rect.bottom() - b),
    };
    for it in &mut items {
        it.turn(turn, ox, oy);
    }
    items
}

/// How deep laid-out items reach below their frame's top.
fn extent(items: &[Placed]) -> f32 {
    let mut bottom = 0.0f32;
    for it in items {
        let b = match it {
            _ if it.turned_bounds().is_some() => it.turned_bounds().map_or(0.0, |r| r.bottom()),
            Placed::Lines { para, l0, l1, y, .. } => {
                let (Some(first), Some(last)) = (para.lines.get(*l0), l1.checked_sub(1).and_then(|k| para.lines.get(k))) else { continue };
                y + last.top + last.height - first.top
            }
            Placed::Rule { y0, y1, .. } => y0.max(*y1),
            Placed::Fill { rect, .. }
            | Placed::Image { rect, .. }
            | Placed::Shape { rect, .. }
            | Placed::Graphic { rect, .. }
            | Placed::Clip { rect, .. }
            | Placed::Cell { rect, .. }
            | Placed::Object { rect, .. } => rect.bottom(),
        };
        if b.is_finite() {
            bottom = bottom.max(b);
        }
    }
    bottom
}

/// Box `id`'s share of its chain's text (items in its unturned frame), if it is linked.
fn chain_share(ctx: &mut Ctx, id: u32, depth: usize) -> Option<Vec<Placed>> {
    ctx.chains.build(ctx.doc);
    let (ci, k) = *ctx.chains.member.get(&id)?;
    if !take_measure(ctx) {
        return Some(Vec::new());
    }
    if !ctx.chains.flows.contains_key(&ci) {
        let chain = ctx.chains.chains.get(ci).cloned().unwrap_or_default();
        let flow = flow(ctx, &chain, depth);
        if ctx.measure_depth > 0 && ctx.measure_exhausted {
            return Some(Vec::new());
        }
        if ctx.chains.flows.insert(ci, flow).is_none() {
            ctx.chains.flow_order.push(ci);
        }
    }
    Some(ctx.chains.flows.get(&ci).and_then(|f| f.get(k)).cloned().unwrap_or_default())
}

/// Where a box's text starts: the line holding byte `byte` of paragraph `path`.
struct Cut {
    path: Path,
    byte: usize,
}

/// Split the text of `chain`'s first box over its boxes.
fn flow(ctx: &mut Ctx, chain: &[u32], depth: usize) -> Vec<Vec<Placed>> {
    let Some(&head) = chain.first() else { return Vec::new() };
    if !enter_box(ctx, head) {
        return vec![Vec::new(); chain.len()];
    }
    let ids: BTreeSet<u32> = chain.iter().copied().collect();
    let sizes = ctx.doc.text_box_sizes(&ids);
    let blocks = box_blocks(ctx, head);
    // Each box lays the whole story out at its own line length (the same list numbers every
    // time), sharing a layout with the boxes before it of the same length.
    let snap = (ctx.counters.clone(), ctx.eq_count);
    let mut after = None;
    let mut layouts: Vec<((u32, TextVert), Vec<Placed>)> = Vec::new();
    let mut out = Vec::with_capacity(chain.len());
    // Where the next box's text starts: `Some(None)` at the beginning, `None` when all is shown.
    let mut start: Option<Option<Cut>> = Some(None);
    for id in chain {
        // A chain can reflow a long story at many widths, even without entering another box.
        if !take_measure(ctx) {
            ctx.boxes.leave();
            return Vec::new();
        }
        let Some(cut) = start.take() else {
            out.push(Vec::new());
            continue;
        };
        let body = ctx.doc.parts.get(id).map(|p| p.body).unwrap_or_default();
        let (w, h) = sizes.get(id).copied().unwrap_or((0.0, 0.0));
        let (len, across) = frame(&body, w, h);
        let key = (len.to_bits(), body.vert);
        let items = match layouts.iter().find(|(k, _)| *k == key) {
            Some((_, items)) => items.clone(),
            None => {
                (ctx.counters, ctx.eq_count) = (snap.0.clone(), snap.1);
                let items = lay(ctx, head, &blocks, len, body.vert, depth);
                after.get_or_insert_with(|| (ctx.counters.clone(), ctx.eq_count));
                layouts.push((key, items.clone()));
                items
            }
        };
        let items = match &cut {
            None => items,
            Some(cut) => match cut_y(&items, cut) {
                Some(top) => from_y(items, top),
                None => {
                    out.push(Vec::new());
                    continue;
                }
            },
        };
        let (kept, next) = fit_flow(items, across);
        start = next.map(Some);
        out.push(kept);
    }
    if let Some(a) = after {
        (ctx.counters, ctx.eq_count) = a;
    }
    ctx.boxes.leave();
    out
}

/// Where `cut` is in a layout: the top of the line holding its byte.
fn cut_y(items: &[Placed], cut: &Cut) -> Option<f32> {
    items.iter().find_map(|it| match it {
        Placed::Lines { path, para, l0, l1, y, .. } if *path == cut.path => {
            let first = para.lines.get(*l0)?;
            let k = (*l0..*l1).find(|k| para.lines.get(*k).is_some_and(|l| l.stop > cut.byte)).unwrap_or(l1.saturating_sub(1).max(*l0));
            para.lines.get(k).map(|l| y + l.top - first.top)
        }
        _ => None,
    })
}

/// The items from `top` down, moved up to start at 0.
fn from_y(items: Vec<Placed>, top: f32) -> Vec<Placed> {
    let lim = top - 0.5;
    let mut out = Vec::with_capacity(items.len());
    for it in items {
        if let Some(r) = it.turned_bounds() {
            if r.y >= lim {
                out.push(it);
            }
            continue;
        }
        match it {
            Placed::Lines { story, path, para, l0, l1, x, y, turn } => {
                let Some(first) = para.lines.get(l0) else { continue };
                let Some(k) = (l0..l1).find(|k| para.lines.get(*k).is_some_and(|l| y + l.top - first.top >= lim)) else { continue };
                let ky = para.lines.get(k).map_or(y, |l| y + l.top - first.top);
                out.push(Placed::Lines { story, path, para, l0: k, l1, x, y: ky, turn });
            }
            Placed::Fill { rect, color } if rect.bottom() > lim => {
                let y0 = rect.y.max(top);
                out.push(Placed::Fill { rect: Rect::new(rect.x, y0, rect.w, rect.bottom() - y0), color });
            }
            Placed::Rule { y0, y1, .. } if y0.max(y1) >= lim => out.push(it),
            Placed::Image { rect, .. }
            | Placed::Shape { rect, .. }
            | Placed::Graphic { rect, .. }
            | Placed::Clip { rect, .. }
            | Placed::Cell { rect, .. }
            | Placed::Object { rect, .. }
                if rect.y >= lim =>
            {
                out.push(it)
            }
            _ => {}
        }
    }
    for it in &mut out {
        it.translate(0.0, -top);
    }
    out
}

/// What fits in `across` (see [`fit_box`]), and where the text left over starts.
fn fit_flow(items: Vec<Placed>, across: f32) -> (Vec<Placed>, Option<Cut>) {
    let limit = across + 0.5;
    let mut kept_line = false;
    let mut cut: Option<(f32, Cut)> = None;
    for it in &items {
        if it.turned_bounds().is_some() {
            continue;
        }
        let Placed::Lines { path, para, l0, l1, y, .. } = it else { continue };
        let Some(first) = para.lines.get(*l0) else { continue };
        for k in *l0..*l1 {
            let Some(l) = para.lines.get(k) else { break };
            let ly = y + l.top - first.top;
            if ly + l.height > limit && kept_line {
                if cut.as_ref().is_none_or(|(cy, _)| ly < *cy) {
                    cut = Some((ly, Cut { path: path.clone(), byte: l.start }));
                }
                break;
            }
            kept_line = true;
        }
    }
    (fit_box(items, across), cut.map(|(_, c)| c))
}

#[cfg(test)]
mod probe_work_tests {
    use super::*;
    use wordcraft_doc::para::{Float, InlineObject, ShapeKind, Wrap};
    use wordcraft_doc::{Document, Paragraph, PartKind, Pos, para_block};

    fn shape(id: u32, w: f32) -> InlineObject {
        InlineObject::Shape {
            kind: ShapeKind::TextBox,
            w,
            h: 2000.0,
            fill: None,
            stroke: None,
            stroke_width: 0.0,
            float: Float { wrap: Wrap::InFrontOfText, ..Default::default() },
            story: Some(id),
            effects: Default::default(),
            freeform: None,
            extra: Default::default(),
        }
    }

    #[derive(Clone, Copy, Debug)]
    enum Graph {
        Shared,
        Varied,
        Linked,
        Long,
    }

    #[derive(Clone, Copy)]
    enum Fit {
        Fixed,
        Auto,
    }

    fn shared_graph(graph: Graph, fit: Fit) -> (Document, u32) {
        let mut d = Document::from_text("");
        d.styles.default_chr.font = Some("Source Sans 3".into());
        let ids: Vec<_> =
            (0..4).map(|k| d.add_part(PartKind::TextBox, vec![para_block(Paragraph::with_text(&format!("Level{k}"), Default::default()))])).collect();
        for id in &ids {
            d.parts.get_mut(id).unwrap().body = TextBody { auto_fit: matches!(fit, Fit::Auto), inset: Some([0; 4]), ..Default::default() };
        }
        if matches!(graph, Graph::Long) {
            d.parts.get_mut(&ids[3]).unwrap().blocks = Document::from_text(&"Long story line.\n".repeat(63)).body;
        }
        if matches!(graph, Graph::Linked) {
            let tail = d.add_part(PartKind::TextBox, vec![para_block(Paragraph::with_text("", Default::default()))]);
            d.parts.get_mut(&ids[3]).unwrap().body.next = Some(tail);
            d.insert_object(&Pos::new(StoryRef::Part(ids[2]), Path::top(0), 0), shape(tail, 180.0), &Default::default()).unwrap();
        }
        for pair in ids.windows(2) {
            for k in 0..wordcraft_doc::BoxBudget::MAX_NESTED {
                let w = if matches!(graph, Graph::Shared) { 180.0 } else { 120.0 + (k % 3) as f32 * 8.0 };
                d.insert_object(&Pos::new(StoryRef::Part(pair[0]), Path::top(0), 0), shape(pair[1], w), &Default::default()).unwrap();
            }
        }
        d.insert_object(&Pos::body(0, 0), shape(ids[0], 240.0), &Default::default()).unwrap();
        (d, ids[0])
    }

    fn layout(d: &Document) -> crate::DocLayout {
        crate::layout(d, &mut crate::LayoutCache::new(), &crate::LayoutOptions::default())
    }

    fn visible(l: &crate::DocLayout) -> HashMap<StoryRef, usize> {
        let mut out = HashMap::new();
        for item in l.pages.iter().flat_map(|p| &p.items) {
            if let Placed::Lines { story, l0, l1, .. } = item {
                *out.entry(*story).or_default() += l1 - l0;
            }
        }
        out
    }

    #[test]
    fn shared_probe_work_is_capped() {
        for graph in [Graph::Shared, Graph::Varied, Graph::Linked, Graph::Long] {
            let (control, _) = shared_graph(graph, Fit::Fixed);
            let expected = layout(&control);
            let (d, root) = shared_graph(graph, Fit::Auto);
            MEASURE_COUNTS.with(|counts| counts.borrow_mut().clear());
            COPIED_BLOCKS.with(|count| count.set(0));
            let l = layout(&d);
            let counts = MEASURE_COUNTS.with(|counts| std::mem::take(&mut *counts.borrow_mut()));
            assert!(counts.contains(&MAX_MEASURE_BOXES), "the hostile graph exhausts measurement work: {counts:?}");
            assert!(counts.len() <= 2 && counts.iter().all(|n| *n <= MAX_MEASURE_BOXES), "{counts:?}");
            assert!(counts.iter().sum::<usize>() <= 2 * MAX_MEASURE_BOXES);
            let copied = COPIED_BLOCKS.with(std::cell::Cell::get);
            let max_blocks = d.parts.values().map(|p| p.blocks.len()).max().unwrap();
            assert!(copied <= counts.iter().sum::<usize>() * max_blocks, "preparation only follows charged work: {copied}");
            assert_eq!(visible(&l), visible(&expected), "probe exhaustion never spends actual text-placement allowance");
            assert_eq!(l.text_boxes, expected.text_boxes);
            assert!(l.text_boxes <= 1 + wordcraft_doc::BoxBudget::MAX_NESTED);
            let frame = l
                .pages
                .iter()
                .flat_map(|p| &p.items)
                .find_map(|it| match it {
                    Placed::Object { rect, text_box: Some(id), .. } if *id == root => Some(*rect),
                    _ => None,
                })
                .unwrap();
            assert_eq!((frame.w, frame.h), (240.0, 2000.0), "an incomplete probe keeps its saved frame");
            eprintln!("PROBE_WORK graph={graph:?} phases={counts:?} copied_blocks={copied} actual_boxes={}", l.text_boxes);
        }
    }

    #[test]
    fn rejected_probe_copies_no_story() {
        let (d, _) = shared_graph(Graph::Linked, Fit::Auto);
        let mut cache = crate::LayoutCache::new();
        let opts = crate::LayoutOptions::default();
        let prior_wraps = crate::PriorBodyWraps::new();
        let mut back_wraps = crate::BackBodyWraps::new();
        let mut ctx = Ctx {
            doc: &d,
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
            measure_left: 3,
            measure_depth: 1,
            measure_exhausted: false,
        };
        ctx.chains.build(ctx.doc);
        let head = *ctx.chains.chains[0].first().unwrap();
        MEASURE_COUNTS.with(|counts| *counts.borrow_mut() = vec![0]);
        COPIED_BLOCKS.with(|count| count.set(0));
        // Chain copy, head entry and first width consume three units. The next width is rejected.
        assert!(chain_share(&mut ctx, head, 0).unwrap().is_empty());
        assert!(ctx.measure_exhausted);
        assert_eq!(ctx.measure_left, 0);
        assert!(ctx.chains.flows.is_empty(), "partial linked flow must not be cached");
        assert!(!ctx.boxes.inside(), "an interrupted flow leaves its head");
        let copied = COPIED_BLOCKS.with(std::cell::Cell::get);
        assert_eq!(copied, 1);
        let obj = shape(1, 240.0);
        for _ in 0..2000 {
            assert!(chain_share(&mut ctx, head, 0).unwrap().is_empty());
            assert_eq!(fit_size(&mut ctx, &obj, 240.0, 2000.0, 0), (240.0, 2000.0));
        }
        assert_eq!(COPIED_BLOCKS.with(std::cell::Cell::get), copied, "rejected probes never clone Blocks");
        assert_eq!(MEASURE_COUNTS.with(|counts| counts.borrow()[0]), 3);
    }

    fn with_probe_ctx(d: &Document, f: impl FnOnce(&mut Ctx)) {
        let mut cache = crate::LayoutCache::new();
        let opts = crate::LayoutOptions::default();
        let prior_wraps = crate::PriorBodyWraps::new();
        let mut back_wraps = crate::BackBodyWraps::new();
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
        f(&mut ctx);
    }

    fn inline_shape(id: u32, w: f32) -> InlineObject {
        let mut obj = shape(id, w);
        if let InlineObject::Shape { float, .. } = &mut obj {
            float.wrap = Wrap::Inline;
        }
        obj
    }

    #[test]
    fn root_none_is_reused() {
        let (mut d, root) = shared_graph(Graph::Shared, Fit::Auto);
        let p = d.para_mut(StoryRef::Body, &Path::top(0)).unwrap();
        p.objects[0] = inline_shape(root, 240.0);
        p.touch();
        let p = d.para(StoryRef::Body, &Path::top(0)).unwrap();
        with_probe_ctx(&d, |ctx| {
            MEASURE_COUNTS.with(|counts| counts.borrow_mut().clear());
            COPIED_BLOCKS.with(|count| count.set(0));
            let first = ctx.para_labelled(p, 500.0, None, &[], None);
            let before = MEASURE_COUNTS.with(|counts| counts.borrow().clone());
            assert_eq!(before, [MAX_MEASURE_BOXES]);
            assert!(ctx.measure_exhausted);
            let copied = COPIED_BLOCKS.with(std::cell::Cell::get);
            for _ in 0..4 {
                let repeated = ctx.para_labelled(p, 500.0, None, &[], None);
                assert!(Arc::ptr_eq(&first, &repeated));
                let measured = ctx.para_measured(p, 500.0, None, None, |pl| (pl.height, pl.height));
                assert_eq!(measured, (first.height, first.height));
            }
            let after = MEASURE_COUNTS.with(|counts| counts.borrow().clone());
            eprintln!(
                "ROOT_PROBE_WORK before={before:?} after={after:?} copied_before={copied} copied_after={}",
                COPIED_BLOCKS.with(std::cell::Cell::get)
            );
            assert_eq!(after, before, "repeated layout and table measurement reuse the exhausted root result");
            assert_eq!(COPIED_BLOCKS.with(std::cell::Cell::get), copied);
            let key = (root, 240.0f32.to_bits());
            assert_eq!(ctx.fitted.get(&key), Some(&None));
            assert_eq!(ctx.fitted_order, [key]);
            assert_eq!(ctx.measure_depth, 0);
            assert!(!ctx.boxes.inside());
            assert_eq!(ctx.boxes.total(), 0);
            assert!(ctx.chains.flows.is_empty());
        });
        let mut fixed = d.clone();
        for part in fixed.parts.values_mut() {
            part.body.auto_fit = false;
        }
        let expected = layout(&fixed);
        let actual = layout(&d);
        assert_eq!(visible(&actual), visible(&expected));
        assert_eq!(actual.text_boxes, expected.text_boxes);
        assert!(actual.text_boxes <= 1 + wordcraft_doc::BoxBudget::MAX_NESTED);
    }

    #[derive(Clone, Copy, Debug)]
    enum Nest {
        ActualBox,
        Probe,
    }

    #[test]
    fn nested_none_is_not_cached() {
        let mut d = Document::from_text("");
        d.styles.default_chr.font = Some("Source Sans 3".into());
        let list = d.numbering.add_list(wordcraft_doc::ListKind::Numbered);
        let parent = d.add_part(PartKind::TextBox, vec![]);
        let leaf = d.add_part(PartKind::TextBox, Document::from_text("Visible child").body);
        let root = d.add_part(PartKind::TextBox, Document::from_text("Numbered root").body);
        for id in [root, leaf] {
            d.parts.get_mut(&id).unwrap().body = TextBody { auto_fit: true, inset: Some([0; 4]), ..Default::default() };
        }
        d.para_mut(StoryRef::Part(root), &Path::top(0)).unwrap().props.numbering = Some(wordcraft_doc::props::NumRef { num: list, level: 0 });
        d.insert_object(&Pos::new(StoryRef::Part(root), Path::top(0), 0), inline_shape(leaf, 180.0), &Default::default()).unwrap();
        d.insert_object(&Pos::body(0, 0), inline_shape(root, 240.0), &Default::default()).unwrap();
        let p = d.para(StoryRef::Body, &Path::top(0)).unwrap();
        for nest in [Nest::ActualBox, Nest::Probe] {
            with_probe_ctx(&d, |ctx| {
                match nest {
                    Nest::ActualBox => assert!(ctx.boxes.enter(parent)),
                    Nest::Probe => ctx.measure_depth = 1,
                }
                ctx.eq_count = 7;
                let before = (ctx.boxes.total(), ctx.measure_depth);
                ctx.measure_left = 1;
                MEASURE_COUNTS.with(|counts| *counts.borrow_mut() = vec![0]);
                assert!(ctx.fitted_boxes(p).is_empty());
                assert!(ctx.measure_exhausted);
                assert_eq!(ctx.measure_left, 0);
                assert_eq!((ctx.boxes.total(), ctx.measure_depth), before);
                assert_eq!(ctx.boxes.inside(), matches!(nest, Nest::ActualBox));
                assert_eq!(ctx.eq_count, 7);
                assert!(ctx.fitted.is_empty(), "exhausted {nest:?} must not poison the fitted cache");
                assert!(ctx.fitted_order.is_empty());
                assert!(ctx.chains.flows.is_empty());
                let mut counters = ctx.counters.clone();
                assert_eq!(counters.next_label(&d.numbering, list, 0).unwrap().0, "1.");
                match nest {
                    Nest::ActualBox => ctx.boxes.leave(),
                    Nest::Probe => ctx.measure_depth = 0,
                }
                let measured = ctx.fitted_boxes(p);
                assert_eq!(measured.len(), 1, "a fresh root must recover from exhausted {nest:?}");
                assert!(measured[0].1 > 0.0);
                assert!(!ctx.measure_exhausted);
                let key = (root, 240.0f32.to_bits());
                assert_eq!(ctx.fitted.get(&key), Some(&Some(measured[0].1)));
                assert_eq!(ctx.fitted_order, [key]);
                assert_eq!(ctx.eq_count, 7);
                let mut counters = ctx.counters.clone();
                assert_eq!(counters.next_label(&d.numbering, list, 0).unwrap().0, "1.");
                eprintln!(
                    "NESTED_PROBE_CACHE context={nest:?} exhausted_uncached=true fresh_height={} phases={:?}",
                    measured[0].1,
                    MEASURE_COUNTS.with(|counts| counts.borrow().clone())
                );
            });
        }
    }
}
