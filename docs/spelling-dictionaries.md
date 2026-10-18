# Installing spelling dictionaries

> **Last reviewed:** 2026-10-16 · **Last updated:** 2026-10-18 · **Change:** minor (Hungarian offline proofing, attribution and pinned dictionary updates); previously trivial (ISO 8859-1 dictionaries are read, #2824; previously major: first version)

English and Hungarian spelling work out of the box. Polish, Russian, Portuguese, Czech, Italian, Croatian and
Turkish spelling use a standard **Hunspell** dictionary (an `.aff` and a `.dic` file) that
WordCraft reads at runtime. These dictionaries are not bundled: they are large, and several are
GPL or LGPL licensed. Without one, the language isn't checked. The status bar then shows the
language with **(no dictionary)**, for example *Português (Brasil) (no dictionary)*.

If a dictionary is installed but can't be read (an unsupported `SET` encoding, a file over the
size limit, a parse error), the language isn't checked either, and the status bar shows
**(dictionary unreadable)** instead. Its tooltip gives the `.dic` file and the reason, and
**File › Options › Proofing** lists the same reason next to the file.

Install a dictionary once, restart WordCraft, and the language is checked as you type.

## Updating the bundled Hungarian dictionary

Hungarian uses the unmodified `hu_HU.aff` and `hu_HU.dic` from
[LibreOffice/dictionaries](https://github.com/LibreOffice/dictionaries/tree/master/hu_HU),
Magyar Ispell 1.9 by László Németh and Ferenc Godó, under the **MPL-2.0 option**.
The original authors' README, MPL license and exact upstream commit/checksums accompany the
files in `assets/spelling/hu/`. It works offline on desktop and WASM; the dictionaries-folder
setting does not override the bundled Hungarian dictionary.

A maintainer can review a new LibreOffice commit and run:

```sh
cargo xtask update-hungarian <full-40-digit-LibreOffice-dictionaries-commit>
cargo test -p wordcraft-proof
cargo test -p wordcraft-engine hungarian_proofing
```

The updater fetches only from LibreOffice's repository over HTTPS, requires an immutable commit,
checks the MPL option in the upstream license statement, parses the candidate files and tests
representative correct/incorrect words **before** replacing bundled files. A changed license
statement or failed language check stops the import. It preserves all original bytes and notices,
and records the revision and SHA-256 hashes in `SOURCE.json`; review the diff before committing.
There are no automatic background downloads or uploads of document text.

The runtime adapter converts legacy byte flags to distinct Unicode identifiers in memory;
UTF-8 words and the original source files stay intact. Spellbook's local signed-syllable and
case-index fixes remain MPL-2.0; see `vendor/spellbook/WORDCRAFT-NOTICE.txt`.
When redistributing binaries, provide the corresponding MPL source (including modifications)
and the notices, and tell recipients where to obtain it, as described in `NOTICE`.

## Which files each language needs

WordCraft looks for the first of these file names (each name means a `<name>.aff` and
`<name>.dic` pair in the same folder):

| Language | File names | Common source |
|---|---|---|
| Polish | `pl_PL`, `pl-PL`, `pl` | SJP.PL |
| Russian | `ru_RU`, `ru-RU`, `ru` | LibreOffice `dict-ru` |
| Portuguese (Brazil and Portugal) | `pt_BR`, `pt-BR`, `pt` | VERO (LibreOffice `dict-pt-BR`) |
| Czech | `cs_CZ`, `cs-CZ`, `cs` | LibreOffice `dict-cs` |
| Italian | `it_IT`, `it_CH`, `it-IT`, `it` | LibreOffice `dict-it` |
| Croatian | `hr_HR`, `hr-HR`, `hr` | LibreOffice `dict-hr` |
| Turkish | `tr_TR`, `tr-TR`, `tr` | LibreOffice `dict-tr` |

Every Portuguese region uses the same `pt_BR` dictionary, so text marked *Português (Portugal)*
is checked with it too.

The files may be UTF-8 or the encoding named on the `.aff`'s `SET` line (ISO 8859-1, ISO 8859-2, KOI8-R
and Windows-1251 are read as well; the original VERO `pt_BR` is ISO 8859-1).

## Where WordCraft looks

The first folder holding a matching pair wins:

1. The folder set in **File › Options › Proofing › Dictionaries folder**. This is the easiest
   way on every system, and it works for an app started from the Start menu, Finder or the Dock.
   The same pane lists the dictionary file each language found.
2. The folder in the `WORDCRAFT_DICTIONARIES` environment variable.
3. The system's usual places:

| System | Folders |
|---|---|
| Windows | `%LOCALAPPDATA%\WordCraft\dictionaries`, `%LOCALAPPDATA%\hunspell`, `%APPDATA%\WordCraft\dictionaries`, `%APPDATA%\hunspell`, LibreOffice's `share\extensions\dict-*` under `Program Files` (on any drive) |
| macOS | `~/Library/Spelling`, `/Library/Spelling`, `/opt/homebrew/share/hunspell`, `/usr/local/share/hunspell`, `/Applications/LibreOffice.app/Contents/Resources/extensions/dict-*` |
| Linux and BSD | `~/.local/share/hunspell`, `/usr/share/hunspell`, `/usr/share/myspell`, `/usr/share/myspell/dicts`, `/usr/local/share/hunspell`, `/usr/local/share/myspell`, LibreOffice's `share/extensions/dict-*` under `/usr/lib/libreoffice` or `/opt/libreoffice` |

The web version can't read these folders, so these languages aren't spell-checked there.

## Example: Brazilian Portuguese (`pt_BR`)

VERO, the Brazilian Portuguese dictionary LibreOffice ships, is available from the
[LibreOffice dictionaries repository](https://github.com/LibreOffice/dictionaries/tree/master/pt_BR)
(LGPLv3 / MPL). Any of these ways works.

### If LibreOffice is installed

Nothing to do: WordCraft finds LibreOffice's `dict-pt-BR` extension on its own. Restart
WordCraft after installing LibreOffice.

### Windows

In PowerShell:

```powershell
$d = "$env:LOCALAPPDATA\WordCraft\dictionaries"
New-Item -ItemType Directory -Force $d | Out-Null
$base = "https://raw.githubusercontent.com/LibreOffice/dictionaries/master/pt_BR"
Invoke-WebRequest "$base/pt_BR.aff" -OutFile "$d\pt_BR.aff" -UseBasicParsing
Invoke-WebRequest "$base/pt_BR.dic" -OutFile "$d\pt_BR.dic" -UseBasicParsing
```

### macOS

```sh
mkdir -p ~/Library/Spelling
cd ~/Library/Spelling
curl -fLO https://raw.githubusercontent.com/LibreOffice/dictionaries/master/pt_BR/pt_BR.aff
curl -fLO https://raw.githubusercontent.com/LibreOffice/dictionaries/master/pt_BR/pt_BR.dic
```

### Linux

From the distribution's packages:

```sh
sudo apt install hunspell-pt-br   # Debian, Ubuntu
```

Or, without root, into your own folder:

```sh
mkdir -p ~/.local/share/hunspell
cd ~/.local/share/hunspell
curl -fLO https://raw.githubusercontent.com/LibreOffice/dictionaries/master/pt_BR/pt_BR.aff
curl -fLO https://raw.githubusercontent.com/LibreOffice/dictionaries/master/pt_BR/pt_BR.dic
```

Other languages work the same way: replace `pt_BR` with the language's folder and file names
from the LibreOffice repository (for example `it_IT/it_IT.aff`).

## Checking that it works

- Restart WordCraft. The status bar no longer says **(no dictionary)**, and a misspelled word
  such as `impramir` is underlined, with `imprimir` among its suggestions.
- **File › Options › Proofing** lists the file each language found.
- From the command line:

  ```sh
  wordcraft-cli run --cmd 'text.insert={"text":"Vou impramir o documento."}' --cmd select.all --cmd 'review.language={"lang":"pt-BR"}' --cmd review.issues --print
  ```

  The `review.issues` line lists `impramir` as a spelling issue with its suggestions.
- The log file (`%APPDATA%\WordCraft\logs\wordcraft.log` on Windows) says
  `Portuguese spelling: no Hunspell pt_BR dictionary installed` when none was found.

Never commit dictionary files to this repository: they stay on the user's system.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-18 | minor | Hungarian offline spelling, basic grammar, source/license notices and a pinned LibreOffice updater |
| 2026-10-18 | trivial | ISO 8859-1 (`SET ISO8859-1`) dictionaries are read, so the original VERO `pt_BR` works without converting it to UTF-8 (#2824) |
| 2026-10-18 | minor | An installed dictionary that can't be read shows **(dictionary unreadable)** in the status bar, with the file and reason in its tooltip and in File › Options › Proofing (#2830) |
| 2026-10-16 | major | First version: the file names per language, the search order and folders on Windows, macOS and Linux, installing `pt_BR` on each, and how to check it works |
