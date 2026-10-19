//! The text context menu: a search field, the formatting controls, clipboard actions and the
//! commands for the click. Object, equation and table menus keep the plain list in `canvas`.

use egui::{Align, Layout, Rect, RichText, Sense, TextEdit, Ui, Vec2, pos2, vec2};
use serde_json::{Value, json};

use crate::theme::{Tokens, regular};
use crate::{WordApp, icons, widgets};

const ROW_H: f32 = 24.0;
const MENU_W: f32 = 280.0;

/// Whether every word of `query` occurs in the translated `label`. An empty query matches.
pub(crate) fn matches_query(query: &str, label: &str) -> bool {
    let query = query.trim();
    if query.is_empty() {
        return true;
    }
    let hay = crate::i18n::lowercase(tl!(label));
    let q = crate::i18n::lowercase(query);
    q.split_whitespace().all(|w| hay.contains(w))
}

fn any_match(query: &str, labels: &[&str]) -> bool {
    labels.iter().any(|l| matches_query(query, l))
}

/// The text context menu, in one popup. `before` is the spelling block; `after` is whatever the
/// click landed on (a field, a table of contents).
pub fn show(app: &mut WordApp, ui: &mut Ui, before: impl FnOnce(&mut Ui, &mut WordApp), after: impl FnOnce(&mut Ui, &mut WordApp)) {
    let t = Tokens::get(ui.ctx());
    egui::Frame::popup(ui.style()).inner_margin(egui::Margin::symmetric(8, 6)).show(ui, |ui| {
        ui.set_width(MENU_W);
        // Menu styling removes input borders; the font and size boxes need the document style.
        ui.set_style(ui.ctx().global_style());
        ui.spacing_mut().item_spacing.y = 4.0;
        before(ui, app);
        search_field(app, ui, &t);
        let query = app.canvas.context_menu_query.clone();
        if any_match(
            &query,
            &[
                "Font",
                "Font Size",
                "Increase Font Size",
                "Decrease Font Size",
                "Clear All Formatting",
                "Change Case",
                "Bold",
                "Italic",
                "Underline",
                "Text Highlight Color",
                "Font Color",
                "Bullets",
                "Numbering",
            ],
        ) {
            ui.add_space(2.0);
            format_rows(app, ui);
        }
        let mut paste_hovered = false;
        if any_match(&query, &["Clipboard", "Cut", "Copy", "Paste", "Merge Formatting", "Keep Text Only"]) {
            ui.add_space(2.0);
            paste_hovered = transfer(app, ui, &t);
        }
        if !paste_hovered {
            clear_paste_preview(app);
        }
        if any_match(&query, &["Font…", "Paragraph…", "Link…", "New Comment", "Synonyms", "Translate", "Read Aloud"]) || synonym_matches(&query, app)
        {
            ui.separator();
            actions(app, ui, &t);
        }
        after(ui, app);
        if !query.trim().is_empty() {
            search_hits(app, ui, &query, &t);
        }
        if app.dialog.is_some() || app.colors.is_some() {
            ui.close();
        }
    });
}

fn search_field(app: &mut WordApp, ui: &mut Ui, t: &Tokens) {
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
        icons::paint(ui.painter(), r, "search", t.text_dim, t.accent);
        let mut query = app.canvas.context_menu_query.clone();
        let edit = TextEdit::singleline(&mut query)
            .id(egui::Id::new("wc_context_search"))
            .hint_text(tl!("Search commands and help"))
            .desired_width(ui.available_width())
            .margin(egui::Margin::symmetric(4, 2));
        let resp = ui.add(edit);
        if app.canvas.context_menu_focus_search {
            resp.request_focus();
            app.canvas.context_menu_focus_search = false;
        }
        if resp.changed() {
            app.canvas.context_menu_query = query;
        }
    });
}

fn format_rows(app: &mut WordApp, ui: &mut Ui) {
    let st: Value = app.session.run("format.state", &json!({})).unwrap_or(Value::Null);
    let flag = |k: &str| st.get(k).and_then(Value::as_bool).unwrap_or(false);
    let row = |ui: &mut Ui, app: &mut WordApp, add: &dyn Fn(&mut Ui, &mut WordApp)| {
        ui.allocate_ui_with_layout(Vec2::new(ui.available_width(), ROW_H), Layout::left_to_right(Align::Center), |ui| {
            ui.set_min_height(ROW_H);
            ui.spacing_mut().item_spacing.x = 2.0;
            add(ui, app);
        });
    };
    row(ui, app, &|ui, app| {
        let font = st.get("font").and_then(Value::as_str).unwrap_or("").to_string();
        let font = app.previews.font_shown(&font);
        let fams = app.previews.families();
        let recent: Vec<String> = app.ui.recent_fonts.iter().map(|f| app.previews.font_shown(f)).collect();
        let prev = app.previews.font_preview_fn();
        if let Some(f) = widgets::font_combo_with_recent_hint(ui, "ctx_font", 96.0, &font, &fams, &recent, Some(&*prev), tl!("Font")) {
            let _ = if f == widgets::ADD_FONT {
                app.run("font.add", json!({}))
            } else if f == widgets::LOAD_SYSTEM_FONTS {
                app.run("font.loadSystem", json!({}))
            } else {
                let f = app.previews.font_stored(&f);
                app.run("format.font", json!({"name": f}))
            };
        }
        let size =
            st.get("size").and_then(Value::as_f64).map(|s| if s.fract() == 0.0 { format!("{s:.0}") } else { s.to_string() }).unwrap_or_default();
        let sizes: Vec<String> =
            wordcraft_engine::cmd::format::SIZES.iter().map(|s| if s.fract() == 0.0 { format!("{s:.0}") } else { s.to_string() }).collect();
        if let Some(v) = widgets::combo_hint(ui, "ctx_size", 48.0, &size, &sizes, None, tl!("Font Size"))
            && let Ok(x) = v.trim().parse::<f64>()
        {
            let _ = app.run("format.size", json!({"size": x}));
        }
        widgets::small(ui, app, "grow", None, "Increase Font Size", "format.growFont", json!({}), false);
        widgets::small(ui, app, "shrink", None, "Decrease Font Size", "format.shrinkFont", json!({}), false);
        widgets::small(ui, app, "clear", None, "Clear All Formatting", "format.clear", json!({}), false);
        widgets::menu_button(ui, app, "case", None, "Change Case", false, "format.changeCase", |ui, app| {
            crate::ribbon::mi(ui, app, "Sentence case.", "format.changeCase", json!({"mode": "sentence"}));
            crate::ribbon::mi(ui, app, "lowercase", "format.changeCase", json!({"mode": "lower"}));
            crate::ribbon::mi(ui, app, "UPPERCASE", "format.changeCase", json!({"mode": "upper"}));
            crate::ribbon::mi(ui, app, "Capitalize Each Word", "format.changeCase", json!({"mode": "title"}));
            crate::ribbon::mi(ui, app, "tOGGLE cASE", "format.changeCase", json!({"mode": "toggle"}));
        });
    });
    row(ui, app, &|ui, app| {
        widgets::small(ui, app, "bold", None, "Bold", "format.bold", json!({}), flag("bold"));
        widgets::small(ui, app, "italic", None, "Italic", "format.italic", json!({}), flag("italic"));
        widgets::small(ui, app, "underline", None, "Underline", "format.underline", json!({}), flag("underline"));
        let hl = app.canvas.last_highlight.clone();
        widgets::split(ui, app, "highlight", "Text Highlight Color", "format.highlight", json!({"color": hl}), false, None, |ui, app| {
            crate::mini_toolbar::highlight_menu(ui, app);
        });
        let fc = app.canvas.last_font_color.clone();
        let sw = wordcraft_doc::Rgb::parse(&fc).map(crate::theme::c32);
        widgets::split(ui, app, "fontcolor", "Font Color", "format.color", json!({"color": fc}), false, sw, |ui, app| {
            crate::mini_toolbar::font_color_menu(ui, app);
        });
        let rp = app.session.doc.para_at(&app.session.sel.focus).map(|p| app.session.doc.styles.resolve_para(&p.props));
        let lists = wordcraft_engine::cmd::para::list_buttons(&app.session.doc, rp.as_ref().and_then(|r| r.numbering));
        widgets::small(ui, app, "bullets", None, "Bullets", "para.bullets", json!({}), lists.bullets);
        widgets::small(ui, app, "numbering", None, "Numbering", "para.numbering", json!({}), lists.numbering);
    });
}

fn transfer(app: &mut WordApp, ui: &mut Ui, t: &Tokens) -> bool {
    ui.horizontal(|ui| {
        ui.label(RichText::new(tl!("Clipboard")).small().weak());
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            for (icon, tip, id) in [("paste", "Paste", "edit.paste"), ("copy", "Copy", "edit.copy"), ("cut", "Cut", "edit.cut")] {
                if widgets::small(ui, app, icon, None, tip, id, json!({}), false).clicked() {
                    // The button already ran. A hover preview is an earlier undo step: drop both
                    // and run this command once on the document as it was.
                    if app.canvas.context_paste_preview.take().is_some() {
                        let _ = app.run("edit.undo", json!({}));
                        let _ = app.run("edit.undo", json!({}));
                        let _ = app.run(id, json!({}));
                    }
                    ui.close();
                }
            }
        });
    });
    let mut hovered = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let w = (ui.available_width() - 8.0) / 3.0;
        hovered |= mode_button(ui, app, t, w, "Paste", "edit.paste");
        hovered |= mode_button(ui, app, t, w, "Merge Formatting", "edit.pasteMerge");
        hovered |= mode_button(ui, app, t, w, "Keep Text Only", "edit.pasteText");
    });
    hovered
}

/// Drop a hover preview, undoing the paste it applied. A click clears the marker without undoing.
pub(crate) fn clear_paste_preview(app: &mut WordApp) {
    if app.canvas.context_paste_preview.take().is_some() {
        let _ = app.run("edit.undo", json!({}));
        app.canvas.scroll_to_caret = true;
        // The page was already drawn this frame; the restored text shows on the next one.
    }
}

/// Show `id`'s paste in the document while the pointer is on that option.
fn show_paste_preview(app: &mut WordApp, ui: &Ui, id: &str) {
    if app.canvas.context_paste_preview.as_deref() == Some(id) {
        return;
    }
    clear_paste_preview(app);
    let depth = app.session.undo_depth();
    if app.run(id, json!({})).is_ok() && app.session.undo_depth() > depth {
        app.canvas.context_paste_preview = Some(id.to_string());
        app.canvas.scroll_to_caret = true;
        ui.ctx().request_repaint();
    }
}

fn mode_button(ui: &mut Ui, app: &mut WordApp, t: &Tokens, w: f32, label: &str, id: &str) -> bool {
    let text = tl!(label);
    let on = widgets::enabled(app, id);
    let (r, resp) = ui.allocate_exact_size(vec2(w, 28.0), Sense::click());
    let fill = if resp.is_pointer_button_down_on() {
        t.pressed
    } else if resp.hovered() {
        t.hover
    } else {
        t.input
    };
    ui.painter().rect_filled(r, 4.0, fill);
    ui.painter().rect_stroke(r, 4.0, egui::Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
    let (c, a) = if on { (t.icon, t.accent) } else { (t.text_disabled, t.text_disabled) };
    let ic = Rect::from_center_size(pos2(r.min.x + 14.0, r.center().y), vec2(14.0, 14.0));
    icons::paint(ui.painter(), ic, "paste", c, a);
    let color = if on { t.text } else { t.text_disabled };
    let shown = shorten(ui, text, (w - 30.0).max(24.0));
    ui.painter().text(pos2(r.min.x + 26.0, r.center().y), egui::Align2::LEFT_CENTER, &shown, regular(11.0), color);
    let resp = resp.on_hover_text(text);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, on, text));
    if resp.hovered() && on {
        show_paste_preview(app, ui, id);
    }
    if resp.clicked() && on {
        // The hover already pasted this way; keep that step instead of inserting it again.
        if app.canvas.context_paste_preview.as_deref() != Some(id) {
            let _ = app.run(id, json!({}));
        }
        app.canvas.context_paste_preview = None;
        ui.close();
    }
    resp.hovered() && on
}

fn actions(app: &mut WordApp, ui: &mut Ui, t: &Tokens) {
    action_row(ui, app, t, "fontdialog", "Font…", "ui.dialog", json!({"name": "font"}));
    action_row(ui, app, t, "pilcrow", "Paragraph…", "ui.dialog", json!({"name": "paragraph"}));
    action_row(ui, app, t, "link", "Link…", "ui.dialog", json!({"name": "link"}));
    action_row(ui, app, t, "newComment", "New Comment", "review.newComment", json!({}));
    synonyms_row(app, ui, t);
    action_row(ui, app, t, "translate", "Translate", "review.translate", json!({}));
    action_row(ui, app, t, "readAloud", "Read Aloud", "review.readAloud", json!({}));
}

fn synonyms_row(app: &mut WordApp, ui: &mut Ui, t: &Tokens) {
    let syns: Vec<serde_json::Value> = app.canvas.context_synonyms.clone().and_then(|v| v.as_array().cloned()).unwrap_or_default();
    let query = app.canvas.context_menu_query.clone();
    if !matches_query(&query, "Synonyms") && !synonym_matches(&query, app) {
        return;
    }
    let text = tl!("Synonyms");
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(r, 3.0, t.hover);
    }
    let ic = Rect::from_center_size(pos2(r.min.x + 12.0, r.center().y), vec2(16.0, 16.0));
    icons::paint(ui.painter(), ic, "thesaurus", t.icon, t.accent);
    ui.painter().text(pos2(r.min.x + 26.0, r.center().y), egui::Align2::LEFT_CENTER, text, regular(12.5), t.text);
    ui.painter().text(pos2(r.max.x - 8.0, r.center().y), egui::Align2::RIGHT_CENTER, "›", regular(14.0), t.text_dim);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, text));
    egui::Popup::menu(&resp).show(|ui| {
        ui.set_min_width(160.0);
        if syns.is_empty() {
            ui.label(RichText::new(tl!("(no suggestions)")).italics());
        } else {
            synonym_menu(app, ui, &syns);
        }
    });
}

/// `text` cut to `max` points, with an ellipsis when it does not fit.
fn shorten(ui: &Ui, text: &str, max: f32) -> String {
    let font = regular(11.0);
    let width = |s: &str| ui.painter().layout_no_wrap(s.to_string(), font.clone(), t_placeholder()).size().x;
    if width(text) <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(48).collect();
    while out.chars().count() > 1 && width(&format!("{out}…")) > max {
        out.pop();
    }
    format!("{out}…")
}

fn t_placeholder() -> egui::Color32 {
    egui::Color32::WHITE
}

fn action_row(ui: &mut Ui, app: &mut WordApp, t: &Tokens, icon: &str, label: &str, id: &str, params: Value) {
    if !matches_query(&app.canvas.context_menu_query, label) {
        return;
    }
    let text = tl!(label);
    let sc = widgets::shortcut_text(app, id);
    let on = widgets::enabled(app, id);
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(r, 3.0, t.hover);
    }
    let (c, a) = if on { (t.icon, t.accent) } else { (t.text_disabled, t.text_disabled) };
    let ic = Rect::from_center_size(pos2(r.min.x + 12.0, r.center().y), vec2(16.0, 16.0));
    icons::paint(ui.painter(), ic, icon, c, a);
    let color = if on { t.text } else { t.text_disabled };
    ui.painter().text(pos2(r.min.x + 26.0, r.center().y), egui::Align2::LEFT_CENTER, text, regular(12.5), color);
    if !sc.is_empty() {
        ui.painter().text(pos2(r.max.x - 4.0, r.center().y), egui::Align2::RIGHT_CENTER, &sc, regular(11.0), t.text_dim);
    }
    let shown = if sc.is_empty() { text.to_string() } else { format!("{text} {sc}") };
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, on, &shown));
    if resp.clicked() && on {
        let _ = app.run(id, params);
        ui.close();
    }
}

fn synonym_matches(query: &str, app: &WordApp) -> bool {
    let Some(syn) = app.canvas.context_synonyms.as_ref().and_then(|v| v.as_array()) else { return false };
    syn.iter().filter_map(|x| x.as_str()).any(|w| matches_query(query, w))
}

/// Replace the word at the caret with a thesaurus suggestion.
pub(crate) fn synonym_menu(app: &mut WordApp, ui: &mut Ui, syn: &[serde_json::Value]) {
    for w in syn.iter().filter_map(|x| x.as_str()) {
        if ui.button(w).clicked() {
            let _ = app.run("select.word", json!({}));
            let (a, b) = app.session.sel.ordered();
            let trimmed = app.session.doc.para_at(&a).and_then(|p| p.text.get(a.off..b.off)).map(|t| t.trim_end().len()).unwrap_or(0);
            let end = wordcraft_doc::Pos { off: a.off + trimmed, ..b };
            app.session.sel = wordcraft_engine::Selection { anchor: a, focus: end };
            let _ = app.run("text.insert", json!({"text": w, "raw": true}));
            ui.close();
        }
    }
}

fn search_hits(app: &mut WordApp, ui: &mut Ui, query: &str, t: &Tokens) {
    let query = query.trim();
    if query.is_empty() {
        return;
    }
    let mut shown = 0;
    let specs: Vec<(&str, &str)> = app.session.registry.all().iter().map(|c| (c.id, c.label)).collect();
    for (id, label) in specs {
        if shown >= 6 {
            break;
        }
        if primary_command(id) || !matches_query(query, label) {
            continue;
        }
        if shown == 0 {
            ui.separator();
            ui.label(RichText::new(tl!("Search Commands")).small().weak());
        }
        shown += 1;
        action_row(ui, app, t, "search", label, id, json!({}));
    }
}

/// Commands the text menu already draws, so a search does not list them twice.
fn primary_command(id: &str) -> bool {
    matches!(
        id,
        "edit.cut"
            | "edit.copy"
            | "edit.paste"
            | "edit.pasteText"
            | "edit.pasteMerge"
            | "format.font"
            | "format.size"
            | "format.growFont"
            | "format.shrinkFont"
            | "format.clear"
            | "format.changeCase"
            | "format.bold"
            | "format.italic"
            | "format.underline"
            | "format.highlight"
            | "format.color"
            | "para.bullets"
            | "para.numbering"
            | "review.newComment"
            | "review.readAloud"
            | "review.translate"
            | "ui.dialog"
            | "tools.search"
    )
}
