//! Side panes: Navigation (headings, pages, search results), Clipboard, Styles, Style Inspector,
//! Accessibility and Alt Text (see `a11y_pane`), Restrict Editing ([`crate::dialogs_review`]), Comments,
//! and the SmartArt Text Pane.

use std::collections::BTreeMap;

use egui::{Stroke, Ui, vec2};
use serde_json::{Value, json};
use wordcraft_doc::{Pos, StoryRef};

use crate::WordApp;
use crate::theme::{Tokens, regular, semibold};

pub fn show(app: &mut WordApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    // SmartArt Design › Text Pane, while a SmartArt graphic WordCraft can edit is selected.
    if app.session.view.smart_art_pane && wordcraft_engine::cmd::smart_art::selected_smart_art(&app.session).is_some() {
        egui::Panel::left("smart_art_pane")
            .default_size(250.0)
            .resizable(true)
            .frame(egui::Frame::NONE.fill(t.panel).inner_margin(10).stroke(Stroke::new(1.0, t.border)))
            .show(ui, |ui| {
                if header(ui, "Text Pane") {
                    let _ = app.run("smartArt.textPane", json!({"value": false}));
                    return;
                }
                crate::dialogs_smart_art::text_pane(app, ui);
            });
    }
    if app.session.view.nav_pane {
        egui::Panel::left("nav_pane")
            .default_size(260.0)
            .resizable(true)
            .frame(egui::Frame::NONE.fill(t.panel).inner_margin(10).stroke(Stroke::new(1.0, t.border)))
            .show(ui, |ui| nav(app, ui));
    }
    if app.session.view.clipboard_pane {
        egui::Panel::left("clipboard_pane")
            .default_size(250.0)
            .resizable(true)
            .frame(egui::Frame::NONE.fill(t.panel).inner_margin(10).stroke(Stroke::new(1.0, t.border)))
            .show(ui, |ui| clipboard(app, ui));
    }
    if app.session.view.styles_pane {
        egui::Panel::right("styles_pane")
            .default_size(250.0)
            .resizable(true)
            .frame(egui::Frame::NONE.fill(t.panel).inner_margin(10).stroke(Stroke::new(1.0, t.border)))
            .show(ui, |ui| styles(app, ui));
    }
    if app.session.view.style_inspector {
        egui::Panel::right("style_inspector")
            .default_size(240.0)
            .resizable(true)
            .frame(egui::Frame::NONE.fill(t.panel).inner_margin(10).stroke(Stroke::new(1.0, t.border)))
            .show(ui, |ui| inspector(app, ui));
    }
    if app.session.view.accessibility_pane {
        egui::Panel::right("accessibility_pane")
            .default_size(280.0)
            .resizable(true)
            .frame(egui::Frame::NONE.fill(t.panel).inner_margin(10).stroke(Stroke::new(1.0, t.border)))
            .show(ui, |ui| crate::a11y_pane::accessibility(app, ui));
    }
    if app.session.view.alt_text_pane {
        egui::Panel::right("alt_text_pane")
            .default_size(260.0)
            .resizable(true)
            .frame(egui::Frame::NONE.fill(t.panel).inner_margin(10).stroke(Stroke::new(1.0, t.border)))
            .show(ui, |ui| crate::a11y_pane::alt_text(app, ui));
    }
    if app.restrict.open {
        egui::Panel::right("restrict_pane")
            .default_size(270.0)
            .resizable(true)
            .frame(egui::Frame::NONE.fill(t.panel).inner_margin(10).stroke(Stroke::new(1.0, t.border)))
            .show(ui, |ui| {
                if header(ui, "Restrict Editing") {
                    app.restrict.open = false;
                    return;
                }
                crate::dialogs_review::restrict_pane(app, ui);
            });
    }
    if app.session.view.comments_pane {
        egui::Panel::right("comments_pane")
            .default_size(290.0)
            .resizable(true)
            .frame(egui::Frame::NONE.fill(t.panel).inner_margin(10).stroke(Stroke::new(1.0, t.border)))
            .show(ui, |ui| comments(app, ui));
    }
}

pub(crate) fn header(ui: &mut Ui, title: &str) -> bool {
    let mut close = false;
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(tl!(title)).font(semibold(15.0)));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // Drawn, not the "✕" character: the interface fonts have no glyph for it.
            if crate::widgets::icon_button(ui, "close", tl!("Close")).clicked() {
                close = true;
            }
        });
    });
    ui.add_space(4.0);
    close
}

fn nav(app: &mut WordApp, ui: &mut Ui) {
    if header(ui, "Navigation") {
        let _ = app.run("view.navigationPane", json!({"value": false}));
        return;
    }
    let qid = egui::Id::new("nav_query");
    let mut q = ui.data(|d| d.get_temp::<String>(qid)).unwrap_or_default();
    // Ctrl+F with text selected searches for it, as Word does (a selection within one line).
    let focus = std::mem::take(&mut app.nav_search_focus);
    let picked = focus.then(|| app.session.selected_text()).filter(|t| !t.is_empty() && !t.contains(['\n', '\r']) && t.chars().count() <= 255);
    if let Some(t) = &picked {
        q = t.clone();
    }
    let mut out = egui::TextEdit::singleline(&mut q).hint_text(tl!("Search document")).desired_width(f32::INFINITY).show(ui);
    let r = out.response.clone();
    if focus {
        // Focused with its text selected, so typing replaces it.
        r.request_focus();
        let all = egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(q.chars().count()));
        out.state.cursor.set_char_range(Some(all));
        out.state.store(ui.ctx(), r.id);
    }
    if r.changed() || picked.is_some() {
        app.ui.nav_tab = "results".into();
        ui.data_mut(|d| d.insert_temp(qid, q.clone()));
        if !q.is_empty() {
            let _ = app.session.run("edit.find", &json!({"text": q}));
        } else {
            app.session.find.results.clear();
        }
    }
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        for (k, l) in [("headings", "Headings"), ("pages", "Pages"), ("results", "Results")] {
            if ui.selectable_label(app.ui.nav_tab == k, tl!(l)).clicked() {
                app.ui.nav_tab = k.into();
            }
        }
    });
    ui.separator();
    egui::ScrollArea::vertical().show(ui, |ui| match app.ui.nav_tab.as_str() {
        "pages" => {
            let l = app.session.layout();
            for (i, p) in l.pages.iter().enumerate() {
                let label = crate::i18n::fmt(tl!("Page {page}"), &[("page", &p.number.to_string())]);
                if ui.selectable_label(app.session.page_hint == i, label).clicked() {
                    let _ = app.run("edit.goto", json!({"page": i + 1}));
                }
            }
        }
        "results" => {
            let results = app.session.find.results.clone();
            ui.label(egui::RichText::new(format!("{} results", results.len())).small().weak());
            for (a, b) in results.iter().take(500) {
                let Some(p) = app.session.doc.para_at(a) else { continue };
                let from = p.clamp(a.off.saturating_sub(30));
                let to = p.clamp((b.off + 40).min(p.len()));
                let pre = p.text.get(from..a.off).unwrap_or("");
                let hit = p.text.get(a.off..b.off).unwrap_or("");
                let post = p.text.get(b.off..to).unwrap_or("");
                let mut job = egui::text::LayoutJob::default();
                let fmt = |c| egui::TextFormat { font_id: regular(12.0), color: c, ..Default::default() };
                job.append(pre, 0.0, fmt(ui.visuals().text_color()));
                // The match is marked the way Find marks it on the page, per theme (#656).
                let mark = Tokens::get(ui.ctx()).find_highlight;
                job.append(hit, 0.0, egui::TextFormat { font_id: semibold(12.0), background: mark, color: ui.visuals().text_color(), ..Default::default() });
                job.append(post, 0.0, fmt(ui.visuals().text_color()));
                job.wrap.max_width = ui.available_width();
                if ui.add(egui::Button::new(job).frame(false)).clicked() {
                    let _ = app.run("select.range", json!({"anchor": a, "focus": b}));
                }
                ui.separator();
            }
        }
        _ => {
            let caret = app.session.sel.focus.clone();
            let mut current: Option<usize> = None;
            let mut items = Vec::new();
            for path in app.session.doc.para_paths(StoryRef::Body) {
                let Some(p) = app.session.doc.para(StoryRef::Body, &path) else { continue };
                let rp = app.session.doc.styles.resolve_para(&p.props);
                if let Some(lv) = rp.outline_level {
                    let txt = p.plain_text().trim().to_string();
                    if txt.is_empty() || rp.style.starts_with("TOC") {
                        continue;
                    }
                    if path <= caret.path {
                        current = Some(items.len());
                    }
                    items.push((path, lv, txt));
                }
            }
            if items.is_empty() {
                ui.label(egui::RichText::new(tl!("Create an interactive outline of your document.\n\nIt's a great way to keep track of where you are or quickly move your content around.\n\nTo get started, go to the Home tab and apply Heading styles to the headings in your document.")).weak());
            }
            for (i, (path, lv, txt)) in items.into_iter().enumerate() {
                if nav_heading(ui, lv, &txt, current == Some(i)).clicked() {
                    let _ = app.run("caret.set", json!({"pos": Pos { story: StoryRef::Body, path, off: 0 }}));
                    // Align the chosen heading's page immediately against this pane, rather
                    // than the indeterminate minimum movement from scroll_to_rect.
                    app.canvas.nav_to_caret = true;
                    app.canvas.scroll_to_caret = false;
                    app.canvas.want_focus = true;
                }
            }
        }
    });
}

/// Constrain each heading to the available pane width so long document text cannot
/// enlarge the panel. The full heading stays accessible on hover.
fn nav_heading(ui: &mut Ui, level: u8, text: &str, selected: bool) -> egui::Response {
    ui.horizontal(|ui| {
        ui.add_space(level as f32 * 14.0);
        let label = egui::RichText::new(text).font(if level == 0 { semibold(12.5) } else { regular(12.5) });
        ui.add_sized(
            vec2(ui.available_width(), ui.spacing().interact_size.y),
            // The trailing grow atom takes the spare width, so the text starts flush left
            // under its parent instead of centred, as in Word (#1199).
            egui::Button::selectable(selected, (label, egui::Atom::grow())).truncate(),
        )
        .on_hover_text(text)
    })
    .inner
}

/// Items collected by Copy and Cut, newest first; clicking one pastes it at the caret.
fn clipboard(app: &mut WordApp, ui: &mut Ui) {
    if header(ui, "Clipboard") {
        let _ = app.run("edit.clipboardPane", json!({"value": false}));
        return;
    }
    let items: Vec<String> = app.session.clip_history.items().iter().map(|i| i.preview()).collect();
    ui.horizontal(|ui| {
        if ui.add_enabled(!items.is_empty(), egui::Button::new(tl!("Paste All"))).clicked() {
            let _ = app.run("edit.pasteAllClipboard", json!({}));
            app.canvas.want_focus = true;
        }
        if ui.add_enabled(!items.is_empty(), egui::Button::new(tl!("Clear All"))).clicked() {
            let _ = app.run("edit.clearClipboard", json!({}));
            app.canvas.want_focus = true;
        }
    });
    ui.add_space(4.0);
    let hint = if items.is_empty() { "Nothing collected yet. Items you copy or cut appear here." } else { "Click an item to paste it." };
    ui.label(egui::RichText::new(tl!(hint)).small().weak());
    ui.separator();
    let t = Tokens::get(ui.ctx());
    egui::ScrollArea::vertical().show(ui, |ui| {
        for (i, preview) in items.iter().enumerate() {
            egui::Frame::NONE.fill(t.input).stroke(Stroke::new(1.0, t.border)).corner_radius(6).inner_margin(8).show(ui, |ui| {
                ui.set_width(ui.available_width());
                let text = if preview.is_empty() { tl!("(picture or object)").to_string() } else { preview.clone() };
                let mut job =
                    egui::text::LayoutJob::single_section(text, egui::TextFormat { font_id: regular(12.5), color: t.text, ..Default::default() });
                job.wrap.max_width = ui.available_width();
                job.wrap.max_rows = 3;
                if ui.add(egui::Button::new(job).frame(false)).on_hover_text(tl!("Paste")).clicked() {
                    let _ = app.run("edit.pasteClipboardItem", json!({"index": i}));
                    app.canvas.want_focus = true;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    if ui.small_button(tl!("Delete")).clicked() {
                        let _ = app.run("edit.deleteClipboardItem", json!({"index": i}));
                        app.canvas.want_focus = true;
                    }
                });
            });
            ui.add_space(6.0);
        }
    });
}

fn styles(app: &mut WordApp, ui: &mut Ui) {
    if header(ui, "Styles") {
        let _ = app.run("view.stylesPane", json!({"value": false}));
        return;
    }
    let st = app.session.run("format.state", &json!({})).unwrap_or_default();
    let cur = st.get("style").and_then(Value::as_str).unwrap_or("Normal").to_string();
    let current_name = app
        .session
        .doc
        .styles
        .get(&cur)
        .map(crate::i18n::style_name)
        .unwrap_or_else(|| st.get("styleName").and_then(Value::as_str).unwrap_or("").to_string());
    ui.label(egui::RichText::new(crate::i18n::fmt(tl!("Current style: {style}"), &[("style", &current_name)])).small());
    ui.horizontal(|ui| {
        if ui.button(tl!("New Style…")).clicked() {
            app.dialog = crate::dialogs::Dialog::open("newStyle", app);
        }
        if ui.button(tl!("Update to Match")).clicked() {
            let _ = app.run("styles.updateToMatch", json!({}));
        }
        if ui.button(tl!("Clear")).clicked() {
            let _ = app.run("format.clear", json!({}));
        }
    });
    if ui.button(tl!("Manage Styles…")).clicked() {
        let _ = app.run("styles.manage", json!({}));
    }
    if ui.selectable_label(app.session.view.style_inspector, tl!("Style Inspector")).clicked() {
        let _ = app.run("styles.inspector", json!({}));
    }
    ui.separator();
    let mut list: Vec<(String, String, bool)> = app
        .session
        .doc
        .styles
        .styles
        .iter()
        .filter(|s| !s.hidden && s.kind != wordcraft_doc::StyleKind::Table)
        .map(|s| (s.id.clone(), crate::i18n::style_name(s), s.kind == wordcraft_doc::StyleKind::Character))
        .collect();
    list.sort_by_key(|s| s.1.to_lowercase());
    egui::ScrollArea::vertical().show(ui, |ui| {
        for (id, name, is_char) in list {
            ui.horizontal(|ui| {
                let r = ui.selectable_label(id == cur, &name);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new(if is_char { "a" } else { "¶" }).weak());
                });
                if r.clicked() {
                    let _ = app.run("para.style", json!({"style": id}));
                    app.canvas.want_focus = true;
                }
                r.context_menu(|ui| {
                    if ui.button(tl!("Modify…")).clicked() {
                        app.dialog = crate::dialogs::Dialog::modify_style(app, &id);
                        ui.close();
                    }
                    if ui.button(tl!("Update to Match Selection")).clicked() {
                        let _ = app.run("styles.updateToMatch", json!({"style": id}));
                        app.canvas.want_focus = true;
                        ui.close();
                    }
                    if ui.button(tl!("Delete")).clicked() {
                        let _ = app.run("styles.delete", json!({"style": id}));
                        app.canvas.want_focus = true;
                        ui.close();
                    }
                });
            });
        }
    });
}

fn comments(app: &mut WordApp, ui: &mut Ui) {
    if header(ui, "Comments") {
        let _ = app.run("view.commentsPane", json!({"value": false}));
        return;
    }
    if crate::widgets::icon_text_button(ui, "newComment", tl!("New Comment"), regular(12.5)).clicked() {
        let _ = app.run("review.newComment", json!({}));
    }
    ui.separator();
    let list = app.session.run("review.comments", &json!({})).unwrap_or_default();
    let t = Tokens::get(ui.ctx());
    let card = egui::Frame::NONE.fill(t.input).stroke(Stroke::new(1.0, t.border)).corner_radius(6).inner_margin(8);
    egui::ScrollArea::vertical().show(ui, |ui| {
        // The comment being written comes first, until it is posted or cancelled.
        if writing(app) {
            card.show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(egui::RichText::new(crate::canvas::author_name(&app.session.author)).font(semibold(12.0)));
                compose_field(app, ui, None, None);
            });
            ui.add_space(6.0);
        }
        for c in list.as_array().cloned().unwrap_or_default() {
            let Some(id) = c.get("id").and_then(Value::as_u64).and_then(|i| u32::try_from(i).ok()) else { continue };
            let Some(comment) = app.session.doc.comments.get(&id).cloned() else { continue };
            card.show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(crate::canvas::author_name(&comment.author)).font(semibold(12.0)));
                    ui.label(egui::RichText::new(crate::local_time::comment_time(&comment)).small().weak());
                    if comment.resolved {
                        ui.label(egui::RichText::new(tl!("Resolved")).small().color(t.green));
                    }
                });
                // Posted: the text reads as written, and only its author can edit it.
                comment_body(app, ui, id, None, None);
                ui.horizontal_wrapped(|ui| {
                    if ui.small_button(tl!("Go to")).clicked()
                        && let Some(a) = c.get("anchor").filter(|a| !a.is_null())
                    {
                        let _ = app.run("caret.set", json!({"pos": a}));
                    }
                    if !replying(app, id)
                        && ui.add_enabled(crate::widgets::enabled(app, "review.reply"), egui::Button::new(tl!("Reply")).small()).clicked()
                    {
                        begin_reply(app, id);
                    }
                    if can_edit(app, id) && !editing(app, id) && ui.small_button(tl!("Edit")).clicked() {
                        begin_edit(app, id);
                    }
                    if ui.small_button(if comment.resolved { tl!("Reopen") } else { tl!("Resolve") }).clicked() {
                        let _ = app.run("review.resolveComment", json!({"id": id}));
                    }
                    if ui.small_button(tl!("Delete")).clicked() {
                        let _ = app.run("review.deleteComment", json!({"id": id}));
                    }
                });
                reply_field(app, ui, id, None, None);
            });
            ui.add_space(6.0);
        }
    });
}

/// Comment text being written and not yet in the document: a new comment, edits of posted
/// comments and replies. Nothing reaches the document until it is posted (or saved, for an edit),
/// and leaving a field keeps what was typed in it. Cancel or Escape throws it away. Saving,
/// exporting, printing and closing write what is pending first ([`flush_comment_drafts`], #1348).
/// Drafts belong to one document ([`Session::document_id`](wordcraft_engine::Session::document_id)):
/// when another document replaces it they are dropped, so a comment there with the same id never
/// shows them. A draft is removed only once its command succeeds, so a refused one stays.
#[derive(Default)]
pub(crate) struct CommentDrafts {
    document: u64,
    /// The new comment being written (New Comment opened its field).
    new: Option<NewComment>,
    /// Posted comments whose Edit is open, with their edited text.
    edits: BTreeMap<u32, String>,
    /// Comments with an open reply field, with what has been typed in it.
    replies: BTreeMap<u32, String>,
    /// The field that gets the keyboard on its next frame.
    focus: Option<Field>,
}

impl CommentDrafts {
    /// The drafts for `document`, dropping any left from another one.
    fn of(&mut self, document: u64) -> &mut Self {
        if self.document != document {
            *self = CommentDrafts { document, ..Default::default() };
        }
        self
    }
}

/// A comment field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Field {
    New,
    Edit(u32),
    Reply(u32),
}

/// A new comment being written: its place and its text.
pub(crate) struct NewComment {
    /// Where it goes: the selection when New Comment was chosen, else the word at the caret.
    anchor: (Pos, Pos),
    /// The document revision the place was last checked at, and the text it covered then (None
    /// when it spans paragraphs). See [`refresh_anchor`].
    rev: u64,
    quoted: Option<String>,
    text: String,
}

/// The drafts of the open document, when there are any.
fn current(app: &WordApp) -> Option<&CommentDrafts> {
    let d = &app.comment_drafts;
    (d.document == app.session.document_id()).then_some(d)
}

/// Whether the open document has a comment or reply draft not yet written to it (typed, or
/// kept after the document refused it): closing the page would lose it.
pub(crate) fn has_comment_drafts(app: &WordApp) -> bool {
    let d = &app.comment_drafts;
    d.document == app.session.document_id() && !(d.edits.is_empty() && d.replies.is_empty())
}

fn drafts(app: &mut WordApp) -> &mut CommentDrafts {
    let document = app.session.document_id();
    app.comment_drafts.of(document)
}

/// A new comment is being written in the open document.
pub(crate) fn writing(app: &WordApp) -> bool {
    current(app).is_some_and(|d| d.new.is_some())
}

/// The new comment's field is in its balloon: balloons show comments. Else it is in the
/// Comments pane.
pub(crate) fn writing_in_balloon(app: &WordApp) -> bool {
    writing(app) && app.session.balloons_shown() && app.session.prefs.markup.comments
}

/// Comment `id`'s Edit is open.
pub(crate) fn editing(app: &WordApp, id: u32) -> bool {
    current(app).is_some_and(|d| d.edits.contains_key(&id))
}

/// Comment `id` has an open reply field.
pub(crate) fn replying(app: &WordApp, id: u32) -> bool {
    current(app).is_some_and(|d| d.replies.contains_key(&id))
}

/// The pending edit of comment `id`'s text in the open document, if any.
#[cfg(test)]
pub(crate) fn comment_draft(app: &WordApp, id: u32) -> Option<String> {
    current(app).and_then(|d| d.edits.get(&id).cloned())
}

/// What has been typed in comment `id`'s reply field, while it is open.
#[cfg(test)]
pub(crate) fn reply_draft(app: &WordApp, id: u32) -> Option<String> {
    current(app).and_then(|d| d.replies.get(&id).cloned())
}

/// Give comment field `which` the keyboard on the next frame, as a click in it does.
#[cfg(test)]
pub(crate) fn focus_field(app: &mut WordApp, which: Field) {
    drafts(app).focus = Some(which);
}

/// What has been typed for the new comment, while its field is open.
#[cfg(test)]
pub(crate) fn new_comment_text(app: &WordApp) -> Option<String> {
    current(app).and_then(|d| d.new.as_ref()).map(|n| n.text.clone())
}

/// The new comment being written, for `ui.inspect`: its text and where its field is.
pub(crate) fn writing_state(app: &WordApp) -> Value {
    match current(app).and_then(|d| d.new.as_ref()) {
        Some(n) => json!({"text": n.text, "in": if writing_in_balloon(app) { "balloon" } else { "pane" }}),
        None => Value::Null,
    }
}

/// New Comment for the user: a field for the comment opens where it will show and takes the
/// keyboard, and nothing goes into the document until Post. The field is in the comment's balloon
/// when balloons show comments, else in the Comments pane, which opens for it where there are no
/// balloons (Draft and Web Layout, Read Mode, markup shown inline), as Word's Reviewing Pane does.
/// A comment already being written keeps its place and gets the keyboard back. `text` fills the
/// field.
pub(crate) fn start_comment(app: &mut WordApp, text: Option<String>) -> Value {
    let anchor = wordcraft_engine::cmd::review::comment_anchor(&app.session);
    let (rev, quoted) = (app.session.rev(), quoted(&app.session.doc, &anchor));
    let d = drafts(app);
    match d.new.as_mut() {
        Some(n) => {
            if let Some(t) = text {
                n.text = t;
            }
        }
        None => d.new = Some(NewComment { anchor, rev, quoted, text: text.unwrap_or_default() }),
    }
    d.focus = Some(Field::New);
    crate::canvas::deselect_balloon(app);
    let balloon = writing_in_balloon(app);
    if !balloon {
        app.session.view.comments_pane = true;
    }
    json!({"pending": "comment", "in": if balloon { "balloon" } else { "pane" }})
}

/// The text from `a` to `b` when they are in one paragraph.
fn quoted(doc: &wordcraft_doc::Document, (a, b): &(Pos, Pos)) -> Option<String> {
    if a.story != b.story || a.path != b.path {
        return None;
    }
    doc.para_at(a)?.text.get(a.off..b.off).map(str::to_string)
}

/// Keep the new comment's place as the document changes while it is written: it stays while it
/// still covers the text it was opened on, follows that text when it moved within its paragraph,
/// and else goes to the selection.
fn refresh_anchor(app: &mut WordApp) {
    let rev = app.session.rev();
    let Some(n) = current(app).and_then(|d| d.new.as_ref()).filter(|n| n.rev != rev) else { return };
    let ((a, b), was) = (n.anchor.clone(), n.quoted.clone());
    let doc = &app.session.doc;
    let fits = |p: &Pos| doc.para_at(p).is_some_and(|para| para.text.is_char_boundary(p.off));
    let anchor = if fits(&a) && fits(&b) && (was.is_none() || quoted(doc, &(a.clone(), b.clone())) == was) {
        (a, b)
    } else {
        was.as_deref()
            .filter(|q| !q.is_empty())
            .and_then(|q| {
                let text = &doc.para_at(&a)?.text;
                let at = text.match_indices(q).take(10_000).map(|(i, _)| i).min_by_key(|i| i.abs_diff(a.off))?;
                Some((Pos { off: at, ..a.clone() }, Pos { off: at + q.len(), ..a.clone() }))
            })
            .unwrap_or_else(|| wordcraft_engine::cmd::review::comment_anchor(&app.session))
    };
    let quoted = quoted(&app.session.doc, &anchor);
    if let Some(n) = drafts(app).new.as_mut() {
        n.anchor = anchor;
        n.quoted = quoted;
        n.rev = rev;
    }
}

/// Where the new comment's balloon hangs: the start of its place.
pub(crate) fn writing_at(app: &mut WordApp) -> Option<Pos> {
    refresh_anchor(app);
    current(app).and_then(|d| d.new.as_ref()).map(|n| n.anchor.0.clone())
}

/// Post the new comment: `review.newComment` with its text at its place, one undo step. The field
/// closes and the caret and selection stay where they were. A refusal is on the status line and
/// the field keeps its text.
pub(crate) fn post_comment(app: &mut WordApp) -> Result<Value, String> {
    refresh_anchor(app);
    let Some(n) = current(app).and_then(|d| d.new.as_ref()) else { return Err("no comment is being written".into()) };
    let ((a, b), text) = (n.anchor.clone(), n.text.trim_end().to_string());
    if text.trim().is_empty() {
        return Err("the comment is empty".into());
    }
    let keep = std::mem::replace(&mut app.session.sel, wordcraft_engine::Selection { anchor: a.clone(), focus: b.clone() });
    let r = app.run("review.newComment", json!({"text": text}));
    if r.is_ok() {
        // Its end mark went in at `b`, then its start mark at `a`: the selection keeps its text.
        let past = |p: &Pos| past_mark(&past_mark(p, &b, false), &a, true);
        app.session.sel = wordcraft_engine::Selection { anchor: past(&keep.anchor), focus: past(&keep.focus) };
        let d = drafts(app);
        d.new = None;
        if d.focus == Some(Field::New) {
            d.focus = None;
        }
        app.canvas.want_focus = true;
    } else {
        app.session.sel = keep;
    }
    r
}

/// `p` once a comment mark went in at `at`: a place after it moves past the mark, and one right
/// at it too when `at_too` (a start mark, which goes before the text there).
fn past_mark(p: &Pos, at: &Pos, at_too: bool) -> Pos {
    if p.story == at.story && p.path == at.path && (p.off > at.off || (at_too && p.off == at.off)) {
        Pos { off: p.off + wordcraft_doc::para::OBJ.len_utf8(), ..p.clone() }
    } else {
        p.clone()
    }
}

/// Cancel the new comment: its field closes and the document is as it was. True when one was
/// being written.
pub(crate) fn cancel_comment(app: &mut WordApp) -> bool {
    let d = drafts(app);
    let was = d.new.take().is_some();
    if d.focus == Some(Field::New) {
        d.focus = None;
    }
    if was {
        app.canvas.want_focus = true;
    }
    was
}

/// The new comment's field with Post and Cancel. Ctrl+Enter (Cmd+Enter on a Mac) posts and Escape
/// cancels. Enter starts a new line.
pub(crate) fn compose_field(app: &mut WordApp, ui: &mut Ui, font: Option<egui::FontId>, small: Option<egui::FontId>) {
    let Some(mut text) = current(app).and_then(|d| d.new.as_ref()).map(|n| n.text.clone()) else { return };
    let can = crate::widgets::enabled(app, "review.newComment");
    let r = field(app, ui, Field::New, &mut text, tl!("Add a comment"), font, can);
    if r.changed()
        && let Some(n) = drafts(app).new.as_mut()
    {
        n.text = text.clone();
    }
    let key = field_key(ui, &r);
    let (post, cancel) = field_buttons(ui, tl!("Post"), can && !text.trim().is_empty(), small);
    if post || key == Some(FieldKey::Submit) {
        let _ = post_comment(app);
    } else if cancel || key == Some(FieldKey::Cancel) {
        cancel_comment(app);
    }
}

/// A posted comment's text, read-only. While its Edit is open, it is a field with Save and
/// Cancel (Ctrl+Enter saves, Escape cancels). Saving is one `review.editComment`, so one undo step.
pub(crate) fn comment_body(app: &mut WordApp, ui: &mut Ui, id: u32, font: Option<egui::FontId>, small: Option<egui::FontId>) {
    let Some((part, resolved)) = app.session.doc.comments.get(&id).map(|c| (c.part, c.resolved)) else { return };
    let Some(mut text) = current(app).and_then(|d| d.edits.get(&id).cloned()) else {
        let t = Tokens::get(ui.ctx());
        let body = app.session.doc.plain_text(StoryRef::Part(part));
        let font = font.unwrap_or_else(|| egui::TextStyle::Body.resolve(ui.style()));
        crate::canvas::comment_text(ui, body.trim(), font, if resolved { t.text_dim } else { t.text });
        return;
    };
    let can = crate::widgets::enabled(app, "review.editComment");
    let r = field(app, ui, Field::Edit(id), &mut text, tl!("Add a comment"), font, can);
    if r.changed() {
        drafts(app).edits.insert(id, text);
    }
    let key = field_key(ui, &r);
    let (save, cancel) = field_buttons(ui, tl!("Save"), can, small);
    if save || key == Some(FieldKey::Submit) {
        // A refusal is on the status line, and the field keeps the text.
        let _ = commit_comment(app, id);
    } else if cancel || key == Some(FieldKey::Cancel) {
        drafts(app).edits.remove(&id);
    }
}

/// Edit is offered on the user's own comments (by author name, as Word tells them apart), where
/// comments can change.
pub(crate) fn can_edit(app: &WordApp, id: u32) -> bool {
    app.session.doc.comments.get(&id).is_some_and(|c| c.author == app.session.author) && crate::widgets::enabled(app, "review.editComment")
}

/// Open comment `id`'s Edit: its text in a field, with the keyboard.
pub(crate) fn begin_edit(app: &mut WordApp, id: u32) {
    let Some(part) = app.session.doc.comments.get(&id).map(|c| c.part) else { return };
    let text = app.session.doc.plain_text(StoryRef::Part(part));
    let d = drafts(app);
    d.edits.entry(id).or_insert(text);
    d.focus = Some(Field::Edit(id));
}

/// Open a reply field under comment `id`, with the keyboard.
pub(crate) fn begin_reply(app: &mut WordApp, id: u32) {
    let d = drafts(app);
    d.replies.entry(id).or_default();
    d.focus = Some(Field::Reply(id));
}

/// Comment `id`'s reply field, while it is open, with Post and Cancel: Ctrl+Enter posts and Escape
/// cancels. Leaving the field posts nothing: what was typed stays in it.
pub(crate) fn reply_field(app: &mut WordApp, ui: &mut Ui, id: u32, font: Option<egui::FontId>, small: Option<egui::FontId>) {
    let Some(mut text) = current(app).and_then(|d| d.replies.get(&id).cloned()) else { return };
    let can = crate::widgets::enabled(app, "review.reply");
    let r = field(app, ui, Field::Reply(id), &mut text, tl!("Write a reply"), font, can);
    if r.changed() {
        drafts(app).replies.insert(id, text.clone());
    }
    let key = field_key(ui, &r);
    let blank = text.trim().is_empty();
    let (post, cancel) = field_buttons(ui, tl!("Post"), can && !blank, small);
    if (post || key == Some(FieldKey::Submit)) && !blank {
        // A refusal is on the status line, and the field keeps the text.
        let _ = commit_reply(app, id);
    } else if cancel || key == Some(FieldKey::Cancel) {
        drafts(app).replies.remove(&id);
    }
}

/// A comment field: several lines, the panel's width, `hint` while empty. It takes the keyboard
/// when [`CommentDrafts::focus`] asks for it.
fn field(app: &mut WordApp, ui: &mut Ui, which: Field, text: &mut String, hint: &str, font: Option<egui::FontId>, enabled: bool) -> egui::Response {
    let (kind, id, rows) = match which {
        Field::New => ("comment_new", 0, 2),
        Field::Edit(id) => ("comment_edit", id, 1),
        Field::Reply(id) => ("comment_reply", id, 1),
    };
    let mut edit = egui::TextEdit::multiline(text)
        .id(field_id(app, ui, kind, id))
        .interactive(enabled)
        .desired_rows(rows)
        .desired_width(f32::INFINITY)
        .hint_text(hint);
    if let Some(f) = font {
        edit = edit.font(f);
    }
    let r = ui.add(edit);
    if current(app).is_some_and(|d| d.focus == Some(which)) {
        drafts(app).focus = None;
        r.request_focus();
        // The page doesn't take the keyboard back after the command that opened the field (a
        // ribbon button hands it to the page otherwise).
        app.canvas.want_focus = false;
    }
    r
}

/// A comment field's widget id, per document: egui's own state for the field (its cursor and
/// undo history) never carries over to another document's comment with the same id.
fn field_id(app: &WordApp, ui: &Ui, kind: &str, id: u32) -> egui::Id {
    ui.id().with((kind, app.session.document_id(), id))
}

/// What a comment field's keys asked for this frame.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FieldKey {
    /// Ctrl+Enter (Cmd+Enter on a Mac): post, or save an edit.
    Submit,
    /// Escape: cancel.
    Cancel,
}

fn field_key(ui: &mut Ui, r: &egui::Response) -> Option<FieldKey> {
    // Escape takes the keyboard from the field (egui's own handling) before this sees it.
    if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        return Some(FieldKey::Cancel);
    }
    // The field leaves Ctrl+Enter alone (Enter alone is a new line).
    (r.has_focus() && ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter))).then_some(FieldKey::Submit)
}

/// A comment field's buttons: `ok` (Post or Save, greyed out unless `can`) and Cancel. Returns
/// which was clicked.
fn field_buttons(ui: &mut Ui, ok: &str, can: bool, small: Option<egui::FontId>) -> (bool, bool) {
    let t = Tokens::get(ui.ctx());
    let label = |s: &str| match &small {
        Some(f) => egui::RichText::new(s).font(f.clone()),
        None => egui::RichText::new(s),
    };
    ui.horizontal(|ui| {
        let ok = ui.add_enabled(can, egui::Button::new(label(ok).color(t.on_accent)).fill(t.accent).small()).clicked();
        let cancel = ui.add(egui::Button::new(label(tl!("Cancel"))).small()).clicked();
        (ok, cancel)
    })
    .inner
}

/// Write a comment's edited text back, when it differs from the comment, and close its Edit. A
/// refused edit keeps it open with the text.
pub(crate) fn commit_comment(app: &mut WordApp, id: u32) -> Result<(), String> {
    let Some(text) = current(app).and_then(|d| d.edits.get(&id).map(|t| t.trim_end().to_string())) else { return Ok(()) };
    if let Some(part) = app.session.doc.comments.get(&id).map(|c| c.part)
        && text != app.session.doc.plain_text(StoryRef::Part(part))
    {
        app.run("review.editComment", json!({"id": id, "text": text}))?;
    }
    drafts(app).edits.remove(&id);
    Ok(())
}

/// The comment a reply to `id` answers: replies to a reply join its thread.
fn thread_root(app: &WordApp, id: u32) -> u32 {
    match app.session.doc.comments.get(&id).and_then(|c| c.parent) {
        Some(p) if app.session.doc.comments.contains_key(&p) => p,
        _ => id,
    }
}

/// Post the reply typed under comment `id` (nothing when it is blank) and close its field. A
/// refused reply keeps the field open with the text.
pub(crate) fn commit_reply(app: &mut WordApp, id: u32) -> Result<(), String> {
    let Some(text) = current(app).and_then(|d| d.replies.get(&id).cloned()) else { return Ok(()) };
    if !text.trim().is_empty() && app.session.doc.comments.contains_key(&id) {
        let root = thread_root(app, id);
        app.run("review.reply", json!({"id": root, "text": text.trim_end()}))?;
    }
    drafts(app).replies.remove(&id);
    Ok(())
}

/// A click on the page: empty comment fields (and Edits with nothing changed) close, and what has
/// been typed stays in its field, unposted, until it is posted or cancelled.
pub(crate) fn leave_fields(app: &mut WordApp) {
    let unchanged: Vec<u32> = current(app)
        .map(|d| {
            d.edits
                .iter()
                .filter(|(id, t)| app.session.doc.comments.get(id).is_none_or(|c| app.session.doc.plain_text(StoryRef::Part(c.part)) == **t))
                .map(|(id, _)| *id)
                .collect()
        })
        .unwrap_or_default();
    let d = drafts(app);
    if d.new.as_ref().is_some_and(|n| n.text.trim().is_empty()) {
        d.new = None;
    }
    d.replies.retain(|_, t| !t.trim().is_empty());
    d.edits.retain(|id, _| !unchanged.contains(id));
    d.focus = None;
}

/// Any comment field is open in the open document.
pub(crate) fn has_fields(app: &WordApp) -> bool {
    current(app).is_some_and(|d| d.new.is_some() || !d.edits.is_empty() || !d.replies.is_empty())
}

/// A field is open in comment `id`'s balloon: its Edit or a reply's, or its reply field.
pub(crate) fn thread_has_fields(app: &WordApp, id: u32) -> bool {
    current(app).is_some_and(|d| {
        d.replies.contains_key(&id) || d.edits.keys().any(|k| *k == id || app.session.doc.comments.get(k).is_some_and(|c| c.parent == Some(id)))
    })
}

/// Write every pending comment, edit and reply to the document, so a save has them and a close,
/// New or Open sees them as unsaved changes (#1348). Every one is tried. The first refusal is
/// returned (and is on the status line), and refused ones stay. A new comment with nothing typed
/// is no change and stays as it is.
pub(crate) fn flush_comment_drafts(app: &mut WordApp) -> Result<(), String> {
    let (edits, replies, new): (Vec<u32>, Vec<u32>, bool) = match current(app) {
        Some(d) => {
            (d.edits.keys().copied().collect(), d.replies.keys().copied().collect(), d.new.as_ref().is_some_and(|n| !n.text.trim().is_empty()))
        }
        None => return Ok(()),
    };
    let mut first = Ok(());
    for id in edits {
        let r = commit_comment(app, id);
        if first.is_ok() {
            first = r;
        }
    }
    for id in replies {
        let r = commit_reply(app, id);
        if first.is_ok() {
            first = r;
        }
    }
    if new {
        let r = post_comment(app).map(|_| ());
        if first.is_ok() {
            first = r;
        }
    }
    first
}

/// Style Inspector: the paragraph and character levels at the caret, each with its style and the
/// direct formatting on top, and a button to clear each level.
fn inspector(app: &mut WordApp, ui: &mut Ui) {
    if header(ui, "Style Inspector") {
        let _ = app.run("styles.inspector", json!({"value": false}));
        return;
    }
    // Read-only, so call it directly: running the command every frame would pay for the command
    // machinery and consume per-command state (a pending `join_next_undo`) just to draw the pane.
    let r = wordcraft_engine::cmd::inspector::inspect(&app.session);
    let t = Tokens::get(ui.ctx());
    let none = tl!("None").to_string();
    let para = &r["paragraph"];
    let chr = &r["character"];
    let pstyle = para["styleName"].as_str().unwrap_or("Normal").to_string();
    let cstyle = chr["styleName"].as_str().map(str::to_string);
    // Built-in style names show in the interface language; the document keeps the English ones.
    let shown = |id: &Value, name: &str| {
        id.as_str().and_then(|id| app.session.doc.styles.get(id)).map(crate::i18n::style_name).unwrap_or_else(|| name.to_string())
    };
    let pstyle_shown = shown(&para["style"], &pstyle);
    let cstyle_shown = cstyle.as_deref().map(|n| shown(&chr["style"], n));
    let pdirect = direct_list(&para["direct"]);
    let cdirect = direct_list(&chr["direct"]);
    let mut clear: Option<&str> = None;
    egui::ScrollArea::vertical().show(ui, |ui| {
        section(ui, &t, "Paragraph", |ui| {
            if level_row(ui, &t, "Style", &pstyle_shown, false, "Reset to Normal", pstyle == "Normal" && para["style"] == "Normal") {
                clear = Some("paragraphStyle");
            }
            let (txt, empty) = if pdirect.is_empty() { (none.clone(), true) } else { (pdirect.join(", "), false) };
            if level_row(ui, &t, "Direct formatting", &txt, empty, "Clear Paragraph Formatting", empty) {
                clear = Some("paragraphFormatting");
            }
        });
        ui.add_space(8.0);
        section(ui, &t, "Characters", |ui| {
            let (txt, empty) = match &cstyle_shown {
                Some(n) => (n.clone(), false),
                None => (none.clone(), true),
            };
            if level_row(ui, &t, "Style", &txt, empty, "Clear Character Style", empty) {
                clear = Some("characterStyle");
            }
            let (txt, empty) = if cdirect.is_empty() { (none.clone(), true) } else { (cdirect.join(", "), false) };
            if level_row(ui, &t, "Direct formatting", &txt, empty, "Clear Character Formatting", empty) {
                clear = Some("characterFormatting");
            }
        });
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if ui.button(tl!("Clear All")).on_hover_text(tl!("Clear All Formatting")).clicked() {
                let _ = app.run("format.clear", json!({}));
                app.canvas.want_focus = true;
            }
            if ui.button(tl!("Styles Pane")).clicked() {
                let _ = app.run("view.stylesPane", json!({"value": true}));
            }
        });
    });
    if let Some(level) = clear {
        let _ = app.run("styles.inspectorClear", json!({"level": level}));
        app.canvas.want_focus = true;
    }
}

/// A titled, bordered group in the inspector.
fn section(ui: &mut Ui, t: &Tokens, title: &str, body: impl FnOnce(&mut Ui)) {
    ui.label(egui::RichText::new(tl!(title)).font(semibold(12.5)).color(t.text));
    ui.add_space(2.0);
    egui::Frame::NONE.fill(t.input).stroke(Stroke::new(1.0, t.border)).corner_radius(4).inner_margin(8).show(ui, |ui| {
        ui.set_width(ui.available_width());
        body(ui);
    });
}

/// One level: a caption, its value, and a clear button (disabled when there's nothing to clear).
/// Returns true when the button was clicked.
fn level_row(ui: &mut Ui, t: &Tokens, caption: &str, value: &str, dim: bool, tip: &str, disabled: bool) -> bool {
    ui.label(egui::RichText::new(tl!(caption)).font(regular(11.0)).color(t.text_dim));
    let mut clicked = false;
    ui.horizontal(|ui| {
        let w = (ui.available_width() - 26.0).max(40.0);
        ui.allocate_ui(vec2(w, 18.0), |ui| {
            ui.set_width(w);
            let text = egui::RichText::new(value).font(regular(12.5)).color(if dim { t.text_disabled } else { t.text });
            ui.add(egui::Label::new(text).wrap());
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
            let (r, resp) = ui.allocate_exact_size(vec2(22.0, 20.0), egui::Sense::click());
            let enabled = !disabled;
            if enabled && resp.hovered() {
                ui.painter().rect_filled(r, 3.0, t.hover);
            }
            let c = if enabled { t.icon } else { t.text_disabled };
            crate::icons::paint(
                ui.painter(),
                egui::Rect::from_center_size(r.center(), vec2(16.0, 16.0)),
                "clear",
                c,
                if enabled { t.accent } else { c },
            );
            let resp = resp.on_hover_text(tl!(tip));
            clicked = enabled && resp.clicked();
        });
    });
    ui.add_space(4.0);
    clicked
}

/// Readable descriptions of the inspector's `direct` entries (`[{"prop", "value"}]`).
fn direct_list(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().take(64).map(|d| describe(d["prop"].as_str().unwrap_or(""), &d["value"])).collect()).unwrap_or_default()
}

/// Each `{"prop", "value"}` of a list (the Style Inspector's, or a formatting change's) as text.
pub(crate) fn describe_props(list: &[Value]) -> Vec<String> {
    list.iter().take(64).map(|d| describe(d["prop"].as_str().unwrap_or(""), &d["value"])).collect()
}

/// "Bold", "Not Italic", "Font: Arial", "Size: 14 pt"…
fn describe(prop: &str, v: &Value) -> String {
    let label = match prop {
        "font" => "Font",
        "size" => "Size",
        "bold" => "Bold",
        "italic" => "Italic",
        "underline" => "Underline",
        "underlineColor" => "Underline color",
        "strike" => "Strikethrough",
        "doubleStrike" => "Double strikethrough",
        "color" => "Font color",
        "highlight" => "Highlight",
        "shading" => "Shading",
        "vertAlign" => "Position",
        "caps" => "All caps",
        "smallCaps" => "Small caps",
        "hidden" => "Hidden",
        "spacing" => "Character spacing",
        "scale" => "Scale",
        "position" => "Raised/lowered",
        "outline" => "Outline",
        "shadow" => "Shadow",
        "emboss" => "Emboss",
        "engrave" => "Engrave",
        "align" => "Alignment",
        "indentLeft" => "Left indent",
        "indentRight" => "Right indent",
        "indentFirst" => "First line",
        "spaceBefore" => "Space before",
        "spaceAfter" => "Space after",
        "lineSpacing" => "Line spacing",
        "contextualSpacing" => "Don't add space between paragraphs of the same style",
        "mirrorIndents" => "Mirror indents",
        "keepNext" => "Keep with next",
        "keepLines" => "Keep lines together",
        "pageBreakBefore" => "Page break before",
        "widowControl" => "Widow/Orphan control",
        "outlineLevel" => "Outline level",
        "tabs" => "Tabs",
        "borders" => "Borders",
        "suppressHyphens" => "Don't hyphenate",
        "suppressLineNumbers" => "Suppress line numbers",
        "bidi" => "Right-to-left",
        "dropCap" => "Drop cap",
        "dropCapMargin" => "In margin",
        "dropCapDistance" => "Distance from text",
        "dropCapFont" => "Font",
        "style" => "Style",
        "numbering" => "Numbering",
        "lang" => "Language",
        other => other,
    };
    let label = tl!(label);
    let pt = |x: f64| crate::i18n::fmt(tl!("{n} pt"), &[("n", &trim_num(x))]);
    let hex = |c: &Value| -> Option<String> {
        let a = c.as_array()?;
        let ch = |i: usize| a.get(i).and_then(Value::as_u64).unwrap_or(0).min(255);
        Some(format!("#{:02X}{:02X}{:02X}", ch(0), ch(1), ch(2)))
    };
    let value = match (prop, v) {
        (_, Value::Bool(true)) => return label.to_string(),
        (_, Value::Bool(false)) => return crate::i18n::fmt(tl!("Not {name}"), &[("name", label)]),
        ("scale", Value::Number(n)) => format!("{}%", trim_num(n.as_f64().unwrap_or(100.0))),
        ("outlineLevel", Value::Number(n)) => match n.as_u64().unwrap_or(9) {
            l @ 0..=8 => crate::i18n::fmt(tl!("Level {n}"), &[("n", &(l + 1).to_string())]),
            _ => tl!("Body Text").to_string(),
        },
        ("dropCapFont", Value::String(s)) => s.clone(),
        ("dropCap", Value::Number(n)) => crate::i18n::fmt(tl!("{n} lines"), &[("n", &n.to_string())]),
        (_, Value::Number(n)) => pt(n.as_f64().unwrap_or(0.0)),
        ("align", Value::String(s)) => tl!(match s.as_str() {
            "center" => "Centered",
            "right" => "Right",
            "justify" => "Justified",
            "distribute" => "Distributed",
            _ => "Left",
        })
        .to_string(),
        ("color", Value::String(_)) => tl!("Automatic").to_string(),
        ("color", Value::Object(o)) => o.get("Rgb").and_then(hex).unwrap_or_default(),
        (_, Value::Array(_)) if prop != "tabs" => hex(v).unwrap_or_default(),
        ("lineSpacing", Value::Object(o)) => {
            let x = o.get("value").and_then(Value::as_f64).unwrap_or(1.0);
            match o.get("rule").and_then(Value::as_str) {
                Some("atLeast") => crate::i18n::fmt(tl!("At least {n}"), &[("n", &pt(x))]),
                Some("exactly") => crate::i18n::fmt(tl!("Exactly {n}"), &[("n", &pt(x))]),
                _ => crate::i18n::fmt(tl!("{n} lines"), &[("n", &trim_num(x))]),
            }
        }
        (_, Value::String(s)) => tl!(&camel_words(s)).to_string(),
        _ => return label.to_string(),
    };
    format!("{label}: {value}")
}

/// `1.50` → `1.5`, `12.0` → `12`.
fn trim_num(x: f64) -> String {
    let s = format!("{:.2}", if x.is_finite() { x } else { 0.0 });
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// `dotDash` → `Dot dash` (enum values from the document model).
fn camel_words(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for (i, c) in s.chars().take(64).enumerate() {
        if i == 0 {
            out.extend(c.to_uppercase());
        } else if c.is_uppercase() {
            out.push(' ');
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    /// #1902: Clipboard Delete returns focus to the document like paste does.
    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn clipboard_delete_returns_focus_to_the_document() {
        use egui_kittest::kittest::Queryable;

        let mut app = crate::WordApp::new(wordcraft_engine::Session::new(wordcraft_doc::Document::from_text("Hello")), crate::Services::default());
        app.run("select.all", serde_json::json!({})).unwrap();
        app.run("edit.copy", serde_json::json!({})).unwrap();
        assert_eq!(app.session.clip_history.items().len(), 1);
        app.run("edit.clipboardPane", serde_json::json!({"value": true})).unwrap();
        let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1440.0, 900.0)).build_ui_state(
            |ui, app: &mut crate::WordApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            app,
        );
        h.run_steps(4);
        // Focus the button for real: a bare accesskit click leaves egui focus empty, and an
        // unrelated chrome-click helper would hand focus to the canvas on its own.
        h.get_by_label("Delete").focus();
        h.run_steps(2);
        h.state_mut().canvas.want_focus = false;
        h.get_by_label("Delete").click();
        h.run_steps(4);
        assert!(h.state().canvas.focused, "Delete returns focus to the document");
        assert!(h.state().session.clip_history.items().is_empty(), "the item was deleted");
    }
}

#[cfg(test)]
mod navigation_tests {
    use super::*;

    #[test]
    fn long_headings_truncate_without_stretching_navigation() {
        let text = "An extremely long heading with words that must stay inside the navigation pane ".repeat(20);
        for (width, level) in [(260.0, 0), (180.0, 5)] {
            let ctx = egui::Context::default();
            crate::theme::install_fonts(&ctx);
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(900.0, 600.0));
            let mut panel_rect = egui::Rect::NOTHING;
            let mut heading_rect = egui::Rect::NOTHING;
            let output = ctx.run_ui(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |ui| {
                let result = egui::Panel::left("navigation_test")
                    .default_size(width)
                    .resizable(true)
                    .show(ui, |ui| egui::ScrollArea::vertical().show(ui, |ui| nav_heading(ui, level, &text, true)).inner);
                panel_rect = result.response.rect;
                heading_rect = result.inner.rect;
            });

            assert!(panel_rect.width() < width + 20.0, "long heading expanded the {width}px pane to {panel_rect:?}");
            assert!(heading_rect.max.x <= panel_rect.max.x, "heading escaped pane: {heading_rect:?} / {panel_rect:?}");
            assert!(
                output.shapes.iter().any(|s| match &s.shape {
                    egui::Shape::Text(t) => {
                        t.galley.elided && t.galley.rows.last().is_some_and(|r| r.row.glyphs.last().is_some_and(|g| g.chr == '…'))
                    }
                    _ => false,
                }),
                "long heading must render a trailing ellipsis at {width}px"
            );
            output.drop_without_applying_deltas();
        }
    }

    /// Word lists headings flush left, each level indented under its parent (#1199).
    #[test]
    fn headings_align_left_and_indent_by_level() {
        let text_left = |level: u8| {
            let ctx = egui::Context::default();
            crate::theme::install_fonts(&ctx);
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(900.0, 600.0));
            let mut panel_left = 0.0;
            let output = ctx.run_ui(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |ui| {
                let result = egui::Panel::left("navigation_test").default_size(260.0).show(ui, |ui| nav_heading(ui, level, "Kilns", false));
                panel_left = result.response.rect.min.x;
            });
            let x = output
                .shapes
                .iter()
                .find_map(|s| match &s.shape {
                    egui::Shape::Text(t) if t.galley.text() == "Kilns" => Some(t.pos.x + t.galley.rect.min.x),
                    _ => None,
                })
                .unwrap();
            output.drop_without_applying_deltas();
            x - panel_left
        };
        let (top, second) = (text_left(0), text_left(1));
        assert!(top < 30.0, "a top-level heading starts at the pane's left edge, not centred: {top}px in");
        assert!((second - top - 14.0).abs() < 0.5, "a level-2 heading is indented 14px under its parent: {top} / {second}");
    }
}
