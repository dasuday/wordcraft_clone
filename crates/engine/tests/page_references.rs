use serde_json::json;
use wordcraft_doc::{Path, StoryRef};
use wordcraft_doc::para::InlineObject;
use wordcraft_engine::Session;

fn page_reference_probe() -> Session {
    let mut d = wordcraft_doc::Document::from_text("Custom entry\nTarget heading\nSaved date");
    let p = d.para_mut(StoryRef::Body, &Path::top(0)).unwrap();
    p.insert_object(0, InlineObject::FieldStart { instr: "TOC \\o \"1-3\" \\h".into(), locked: false, dirty: false }, &Default::default()).unwrap();
    p.insert_object(p.len(), InlineObject::Field { instr: "PAGEREF Target \\h".into(), result: "99".into(), locked: false,
        dirty: true, code: false, form: String::new() }, &Default::default()).unwrap();
    p.insert_object(p.len(), InlineObject::FieldEnd, &Default::default()).unwrap();
    let p = d.para_mut(StoryRef::Body, &Path::top(1)).unwrap();
    p.props.page_break_before = Some(true);
    p.insert_object(0, InlineObject::BookmarkStart { name: "Target".into() }, &Default::default()).unwrap();
    p.insert_object(p.len(), InlineObject::BookmarkEnd { name: "Target".into() }, &Default::default()).unwrap();
    let p = d.para_mut(StoryRef::Body, &Path::top(2)).unwrap();
    p.insert_object(p.len(), InlineObject::Field { instr: "DATE".into(), result: "Saved date cache".into(), locked: false,
        dirty: false, code: false, form: String::new() }, &Default::default()).unwrap();
    Session::new(d)
}

#[test]
fn page_reference_only_update_preserves_imported_toc_and_other_fields() {
    let mut s = page_reference_probe();
    let mut expected = s.doc.clone();
    let p = expected.para_mut(StoryRef::Body, &Path::top(0)).unwrap();
    for object in &mut p.objects {
        if let InlineObject::Field { instr, result, dirty, .. } = object {
            if instr.starts_with("PAGEREF") { *result = "2".into(); *dirty = false; }
        }
    }
    let receipt = s.run("references.updateFields", &json!({"pageReferencesOnly": true})).unwrap();
    assert_eq!(receipt["pageReferences"], 1);
    assert_eq!(s.doc.body.len(), expected.body.len(), "do not regenerate the imported TOC");
    for (a, b) in s.doc.body.iter().zip(&expected.body) {
        assert_eq!(serde_json::to_value(a).unwrap(), serde_json::to_value(b).unwrap());
    }
    let repeated = s.run("references.updateFields", &json!({"pageReferencesOnly": true})).unwrap();
    assert_eq!(repeated["changed"], 0);
}

#[test]
fn page_reference_only_update_refuses_missing_or_ambiguous_target_without_changes() {
    for duplicate in [false, true] {
        let mut s = page_reference_probe();
        let p = s.doc.para_mut(StoryRef::Body, &Path::top(1)).unwrap();
        if duplicate {
            p.insert_object(0, InlineObject::BookmarkStart { name: "Target".into() }, &Default::default()).unwrap();
        } else {
            for object in &mut p.objects {
                if let InlineObject::BookmarkStart { name } | InlineObject::BookmarkEnd { name } = object { *name = "Other".into(); }
            }
        }
        let before = serde_json::to_value(&s.doc.body).unwrap();
        assert!(s.run("references.updateFields", &json!({"pageReferencesOnly": true})).is_err());
        assert_eq!(serde_json::to_value(&s.doc.body).unwrap(), before);
    }
}

#[test]
fn page_reference_only_update_respects_field_and_enclosing_range_locks() {
    for outer in [false, true] {
        let mut s = page_reference_probe();
        let p = s.doc.para_mut(StoryRef::Body, &Path::top(0)).unwrap();
        for o in &mut p.objects {
            match o {
                InlineObject::FieldStart { locked, .. } if outer => *locked = true,
                InlineObject::Field { locked, .. } if !outer => *locked = true,
                _ => {}
            }
        }
        let before = serde_json::to_value(&s.doc.body).unwrap();
        let result = s.run("references.updateFields", &json!({"pageReferencesOnly":true})).unwrap();
        assert_eq!(result["pageReferences"], 0);
        assert_eq!(serde_json::to_value(&s.doc.body).unwrap(), before);
    }
}

#[test]
fn page_reference_only_update_uses_numbered_page_format_and_refuses_relative_results() {
    let mut s = page_reference_probe();
    s.doc.last_section.page_num_format = wordcraft_doc::section::NumFormat::LowerRoman;
    let p = s.doc.para_mut(StoryRef::Body, &Path::top(0)).unwrap();
    for o in &mut p.objects {
        if let InlineObject::Field { instr, .. } = o { *instr = "PAGEREF \"Target\" \\h".into(); }
    }
    s.run("references.updateFields", &json!({"pageReferencesOnly":true})).unwrap();
    let p = s.doc.para(StoryRef::Body, &Path::top(0)).unwrap();
    assert!(p.objects.iter().any(|o| matches!(o, InlineObject::Field { result, .. } if result=="ii")));
    s.run("edit.undo", &json!({})).unwrap();
    let p = s.doc.para_mut(StoryRef::Body, &Path::top(0)).unwrap();
    for o in &mut p.objects {
        if let InlineObject::Field { instr, .. } = o { *instr = "PAGEREF Target \\p".into(); }
    }
    let before = serde_json::to_value(&s.doc.body).unwrap();
    assert!(s.run("references.updateFields", &json!({"pageReferencesOnly":true})).is_err());
    assert_eq!(serde_json::to_value(&s.doc.body).unwrap(),before);
}

