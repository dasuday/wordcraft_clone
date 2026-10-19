//! A draw.io picture, edited in place.
//!
//! A PNG exported from draw.io carries the diagram's own XML ([`wordcraft_diagram::png`]), so it
//! can be opened again where it is and edited — the way Word opens a Visio object in place.
//! Double-click it (or pick Edit Diagram from its context menu) and draw.io runs in a child view
//! of the app's window, over the document; Save puts the new picture back with its XML inside it.
//!
//! The editor is [`editor::Editor`]: a view the host's window has to be there for, which is why
//! opening takes two steps. The UI asks ([`Diagram::request`]); the host opens it on its next
//! frame ([`Diagram::open`]), because only the host holds the window.

use egui::{self, Rect, Ui};
use serde_json::json;

use crate::WordApp;
use editor::{Editor, Event, Window};

/// macOS and Windows: draw.io in a child view of the app's window.
#[cfg(any(target_os = "macos", target_os = "windows"))]
#[path = "editor_native.rs"]
pub mod editor;

/// Everywhere else the same picture, read-only.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
#[path = "editor_absent.rs"]
pub mod editor;

/// Whether a picture can be edited here: the editor needs a native window (macOS, Windows).
pub const CAN_EDIT: bool = cfg!(any(target_os = "macos", target_os = "windows"));

/// Why there is no editor here (`None` where there is one): what `ui.editDiagram` answers when a
/// draw.io picture can't be opened.
pub fn unsupported() -> Option<&'static str> {
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Some(editor::Editor::WHY)
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        None
    }
}

/// The strip above the editor that says what is being edited and how to leave it, in points.
const BAR: f32 = 30.0;

/// Is the selected picture a draw.io diagram (a PNG with the diagram's XML inside it)?
pub fn is_drawio_selected(app: &WordApp) -> bool {
    let Some(media) = app.selected_picture_media() else { return false };
    app.session.doc.media.get(&media).is_some_and(|b| wordcraft_diagram::png::is_drawio(b))
}

/// The editor over one picture, and what it has said.
#[derive(Default)]
pub struct Diagram {
    /// A picture the user asked to edit: opened once the host hands over its window.
    request: Option<String>,
    /// The picture the editor was opened for: what its export goes back into, whatever is
    /// selected by the time draw.io answers.
    media: Option<String>,
    /// Where that picture was — story, paragraph and offset — so the export goes back to *that*
    /// occurrence: a copied picture shares the media key, and the selection can move on while the
    /// editor is open.
    at: Option<wordcraft_doc::Pos>,
    editor: Option<Editor>,
    /// Whether it has been put in place yet. It starts hidden, so it never flashes at a spot it
    /// isn't meant to be at.
    placed: bool,
    /// Save was pressed: waiting for the PNG draw.io answers with.
    saving: bool,
    /// Where to put it, in points, written by [`show`] and read by [`WordApp::diagram_frame`].
    rect: Option<Rect>,
    /// Whether [`show`] drew the document area this frame.
    drawn: bool,
}

impl Diagram {
    /// An editor is open over a picture.
    pub fn is_open(&self) -> bool {
        self.editor.is_some()
    }
    /// The media key of a picture waiting to be opened.
    pub fn pending(&self) -> Option<&str> {
        self.request.as_deref()
    }
    /// The media key of the picture the editor is over.
    pub fn media_key(&self) -> Option<&str> {
        self.media.as_deref()
    }
    /// Where that picture was in the document, when the caller knew.
    pub fn at(&self) -> Option<&wordcraft_doc::Pos> {
        self.at.as_ref()
    }

    /// Edit the picture whose media key is `media` — where it is in the document, when the caller
    /// knows — on the frame the window is at hand.
    pub fn request(&mut self, media: String, at: Option<wordcraft_doc::Pos>) {
        self.request = Some(media);
        self.at = at;
    }
    /// Drop a request that can't be served (the picture is gone).
    pub fn cancel(&mut self) {
        self.request = None;
        self.at = None;
    }

    /// Open draw.io over `png`, inside `window`.
    pub fn open(&mut self, window: &Window<'_>, media: &str, png: &[u8]) -> Result<(), String> {
        self.request = None;
        self.saving = false;
        self.placed = false;
        self.media = Some(media.to_owned());
        self.editor = Some(Editor::open(window, png)?);
        Ok(())
    }

    /// draw.io would come from its own site rather than the app: the picture goes to a third
    /// party unless the person says yes ([`Self::allow_remote`]).
    pub fn needs_consent(&self) -> bool {
        self.editor.as_ref().is_some_and(Editor::needs_consent)
    }

    /// The person agreed to load draw.io from its site: hand the diagram over now.
    pub fn allow_remote(&mut self) {
        if let Some(editor) = self.editor.as_mut() {
            editor.allow_remote();
        }
    }

    /// What draw.io has said since the last frame.
    pub fn poll(&mut self) -> Vec<Event> {
        let Some(editor) = self.editor.as_mut() else { return Vec::new() };
        let events = editor.poll();
        // The diagram is on screen: give it the keyboard.
        if events.iter().any(|e| matches!(e, Event::Ready)) {
            editor.focus();
        }
        events
    }

    /// Ask draw.io for the picture: the answer comes back as an `Exported` event.
    pub fn save(&mut self) {
        if let Some(editor) = self.editor.as_mut() {
            self.saving = true;
            editor.save();
        }
    }

    /// Put the editor away.
    pub fn close(&mut self) {
        self.editor = None;
        self.saving = false;
        self.placed = false;
        self.media = None;
        self.at = None;
        self.rect = None;
    }

    /// Where the editor goes, in points, written by [`show`] and read by [`WordApp::diagram_frame`].
    pub fn placement(&self) -> Option<Rect> {
        self.rect
    }

    /// A frame is starting: [`show`] says whether the document area is drawn in it.
    pub fn begin_frame(&mut self) {
        self.drawn = false;
    }

    /// The frame is over: the editor is a native view above everything the app paints, so put it
    /// away when the document area wasn't drawn (File, no document) or something covers it (a
    /// dialog) — measured by what [`show`] did and by `covered`.
    pub fn settle(&mut self, covered: bool) {
        let Some(editor) = self.editor.as_ref() else { return };
        if covered || !self.drawn {
            editor.set_visible(false);
            self.placed = false;
            self.rect = None;
        }
    }

    /// Put the editor over `rect` (egui points, top-left origin) and show it. wry turns the
    /// top-left y into whatever the platform's native views count from (on macOS it flips the
    /// y itself, see its `window_position`), so no flipping here — doing it here too put the
    /// view upside down over the strip that says how to leave the editor.
    pub fn place(&mut self, rect: Rect) {
        let Some(editor) = self.editor.as_ref() else { return };
        editor.set_bounds(f64::from(rect.min.x), f64::from(rect.min.y.max(0.0)), f64::from(rect.width()), f64::from(rect.height()));
        // Waiting for a yes to draw.io's site: the view stays hidden behind the question.
        if !self.placed && !self.needs_consent() {
            self.placed = true;
            editor.set_visible(true);
            editor.focus();
        }
    }
}

/// The document area, minus a strip for the bar while the editor is open: drawn after the
/// canvas, so the page can't paint over the strip that says how to leave the editor.
pub fn show(app: &mut WordApp, ui: &mut Ui) {
    let full = ui.clip_rect();
    let d = &mut app.diagram;
    d.drawn = true;
    if !d.is_open() {
        d.rect = Some(full);
        return;
    }
    let (bar, rest) = full.split_top_bottom_at_y(full.min.y + BAR);
    d.rect = Some(rest);
    // Over the editor, so it stays reachable while draw.io has the keyboard.
    ui.scope_builder(egui::UiBuilder::new().max_rect(bar), |ui| {
        let t = crate::theme::Tokens::get(ui.ctx());
        egui::Frame::NONE.fill(t.canvas).show(ui, |ui| {
            // The build has no bundled draw.io: the editor would run from draw.io's own site, so
            // the picture would be sent there. Ask first; until then nothing has been handed over.
            if app.diagram.needs_consent() {
                ui.horizontal_centered(|ui| {
                    ui.label(egui::RichText::new(tl!("draw.io isn't bundled in this build.")).strong());
                    ui.label(tl!("Editing here loads draw.io from embed.diagrams.net, which sends this diagram to that site."));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(tl!("Cancel")).clicked() {
                            app.diagram.close();
                            app.status(tl!("The diagram was left as it was."));
                        }
                        if ui.button(tl!("Load from the web")).clicked() {
                            app.diagram.allow_remote();
                        }
                    });
                });
                return;
            }
            ui.horizontal_centered(|ui| {
                ui.label(egui::RichText::new(tl!("Editing diagram")).strong());
                ui.label(tl!("Save puts it back into the document; Cancel leaves it as it was."));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(tl!("Cancel")).clicked() {
                        app.diagram.close();
                        app.status(tl!("The diagram was left as it was."));
                    }
                    let saving = app.diagram.saving;
                    if ui.add_enabled(!saving, egui::Button::new(tl!("Save"))).clicked() {
                        app.diagram.save();
                    }
                });
            });
        });
    });
    // Escape leaves as well: it commits, as clicking away from an activated object does.
    if !app.diagram.needs_consent() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.diagram.save();
    }
    // The view is a native child, so its place has to be kept up with the window: draw as long
    // as it is open.
    ui.ctx().request_repaint();
}

/// An `Exported` PNG, put back where the picture is (base64, as every command carries bytes).
///
/// `media` is the picture the editor was opened for and `at` where it was: `diagram.update` works
/// on the selection, so the picture is selected again first. The selection can move on while
/// draw.io is open, and a copy of the picture shares its media key — the key alone would let the
/// edit land on the copy, or nowhere.
pub fn apply(app: &mut WordApp, png: Vec<u8>, media: &str, at: Option<&wordcraft_doc::Pos>) {
    if app.selected_picture_media().as_deref() != Some(media) {
        if let Some(at) = at.filter(|at| picture_at(app, at) == Some(media)) {
            let _ = app.run("caret.set", json!({"pos": at}));
        }
        if app.selected_picture_media().as_deref() != Some(media) {
            app.status(tl!("The diagram was left as it was: the picture it belongs to isn't in the document any more."));
            return;
        }
    }
    let data = wordcraft_engine::cmd::insert::base64_encode(&png);
    match app.run("diagram.update", json!({"data": data})) {
        Ok(_) => app.status(tl!("The diagram was updated.")),
        Err(e) => app.status(e),
    }
}

/// The media key of the picture at `at`, when there is a picture there.
fn picture_at<'a>(app: &'a WordApp, at: &wordcraft_doc::Pos) -> Option<&'a str> {
    let para = app.session.doc.para(at.story, &at.path)?;
    match para.object_at(at.off) {
        Some(wordcraft_doc::para::InlineObject::Image { media, .. }) => Some(media.as_str()),
        _ => None,
    }
}
