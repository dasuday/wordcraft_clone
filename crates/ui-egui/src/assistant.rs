//! In-app writing assistant. Only reviewed plain text enters the engine; models get no tools.
use crate::WordApp;
use serde_json::{Value, json};
use zeroize::Zeroize;

pub struct State {
    pub open: bool,
    endpoint: String,
    model: String,
    key: String,
    mode: String,
    prompt: String,
    request: Option<Value>,
    result: String,
    error: String,
    cancelled: bool,
    pending: Option<std::sync::mpsc::Receiver<Result<String, String>>>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            open: false,
            endpoint: String::new(),
            model: String::new(),
            key: String::new(),
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

impl Drop for State {
    fn drop(&mut self) {
        self.key.zeroize();
    }
}

/// UI commands are also available through the control channel. Configuration, including the
/// password field, stays in memory and is never included in preferences or document saves.
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
            let rx = start(&app.assistant, &request)?;
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
fn start(_: &State, _: &Value) -> Result<std::sync::mpsc::Receiver<Result<String, String>>, String> {
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
fn start(state: &State, request: &Value) -> Result<std::sync::mpsc::Receiver<Result<String, String>>, String> {
    let url = endpoint(&state.endpoint)?;
    if state.model.trim().is_empty() || state.model.len() > 256 {
        return Err("Enter a model name (up to 256 bytes)".into());
    }
    if state.key.len() > 8192 {
        return Err("API key is too long".into());
    }
    let body = json!({"model": state.model.trim(), "messages":request.get("messages"), "stream":false});
    let key = zeroize::Zeroizing::new(state.key.clone());
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("writing-assistant".into())
        .spawn(move || {
            let _ = tx.send(generate(url, key, body));
        })
        .map_err(|_| "Could not start the writing assistant")?;
    Ok(rx)
}

#[cfg(not(target_arch = "wasm32"))]
fn generate(url: reqwest::Url, key: zeroize::Zeroizing<String>, body: Value) -> Result<String, String> {
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
    // Bound the actual read, not just Content-Length. Never expose provider error bodies or keys.
    const MAX_RESPONSE: u64 = 1024 * 1024;
    let mut bytes = Vec::new();
    response.take(MAX_RESPONSE + 1).read_to_end(&mut bytes).map_err(|_| "Could not read the model response")?;
    if bytes.len() as u64 > MAX_RESPONSE {
        return Err("Model response is too large".into());
    }
    parse_response(&bytes)
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
        let result = match rx.try_recv() {
            Ok(r) => Some(r),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Some(Err("Model worker stopped before returning a result".into())),
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
        };
        if let Some(result) = result {
            app.assistant.pending = None;
            if !app.assistant.cancelled {
                match result {
                    Ok(text) => app.assistant.result = text,
                    Err(error) => app.assistant.error = error,
                }
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
        egui::CollapsingHeader::new(tl!("Model connection")).default_open(state.model.is_empty()).show(ui, |ui| {
            ui.label(tl!("OpenAI-compatible chat completions URL"));
            ui.add(
                egui::TextEdit::singleline(&mut state.endpoint)
                    .hint_text(tl!("https://your-server/v1/chat/completions"))
                    .char_limit(2048)
                    .desired_width(f32::INFINITY),
            );
            ui.label(tl!("Model name"));
            ui.add(egui::TextEdit::singleline(&mut state.model).char_limit(256));
            ui.label(tl!("API key (if required by your server; kept only in memory)"));
            ui.add(egui::TextEdit::singleline(&mut state.key).password(true).char_limit(8192));
        });
        ui.horizontal(|ui| {
            for (mode, label) in [("draft", tl!("Draft")), ("rewrite", tl!("Rewrite")), ("summarize", tl!("Summarize"))] {
                ui.selectable_value(&mut state.mode, mode.into(), label);
            }
        });
        ui.label(tl!("Instructions"));
        ui.add(egui::TextEdit::multiline(&mut state.prompt).desired_rows(3).char_limit(8192).desired_width(f32::INFINITY));
        ui.label(tl!("Generate sends your instructions and selected text to the endpoint above. No other document content is sent."));
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
        if !state.result.is_empty() {
            ui.separator();
            ui.label(tl!("Preview — review and edit before accepting"));
            egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                ui.add(egui::TextEdit::multiline(&mut state.result).desired_rows(8).desired_width(f32::INFINITY).char_limit(65536));
            });
            ui.horizontal_wrapped(|ui| {
                if ui.button(tl!("Insert after selection")).clicked() {
                    action = Some(("ui.assistant.apply", json!({"placement":"after"})));
                }
                if ui.button(tl!("Replace selection")).clicked() {
                    action = Some(("ui.assistant.apply", json!({"placement":"replace"})));
                }
                if ui.button(tl!("Copy")).clicked() {
                    ctx.copy_text(state.result.clone());
                }
            });
        }
    });
    app.assistant.open = open;
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
    fn discard_ignores_late_results_and_does_not_allow_overlapping_requests() {
        let mut app = WordApp::new(wordcraft_engine::Session::new(wordcraft_engine::doc::Document::new()), crate::Services::default());
        let (tx, rx) = std::sync::mpsc::channel();
        app.assistant.pending = Some(rx);
        command(&mut app, "ui.assistant.discard", &json!({})).unwrap();
        assert!(command(&mut app, "ui.assistant.generate", &json!({})).is_err());
        tx.send(Ok("Late result".into())).unwrap();
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
        use std::io::{BufRead, Read, Write};
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
            assert_eq!(body["stream"], false);
            assert!(body["messages"].to_string().contains("A greeting"));
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
        app.assistant.endpoint = format!("http://{address}/v1/chat/completions");
        app.assistant.model = "test-model".into();
        app.assistant.key = "test-key".into();
        app.assistant.prompt = "A greeting".into();
        command(&mut app, "ui.assistant.generate", &json!({})).unwrap();
        let rx = app.assistant.pending.take().unwrap();
        assert_eq!(rx.recv_timeout(std::time::Duration::from_secs(15)).unwrap().unwrap(), "Hello from the model");
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
