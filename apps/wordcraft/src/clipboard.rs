//! The system clipboard beyond egui's text (#45, #584, #687, #833): text for the ribbon's Paste,
//! another program's formatted copy (HTML, and RTF where the platform offers it), pictures and
//! copied files, and the names of the formats there for a paste that finds nothing it can use.
//!
//! On Linux, arboard talks to Wayland compositors through the data-control protocol (KDE Plasma,
//! wlroots compositors); without it, a native Wayland window only sees what the compositor copies
//! over to Xwayland, which KWin does only while an X11 window has focus — so pictures copied in a
//! browser or Spectacle never arrived (#833). Compositors without the protocol fall back to X11.
//!
//! Everything read here comes from other programs: sizes are capped as it is read.
//!
//! Written here: a copy puts HTML beside its text, so other programs keep its formatting (#1523),
//! with equations as MathML (#816). arboard sets HTML with a text alternative on every desktop
//! platform. A copy of one equation alone also goes in the platform's MathML formats (#1297),
//! added beside arboard's: on Windows "MathML" and "MathML Presentation" (UTF-16LE), on macOS
//! `public.mathml.presentation` and `public.mathml` — the names W3C MathML 3 §6.2.4 registers
//! for transferring MathML, and the ones Microsoft documents its Office apps reading first when
//! they paste math (learn.microsoft.com/office/math/mathml, "Clipboard support"). Pasting into
//! Word and LibreOffice is not yet verified. Linux and the BSDs have no Word, and arboard can't
//! add MIME types of other names, so there the HTML carries the MathML alone.

use wordcraft_engine::cmd::paste::MAX_PASTE_BYTES;
use wordcraft_ui_egui::paste_picture::ClipboardPicture;

fn open() -> Option<arboard::Clipboard> {
    arboard::Clipboard::new().map_err(|e| log::warn!("clipboard: {e}")).ok()
}

thread_local! {
    /// The clipboard copies are set through, kept open: on X11 the contents are served from it.
    static WRITER: std::cell::RefCell<Option<arboard::Clipboard>> = const { std::cell::RefCell::new(None) };
}

/// Put `html` on the clipboard with `text` as its plain-text alternative, and `mathml` (a copy of
/// one equation alone; empty for any other copy) in the platform's MathML formats. `false` when
/// the HTML and text couldn't be set (the caller copies the text alone), including a Wayland
/// session without the data-control protocol, where arboard would fall back to X11 and native
/// Wayland programs might not see it. MathML that can't be added is logged: the HTML still
/// carries it.
pub fn set_html(html: &str, text: &str, mathml: &str) -> bool {
    if !platform::can_set() {
        return false;
    }
    let set = set_html_text(html, text);
    if set && !mathml.is_empty() && mathml.len() <= MAX_PASTE_BYTES {
        platform::add_mathml(mathml);
    }
    set
}

fn set_html_text(html: &str, text: &str) -> bool {
    WRITER.with(|w| {
        let Ok(mut slot) = w.try_borrow_mut() else { return false };
        if slot.is_none() {
            *slot = open();
        }
        match slot.as_mut().map(|c| c.set().html(html, Some(text))) {
            Some(Ok(())) => {
                #[cfg(target_os = "windows")]
                if let Err(e) = platform::mark_own() {
                    log::warn!("clipboard owner: {e}");
                }
                true
            }
            Some(Err(e)) => {
                log::warn!("clipboard: {e}");
                *slot = None;
                false
            }
            None => false,
        }
    })
}

/// Windows: publish pixels, HTML and text while holding one clipboard lock.
#[cfg(target_os = "windows")]
pub fn set_image(image: &wordcraft_ui_egui::system_clipboard::CopyImage, html: &str, text: &str) -> Result<(), String> {
    let pixels = image.width.checked_mul(image.height);
    if image.width == 0
        || image.height == 0
        || image.width > wordcraft_ui_egui::paste_picture::MAX_SIDE
        || image.height > wordcraft_ui_egui::paste_picture::MAX_SIDE
        || pixels.is_none_or(|n| n > wordcraft_ui_egui::paste_picture::MAX_PIXELS || n.checked_mul(4) != Some(image.rgba.len()))
    {
        return Err("invalid picture dimensions".into());
    }
    platform::set_image(image, html, text)
}

/// Whether the system offer still has the marker from this process's latest successful write.
#[cfg(target_os = "windows")]
pub fn is_own() -> bool {
    platform::is_own()
}

#[cfg(all(test, target_os = "windows"))]
mod image_tests {
    use super::*;
    use wordcraft_ui_egui::system_clipboard::CopyImage;

    /// Supply an explicit BMP pixel offset: image 0.25.10's headerless decoder skips an
    /// extra twelve mask bytes after a V5 header even though its masks are inside the header.
    pub(super) fn decode_dib(bytes: &[u8]) -> Vec<u8> {
        let mut bmp = b"BM".to_vec();
        bmp.extend_from_slice(&((bytes.len() + 14) as u32).to_le_bytes());
        bmp.extend_from_slice(&[0; 4]);
        bmp.extend_from_slice(&138u32.to_le_bytes());
        bmp.extend_from_slice(bytes);
        let decoder = image::codecs::bmp::BmpDecoder::new(std::io::Cursor::new(bmp)).unwrap();
        image::DynamicImage::from_decoder(decoder).unwrap().into_rgba8().into_raw()
    }

    /// Invalid pixel buffers fail before acquiring or changing the clipboard.
    #[test]
    fn invalid_pixels_are_rejected_before_opening_the_clipboard() {
        let image = CopyImage { width: usize::MAX, height: 2, rgba: Vec::new() };
        assert!(set_image(&image, "", "").is_err());
        let image = CopyImage { width: 2, height: 2, rgba: vec![0; 15] };
        assert!(set_image(&image, "", "").is_err());
    }

    /// Explicit desktop integration check: this publishes a synthetic picture to the clipboard.
    #[test]
    #[ignore = "writes the real Windows clipboard; run explicitly during desktop validation"]
    fn windows_image_offer_keeps_pixels_html_and_text_together() {
        let rgba: Vec<u8> = (0..4098).flat_map(|i| [(i % 251) as u8, 37, 219, (i % 255) as u8]).collect();
        let image = CopyImage { width: 2049, height: 2, rgba: rgba.clone() };
        let html = "<p>Картинка</p>";
        set_image(&image, html, "").unwrap();
        {
            let _open = clipboard_win::Clipboard::new_attempts(10).unwrap();
            let mut bytes = Vec::new();
            clipboard_win::raw::get_vec(clipboard_win::formats::CF_DIBV5, &mut bytes).unwrap();
            assert_eq!(decode_dib(&bytes), rgba);
        }
        let mut clipboard = open().unwrap();
        let got = clipboard.get_image().unwrap();
        assert_eq!((got.width, got.height), (2049, 2));
        assert_eq!(got.bytes.as_ref(), rgba);
        assert_eq!(clipboard.get_text().unwrap(), "");
        assert_eq!(clipboard.get().html().unwrap(), html);
        assert!(is_own());
        // A subsequent copy from another program removes the private owner format.
        clipboard.set_text("external synthetic text").unwrap();
        assert!(!is_own());
    }
}

/// The clipboard's text.
pub fn text() -> Option<String> {
    open()?.get_text().ok()
}

/// The clipboard's HTML (Windows: the fragment of its "HTML Format"), if not too large.
pub fn html() -> Option<String> {
    open()?.get().html().ok().filter(|h| h.len() <= MAX_PASTE_BYTES)
}

/// The clipboard's RTF: Windows ("Rich Text Format") and Wayland (`text/rtf`); not offered by
/// arboard, so read here.
pub fn rtf() -> Option<String> {
    let bytes = platform::rtf()?;
    if bytes.len() > MAX_PASTE_BYTES {
        return None;
    }
    // RTF is 7-bit by the spec; stray 8-bit bytes become replacement characters.
    let s = String::from_utf8_lossy(&bytes).trim_end_matches('\0').to_string();
    s.trim_start().starts_with("{\\rtf").then_some(s)
}

/// The names of the formats on the clipboard, where the platform lists them (Wayland, Windows).
pub fn formats() -> Vec<String> {
    platform::formats()
}

/// The picture on the clipboard: copied files, else image pixels, else (Wayland) an encoded
/// picture in a format arboard doesn't decode. A picture that is there but can't be read says
/// why (#833). The UI checks the sizes before inserting anything.
pub fn picture() -> Option<ClipboardPicture> {
    let mut clipboard = match arboard::Clipboard::new() {
        Ok(c) => c,
        Err(e) => {
            log::warn!("clipboard: {e}");
            return Some(ClipboardPicture::Unreadable(format!("the system clipboard couldn't be opened ({e})")));
        }
    };
    if let Ok(files) = clipboard.get().file_list()
        && !files.is_empty()
    {
        return Some(ClipboardPicture::Files(files));
    }
    match clipboard.get_image() {
        Ok(image) => Some(ClipboardPicture::Pixels { width: image.width, height: image.height, rgba: image.bytes.into_owned() }),
        Err(arboard::Error::ContentNotAvailable) => platform::encoded_picture().map(ClipboardPicture::Encoded),
        Err(arboard::Error::ConversionFailure) => Some(
            platform::encoded_picture()
                .map(ClipboardPicture::Encoded)
                .unwrap_or_else(|| ClipboardPicture::Unreadable("WordCraft can't decode its format".into())),
        ),
        Err(e) => {
            log::warn!("clipboard picture: {e}");
            platform::encoded_picture().map(ClipboardPicture::Encoded).or(Some(ClipboardPicture::Unreadable(e.to_string())))
        }
    }
}

/// Picture types read straight from Wayland when arboard (PNG only) finds none, best first.
#[cfg(all(unix, not(any(target_os = "macos", target_os = "android", target_os = "emscripten"))))]
const PICTURE_MIMES: [&str; 6] = ["image/png", "image/jpeg", "image/gif", "image/webp", "image/bmp", "image/x-bmp"];

#[cfg(all(unix, not(any(target_os = "macos", target_os = "android", target_os = "emscripten"))))]
mod platform {
    use std::io::Read;

    use wl_clipboard_rs::paste::{ClipboardType, MimeType, Seat, get_contents, get_mime_types_ordered};

    fn wayland() -> bool {
        std::env::var_os("WAYLAND_DISPLAY").is_some()
    }

    /// One MIME type's bytes, at most `cap`; `None` when absent or larger.
    fn read(mime: &str, cap: u64) -> Option<Vec<u8>> {
        let (pipe, _) = get_contents(ClipboardType::Regular, Seat::Unspecified, MimeType::Specific(mime)).ok()?;
        let mut out = Vec::new();
        pipe.take(cap.saturating_add(1)).read_to_end(&mut out).ok()?;
        (out.len() as u64 <= cap).then_some(out)
    }

    pub fn formats() -> Vec<String> {
        if !wayland() {
            return Vec::new();
        }
        get_mime_types_ordered(ClipboardType::Regular, Seat::Unspecified).unwrap_or_default()
    }

    /// Whether arboard sets the clipboard natively: X11, or Wayland with data-control.
    pub fn can_set() -> bool {
        !wayland()
            || !matches!(
                get_mime_types_ordered(ClipboardType::Regular, Seat::Unspecified),
                Err(wl_clipboard_rs::paste::Error::MissingProtocol { .. } | wl_clipboard_rs::paste::Error::WaylandConnection(_))
            )
    }

    /// arboard serves its own MIME types only; Word, which reads the MathML formats, isn't here.
    pub fn add_mathml(_mathml: &str) {}

    pub fn rtf() -> Option<Vec<u8>> {
        if !wayland() {
            return None;
        }
        let offered = formats();
        let mime = ["text/rtf", "application/rtf", "text/richtext"].into_iter().find(|m| offered.iter().any(|o| o == m))?;
        read(mime, super::MAX_PASTE_BYTES as u64)
    }

    pub fn encoded_picture() -> Option<Vec<u8>> {
        if !wayland() {
            return None;
        }
        let offered = formats();
        let mime = super::PICTURE_MIMES.into_iter().find(|m| offered.iter().any(|o| o == m))?;
        read(mime, wordcraft_ui_egui::paste_picture::MAX_FILE_BYTES)
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use clipboard_win::{Clipboard, EnumFormats, raw};

    const OWNER_FORMAT: &str = "WordCraft Copy Owner";

    thread_local! {
        static OWNER: std::cell::Cell<Option<[u8; 12]>> = const { std::cell::Cell::new(None) };
    }

    /// Bind the private fragment to this publication, rather than any copy from the same PID.
    fn write_owner() -> Result<(), String> {
        static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        OWNER.set(None);
        let generation = GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut marker = [0; 12];
        marker[..4].copy_from_slice(&std::process::id().to_le_bytes());
        marker[4..].copy_from_slice(&generation.to_le_bytes());
        let format = raw::register_format(OWNER_FORMAT).ok_or("couldn't register clipboard owner")?;
        raw::set_without_clear(format.get(), &marker).map_err(|e| e.to_string())?;
        OWNER.set(Some(marker));
        Ok(())
    }

    /// An optional marker failure cannot undo an otherwise successful public offer.
    pub fn mark_own() -> Result<(), String> {
        OWNER.set(None);
        let _open = Clipboard::new_attempts(10).map_err(|e| e.to_string())?;
        write_owner()
    }

    /// Encode a top-down 32-bit sRGB DIBV5, with straight alpha and BGRA channels.
    /// Layout: learn.microsoft.com/windows/win32/api/wingdi/ns-wingdi-bitmapv5header.
    fn dibv5(image: &wordcraft_ui_egui::system_clipboard::CopyImage) -> Result<Vec<u8>, String> {
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(124 + image.rgba.len()).map_err(|e| e.to_string())?;
        for word in [
            124,
            image.width as u32,
            (-(image.height as i32)) as u32,
            1 | (32 << 16),
            3,
            image.rgba.len() as u32,
            0,
            0,
            0,
            0,
            0x00ff_0000,
            0x0000_ff00,
            0x0000_00ff,
            0xff00_0000,
            0x7352_4742,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            4,
            0,
            0,
            0,
        ] {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        for pixel in image.rgba.chunks_exact(4) {
            if let [r, g, b, a] = pixel {
                bytes.extend_from_slice(&[*b, *g, *r, *a]);
            }
        }
        Ok(bytes)
    }

    /// Keep all image formats under one open clipboard so another writer cannot interleave.
    pub fn set_image(image: &wordcraft_ui_egui::system_clipboard::CopyImage, html: &str, text: &str) -> Result<(), String> {
        OWNER.set(None);
        let pixels = dibv5(image)?;
        let mut png = Vec::new();
        image::ImageEncoder::write_image(
            image::codecs::png::PngEncoder::new(&mut png),
            &image.rgba,
            image.width as u32,
            image.height as u32,
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|e| e.to_string())?;
        let png_format = raw::register_format("PNG").ok_or("couldn't register PNG clipboard format")?;
        let format = raw::register_format("HTML Format").ok_or("couldn't register HTML clipboard format")?;
        let _open = Clipboard::new_attempts(10).map_err(|e| e.to_string())?;
        raw::empty().map_err(|e| e.to_string())?;
        raw::set_without_clear(clipboard_win::formats::CF_DIBV5, &pixels).map_err(|e| e.to_string())?;
        raw::set_without_clear(png_format.get(), &png).map_err(|e| e.to_string())?;
        raw::set_string_with(text, clipboard_win::options::NoClear).map_err(|e| e.to_string())?;
        raw::set_html_with(format.get(), html, clipboard_win::options::NoClear).map_err(|e| e.to_string())?;
        if let Err(e) = write_owner() {
            log::warn!("clipboard owner: {e}");
        }
        Ok(())
    }

    /// A stale marker from an earlier copy cannot claim the current private fragment.
    pub fn is_own() -> bool {
        let Some(expected) = OWNER.get() else { return false };
        let Some(format) = raw::register_format(OWNER_FORMAT) else { return false };
        let Ok(_open) = Clipboard::new_attempts(10) else { return false };
        let mut bytes = [0; 12];
        raw::get(format.get(), &mut bytes).ok() == Some(bytes.len()) && bytes == expected
    }

    /// The public bitmap representation must preserve orientation, channels and alpha without
    /// depending on a PNG format masking malformed DIB data in the integration test.
    #[test]
    fn dibv5_roundtrip_keeps_all_pixels() {
        let rgba = vec![255, 0, 0, 255, 0, 255, 0, 128, 0, 0, 255, 0, 73, 91, 127, 192];
        let image = wordcraft_ui_egui::system_clipboard::CopyImage { width: 2, height: 2, rgba: rgba.clone() };
        let bytes = dibv5(&image).unwrap();
        assert_eq!(bytes.len(), 124 + rgba.len());
        assert_eq!(super::image_tests::decode_dib(&bytes), rgba);
    }

    pub fn formats() -> Vec<String> {
        let Ok(_open) = Clipboard::new_attempts(10) else { return Vec::new() };
        // Standard formats are named too (CF_BITMAP, CF_UNICODETEXT…).
        EnumFormats::new().take(64).filter_map(raw::format_name_big).collect()
    }

    pub fn rtf() -> Option<Vec<u8>> {
        let format = raw::register_format("Rich Text Format")?;
        let _open = Clipboard::new_attempts(10).ok()?;
        if raw::size(format.get()).is_none_or(|n| n.get() > super::MAX_PASTE_BYTES) {
            return None;
        }
        let mut out = Vec::new();
        raw::get_vec(format.get(), &mut out).ok()?;
        Some(out)
    }

    pub fn encoded_picture() -> Option<Vec<u8>> {
        None
    }

    pub fn can_set() -> bool {
        true
    }

    /// Add the MathML to what arboard just set, as "MathML" and "MathML Presentation": UTF-16LE
    /// text (with its terminating NUL, as Windows text formats have).
    pub fn add_mathml(mathml: &str) {
        let Ok(_open) = Clipboard::new_attempts(10) else {
            log::warn!("clipboard: couldn't open it to add MathML");
            return;
        };
        let bytes: Vec<u8> = mathml.encode_utf16().chain(std::iter::once(0)).flat_map(u16::to_le_bytes).collect();
        for name in super::MATHML_FORMATS {
            let Some(format) = raw::register_format(name) else { continue };
            if let Err(e) = raw::set_without_clear(format.get(), &bytes) {
                log::warn!("clipboard: {name}: {e}");
            }
        }
    }
}

/// The clipboard format names for MathML that Windows programs read (W3C MathML 3 §6.2.4; Office
/// reads and writes both).
#[cfg(target_os = "windows")]
const MATHML_FORMATS: [&str; 2] = ["MathML", "MathML Presentation"];

#[cfg(target_os = "macos")]
mod platform {
    use objc2_app_kit::NSPasteboard;
    use objc2_foundation::NSString;

    pub fn formats() -> Vec<String> {
        Vec::new()
    }

    pub fn rtf() -> Option<Vec<u8>> {
        None
    }

    pub fn encoded_picture() -> Option<Vec<u8>> {
        None
    }

    pub fn can_set() -> bool {
        true
    }

    /// Add the MathML to the pasteboard item arboard just wrote, as the MathML types (W3C MathML 3
    /// §6.2.4): presentation markup, and the generic type for readers that only know that one.
    pub fn add_mathml(mathml: &str) {
        let board = NSPasteboard::generalPasteboard();
        let text = NSString::from_str(mathml);
        for name in ["public.mathml.presentation", "public.mathml"] {
            if !board.setString_forType(&text, &NSString::from_str(name)) {
                log::warn!("clipboard: couldn't add {name}");
            }
        }
    }
}

#[cfg(not(any(
    target_os = "windows",
    target_os = "macos",
    all(unix, not(any(target_os = "macos", target_os = "android", target_os = "emscripten")))
)))]
mod platform {
    pub fn formats() -> Vec<String> {
        Vec::new()
    }

    pub fn rtf() -> Option<Vec<u8>> {
        None
    }

    pub fn encoded_picture() -> Option<Vec<u8>> {
        None
    }

    pub fn can_set() -> bool {
        true
    }

    pub fn add_mathml(_mathml: &str) {}
}
