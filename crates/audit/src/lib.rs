//! Audit a `.docx` file for hidden information before sharing.
//!
//! [`audit_bytes`] scans the raw OPC package (the ZIP bytes) and reports what a
//! recipient of the file could learn about its authors: names on tracked changes
//! and comments, creator/last-modified-by in document properties, hidden text,
//! macro/embedded-object parts, and package parts nothing references.
//!
//! The audit is visibility only: it never modifies the file. Sanitizing (removing
//! what it finds) is a separate, future step.
//!
//! Detection notes (documented, not hidden):
//!
//! - Matching is by namespace URI, not by prefix, so a file that spells the
//!   WordprocessingML namespace with an unusual prefix is still scanned.
//! - `w:vanish` hides text only when its `w:val` is absent or true-ish
//!   (ECMA-376 §17.18.67 `ST_OnOff`); `w:val="false"` is not flagged.
//! - Only `word/document.xml` is scanned for revisions and hidden text in v1;
//!   headers, footers, footnotes and endnotes can carry the same constructs.
//! - A `dc:creator` / `cp:lastModifiedBy` value is reported as-is; producers
//!   that anonymize (WordCraft writes `Author`) are reported too, since the
//!   value is what a recipient sees.
//!
//! The audit fails closed: a part that cannot be read or parsed is an error,
//! never a silent pass.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Read};

use quick_xml::XmlVersion;
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::ResolveResult;
use quick_xml::reader::NsReader;
use serde::Serialize;

#[cfg(test)]
mod tests;

// ECMA-376 Part 1 namespaces the audit understands.
const NS_W: &[u8] = b"http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const NS_DC: &[u8] = b"http://purl.org/dc/elements/1.1/";
const NS_CP: &[u8] = b"http://schemas.openxmlformats.org/package/2006/metadata/core-properties";
const NS_EP: &[u8] = b"http://schemas.openxmlformats.org/officeDocument/2006/extended-properties";
const NS_RELS: &[u8] = b"http://schemas.openxmlformats.org/package/2006/relationships";

/// Cap on ZIP entries read: a package is a handful of parts, not millions.
const MAX_PARTS: usize = 100_000;
/// Cap on decompressed bytes read from a single part (zip-bomb guard).
const MAX_PART_BYTES: u64 = 256 * 1024 * 1024;
/// Cap on total decompressed bytes across all scanned parts. Per-part caps
/// alone don't bound N parts that each compress well (zip-bomb guard).
const MAX_TOTAL_BYTES: u64 = 1024 * 1024 * 1024;
/// Cap on XML elements walked in a single part.
const MAX_XML_ELEMENTS: u64 = 2_000_000;

/// What the audit found, as a stable machine-readable vocabulary.
///
/// The [`FindingKind::id`] strings are the crate's public contract: they appear
/// in CLI output and serialized reports. Do not rename them; add a new variant
/// instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum FindingKind {
    /// `w:author`/`w:initials` on a tracked change (the `CT_TrackChange` and
    /// `CT_MoveBookmark` elements, ECMA-376 Part 1 §17.13.5).
    RevisionAuthor,
    /// `w:author`/`w:initials`/`w:date` on a `w:comment` (ECMA-376 §17.13.4.2).
    CommentAuthor,
    /// `dc:creator` / `cp:lastModifiedBy` in `docProps/core.xml`
    /// (ECMA-376 Part 2 §12.3.1).
    CorePropertiesAuthor,
    /// `Company` / `Manager` in `docProps/app.xml` (ECMA-376 Part 2 §12.3.2).
    ExtendedProperties,
    /// A `w:vanish` run property (ECMA-376 §17.3.2.42): text that renders hidden.
    HiddenText,
    /// A VBA macro project part (by relationship type, or `word/vbaProject.bin`):
    /// the document carries macros.
    VbaProject,
    /// An embedded object part (by relationship type, or under
    /// `word/embeddings/`): an embedded (often OLE) object.
    OleEmbedding,
    /// A ZIP entry no relationship targets: leftover content nothing references.
    OrphanPart,
}

impl FindingKind {
    /// The stable machine-readable ID for this finding class.
    pub fn id(self) -> &'static str {
        match self {
            FindingKind::RevisionAuthor => "author-pii/revision",
            FindingKind::CommentAuthor => "author-pii/comment",
            FindingKind::CorePropertiesAuthor => "author-pii/core-properties",
            FindingKind::ExtendedProperties => "author-pii/extended-properties",
            FindingKind::HiddenText => "hidden-text",
            FindingKind::VbaProject => "active-content/vba",
            FindingKind::OleEmbedding => "active-content/ole",
            FindingKind::OrphanPart => "package/orphan-part",
        }
    }

    /// The human-readable title for this finding class.
    pub fn title(self) -> &'static str {
        match self {
            FindingKind::RevisionAuthor => "author name on tracked change",
            FindingKind::CommentAuthor => "author name on comment",
            FindingKind::CorePropertiesAuthor => "author name in document properties",
            FindingKind::ExtendedProperties => "organization name in extended properties",
            FindingKind::HiddenText => "hidden text",
            FindingKind::VbaProject => "VBA macro project",
            FindingKind::OleEmbedding => "embedded object",
            FindingKind::OrphanPart => "orphan package part",
        }
    }

    /// How much attention this class deserves: `warn` for identifying or
    /// executable content, `info` for leftovers worth knowing about.
    pub fn severity(self) -> &'static str {
        match self {
            FindingKind::OrphanPart | FindingKind::ExtendedProperties => "info",
            _ => "warn",
        }
    }
}

/// One thing the audit found.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AuditFinding {
    /// Stable finding-class ID (see [`FindingKind::id`]).
    pub id: String,
    /// Human-readable title (see [`FindingKind::title`]).
    pub title: String,
    /// `"warn"` or `"info"`.
    pub severity: String,
    /// Where it was found: the part name plus the element or attribute,
    /// e.g. `word/document.xml <w:ins>`.
    pub location: String,
    /// The matched value: an author name, a property value, a part name.
    pub detail: String,
}

impl AuditFinding {
    fn new(kind: FindingKind, location: String, detail: String) -> AuditFinding {
        AuditFinding { id: kind.id().to_string(), title: kind.title().to_string(), severity: kind.severity().to_string(), location, detail }
    }
}

/// Why the audit could not run.
#[derive(Debug, thiserror::Error)]
pub enum AuditError {
    /// The bytes are not a readable ZIP archive.
    #[error("not a readable ZIP archive: {0}")]
    Zip(String),
    /// A package part needed for the audit could not be parsed as XML.
    #[error("part {0}: {1}")]
    Xml(String, String),
    /// A hostile-input guard tripped (too many parts, part too large, …).
    #[error("{0}")]
    Limit(String),
}

/// Scan the raw `.docx` (OPC ZIP) bytes and return every finding.
///
/// Findings come back in a deterministic order (by class, location, detail).
/// Fails closed: unreadable or unparsable input is an [`AuditError`], never an
/// empty finding list that looks like a clean bill of health.
pub fn audit_bytes(bytes: &[u8]) -> Result<Vec<AuditFinding>, AuditError> {
    // One archive for the whole scan: re-opening it per part would re-parse
    // the central directory each time (quadratic on hostile input).
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| AuditError::Zip(e.to_string()))?;
    let names = part_names(&mut archive)?;
    let mut parts: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut total: u64 = 0;
    for name in names.iter().filter(|n| is_scanned_part(n)) {
        let data = read_part(&mut archive, name, &mut total)?;
        parts.insert(name.clone(), data);
    }

    let mut findings = Vec::new();
    if let Some(xml) = parts.get("word/document.xml") {
        scan_revisions_and_hidden_text(xml, &mut findings)?;
    } else {
        return Err(AuditError::Xml("word/document.xml".to_string(), "missing: not a Word document package".to_string()));
    }
    if let Some(xml) = parts.get("word/comments.xml") {
        scan_comments(xml, &mut findings)?;
    }
    if let Some(xml) = parts.get("docProps/core.xml") {
        scan_core_properties(xml, &mut findings)?;
    }
    if let Some(xml) = parts.get("docProps/app.xml") {
        scan_extended_properties(xml, &mut findings)?;
    }
    scan_active_content(&names, &parts, &mut findings)?;
    scan_orphans(&names, &parts, &mut findings)?;

    findings.sort_by(|a, b| (&a.id, &a.location, &a.detail).cmp(&(&b.id, &b.location, &b.detail)));
    Ok(findings)
}

/// Parts the audit parses as XML. Everything else is only listed by name.
fn is_scanned_part(name: &str) -> bool {
    name == "word/document.xml" || name == "word/comments.xml" || name == "docProps/core.xml" || name == "docProps/app.xml" || name.ends_with(".rels")
}

/// Every non-directory entry name in the ZIP, in archive order.
fn part_names(archive: &mut zip::ZipArchive<Cursor<&[u8]>>) -> Result<Vec<String>, AuditError> {
    let len = archive.len();
    if len > MAX_PARTS {
        return Err(AuditError::Limit(format!("too many ZIP entries: {len}")));
    }
    let mut names = Vec::with_capacity(len.min(1024));
    for i in 0..len {
        let file = archive.by_index(i).map_err(|e| AuditError::Zip(e.to_string()))?;
        let name = file.name().to_string();
        if !name.ends_with('/') {
            names.push(name);
        }
    }
    Ok(names)
}

/// Read one part, capped against decompression bombs, and charge it against
/// the cumulative budget. Fails closed: with a single archive for the whole
/// scan, a listed name that can't be read is an error, never a silent skip.
fn read_part(archive: &mut zip::ZipArchive<Cursor<&[u8]>>, name: &str, total: &mut u64) -> Result<Vec<u8>, AuditError> {
    let mut data = Vec::new();
    {
        let mut file = archive.by_name(name).map_err(|e| AuditError::Zip(format!("{name}: {e}")))?;
        let mut limited = (&mut file).take(MAX_PART_BYTES + 1);
        limited.read_to_end(&mut data).map_err(|e| AuditError::Zip(format!("{name}: {e}")))?;
    }
    if data.len() as u64 > MAX_PART_BYTES {
        return Err(AuditError::Limit(format!("part too large: {name}")));
    }
    *total = total.saturating_add(data.len() as u64);
    if *total > MAX_TOTAL_BYTES {
        return Err(AuditError::Limit(format!("package too large: {total} decompressed bytes over budget")));
    }
    Ok(data)
}

/// One element the walker saw: namespace URI, local name, decoded attributes.
struct Element {
    ns: Vec<u8>,
    local: String,
    /// (namespace URI, local name, decoded value) per attribute.
    attrs: Vec<(Vec<u8>, String, String)>,
}

/// Call `f` for every start/empty element in `xml`, resolving names by
/// namespace URI so producer-chosen prefixes do not matter.
fn for_each_element(xml: &[u8], part: &str, f: &mut impl FnMut(&Element) -> Result<(), AuditError>) -> Result<(), AuditError> {
    let mut reader = NsReader::from_reader(xml);
    {
        let config = reader.config_mut();
        config.trim_text_start = false;
        config.trim_text_end = false;
        config.expand_empty_elements = false;
        // Reject well-tokenized but mismatched closing tags: a malformed part
        // must fail the audit, never produce a partial (possibly clean-looking)
        // finding list.
        config.check_end_names = true;
    }
    let mut buf = Vec::new();
    let mut count: u64 = 0;
    let mut depth: u32 = 0;
    loop {
        buf.clear();
        let (resolution, event) = reader.read_resolved_event_into(&mut buf).map_err(|e| AuditError::Xml(part.to_string(), e.to_string()))?;
        // Copy the namespace URI out so the reader borrow ends here; the
        // attribute resolver below needs `&mut reader` again.
        let ns: Vec<u8> = match &resolution {
            ResolveResult::Bound(n) => n.0.to_vec(),
            _ => Vec::new(),
        };
        match event {
            Event::Start(ref e) => {
                depth = depth.saturating_add(1);
                count += 1;
                if count > MAX_XML_ELEMENTS {
                    return Err(AuditError::Limit(format!("{part}: too many XML elements")));
                }
                f(&element_of(&mut reader, &ns, e, part)?)?;
            }
            Event::Empty(ref e) => {
                count += 1;
                if count > MAX_XML_ELEMENTS {
                    return Err(AuditError::Limit(format!("{part}: too many XML elements")));
                }
                f(&element_of(&mut reader, &ns, e, part)?)?;
            }
            Event::End(_) => {
                depth = depth.saturating_sub(1);
            }
            Event::Eof => {
                if depth != 0 {
                    return Err(AuditError::Xml(part.to_string(), "unclosed elements at end of part".to_string()));
                }
                break;
            }
            _ => {}
        }
    }
    Ok(())
}

/// Resolve one start tag into an [`Element`].
fn element_of(reader: &mut NsReader<&[u8]>, ns: &[u8], e: &BytesStart<'_>, part: &str) -> Result<Element, AuditError> {
    let ns = ns.to_vec();
    let local = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
    let mut attrs = Vec::new();
    for attr in e.attributes() {
        let attr = attr.map_err(|err| AuditError::Xml(part.to_string(), err.to_string()))?;
        let key = attr.key;
        if key.as_ref() == b"xmlns" || key.as_ref().starts_with(b"xmlns:") {
            continue;
        }
        let (aresolution, alocal) = reader.resolver_mut().resolve_attribute(key);
        let ans = match aresolution {
            ResolveResult::Bound(ns) => ns.0.to_vec(),
            _ => Vec::new(),
        };
        let alocal = String::from_utf8_lossy(alocal.as_ref()).into_owned();
        let value = attr
            .normalized_value(XmlVersion::Explicit1_0)
            .map(|v| v.into_owned())
            .unwrap_or_else(|_| String::from_utf8_lossy(&attr.value).into_owned());
        attrs.push((ans, alocal, value));
    }
    Ok(Element { ns, local, attrs })
}

/// The value of the attribute with namespace `ns` and local name `local`, if present.
fn attr<'a>(el: &'a Element, ns: &[u8], local: &str) -> Option<&'a str> {
    el.attrs.iter().find(|(ans, aname, _)| ans.as_slice() == ns && aname == local).map(|(_, _, v)| v.as_str())
}

/// Collect the text content of every element matching (`ns`, `local`).
fn texts_of(xml: &[u8], part: &str, ns: &[u8], local: &str) -> Result<Vec<String>, AuditError> {
    let mut reader = NsReader::from_reader(xml);
    {
        let config = reader.config_mut();
        config.trim_text_start = false;
        config.trim_text_end = false;
        config.expand_empty_elements = false;
        // See `for_each_element`: malformed parts fail the audit.
        config.check_end_names = true;
    }
    let mut buf = Vec::new();
    let mut out = Vec::new();
    let mut depth: u32 = 0;
    let mut open: u32 = 0;
    let mut current = String::new();
    let mut count: u64 = 0;
    loop {
        buf.clear();
        let (resolution, event) = reader.read_resolved_event_into(&mut buf).map_err(|e| AuditError::Xml(part.to_string(), e.to_string()))?;
        match event {
            Event::Start(ref e) => {
                open = open.saturating_add(1);
                count += 1;
                if count > MAX_XML_ELEMENTS {
                    return Err(AuditError::Limit(format!("{part}: too many XML elements")));
                }
                let el_ns: &[u8] = match &resolution {
                    ResolveResult::Bound(n) => n.0,
                    _ => b"",
                };
                let el_local = e.local_name();
                let el_local = String::from_utf8_lossy(el_local.as_ref());
                if depth == 0 && el_ns == ns && el_local.as_ref() == local {
                    depth = 1;
                    current.clear();
                } else if depth > 0 {
                    depth = depth.saturating_add(1);
                }
            }
            Event::Empty(ref e) => {
                open = open.saturating_add(1);
                count += 1;
                if count > MAX_XML_ELEMENTS {
                    return Err(AuditError::Limit(format!("{part}: too many XML elements")));
                }
                let el_ns: &[u8] = match &resolution {
                    ResolveResult::Bound(n) => n.0,
                    _ => b"",
                };
                let el_local = e.local_name();
                let el_local = String::from_utf8_lossy(el_local.as_ref());
                if depth == 0 && el_ns == ns && el_local.as_ref() == local {
                    // An empty target element contributes its (empty) text.
                    out.push(String::new());
                } else if depth > 0 {
                    // Empty elements open and close immediately: no depth change.
                }
                open = open.saturating_sub(1);
            }
            Event::End(_) => {
                open = open.saturating_sub(1);
                if depth > 0 {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        out.push(std::mem::take(&mut current));
                    }
                }
            }
            Event::Text(ref t) => {
                if depth > 0 {
                    let decoded = t.xml10_content().map(|v| v.into_owned()).unwrap_or_else(|_| String::from_utf8_lossy(t.as_ref()).into_owned());
                    current.push_str(&decoded);
                }
            }
            Event::CData(ref t) => {
                if depth > 0 {
                    current.push_str(&String::from_utf8_lossy(t.as_ref()));
                }
            }
            Event::Eof => {
                if open != 0 {
                    return Err(AuditError::Xml(part.to_string(), "unclosed elements at end of part".to_string()));
                }
                break;
            }
            _ => {}
        }
    }
    Ok(out)
}

/// Local names of the WordprocessingML revision elements that carry `w:author`
/// (the `CT_TrackChange` and `CT_MoveBookmark` types plus the tracked
/// property-change types, ECMA-376 Part 1 §17.13.5).
/// Excluded: `w:tblGridChange` (`CT_TblGridChange`) and the `*RangeEnd`
/// markers (`CT_MarkupRange`), whose types define no `w:author`.
const REVISION_ELEMENTS: &[&str] = &[
    // Insertions, deletions, moves.
    "ins",
    "del",
    "moveFrom",
    "moveTo",
    // Move bookmarks.
    "moveFromRangeStart",
    "moveToRangeStart",
    // Table revisions.
    "cellIns",
    "cellDel",
    "cellMerge",
    // Custom XML markup revisions.
    "customXmlInsRangeStart",
    "customXmlDelRangeStart",
    "customXmlMoveFromRangeStart",
    "customXmlMoveToRangeStart",
    // Tracked property changes (run, paragraph, section, table, row, cell,
    // numbering): Word writes these for every tracked formatting change.
    "rPrChange",
    "pPrChange",
    "sectPrChange",
    "tblPrChange",
    "tblPrExChange",
    "trPrChange",
    "tcPrChange",
    "numberingChange",
];

/// Author-bearing revision elements and `w:vanish` runs in `word/document.xml`.
fn scan_revisions_and_hidden_text(xml: &[u8], findings: &mut Vec<AuditFinding>) -> Result<(), AuditError> {
    for_each_element(xml, "word/document.xml", &mut |el| {
        if el.ns.as_slice() != NS_W {
            return Ok(());
        }
        if REVISION_ELEMENTS.contains(&el.local.as_str()) {
            if let Some(author) = attr(el, NS_W, "author")
                && !author.is_empty()
            {
                let mut detail = format!("author=\"{author}\"");
                if let Some(initials) = attr(el, NS_W, "initials")
                    && !initials.is_empty()
                {
                    detail.push_str(&format!(" initials=\"{initials}\""));
                }
                findings.push(AuditFinding::new(FindingKind::RevisionAuthor, format!("word/document.xml <w:{}>", el.local), detail));
            }
            return Ok(());
        }
        // ECMA-376 §17.3.2.42 `CT_OnOff`: absent `w:val` means on; only an
        // explicit false-ish value means the text is not hidden.
        if el.local == "vanish" {
            let hidden = match attr(el, NS_W, "val") {
                None => true,
                Some(v) => !matches!(v.to_ascii_lowercase().as_str(), "false" | "off" | "0" | "f" | "no"),
            };
            if hidden {
                findings.push(AuditFinding::new(
                    FindingKind::HiddenText,
                    "word/document.xml <w:vanish>".to_string(),
                    "run property hides text".to_string(),
                ));
            }
        }
        Ok(())
    })
}

/// `w:comment` authors in `word/comments.xml` (ECMA-376 §17.13.4.2).
fn scan_comments(xml: &[u8], findings: &mut Vec<AuditFinding>) -> Result<(), AuditError> {
    for_each_element(xml, "word/comments.xml", &mut |el| {
        if el.ns.as_slice() == NS_W
            && el.local == "comment"
            && let Some(author) = attr(el, NS_W, "author")
            && !author.is_empty()
        {
            let mut detail = format!("author=\"{author}\"");
            if let Some(initials) = attr(el, NS_W, "initials")
                && !initials.is_empty()
            {
                detail.push_str(&format!(" initials=\"{initials}\""));
            }
            if let Some(date) = attr(el, NS_W, "date")
                && !date.is_empty()
            {
                detail.push_str(&format!(" date=\"{date}\""));
            }
            findings.push(AuditFinding::new(FindingKind::CommentAuthor, "word/comments.xml <w:comment>".to_string(), detail));
        }
        Ok(())
    })
}

/// `dc:creator` / `cp:lastModifiedBy` in `docProps/core.xml`.
fn scan_core_properties(xml: &[u8], findings: &mut Vec<AuditFinding>) -> Result<(), AuditError> {
    for (ns, local, element) in [(NS_DC, "creator", "dc:creator"), (NS_CP, "lastModifiedBy", "cp:lastModifiedBy")] {
        for value in texts_of(xml, "docProps/core.xml", ns, local)? {
            let value = value.trim();
            if !value.is_empty() {
                findings.push(AuditFinding::new(FindingKind::CorePropertiesAuthor, format!("docProps/core.xml <{element}>"), value.to_string()));
            }
        }
    }
    Ok(())
}

/// `Company` / `Manager` in `docProps/app.xml`: organizational, not personal,
/// but still identifying — reported at `info` severity.
fn scan_extended_properties(xml: &[u8], findings: &mut Vec<AuditFinding>) -> Result<(), AuditError> {
    for local in ["Company", "Manager"] {
        for value in texts_of(xml, "docProps/app.xml", NS_EP, local)? {
            let value = value.trim();
            if !value.is_empty() {
                findings.push(AuditFinding::new(FindingKind::ExtendedProperties, format!("docProps/app.xml <{local}>"), value.to_string()));
            }
        }
    }
    Ok(())
}

/// Relationship types that mark active content: the VBA project part and
/// embedded objects/packages (ECMA-376 Part 2 §11, plus the Office VBA extension).
const RT_VBA_PROJECT: &str = "http://schemas.microsoft.com/office/2006/relationships/vbaProject";
const RT_OLE_OBJECT: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/oleObject";
const RT_PACKAGE: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/package";

/// Macro and embedded-object parts: reported as visibility, never blocked or
/// removed. Detection is by relationship type first — a reachable VBA or OLE
/// part may live at any valid package path — with the fixed names and
/// directories as fallback for parts whose relationships are missing.
fn scan_active_content(names: &[String], parts: &BTreeMap<String, Vec<u8>>, findings: &mut Vec<AuditFinding>) -> Result<(), AuditError> {
    // Parts already reported, so the two passes never double-report one part.
    let mut reported: BTreeSet<String> = BTreeSet::new();
    let mut report = |kind: FindingKind, location: String, detail: &str, findings: &mut Vec<AuditFinding>| {
        if reported.insert(location.clone()) {
            findings.push(AuditFinding::new(kind, location, detail.to_string()));
        }
    };
    for name in names {
        if name == "word/vbaProject.bin" {
            report(FindingKind::VbaProject, name.clone(), "VBA macro project present", findings);
        } else if name.starts_with("word/embeddings/") {
            report(FindingKind::OleEmbedding, name.clone(), "embedded object present", findings);
        }
    }
    for name in names.iter().filter(|n| n.ends_with(".rels")) {
        let Some(xml) = parts.get(name) else { continue };
        let source_dir = source_dir_of_rels(name);
        for_each_element(xml, name, &mut |el| {
            if el.ns.as_slice() == NS_RELS
                && el.local == "Relationship"
                && attr(el, b"", "TargetMode") != Some("External")
                && let Some(target) = attr(el, b"", "Target")
            {
                let resolved = resolve_target(&source_dir, target);
                match attr(el, b"", "Type").unwrap_or("") {
                    RT_VBA_PROJECT => report(FindingKind::VbaProject, resolved, "VBA macro project present (relationship type)", findings),
                    RT_OLE_OBJECT | RT_PACKAGE => {
                        report(FindingKind::OleEmbedding, resolved, "embedded object present (relationship type)", findings)
                    }
                    _ => {}
                }
            }
            Ok(())
        })?;
    }
    Ok(())
}

/// ZIP entries no relationship targets: content nothing references.
/// Reachability is computed lexically from every `*.rels` part; external
/// relationship targets (`TargetMode="External"`) are not package parts.
fn scan_orphans(names: &[String], parts: &BTreeMap<String, Vec<u8>>, findings: &mut Vec<AuditFinding>) -> Result<(), AuditError> {
    // Reachability by traversal from the package root: only relationships in
    // reachable parts count. A `.rels` file inside an orphaned subtree must
    // not rescue its targets from orphan status.
    let name_set: BTreeSet<&str> = names.iter().map(String::as_str).collect();
    let mut reachable: BTreeSet<String> = BTreeSet::new();
    reachable.insert("[Content_Types].xml".to_string());
    let mut stack = vec!["_rels/.rels".to_string()];
    while let Some(rels_name) = stack.pop() {
        if !name_set.contains(rels_name.as_str()) || !reachable.insert(rels_name.clone()) {
            continue;
        }
        let Some(xml) = parts.get(&rels_name) else { continue };
        let source_dir = source_dir_of_rels(&rels_name);
        let mut targets = Vec::new();
        for_each_element(xml, &rels_name, &mut |el| {
            if el.ns.as_slice() == NS_RELS
                && el.local == "Relationship"
                && let Some(target) = attr(el, b"", "Target")
                && attr(el, b"", "TargetMode") != Some("External")
            {
                targets.push(resolve_target(&source_dir, target));
            }
            Ok(())
        })?;
        for target in targets {
            if name_set.contains(target.as_str()) && reachable.insert(target.clone()) && let Some(rels) = rels_of_part(&target) {
                stack.push(rels);
            }
        }
    }
    for name in names {
        if !reachable.contains(name) {
            findings.push(AuditFinding::new(FindingKind::OrphanPart, name.clone(), "no relationship targets this part".to_string()));
        }
    }
    Ok(())
}

/// The directory of the part a `*.rels` file describes:
/// `word/_rels/document.xml.rels` describes `word/document.xml`, whose
/// directory is `word`; `_rels/.rels` describes the package root.
fn source_dir_of_rels(rels: &str) -> String {
    let without_ext = rels.strip_suffix(".rels").unwrap_or(rels);
    match without_ext.rfind("/_rels/") {
        Some(i) => without_ext[..i].to_string(),
        None => String::new(),
    }
}

/// The `*.rels` path for a package part (`word/document.xml` →
/// `word/_rels/document.xml.rels`), per OPC §12.2.4. `None` for parts that
/// have no relationships part (`[Content_Types].xml`).
fn rels_of_part(part: &str) -> Option<String> {
    if part == "[Content_Types].xml" {
        return None;
    }
    let (dir, file) = match part.rfind('/') {
        Some(i) => (&part[..i], &part[i + 1..]),
        None => ("", part),
    };
    if file.is_empty() {
        return None;
    }
    Some(if dir.is_empty() { format!("_rels/{file}.rels") } else { format!("{dir}/_rels/{file}.rels") })
}

/// Resolve a relationship target against the source part's directory, lexically.
/// Absolute targets (`/word/...`) resolve from the package root.
fn resolve_target(source_dir: &str, target: &str) -> String {
    let joined = if let Some(abs) = target.strip_prefix('/') {
        abs.to_string()
    } else if source_dir.is_empty() {
        target.to_string()
    } else {
        format!("{source_dir}/{target}")
    };
    let mut segments: Vec<&str> = Vec::new();
    for segment in joined.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            s => segments.push(s),
        }
    }
    segments.join("/")
}
