# Local document recovery

Desktop WordCraft periodically keeps a local copy of unsaved work, including new drafts.
File › Recover lists available copies. The page opens at startup when copies are found.
Restore opens an unsaved document with no file path. Save As chooses where to keep it.
Recovery never writes to the original document. AutoSave's permission to write to an earlier
file is cleared on restore.

Continue without restoring leaves copies available. Discard asks for confirmation and deletes
the selected copy, including its retained generation. It does not delete the editing document.
After discarding that document's copy, another copy is made only after a further edit.
A successful save removes that document's recovery copy on the next worker cycle while it
remains open; if the document is replaced first (Save, then New or Open straight away), its copy
is removed as soon as the worker is free, and closing the app with a saved document removes it
before exit. Don't Save removes the abandoned document's copy at once. An unsaved document
replaced without a choice (an agent's `file.open`) keeps its copy, offered at the next start.
Copies left from earlier sessions can still be explicitly discarded.

Documents with a password do not get plaintext recovery copies. Applying a password waits for
any pending snapshot, removes the current document's recovery generations, and fails if cleanup
fails. Encrypted recovery is not implemented. Documents opened with a password keep recovery
disabled for that session. Removing the password allows future recovery again.

## Storage and timing

Copies live in `recovery/` under the app's profile folder. `WORDCRAFT_PROFILE_DIR` selects a
separate profile for isolated runs, including preferences, logs and control keys.
`WORDCRAFT_NO_PREFS` disables recovery as well as preferences. Headless engine/CLI sessions
and the browser do not create recovery files implicitly.

The desktop host checks for a dirty document every frame but snapshots at most once per ten
seconds, and only when its revision changes. After a snapshot that took `t`, the next waits at
least 20·`t` (at most five minutes), so recovery stays under about 5% of a core on large
documents. A snapshot that fails in a way only a change can clear (over a size limit or budget,
or holding a value the format can't keep) keeps its revision: it is tried again only after the
next edit, still paced, instead of at every interval. Other failures (a record locked for a
moment, an I/O error) are retried at the next interval. It clones the shared document model for a bounded worker queue. Encoding, file I/O,
full-document validation and discovery run on the worker.
An unchanged document does not schedule repeated recovery repaints. The most recent edits can
be lost if a crash happens before the next snapshot or while its write is still pending. When
the session ends normally, the worker finishes the snapshot under way and releases its locks
first.

Snapshots wrap the native JSON document with an id, document name, timestamp and monotonic
generation. Native JSON includes media bytes, opaque preserved parts, and chart/SmartArt/OLE
source markup and relationships. Version 2 snapshots keep each picture and preserved part once
per record, as a file `parts/<sha256>` in the record folder, and the JSON names it by hash and
length (`"parts": {"media": {"media/image1.png": {"part": "<sha256>", "len": N}}}`). A part file
is written the first time a snapshot needs it (temporary file, sync, rename) and shared by later
generations, so typing into a document with large pictures writes only its text and structure:
with 300 MB of pictures, a text edit writes a 27 KB snapshot. Version 1 snapshots, with parts
inside the JSON as base64 or (older still) arrays of numbers, still restore; the next snapshot
of a restored document moves its parts to files. In `.json` files, parts stay inline as base64.
Paragraph cache revisions are transient and regenerated on load; undo history and selection are
not part of the document snapshot. Recovery preserves the current in-memory model. It cannot
recover content already lost during import or make later export more faithful than the exporter.

Each generation is written exclusively to a temporary file, flushed and synchronized, decoded
and compared with the source document, checked to name only part files present at their
lengths, then renamed to a new generation. The last valid snapshot, and the part files it uses,
are retained until its successor is valid; part files neither uses are then removed. Validation
doesn't read the pictures back: each part's SHA-256 is checked when the copy is restored. A part
file that is missing, truncated or altered leaves out that picture or part, never the text; the
restore names what it left out. Discovery falls back to an older valid copy
if the newest is corrupt and reports unreadable files. A record that can't be read at all (say,
with more generations than discovery reads) is skipped and reported; the others are listed.
Abandoned temporary files are cleaned only after acquiring that record's lock. Native file locks
prevent other running instances from restoring or deleting a live record. Unix folders/files
use modes 0700/0600, and an existing recovery or record folder that others could read is made
0700 before a copy is written into it; Windows uses the profile's inherited permissions.
Directory synchronization runs on Unix; Windows synchronizes the file but does not yet
explicitly synchronize directory metadata. Power-loss durability on Windows is therefore
unverified.

Limits are 256 MiB of JSON per generation (the text and structure, without parts), 1 GiB per
record (its generations and part files), 2 GiB for the whole recovery folder, 128 records, 16
generations and 65,536 part files per record during discovery. Ordinarily only two valid
generations remain. Copies are retained until saved or explicitly discarded; there is no
automatic age-based deletion. A snapshot is planned against the budgets before anything is
written. The text and structure are always kept when they fit; pictures and parts are added
smallest first while they fit, and the ones left out are named in the copy (`omitted`). While
the current document's copy leaves anything out, or no copy could be written, the status bar
shows "Not protected by recovery" (its tooltip names the parts or the error; a click opens File ›
Recover) until a later copy covers everything or the document is saved. A generation whose text
alone doesn't fit reports an error and retains the previous valid copy. Part files hold the
bytes as they are, so a copy is about the size of the document's pictures plus its text, which
can still be larger than a DOCX that compresses. There is no cloud, version-history or browser
recovery.

## Commands and verification

`file.recover={}` reads cached `{enabled,busy,documents,error,gap}`. Each document has `id`,
`name` and `savedAt`, and `omitted` when its copy left out pictures or parts. `gap` is what the
current document's copy doesn't cover: `{"omitted":[names]}`, `{"failed":"error"}` or `null`. `{"refresh":true}`, `{"restore":"id"}` and `{"discard":"id"}` queue worker
operations. Poll until `busy` is false; inspect `error` for failure. A restore is cancelled if the
document id or revision changes while it loads. Programmatic restores do not open dialogs;
user restores over dirty work use the existing Save Changes prompt.
`ui.recoveryDiscard={"id":"id"}` opens confirmation; `{"answer":"discard"}` or
`{"answer":"cancel"}` answers it.

Focused tests are `cargo test -p wordcraft-engine recovery`,
`cargo test -p wordcraft-engine --test recovery_format`, and
`cargo test -p wordcraft-ui-egui recovery`. They cover lifecycle, preservation, original-file
protection, failed writes, corruption, locks, password cleanup and the actual recovery buttons.
The contribution also uses a native SIGKILL/restart replay in temporary profiles for both new
drafts and edited existing files. It uses synthetic inputs and hashes the original before and
after. No Microsoft Word operation is needed for this reliability test.

## Maintenance validation

The 2026-10-12 rebase retains main’s editable graphic models, native font setup, current
options module, fixed-width Backstage and filename-aware Save As. The SIGKILL/restart
observations above belong to the original contribution, not a rerun on this branch. Current
validation executes the focused lifecycle and preservation tests in temporary profiles and
all repository CI gates; no daily-driver recovery folder or native app is operated.

The desktop host chooses the private recovery profile. Recovery commands accept known
snapshot IDs, never file paths or a profile override. This internal store is separate from
the automation roots that constrain opening and saving user documents.
