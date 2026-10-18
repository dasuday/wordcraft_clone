# Clipboard image copies

A Windows selection containing exactly one inline image offers a native bitmap, PNG, HTML and
Unicode text in the same clipboard update. Other programs such as Paint can paste the pixels.
A mixed text/image selection offers HTML with embedded image data and plain text; it does not
replace the whole selection with its first picture.

Raster metadata is checked before decoding: at most 16,000 pixels per side, 50 million pixels
and 200 MB of source bytes. Decoder allocations are capped. This permits narrow images wider
than 2,048 pixels without accepting unbounded images. WMF/EMF pictures use the existing bounded
renderer at 96 dpi. Unsupported/corrupt pictures report an error and fall back to HTML/text instead of panicking.

A per-publication owner marker identifies the last successful copy from the current process.
A failed native write invalidates reuse of the new private fragment, even if an earlier marker
remains on the clipboard. A marker failure is logged without reporting a successful public
offer as failed. While the current marker remains on the clipboard,
regular paste in that window uses the original fragment, retaining picture crop, display size
and object properties. Copying in another application replaces the marker. Another WordCraft
window uses the public HTML/bitmap formats instead of the first window's private fragment.

Bitmap, PNG, HTML and Unicode text are written under a single Windows clipboard lock, using
a top-down sRGB DIBV5 with straight alpha. Another writer cannot interleave between these
formats. If the image offer fails, one HTML/text fallback runs; a successful image offer is
never overwritten by a subsequent fallback.

Native bitmap publication and owner markers are Windows-specific in this change. Other hosts
retain their existing HTML/text services. The source image pixels are exported; the document's
crop/size remain in its HTML/private fragment. Multiple selected pictures are represented in
HTML, not combined into a bitmap.

## Validation

Regression tests cover copy/cut of a 2,049-pixel-wide picture, empty selections, mixed text and
pictures, retaining crop/display size on local paste, oversized/corrupt raster metadata and
EMF rasterization and independent DIB pixel decoding. The explicitly ignored Windows integration test
`windows_image_offer_keeps_pixels_html_and_text_together` writes synthetic clipboard content
and verifies bitmap, HTML, Cyrillic text and owner-marker invalidation together. Run it alone
on an interactive Windows desktop; it changes the current clipboard.

A local Windows preview was exercised by copying a synthetic 2,500 by 500 PNG from WordCraft
and pasting it into Paint. Other receiving applications and operating systems still need
manual coverage before a cross-platform release.
