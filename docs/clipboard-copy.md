# Clipboard image copies

A Windows selection containing exactly one inline image offers a native bitmap, HTML and
Unicode text in the same clipboard update. Other programs such as Paint can paste the pixels.
A mixed text/image selection offers HTML with embedded image data and plain text; it does not
replace the whole selection with its first picture.

Raster metadata is checked before decoding: at most 16,000 pixels per side, 50 million pixels
and 200 MB of source bytes. Decoder allocations are capped. This permits narrow images wider
than 2,048 pixels without accepting unbounded images. WMF/EMF pictures use the existing bounded
renderer at 96 dpi. Unsupported/corrupt pictures report an error instead of panicking.

An owner marker identifies copies from the current process. While it remains on the clipboard,
regular paste in that window uses the original fragment, retaining picture crop, display size
and object properties. Copying in another application replaces the marker. Another WordCraft
window uses the public HTML/bitmap formats instead of the first window's private fragment.

Native bitmap publication and owner markers are Windows-specific in this change. Other hosts
retain their existing HTML/text services. The source image pixels are exported; the document's
crop/size remain in its HTML/private fragment. Multiple selected pictures are represented in
HTML, not combined into a bitmap.

## Validation

Regression tests cover copy/cut of a 2,049-pixel-wide picture, empty selections, mixed text and
pictures, retaining crop/display size on local paste, oversized/corrupt raster metadata and
EMF rasterization. The explicitly ignored Windows integration test
`windows_image_offer_keeps_pixels_html_and_text_together` writes synthetic clipboard content
and verifies bitmap, HTML, Cyrillic text and owner-marker invalidation together. Run it alone
on an interactive Windows desktop; it changes the current clipboard.

A local Windows preview was exercised by copying a synthetic 2,500 by 500 PNG from WordCraft
and pasting it into Paint. Other receiving applications and operating systems still need
manual coverage before a cross-platform release.
