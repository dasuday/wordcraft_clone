use serde_json::json;
use wordcraft_doc::{Document, StoryRef, section::NumFormat};
use wordcraft_engine::Session;
use wordcraft_formats::{
    model::{self, FBlock, Flow, Fmt, FmtChange, ListInfo, Para, RevMark},
    rtf,
};

fn changed_list(old: Option<bool>, new: Option<bool>, level: u8) -> Document {
    let list = |ordered| ListInfo { ordered, level };
    let mut item = Para {
        list: new.map(list),
        fmt_change: Some(Box::new(FmtChange {
            mark: RevMark { author: "Alice".into(), date: "2024-03-15T10:30:00Z".into(), id: None },
            old: Para { list: old.map(list), ..Default::default() },
        })),
        ..Default::default()
    };
    item.push_text("Item", &Fmt::default());
    let mut next = Para { list: new.map(list), ..Default::default() };
    next.push_text("Next", &Fmt::default());
    model::to_doc(&Flow { blocks: vec![FBlock::Para(item), FBlock::Para(next)], ..Default::default() })
}

fn lists(doc: &Document) -> Vec<Option<(bool, u8)>> {
    doc.para_paths(StoryRef::Body)
        .iter()
        .map(|path| {
            doc.para(StoryRef::Body, path).unwrap().props.numbering.map(|n| {
                let def = doc.numbering.level(n.num, n.level).expect("list definition exists");
                (def.format != NumFormat::Bullet, n.level)
            })
        })
        .collect()
}

#[test]
fn rtf_prior_list_accept_and_reject_after_save_reopen() {
    for (old, new) in [
        (Some(true), Some(false)),
        (Some(false), Some(true)),
        (Some(true), None),
        (Some(false), None),
        (None, Some(true)),
        (None, Some(false)),
        (Some(true), Some(true)),
        (Some(false), Some(false)),
    ] {
        for level in [0, 2, 8] {
            let mut doc = changed_list(old, new, level);
            // The unsaved document is the control; repeat the save/reopen to catch drift.
            for saves in 0..=2 {
                for (command, expected) in [("review.rejectAll", old), ("review.acceptAll", new)] {
                    let mut s = Session::new(doc.clone());
                    s.run(command, &json!({})).unwrap();
                    assert_eq!(
                        lists(&s.doc),
                        vec![expected.map(|ordered| (ordered, level)), new.map(|ordered| (ordered, level))],
                        "{command}, old={old:?}, new={new:?}, level={level}, saves={saves}"
                    );
                    assert_eq!(s.doc.plain_text(StoryRef::Body).trim_end(), "Item\nNext");
                    assert!(s.run("review.changes", &json!({})).unwrap().as_array().unwrap().is_empty());
                }
                doc = rtf::import(rtf::export(&doc).as_bytes()).unwrap();
            }
        }
    }
}
