# Hungarian interface and proofing

> **Last reviewed:** 2026-10-11 · **Last updated:** 2026-10-18 · **Change:** minor (Hungarian offline proofing, attribution and pinned dictionary updates); previously minor (Hungarian interface catalog and regression checks) · **Target:** WordCraft 0.5.0

Choose **File › Options › Interface language › Magyar**, or follow a Hungarian system display
language (`hu-HU`, `hu_HU.UTF-8`). The catalog code is `hu`; `ui.language` with `{"value":"hu"}`
selects it explicitly, and `WORDCRAFT_LANGUAGE=hu-HU` does the same for a headless screenshot or a
launch while the preference is `auto`. The choice is saved with the other interface settings.
Document text, command ids, CLI/MCP/control responses and file paths do not change.

`crates/ui-egui/src/i18n/hu.tsv` has 2,419 entries: every key of the Russian catalog (built-in style
names, equation gallery labels, paper sizes, watermarks and greetings included) plus the citation
style names. It is written from the meaning of the English labels; no proprietary translation resources were consulted. The catalog
format has no plural forms, and Hungarian doesn't need them: a noun after a number stays singular
(`0 szó`, `1 szó`, `21 szó`). Placeholders, `\n` breaks in ribbon labels and dialog ellipses are
kept, and strings added later fall back to English until they are translated.

## Conventions

- Commands are nouns (`Mentés`, `Másolás`, `Beillesztés`); messages address the user with the
  informal second person (`Mentsd a dokumentumot`).
- Only the first word of a label is capitalised, as Hungarian spelling requires
  (`Összes módosítás elfogadása`).
- Decimal comma (`1,5-es sorköz`) and Hungarian quotation marks („…”).
- A placeholder never takes a case ending, because the value is not known when the text is
  written. The wording puts it after a colon or in parentheses instead
  (`Undo {action}` → `Visszavonás: {action}`, `Couldn't open {path}: {error}` →
  `Nem sikerült megnyitni: {path} ({error})`).
- Text columns and table columns are different words: `Hasábok` (Layout › Columns, column break)
  and `Oszlopok` (table rows and columns).

## Terminology

| Source | Hungarian |
|---|---|
| Home / Insert / Draw / Design / Layout | Kezdőlap / Beszúrás / Rajzolás / Tervezés / Elrendezés |
| References / Mailings / Review / View / Help | Hivatkozások / Levelezés / Véleményezés / Nézet / Súgó |
| Font / Paragraph / Styles | Betűtípus / Bekezdés / Stílusok |
| Header / Footer / Heading | Élőfej / Élőláb / Címsor |
| Footnote / Endnote / Caption | Lábjegyzet / Végjegyzet / Felirat |
| Table of Contents / Index | Tartalomjegyzék / Tárgymutató |
| Citation / Bibliography | Forráshivatkozás / Irodalomjegyzék |
| Link / Hyperlink | Link |
| Track Changes / Accept / Reject | Változáskövetés / Elfogadás / Elutasítás |
| Comment / Markup | Megjegyzés / Jelölések |
| Mail Merge / Recipients / Merge Field | Körlevél / Címzettek / Egyesítési mező |
| Columns (text) / Columns (table) | Hasábok / Oszlopok |
| Section / Section Break | Szakasz / Szakasztörés |
| Undo / Redo / Repeat | Visszavonás / Újra / Ismétlés |
| Undo {action} | Visszavonás: {action} |
| Page {page} of {pages} | Oldal: {page}/{pages} |
| Ribbon / Pane / Quick Access Toolbar | Menüszalag / Panel / Gyorselérési eszköztár |
| Proofing / Word Count | Nyelvi ellenőrzés / Szavak száma |

Product and format names stay as they are (WordCraft, PDF, DOCX, Markdown, MCP, Zotero). The Font
dialog's preview sample is `AaBbYyZz ŐőŰű`, so the All Caps preview shows the Hungarian double
acute capitals.

## Verification and remaining scope

`cargo test -p wordcraft-ui-egui --lib i18n::tests` covers Hungarian locale tags and preference
order, saving and restoring `hu`, unchanged document content, every key of the Russian catalog,
built-in style names, placeholders, count wording, the font sample's capitals, English fallback,
and the glyphs `ÁáÉéÍíÓóÖöŐőÚúÜüŰű` in the bundled Inter (Regular, Medium, SemiBold) and JetBrains Mono. The shared catalog checks
(validation, every tab and command translated) include Hungarian automatically, and the on-demand
interface audit (`untranslated_interface_text`) checks Hungarian alongside Russian.

Hungarian spelling now works offline from the bundled Magyar Ispell 1.9 dictionary.
A Hungarian interface uses `hu-HU` for new documents; existing document languages are preserved.
For existing text choose Review › Language › Hungarian (or Set Proofing Language).
Spelling suggestions handle accents, inflections and compounds. Grammar checks are limited to
possible repeated words and spacing around punctuation; they are not a full Hungarian grammar
or style checker. Hungarian automatic hyphenation is disabled until suitable patterns are added,
so English patterns cannot introduce incorrect Hungarian breaks.

The original dictionary files are from LibreOffice/dictionaries, by László Németh and Ferenc Godó.
They are used under MPL-2.0, with the original notices, license text and pinned provenance in
`assets/spelling/hu/`. Spellbook 0.4.2 is used under MPL-2.0; the two local safety fixes and upstream
source are documented in `vendor/spellbook/WORDCRAFT-NOTICE.txt`. About › Third-party licenses
shows the notices and MPL text. See [dictionary updates](spelling-dictionaries.md#updating-the-bundled-hungarian-dictionary).

Tests use the real bundled dictionary, including `árvíztűrő`, `tükörfúrógép`, `házainkban`,
`muszály` → `muszáj`, `hejesírás` → `helyesírás`, mixed-language byte ranges and arbitrary Unicode.
A native-speaker review of the catalog and proofing behavior is welcome.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-18 | minor | Hungarian offline spelling, basic grammar, source/license notices and a pinned LibreOffice updater |
| 2026-10-11 | minor | Hungarian catalog (2,419 entries: built-in style names, equation gallery, citation styles), conventions, terminology, scope and regression checks |
