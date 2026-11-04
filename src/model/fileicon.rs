//! File-type classification for the Explorer.
//!
//! Pure name analysis: exact file names win over extensions, extensions are
//! case-insensitive, and anything unknown falls back to a plain document. The
//! artwork for each [`FileKind`] lives in `assets/icons/material` (MIT, see
//! `assets/icons/README.md`) and is painted by `ui::fileicon`; keeping the table
//! here keeps it unit-testable without any UI type.

use std::path::Path;

/// One file-type icon in the Explorer.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum FileKind {
    Document,
    Rust,
    Python,
    JavaScript,
    TypeScript,
    React,
    Json,
    Markdown,
    Html,
    Css,
    Sass,
    Godot,
    Git,
    Console,
    Yaml,
    Toml,
    Xml,
    Image,
    Svg,
    Pdf,
    Archive,
    Audio,
    Video,
    Database,
    Settings,
    Lock,
    Diff,
    Readme,
    License,
    Makefile,
    Cmake,
    Docker,
    Vue,
    Go,
    C,
    Cpp,
    Java,
    Swift,
    Lua,
    Ruby,
    Php,
    Csharp,
    Font,
}

impl FileKind {
    /// Every kind, in icon-cache order (see [`FileKind::index`]).
    pub const ALL: [FileKind; 43] = [
        FileKind::Document,
        FileKind::Rust,
        FileKind::Python,
        FileKind::JavaScript,
        FileKind::TypeScript,
        FileKind::React,
        FileKind::Json,
        FileKind::Markdown,
        FileKind::Html,
        FileKind::Css,
        FileKind::Sass,
        FileKind::Godot,
        FileKind::Git,
        FileKind::Console,
        FileKind::Yaml,
        FileKind::Toml,
        FileKind::Xml,
        FileKind::Image,
        FileKind::Svg,
        FileKind::Pdf,
        FileKind::Archive,
        FileKind::Audio,
        FileKind::Video,
        FileKind::Database,
        FileKind::Settings,
        FileKind::Lock,
        FileKind::Diff,
        FileKind::Readme,
        FileKind::License,
        FileKind::Makefile,
        FileKind::Cmake,
        FileKind::Docker,
        FileKind::Vue,
        FileKind::Go,
        FileKind::C,
        FileKind::Cpp,
        FileKind::Java,
        FileKind::Swift,
        FileKind::Lua,
        FileKind::Ruby,
        FileKind::Php,
        FileKind::Csharp,
        FileKind::Font,
    ];

    /// Index into the icon cache. Must match [`FileKind::ALL`] order.
    pub(crate) const fn index(self) -> usize {
        match self {
            FileKind::Document => 0,
            FileKind::Rust => 1,
            FileKind::Python => 2,
            FileKind::JavaScript => 3,
            FileKind::TypeScript => 4,
            FileKind::React => 5,
            FileKind::Json => 6,
            FileKind::Markdown => 7,
            FileKind::Html => 8,
            FileKind::Css => 9,
            FileKind::Sass => 10,
            FileKind::Godot => 11,
            FileKind::Git => 12,
            FileKind::Console => 13,
            FileKind::Yaml => 14,
            FileKind::Toml => 15,
            FileKind::Xml => 16,
            FileKind::Image => 17,
            FileKind::Svg => 18,
            FileKind::Pdf => 19,
            FileKind::Archive => 20,
            FileKind::Audio => 21,
            FileKind::Video => 22,
            FileKind::Database => 23,
            FileKind::Settings => 24,
            FileKind::Lock => 25,
            FileKind::Diff => 26,
            FileKind::Readme => 27,
            FileKind::License => 28,
            FileKind::Makefile => 29,
            FileKind::Cmake => 30,
            FileKind::Docker => 31,
            FileKind::Vue => 32,
            FileKind::Go => 33,
            FileKind::C => 34,
            FileKind::Cpp => 35,
            FileKind::Java => 36,
            FileKind::Swift => 37,
            FileKind::Lua => 38,
            FileKind::Ruby => 39,
            FileKind::Php => 40,
            FileKind::Csharp => 41,
            FileKind::Font => 42,
        }
    }
}

/// The file-type icon for `path`: exact names first, then the extension.
pub fn kind_for(path: &Path) -> FileKind {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();

    let named = match name.as_str() {
        "readme" | "readme.md" | "readme.markdown" | "readme.rst" | "readme.txt" => {
            Some(FileKind::Readme)
        }
        "license" | "license.md" | "license.txt" | "licence" | "copying" | "unlicense" => {
            Some(FileKind::License)
        }
        "makefile" | "gnumakefile" | "justfile" => Some(FileKind::Makefile),
        "cmakelists.txt" => Some(FileKind::Cmake),
        "dockerfile" | "containerfile" => Some(FileKind::Docker),
        "cargo.toml" | "cargo.lock" => Some(FileKind::Rust),
        "project.godot" => Some(FileKind::Godot),
        ".gitignore" | ".gitattributes" | ".gitmodules" | ".gitconfig" => Some(FileKind::Git),
        ".editorconfig" | ".env" | "settings.json" => Some(FileKind::Settings),
        ".bashrc" | ".bash_profile" | ".zshrc" | ".profile" => Some(FileKind::Console),
        _ => None,
    };
    if let Some(kind) = named {
        return kind;
    }

    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "rs" => FileKind::Rust,
        "py" | "pyi" | "pyw" => FileKind::Python,
        "js" | "mjs" | "cjs" => FileKind::JavaScript,
        "jsx" => FileKind::React,
        "ts" | "tsx" | "mts" | "cts" => FileKind::TypeScript,
        "json" | "jsonc" | "json5" | "webmanifest" => FileKind::Json,
        "md" | "markdown" | "mdx" | "rst" => FileKind::Markdown,
        "html" | "htm" | "xhtml" => FileKind::Html,
        "css" => FileKind::Css,
        "sass" | "scss" | "less" => FileKind::Sass,
        "gd" | "tscn" | "tres" | "gdshader" | "godot" => FileKind::Godot,
        "sh" | "bash" | "zsh" | "fish" | "ksh" => FileKind::Console,
        "yml" | "yaml" => FileKind::Yaml,
        "toml" => FileKind::Toml,
        "xml" | "plist" | "xaml" => FileKind::Xml,
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "ico" | "icns" | "webp" | "tif" | "tiff" => {
            FileKind::Image
        }
        "svg" => FileKind::Svg,
        "pdf" => FileKind::Pdf,
        "zip" | "tar" | "gz" | "tgz" | "xz" | "bz2" | "7z" | "rar" => FileKind::Archive,
        "mp3" | "wav" | "ogg" | "flac" | "m4a" | "aac" => FileKind::Audio,
        "mp4" | "mov" | "webm" | "mkv" | "avi" => FileKind::Video,
        "sql" | "db" | "sqlite" | "sqlite3" => FileKind::Database,
        "ini" | "cfg" | "conf" | "properties" | "env" => FileKind::Settings,
        "lock" => FileKind::Lock,
        "diff" | "patch" => FileKind::Diff,
        "vue" => FileKind::Vue,
        "go" => FileKind::Go,
        "c" | "h" => FileKind::C,
        "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" => FileKind::Cpp,
        "java" => FileKind::Java,
        "swift" => FileKind::Swift,
        "lua" => FileKind::Lua,
        "rb" => FileKind::Ruby,
        "php" => FileKind::Php,
        "cs" => FileKind::Csharp,
        "ttf" | "otf" | "woff" | "woff2" => FileKind::Font,
        _ => FileKind::Document,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind(name: &str) -> FileKind {
        kind_for(Path::new(name))
    }

    #[test]
    fn cache_order_matches_enum_indices() {
        for (index, kind) in FileKind::ALL.into_iter().enumerate() {
            assert_eq!(kind.index(), index, "{kind:?}");
        }
    }

    #[test]
    fn extensions_map_to_their_type() {
        assert_eq!(kind("main.rs"), FileKind::Rust);
        assert_eq!(kind("notes.md"), FileKind::Markdown);
        assert_eq!(kind("app.gd"), FileKind::Godot);
        assert_eq!(kind("scene.tscn"), FileKind::Godot);
        assert_eq!(kind("index.tsx"), FileKind::TypeScript);
        assert_eq!(kind("widget.jsx"), FileKind::React);
        assert_eq!(kind("data.json"), FileKind::Json);
        assert_eq!(kind("style.scss"), FileKind::Sass);
        assert_eq!(kind("script.sh"), FileKind::Console);
        assert_eq!(kind("types.d.ts"), FileKind::TypeScript);
    }

    #[test]
    fn extensions_are_case_insensitive() {
        assert_eq!(kind("MAIN.RS"), FileKind::Rust);
        assert_eq!(kind("Photo.PNG"), FileKind::Image);
    }

    #[test]
    fn file_names_win_over_extensions() {
        assert_eq!(kind("Cargo.toml"), FileKind::Rust);
        assert_eq!(kind("README.md"), FileKind::Readme);
        assert_eq!(kind("LICENSE"), FileKind::License);
        assert_eq!(kind("Dockerfile"), FileKind::Docker);
        assert_eq!(kind("CMakeLists.txt"), FileKind::Cmake);
        assert_eq!(kind(".gitignore"), FileKind::Git);
        assert_eq!(kind("settings.json"), FileKind::Settings);
        assert_eq!(kind("project.godot"), FileKind::Godot);
    }

    #[test]
    fn dotfiles_and_unknown_types_fall_back_to_document() {
        assert_eq!(kind(".bashrc"), FileKind::Console);
        assert_eq!(kind("notes"), FileKind::Document);
        assert_eq!(kind("archive.unknownext"), FileKind::Document);
    }

    #[test]
    fn directories_classify_by_their_name() {
        // The Explorer checks `is_dir` first; the fallback stays predictable.
        assert_eq!(kind_for(Path::new("src/vendor")), FileKind::Document);
    }
}
