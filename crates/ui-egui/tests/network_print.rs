//! The Print page's network destination: nothing is sent without a query and the confirmation, the
//! job carries the engine's print-ready PDF, and its outcome reaches the status bar naming the
//! printer. The service is an in-process fake: no socket, no printer.
use std::cell::RefCell;
use std::rc::Rc;

use craft_ui::capability::Capability;
use craft_ui::printing::network as net;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::json;
use wordcraft_engine::Session;
use wordcraft_ui_egui::network_print::{FORMATS, NetworkPrint};
use wordcraft_ui_egui::{Services, WordApp};

const ADDRESS: &str = "http://127.0.0.1:5357/print";

#[derive(Default)]
struct Seen {
    queries: Vec<String>,
    /// (name, bytes, confirmed) of every worker the app asked for.
    workers: Vec<(String, Vec<u8>, bool)>,
    /// (address, request) of every job started.
    jobs: Vec<(String, net::Request)>,
}

fn service(seen: &Rc<RefCell<Seen>>, formats: &'static [&'static str], outcome: net::Outcome) -> NetworkPrint {
    let (query_seen, send_seen) = (seen.clone(), seen.clone());
    NetworkPrint {
        validate: |text| if text.contains("127.0.0.1") { Ok(()) } else { Err(net::Failure::new(net::FailureKind::Policy, "refused")) },
        query: Box::new(move |address| {
            query_seen.borrow_mut().queries.push(address.to_owned());
            let (tx, rx) = std::sync::mpsc::channel();
            let caps = net::Caps {
                formats: formats.iter().map(|f| (*f).to_owned()).collect(),
                sides: Capability::Unknown,
                copies: Capability::from_option_range(Some((1, 9))),
                ..Default::default()
            };
            tx.send(Ok(net::PrinterSummary { name: "Test Laser".into(), state: "Idle".into(), caps, ..Default::default() })).unwrap();
            rx
        }),
        send: Box::new(move |name, pdf, confirmed| {
            send_seen.borrow_mut().workers.push((name, pdf.to_vec(), confirmed));
            let seen = send_seen.clone();
            Some(Box::new(move |address: &str, request: net::Request| {
                seen.borrow_mut().jobs.push((address.to_owned(), request));
                let (tx, rx) = std::sync::mpsc::channel();
                tx.send(Ok(net::Report { outcome, printer: "Test Laser".into(), sheets: 1, notes: Vec::new() })).unwrap();
                net::JobHandle::new(rx, Box::new(|| {}))
            }))
        }),
    }
}

fn app_with(service: NetworkPrint) -> WordApp {
    let mut app = WordApp::new(Session::new(wordcraft_doc::Document::new()), Services::default());
    app.run("text.insert", json!({"text": "A letter to print over the network"})).unwrap();
    app.services.print_network = Some(service);
    app.ui.print.network = true;
    app
}

/// Query the printer the way the Print page's button does.
fn query(app: &mut WordApp) {
    app.net_print.draft.url = ADDRESS.into();
    let service = app.services.print_network.as_ref().unwrap();
    assert!(app.net_print.start_query(&service.query, (service.validate)(ADDRESS).is_ok()));
    app.net_print.poll(&egui::Context::default());
    assert!(app.net_print.draft.printer().is_some(), "the answer is in");
}

#[test]
fn nothing_is_sent_before_a_query_and_the_confirmation() {
    let seen = Rc::new(RefCell::new(Seen::default()));
    let mut app = app_with(service(&seen, FORMATS, net::Outcome::Completed));
    app.net_print.draft.url = ADDRESS.into();
    assert!(app.run("ui.print", json!({})).is_err(), "not queried");
    query(&mut app);
    assert!(app.run("ui.print", json!({})).is_err(), "not confirmed");
    assert!(seen.borrow().jobs.is_empty(), "no job started");
    assert!(!app.net_print.draft.running());
}

#[test]
fn a_confirmed_job_carries_the_pdf_one_copy_and_the_printers_own_settings() {
    let seen = Rc::new(RefCell::new(Seen::default()));
    let mut app = app_with(service(&seen, FORMATS, net::Outcome::Completed));
    query(&mut app);
    app.net_print.draft.confirmed = true;
    app.net_print.draft.copies = 3;
    let reply = app.run("ui.print", json!({"copies": 5})).expect("the job starts");
    assert_eq!(reply["network"], true);
    assert_eq!(reply["pending"], true);
    assert!(!app.net_print.draft.confirmed, "the confirmation is spent by the job");
    {
        let seen = seen.borrow();
        let (name, pdf, confirmed) = &seen.workers[0];
        assert!(pdf.starts_with(b"%PDF"), "the engine's print-ready PDF");
        assert!(*confirmed && !name.is_empty());
        let (address, request) = &seen.jobs[0];
        assert_eq!(address, ADDRESS);
        assert_eq!(request.copies, Some(3), "copies are the printer section's, not baked into the PDF");
        assert_eq!(request.expected_pages, Some(1));
    }
    assert!(app.run("ui.print", json!({})).is_err(), "a second job needs a new confirmation");
    app.net_print.poll(&egui::Context::default());
    assert!(matches!(app.net_print.draft.job, net::JobView::Finished(ref report) if report.printer == "Test Laser"));
}

#[test]
fn a_printer_that_lists_no_pdf_blocks_print() {
    let seen = Rc::new(RefCell::new(Seen::default()));
    let mut app = app_with(service(&seen, &["image/pwg-raster"], net::Outcome::Completed));
    query(&mut app);
    app.net_print.draft.confirmed = true;
    assert_eq!(app.net_print.draft.block(true, true, FORMATS), Some(net::Block::Format));
    assert!(app.run("ui.print", json!({})).is_err());
    assert!(seen.borrow().jobs.is_empty());
}

#[test]
fn without_the_service_there_is_no_network_destination() {
    let mut app = WordApp::new(Session::new(wordcraft_doc::Document::new()), Services::default());
    app.ui.print.network = true;
    let commands = wordcraft_ui_egui::printing::commands(&app);
    let submit = commands.iter().find(|c| c["id"] == "ui.printSubmit").unwrap();
    assert_eq!(submit["enabled"], false, "no service, no hook: nothing to print to");
}

#[test]
fn a_job_that_finished_while_the_page_was_closed_is_announced_naming_the_printer() {
    let seen = Rc::new(RefCell::new(Seen::default()));
    let mut app = app_with(service(&seen, FORMATS, net::Outcome::CompletedWithWarnings));
    query(&mut app);
    app.net_print.draft.confirmed = true;
    app.run("ui.print", json!({})).expect("starts");
    assert!(!app.ui.backstage, "the Print page is not open");
    app.poll_printing(&egui::Context::default());
    let status = app.status_msg.as_ref().map(|(text, _)| text.clone()).unwrap_or_default();
    assert!(status.contains("warnings"), "the outcome reaches the status bar: {status:?}");
}

const CONFIRM: &str = "I confirm sending this document to this printer";

/// The app in a harness with the Print page open and the printer already queried.
fn print_page(seen: &Rc<RefCell<Seen>>) -> egui_kittest::Harness<'static, WordApp> {
    let mut app = app_with(service(seen, FORMATS, net::Outcome::Completed));
    query(&mut app);
    app.run("ui.backstage", json!({"value": true, "page": "print"})).unwrap();
    let mut harness = egui_kittest::Harness::builder().with_size(egui::vec2(1440.0, 900.0)).build_ui_state(
        |ui, app: &mut WordApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    harness.run_steps(4);
    harness
}

#[test]
fn ticking_the_confirmation_box_and_clicking_print_starts_one_job() {
    let seen = Rc::new(RefCell::new(Seen::default()));
    let mut h = print_page(&seen);
    assert!(!h.state().net_print.draft.confirmed, "never ticked by default");
    h.get_by_label(CONFIRM).click();
    h.run_steps(4);
    assert!(h.state().net_print.draft.confirmed, "the checkbox is the control that confirms");
    assert!(seen.borrow().jobs.is_empty(), "ticking sends nothing");
    // The File menu has a "Print" entry too; the page's button is the wide one.
    let print = h
        .get_all_by_role_and_label(egui::accesskit::Role::Button, "Print")
        .find(|node| node.rect().width() >= 300.0)
        .expect("the Print page's button");
    assert!(!print.accesskit_node().is_disabled(), "ticked and queried: Print is enabled");
    print.click();
    h.run_steps(4);
    assert_eq!(seen.borrow().jobs.len(), 1, "one job started");
    assert_eq!(seen.borrow().workers.len(), 1);
    assert!(seen.borrow().workers[0].2, "the worker was given the confirmation");
    assert!(!h.state().net_print.draft.confirmed, "the job spent it");
}

#[test]
fn closing_and_reopening_the_print_page_clears_the_confirmation() {
    let seen = Rc::new(RefCell::new(Seen::default()));
    let mut h = print_page(&seen);
    h.get_by_label(CONFIRM).click();
    h.run_steps(4);
    assert!(h.state().net_print.draft.confirmed);
    h.state_mut().run("ui.backstage", json!({"value": false})).unwrap();
    h.run_steps(6);
    assert!(h.state().net_print.draft.confirmed, "closing alone does not decide; reopening does");
    h.state_mut().run("ui.backstage", json!({"value": true, "page": "print"})).unwrap();
    h.run_steps(4);
    assert!(!h.state().net_print.draft.confirmed, "a reopened page starts unconfirmed");
    assert!(h.state().net_print.draft.printer().is_some(), "the queried printer is still there");
    assert!(seen.borrow().jobs.is_empty(), "nothing was submitted");
}
