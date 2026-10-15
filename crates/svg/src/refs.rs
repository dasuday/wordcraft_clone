//! Which clip paths, masks, patterns, filters and markers an SVG's own `url(#…)` links make refer
//! to each other (#1910 review 6). usvg builds each of them by building what they hold, and
//! recurses without limit when that leads back to them, through more than one hop or across
//! kinds: a stack overflow, which aborts the process. This builds the graph of those references
//! (without recursion, and bounded) so the pre-check can refuse a loop, and can count how often
//! each one is built along the graph.

use std::collections::HashMap;

use super::SVG_NS;

type Node<'a, 'i> = usvg::roxmltree::Node<'a, 'i>;

/// Most clip paths, masks, patterns, filters and markers a picture may have.
const MAX_SPECIALS: usize = 1 << 19;
/// Most references between them.
const MAX_EDGES: usize = 1 << 22;
/// Most elements, links and style rules looked at while finding the references.
const MAX_STEPS: u64 = 1 << 25;
/// Most combinators one selector may have (the matcher recurses along them).
const MAX_COMBINATORS: usize = 32;
/// Most matching work the style sheets may cost: for each selector, the elements times the
/// ancestors it may try for each descendant combinator.
const MAX_MATCH_WORK: u64 = 1 << 30;
/// Most rules naming a `url(` that are matched.
const MAX_URL_RULES: usize = 4096;

/// Why no graph was made.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// Some of them lead back to themselves.
    Loop,
    /// More than the bounds above.
    TooBig,
    /// A `style` attribute names an id in text the CSS reader can't parse.
    Unreadable,
    /// A style sheet selector that matching would recurse or backtrack too far on.
    Selectors,
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
    /// Edges left out because they close a loop through a marker (usvg builds a marker inside
    /// itself once at most): `(from, to)`.
    pub closing: Vec<(u32, u32)>,
}

fn is_special(n: Node) -> bool {
    n.is_element()
        && matches!(n.tag_name().namespace(), None | Some(SVG_NS))
        && matches!(n.tag_name().name(), "marker" | "clipPath" | "mask" | "pattern" | "filter" | "linearGradient" | "radialGradient")
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

/// The `id`s in `v` named by `url(#id)`.
fn url_count(v: &str) -> usize {
    let mut n = 0usize;
    url_ids(v, |_| n += 1);
    n
}

/// A style sheet rule that names ids: `(name, ids)` for each such declaration.
struct Rule<'a> {
    selector: simplecss::Selector<'a>,
    names: Vec<(&'a str, Vec<&'a str>)>,
}

/// Elements called `style` in any namespace (usvg reads sheets by local name).
fn style_texts<'a>(xml: &'a usvg::roxmltree::Document<'_>) -> Vec<&'a str> {
    xml.descendants()
        .filter(|e| e.is_element() && e.tag_name().name() == "style")
        .flat_map(|e| e.descendants())
        .filter_map(|d| d.text().filter(|_| d.is_text()))
        .collect()
}

/// The selectors in a style sheet's text: what comes before each `{` outside a block, with
/// comments, strings, at-rules and declaration blocks left out.
fn preludes(text: &str) -> Result<Vec<String>, Refusal> {
    let b = text.as_bytes();
    let (mut out, mut cur): (Vec<String>, Vec<u8>) = (Vec::new(), Vec::new());
    let (mut depth, mut i) = (0usize, 0usize);
    while let Some(&c) = b.get(i) {
        i += 1;
        match c {
            b'/' if b.get(i) == Some(&b'*') => {
                i = b.get(i + 1..).and_then(|r| r.windows(2).position(|w| w == b"*/")).map_or(b.len(), |p| i + 1 + p + 2);
            }
            b'"' | b'\'' => {
                // A string: to its closing quote (backslash escapes the next byte).
                while let Some(&d) = b.get(i) {
                    i += 1;
                    if d == b'\\' {
                        i += 1;
                    } else if d == c {
                        break;
                    }
                }
                if depth == 0 {
                    cur.push(b' ');
                }
            }
            b'{' => {
                if depth == 0 && !cur.trim_ascii().starts_with(b"@") {
                    out.push(String::from_utf8_lossy(cur.trim_ascii()).into_owned());
                }
                if depth == 0 {
                    cur.clear();
                }
                depth += 1;
            }
            b'}' => depth = depth.saturating_sub(1),
            b';' if depth == 0 => cur.clear(),
            _ if depth == 0 => {
                cur.push(c);
                if cur.len() > (1 << 20) {
                    return Err(Refusal::Selectors);
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

/// Refuses style sheets whose selectors usvg's matcher (simplecss) would overflow the stack on
/// (it recurses once per combinator) or take forever on (it tries every ancestor for each
/// descendant combinator). Reads every `style` element, in any namespace.
pub(crate) fn check_css(xml: &usvg::roxmltree::Document<'_>) -> Result<(), Refusal> {
    let sheets = style_texts(xml);
    if sheets.is_empty() {
        return Ok(());
    }
    let (mut depth, mut deepest, mut count) = (HashMap::<usize, u64>::new(), 0u64, 0u64);
    for n in xml.descendants() {
        count += 1;
        if n.is_element() {
            let d = n.parent().and_then(|p| depth.get(&p.id().get_usize()).copied()).map_or(1, |d| d + 1);
            depth.insert(n.id().get_usize(), d);
            deepest = deepest.max(d);
        }
    }
    let mut work = 0u64;
    for t in sheets {
        for sel in preludes(t)? {
            for one in sel.split(',') {
                let (mut combinators, mut descendants) = (0usize, 0u32);
                for tok in simplecss::SelectorTokenizer::from(one) {
                    match tok {
                        Ok(simplecss::SelectorToken::DescendantCombinator) => {
                            combinators += 1;
                            descendants += 1;
                        }
                        Ok(simplecss::SelectorToken::ChildCombinator | simplecss::SelectorToken::AdjacentCombinator) => combinators += 1,
                        Ok(_) => {}
                        Err(_) => break,
                    }
                    if combinators > MAX_COMBINATORS {
                        return Err(Refusal::Selectors);
                    }
                }
                let mut tries = count;
                for _ in 0..descendants {
                    tries = tries.saturating_mul(deepest);
                    if tries > MAX_MATCH_WORK {
                        return Err(Refusal::Selectors);
                    }
                }
                work = work.saturating_add(tries);
                if work > MAX_MATCH_WORK {
                    return Err(Refusal::Selectors);
                }
            }
        }
    }
    Ok(())
}

/// A depth-first walk with an explicit stack over `edges` (leaving out nodes `skip` names).
/// Returns the nodes in the order they finish, and the edges that lead back to a node on the
/// walk's path (each closes a loop).
fn walk(edges: &[Vec<u32>], skip: &dyn Fn(usize) -> bool) -> (Vec<u32>, Vec<(usize, u32)>) {
    let n = edges.len();
    let mut colour = vec![0u8; n]; // 0 new, 1 on the path, 2 done
    let (mut order, mut back) = (Vec::with_capacity(n), Vec::new());
    for root in 0..n {
        if skip(root) || colour.get(root) != Some(&0) {
            continue;
        }
        let mut stack: Vec<(usize, usize)> = vec![(root, 0)];
        if let Some(c) = colour.get_mut(root) {
            *c = 1;
        }
        while let Some(&(v, k)) = stack.last() {
            match edges.get(v).and_then(|e| e.get(k)).copied() {
                Some(t) => {
                    if let Some(top) = stack.last_mut() {
                        top.1 = k + 1;
                    }
                    if skip(t as usize) {
                        continue;
                    }
                    match colour.get(t as usize).copied() {
                        Some(0) => {
                            if let Some(c) = colour.get_mut(t as usize) {
                                *c = 1;
                            }
                            stack.push((t as usize, 0));
                        }
                        Some(1) => back.push((v, t)),
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
    (order, back)
}

/// The graph for `xml`. It reads references more widely than usvg does, so that whichever way
/// usvg reads one, the loop is there to find: an id names every element that carries it, every
/// `href` attribute is followed, sheets are read in any namespace, and a `style` attribute is
/// read like a sheet's declarations.
pub(crate) fn graph(xml: &usvg::roxmltree::Document<'_>) -> Result<Graph, Refusal> {
    type Node<'a, 'i> = usvg::roxmltree::Node<'a, 'i>;
    let all: Vec<Node> = xml.descendants().collect();
    let len = all.iter().map(|n| n.id().get_usize() + 1).max().unwrap_or(0);
    let mut special_at = vec![u32::MAX; len];
    let (mut nodes, mut marker): (Vec<usize>, Vec<bool>) = (Vec::new(), Vec::new());
    let mut specials: Vec<Node> = Vec::new();
    let mut by_id: HashMap<&str, Vec<Node>> = HashMap::new();
    let mut spec_by_id: HashMap<&str, Vec<u32>> = HashMap::new();
    for n in &all {
        let special = is_special(*n);
        if special {
            if specials.len() >= MAX_SPECIALS {
                return Err(Refusal::TooBig);
            }
            if let Some(s) = special_at.get_mut(n.id().get_usize()) {
                *s = specials.len() as u32;
            }
            nodes.push(n.id().get_usize());
            marker.push(n.tag_name().name() == "marker");
        }
        if n.is_element()
            && let Some(id) = n.attribute("id")
        {
            by_id.entry(id).or_default().push(*n);
            if special {
                spec_by_id.entry(id).or_default().push(specials.len() as u32);
            }
        }
        if special {
            specials.push(*n);
        }
    }
    let n = specials.len();
    let mut named = vec![false; n];
    let mut edges: Vec<Vec<u32>> = vec![Vec::new(); n];
    if n == 0 {
        return Ok(Graph { nodes, marker, named, edges, order: Vec::new(), closing: Vec::new() });
    }
    let no_specials: Vec<u32> = Vec::new();
    let named_by = |id: &str| spec_by_id.get(id).unwrap_or(&no_specials);
    // Every element a `href` of `e` (in any namespace) links to.
    let links = |e: Node| -> Vec<Node> {
        let mut v = Vec::new();
        for a in e.attributes().filter(|a| a.name() == "href") {
            if let Some(id) = svgtypes::IRI::from_str(a.value()).ok().map(|i| i.0) {
                v.extend(by_id.get(id).into_iter().flatten().copied());
            }
        }
        v
    };

    // Style sheet rules that name ids, read as usvg reads them (its matcher is simplecss).
    let mut sheet = simplecss::StyleSheet::new();
    for t in style_texts(xml) {
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
        for id in names.iter().flat_map(|(_, v)| v) {
            for s in named_by(id) {
                if let Some(f) = named.get_mut(*s as usize) {
                    *f = true;
                }
            }
        }
        if rules.len() >= MAX_URL_RULES {
            return Err(Refusal::TooBig);
        }
        rules.push(Rule { selector: r.selector.clone(), names });
    }

    // What the file names anywhere: `url(#…)` in attributes, and links.
    for e in all.iter().filter(|e| e.is_element()) {
        for a in e.attributes() {
            if a.name() == "href" {
                if let Some(id) = svgtypes::IRI::from_str(a.value()).ok().map(|i| i.0) {
                    for s in named_by(id) {
                        if let Some(f) = named.get_mut(*s as usize) {
                            *f = true;
                        }
                    }
                }
            } else {
                url_ids(a.value(), |id| {
                    for s in named_by(id) {
                        if let Some(f) = named.get_mut(*s as usize) {
                            *f = true;
                        }
                    }
                });
            }
        }
    }

    // Edges: from each one to the ones its content, ancestors, own attributes and links name.
    let mut steps = 0u64;
    let mut total_edges = 0usize;
    // Visited per special: through plain content, and through a filter image (where usvg's
    // fix for a filter naming itself does not reach).
    let (mut plain, mut image) = (vec![0u32; len], vec![0u32; len]);
    for (si, s) in specials.iter().enumerate() {
        let mark = si as u32 + 1;
        let (mut out, over, unreadable) = (Vec::<u32>::new(), std::cell::Cell::new(false), std::cell::Cell::new(false));
        // usvg removes a pattern's own fill or stroke, and a clip path's, mask's or filter's own
        // clip path, mask or filter, from what it holds when it names the element itself, so
        // such a link is no loop (an inherited one, or one reached through a filter image, is
        // not removed).
        let removed = |name: &str| match s.tag_name().name() {
            "pattern" => matches!(name, "fill" | "stroke"),
            "clipPath" => name == "clip-path",
            "mask" => name == "mask",
            "filter" => name == "filter",
            _ => false,
        };
        // The ids `e` names itself (attributes, `style`, and matching rules); only the
        // properties that pass down to what an element holds if `down`.
        let collect = |e: Node, down: bool, exempt: bool, out: &mut Vec<u32>, steps: &mut u64| -> Result<(), Refusal> {
            *steps = steps.saturating_add(1).saturating_add(rules.len() as u64);
            if *steps > MAX_STEPS {
                return Err(Refusal::TooBig);
            }
            let before = out.len();
            let mut push = |id: &str, name: &str| {
                for t in named_by(id) {
                    if t.eq(&(si as u32)) && !down && exempt && removed(name) {
                        continue;
                    }
                    if out.len() >= MAX_EDGES {
                        over.set(true);
                    } else {
                        out.push(*t);
                    }
                }
            };
            for a in e.attributes() {
                if a.name() == "style" {
                    let mut found = 0usize;
                    for d in simplecss::DeclarationTokenizer::from(a.value()) {
                        found += url_count(d.value);
                        if !down || inherited(d.name) {
                            url_ids(d.value, |id| push(id, d.name));
                        }
                    }
                    // Text the CSS reader skipped: if it names an id, read it as a property that
                    // passes down.
                    if url_count(a.value()) > found {
                        unreadable.set(true);
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
            *steps = steps.saturating_add((out.len() - before) as u64);
            Ok(())
        };
        // Ancestors pass their inheritable properties down to it.
        for a in s.ancestors().skip(1).filter(|a| a.is_element()) {
            collect(a, true, false, &mut out, &mut steps)?;
        }
        let mut stack: Vec<(Node, bool)> = vec![(*s, false)];
        if let Some(m) = plain.get_mut(s.id().get_usize()) {
            *m = mark;
        }
        while let Some((e, via_image)) = stack.pop() {
            collect(e, false, !via_image, &mut out, &mut steps)?;
            if over.get() {
                return Err(Refusal::TooBig);
            }
            if unreadable.get() {
                return Err(Refusal::Unreadable);
            }
            let name = e.tag_name().name();
            // Content it draws from elsewhere: a `<use>`, a filter image, a template.
            let follows =
                matches!(name, "use" | "feImage") || (e == *s && matches!(name, "pattern" | "filter" | "linearGradient" | "radialGradient"));
            let image_next = via_image || name == "feImage";
            let mut next: Vec<(Node, bool)> = Vec::new();
            if follows {
                for t in links(e) {
                    match special_at.get(t.id().get_usize()).copied().filter(|t| *t != u32::MAX) {
                        Some(t) => out.push(t),
                        None => next.push((t, image_next)),
                    }
                }
            }
            for c in e.children() {
                if c.is_element()
                    && c.tag_name().name() != "style"
                    && matches!(c.tag_name().namespace(), None | Some(SVG_NS))
                    && special_at.get(c.id().get_usize()) == Some(&u32::MAX)
                {
                    next.push((c, via_image));
                }
            }
            for (c, img) in next {
                let seen = if img { image.get_mut(c.id().get_usize()) } else { plain.get_mut(c.id().get_usize()) };
                if let Some(m) = seen
                    && *m != mark
                {
                    *m = mark;
                    steps = steps.saturating_add(1);
                    if steps > MAX_STEPS {
                        return Err(Refusal::TooBig);
                    }
                    stack.push((c, img));
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
    }
    for t in edges.iter().flatten() {
        if let Some(f) = named.get_mut(*t as usize) {
            *f = true;
        }
    }

    // Loops. usvg never builds a marker inside itself, so a loop must avoid markers to go on
    // without end: the graph without them has to be loop free. Loops through markers are then
    // left out of the walk's order, and counted once.
    let is_marker = |v: usize| marker.get(v) == Some(&true);
    if !walk(&edges, &is_marker).1.is_empty() {
        return Err(Refusal::Loop);
    }
    let (order, back) = walk(&edges, &|_| false);
    for (v, t) in &back {
        if let Some(e) = edges.get_mut(*v) {
            e.retain(|x| x != t);
        }
    }
    // The order must hold for what is left: finished last = refers to the others.
    let (mut order2, again) = walk(&edges, &|_| false);
    if !again.is_empty() {
        return Err(Refusal::Loop);
    }
    order2.reverse();
    let _ = order;
    let closing = back.into_iter().map(|(v, t)| (v as u32, t)).collect();
    Ok(Graph { nodes, marker, named, edges, order: order2, closing })
}
