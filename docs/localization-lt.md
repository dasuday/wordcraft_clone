# Lithuanian interface

> **Last reviewed:** 2026-10-17 · **Last updated:** 2026-10-17 · **Change:** minor (Lithuanian interface catalog and regression checks) · **Target:** WordCraft 0.7.0 source

Select **File › Options › Interface language › Lietuvių**, or use a Lithuanian system display
language (`lt`, `lt-LT`, `lt_LT.UTF-8`). The catalog code is `lt`. The saved `ui.language`
preference accepts `{"value":"lt"}`; `WORDCRAFT_LANGUAGE=lt-LT` also selects Lithuanian when
the interface preference is automatic. Generic locale matching handles region, script and case.

The catalog contains 2,616 entries: every existing catalog's `(context, English source)` pair,
including `preview` and `template` contexts, built-in styles, equation galleries, printing and
accessibility. It is an AI-assisted draft written from the meaning of this repository's English
strings for @ulrichas with Codex. No other product's Lithuanian interface or terminology database
was consulted or copied. Native-speaker review and review of the running app are pending.

## Scope and conventions

This change translates the interface only. It adds no Lithuanian spelling dictionary, grammar,
hyphenation, AutoCorrect rules or document templates. The registry's `editing_lang` remains
`None`; selecting the interface does not select Lithuanian proofing or change document text.
Command ids, file paths, control/CLI/MCP messages and custom style names keep their existing values.

- Commands use the infinitive (`Įrašyti`, `Įterpti`); labels use sentence case.
- Count labels avoid inflection based on numbers: `Žodžių skaičius: {words}` and
  `Pažymėta žodžių: {selected} iš {words}` work for 1, 2, 10, 11, 21 and larger counts.
- Keep placeholders, escaped line breaks in ribbon buttons and dialog ellipses intact.
- Use Lithuanian quotation marks (`„…“`) and decimal commas in fixed numeric labels.
- Product names, formats, technology names and paper sizes keep their established spelling.
  `Letter` stays `Letter` for paper; the `template` context translates it as `Laiškas`.
- New strings without a catalog entry fall back to English, as in the other interfaces.

## Terminology

| English | Lithuanian |
|---|---|
| File / Home / Insert / Draw | Failas / Pradžia / Įterpti / Piešti |
| Design / Layout / References / Mailings | Dizainas / Maketas / Nuorodos / Siuntimas |
| Review / View / Help | Recenzavimas / Rodinys / Žinynas |
| Font / Paragraph / Style | Šriftas / Pastraipa / Stilius |
| Header / Footer / Heading | Puslapinė antraštė / Poraštė / Antraštė |
| Footnotes / Endnotes | Puslapinės išnašos / Galinės išnašos |
| Save / Undo / Redo | Įrašyti / Anuliuoti / Grąžinti |
| Select / Selection | Pažymėti / Pažymėta sritis |
| Table of Contents / Index / Bibliography | Turinys / Rodyklė / Literatūros sąrašas |
| Track Changes / Mail Merge | Sekti pakeitimus / Laiškų suliejimas |

## Review and verification

The shared catalog and ribbon-command tests include Lithuanian through the registry. Lithuanian
regressions cover locale tags, saved preferences, document preservation, complete context/source
coverage, English fallback, template contexts, count wording and the bundled fonts' Lithuanian
glyphs. The interface audit includes `lt` among catalogs required to contain every looked-up key.

Local checks verified all 2,616 catalog rows, duplicates, escapes, placeholders, trailing ellipses,
Unicode normalization, reference coverage and Lithuanian glyphs in the four bundled interface
font faces. `cargo xtask ci`, the Rust regression tests and a running-app screenshot could not be
run in the preparation environment because it has no Rust toolchain. They remain required before
merge, along with native-speaker review. No passing Rust or visual check is claimed here.

Some English keys have multiple meanings without a translation context. In particular, `Outline`
uses `Struktūra` for the document outline, although the same key also labels a font outline effect;
`Open` uses `Atidaryti` for opening files, although it also names a spacing preset. These shared
limitations should be reviewed in the app before calling the interface complete.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-17 | minor | Lithuanian interface draft, locale registration, catalog/font regressions and audit coverage; proofing excluded |
