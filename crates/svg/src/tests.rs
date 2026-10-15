use super::*;

/// A 40 × 20 px SVG: red left half, blue right half.
fn halves() -> Vec<u8> {
    br##"<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20" viewBox="0 0 40 20">
  <rect x="0" y="0" width="20" height="20" fill="#ff0000"/>
  <rect x="20" y="0" width="20" height="20" fill="#0000ff"/>
</svg>"##
        .to_vec()
}

fn px(r: &Raster, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * r.width + x) * 4) as usize;
    let p = r.rgba.get(i..i + 4).unwrap();
    [p[0], p[1], p[2], p[3]]
}

#[test]
fn recognises_svg_files() {
    assert!(is_svg(&halves()));
    assert!(is_svg(b"  <svg xmlns=\"http://www.w3.org/2000/svg\" width=\"1\" height=\"1\"/>"));
    assert!(is_svg(b"\xEF\xBB\xBF<!-- c --><!DOCTYPE svg><svg width=\"1\" height=\"1\"/>"));
    assert!(!is_svg(b"\x89PNG\r\n\x1a\n"));
    assert!(!is_svg(b"<?xml version=\"1.0\"?><w:document/>"));
    assert!(!is_svg(b""));
    // Compressed SVG is refused (a gzip bomb could expand without bound).
    assert!(!is_svg(&[0x1f, 0x8b, 8, 0, 0, 0]));
}

#[test]
fn natural_size_is_css_pixels_at_96_ppi() {
    assert_eq!(natural_size(&halves()), Some((30.0, 15.0)));
    // A viewBox alone gives the size.
    let vb = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 96 48"/>"#;
    assert_eq!(natural_size(vb), Some((72.0, 36.0)));
    // Huge pictures are scaled down to 22 inches on the longer side.
    let big = br#"<svg xmlns="http://www.w3.org/2000/svg" width="96000" height="48000"/>"#;
    assert_eq!(natural_size(big), Some((1584.0, 792.0)));
    assert_eq!(natural_size(b"not svg"), None);
}

#[test]
fn renders_at_the_requested_resolution() {
    let svg = parse(&halves()).unwrap();
    assert_eq!(svg.size_px(), (40.0, 20.0));
    // Ten times the natural size: the edge between the halves is still one pixel sharp.
    let r = svg.render(400, 200, [10.0, 0.0, 0.0, 10.0, 0.0, 0.0]).unwrap();
    assert_eq!((r.width, r.height), (400, 200));
    assert_eq!(px(&r, 199, 100), [255, 0, 0, 255]);
    assert_eq!(px(&r, 200, 100), [0, 0, 255, 255]);
}

#[test]
fn fallback_png_is_twice_the_natural_size() {
    let png = fallback_png(&halves()).unwrap();
    assert!(png.starts_with(b"\x89PNG"));
    // IHDR width and height.
    assert_eq!(u32::from_be_bytes(png[16..20].try_into().unwrap()), 80);
    assert_eq!(u32::from_be_bytes(png[20..24].try_into().unwrap()), 40);
    let big = br#"<svg xmlns="http://www.w3.org/2000/svg" width="96000" height="48000"><rect width="10" height="10"/></svg>"#;
    let png = fallback_png(big).unwrap();
    assert_eq!(u32::from_be_bytes(png[16..20].try_into().unwrap()), MAX_FALLBACK_SIDE);
}

#[test]
fn hostile_svgs_are_errors_not_panics() {
    for bad in [
        b"".as_slice(),
        b"<svg",
        b"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"0\" height=\"0\"/>",
        b"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"-5\" height=\"NaN\"><rect/></svg>",
        b"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"1e39\" height=\"1e39\"/>",
        b"\xff\xfe<\x00s\x00",
    ] {
        assert!(parse(bad).is_err(), "{:?}", String::from_utf8_lossy(bad));
    }
}

#[test]
fn deep_nesting_is_refused() {
    // Refused before usvg recurses into it.
    let deep = format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">{}{}</svg>"#, "<g>".repeat(5000), "</g>".repeat(5000));
    assert_eq!(parse(deep.as_bytes()).err(), Some(Error::TooDeep));
}

#[test]
fn oversized_files_are_refused() {
    let mut huge = halves();
    huge.resize(MAX_SVG_BYTES + 1, b' ');
    assert_eq!(parse(&huge).err(), Some(Error::TooLarge));
}

#[test]
fn billion_laughs_is_bounded() {
    let lol = r#"<?xml version="1.0"?><!DOCTYPE svg [<!ENTITY a "aaaaaaaaaa"><!ENTITY b "&a;&a;&a;&a;&a;&a;&a;&a;&a;&a;"><!ENTITY c "&b;&b;&b;&b;&b;&b;&b;&b;&b;&b;"><!ENTITY d "&c;&c;&c;&c;&c;&c;&c;&c;&c;&c;"><!ENTITY e "&d;&d;&d;&d;&d;&d;&d;&d;&d;&d;"><!ENTITY f "&e;&e;&e;&e;&e;&e;&e;&e;&e;&e;"><!ENTITY g "&f;&f;&f;&f;&f;&f;&f;&f;&f;&f;"><!ENTITY h "&g;&g;&g;&g;&g;&g;&g;&g;&g;&g;">]><svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><text>&h;</text></svg>"#;
    assert!(parse(lol.as_bytes()).is_err(), "100 MB of expanded text is refused");
}

/// Review 1 #1: one wide entity referenced many times at depth 0 (which roxmltree allows without
/// limit) would expand to gigabytes. Scaled down: 64 KB × 2000 = 128 MB, refused before parsing.
#[test]
fn wide_entity_expansion_is_refused() {
    let doc = format!(
        r#"<?xml version="1.0"?><!DOCTYPE svg [<!ENTITY e "{}">]><svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><text>{}</text></svg>"#,
        "a".repeat(16 << 10),
        "&e;".repeat(8000)
    );
    assert!(is_svg(doc.as_bytes()));
    let t = std::time::Instant::now();
    let r = parse(doc.as_bytes());
    assert!(matches!(&r, Err(Error::Parse(m)) if m.contains("entit")), "refused for its entities: {:?}", r.err());
    assert!(t.elapsed() < std::time::Duration::from_secs(2), "{:?}", t.elapsed());
}

/// Short entities, as Illustrator writes for its namespaces, still read.
#[test]
fn small_namespace_entities_are_accepted() {
    let doc = r##"<?xml version="1.0" encoding="utf-8"?>
<!DOCTYPE svg PUBLIC "-//W3C//DTD SVG 1.1//EN" "http://www.w3.org/Graphics/SVG/1.1/DTD/svg11.dtd" [
	<!ENTITY ns_svg "http://www.w3.org/2000/svg">
	<!ENTITY ns_xlink "http://www.w3.org/1999/xlink">
]>
<svg version="1.1" xmlns="&ns_svg;" xmlns:xlink="&ns_xlink;" width="20" height="10"><rect width="10" height="10" fill="#f00"/></svg>"##;
    let svg = parse(doc.as_bytes()).unwrap();
    assert_eq!(svg.size_px(), (20.0, 10.0));
}

/// Review 1 #2: the declaration scan looked for `[` to the end of the file at every `<!`, which
/// is quadratic: a few MB of `<!a>` never finished.
#[test]
fn declarations_scan_in_linear_time() {
    let doc = format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">{}</svg>"#, "<!a>".repeat(1 << 16));
    let t = std::time::Instant::now();
    let _ = parse(doc.as_bytes());
    assert!(t.elapsed() < std::time::Duration::from_secs(1), "{:?}", t.elapsed());
    let prolog = format!("{}<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"1\" height=\"1\"/>", "<!DOCTYPE a>".repeat(5000));
    let t = std::time::Instant::now();
    for _ in 0..20 {
        let _ = is_svg(prolog.as_bytes());
    }
    assert!(t.elapsed() < std::time::Duration::from_secs(1), "{:?}", t.elapsed());
}

/// Review 1 #3: text in an SVG draws with the fonts documents use (Source Sans 3 is compiled in,
/// so this holds without system fonts or craft-fonts).
#[test]
fn text_is_drawn_with_document_fonts() {
    let doc = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="60"><text x="5" y="50" font-family="Source Sans 3" font-size="50" fill="black">WW</text></svg>"#;
    let r = parse(doc).unwrap().render(100, 60, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]).unwrap();
    let inked = r.rgba.chunks(4).filter(|p| p[3] > 128).count();
    assert!(inked > 200, "the text is drawn: {inked} inked pixels");
}

/// Review 1 #13: a prefixed root element is still an SVG.
#[test]
fn prefixed_svg_root_is_accepted() {
    let doc = br#"<svg:svg xmlns:svg="http://www.w3.org/2000/svg" width="10" height="10"><svg:rect width="10" height="10"/></svg:svg>"#;
    assert!(is_svg(doc));
    assert_eq!(natural_size(doc), Some((7.5, 7.5)));
    assert!(!is_svg(b"<w:svgish/>"));
}

#[test]
fn rasters_past_the_cap_are_refused() {
    let svg = parse(&halves()).unwrap();
    assert!(svg.render(100_000, 100_000, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]).is_none());
    assert!(svg.render(0, 10, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]).is_none());
    assert!(svg.render(10, 10, [f32::NAN, 0.0, 0.0, 1.0, 0.0, 0.0]).is_none());
}

#[test]
fn external_pictures_are_never_loaded() {
    // A real PNG on disk, linked by absolute path and by file: URL: neither is read.
    let dir = std::env::temp_dir().join(format!("wordcraft-svg-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let png_path = dir.join("red.png");
    let png = fallback_png(&halves()).unwrap();
    std::fs::write(&png_path, &png).unwrap();
    let p = png_path.to_string_lossy();
    let doc = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="20" height="20"><image href="{p}" width="10" height="20"/><image xlink:href="file://{p}" x="10" width="10" height="20"/></svg>"#
    );
    let svg = parse(doc.as_bytes()).unwrap();
    let r = svg.render(20, 20, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]).unwrap();
    assert!(r.rgba.iter().all(|b| *b == 0), "nothing from the file is drawn");
    let _ = std::fs::remove_dir_all(&dir);
    // A data: picture is drawn.
    let b64 = base64_encode(&png);
    let inline = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20"><image href="data:image/png;base64,{b64}" width="20" height="20"/></svg>"#
    );
    let r = parse(inline.as_bytes()).unwrap().render(20, 20, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]).unwrap();
    assert!(r.rgba.iter().any(|b| *b != 0), "the data: picture is drawn");
}

fn base64_encode(b: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for ch in b.chunks(3) {
        let n = (ch[0] as u32) << 16 | (*ch.get(1).unwrap_or(&0) as u32) << 8 | *ch.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            out.push(if i <= ch.len() { T[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
        }
    }
    out
}

#[test]
fn chained_uses_and_nested_pictures_stay_within_the_stack() {
    // Each level wraps the previous one 200 deep through `use`: about 1000 levels once expanded.
    // Parsing and drawing must not overflow a test thread's 2 MB stack.
    let wrap = |inner: &str| format!("{}{inner}{}", "<g>".repeat(200), "</g>".repeat(200));
    let mut defs = format!(r#"<g id="l0">{}</g>"#, wrap(r##"<rect width="10" height="10" fill="#f00"/>"##));
    for i in 1..6 {
        defs.push_str(&format!(r##"<g id="l{i}">{}</g>"##, wrap(&format!(r##"<use href="#l{}"/>"##, i - 1))));
    }
    let doc = format!(r##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><defs>{defs}</defs><use href="#l5"/></svg>"##);
    if let Ok(svg) = parse(doc.as_bytes()) {
        let _ = svg.render(10, 10, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    }
    // SVG pictures inside SVG pictures (data: links), nested 12 deep.
    let mut svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10" fill="#f00"/></svg>"##.to_string();
    for _ in 0..12 {
        let b64 = base64_encode(svg.as_bytes());
        svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">{}<image href="data:image/svg+xml;base64,{b64}" width="10" height="10"/>{}</svg>"#,
            "<g>".repeat(200),
            "</g>".repeat(200)
        );
    }
    if let Ok(s) = parse(svg.as_bytes()) {
        let _ = s.render(10, 10, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    }
    // A gzip-compressed SVG inside a data: link is not expanded.
    let gz = base64_encode(&[0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 3, 1, 2, 3]);
    let doc = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><image href="data:image/svg+xml;base64,{gz}" width="10" height="10"/></svg>"#
    );
    assert!(parse(doc.as_bytes()).is_ok());
}

/// Review 2 N1: every entity declaration outside a DOCTYPE rescanned the whole file for `&`.
#[test]
fn many_entity_declarations_scan_quickly() {
    let doc = format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">{}</svg>"#, "<!ENTITY a \"\">".repeat(20_000));
    let t = std::time::Instant::now();
    let _ = parse(doc.as_bytes());
    assert!(t.elapsed() < std::time::Duration::from_millis(500), "{:?}", t.elapsed());
}

/// Review 2 N2: each reference inside an entity value looked the name up in a list (quadratic),
/// and the number of declarations wasn't capped.
#[test]
fn entity_declarations_are_capped() {
    let mut decls = String::from("<!ENTITY e0 \"x\">");
    for i in 1..20_000 {
        decls.push_str(&format!("<!ENTITY e{i} \"&e{};\">", i - 1));
    }
    // After the root, so the sniff (which reads 64 KB of prolog) lets the file through to the scan.
    let doc = format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">{decls}</svg>"#);
    let t = std::time::Instant::now();
    let r = parse(doc.as_bytes());
    assert!(matches!(&r, Err(Error::Parse(m)) if m.contains("entit")), "{:?}", r.err());
    assert!(t.elapsed() < std::time::Duration::from_millis(500), "{:?}", t.elapsed());
}

/// Review 2 N3: text is laid out with real fonts now, so a huge text body is refused before usvg
/// shapes it — written out, or expanded from entities.
#[test]
fn huge_text_bodies_are_refused() {
    let plain = format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><text>{}</text></svg>"#, "a".repeat(1_200_000));
    let r = parse(plain.as_bytes());
    assert!(matches!(&r, Err(Error::Parse(m)) if m.contains("text")), "{:?}", r.err());
}

#[test]
fn huge_entity_expanded_text_is_refused() {
    let expanded = format!(
        r#"<?xml version="1.0"?><!DOCTYPE svg [<!ENTITY e "{}">]><svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><text><tspan>{}</tspan></text></svg>"#,
        "a".repeat(4 << 10),
        "&e;".repeat(300)
    );
    let r = parse(expanded.as_bytes());
    assert!(matches!(&r, Err(Error::Parse(m)) if m.contains("text")), "{:?}", r.err());
}

#[test]
fn text_outside_text_elements_is_not_counted() {
    // Ordinary labels are fine, and text outside text elements (a title) doesn't count.
    let ok =
        format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><title>{}</title><text>Label</text></svg>"#, "t".repeat(2 << 20));
    assert!(parse(ok.as_bytes()).is_ok());
}

/// An SVG picture with `n` bytes of (blank) text in a text element, as a `data:` link.
fn nested_text_picture(n: usize) -> String {
    let inner = format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><text>{}a</text></svg>"#, " ".repeat(n));
    format!(r#"<image href="data:image/svg+xml;base64,{}" width="10" height="10"/>"#, base64_encode(inner.as_bytes()))
}

/// Review 3 #1: the text cap covers the whole picture, nested `data:` SVG pictures included,
/// rather than giving each nested picture a budget of its own.
#[test]
fn text_cap_is_shared_with_nested_pictures() {
    let one = format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">{}</svg>"#, nested_text_picture(600_000));
    assert!(parse(one.as_bytes()).is_ok(), "one nested picture with 600 KB of text is fine");
    let two = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">{}{}</svg>"#,
        nested_text_picture(600_000),
        nested_text_picture(600_000)
    );
    let r = parse(two.as_bytes());
    assert!(matches!(&r, Err(Error::Parse(m)) if m.contains("text")), "1.2 MB of text across nested pictures is refused: {:?}", r.err());
    // The outer picture's own text counts too.
    let mixed = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><text>{}a</text>{}</svg>"#,
        " ".repeat(600_000),
        nested_text_picture(600_000)
    );
    let r = parse(mixed.as_bytes());
    assert!(matches!(&r, Err(Error::Parse(m)) if m.contains("text")), "{:?}", r.err());
}

/// Review 3 #2: only `<use>` copies of text multiply the text drawn: a chart with 100 000
/// markers and a short caption is fine, while many copies of a long text are still refused.
#[test]
fn use_copies_count_only_when_they_copy_text() {
    let markers = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><defs><circle id="dot" r="1"/></defs>{}<text>{}</text></svg>"##,
        r##"<use href="#dot"/>"##.repeat(100_000),
        "c".repeat(300)
    );
    let r = parse(markers.as_bytes());
    assert!(r.is_ok(), "marker-heavy chart: {:?}", r.err());
    // Markers drawn through a group of `<use>`s of a shape copy no text either.
    let grouped = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><defs><circle id="dot" r="1"/><g id="m"><use href="#dot"/></g></defs>{}<text>{}</text></svg>"##,
        r##"<use href="#m"/>"##.repeat(100_000),
        "c".repeat(300)
    );
    let r = parse(grouped.as_bytes());
    assert!(r.is_ok(), "grouped markers: {:?}", r.err());
    let copies = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><defs><text id="t">{}</text></defs>{}</svg>"##,
        "c".repeat(100_000),
        r##"<use xlink:href="#t" xmlns:xlink="http://www.w3.org/1999/xlink"/>"##.repeat(200)
    );
    let r = parse(copies.as_bytes());
    assert!(matches!(&r, Err(Error::Parse(m)) if m.contains("text")), "{:?}", r.err());
    // A group of text copies, itself copied (and referenced before it is defined).
    let chained = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">{}<defs><g id="g">{}</g><text id="t">{}</text></defs></svg>"##,
        r##"<use href="#g"/>"##.repeat(150),
        r##"<use href="#t"/>"##.repeat(10),
        "c".repeat(100_000)
    );
    let r = parse(chained.as_bytes());
    assert!(matches!(&r, Err(Error::Parse(m)) if m.contains("text")), "{:?}", r.err());
}

/// Fix round 3: usvg reads a nested `data:` picture once per copy of it, so the nested pictures
/// read for one picture share a cap ([`MAX_SVG_BYTES`] in all) rather than each copy having its own.
#[test]
fn nested_pictures_read_per_copy_share_a_cap() {
    let inner = format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><!--{}--></svg>"#, " ".repeat(100_000));
    let image = format!(r#"<image id="i" href="data:image/svg+xml;base64,{}" width="10" height="10"/>"#, base64_encode(inner.as_bytes()));
    let svg = |copies: usize| {
        format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><defs>{image}</defs>{}</svg>"##,
            r##"<use href="#i"/>"##.repeat(copies)
        )
    };
    assert!(parse(svg(10).as_bytes()).is_ok(), "ten copies (1 MB read) are fine");
    let r = parse(svg(400).as_bytes());
    assert!(matches!(&r, Err(Error::Parse(m)) if m.contains("nested")), "40 MB of nested pictures: {:?}", r.err());
}

/// `copies` of `link` after the definitions `def`, whose `TEXT` is `n` bytes of text.
fn copied_text(def: &str, link: &str, copies: usize, n: usize) -> String {
    let def = def.replace("TEXT", &"c".repeat(n));
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" xmlns:z="urn:z" width="10" height="10"><defs><circle id="dot" r="1"/>{def}</defs>{}</svg>"##,
        link.repeat(copies)
    )
}

fn refused_as_text(doc: &str) -> bool {
    matches!(parse(doc.as_bytes()), Err(Error::Parse(m)) if m.contains("text"))
}

/// Review 4 I-1: a `<use>` inside a group that points back at the group's ancestor is skipped by
/// usvg, but the text next to it is still built once per copy of the outer group.
#[test]
fn text_beside_a_self_reference_counts_per_copy() {
    let def = r##"<g id="P"><g id="t"><g id="G"><text>TEXT</text><use href="#t"/></g></g></g>"##;
    let doc = copied_text(def, r##"<use href="#P"/>"##, 200, 100_000);
    let t = std::time::Instant::now();
    assert!(refused_as_text(&doc), "20 MB of text drawn through copies of P");
    assert!(t.elapsed() < std::time::Duration::from_secs(2), "{:?}", t.elapsed());
    // Without the copies it is an ordinary picture.
    assert!(parse(copied_text(def, "", 0, 100_000).as_bytes()).is_ok());
}

/// Review 4 I-2: links are read the way usvg reads them: character references resolved, spaces
/// around the `#id` allowed, and `xlink:href` taken before the first other attribute named `href`.
#[test]
fn use_links_are_read_like_usvg() {
    let def = r#"<text id="t">TEXT</text>"#;
    for link in [r##"<use href="&#35;t"/>"##, r##"<use href=" #t"/>"##, r##"<use href="#t "/>"##, r##"<use z:href="#dot" xlink:href="#t"/>"##] {
        let doc = copied_text(def, link, 200, 100_000);
        assert!(refused_as_text(&doc), "{link}");
    }
    // usvg looks a plain `href` up by its local name in any namespace (roxmltree's lookup), so a
    // link only in another namespace is followed too.
    assert!(refused_as_text(&copied_text(def, r##"<use z:href="#t"/>"##, 200, 100_000)));
}

/// Round 4: copies that lead back to themselves never end; usvg would build them a thousand
/// levels deep before giving up.
#[test]
fn endless_copies_are_refused() {
    let def = r##"<g id="a"><text>TEXT</text><use href="#b"/></g><g id="b"><use href="#a"/></g>"##;
    let r = parse(copied_text(def, "", 0, 1000).as_bytes());
    assert!(matches!(&r, Err(Error::Parse(m)) if m.contains("never end")), "{:?}", r.err());
}

/// Round 4: every `<tref>` makes usvg look its target up by scanning the whole file, so many of
/// them in a large file would take minutes.
#[test]
fn many_trefs_in_a_large_file_are_refused_quickly() {
    let doc = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><text>{}</text>{}<g id="far"/></svg>"##,
        r##"<tref href="#far"/>"##.repeat(20_000),
        "<g/>".repeat(200_000)
    );
    let t = std::time::Instant::now();
    let r = parse(doc.as_bytes());
    assert!(r.is_err(), "refused");
    assert!(t.elapsed() < std::time::Duration::from_secs(2), "{:?}", t.elapsed());
}

/// The number of text elements in a parsed picture (usvg's tree after conversion), counting those
/// drawn in markers, clip paths, masks, patterns and filter images too.
fn usvg_texts(g: &usvg::Group) -> u64 {
    g.children()
        .iter()
        .map(|n| {
            let mut k = match n {
                usvg::Node::Group(g) => usvg_texts(g),
                usvg::Node::Text(_) => 1,
                _ => 0,
            };
            // A nested picture's text is its own (counted when it is read).
            if !matches!(n, usvg::Node::Image(_)) {
                n.subroots(|r| k += usvg_texts(r));
            }
            k
        })
        .sum()
}

/// Round 4: the pre-check counts the text copies usvg builds, exactly, for small crafted
/// pictures (no `<defs>`, so every element built is drawn). A change in usvg's `<use>` rules
/// shows up here.
#[test]
fn text_copies_match_usvg() {
    let cases = [
        r##"<text>a</text><text>b</text>"##,
        r##"<text id="t">a</text><use href="#t"/><use href="#t"/><use href="#t"/>"##,
        r##"<g id="g"><text>a</text><text>b</text></g><use href="#g"/><use href="#g"/>"##,
        r##"<text id="t">a</text><g id="g1"><use href="#t"/><use href="#t"/></g><g id="g2"><use href="#g1"/><use href="#g1"/><use href="#g1"/></g><use href="#g2"/>"##,
        // Review 4 I-1, small.
        r##"<g id="P"><g id="t"><g id="G"><text>a</text><use href="#t"/></g></g></g><use href="#P"/><use href="#P"/><use href="#P"/>"##,
        // Review 4 I-2.
        r##"<text id="t">a</text><use href="&#35;t"/><use href=" #t "/><use z:href="#x" xlink:href="#t"/><use z:href="#t"/>"##,
        // A `<use>` of itself, of its own group, two `<use>`s of each other, a missing target.
        r##"<g id="g"><text>a</text><use id="u" href="#u"/><use href="#g"/></g><use id="a" href="#b"/><use id="b" href="#a"/><use href="#none"/>"##,
        // A group whose `<use>` points at a `<use>` of the group.
        r##"<g id="g1"><text>a</text><use href="#u2"/></g><use id="u2" href="#g1"/><use href="#g1"/>"##,
        // Later ids don't replace the first one.
        r##"<text id="t">a</text><text id="t">b</text><use href="#t"/>"##,
        // `<tref>` text, and a group holding a `<use>` of itself (which no `<use>` copies).
        r##"<text id="s">a</text><text>b<tref href="#s"/></text><g id="g"><text>c</text><use href="#g"/></g><use href="#g"/>"##,
        // Elements outside the SVG namespace aren't built.
        r##"<text id="t">a</text><z:g><text>b</text><use href="#t"/></z:g>"##,
        // Links are groups.
        r##"<a id="l" href="https://example.com"><text>a</text></a><use href="#l"/>"##,
        // A nested `data:` picture's text is counted when that picture is read, not here.
        &format!(r#"<text>a</text>{}"#, nested_text_picture(3)),
    ];
    let svg = |body: &str| {
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" xmlns:z="urn:z" width="100" height="100" font-family="Source Sans 3" font-size="10">{body}</svg>"#
        )
    };
    for body in cases {
        let doc = svg(body);
        let xml = usvg::roxmltree::Document::parse(&doc).unwrap();
        let counted = copies(&xml).unwrap().texts;
        let built = usvg_texts(parse(doc.as_bytes()).unwrap().tree().root());
        assert_eq!(counted, built, "{body}");
    }
    // Where usvg builds elements it doesn't draw (`<symbol>`, `<defs>`, the other `<switch>`
    // children), or draws copies the check bounds rather than counts (markers, clip paths, masks,
    // patterns, filter images), the count is at least what usvg draws.
    let bounded = [
        r##"<symbol id="s"><text>a</text></symbol><use href="#s"/><use href="#s"/>"##,
        r##"<defs><text id="t">a</text></defs><use href="#t"/>"##,
        r##"<switch><text>a</text><text>b</text></switch>"##,
        r##"<marker id="m" markerWidth="9" markerHeight="9" overflow="visible"><text>a</text></marker><polyline points="1,1 5,5 9,1 20,20" stroke="black" marker-start="url(#m)" marker-mid="url(#m)" marker-end="url(#m)"/>"##,
        r##"<clipPath id="c" clipPathUnits="objectBoundingBox"><text y="1" font-size="1">a</text></clipPath><rect width="9" height="9" clip-path="url(#c)"/><rect x="20" width="9" height="9" clip-path="url(#c)"/>"##,
        r##"<mask id="k"><text y="9">a</text></mask><rect width="9" height="9" mask="url(#k)"/><rect x="20" width="9" height="9" mask="url(#k)"/>"##,
        r##"<pattern id="p" width="9" height="9" patternUnits="userSpaceOnUse"><text y="9">a</text></pattern><rect width="9" height="9" fill="url(#p)"/><rect x="20" width="9" height="9" fill="url(#p)"/>"##,
        r##"<text id="t" y="9">a</text><filter id="f"><feImage href="#t"/></filter><rect width="9" height="9" filter="url(#f)"/><rect x="20" width="9" height="9" filter="url(#f)"/>"##,
    ];
    for body in bounded {
        let doc = svg(body);
        let xml = usvg::roxmltree::Document::parse(&doc).unwrap();
        let counted = copies(&xml).unwrap().texts;
        let built = usvg_texts(parse(doc.as_bytes()).unwrap().tree().root());
        assert!(built > 0 && counted >= built, "{body}: counted {counted}, usvg drew {built}");
    }
}

/// `body` in a picture, with `TEXT` replaced by `n` bytes of text.
fn picture_with_text(body: &str, n: usize) -> String {
    format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">{}</svg>"#, body.replace("TEXT", &"c".repeat(n)))
}

fn refused_quickly(doc: &str) -> bool {
    let t = std::time::Instant::now();
    let r = parse(doc.as_bytes());
    let fast = t.elapsed() < std::time::Duration::from_secs(2);
    matches!(&r, Err(Error::Parse(m)) if m.contains("text")) && fast
}

/// Review 5 I-1: usvg draws a marker's content once per vertex of every shape that uses it.
#[test]
fn text_in_markers_counts_per_vertex() {
    let points: String = (0..2000).map(|i| format!("{i},{} ", i % 7)).collect();
    let marker = |text: &str, n| {
        picture_with_text(
            &format!(r##"<marker id="m" overflow="visible">{text}</marker><polyline points="{points}" stroke="black" marker-mid="url(#m)"/>"##),
            n,
        )
    };
    assert!(refused_quickly(&marker("<text>TEXT</text>", 100_000)), "200 MB of marker text");
    // A chart's dot markers (no text) on 2000 points, with a caption, opens.
    let chart = picture_with_text(
        &format!(r##"<marker id="m"><circle r="2"/></marker><polyline points="{points}" marker-mid="url(#m)"/><text y="20">TEXT</text>"##),
        200,
    );
    assert!(parse(chart.as_bytes()).is_ok());
    // A short label on a few points opens.
    let few = picture_with_text(r##"<marker id="m"><text>TEXT</text></marker><polyline points="1,1 5,5 9,1" marker-mid="url(#m)"/>"##, 10);
    assert!(parse(few.as_bytes()).is_ok());
}

/// `n` rects using `attr`, after `def`.
fn many_users(def: &str, attr: &str, n: usize) -> String {
    format!("{def}{}", format!(r#"<rect width="9" height="9" {attr}/>"#).repeat(n))
}

/// Review 5 I-1: usvg builds an `objectBoundingBox` clip path's or mask's content once per
/// element that uses it.
#[test]
fn text_in_clip_paths_and_masks_counts_per_use() {
    let clip = r##"<clipPath id="c" clipPathUnits="objectBoundingBox"><text>TEXT</text></clipPath>"##;
    assert!(refused_quickly(&picture_with_text(&many_users(clip, r##"clip-path="url(#c)""##, 2000), 10_000)), "clip path");
    let mask = r##"<mask id="k"><text>TEXT</text></mask>"##;
    assert!(refused_quickly(&picture_with_text(&many_users(mask, r##"mask="url(#k)""##, 2000), 10_000)), "mask");
    let filter = r##"<text id="t">TEXT</text><filter id="f"><feImage href="#t"/></filter>"##;
    assert!(refused_quickly(&picture_with_text(&many_users(filter, r##"filter="url(#f)""##, 2000), 10_000)), "filter image");
    // Ordinary clip paths and masks open.
    let plain = r##"<clipPath id="c"><rect width="5" height="5"/></clipPath><mask id="k"><rect width="5" height="5" fill="white"/></mask>"##;
    let doc = picture_with_text(&format!("{}<text>TEXT</text>", many_users(plain, r##"clip-path="url(#c)" mask="url(#k)""##, 2000)), 200);
    assert!(parse(doc.as_bytes()).is_ok());
    // A short text clip used a few times opens.
    let few = picture_with_text(&many_users(clip, r##"clip-path="url(#c)""##, 5), 20);
    assert!(parse(few.as_bytes()).is_ok());
}

/// A picture around `body`, for the reference tests.
fn pic(body: &str) -> String {
    format!(r##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="100" height="100">{body}</svg>"##)
}

/// Whether `doc` is refused for references that lead back to themselves. (usvg builds the things
/// such links name by building what they hold, without limit: a stack overflow, which aborts the
/// process, so each shape below is its own test.)
fn refused_as_loop(doc: &str) -> bool {
    matches!(parse(doc.as_bytes()), Err(Error::Parse(m)) if m.contains("in a loop"))
}

const PAT: &str = r#"patternUnits="userSpaceOnUse" width="10" height="10""#;

/// Review 6 C-1 (a): three patterns in a ring.
#[test]
fn loop_pattern_ring() {
    let d = pic(&format!(
        r##"<defs><pattern id="p1" {PAT}><rect width="5" height="5" fill="url(#p2)"/></pattern><pattern id="p2" {PAT}><rect width="5" height="5" fill="url(#p3)"/></pattern><pattern id="p3" {PAT}><rect width="5" height="5" fill="url(#p1)"/></pattern></defs><rect width="50" height="50" fill="url(#p1)"/>"##
    ));
    assert!(refused_as_loop(&d));
}

/// Review 6 C-1 (b): two patterns, fill and stroke.
#[test]
fn loop_pattern_fill_and_stroke() {
    let d = pic(&format!(
        r##"<defs><pattern id="p1" {PAT}><rect width="5" height="5" fill="url(#p2)"/></pattern><pattern id="p2" {PAT}><rect width="5" height="5" stroke="url(#p1)"/></pattern></defs><rect width="50" height="50" fill="url(#p1)"/>"##
    ));
    assert!(refused_as_loop(&d));
}

/// Review 6 C-1 (c): a pattern and a mask.
#[test]
fn loop_pattern_and_mask() {
    let d = pic(&format!(
        r##"<defs><pattern id="p" {PAT}><rect width="5" height="5" mask="url(#m)"/></pattern><mask id="m" maskUnits="userSpaceOnUse" x="0" y="0" width="50" height="50"><rect width="50" height="50" fill="url(#p)"/></mask></defs><rect width="50" height="50" fill="url(#p)"/>"##
    ));
    assert!(refused_as_loop(&d));
}

/// Review 6 C-1 (d): three masks in a ring.
#[test]
fn loop_mask_ring() {
    let m = |a: &str, b: &str| {
        format!(
            r##"<mask id="{a}" maskUnits="userSpaceOnUse" x="0" y="0" width="50" height="50"><rect width="50" height="50" fill="white" mask="url(#{b})"/></mask>"##
        )
    };
    let d = pic(&format!(r##"<defs>{}{}{}</defs><rect width="50" height="50" mask="url(#m1)"/>"##, m("m1", "m2"), m("m2", "m3"), m("m3", "m1")));
    assert!(refused_as_loop(&d));
}

/// Review 6 C-1 (e): three clip paths whose own `clip-path` leads round.
#[test]
fn loop_clip_path_ring() {
    let d = pic(
        r##"<defs><clipPath id="c1" clip-path="url(#c2)"><rect width="9" height="9"/></clipPath><clipPath id="c2" clip-path="url(#c3)"><rect width="9" height="9"/></clipPath><clipPath id="c3" clip-path="url(#c1)"><rect width="9" height="9"/></clipPath></defs><rect width="50" height="50" clip-path="url(#c1)"/>"##,
    );
    assert!(refused_as_loop(&d));
}

/// Review 6 C-1 (f): a filter, its image, a pattern.
#[test]
fn loop_filter_image_pattern() {
    let d = pic(&format!(
        r##"<defs><filter id="f"><feImage href="#r"/></filter><rect id="r" width="5" height="5" fill="url(#p)"/><pattern id="p" {PAT}><rect width="5" height="5" filter="url(#f)"/></pattern></defs><rect width="50" height="50" filter="url(#f)"/>"##
    ));
    assert!(refused_as_loop(&d));
}

/// Review 6 C-1 (g): a mask, a filter, its image.
#[test]
fn loop_mask_filter_image() {
    let d = pic(
        r##"<defs><mask id="m" maskUnits="userSpaceOnUse" x="0" y="0" width="50" height="50"><rect width="50" height="50" fill="white" filter="url(#f)"/></mask><filter id="f"><feImage href="#r"/></filter><rect id="r" width="5" height="5" mask="url(#m)"/></defs><rect width="50" height="50" mask="url(#m)"/>"##,
    );
    assert!(refused_as_loop(&d));
}

/// Review 6 C-1 (j): a pattern inherits the paint of the group around it, which names the pattern.
#[test]
fn loop_through_inherited_paint() {
    let d = pic(&format!(r##"<g fill="url(#p)"><pattern id="p" {PAT}><rect width="5" height="5"/></pattern><rect width="50" height="50"/></g>"##));
    assert!(refused_as_loop(&d));
    // A pattern template chain that comes back to a later link (usvg's template walk never ends).
    let rho = pic(&format!(
        r##"<pattern id="a" {PAT} href="#b"/><pattern id="b" {PAT} href="#c"/><pattern id="c" {PAT} href="#b"><rect width="5" height="5"/></pattern><rect width="50" height="50" fill="url(#a)"/>"##
    ));
    assert!(refused_as_loop(&rho));
    // A pattern's own fill or stroke naming itself (in a style sheet, a `style` attribute, or
    // through a `<use>`) is removed by usvg, so these are drawn.
    let ok = |body: &str| {
        let r = parse(pic(body).as_bytes());
        assert!(r.is_ok(), "{body}: {:?}", r.err());
    };
    ok(&format!(
        r##"<style>.k{{fill:url(#p)}}</style><pattern id="p" {PAT}><rect class="k" width="5" height="5"/></pattern><rect width="50" height="50" fill="url(#p)"/>"##
    ));
    ok(&format!(
        r##"<pattern id="p" {PAT}><rect style="stroke:url( '#p' )" width="5" height="5"/></pattern><rect width="50" height="50" fill="url(#p)"/>"##
    ));
    ok(&format!(
        r##"<g id="g" fill="url(#p)"><rect width="5" height="5"/></g><pattern id="p" {PAT}><use href="#g"/></pattern><rect width="50" height="50" fill="url(#p)"/>"##
    ));
}

/// Review 6: shapes that don't loop, or that usvg stops itself, are drawn.
#[test]
fn references_without_loops_are_accepted() {
    let ok = |body: &str| {
        let r = parse(pic(body).as_bytes());
        assert!(r.is_ok(), "{body}: {:?}", r.err());
    };
    // A pattern whose marker draws the pattern (usvg doesn't build a marker inside itself).
    ok(&format!(
        r##"<pattern id="p" {PAT}><path d="M0 0L5 5L9 0" stroke="black" marker-mid="url(#m)"/></pattern><marker id="m" markerWidth="4" markerHeight="4"><rect width="2" height="2" fill="url(#p)"/></marker><rect width="50" height="50" fill="url(#p)"/>"##
    ));
    // A `<use>` in a pattern that links to the shape using the pattern.
    ok(&format!(r##"<rect id="r" width="50" height="50" fill="url(#p)"/><pattern id="p" {PAT}><use href="#r"/></pattern>"##));
    // A style sheet's two-hop masks.
    ok(
        r##"<style>.a{mask:url(#m2)}.b{mask:url(#m1)}</style><mask id="m1" maskUnits="userSpaceOnUse" x="0" y="0" width="50" height="50"><rect class="a" width="50" height="50" fill="white"/></mask><mask id="m2" maskUnits="userSpaceOnUse" x="0" y="0" width="50" height="50"><rect width="50" height="50" fill="white"/></mask><rect class="b" width="50" height="50"/>"##,
    );
    // Chains that end, a gradient pattern, a clip path inside a masked group.
    ok(&format!(
        r##"<linearGradient id="g"><stop offset="0" stop-color="red"/><stop offset="1" stop-color="blue"/></linearGradient><clipPath id="c"><rect width="30" height="30"/></clipPath><pattern id="p" {PAT} href="#q"/><pattern id="q" {PAT}><rect width="5" height="5" fill="url(#g)" clip-path="url(#c)"/></pattern><g fill="url(#g)" clip-path="url(#c)"><rect width="50" height="50" fill="url(#p)"/></g>"##
    ));
}

/// Review 6 I-1: an Illustrator-style logo (style sheet with `url(`s, clip paths, a text mask,
/// many paths) is an ordinary picture.
#[test]
fn illustrator_style_logo_is_accepted() {
    let clips: String = (0..5).map(|i| format!(r##"<clipPath id="clippath-{i}"><rect width="100" height="100"/></clipPath>"##)).collect();
    let paths: String = (0..200).map(|i| format!(r##"<path class="cls-1" d="M{i} 0h5v5z"/>"##)).collect();
    let d = pic(&format!(
        r##"<defs><style>.cls-1{{fill:url(#lg)}}.cls-2{{clip-path:url(#clippath-0)}}.cls-3{{clip-path:url(#clippath-1)}}.cls-4{{mask:url(#mask)}}</style><linearGradient id="lg"><stop offset="0" stop-color="#f00"/><stop offset="1" stop-color="#00f"/></linearGradient>{clips}<mask id="mask" maskUnits="userSpaceOnUse" x="0" y="0" width="100" height="100"><rect width="100" height="100" fill="#fff"/><text y="50" font-size="10">ACME Studio</text></mask></defs><g class="cls-2"><g class="cls-3">{paths}</g></g><g class="cls-4"><rect width="100" height="100" fill="#ccc"/></g><text y="90" font-size="8">ACME Studio, since 1920</text>"##
    ));
    let r = parse(d.as_bytes());
    assert!(r.is_ok(), "{:?}", r.err());
}

/// Review 6 M-5: text in a pattern is built again for every element the pattern paints.
#[test]
fn text_in_patterns_counts_per_use() {
    let pattern = format!(r##"<pattern id="p" {PAT}><text y="9">TEXT</text></pattern>"##);
    assert!(refused_quickly(&picture_with_text(&many_users(&pattern, r##"fill="url(#p)""##, 2000), 10_000)), "pattern text");
    let few = picture_with_text(&many_users(&pattern, r##"fill="url(#p)""##, 5), 20);
    assert!(parse(few.as_bytes()).is_ok());
}

fn refused_as_elements(doc: &str) -> bool {
    let t = std::time::Instant::now();
    let r = parse(doc.as_bytes());
    let fast = t.elapsed() < std::time::Duration::from_secs(5);
    matches!(&r, Err(Error::Parse(m)) if m.contains("this many elements") || m.contains("draws this many")) && fast
}

/// Review 6 C-2: markers and clip paths copy their content (not only text) per vertex or per user.
#[test]
fn elements_built_through_markers_and_clips_are_capped() {
    // 10,000 rects in a marker, 100,000 vertices (a file of about half a megabyte).
    let marker = format!(
        r##"<marker id="m" overflow="visible">{}</marker><path d="M0 0{}" stroke="black" marker-mid="url(#m)"/>"##,
        r#"<rect width="1" height="1"/>"#.repeat(10_000),
        "h1".repeat(100_000)
    );
    assert!(refused_as_elements(&pic(&marker)));
    // Clip path shapes times users.
    let clip = format!(r##"<clipPath id="c" clipPathUnits="objectBoundingBox">{}</clipPath>"##, r#"<rect width="1" height="1"/>"#.repeat(5000));
    assert!(refused_as_elements(&pic(&many_users(&clip, r##"clip-path="url(#c)""##, 2000))));
    // A chart's dot markers on 2000 points open.
    let points: String = (0..2000).map(|i| format!("{i},{} ", i % 7)).collect();
    let chart =
        pic(&format!(r##"<marker id="m"><circle r="2"/><circle r="1"/><circle r="3"/></marker><polyline points="{points}" marker-mid="url(#m)"/>"##));
    let r = parse(chart.as_bytes());
    assert!(r.is_ok(), "{:?}", r.err());
}

/// Review 7 (n1): an earlier element with the same id doesn't hide a pattern from the check.
#[test]
fn loop_behind_a_duplicate_id() {
    let d = pic(&format!(
        r##"<defs><g id="p"/><pattern id="p" {PAT}><rect width="5" height="5" mask="url(#m)"/></pattern><mask id="m" maskUnits="userSpaceOnUse" x="0" y="0" width="50" height="50"><rect width="50" height="50" fill="url(#p)"/></mask></defs><rect width="50" height="50" fill="url(#p)"/>"##
    ));
    assert!(refused_as_loop(&d));
    let later = d.replace(r##"<g id="p"/>"##, "").replace("</defs>", r##"<g id="p"/></defs>"##);
    assert!(refused_as_loop(&later));
}

/// Review 7 (n2): a filter image draws its target's whole subtree, which may use the filter.
#[test]
fn loop_below_a_filter_image_target() {
    let d = pic(
        r##"<filter id="f"><feImage href="#g"/></filter><g id="g"><rect width="5" height="5" filter="url(#f)"/></g><rect width="50" height="50" filter="url(#f)"/>"##,
    );
    assert!(refused_as_loop(&d));
}

/// Review 7 (n3): style sheets are read in any namespace, as usvg reads them.
#[test]
fn loop_through_a_sheet_in_another_namespace() {
    let m = |a: &str, c: &str| {
        format!(
            r##"<mask id="{a}" maskUnits="userSpaceOnUse" x="0" y="0" width="50" height="50"><rect class="{c}" width="50" height="50" fill="white"/></mask>"##
        )
    };
    let d = pic(&format!(
        r##"<x:style xmlns:x="urn:x">.a{{mask:url(#m2)}} .b{{mask:url(#m3)}} .c{{mask:url(#m1)}}</x:style><defs>{}{}{}</defs><rect width="50" height="50" mask="url(#m1)"/>"##,
        m("m1", "a"),
        m("m2", "b"),
        m("m3", "c")
    ));
    assert!(refused_as_loop(&d));
}

/// Review 7 (n4): a comment in a `style` attribute doesn't hide the property.
#[test]
fn loop_through_a_style_attribute_with_a_comment() {
    let d = pic(&format!(
        r##"<g style="/**/fill:url(#p)"><pattern id="p" {PAT}><rect width="5" height="5"/></pattern><rect width="50" height="50"/></g>"##
    ));
    assert!(refused_as_loop(&d));
}

/// Review 7 (n5): a pattern's template is the first `href` attribute, whichever the `<use>` rule picks.
#[test]
fn loop_through_either_href() {
    let d = pic(&format!(
        r##"<pattern id="s" {PAT} href="#t" xlink:href="#u"/><pattern id="t" {PAT}><rect width="5" height="5" fill="url(#s)"/></pattern><pattern id="u" {PAT}><rect width="5" height="5"/></pattern><rect width="50" height="50" fill="url(#s)"/>"##
    ));
    assert!(refused_as_loop(&d));
}

/// Review 7 (n6): the same for a filter image.
#[test]
fn loop_through_either_filter_image_href() {
    let d = pic(&format!(
        r##"<defs><filter id="f"><feImage href="#r" xlink:href="#ok"/></filter><rect id="r" width="5" height="5" fill="url(#p)"/><rect id="ok" width="5" height="5"/><pattern id="p" {PAT}><rect width="5" height="5" filter="url(#f)"/></pattern></defs><rect width="50" height="50" filter="url(#f)"/>"##
    ));
    assert!(refused_as_loop(&d));
}

/// Review 7 (n7): a marker beside a loop that doesn't go through it must not hide the loop.
#[test]
fn loop_beside_a_marker() {
    let d = pic(&format!(
        r##"<defs><pattern id="p" {PAT}><path d="M0 0L5 5L9 0" stroke="black" marker-mid="url(#m)"/><rect width="5" height="5" mask="url(#q)"/></pattern><marker id="m" markerWidth="4" markerHeight="4"><rect width="2" height="2" mask="url(#q)"/></marker><mask id="q" maskUnits="userSpaceOnUse" x="0" y="0" width="50" height="50"><rect width="50" height="50" fill="url(#p)"/></mask></defs><rect width="50" height="50" fill="url(#p)"/>"##
    ));
    assert!(refused_as_loop(&d));
}

/// Review 7 C-3: a marker holding a long path, used by a long path, builds a segment per segment pair.
#[test]
fn big_paths_in_markers_are_weighted() {
    let long = format!("M0 0{}", "h1".repeat(60_000));
    let d = pic(&format!(r##"<marker id="m" overflow="visible"><path d="{long}"/></marker><path d="{long}" stroke="black" marker-mid="url(#m)"/>"##));
    let t = std::time::Instant::now();
    let r = parse(d.as_bytes());
    assert!(matches!(&r, Err(Error::Parse(m)) if m.contains("many elements")), "{:?}", r.err());
    assert!(t.elapsed() < std::time::Duration::from_secs(5));
}

fn refused_as_selectors(doc: &str) -> bool {
    let t = std::time::Instant::now();
    let r = parse(doc.as_bytes());
    matches!(&r, Err(Error::Parse(m)) if m.contains("selectors")) && t.elapsed() < std::time::Duration::from_secs(5)
}

/// Review 7 C-4: simplecss matches a selector by recursion along its combinators, deep enough to
/// overflow the stack; in any namespace, with or without clip paths and the like.
#[test]
fn long_selectors_are_refused() {
    let sel = "*+".repeat(100_000);
    let plain = pic(&format!("<style>{sel}*{{fill:red}}</style>{}", "<g/>".repeat(100_000)));
    assert!(refused_as_selectors(&plain));
    let foreign = pic(&format!(
        r##"<x:style xmlns:x="urn:x">{sel}*{{fill:red}}</x:style><clipPath id="c"><rect width="5" height="5"/></clipPath><rect width="9" height="9" clip-path="url(#c)"/>{}"##,
        "<g/>".repeat(100_000)
    ));
    assert!(refused_as_selectors(&foreign));
}

/// Review 7 I-3: a descendant selector with many hops against a deep tree backtracks forever.
#[test]
fn backtracking_selectors_are_refused() {
    let deep = format!("{}<rect width=\"5\" height=\"5\"/>{}", "<g>".repeat(200), "</g>".repeat(200));
    let d = pic(&format!("<style>q g g g g g g g g g g g g rect{{fill:red}}</style>{deep}"));
    assert!(refused_as_selectors(&d));
    // A few hops in an ordinary tree are fine.
    let ok = pic("<style>g g rect{fill:red} g > rect{fill:blue} .a + .b{fill:green}</style><g><g><rect width=\"5\" height=\"5\"/></g></g>");
    assert!(parse(ok.as_bytes()).is_ok());
}

/// Review 7 I-3: a gradient template chain that comes back to a later link never ends in usvg.
#[test]
fn gradient_template_chains_that_loop_are_refused() {
    let d = pic(
        r##"<linearGradient id="a" href="#b"/><linearGradient id="b" href="#c"/><linearGradient id="c" href="#b"><stop offset="0" stop-color="red"/></linearGradient><rect width="50" height="50" fill="url(#a)"/>"##,
    );
    assert!(refused_as_loop(&d));
    let ok = pic(
        r##"<linearGradient id="a" href="#b"/><linearGradient id="b"><stop offset="0" stop-color="red"/></linearGradient><rect width="50" height="50" fill="url(#a)"/>"##,
    );
    assert!(parse(ok.as_bytes()).is_ok());
}

/// Review 7 I-2: a web font and a long comment in a sheet are ordinary.
#[test]
fn sheets_with_fonts_and_comments_are_accepted() {
    let font = "A".repeat(40_000);
    let d = pic(&format!(
        r##"<style>@font-face{{font-family:"F";src:url(data:font/woff2;base64,{font})}} .a{{clip-path:url(#c)}}</style><clipPath id="c"><rect width="5" height="5"/></clipPath><rect class="a" width="9" height="9"/>"##
    ));
    let r = parse(d.as_bytes());
    assert!(r.is_ok(), "{:?}", r.err());
    let comment = "x".repeat(2800);
    let d = pic(&format!(
        r##"<style>/* generated by {comment} */ .a{{clip-path:url(#c)}}</style><clipPath id="c"><rect width="5" height="5"/></clipPath><rect class="a" width="9" height="9"/>"##
    ));
    let r = parse(d.as_bytes());
    assert!(r.is_ok(), "{:?}", r.err());
}
