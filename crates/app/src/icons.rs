//! The icons in front of tree rows, chosen by file type as IntelliJ does: Lucide outlines that come
//! with gpui-kit (ISC), in their type's color.

use std::borrow::Cow;

use gpui_kit::{assets::IconName, *};

use crate::theme::Palette;

// Only these are embedded beyond gpui-kit's own icons, not the whole catalog.
gpui_kit::assets::icon_assets!(
    ExtraIcons,
    [
        File,
        FileCode,
        FileText,
        FileBraces,
        FileCog,
        FileImage,
        FileTerminal,
        FileLock,
        Database,
        Container,
        LocateFixed,
        ChevronsDownUp,
        X,
        Code,
        SquareCode,
        FolderPlus,
        FolderMinus,
        Settings,
        Clock,
    ]
);

/// gpui-kit's icons and subwy's extra ones.
pub struct SubwyAssets;

impl AssetSource for SubwyAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(bytes) = ExtraIcons.load(path)? {
            return Ok(Some(bytes));
        }
        gpui_kit::assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut all = ExtraIcons.list(path)?;
        all.extend(gpui_kit::assets::Assets.list(path)?);
        all.sort();
        all.dedup();
        Ok(all)
    }
}

/// The icon for a folder: open or closed.
pub fn folder_icon(expanded: bool) -> IconName {
    if expanded {
        IconName::FolderOpen
    } else {
        IconName::Folder
    }
}

/// The icon for a file, from its name: a code page, braces for JSON, a cog for settings, and so on.
/// Anything else is a plain page.
pub fn file_icon(name: &str) -> IconName {
    let name = name.to_ascii_lowercase();
    match name.as_str() {
        "dockerfile" | ".dockerignore" | "docker-compose.yml" | "docker-compose.yaml" => {
            return IconName::Container;
        }
        "makefile" | "justfile" => return IconName::FileTerminal,
        _ => {}
    }
    if name.starts_with(".env") || name.starts_with(".git") {
        return IconName::FileCog;
    }
    let extension = name.rsplit_once('.').map_or("", |(_, extension)| extension);
    match extension {
        "rs" | "kt" | "kts" | "java" | "html" | "htm" | "xml" | "ts" | "tsx" | "py" | "css"
        | "scss" | "js" | "jsx" | "mjs" | "cjs" | "go" | "c" | "h" | "cpp" | "swift" => {
            IconName::FileCode
        }
        "sh" | "bash" | "zsh" | "fish" => IconName::FileTerminal,
        "md" | "markdown" | "txt" | "rst" | "csv" => IconName::FileText,
        "json" | "jsonc" => IconName::FileBraces,
        "yaml" | "yml" | "toml" | "ini" | "conf" | "properties" | "gradle" | "editorconfig" => {
            IconName::FileCog
        }
        "sql" => IconName::Database,
        "lock" => IconName::FileLock,
        "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" | "ico" | "bmp" => IconName::FileImage,
        _ => IconName::File,
    }
}

/// The color an icon takes for its type, as IntelliJ colors its icons. Only the skin's own route
/// colors (picked for Kraft's hues: brown, blue, red, green, purple, teal), so no new color comes
/// in and every skin has them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tint {
    Ink,
    Route(usize),
}

impl Tint {
    pub fn color(self, p: &Palette) -> u32 {
        match self {
            Tint::Ink => p.ink,
            Tint::Route(index) => p.route[index % p.route.len()],
        }
    }
}

/// The tint of a folder known by its name, or of a file by its type; `None` keeps the plain icon.
pub fn tint(name: &str, is_dir: bool) -> Option<Tint> {
    let name = name.to_ascii_lowercase();
    if is_dir {
        return match name.as_str() {
            ".github" => Some(Tint::Ink),
            ".gradle" | "gradle" => Some(Tint::Route(5)),
            ".kotlin" | "modules" => Some(Tint::Route(4)),
            "build" | "target" => Some(Tint::Route(2)),
            "docs" | "src" | "services" => Some(Tint::Route(1)),
            "test" | "tests" => Some(Tint::Route(3)),
            _ => None,
        };
    }
    match name.as_str() {
        "dockerfile" | ".dockerignore" | "docker-compose.yml" | "docker-compose.yaml" => {
            return Some(Tint::Route(1));
        }
        "makefile" | "justfile" | "gradlew" | "gradlew.bat" => return Some(Tint::Route(3)),
        _ => {}
    }
    if name.starts_with(".git") {
        return Some(Tint::Route(2));
    }
    // Before the extension: `build.gradle.kts` is Gradle's, not Kotlin's.
    if name.ends_with(".gradle.kts") {
        return Some(Tint::Route(5));
    }
    let extension = name.rsplit_once('.').map_or("", |(_, extension)| extension);
    let route = match extension {
        "rs" | "js" | "jsx" | "mjs" | "cjs" | "json" | "jsonc" | "toml" => 0,
        "ts" | "tsx" | "py" | "c" | "h" | "cpp" | "md" | "markdown" | "sql" => 1,
        "java" | "swift" | "html" | "htm" | "xml" | "yaml" | "yml" => 2,
        "sh" | "bash" | "zsh" | "fish" | "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp"
        | "ico" | "bmp" => 3,
        "kt" | "kts" | "css" | "scss" => 4,
        "go" | "gradle" => 5,
        _ => return None,
    };
    Some(Tint::Route(route))
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that brings gpui's `test` attribute, which shadows the standard one.
    use gpui_kit::assets::IconName;

    use super::{Tint, file_icon, folder_icon, tint};

    #[test]
    fn files_get_the_icon_of_their_type() {
        assert_eq!(file_icon("main.rs"), IconName::FileCode);
        assert_eq!(file_icon("Booking.KT"), IconName::FileCode);
        assert_eq!(file_icon("README.md"), IconName::FileText);
        assert_eq!(file_icon("package.json"), IconName::FileBraces);
        assert_eq!(file_icon("Cargo.lock"), IconName::FileLock);
        assert_eq!(file_icon("query.sql"), IconName::Database);
        assert_eq!(file_icon("Dockerfile"), IconName::Container);
        assert_eq!(file_icon(".gitignore"), IconName::FileCog);
        assert_eq!(file_icon(".env.local"), IconName::FileCog);
        // No extension, or one subwy does not know: a plain page.
        assert_eq!(file_icon("LICENSE"), IconName::File);
        assert_eq!(file_icon("notes.xyz"), IconName::File);
    }

    #[test]
    fn icons_take_the_color_of_their_type() {
        assert_eq!(tint("LoginDto.kt", false), Some(Tint::Route(4)));
        assert_eq!(tint("main.rs", false), Some(Tint::Route(0)));
        assert_eq!(tint("README.md", false), Some(Tint::Route(1)));
        assert_eq!(tint("application.yml", false), Some(Tint::Route(2)));
        assert_eq!(tint("gradlew", false), Some(Tint::Route(3)));
        // Gradle's script, not a Kotlin file.
        assert_eq!(tint("build.gradle.kts", false), Some(Tint::Route(5)));
        assert_eq!(tint(".gitignore", false), Some(Tint::Route(2)));
        assert_eq!(tint("notes.txt", false), None);
        // Folders by name only: a file called `build` is not the build folder.
        assert_eq!(tint("build", true), Some(Tint::Route(2)));
        assert_eq!(tint(".github", true), Some(Tint::Ink));
        assert_eq!(tint("build", false), None);
        assert_eq!(tint("build-logic", true), None);
    }

    #[test]
    fn folders_open() {
        assert_eq!(folder_icon(false), IconName::Folder);
        assert_eq!(folder_icon(true), IconName::FolderOpen);
    }
}
