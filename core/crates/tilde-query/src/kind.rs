use tilde_types::{Entry, EntryKind};

/// Broad file categories used by `kind:` filters and the UI's type badges.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Folder,
    App,
    Image,
    Video,
    Audio,
    Pdf,
    Document,
    Code,
    Text,
    Archive,
    Other,
}

impl Kind {
    /// Parses the value of a `kind:` filter, accepting common aliases.
    pub fn parse(s: &str) -> Option<Kind> {
        Some(match s {
            "folder" | "folders" | "dir" | "directory" => Kind::Folder,
            "app" | "apps" | "application" => Kind::App,
            "image" | "images" | "img" | "photo" | "picture" => Kind::Image,
            "video" | "videos" | "movie" => Kind::Video,
            "audio" | "music" | "sound" => Kind::Audio,
            "pdf" => Kind::Pdf,
            "doc" | "docs" | "document" | "documents" => Kind::Document,
            "code" | "source" => Kind::Code,
            "text" | "txt" => Kind::Text,
            "archive" | "archives" | "zip" => Kind::Archive,
            _ => return None,
        })
    }
}

pub fn kind_of(entry: &Entry) -> Kind {
    match entry.kind {
        EntryKind::Folder => return Kind::Folder,
        EntryKind::Package if entry.extension() == "app" => return Kind::App,
        _ => {}
    }
    match entry.extension().as_str() {
        "png" | "jpg" | "jpeg" | "heic" | "heif" | "gif" | "webp" | "tiff" | "tif" | "bmp" | "svg" | "raw" | "dng"
        | "psd" => Kind::Image,
        "mov" | "mp4" | "m4v" | "mkv" | "avi" | "webm" => Kind::Video,
        "mp3" | "m4a" | "wav" | "aiff" | "aif" | "flac" | "ogg" | "aac" => Kind::Audio,
        "pdf" => Kind::Pdf,
        "doc" | "docx" | "pages" | "key" | "keynote" | "numbers" | "xls" | "xlsx" | "ppt" | "pptx" | "rtf" | "rtfd"
        | "odt" | "csv" | "epub" => Kind::Document,
        "rs" | "swift" | "c" | "h" | "m" | "mm" | "cpp" | "hpp" | "cc" | "go" | "py" | "rb" | "js" | "mjs" | "ts"
        | "tsx" | "jsx" | "java" | "kt" | "cs" | "php" | "lua" | "sh" | "zsh" | "bash" | "fish" | "html" | "css"
        | "scss" | "sql" | "toml" | "yaml" | "yml" | "json" | "xml" | "zig" | "ex" | "exs" | "hs" | "ml" | "vim" => {
            Kind::Code
        }
        "txt" | "md" | "markdown" | "log" | "org" | "rst" | "tex" => Kind::Text,
        "zip" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "zst" | "7z" | "rar" | "dmg" | "iso" | "pkg" => Kind::Archive,
        _ => Kind::Other,
    }
}
