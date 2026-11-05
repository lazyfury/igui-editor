//! Image-asset detection for the read-only preview tab.
//!
//! Pure path analysis only: decoding and texture upload live in
//! `state::images` (they touch disk and the render backend). The set here is
//! deliberately broader than the decoder so binary formats such as JPEG still
//! open as a read-only image tab that explains what *is* supported, instead of
//! failing as binary text. SVG is deliberately **not** in the set: it is text,
//! so it opens in the normal editor as SVG source.

use std::path::Path;

/// Binary image extensions handled by the image preview tab (case-insensitive).
const IMAGE_EXTENSIONS: [&str; 15] = [
    "png", "jpg", "jpeg", "gif", "bmp", "ico", "icns", "webp", "tif", "tiff", "dds", "exr", "hdr",
    "tga", "ktx",
];

/// Whether `path` should open in the image preview instead of the text editor.
pub fn is_image_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            let extension = extension.to_ascii_lowercase();
            IMAGE_EXTENSIONS.contains(&extension.as_str())
        })
}

/// Whether the decoder can display `path` (PNG is the only codec in the engine).
pub fn is_png_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("png"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_extensions_are_case_insensitive_and_svg_stays_text() {
        assert!(is_image_path(Path::new("icon.png")));
        assert!(is_image_path(Path::new("SHOT.PNG")));
        assert!(is_image_path(Path::new("sprite.webp")));
        assert!(is_image_path(Path::new("photo.jpg")));
        // SVG is text: it must open in the editor, not the image tab.
        assert!(!is_image_path(Path::new("logo.svg")));
        assert!(!is_image_path(Path::new("main.rs")));
        assert!(!is_image_path(Path::new("notes.md")));
        assert!(!is_image_path(Path::new("icon")));
    }

    #[test]
    fn only_png_is_decodable() {
        assert!(is_png_path(Path::new("icon.png")));
        assert!(is_png_path(Path::new("ICON.PNG")));
        assert!(!is_png_path(Path::new("photo.jpg")));
        assert!(!is_png_path(Path::new("logo.svg")));
    }
}
