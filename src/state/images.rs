//! Decoded image assets and their upload to the render backend.
//!
//! The read-only image tab never reads a file while painting. [`ImageStore`]
//! loads and decodes the active image during `update`, uploads the RGBA8 bytes
//! through a backend-neutral closure and caches the resulting handle. Painters
//! only look up the cached entry.
//!
//! Only PNG is decodable today (`igui_assets` is the engine's PNG codec). Other
//! binary image extensions still open in the image tab, but the tab explains
//! that the format is not displayable instead of showing binary text; SVG is
//! text and never reaches this module (it opens in the editor).

use super::*;
use igui_render::TextureId;

/// Files above this size are not decoded (mirrors the editor's open limit).
pub const MAX_IMAGE_BYTES: u64 = 32 * 1024 * 1024;
/// Guard against RGBA8 buffers larger than 256 MB (`width * height * 4`).
pub const MAX_IMAGE_PIXELS: u64 = 64 * 1024 * 1024;

type Uploader = Box<dyn FnMut(TextureId, u32, u32, &[u8]) -> Result<(), String>>;

/// A decoded image plus its backend handle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadyImage {
    pub texture: TextureId,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug)]
enum Entry {
    Ready(ReadyImage),
    Failed(String),
}

/// What a painter sees for one path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageStatus<'a> {
    Ready(ReadyImage),
    Failed(&'a str),
}

/// Decoded images keyed by canonical path.
#[derive(Default)]
pub struct ImageStore {
    entries: HashMap<PathBuf, Entry>,
    uploader: Option<Uploader>,
    next_id: u32,
}

impl std::fmt::Debug for ImageStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImageStore")
            .field("entries", &self.entries.len())
            .field("uploader", &self.uploader.is_some())
            .finish()
    }
}

impl ImageStore {
    /// Installs the texture upload path (the windowed host owns the backend;
    /// headless runs leave it unset and still record `DrawImage`).
    pub fn set_uploader(&mut self, uploader: Uploader) {
        self.uploader = Some(uploader);
    }

    /// The cached state for `path`, if it has been loaded.
    pub fn status(&self, path: &Path) -> Option<ImageStatus<'_>> {
        self.entries.get(path).map(|entry| match entry {
            Entry::Ready(image) => ImageStatus::Ready(*image),
            Entry::Failed(message) => ImageStatus::Failed(message.as_str()),
        })
    }

    /// Drops a cached image so the next load re-reads it (external edit).
    pub fn invalidate(&mut self, path: &Path) {
        self.entries.remove(path);
    }

    /// Drops every decoded image (project switch/close). The uploader stays
    /// installed so the next open can upload again.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Keeps only images for paths that are still open (tab close).
    pub fn prune(&mut self, open: &HashSet<PathBuf>) {
        self.entries.retain(|path, _| open.contains(path));
    }

    /// Loads and decodes `path` when it is not cached. Returns whether the
    /// cache changed (the caller marks the frame dirty).
    pub fn load(&mut self, path: &Path) -> bool {
        if self.entries.contains_key(path) {
            return false;
        }
        let entry = self.decode(path);
        self.entries.insert(path.to_path_buf(), entry);
        true
    }

    fn decode(&mut self, path: &Path) -> Entry {
        if !crate::model::image::is_png_path(path) {
            return Entry::Failed(
                "binary image — cannot be opened; PNG is the only preview format".into(),
            );
        }
        let metadata = match std::fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(error) => return Entry::Failed(format!("cannot read image: {error}")),
        };
        if metadata.len() > MAX_IMAGE_BYTES {
            return Entry::Failed(format!(
                "image is too large ({} MB limit)",
                MAX_IMAGE_BYTES / (1024 * 1024)
            ));
        }
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) => return Entry::Failed(format!("cannot read image: {error}")),
        };
        let image = match igui_assets::decode_png(&bytes) {
            Ok(image) => image,
            Err(error) => return Entry::Failed(error.to_string()),
        };
        let pixels = u64::from(image.width()) * u64::from(image.height());
        if pixels == 0 || pixels > MAX_IMAGE_PIXELS {
            return Entry::Failed("image dimensions are not supported".into());
        }
        let texture = TextureId::new(self.next_id.max(1));
        self.next_id = texture.raw().saturating_add(1);
        if let Some(uploader) = &mut self.uploader {
            if let Err(error) = uploader(texture, image.width(), image.height(), image.rgba8()) {
                return Entry::Failed(format!("texture upload failed: {error}"));
            }
        }
        Entry::Ready(ReadyImage {
            texture,
            width: image.width(),
            height: image.height(),
        })
    }
}

impl AppState {
    /// Loads the active image tab's texture before paint. The image view only
    /// reads [`ImageStore::status`], so disk and GPU work stay out of `ui/`.
    pub fn tick_image_preview(&mut self) {
        if !self.settings.image_preview {
            return;
        }
        let path = self
            .ws
            .active_doc()
            .filter(|doc| doc.is_image_view())
            .and_then(|doc| doc.path().map(Path::to_path_buf));
        let Some(path) = path else {
            return;
        };
        if !self.images.load(&path) {
            return;
        }
        self.dirty = true;
        let failed = match self.images.status(&path) {
            Some(ImageStatus::Failed(message)) => Some(message.to_string()),
            _ => None,
        };
        if let Some(message) = failed {
            let name = self
                .ws
                .active_doc()
                .map(|doc| doc.name().to_string())
                .unwrap_or_else(|| "image".into());
            // Unsupported/corrupt assets are a property of the file, not an
            // app failure: show a neutral status and let the tab explain it.
            self.status(format!("{name}: {message}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2x2 RGBA PNG (red/green top row, blue/white bottom row).
    const PNG_2X2: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x02, 0x08, 0x06, 0x00, 0x00, 0x00, 0x72,
        0xb6, 0x0d, 0x24, 0x00, 0x00, 0x00, 0x12, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0xf8,
        0xcf, 0xc0, 0xf0, 0x1f, 0x0c, 0x81, 0x34, 0x18, 0x00, 0x00, 0x49, 0xc8, 0x09, 0xf7, 0x03,
        0xd9, 0x64, 0xf1, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "igui-editor-image-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_png_is_decoded_once_and_cached() {
        let dir = temp_dir("png");
        let path = dir.join("icon.png");
        std::fs::write(&path, PNG_2X2).unwrap();
        let mut images = ImageStore::default();
        assert!(images.load(&path));
        match images.status(&path) {
            Some(ImageStatus::Ready(image)) => {
                assert_eq!((image.width, image.height), (2, 2));
                assert!(image.texture.is_valid());
            }
            other => panic!("expected a ready image, got {other:?}"),
        }
        assert!(!images.load(&path), "second load must use the cache");
        images.prune(&HashSet::new());
        assert!(images.status(&path).is_none(), "prune drops closed tabs");
        images.load(&path);
        images.invalidate(&path);
        assert!(images.status(&path).is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_unsupported_format_explains_itself() {
        let dir = temp_dir("jpg");
        let path = dir.join("photo.jpg");
        std::fs::write(&path, [0xff, 0xd8, 0xff, 0xe0]).unwrap();
        let mut images = ImageStore::default();
        assert!(images.load(&path));
        match images.status(&path) {
            Some(ImageStatus::Failed(message)) => {
                assert!(message.contains("binary"));
                assert!(message.contains("PNG"));
            }
            other => panic!("expected a failed image, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_corrupt_png_is_reported_without_panicking() {
        let dir = temp_dir("corrupt");
        let path = dir.join("broken.png");
        std::fs::write(&path, b"not a png").unwrap();
        let mut images = ImageStore::default();
        assert!(images.load(&path));
        assert!(matches!(images.status(&path), Some(ImageStatus::Failed(_))));
        let _ = std::fs::remove_dir_all(dir);
    }
}
