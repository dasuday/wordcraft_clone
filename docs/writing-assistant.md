# Writing Assistant

> **Last reviewed:** 2026-10-16 · **Last updated:** 2026-10-16 · **Change:** minor (saved model profiles, default and session override)

## Use

Open **Home › Assistant › Writing Assistant**, or search for **Writing Assistant** in the
command search. Place the caret for a draft, or select the text to rewrite or summarize.

1. Go to **File › Options › Writing Assistant**, or click **Model connection** in the
   assistant to open Writing Assistant options. Use **Add** to save another connection. Give each
   profile a name, full chat completions endpoint, model name and optional API key. Select a profile
   in the list to edit it; select **Default** to make it the default. **Remove** deletes that profile.
   Up to 32 profiles can be saved, including multiple models at the same endpoint. All values use WordCraft's
   normal preferences: saved in local `ui.json` on normal exit and restored on the next launch.
   The key is visible in Options and saved in plain text, as requested. It is omitted from
   diagnostic inspection and debug output and never stored in documents. Clear the key field
   to remove it from the next saved preferences. Runs with `WORDCRAFT_NO_PREFS` do not retain settings.
   Existing single-connection settings automatically become the first default profile.
2. Choose **Draft**, **Rewrite**, or **Summarize** and enter instructions. Draft requires
   instructions; Rewrite and Summarize require selected text. For example: “Make this more
   concise while keeping the technical details” or “Summarize in three sentences.”
3. The assistant starts with **Model — Default** collapsed. Expand it to choose a saved model
   for this app session, or choose **Default** to follow the default set in Options. This override
   does not change the saved default and resets on restart. Selection is disabled during generation;
   an in-flight request keeps its original endpoint, model and key. Removing the default selects
   the first remaining profile; removing the session override returns it to Default.
   Click **Generate**. Only the instructions and selected plain text go to the chosen
   endpoint. The preview fills as text arrives and follows new text while you stay at the bottom.
   No surrounding text, files, images, history, or tool access is supplied.
4. Once generation finishes successfully, review and optionally edit the preview. **Insert after selection** preserves the source and
   adds the result as a new paragraph (at a bare caret it inserts directly). **Replace selection**
   replaces it. Both create one Undo step and respect document protection and Track Changes.
   **Copy** copies the preview without editing. **Discard** clears it.

Use your external **OpenAI-compatible serving endpoint**, for example
`https://models.example.com/v1/chat/completions`, and the model identifier advertised by that
server. The URL starts empty: WordCraft chooses no provider for you, installs no model, and
runs no local inference. The server should accept chat completions with `model`,
`messages`, and `stream: true`, returning server-sent events with `choices[0].delta.content`,
`finish_reason: "stop"` and a final `[DONE]` event. See the
[streaming format](https://developers.openai.com/api/reference/resources/chat/subresources/completions/streaming-events).
Servers that respond with ordinary JSON still work through `choices[0].message.content`. If supplied,
the API key is sent as a Bearer token. Hosted endpoints require HTTPS; HTTP is allowed only
for loopback testing. Redirects are refused. Provider usage is billed by your provider.

If you edit the document, open another document, or change the selection, the old result cannot
be accepted. Restore an unchanged selection or generate again. You can still copy the preview.
Closing the assistant hides the window; Discard ignores an in-flight response, but cannot
recall text already sent to a server. Only one request runs at a time, with a 90-second timeout.

## Scope and limits

This is plain-text assistance. Inserted paragraphs inherit the insertion point's formatting;
replacement does not reconstruct mixed formatting. Text spanning a table boundary, object
markers, equations, column selections and whole-cell selections is refused. A selection can
cover ordinary paragraphs, including paragraphs within one table cell or header/footer story.
Instructions are limited to 8 KiB, source and result to 64 KiB, and result paragraphs to 4096.
The response body is capped at 1 MiB. Empty, malformed, truncated or filtered output is an error.
Provider error bodies and credentials are not displayed in errors.

The desktop app performs network generation on a worker thread. Web builds retain the portable
engine commands but disable generation; browser transport, encrypted credential storage and
conversation history are follow-up work. While streaming, the preview is
read-only and acceptance is disabled. Interrupted or filtered streams retain their partial preview
for copying, but cannot be inserted or used to replace a selection.

## Design and implementation plan

1. Add provider-independent `assistant.prepare` and `assistant.apply` engine commands, with
   bounded plain-text extraction, document/selection freshness checks, and normal editing guards.
2. Add a desktop transport for compatible chat completions servers, bounded responses, timeouts,
   explicit errors and discard handling. Keep network dependencies outside the portable engine.
3. Add a discoverable assistant window with three modes, saved connection settings in Options, an editable preview,
   and explicit acceptance. Route generation, discard and acceptance through UI commands.
4. Verify engine semantics, provider handling, UI lifecycle, hostile input and the headless UI;
   run repository quality gates and document any environment or baseline blockers.

## Relationship to PR #178

[PR #178](https://github.com/storytold/wordcraft/pull/178) was reviewed as an architectural
reference: its chat model, engine commands, command gate and usage documentation. It provides
invitations, membership, agent conversations, isolated selections and author-attributed tracked
edits for external agents. It does not supply an in-process model provider or this three-mode
preview workflow. This change does not import or merge the PR, open its control port, reuse its
membership keys, or change its agent allow-list. The `assistant.*` namespace can coexist with
`chat.*`; future integration should keep model text separate from executable agent commands.
This was a scoped overlap review, not a full security or merge-readiness review of that PR.

## Commands

`assistant.open` requests the window. `assistant.prepare` accepts
`{"mode":"rewrite","prompt":"Make it concise"}` and returns a request object with messages,
source text, selection, document identity and revision. It performs no network call and does
not change the document. External hosts can send its `messages` to their chosen model.

`assistant.apply` takes `{"request": <returned object>, "text":"Reviewed result",
"placement":"replace"}` (or `"after"`). It checks the document revision and selection, then
applies one undoable edit. These commands are available through the registry, CLI and MCP.
The request object is a freshness guard, not an authentication credential.

The desktop controls dispatch `ui.assistant.generate`, `ui.assistant.discard`, and
`ui.assistant.apply {"placement":"replace"|"after"}`. They use the connection settings and
preview of the current window. Network credentials never enter a recorded engine command.

## Verification

Verified on Windows ARM64 with an HTTP mock provider; no real hosted model or account was used.
The 622 engine tests (including hostile command parameters and seven new assistant tests) pass.
Ten assistant UI/transport tests cover response parsing, endpoint validation, late-result
discard, preview acceptance/Undo, fragmented UTF-8, limits, incomplete streams and the HTTP contract.
A mock streaming server waits for the client to receive its first chunk before sending the
remainder, proving incremental delivery. The interface was
rendered with `ui_shot` and visually inspected. Command names are catalogued in every supported
language; complete catalogs also include the assistant's controls.
All 72 localization tests pass after those additions.

Formatting, workspace Clippy with warnings denied, asset attribution, dependency layering and
all 16 required WASM crate checks pass. `cargo xtask ci` was run twice but is not green: existing
colour-emoji renderer tests fail on this machine (also reproduced with the renderer tested in
isolation), and a later run failed the PDF right-to-left glyph-position test. No renderer, font,
layout or PDF code was changed. These failures remain outside this feature's scope.

Streaming follow-up: all ten assistant tests pass, and the in-progress preview was rendered
headlessly and visually checked. Formatting, workspace Clippy, assets, layers and all 16 WASM
checks pass. Full CI stopped in the unchanged PDF test `right_to_left_text_extracts_in_logical_order`
on one run and the existing renderer colour-emoji tests on the final run; the streaming change
only modifies the UI crate and documentation.
The complete UI library suite passes: 561 tests passed, two ignored.

Saved-options follow-up: the full UI suite passes (563 tests, two ignored), including twelve
assistant tests. An additional navigation test passes for the dedicated Writing Assistant pane.
Preference tests cover restart restoration, clearing the key, old/malformed settings and
diagnostic redaction; both HTTP mock tests generate using restored preferences. The new pane
and its assistant shortcut were rendered and visually checked. Formatting, Clippy, assets,
layers and WASM checks pass; full CI still encounters the existing PDF right-to-left tests.

Profile follow-up: 567 UI tests passed (two ignored), and all 15 assistant tests pass after the
final UI adjustments. Coverage includes default/override routing to an HTTP mock, migration,
multiple keys, malformed settings, profile limits, deletion and collapsed UI controls. Both the
profile editor and expanded selector were rendered with synthetic profiles and visually checked.
Formatting, workspace Clippy, assets, layers and all 16 WASM checks pass. Full CI stops at the
existing renderer colour-emoji tests on this machine.

Network generation uses native TLS and the platform certificate store. Linux/BSD builds require
the native TLS backend's system OpenSSL build/runtime dependencies; cross-platform packaging
and a live connection to your particular hosted model have not been verified here.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-16 | minor | Multiple saved server/model profiles, default selection, collapsed session override and single-connection migration |
| 2026-10-16 | minor | Connection settings moved to Writing Assistant options and retained through the normal preferences lifecycle; plain-text key storage |
| 2026-10-16 | minor | Live streaming previews, completion guards and incremental transport tests |
| 2026-10-16 | minor | Initial implementation and scoped comparison with PR #178 |
