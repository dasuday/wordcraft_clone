//! In-app writing assistant. Only reviewed plain text enters the engine; models get no tools.
use crate::WordApp;
use serde_json::{Value, json};
use zeroize::Zeroize;

/// Connection preferences use the same local settings file and save lifecycle as other options.
#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Connection {
    #[serde(default)]
    pub(crate) endpoint: String,
    #[serde(default)]
    pub(crate) model: String,
    #[serde(default)]
    pub(crate) key: String,
}

impl std::fmt::Debug for Connection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connection").field("endpoint", &self.endpoint).field("model", &self.model).finish_non_exhaustive()
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.key.zeroize();
    }
}

const MAX_PROFILES: usize = 32;

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Profile {
    id: u64,
    name: String,
    connection: Connection,
}

impl Profile {
    fn label(&self) -> String {
        let name = if self.name.trim().is_empty() { format!("{} {}", tl!("Model connection"), self.id) } else { self.name.clone() };
        if self.connection.model.is_empty() { name } else { format!("{name} — {}", self.connection.model) }
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Profiles {
    #[serde(deserialize_with = "read_profiles")]
    entries: Vec<Profile>,
    default_id: u64,
    next_id: u64,
}

fn read_profiles<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Vec<Profile>, D::Error> {
    use serde::Deserialize;
    let values = Vec::<Value>::deserialize(deserializer)?;
    Ok(values.iter().take(MAX_PROFILES).map(crate::options::lenient).collect())
}

impl Default for Profiles {
    fn default() -> Self {
        Self { entries: vec![Profile { id: 1, ..Default::default() }], default_id: 1, next_id: 2 }
    }
}

impl Profiles {
    pub(crate) fn sanitize(&mut self) {
        self.entries.truncate(MAX_PROFILES);
        let mut ids = std::collections::HashSet::new();
        self.entries.retain(|p| p.id != 0 && ids.insert(p.id));
        if !self.entries.iter().any(|p| p.id == self.default_id) {
            self.default_id = self.entries.first().map_or(0, |p| p.id);
        }
    }

    pub(crate) fn redact(&mut self) {
        for p in &mut self.entries {
            p.connection.key.zeroize();
        }
    }

    fn selected(&self, override_id: Option<u64>) -> Result<&Connection, String> {
        let id = override_id.unwrap_or(self.default_id);
        self.entries
            .iter()
            .find(|p| p.id == id)
            .map(|p| &p.connection)
            .ok_or_else(|| "Choose a saved model connection in Writing Assistant options".into())
    }

    fn add(&mut self) -> Result<u64, String> {
        if self.entries.len() >= MAX_PROFILES {
            return Err("At most 32 model connections can be saved".into());
        }
        let max = self.entries.iter().map(|p| p.id).max().unwrap_or(0);
        let id = self.next_id.max(max.checked_add(1).ok_or("Model connection identifiers exhausted")?);
        self.next_id = id.checked_add(1).ok_or("Model connection identifiers exhausted")?;
        self.entries.push(Profile { id, ..Default::default() });
        if self.default_id == 0 {
            self.default_id = id;
        }
        Ok(id)
    }

    fn remove(&mut self, id: u64) {
        self.entries.retain(|p| p.id != id);
        self.sanitize();
    }
}

/// Upgrade the original single connection without losing its key or model.
pub(crate) fn migrate_preferences(value: &mut Value) {
    let Some(object) = value.as_object_mut() else { return };
    let legacy = object.remove("assistantConnection");
    if !object.contains_key("assistantProfiles")
        && let Some(legacy) = legacy
    {
        let connection: Connection = crate::options::lenient(&legacy);
        let profiles = Profiles { entries: vec![Profile { id: 1, connection, ..Default::default() }], ..Default::default() };
        if let Ok(saved) = serde_json::to_value(profiles) {
            object.insert("assistantProfiles".into(), saved);
        }
    }
}

/// Called only by File > Options > Writing Assistant; the assistant window contains writing controls.
pub(crate) fn settings(profiles: &mut Profiles, state: &mut State, ui: &mut egui::Ui) {
    if !profiles.entries.iter().any(|p| p.id == state.edit_profile) {
        state.edit_profile = profiles.default_id;
    }
    let label = profiles.entries.iter().find(|p| p.id == state.edit_profile).map(Profile::label).unwrap_or_else(|| tl!("Model connection").into());
    egui::ComboBox::from_id_salt("assistant_profile_editor").selected_text(label).show_ui(ui, |ui| {
        for p in &profiles.entries {
            ui.selectable_value(&mut state.edit_profile, p.id, p.label());
        }
    });
    ui.horizontal(|ui| {
        if ui.add_enabled(profiles.entries.len() < MAX_PROFILES, egui::Button::new(tl!("Add"))).clicked() {
            match profiles.add() {
                Ok(id) => state.edit_profile = id,
                Err(e) => state.error = e,
            }
        }
        if ui.add_enabled(!profiles.entries.is_empty(), egui::Button::new(tl!("Remove"))).clicked() {
            profiles.remove(state.edit_profile);
            if state.override_profile == Some(state.edit_profile) {
                state.override_profile = None;
            }
            state.edit_profile = profiles.default_id;
        }
    });
    let Some(profile) = profiles.entries.iter_mut().find(|p| p.id == state.edit_profile) else { return };
    if ui.radio(profiles.default_id == profile.id, tl!("Default")).clicked() {
        profiles.default_id = profile.id;
    }
    ui.label(tl!("Name"));
    ui.add(egui::TextEdit::singleline(&mut profile.name).char_limit(128).desired_width(320.0));
    let connection = &mut profile.connection;
    ui.label(tl!("OpenAI-compatible chat completions URL"));
    ui.add(
        egui::TextEdit::singleline(&mut connection.endpoint)
            .hint_text(tl!("https://your-server/v1/chat/completions"))
            .char_limit(2048)
            .desired_width(f32::INFINITY),
    );
    ui.label(tl!("Model name"));
    ui.add(egui::TextEdit::singleline(&mut connection.model).char_limit(256).desired_width(320.0));
    ui.label(tl!("API key (saved unencrypted with local preferences)"));
    ui.add(egui::TextEdit::singleline(&mut connection.key).char_limit(8192).desired_width(320.0));
}

#[derive(Debug)]
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
enum Update {
    Delta(String),
    Finished(Result<(), String>),
}

pub struct State {
    pub open: bool,
    edit_profile: u64,
    override_profile: Option<u64>,
    mode: String,
    prompt: String,
    request: Option<Value>,
    result: String,
    error: String,
    cancelled: bool,
    pending: Option<std::sync::mpsc::Receiver<Update>>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            open: false,
            edit_profile: 0,
            override_profile: None,
            mode: "draft".into(),
            prompt: String::new(),
            request: None,
            result: String::new(),
            error: String::new(),
            cancelled: false,
            pending: None,
        }
    }
}

/// UI commands are also available through the control channel. Credentials are read from
/// local preferences, never recorded in document commands or document saves.
pub fn command(app: &mut WordApp, id: &str, v: &Value) -> Result<Value, String> {
    match id {
        "ui.assistant.generate" => {
            if app.assistant.pending.is_some() {
                return Err("A request is already running".into());
            }
            let request = app
                .session
                .run("assistant.prepare", &json!({"mode":app.assistant.mode, "prompt":app.assistant.prompt}))
                .map_err(|e| e.to_string())?;
            let rx = start(app.ui.assistant_profiles.selected(app.assistant.override_profile)?, &request)?;
            app.assistant.request = Some(request);
            app.assistant.pending = Some(rx);
            app.assistant.cancelled = false;
            app.assistant.result.clear();
            app.assistant.error.clear();
        }
        "ui.assistant.discard" => {
            app.assistant.cancelled = true;
            app.assistant.request = None;
            app.assistant.result.clear();
            app.assistant.error.clear();
        }
        "ui.assistant.apply" => {
            if app.assistant.pending.is_some() || !app.assistant.error.is_empty() {
                return Err("Wait for a complete, successful suggestion before accepting".into());
            }
            let r = app.assistant.request.clone().ok_or("Generate a suggestion first")?;
            app.run("assistant.apply", json!({"request":r,"text":app.assistant.result,"placement":v.get("placement")}))?;
            app.assistant.request = None;
            app.assistant.result.clear();
            app.assistant.error.clear();
        }
        _ => return Err(format!("Unknown assistant command: {id}")),
    }
    Ok(json!({}))
}

#[cfg(target_arch = "wasm32")]
fn start(_: &Connection, _: &Value) -> Result<std::sync::mpsc::Receiver<Update>, String> {
    Err("Model connections are available in the desktop app. The web editor can still use assistant.prepare and assistant.apply through an external host.".into())
}

#[cfg(not(target_arch = "wasm32"))]
fn endpoint(value: &str) -> Result<reqwest::Url, String> {
    let url = reqwest::Url::parse(value).map_err(|_| "Enter a valid chat completions URL")?;
    let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if !(url.scheme() == "https" || (url.scheme() == "http" && local))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("Use HTTPS, or HTTP on localhost, without credentials, query or fragment in the URL".into());
    }
    Ok(url)
}

#[cfg(not(target_arch = "wasm32"))]
fn start(state: &Connection, request: &Value) -> Result<std::sync::mpsc::Receiver<Update>, String> {
    if state.endpoint.len() > 2048 {
        return Err("Model endpoint is too long".into());
    }
    let url = endpoint(&state.endpoint)?;
    if state.model.trim().is_empty() || state.model.len() > 256 {
        return Err("Enter a model name (up to 256 bytes)".into());
    }
    if state.key.len() > 8192 {
        return Err("API key is too long".into());
    }
    let body = json!({"model": state.model.trim(), "messages":request.get("messages"), "stream":true});
    let key = zeroize::Zeroizing::new(state.key.clone());
    let (tx, rx) = std::sync::mpsc::sync_channel(32);
    std::thread::Builder::new()
        .name("writing-assistant".into())
        .spawn(move || {
            let result = generate(url, key, body, |text| tx.send(Update::Delta(text)).map_err(|_| "Assistant closed".into()));
            let _ = tx.send(Update::Finished(result));
        })
        .map_err(|_| "Could not start the writing assistant")?;
    Ok(rx)
}

#[cfg(not(target_arch = "wasm32"))]
fn generate(
    url: reqwest::Url,
    key: zeroize::Zeroizing<String>,
    body: Value,
    mut emit: impl FnMut(String) -> Result<(), String>,
) -> Result<(), String> {
    use std::io::Read;
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(90))
        .connect_timeout(std::time::Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "Could not initialize the model connection")?;
    let mut request = client.post(url).json(&body);
    if !key.is_empty() {
        request = request.bearer_auth(key.as_str());
    }
    let response = request.send().map_err(|e| {
        if e.is_timeout() { "The model request timed out" } else { "Could not connect to the model. Check the endpoint and connection" }
    })?;
    if !response.status().is_success() {
        return Err(format!("Model returned HTTP {}. Check your model name, API key and provider quota", response.status().as_u16()));
    }
    if response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(';').next().is_some_and(|mime| mime.trim().eq_ignore_ascii_case("text/event-stream")))
    {
        return parse_stream(response, emit);
    }
    // Some compatible servers return JSON even when streaming is requested.
    let mut bytes = Vec::new();
    response.take(MAX_RESPONSE + 1).read_to_end(&mut bytes).map_err(|_| "Could not read the model response")?;
    if bytes.len() as u64 > MAX_RESPONSE {
        return Err("Model response is too large".into());
    }
    emit(parse_response(&bytes)?)
}

#[cfg(any(test, not(target_arch = "wasm32")))]
const MAX_RESPONSE: u64 = 1024 * 1024;

/// Parse SSE at event boundaries, not network chunk boundaries (which can split UTF-8).
#[cfg(any(test, not(target_arch = "wasm32")))]
fn parse_stream(reader: impl std::io::Read, mut emit: impl FnMut(String) -> Result<(), String>) -> Result<(), String> {
    use std::io::BufRead;
    let mut reader = std::io::BufReader::new(reader.take(MAX_RESPONSE + 1));
    let mut total = 0u64;
    let mut data = String::new();
    let mut text_len = 0usize;
    let mut nonempty = false;
    let mut stopped = false;
    loop {
        let mut line = String::new();
        let count = reader.read_line(&mut line).map_err(|_| "Could not read the model stream")?;
        total = total.saturating_add(count as u64);
        if total > MAX_RESPONSE {
            return Err("Model response is too large".into());
        }
        if count == 0 {
            return Err("Model stream ended before completion".into());
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if !line.is_empty() {
            if let Some(value) = line.strip_prefix("data:") {
                data.push_str(value.strip_prefix(' ').unwrap_or(value));
                data.push('\n');
            }
            continue;
        }
        if data.is_empty() {
            continue;
        }
        if data.trim() == "[DONE]" {
            return if stopped && nonempty { Ok(()) } else { Err("Model returned empty or incomplete text".into()) };
        }
        let value: Value = serde_json::from_str(&data).map_err(|_| "Model returned invalid streaming JSON")?;
        data.clear();
        if value.get("error").is_some() {
            return Err("Model reported a streaming error".into());
        }
        let choices = value.get("choices").and_then(Value::as_array).ok_or("Model returned no choices")?;
        // Usage-only events may contain no choices.
        for choice in choices {
            if choice.get("index").and_then(Value::as_u64) != Some(0) {
                continue;
            }
            if let Some(content) = choice.get("delta").and_then(|v| v.get("content")).filter(|v| !v.is_null()) {
                let text = content.as_str().ok_or("Model returned invalid text")?;
                if !text.is_empty() {
                    if stopped {
                        return Err("Model sent text after completion".into());
                    }
                    text_len = text_len.saturating_add(text.len());
                    if text_len > wordcraft_engine::cmd::assistant::MAX_TEXT {
                        return Err("Model exceeded the 64 KiB suggestion limit".into());
                    }
                    nonempty |= !text.trim().is_empty();
                    emit(text.to_owned())?;
                }
            }
            if let Some(reason) = choice.get("finish_reason").filter(|v| !v.is_null()) {
                if reason.as_str() != Some("stop") {
                    return Err("Model output was incomplete or filtered. Try a shorter request".into());
                }
                stopped = true;
            }
        }
    }
}

#[cfg(any(test, not(target_arch = "wasm32")))]
fn parse_response(bytes: &[u8]) -> Result<String, String> {
    let value: Value = serde_json::from_slice(bytes).map_err(|_| "Model returned invalid JSON")?;
    let choice = value.get("choices").and_then(|v| v.get(0)).ok_or("Model returned no choices")?;
    if choice.get("finish_reason").and_then(Value::as_str).is_some_and(|r| r != "stop") {
        return Err("Model output was incomplete or filtered. Try a shorter request".into());
    }
    let text = choice.get("message").and_then(|v| v.get("content")).and_then(Value::as_str).ok_or("Model returned no text")?;
    if text.trim().is_empty() || text.len() > wordcraft_engine::cmd::assistant::MAX_TEXT {
        return Err("Model returned empty text or exceeded the 64 KiB suggestion limit".into());
    }
    Ok(text.to_owned())
}

pub fn show(app: &mut WordApp, ctx: &egui::Context) {
    if let Some(rx) = &app.assistant.pending {
        let mut finished = None;
        // Bound work per frame even when a provider produces many tiny deltas.
        for _ in 0..128 {
            match rx.try_recv() {
                Ok(Update::Delta(text)) => {
                    if !app.assistant.cancelled {
                        app.assistant.result.push_str(&text);
                    }
                }
                Ok(Update::Finished(result)) => {
                    finished = Some(result);
                    break;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    finished = Some(Err("Model worker stopped before returning a result".into()));
                    break;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
            }
        }
        if let Some(result) = finished {
            app.assistant.pending = None;
            if !app.assistant.cancelled
                && let Err(error) = result
            {
                app.assistant.error = error;
                app.assistant.request = None;
            }
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }
    if !app.assistant.open {
        return;
    }
    let mut open = true;
    let mut action = None;
    egui::Window::new(tl!("Writing Assistant")).open(&mut open).default_width(430.0).resizable(true).show(ctx, |ui| {
        let state = &mut app.assistant;
        ui.label(tl!("Draft new text, rewrite a selection, or summarize it."));
        #[cfg(target_arch = "wasm32")]
        ui.label(tl!("Model connections require the desktop app."));
        if ui.button(tl!("Model connection")).clicked() {
            action = Some(("file.options", json!({"pane":"assistant"})));
        }
        let model_label = state
            .override_profile
            .and_then(|id| app.ui.assistant_profiles.entries.iter().find(|p| p.id == id))
            .map(Profile::label)
            .unwrap_or_else(|| tl!("Default").into());
        egui::CollapsingHeader::new(format!("{} — {model_label}", tl!("Model"))).id_salt("assistant_model_choice").default_open(false).show(
            ui,
            |ui| {
                ui.add_enabled_ui(state.pending.is_none(), |ui| {
                    let profiles = &app.ui.assistant_profiles;
                    let default_name = profiles.entries.iter().find(|p| p.id == profiles.default_id).map(Profile::label).unwrap_or_default();
                    ui.radio_value(&mut state.override_profile, None, format!("{} — {default_name}", tl!("Default")));
                    egui::ScrollArea::vertical().max_height(160.0).show(ui, |ui| {
                        for profile in &profiles.entries {
                            ui.radio_value(&mut state.override_profile, Some(profile.id), profile.label())
                                .on_hover_text(&profile.connection.endpoint);
                        }
                    });
                });
            },
        );
        ui.horizontal(|ui| {
            for (mode, label) in [("draft", tl!("Draft")), ("rewrite", tl!("Rewrite")), ("summarize", tl!("Summarize"))] {
                ui.selectable_value(&mut state.mode, mode.into(), label);
            }
        });
        ui.label(tl!("Instructions"));
        ui.add(egui::TextEdit::multiline(&mut state.prompt).desired_rows(3).char_limit(8192).desired_width(f32::INFINITY));
        ui.label(tl!("Generate sends your instructions and selected text to the configured endpoint. No other document content is sent."));
        ui.horizontal(|ui| {
            if ui.add_enabled(state.pending.is_none() && !cfg!(target_arch = "wasm32"), egui::Button::new(tl!("Generate"))).clicked() {
                action = Some(("ui.assistant.generate", json!({})));
            }
            if state.pending.is_some() {
                ui.spinner();
                ui.label(if state.cancelled { tl!("Discarded; waiting for request to finish…") } else { tl!("Generating…") });
            }
            if ui.button(tl!("Discard")).clicked() {
                action = Some(("ui.assistant.discard", json!({})));
            }
        });
        if !state.error.is_empty() {
            ui.label(&state.error);
        }
        if !state.result.is_empty() || (state.pending.is_some() && !state.cancelled) {
            ui.separator();
            ui.label(tl!("Preview — review and edit before accepting"));
            egui::ScrollArea::vertical().max_height(240.0).stick_to_bottom(state.pending.is_some()).show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut state.result)
                        .interactive(state.pending.is_none())
                        .desired_rows(8)
                        .desired_width(f32::INFINITY)
                        .char_limit(65536),
                );
            });
            ui.horizontal_wrapped(|ui| {
                let ready = state.pending.is_none() && state.error.is_empty() && state.request.is_some();
                if ui.add_enabled(ready, egui::Button::new(tl!("Insert after selection"))).clicked() {
                    action = Some(("ui.assistant.apply", json!({"placement":"after"})));
                }
                if ui.add_enabled(ready, egui::Button::new(tl!("Replace selection"))).clicked() {
                    action = Some(("ui.assistant.apply", json!({"placement":"replace"})));
                }
                if ui.button(tl!("Copy")).clicked() {
                    ctx.copy_text(state.result.clone());
                }
            });
        }
    });
    app.assistant.open = open && !action.as_ref().is_some_and(|(id, _)| *id == "file.options");
    if let Some((id, params)) = action
        && let Err(e) = app.run(id, params)
    {
        app.assistant.error = e;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_keep_defaults_across_restarts_and_overrides_are_temporary() {
        let mut app = WordApp::new(wordcraft_engine::Session::new(wordcraft_engine::doc::Document::new()), crate::Services::default());
        app.ui.assistant_profiles.entries[0].connection.model = "first".into();
        let id = app.ui.assistant_profiles.add().unwrap();
        app.ui.assistant_profiles.entries[1].connection.model = "second".into();
        app.ui.assistant_profiles.entries[1].connection.key = "second-secret".into();
        app.ui.assistant_profiles.default_id = id;
        app.assistant.override_profile = Some(1);
        assert_eq!(app.ui.assistant_profiles.selected(app.assistant.override_profile).unwrap().model, "first");
        assert_eq!(app.ui.assistant_profiles.selected(None).unwrap().model, "second");
        let saved = serde_json::to_vec(&app.prefs()).unwrap();
        let mut restored = WordApp::new(wordcraft_engine::Session::new(wordcraft_engine::doc::Document::new()), crate::Services::default());
        restored.apply_prefs(crate::UiState::from_json(&saved).unwrap());
        assert!(restored.assistant.override_profile.is_none());
        assert_eq!(restored.ui.assistant_profiles.selected(None).unwrap().model, "second");
        assert!(!crate::control::inspect(&mut restored, &egui::Context::default()).to_string().contains("second-secret"));
        restored.ui.assistant_profiles.remove(id);
        assert_eq!(restored.ui.assistant_profiles.default_id, 1);
        assert!(restored.ui.assistant_profiles.selected(Some(id)).is_err());
        assert!(restored.ui.assistant_profiles.add().unwrap() > id);
        restored.ui.assistant_profiles.remove(1);
        let last = restored.ui.assistant_profiles.default_id;
        restored.ui.assistant_profiles.remove(last);
        assert!(restored.ui.assistant_profiles.selected(None).is_err());
        let new = restored.ui.assistant_profiles.add().unwrap();
        assert_eq!(restored.ui.assistant_profiles.default_id, new);
    }

    #[test]
    fn profile_loading_is_bounded_and_preserves_valid_settings() {
        let raw = json!({"assistantProfiles":{"entries":[
            {"id":1,"name":"One","connection":{"model":"first","key":"key1"}},
            {"id":2,"name":"Two","connection":{"model":false,"key":"key2"}},
            {"id":1,"name":"Duplicate"}
        ],"defaultId":999},"author":"Writer"});
        let mut ui = crate::UiState::from_json(&serde_json::to_vec(&raw).unwrap()).unwrap();
        ui.assistant_profiles.sanitize();
        assert_eq!(ui.assistant_profiles.entries.len(), 2);
        assert_eq!(ui.assistant_profiles.default_id, 1);
        assert_eq!(ui.assistant_profiles.entries[1].connection.key, "key2");
        assert!(ui.assistant_profiles.entries[1].connection.model.is_empty());
        assert_eq!(ui.author, "Writer");
        while ui.assistant_profiles.entries.len() < MAX_PROFILES {
            ui.assistant_profiles.add().unwrap();
        }
        assert!(ui.assistant_profiles.add().is_err());
        ui.assistant_profiles.remove(1);
        ui.assistant_profiles.next_id = u64::MAX;
        assert!(ui.assistant_profiles.add().is_err());
        // A new-format empty list wins over any stale legacy connection.
        let empty = crate::UiState::from_json(br#"{"assistantProfiles":{"entries":[]},"assistantConnection":{"key":"old"}}"#).unwrap();
        assert!(empty.assistant_profiles.entries.is_empty());
    }

    #[test]
    fn model_override_is_collapsed_and_does_not_change_default() {
        use egui_kittest::kittest::Queryable;
        let mut app = WordApp::new(wordcraft_engine::Session::new(wordcraft_engine::doc::Document::new()), crate::Services::default());
        app.ui.assistant_profiles.entries[0].name = "Primary".into();
        let second = app.ui.assistant_profiles.add().unwrap();
        app.ui.assistant_profiles.entries[1].name = "Alternative".into();
        app.assistant.open = true;
        let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1000.0, 800.0)).build_ui_state(
            |ui, app: &mut WordApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            app,
        );
        h.run_steps(4);
        assert!(h.query_by_label("Alternative").is_none());
        h.get_by_label("Model — Default").click();
        h.run_steps(4);
        h.get_by_label("Alternative").click();
        h.run_steps(4);
        assert_eq!(h.state().assistant.override_profile, Some(second));
        assert_eq!(h.state().ui.assistant_profiles.default_id, 1);
        h.get_by_label("Default — Primary").click();
        h.run_steps(4);
        assert!(h.state().assistant.override_profile.is_none());
        h.get_by_label("Model connection").click();
        h.run_steps(4);
        h.get_by_label("Add").click();
        h.run_steps(4);
        assert_eq!(h.state().ui.assistant_profiles.entries.len(), 3);
        let added = h.state().assistant.edit_profile;
        h.get_by_label("Default").click();
        h.run_steps(4);
        assert_eq!(h.state().ui.assistant_profiles.default_id, added);
        h.get_by_label("Remove").click();
        h.run_steps(4);
        assert_eq!(h.state().ui.assistant_profiles.default_id, 1);
    }

    #[test]
    fn connection_preferences_survive_restart_without_saving_writing_content() {
        let mut app = WordApp::new(wordcraft_engine::Session::new(wordcraft_engine::doc::Document::new()), crate::Services::default());
        app.ui.assistant_profiles.entries[0].connection = Connection {
            endpoint: "https://models.example.com/v1/chat/completions".into(),
            model: "my-model".into(),
            key: "test-persisted-key".into(),
        };
        app.assistant.prompt = "private instructions".into();
        app.assistant.result = "private draft".into();
        let saved = serde_json::to_vec(&app.prefs()).unwrap();
        let json: Value = serde_json::from_slice(&saved).unwrap();
        assert_eq!(json["assistantProfiles"]["entries"][0]["connection"]["key"], "test-persisted-key");
        assert!(!String::from_utf8_lossy(&saved).contains("private"));
        let mut restored = WordApp::new(wordcraft_engine::Session::new(wordcraft_engine::doc::Document::new()), crate::Services::default());
        restored.apply_prefs(crate::UiState::from_json(&saved).unwrap());
        assert_eq!(restored.ui.assistant_profiles.entries[0].connection.endpoint, app.ui.assistant_profiles.entries[0].connection.endpoint);
        assert_eq!(restored.ui.assistant_profiles.entries[0].connection.model, "my-model");
        assert_eq!(restored.ui.assistant_profiles.entries[0].connection.key, "test-persisted-key");
        assert!(!format!("{:?}", restored.prefs()).contains("test-persisted-key"));
        let inspection = crate::control::inspect(&mut restored, &egui::Context::default());
        assert!(!inspection.to_string().contains("test-persisted-key"));
        assert_eq!(restored.ui.assistant_profiles.entries[0].connection.key, "test-persisted-key");
        // Clearing a key is saved too; an old key must not come back on restart.
        restored.ui.assistant_profiles.entries[0].connection.key.clear();
        let cleared = serde_json::to_vec(&restored.prefs()).unwrap();
        assert!(crate::UiState::from_json(&cleared).unwrap().assistant_profiles.entries[0].connection.key.is_empty());
    }

    #[test]
    fn old_and_malformed_preferences_keep_valid_fields() {
        let old = crate::UiState::from_json(br#"{"author":"Writer"}"#).unwrap();
        assert!(old.assistant_profiles.entries[0].connection.endpoint.is_empty());
        assert_eq!(old.author, "Writer");
        let saved = crate::UiState::from_json(
            br#"{"assistantConnection":{"endpoint":"https://example.com/v1/chat/completions","model":42,"key":"saved-key"},"author":"Writer"}"#,
        )
        .unwrap();
        assert_eq!(saved.assistant_profiles.entries[0].connection.endpoint, "https://example.com/v1/chat/completions");
        assert!(saved.assistant_profiles.entries[0].connection.model.is_empty());
        assert_eq!(saved.assistant_profiles.entries[0].connection.key, "saved-key");
        assert_eq!(saved.author, "Writer");
    }

    fn event(text: &str, reason: Value) -> String {
        format!("data: {}\r\n\r\n", json!({"choices":[{"index":0,"delta":{"content":text},"finish_reason":reason}]}))
    }

    #[test]
    fn streaming_preview_updates_before_completion_and_guards_acceptance() {
        let mut app = WordApp::new(wordcraft_engine::Session::new(wordcraft_engine::doc::Document::new()), crate::Services::default());
        let request = app.run("assistant.prepare", json!({"mode":"draft","prompt":"A greeting"})).unwrap();
        app.assistant.request = Some(request);
        let (tx, rx) = std::sync::mpsc::channel();
        app.assistant.pending = Some(rx);
        tx.send(Update::Delta("Hello".into())).unwrap();
        show(&mut app, &egui::Context::default());
        assert_eq!(app.assistant.result, "Hello");
        assert!(app.assistant.pending.is_some());
        assert!(command(&mut app, "ui.assistant.apply", &json!({"placement":"after"})).is_err());
        assert!(!app.session.dirty);
        tx.send(Update::Delta(" world".into())).unwrap();
        tx.send(Update::Finished(Ok(()))).unwrap();
        show(&mut app, &egui::Context::default());
        assert_eq!(app.assistant.result, "Hello world");
        command(&mut app, "ui.assistant.apply", &json!({"placement":"after"})).unwrap();
    }

    #[test]
    fn interrupted_stream_keeps_preview_but_cannot_be_accepted() {
        for explicit_error in [true, false] {
            let mut app = WordApp::new(wordcraft_engine::Session::new(wordcraft_engine::doc::Document::new()), crate::Services::default());
            app.assistant.request = Some(json!({}));
            let (tx, rx) = std::sync::mpsc::channel();
            app.assistant.pending = Some(rx);
            tx.send(Update::Delta("Partial".into())).unwrap();
            if explicit_error {
                tx.send(Update::Finished(Err("Disconnected".into()))).unwrap();
            }
            drop(tx);
            show(&mut app, &egui::Context::default());
            assert_eq!(app.assistant.result, "Partial");
            assert!(!app.assistant.error.is_empty());
            assert!(app.assistant.request.is_none());
            assert!(command(&mut app, "ui.assistant.apply", &json!({"placement":"after"})).is_err());
        }
    }

    #[test]
    fn stream_handles_fragmented_utf8_comments_multiline_and_usage_events() {
        // A one-byte reader splits both JSON and multibyte text across reads.
        struct Fragmented(std::io::Cursor<Vec<u8>>);
        impl std::io::Read for Fragmented {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                let len = buf.len().min(1);
                std::io::Read::read(&mut self.0, &mut buf[..len])
            }
        }
        let wire = format!(
            ": keepalive\r\n\r\ndata: {{\ndata: \"choices\":[]}}\n\n{}{}data: {{\"choices\":[],\"usage\":{{}}}}\n\ndata: [DONE]\n\n",
            event("Café \u{1f642}", Value::Null),
            event("!", json!("stop"))
        );
        let mut chunks = Vec::new();
        parse_stream(Fragmented(std::io::Cursor::new(wire.into_bytes())), |text| {
            chunks.push(text);
            Ok(())
        })
        .unwrap();
        assert_eq!(chunks, ["Café \u{1f642}", "!"]);
    }

    #[test]
    fn stream_rejects_incomplete_invalid_filtered_and_oversized_output() {
        for wire in [
            event("partial", Value::Null),
            format!("{}data: [DONE]\n\n", event("partial", Value::Null)),
            format!("{}data: [DONE]\n\n", event("", json!("stop"))),
            event("partial", json!("length")),
            event("partial", json!("content_filter")),
            "data: not JSON\n\n".into(),
            "data: {\"error\":{\"message\":\"secret\"}}\n\n".into(),
            event(&"x".repeat(wordcraft_engine::cmd::assistant::MAX_TEXT + 1), Value::Null),
            "x".repeat(MAX_RESPONSE as usize + 1),
        ] {
            assert!(parse_stream(wire.as_bytes(), |_| Ok(())).is_err());
        }
    }

    #[test]
    fn discard_ignores_late_results_and_does_not_allow_overlapping_requests() {
        let mut app = WordApp::new(wordcraft_engine::Session::new(wordcraft_engine::doc::Document::new()), crate::Services::default());
        let (tx, rx) = std::sync::mpsc::channel();
        app.assistant.pending = Some(rx);
        command(&mut app, "ui.assistant.discard", &json!({})).unwrap();
        assert!(command(&mut app, "ui.assistant.generate", &json!({})).is_err());
        tx.send(Update::Delta("Late result".into())).unwrap();
        tx.send(Update::Finished(Ok(()))).unwrap();
        show(&mut app, &egui::Context::default());
        assert!(app.assistant.pending.is_none());
        assert!(app.assistant.result.is_empty());
        assert!(!app.session.dirty);
    }

    #[test]
    fn preview_acceptance_uses_the_engine_and_cannot_be_repeated() {
        let mut app = WordApp::new(wordcraft_engine::Session::new(wordcraft_engine::doc::Document::new()), crate::Services::default());
        app.run("assistant.open", json!({})).unwrap();
        assert!(app.assistant.open);
        let request = app.run("assistant.prepare", json!({"mode":"draft","prompt":"A greeting"})).unwrap();
        app.assistant.request = Some(request);
        app.assistant.result = "Hello".into();
        command(&mut app, "ui.assistant.apply", &json!({"placement":"after"})).unwrap();
        assert!(app.assistant.result.is_empty());
        assert!(command(&mut app, "ui.assistant.apply", &json!({"placement":"after"})).is_err());
        app.run("edit.undo", json!({})).unwrap();
        assert!(app.session.doc.plain_text(wordcraft_engine::doc::StoryRef::Body).trim().is_empty());
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn local_provider_receives_messages_and_returns_preview_without_editing() {
        local_provider(false);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn local_provider_stream_is_visible_before_server_finishes() {
        local_provider(true);
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn local_provider(streaming: bool) {
        use std::io::{BufRead, Read, Write};
        let (ack_tx, ack_rx) = std::sync::mpsc::channel();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            let (stream, _) = loop {
                match listener.accept() {
                    Ok(connection) => break connection,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && std::time::Instant::now() < deadline => {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    Err(e) => panic!("mock provider: {e}"),
                }
            };
            stream.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
            let mut reader = std::io::BufReader::new(stream);
            let mut headers = String::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" || line.is_empty() {
                    break;
                }
                headers.push_str(&line);
                assert!(headers.len() < 16384);
            }
            assert!(headers.starts_with("POST /v1/chat/completions "));
            assert!(headers.to_ascii_lowercase().contains("authorization: bearer test-key"));
            let len: usize = headers
                .lines()
                .find_map(|line| line.to_ascii_lowercase().strip_prefix("content-length: ").map(str::to_owned))
                .unwrap()
                .trim()
                .parse()
                .unwrap();
            assert!(len < 65536);
            let mut body = vec![0; len];
            reader.read_exact(&mut body).unwrap();
            let body: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(body["model"], "test-model");
            assert_eq!(body["stream"], true);
            assert!(body["messages"].to_string().contains("A greeting"));
            if streaming {
                write!(
                    reader.get_mut(),
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream; charset=utf-8\r\nConnection: close\r\n\r\n{}",
                    event("Hello", Value::Null)
                )
                .unwrap();
                reader.get_mut().flush().unwrap();
                // The client must receive the first chunk while the response is still open.
                ack_rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
                write!(reader.get_mut(), "{}data: [DONE]\n\n", event(" from the model", json!("stop"))).unwrap();
                return;
            }
            let response = r#"{"choices":[{"finish_reason":"stop","message":{"content":"Hello from the model"}}]}"#;
            write!(
                reader.get_mut(),
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response.len(),
                response
            )
            .unwrap();
        });
        let mut app = WordApp::new(wordcraft_engine::Session::new(wordcraft_engine::doc::Document::new()), crate::Services::default());
        app.ui.assistant_profiles.entries[0].connection.endpoint = format!("http://{address}/v1/chat/completions");
        app.ui.assistant_profiles.entries[0].connection.model = "test-model".into();
        app.ui.assistant_profiles.entries[0].connection.key = "test-key".into();
        let connection = app.ui.assistant_profiles.entries[0].connection.clone();
        let second = app.ui.assistant_profiles.add().unwrap();
        app.ui.assistant_profiles.entries[1].connection = connection;
        app.ui.assistant_profiles.entries[0].connection.endpoint = "http://127.0.0.1:1/v1/chat/completions".into();
        app.ui.assistant_profiles.entries[0].connection.key = "wrong-key".into();
        if streaming {
            app.ui.assistant_profiles.default_id = second;
        }
        let saved = serde_json::to_vec(&app.prefs()).unwrap();
        let mut app = WordApp::new(wordcraft_engine::Session::new(wordcraft_engine::doc::Document::new()), crate::Services::default());
        app.apply_prefs(crate::UiState::from_json(&saved).unwrap());
        if !streaming {
            app.assistant.override_profile = Some(second);
        }
        app.assistant.prompt = "A greeting".into();
        command(&mut app, "ui.assistant.generate", &json!({})).unwrap();
        let rx = app.assistant.pending.take().unwrap();
        let first = rx.recv_timeout(std::time::Duration::from_secs(15)).unwrap();
        if streaming {
            assert!(matches!(first, Update::Delta(text) if text == "Hello"));
            ack_tx.send(()).unwrap();
            assert!(matches!(rx.recv_timeout(std::time::Duration::from_secs(15)).unwrap(), Update::Delta(text) if text == " from the model"));
        } else {
            assert!(matches!(first, Update::Delta(text) if text == "Hello from the model"));
        }
        assert!(matches!(rx.recv_timeout(std::time::Duration::from_secs(15)).unwrap(), Update::Finished(Ok(()))));
        assert!(!app.session.dirty);
        server.join().unwrap();
    }
    #[test]
    fn malformed_filtered_and_empty_responses_are_errors() {
        for bytes in
            [b"not json".as_slice(), br#"{}"#, br#"{"choices":[]}"#, br#"{"choices":[{"finish_reason":"length","message":{"content":"partial"}}]}"#]
        {
            assert!(parse_response(bytes).is_err());
        }
        assert_eq!(parse_response(br#"{"choices":[{"finish_reason":"stop","message":{"content":"Hello"}}]}"#).unwrap(), "Hello");
    }
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn endpoints_require_tls_except_loopback_and_no_embedded_secrets() {
        assert!(endpoint("http://localhost:11434/v1/chat/completions").is_ok());
        assert!(endpoint("https://example.com/v1/chat/completions").is_ok());
        for url in ["http://example.com/v1", "https://key@example.com/v1", "https://example.com/v1?key=secret", "file:///etc/passwd"] {
            assert!(endpoint(url).is_err());
        }
    }
}
