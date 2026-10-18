//! Design › Watermark: text or a picture drawn behind the text of every page.

use serde::{Deserialize, Serialize};
use wordcraft_geom::Rect;

use crate::props::Rgb;
use crate::styles;

/// Largest picture watermark scale, percent (the top of Word's Scale list).
pub const MAX_SCALE: f32 = 500.0;

/// Smallest picture watermark scale, percent.
pub const MIN_SCALE: f32 = 1.0;

/// Longest side of a placed picture watermark, points.
const MAX_SIDE: f32 = 31_680.0;

/// Longest side, in pixels, a washed-out watermark picture is drawn from: a larger picture is
/// scaled down first, so the faded copy stays a sensible size on screen and in a PDF.
pub const MAX_WASHED_PX: u32 = 4096;

/// The document's watermark: its text settings, and a picture when it is a picture watermark.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Watermark {
    pub text: String,
    pub font: String,
    pub color: Rgb,
    pub diagonal: bool,
    /// Word's Semitransparent check box (the text 50 % transparent).
    pub semitransparent: bool,
    /// The text's transparency in percent when it is neither 0 nor 50 (see
    /// [`Watermark::transparency`]); None follows `semitransparent`, as older documents do.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transparency: Option<u8>,
    /// A picture watermark, drawn instead of the text (which is kept for the dialog).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub picture: Option<WatermarkPicture>,
}

impl Default for Watermark {
    fn default() -> Self {
        Watermark {
            text: "DRAFT".into(),
            font: styles::BODY_FONT.into(),
            color: Rgb(0xC0, 0xC0, 0xC0),
            diagonal: true,
            semitransparent: true,
            transparency: None,
            picture: None,
        }
    }
}

impl Watermark {
    /// The text's transparency in percent: 0 is opaque, 100 invisible, Semitransparent is 50.
    pub fn transparency(&self) -> u8 {
        match self.transparency {
            Some(t) => t.min(100),
            None if self.semitransparent => 50,
            None => 0,
        }
    }

    /// Set the text's transparency in percent (at most 100). Semitransparent follows it.
    pub fn set_transparency(&mut self, percent: u8) {
        let t = percent.min(100);
        self.semitransparent = t > 0;
        self.transparency = (t != 0 && t != 50).then_some(t);
    }

    /// The text's opacity, 0 to 1.
    pub fn opacity(&self) -> f32 {
        1.0 - self.transparency() as f32 / 100.0
    }
}

/// A picture watermark (Design › Watermark › Custom Watermark › Picture watermark).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WatermarkPicture {
    /// Key into [`crate::Document::media`].
    pub media: String,
    /// Percent of the picture's own size (its pixels at 96 ppi); None for Auto, as large as fits
    /// inside the margins.
    pub scale: Option<f32>,
    /// Word's Washout: the picture faded so the text over it can be read.
    pub washout: bool,
}

impl Default for WatermarkPicture {
    fn default() -> Self {
        WatermarkPicture { media: String::new(), scale: None, washout: true }
    }
}

impl WatermarkPicture {
    /// Where the picture goes, centred in `area` (the page's margins) as Word places it, for a
    /// picture `own_w` × `own_h` points. None for an empty or unusable size.
    pub fn place(&self, own_w: f32, own_h: f32, area: Rect) -> Option<Rect> {
        let pos = |v: f32| v.is_finite() && v > 0.0;
        if !(pos(own_w) && pos(own_h) && pos(area.w) && pos(area.h) && area.x.is_finite() && area.y.is_finite()) {
            return None;
        }
        let k = match self.scale {
            Some(s) if s.is_finite() => s.clamp(MIN_SCALE, MAX_SCALE) / 100.0,
            _ => (area.w / own_w).min(area.h / own_h),
        };
        let k = k.min(MAX_SIDE / own_w.max(own_h));
        let (w, h) = (own_w * k, own_h * k);
        (pos(w) && pos(h)).then(|| Rect::new(area.x + (area.w - w) / 2.0, area.y + (area.h - h) / 2.0, w, h))
    }
}

/// Word's Washout on one colour channel: 30 % contrast with the black level raised, so black
/// becomes a light grey and light colours white.
pub fn washout(v: u8) -> u8 {
    (v as f32 * 0.3 + 205.0).round().min(255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transparency_follows_semitransparent() {
        let mut wm = Watermark::default();
        assert_eq!((wm.transparency(), wm.opacity()), (50, 0.5));
        wm.semitransparent = false;
        assert_eq!(wm.transparency(), 0);
        wm.set_transparency(80);
        assert_eq!((wm.semitransparent, wm.transparency, wm.transparency()), (true, Some(80), 80));
        // Word's two values are stored as the check box alone.
        wm.set_transparency(50);
        assert_eq!((wm.semitransparent, wm.transparency), (true, None));
        wm.set_transparency(0);
        assert_eq!((wm.semitransparent, wm.transparency), (false, None));
        wm.set_transparency(255);
        assert_eq!((wm.transparency(), wm.opacity()), (100, 0.0));
        // Older documents have no transparency field.
        let old: Watermark = serde_json::from_str(r#"{"text":"X","semitransparent":false}"#).unwrap();
        assert_eq!((old.transparency(), old.picture), (0, None));
        let hostile: Watermark = serde_json::from_str(r#"{"transparency":200}"#).unwrap();
        assert_eq!(hostile.transparency(), 100);
    }

    /// Rounded to a hundredth of a point: (x, y, w, h).
    fn r(r: Option<Rect>) -> Option<(f32, f32, f32, f32)> {
        let q = |v: f32| (v * 100.0).round() / 100.0;
        r.map(|r| (q(r.x), q(r.y), q(r.w), q(r.h)))
    }

    #[test]
    fn picture_is_placed_in_the_margins() {
        let area = Rect::new(72.0, 72.0, 468.0, 648.0);
        // Auto: as large as fits, centred.
        let auto = WatermarkPicture::default();
        assert_eq!(r(auto.place(200.0, 100.0, area)), Some((72.0, 279.0, 468.0, 234.0)));
        assert_eq!(r(auto.place(100.0, 400.0, area)), Some((225.0, 72.0, 162.0, 648.0)));
        // A scale is a percentage of the picture's own size, and may spill past the margins.
        let half = WatermarkPicture { scale: Some(50.0), ..Default::default() };
        assert_eq!(r(half.place(200.0, 100.0, area)), Some((256.0, 371.0, 100.0, 50.0)));
        let big = WatermarkPicture { scale: Some(500.0), ..Default::default() };
        assert_eq!(r(big.place(200.0, 100.0, area)), Some((-194.0, 146.0, 1000.0, 500.0)));
        // Hostile sizes and scales.
        let wild = WatermarkPicture { scale: Some(1e30), ..Default::default() };
        assert_eq!(r(wild.place(200.0, 100.0, area)).map(|r| r.2), Some(1000.0));
        let tiny = WatermarkPicture { scale: Some(-5.0), ..Default::default() };
        assert_eq!(r(tiny.place(200.0, 100.0, area)).map(|r| r.2), Some(2.0));
        let nan = WatermarkPicture { scale: Some(f32::NAN), ..Default::default() };
        assert_eq!(r(nan.place(200.0, 100.0, area)).map(|r| r.2), Some(468.0));
        assert!(auto.place(0.0, 100.0, area).is_none());
        assert!(auto.place(f32::INFINITY, 100.0, area).is_none());
        assert!(auto.place(200.0, 100.0, Rect::new(0.0, 0.0, -1.0, 10.0)).is_none());
        let huge = auto.place(1e30, 1e30, Rect::new(0.0, 0.0, 1e38, 1e38)).unwrap();
        assert!(huge.w <= MAX_SIDE + 1.0 && huge.h <= MAX_SIDE + 1.0);
    }

    #[test]
    fn washout_lightens() {
        assert_eq!(washout(0), 205);
        assert_eq!(washout(100), 235);
        assert_eq!(washout(255), 255);
        assert!((0..=255u8).all(|v| washout(v) >= v.max(205)));
    }
}
