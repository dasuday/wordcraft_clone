# ODT writer

> **Status:** styles, page layouts, master pages, headers and footers from the full model; lists,
> tables and the other content as the exchange-flow writer wrote them (#2669)

`odt::export` writes OpenDocument Text (ODF 1.3) from the full `wordcraft_doc::Document`, the
way the DOCX writer does, instead of lowering it to the exchange `Flow` first (which Markdown,
HTML and RTF still use). The writer lives in `crates/formats/src/odt/write/`. The reader
(`odt::import`) stays on `Flow`.

A document with tracked changes (or with Track Changes on) is still written from the `Flow`
(`odt::export_flow`), which carries the revisions, until the writer writes them itself.

## Architecture

`write::write(doc)` walks the document once and builds the two XML parts:

1. `pages::plan` gives every section a master page (`Standard` for the first, `MP2`, `MP3`…) and
   splits the body into segments. A continuous section that changes nothing a page holds stays
   on the master page before it, as a `text:section` when its columns differ from the page's.
2. `text::body` writes the body into `content.xml`. The block that starts a section carries the
   master page and the page-number restart on its automatic style
   (`style:master-page-name`, `style:page-number`).
3. `pages::master_styles` writes the page layouts and master pages into `styles.xml`, with the
   headers and footers inside them.
4. `styles::common` writes `office:styles`: the default style and the document's paragraph and
   character styles.
5. `package::zip` packages `content.xml`, `styles.xml`, `meta.xml`, `settings.xml`, the
   manifest and the pictures.

Every story (body, header, footer, cell) goes through `text::blocks`, which keeps the waiting
page start out of it, bounds nesting and ends the story with a paragraph after a final table,
as Word does.

`Ctx` (`mod.rs`) is the state the parts share: `auto_style` and `auto` make automatic styles,
deduplicated, in the file being written (`target`: automatic styles of headers and footers
belong in `styles.xml`, their names start with `M`); `font` declares a font face; `picture`
packages a `Document::media` entry once; `name` makes unique numbered names; `page_start` is the
master page waiting for the next body paragraph or table.

### Modules

| File | Writes |
|---|---|
| `package.rs` | zip, manifest, `meta.xml`, `settings.xml`, font declarations |
| `styles.rs` | the default style, named paragraph and character styles with `based on`, automatic paragraph and text styles, tab stops, borders |
| `pages.rs` | page layouts and master pages per section: size, orientation, margins, gutter, columns, page number format, header and footer areas; default, first-page and even-page headers and footers inherited as Word links them; columns-only continuous sections as `text:section` |
| `text.rs` | blocks, paragraphs and headings, spans, links, bookmarks, tabs, line, page and column breaks, ruby; pictures in line, `PAGE` and `NUMPAGES` fields, other fields as their result, equations as their linear text |
| `lists.rs` | lists as the `Flow` writer wrote them: a generated list style per list, bullets or `1.`/`a.`/`i.` per level |
| `tables.rs` | tables as the `Flow` writer wrote them: grid widths, header rows, spans, cell shading, a thin border on every cell |

Lists from the numbering definitions, the other fields, notes, comments, drawings, the tables'
own formatting and tracked changes come in the next parts of #2669.

## LibreOffice behaviours the writer works around

The reference is LibreOffice opening the DOCX of the same document: the ODT must lay out the
same.

- `config:config-item-set config:name="ooo:configuration-settings"` is a qualified name: without
  `xmlns:ooo` on the root of `settings.xml` every setting in it is ignored. The settings are the
  layout options LibreOffice gives a document it opens from Word formats.
- Tab stops are relative to the paragraph indent unless `TabsRelativeToIndent` is false; the
  model measures them from the text column, so `settings.xml` turns it off.
- The space above, the space below and contextual spacing are one property: one set alone is
  completed from the default paragraph style, not from the parent. Styles and paragraphs that
  set any of them get all three, resolved along their style chain.
- A child style's tab stops replace its parent's in ODF and add to them in the model: each style
  writes its whole merged list.
- Widow and orphan control is on when the model leaves it unset; ODF's default is off, so the
  default style says so.
- Word measures the header from the page edge and the body from the top margin; ODF puts the
  page margin at the header. With a header, the page margin is the header distance and the
  header is at least as high as the rest of the margin (footers alike).
- A paragraph ending with a page break: Word draws its mark beside the break and starts the next
  block on the new page, so the break goes on the next block, not on an empty paragraph.

## Checking

- Unit tests (`crates/formats/src/odt/write/tests.rs`) build synthetic documents through the
  model API and read the package back with a small normalizer: styles of both files resolved
  along their parents to the properties in effect, paragraphs with their text, master pages
  with their headers and footers. Every package is checked as an ODF package (`mimetype` first
  and stored, the manifest listing exactly the files there are); with `ODF_JING` (a jing jar)
  and `ODF_SCHEMA_DIR` (the OASIS ODF 1.3 RelaxNG schemas, not vendored) set, every part is also
  validated against the schema.
- `crates/engine/tests/odt_lo_render.rs` (ignored by default; `-- --ignored`) saves a synthetic
  document as `.docx` and `.odt`, has LibreOffice render both to PDF and compares pages and line
  positions (`pdftotext -bbox-layout`): three sections with headers, a different first page,
  page number footers and a landscape part restarting the numbers; named styles with spacing and
  inheritance; tab stops with leaders; a split paragraph. Every line matches within 1 pt.
  `WORDCRAFT_SOFFICE` points at LibreOffice, `WORDCRAFT_LO_KEEP=DIR` keeps the files.
- The round trips in `crates/formats/src/tests.rs` and the reader's tests pass through the new
  writer.
