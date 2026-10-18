//! The document's watermark in the headers: a text watermark as a VML text shape (ECMA-376
//! Part 4, VML: `shape`, `shapetype`, `path`, `textpath`, `fill`), a picture watermark as a VML
//! picture (`imagedata`), which is how a WordprocessingML consumer draws them behind every page.
//! The reader (`read::story::vml_watermark`, `read::watermark`) maps them back.

use wordcraft_doc::{Document, SectionProps, Watermark, WatermarkPicture};

use crate::xml::esc;

/// Word's Washout on `v:imagedata`: `gain` 0.3 and `blacklevel` 0.35, in 65536ths.
const WASHOUT: &str = r#" gain="19661f" blacklevel="22938f""#;

/// A picture's own size in points (its pixels at 96 ppi), from its header. None when it can't
/// be read.
pub(crate) fn own_size(bytes: &[u8]) -> Option<(f32, f32)> {
    let (w, h) = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().ok()?.into_dimensions().ok()?;
    (w > 0 && h > 0).then_some((w as f32 * 0.75, h as f32 * 0.75))
}

/// The document's picture watermark and its picture's own size, when its picture can be read.
pub(super) fn picture(doc: &Document) -> Option<(&WatermarkPicture, (f32, f32))> {
    let pic = doc.settings.watermark.as_ref()?.picture.as_ref()?;
    Some((pic, doc.media.get(&pic.media).and_then(|b| own_size(b))?))
}

/// Whether the document's watermark is written: text that isn't blank, or a picture that can be read.
pub(super) fn shown(doc: &Document) -> bool {
    match &doc.settings.watermark {
        Some(wm) if wm.picture.is_some() => picture(doc).is_some(),
        Some(wm) => text(wm).is_some(),
        None => false,
    }
}

/// The `v:fill` `opacity` for the text's transparency: Word's `.5` for Semitransparent.
fn opacity(wm: &Watermark) -> Option<String> {
    let t = wm.transparency();
    if t == 0 {
        return None;
    }
    let o = format!("{:.2}", (100 - t) as f32 / 100.0);
    let o = o.trim_end_matches('0').trim_end_matches('.');
    Some(o.strip_prefix("0.").map_or_else(|| o.to_string(), |f| format!(".{f}")))
}

/// The shape type the watermark shape uses: two straight subpaths, the box's top and bottom
/// edges, between which the `v:textpath` text is stretched to fit the shape (`fitshape`). A single
/// path gives the text no height, and Word draws nothing.
const SHAPETYPE_ID: &str = "_x0000_t136";
const SHAPETYPE_PATH: &str = "m0,0l21600,0m0,21600l21600,21600e";

/// The watermark text as written: what the reader accepts (non-blank, at most 200 characters).
pub(super) fn text(wm: &Watermark) -> Option<String> {
    let t: String = wm.text.chars().take(200).collect();
    (!t.trim().is_empty()).then_some(t)
}

/// A `w:r` holding the watermark shape (and its shape type), for the first paragraph of header
/// part number `n`. Sized like the renderers draw it: half the page width across, or half the
/// page diagonal when diagonal; the height follows the text's length.
pub(super) fn run(wm: &Watermark, n: usize, page: &SectionProps) -> Option<String> {
    let text = text(wm)?;
    let (pw, ph) = (page.page_w.clamp(72.0, 4000.0), page.page_h.clamp(72.0, 4000.0));
    let across = if wm.diagonal { (pw * pw + ph * ph).sqrt() } else { pw };
    let width = (across * 0.5).clamp(36.0, 2000.0);
    let chars = text.chars().count().clamp(1, 200) as f32;
    let height = (width / (0.55 * chars)).clamp(12.0, 400.0);
    // A font name can't break out of the textpath's CSS-like style.
    let font: String = wm.font.chars().filter(|c| !matches!(c, '"' | '\'' | ';' | ':')).take(100).collect();
    let font = if font.trim().is_empty() { Watermark::default().font } else { font };
    let rotation = if wm.diagonal { "rotation:315;" } else { "" };
    let style = format!(
        "position:absolute;margin-left:0;margin-top:0;width:{width:.1}pt;height:{height:.1}pt;{rotation}z-index:-251654144;\
         mso-position-horizontal:center;mso-position-horizontal-relative:margin;\
         mso-position-vertical:center;mso-position-vertical-relative:margin"
    );
    let fill = opacity(wm).map(|o| format!(r#"<v:fill opacity="{o}"/>"#)).unwrap_or_default();
    Some(format!(
        concat!(
            r#"<w:r><w:rPr><w:noProof/></w:rPr><w:pict>"#,
            r#"<v:shapetype id="{st}" coordsize="21600,21600" o:spt="136" path="{path}">"#,
            r#"<v:path textpathok="t"/><v:textpath on="t" fitshape="t"/></v:shapetype>"#,
            r##"<v:shape id="PowerPlusWaterMarkObject{n}" type="#{st}" style="{style}" o:allowincell="f" fillcolor="#{color}" stroked="f">"##,
            r#"{fill}<v:textpath style="{tstyle}" string="{text}"/></v:shape></w:pict></w:r>"#
        ),
        st = SHAPETYPE_ID,
        path = SHAPETYPE_PATH,
        n = n,
        style = esc(&style),
        color = wm.color.hex(),
        fill = fill,
        tstyle = esc(&format!("font-family:\"{font}\";font-size:1pt")),
        text = esc(&text),
    ))
}

/// A `w:r` holding the picture watermark for header part number `n`, its picture relationship
/// `rid`: Word's picture shape, centred on the margins and sized as the renderers draw it, behind
/// the text, with Word's Washout values when washed out.
pub(super) fn picture_run(pic: &WatermarkPicture, own: (f32, f32), n: usize, page: &SectionProps, rid: &str) -> Option<String> {
    let r = pic.place(own.0, own.1, page.margin_area())?;
    let style = format!(
        "position:absolute;margin-left:0;margin-top:0;width:{w:.3}pt;height:{h:.3}pt;z-index:-251657216;\
         mso-position-horizontal:center;mso-position-horizontal-relative:margin;\
         mso-position-vertical:center;mso-position-vertical-relative:margin",
        w = r.w,
        h = r.h
    );
    Some(format!(
        concat!(
            r#"<w:r><w:rPr><w:noProof/></w:rPr><w:pict>"#,
            r#"<v:shapetype id="_x0000_t75" coordsize="21600,21600" o:spt="75" o:preferrelative="t" path="m0,0l0,21600,21600,21600,21600,0xe" filled="f" stroked="f">"#,
            r#"<v:path o:extrusionok="f" gradientshapeok="t" o:connecttype="rect"/><o:lock v:ext="edit" aspectratio="t"/></v:shapetype>"#,
            r##"<v:shape id="WordPictureWatermark{n}" type="#_x0000_t75" style="{style}" o:allowincell="f">"##,
            r#"<v:imagedata r:id="{rid}"{wash}/></v:shape></w:pict></w:r>"#
        ),
        n = n,
        style = esc(&style),
        rid = esc(rid),
        wash = if pic.washout { WASHOUT } else { "" },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opacity_is_written_like_word() {
        let mut wm = Watermark::default();
        assert_eq!(opacity(&wm).as_deref(), Some(".5"));
        for (t, want) in [(0, None), (25, Some(".75")), (80, Some(".2")), (99, Some(".01")), (100, Some("0"))] {
            wm.set_transparency(t);
            assert_eq!(opacity(&wm).as_deref(), want, "{t}");
        }
    }
}
