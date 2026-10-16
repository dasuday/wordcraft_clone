//! TEMPORARY bug probe: measures UI-thread frame stalls caused by AutoSave while typing.
//! `cargo run --release -p wordcraft-ui-egui --example autosave_probe -- doc.docx`
//!
//! Simulates: open a saved document, save it (so AutoSave covers it), type a burst, pause
//! more than 1.5 s (AutoSave fires), repeat. The "autosave frame" is the frame in which the
//! document's dirty flag clears — with an inline write it carries the whole serialize+fsync;
//! with the background writer it only stamps and clones. Ends by checking the typed text
//! reached disk.

use std::time::Instant;

use serde_json::json;
use wordcraft_engine::Session;
use wordcraft_ui_egui::{Services, WordApp};

static READY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn step(h: &mut egui_kittest::Harness<'_, WordApp>) -> f64 {
    let mut raw = std::mem::take(h.input_mut());
    h.state_mut().raw_input_hook(&mut raw);
    *h.input_mut() = raw;
    let t = Instant::now();
    h.step();
    t.elapsed().as_secs_f64() * 1000.0
}

fn stats(v: &[f64]) -> serde_json::Value {
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.total_cmp(b));
    let at = |q: f64| s.get(((s.len() as f64 - 1.0) * q).round() as usize).copied().unwrap_or(0.0);
    json!({"n": s.len(), "med": (at(0.5) * 10.0).round() / 10.0, "p95": (at(0.95) * 10.0).round() / 10.0, "max": (at(1.0) * 10.0).round() / 10.0})
}

/// Types a burst, then idle-steps `idle_ms` of wall time, returning frame stats and the
/// AutoSave frame's time (the frame where dirty flipped false).
fn burst(h: &mut egui_kittest::Harness<'_, WordApp>, chars: usize, idle_ms: f64) -> serde_json::Value {
    let mut frames: Vec<f64> = Vec::new();
    let mut autosave_frame: Option<f64> = None;
    let t_end = Instant::now() + std::time::Duration::from_secs_f64(idle_ms / 1000.0);
    let mut typed = 0;
    while Instant::now() < t_end {
        let was_dirty = h.state().session.dirty;
        if typed < chars {
            h.input_mut().events.push(egui::Event::Text("x".into()));
            typed += 1;
        }
        let f = step(h);
        frames.push(f);
        if was_dirty && !h.state().session.dirty && autosave_frame.is_none() {
            autosave_frame = Some(f);
        }
    }
    json!({"frames": stats(&frames), "autosave_frame_ms": autosave_frame.map(|f| (f * 10.0).round() / 10.0)})
}

fn main() {
    let path = std::env::args().nth(1).expect("usage: autosave_probe doc.docx");
    let mut session = Session::new(wordcraft_doc::Document::new());
    session.run("file.open", &json!({"path": path})).expect("open");
    let app = WordApp::new(session, Services::default());
    let mut h =
        egui_kittest::Harness::builder().with_size(egui::vec2(1440.0, 900.0)).with_pixels_per_point(2.0).with_max_steps(1_000_000).build_ui_state(
            |ui, app: &mut WordApp| {
                if !READY.load(std::sync::atomic::Ordering::Relaxed) {
                    return;
                }
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            app,
        );
    READY.store(true, std::sync::atomic::Ordering::Relaxed);
    for _ in 0..6 {
        step(&mut h);
    }

    // Save once so AutoSave is eligible (path set in this session).
    let t = Instant::now();
    let r = h.state_mut().execute("file.save", json!({"path": path}));
    let save_alone_ms = t.elapsed().as_secs_f64() * 1000.0;
    assert!(r.is_ok(), "save failed: {r:?}");

    // Typing baseline: 30 chars, one per frame.
    h.state_mut().canvas.want_focus = true;
    h.state_mut().session.run("caret.docStart", &json!({})).ok();
    for _ in 0..3 {
        step(&mut h);
    }
    let mut typing_frames = Vec::new();
    for _ in 0..30 {
        h.input_mut().events.push(egui::Event::Text("a".into()));
        typing_frames.push(step(&mut h));
    }
    let out = json!({
        "save_alone_ms": (save_alone_ms * 10.0).round() / 10.0,
        "typing_frames": stats(&typing_frames),
        "burst1": burst(&mut h, 20, 8000.0),
        "burst2": burst(&mut h, 20, 8000.0),
        "burst3": burst(&mut h, 20, 8000.0),
    });
    // The background writes must have landed on disk.
    h.state_mut().finish_pending_autosave();
    let saved = wordcraft_engine::io::open_path(std::path::Path::new(&path)).unwrap();
    let text = saved.plain_text(wordcraft_doc::StoryRef::Body);
    let xs = text.matches('x').count();
    println!("{out}");
    println!("{{\"typed_x_on_disk\": {xs}}}");
}
