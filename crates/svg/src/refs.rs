//! Which clip paths, masks, patterns, filters and markers an SVG's own `url(#…)` links make refer
//! to each other (#1910 review 6). usvg builds each of them by building what they hold, and
//! recurses without limit when that leads back to them, through more than one hop or across
//! kinds: a stack overflow, which aborts the process. This builds the graph of those references
//! (without recursion, and bounded) so the pre-check can refuse a loop, and can count how often
//! each one is built along the graph.

use std::collections::HashMap;

use super::{SVG_NS, link};

type Node<'a, 'i> = usvg::roxmltree::Node<'a, 'i>;

/// Most clip paths, masks, patterns, filters and markers a picture may have.
const MAX_SPECIALS: usize = 1 << 19;
/// Most references between them.
const MAX_EDGES: usize = 1 << 22;
/// Most elements, links and style rules looked at while finding the references.
const MAX_STEPS: u64 = 1 << 25;
/// Longest rule selector read (the matcher recurses along a selector's parts).
const MAX_SELECTOR: usize = 2048;
/// Most rules naming a `url(` that are matched.
const MAX_URL_RULES: usize = 4096;

/// Why no graph was made.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// Some of them lead back to themselves.
    Loop,
    /// More than the bounds above.
    TooBig,
}

/// The clip paths, masks, patterns, filters and markers of a picture, and who refers to whom.
pub(crate) struct Graph {
    /// Each one's node id in the XML document.
    pub nodes: Vec<usize>,
    /// Whether each is a marker (drawn per vertex rather than per element).
    pub marker: Vec<bool>,
    /// Whether anything in the file names it.
    pub named: Vec<bool>,
    /// Who each one refers to (what it holds, inherits, names itself or links to).
    pub edges: Vec<Vec<u32>>,
    /// Every one after all that refer to it.
    pub order: Vec<u32>,
}

fn is_special(n: Node) -> bool {
    n.is_element()
        && matches!(n.tag_name().namespace(), None | Some(SVG_NS))
        && matches!(n.tag_name().name(), "marker" | "clipPath" | "mask" | "pattern" | "filter")
}

/// The properties an element passes on to what it holds.
fn inherited(name: &str) -> bool {
    matches!(name, "fill" | "stroke" | "marker-start" | "marker-mid" | "marker-end" | "marker")
}

/// The ids `v` names with `url(#id)` (any case, quotes and spaces allowed).
fn url_ids<'s>(v: &'s str, mut f: impl FnMut(&'s str)) {
    let b = v.as_bytes();
    let mut i = 0usize;
    while let Some(p) = b.get(i..).and_then(|r| r.windows(4).position(|w| w.eq_ignore_ascii_case(b"url("))) {
        let start = i + p + 4;
        i = start;
        let Some(rest) = v.get(start..) else { continue };
        let rest = rest.trim_start_matches(|c: char| c.is_whitespace() || c == '"' || c == '\'');
        let Some(rest) = rest.strip_prefix('#') else { continue };
        let end = rest.find(|c: char| c == ')' || c == '"' || c == '\'' || c.is_whitespace()).unwrap_or(rest.len());
        if let Some(id) = rest.get(..end) {
            f(id);
        }
    }
}

/// An element, for the CSS selector matcher.
#[derive(Clone, Copy)]
struct Css<'a, 'i>(Node<'a, 'i>);

impl simplecss::Element for Css<'_, '_> {
    fn parent_element(&self) -> Option<Self> {
        self.0.parent_element().map(Css)
    }
    fn prev_sibling_element(&self) -> Option<Self> {
        self.0.prev_sibling_element().map(Css)
    }
    fn has_local_name(&self, name: &str) -> bool {
        self.0.tag_name().name() == name
    }
    fn attribute_matches(&self, local_name: &str, operator: simplecss::AttributeOperator<'_>) -> bool {
        self.0.attribute(local_name).is_some_and(|v| operator.matches(v))
    }
    fn pseudo_class_matches(&self, class: simplecss::PseudoClass<'_>) -> bool {
        matches!(class, simplecss::PseudoClass::FirstChild) && self.0.prev_sibling_element().is_none()
    }
}

/// A style sheet rule that names ids: `(name, ids)` for each such declaration.
struct Rule<'a> {
    selector: simplecss::Selector<'a>,
    names: Vec<(&'a str, Vec<&'a str>)>,
}

/// The graph for `xml`. `ids` maps an id to the first element holding it.
pub(crate) fn graph<'a, 'i>(xml: &'a usvg::roxmltree::Document<'i>, ids: &HashMap<&str, Node<'a, 'i>>) -> Result<Graph, Refusal> {
    let all: Vec<Node> = xml.descendants().collect();
    let len = all.iter().map(|n| n.id().get_usize() + 1).max().unwrap_or(0);
    let mut special_at = vec![u32::MAX; len];
    let mut nodes = Vec::new();
    let mut marker = Vec::new();
    let mut specials: Vec<Node> = Vec::new();
    for n in &all {
        if is_special(*n) {
            if specials.len() >= MAX_SPECIALS {
                return Err(Refusal::TooBig);
            }
            if let Some(s) = special_at.get_mut(n.id().get_usize()) {
                *s = specials.len() as u32;
            }
            nodes.push(n.id().get_usize());
            marker.push(n.tag_name().name() == "marker");
            specials.push(*n);
        }
    }
    let n = specials.len();
    let mut named = vec![false; n];
    let mut edges: Vec<Vec<u32>> = vec![Vec::new(); n];
    if n == 0 {
        return Ok(Graph { nodes, marker, named, edges, order: Vec::new() });
    }
    let special_named = |id: &str| ids.get(id).and_then(|e| special_at.get(e.id().get_usize()).copied()).filter(|s| *s != u32::MAX);

    // Style sheet rules that name ids. (The sheet is matched as usvg does, so an id a rule names
    // is reached only from the elements the rule's selector matches.)
    let sheet_text: Vec<&str> = all
        .iter()
        .filter(|e| e.is_element() && matches!(e.tag_name().namespace(), None | Some(SVG_NS)) && e.tag_name().name() == "style")
        .flat_map(|e| e.descendants())
        .filter_map(|d| d.text().filter(|_| d.is_text()))
        .collect();
    let mut sheet = simplecss::StyleSheet::new();
    for t in &sheet_text {
        // The text before a `{` is a selector; long ones are refused rather than matched.
        let mut run = 0usize;
        for c in t.bytes() {
            match c {
                b'{' | b'}' => run = 0,
                _ => run += 1,
            }
            if run > MAX_SELECTOR {
                return Err(Refusal::TooBig);
            }
        }
        sheet.parse_more(t);
    }
    let mut rules: Vec<Rule> = Vec::new();
    for r in &sheet.rules {
        let names: Vec<(&str, Vec<&str>)> = r
            .declarations
            .iter()
            .filter_map(|d| {
                let mut v = Vec::new();
                url_ids(d.value, |id| v.push(id));
                (!v.is_empty()).then_some((d.name, v))
            })
            .collect();
        if names.is_empty() {
            continue;
        }
        for (_, v) in &names {
            for id in v {
                if let Some(s) = special_named(id)
                    && let Some(f) = named.get_mut(s as usize)
                {
                    *f = true;
                }
            }
        }
        if rules.len() >= MAX_URL_RULES {
            return Err(Refusal::TooBig);
        }
        rules.push(Rule { selector: r.selector.clone(), names });
    }

    // What the file names anywhere: `url(#…)` in attributes, and links to patterns and filters.
    for e in &all {
        if !(e.is_element() && matches!(e.tag_name().namespace(), None | Some(SVG_NS))) {
            continue;
        }
        for a in e.attributes() {
            if a.name() == "href" {
                if let Some(s) = svgtypes::IRI::from_str(a.value()).ok().and_then(|i| special_named(i.0))
                    && let Some(f) = named.get_mut(s as usize)
                {
                    *f = true;
                }
            } else {
                url_ids(a.value(), |id| {
                    if let Some(s) = special_named(id)
                        && let Some(f) = named.get_mut(s as usize)
                    {
                        *f = true;
                    }
                });
            }
        }
    }

    // Edges: from each one to the ones its content, ancestors, own attributes and links name.
    let mut steps = 0u64;
    let mut total_edges = 0usize;
    let mut stamp = vec![0u32; len];
    for (si, s) in specials.iter().enumerate() {
        let mut out: Vec<u32> = Vec::new();
        // The ids `e` names itself (attributes, `style`, and matching rules); only the properties
        // that pass down to what an element holds if `down`.
        let collect = |e: Node, down: bool, out: &mut Vec<u32>, steps: &mut u64| -> Result<(), Refusal> {
            *steps = steps.saturating_add(1).saturating_add(rules.len() as u64);
            if *steps > MAX_STEPS {
                return Err(Refusal::TooBig);
            }
            // usvg removes a pattern's own fill or stroke, and a clip path's, mask's or filter's
            // own clip path, mask or filter, from what it holds when it names the element
            // itself, so such a link is no loop (an inherited one is not removed).
            let removed = |name: &str| match s.tag_name().name() {
                "pattern" => matches!(name, "fill" | "stroke"),
                "clipPath" => name == "clip-path",
                other => name == other.to_ascii_lowercase() && matches!(other, "mask" | "filter"),
            };
            let mut push = |id: &str, name: &str| {
                if let Some(t) = special_named(id)
                    && !(t as usize == si && !down && removed(name))
                {
                    out.push(t);
                }
            };
            for a in e.attributes() {
                if a.name() == "style" {
                    for d in a.value().split(';') {
                        if let Some((k, v)) = d.split_once(':')
                            && (!down || inherited(k.trim()))
                        {
                            let k = k.trim();
                            url_ids(v, |id| push(id, k));
                        }
                    }
                } else if a.name() != "href" && (!down || inherited(a.name())) {
                    url_ids(a.value(), |id| push(id, a.name()));
                }
            }
            for r in &rules {
                if r.selector.matches(&Css(e)) {
                    for (name, v) in &r.names {
                        if !down || inherited(name) {
                            v.iter().for_each(|id| push(id, name));
                        }
                    }
                }
            }
            Ok(())
        };
        // Ancestors pass their inheritable properties down to it.
        for a in s.ancestors().skip(1) {
            if a.is_element() {
                collect(a, true, &mut out, &mut steps)?;
            }
        }
        let mut stack = vec![*s];
        if let Some(m) = stamp.get_mut(s.id().get_usize()) {
            *m = si as u32 + 1;
        }
        while let Some(e) = stack.pop() {
            collect(e, false, &mut out, &mut steps)?;
            let name = e.tag_name().name();
            // A pattern or filter built from a template, a `<use>` or filter image drawing what it
            // links to.
            let follows = matches!(name, "use" | "feImage") || (e == *s && matches!(name, "pattern" | "filter"));
            // A link to a special is a reference; one that merely stands inside this one is not
            // built with it, and is skipped below.
            let mut next: Vec<Node> = Vec::new();
            if follows && let Some(t) = link(e, ids) {
                match special_at.get(t.id().get_usize()).copied().filter(|t| *t != u32::MAX) {
                    Some(t) => out.push(t),
                    None => next.push(t),
                }
            }
            for c in e.children() {
                if c.is_element()
                    && matches!(c.tag_name().namespace(), None | Some(SVG_NS))
                    && c.tag_name().name() != "style"
                    && special_at.get(c.id().get_usize()) == Some(&u32::MAX)
                {
                    next.push(c);
                }
            }
            for c in next {
                if let Some(m) = stamp.get_mut(c.id().get_usize())
                    && *m != si as u32 + 1
                {
                    *m = si as u32 + 1;
                    steps = steps.saturating_add(1);
                    if steps > MAX_STEPS {
                        return Err(Refusal::TooBig);
                    }
                    stack.push(c);
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        total_edges = total_edges.saturating_add(out.len());
        if total_edges > MAX_EDGES {
            return Err(Refusal::TooBig);
        }
        if let Some(slot) = edges.get_mut(si) {
            *slot = out;
        }
        for t in edges.get(si).into_iter().flatten() {
            if let Some(f) = named.get_mut(*t as usize) {
                *f = true;
            }
        }
    }

    // Loops, by a depth-first walk with an explicit stack. Colours: 0 new, 1 on the path, 2 done.
    // usvg stops a marker from being built inside itself, so a loop through a marker ends there;
    // the edge that closes such a loop is left out. Any other loop is refused.
    let mut colour = vec![0u8; n];
    let mut order: Vec<u32> = Vec::with_capacity(n);
    let mut back: Vec<(usize, u32)> = Vec::new();
    // Where each node on the path stands in the stack.
    let mut at = vec![0usize; n];
    for root in 0..n {
        if colour.get(root) != Some(&0) {
            continue;
        }
        // (node, next edge, markers on the path up to and including it)
        let is_marker = |v: usize| usize::from(marker.get(v) == Some(&true));
        let mut stack: Vec<(usize, usize, usize)> = vec![(root, 0, is_marker(root))];
        if let (Some(c), Some(a)) = (colour.get_mut(root), at.get_mut(root)) {
            *c = 1;
            *a = 0;
        }
        while let Some(&(v, k, marks)) = stack.last() {
            let next = edges.get(v).and_then(|e| e.get(k)).copied();
            match next {
                Some(t) => {
                    if let Some(top) = stack.last_mut() {
                        top.1 = k + 1;
                    }
                    match colour.get(t as usize).copied() {
                        Some(0) => {
                            if let (Some(c), Some(a)) = (colour.get_mut(t as usize), at.get_mut(t as usize)) {
                                *c = 1;
                                *a = stack.len();
                            }
                            stack.push((t as usize, 0, marks + is_marker(t as usize)));
                        }
                        Some(1) => {
                            // The loop is the path from `t` to here; markers on it, counted
                            // from the path's running total.
                            let before = at.get(t as usize).and_then(|i| i.checked_sub(1)).and_then(|i| stack.get(i)).map_or(0, |e| e.2);
                            if marks > before || marker.get(t as usize) == Some(&true) {
                                back.push((v, t));
                            } else {
                                return Err(Refusal::Loop);
                            }
                        }
                        _ => {}
                    }
                }
                None => {
                    if let Some(c) = colour.get_mut(v) {
                        *c = 2;
                    }
                    order.push(v as u32);
                    stack.pop();
                }
            }
        }
    }
    for (v, t) in back {
        if let Some(e) = edges.get_mut(v) {
            e.retain(|x| *x != t);
        }
    }
    // Finished last = refers to the others: a referrer comes after what it refers to.
    order.reverse();
    Ok(Graph { nodes, marker, named, edges, order })
}
