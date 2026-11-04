//! Directory listings and text files, read and written the way the user left them.

use std::{
    io::{self, Read as _, Write as _},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::SystemTime,
};

use ignore::{
    Match,
    gitignore::{Gitignore, GitignoreBuilder},
};

/// Files above this size are not opened.
pub const MAX_OPEN_BYTES: u64 = 50 << 20;
/// Files above this size open without syntax highlighting (it costs about 25 MB of memory per MB).
pub const MAX_HIGHLIGHT_BYTES: u64 = 2 << 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EntryKind {
    Dir,
    File,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirEntry {
    /// The name as shown and copied (NFC).
    pub name: String,
    pub kind: EntryKind,
    /// Matched by a `.gitignore`; shown dimmed.
    pub ignored: bool,
    /// A symlinked folder that leads back to the folder holding it or to one above it.
    /// Expanding it would repeat the tree forever, so the tree keeps it closed.
    pub loops: bool,
}

/// Lists one directory of the tree: folders first, then files, by case-insensitive name.
/// `dir` is relative to `root`. `.git` and `.DS_Store` are left out. Entries matched by a
/// `.gitignore` from `root` down to `dir`, or by `.git/info/exclude`, are kept and marked.
pub fn list_dir(root: &Path, dir: &Path) -> io::Result<Vec<DirEntry>> {
    let abs_dir = root.join(dir);
    let ignores = Ignores::for_dir(root, dir);
    let ancestors = canonical_ancestors(root, dir);
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(&abs_dir)? {
        let entry = entry?;
        let name = crate::paths::display_name(&entry.file_name());
        // `.subwy-save-*.tmp` lives only for the instant of an atomic save.
        if name == ".git" || name == ".DS_Store" || name.starts_with(".subwy-save-") {
            continue;
        }
        let path = entry.path();
        // `is_dir` follows symlinks, so a link to a folder lists and expands as a folder.
        let is_dir = path.is_dir();
        let loops = is_dir
            && entry.file_type().is_ok_and(|t| t.is_symlink())
            && std::fs::canonicalize(&path)
                .is_ok_and(|target| ancestors.iter().any(|a| a.starts_with(&target)));
        entries.push(DirEntry {
            // Matched by the NFC name: git on macOS precomposes names the same way.
            ignored: ignores.matched(&abs_dir.join(&name), is_dir),
            name,
            kind: if is_dir {
                EntryKind::Dir
            } else {
                EntryKind::File
            },
            loops,
        });
    }
    sort_entries(&mut entries);
    Ok(entries)
}

pub(crate) fn sort_entries(entries: &mut [DirEntry]) {
    entries.sort_by(compare_entries);
}

/// The tree's order: folders first, then case-insensitive name, then the exact name so the
/// order never depends on how the disk returned the entries.
pub(crate) fn compare_entries(a: &DirEntry, b: &DirEntry) -> std::cmp::Ordering {
    (a.kind != EntryKind::Dir)
        .cmp(&(b.kind != EntryKind::Dir))
        .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        .then_with(|| a.name.cmp(&b.name))
}

/// The ignore rules in force inside one folder.
struct Ignores {
    /// Outermost first; a deeper `.gitignore` overrides the ones above it.
    matchers: Vec<Gitignore>,
    /// The folder itself, or one above it, is ignored. Git cannot re-include anything inside
    /// an ignored folder, so every entry is ignored too.
    inherited: bool,
}

impl Ignores {
    fn for_dir(root: &Path, dir: &Path) -> Self {
        let mut exclude = GitignoreBuilder::new(root);
        exclude.add(root.join(".git/info/exclude"));
        let mut ignores = Self {
            matchers: exclude.build().into_iter().collect(),
            inherited: false,
        };
        ignores.push(root);
        let mut current = root.to_path_buf();
        for part in dir.components() {
            current.push(part);
            ignores.inherited = ignores.matched(&current, true);
            ignores.push(&current);
        }
        ignores
    }

    fn push(&mut self, dir: &Path) {
        // A missing or unreadable `.gitignore` simply adds no rules.
        let (matcher, _) = Gitignore::new(dir.join(".gitignore"));
        if !matcher.is_empty() {
            self.matchers.push(matcher);
        }
    }

    fn matched(&self, path: &Path, is_dir: bool) -> bool {
        if self.inherited {
            return true;
        }
        for matcher in self.matchers.iter().rev() {
            match matcher.matched(path, is_dir) {
                Match::Ignore(_) => return true,
                Match::Whitelist(_) => return false,
                Match::None => {}
            }
        }
        false
    }
}

/// The real paths of the open folder and of every folder down to `dir`: the tree ancestors of
/// the entries listed in `dir`.
fn canonical_ancestors(root: &Path, dir: &Path) -> Vec<PathBuf> {
    let mut current = root.to_path_buf();
    let mut ancestors: Vec<PathBuf> = std::fs::canonicalize(&current).into_iter().collect();
    for part in dir.components() {
        current.push(part);
        ancestors.extend(std::fs::canonicalize(&current));
    }
    ancestors
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum LineEnding {
    #[default]
    Lf,
    Crlf,
}

/// When a file was last seen on disk, to notice changes made outside subwy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileStamp {
    pub modified: SystemTime,
    pub len: u64,
}

#[derive(Clone, Debug)]
pub struct TextFile {
    /// The text with `\n` line endings.
    pub text: String,
    pub line_ending: LineEnding,
    pub bom: bool,
    /// Both `\n` and `\r\n` appear; saving would change lines the user did not touch, so open read-only.
    pub mixed_line_endings: bool,
    pub stamp: FileStamp,
}

#[derive(Debug)]
pub enum ReadError {
    Binary,
    NotUtf8,
    TooLarge(u64),
    Io(io::Error),
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReadError::Binary => write!(f, "binary file"),
            ReadError::NotUtf8 => write!(f, "not UTF-8"),
            ReadError::TooLarge(len) => write!(f, "too large ({} MB)", len >> 20),
            ReadError::Io(err) => write!(f, "{err}"),
        }
    }
}

impl From<io::Error> for ReadError {
    fn from(err: io::Error) -> Self {
        ReadError::Io(err)
    }
}

pub fn stamp(path: &Path) -> io::Result<FileStamp> {
    let meta = std::fs::metadata(path)?;
    Ok(FileStamp {
        modified: meta.modified()?,
        len: meta.len(),
    })
}

/// How far to look for a NUL byte to call a file binary (the same probe git uses).
const BINARY_PROBE_BYTES: usize = 8 << 10;
const UTF8_BOM: &[u8] = b"\xEF\xBB\xBF";

/// Reads a file for editing. Binary, non-UTF-8 and oversized files are refused. A UTF-8 BOM is
/// stripped and `\r\n` becomes `\n`; both are remembered so saving writes them back.
pub fn read_text(path: &Path) -> Result<TextFile, ReadError> {
    let file = std::fs::File::open(path)?;
    // The stamp comes from the same open file as the text, so it describes what was read.
    let meta = file.metadata()?;
    let stamp = FileStamp {
        modified: meta.modified()?,
        len: meta.len(),
    };
    if stamp.len > MAX_OPEN_BYTES {
        return Err(ReadError::TooLarge(stamp.len));
    }
    let mut bytes = Vec::with_capacity(stamp.len as usize);
    // The file may grow after the size check; never read past the limit.
    file.take(MAX_OPEN_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_OPEN_BYTES {
        return Err(ReadError::TooLarge(bytes.len() as u64));
    }
    if bytes[..bytes.len().min(BINARY_PROBE_BYTES)].contains(&0) {
        return Err(ReadError::Binary);
    }
    let bom = bytes.starts_with(UTF8_BOM);
    if bom {
        bytes.drain(..UTF8_BOM.len());
    }
    let text = String::from_utf8(bytes).map_err(|_| ReadError::NotUtf8)?;
    let line_ending = match text.find('\n') {
        Some(i) if text[..i].ends_with('\r') => LineEnding::Crlf,
        _ => LineEnding::Lf,
    };
    let crlf = text.matches("\r\n").count();
    let mixed_line_endings = crlf > 0 && crlf < text.matches('\n').count();
    let text = if crlf > 0 {
        text.replace("\r\n", "\n")
    } else {
        text
    };
    Ok(TextFile {
        text,
        line_ending,
        bom,
        mixed_line_endings,
        stamp,
    })
}

/// Writes `text` (with `\n` endings) back with the file's own line ending and BOM.
///
/// The bytes go to a temporary file in the same folder, reach the disk, and then replace the
/// file in one rename: a crash or a full disk leaves the old file whole, never half written.
/// The file keeps its permissions, and a symlink is written through to its target.
pub fn write_text(
    path: &Path,
    text: &str,
    line_ending: LineEnding,
    bom: bool,
) -> io::Result<FileStamp> {
    let mut out = String::with_capacity(text.len() + 3);
    if bom {
        out.push('\u{feff}');
    }
    match line_ending {
        LineEnding::Lf => out.push_str(text),
        LineEnding::Crlf => out.push_str(&text.replace('\n', "\r\n")),
    }
    let target = write_target(path);
    let permissions = match std::fs::metadata(&target) {
        Ok(meta) if meta.permissions().readonly() => {
            // A plain write would fail here; the rename would quietly replace the file.
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "the file is read-only",
            ));
        }
        Ok(meta) => Some(meta.permissions()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => None,
        Err(err) => return Err(err),
    };
    let (temp, file) = create_temp_beside(&target)?;
    if let Err(err) = replace_with(file, &temp, &target, out.as_bytes(), permissions) {
        let _ = std::fs::remove_file(&temp);
        return Err(err);
    }
    stamp(&target)
}

fn replace_with(
    mut file: std::fs::File,
    temp: &Path,
    target: &Path,
    bytes: &[u8],
    permissions: Option<std::fs::Permissions>,
) -> io::Result<()> {
    if let Some(permissions) = permissions {
        file.set_permissions(permissions)?;
    }
    file.write_all(bytes)?;
    file.sync_all()?;
    std::fs::rename(temp, target)
}

/// A new, empty file next to `target`, named so a leftover from a crash is recognizable.
fn create_temp_beside(target: &Path) -> io::Result<(PathBuf, std::fs::File)> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = target.parent().unwrap_or(Path::new(""));
    loop {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let temp = dir.join(format!(".subwy-save-{}-{n}.tmp", std::process::id()));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
        {
            Ok(file) => return Ok((temp, file)),
            // A leftover from an earlier run with the same process id.
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err),
        }
    }
}

/// Writes `bytes` to a new file in the folder `dir`, made if missing: `{stem}-1.{ext}`, or the
/// first of `-2`, `-3`, ... that is free. Returns its path; nothing that exists is replaced.
pub fn write_new(dir: &Path, stem: &str, ext: &str, bytes: &[u8]) -> io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let mut n = 1;
    loop {
        let path = dir.join(format!("{stem}-{n}.{ext}"));
        // `create_new` checks and creates in one step, so a file that appears meanwhile is kept.
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                if let Err(err) = file.write_all(bytes).and_then(|()| file.sync_all()) {
                    // Only the half-written file this call made, as a failed save drops its
                    // temporary file.
                    let _ = std::fs::remove_file(&path);
                    return Err(err);
                }
                return Ok(path);
            }
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => n += 1,
            Err(err) => return Err(err),
        }
    }
}

/// Where a picture dropped on a Markdown file in the folder `dir` is linked from, relative to
/// `dir` with `/` between folders: its own place when it is already under `dir`, else a copy in
/// the images folder there, under a name nothing has (Finder's ` copy`).
pub fn place_picture(dir: &Path, picture: &Path) -> Result<String, NameError> {
    let slashed = |path: &Path| {
        path.components()
            .map(|part| part.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/")
    };
    if let Ok(inside) = picture.strip_prefix(dir) {
        return Ok(slashed(inside));
    }
    let images = dir.join(crate::markdown::IMAGE_DIR);
    std::fs::create_dir_all(&images)?;
    let name = picture
        .file_name()
        .ok_or(NameError::Invalid)?
        .to_string_lossy()
        .into_owned();
    let copy = copy_into(
        picture,
        &images,
        &free_name(&images, &name, EntryKind::File),
    )?;
    Ok(slashed(copy.strip_prefix(dir).unwrap_or(&copy)))
}

/// The path a file should be written to: a symlink's target, so saving never replaces the link.
pub fn write_target(path: &Path) -> PathBuf {
    if let Ok(target) = std::fs::canonicalize(path) {
        return target;
    }
    // A missing file, or a link whose target is gone: follow the links by hand so saving
    // recreates the target instead of replacing the link. The limit stops a link cycle.
    let mut path = path.to_path_buf();
    for _ in 0..MAX_SYMLINK_HOPS {
        let Ok(next) = std::fs::read_link(&path) else {
            break;
        };
        path = match path.parent() {
            Some(dir) => dir.join(next),
            None => next,
        };
    }
    path
}

/// The same limit macOS puts on links followed while resolving one path.
const MAX_SYMLINK_HOPS: usize = 32;

/// Why a name typed into the tree was not used. The text is what the tree shows after the input.
#[derive(Debug)]
pub enum NameError {
    Empty,
    /// `.` or `..`, an empty folder name (`a//b`), or a path that starts at `/`.
    Invalid,
    /// Something by that name is already there; subwy never replaces it.
    Exists,
    /// A folder on the way is a file.
    NotAFolder(String),
    /// A folder would go inside itself.
    IntoItself,
    Io(io::Error),
}

impl std::fmt::Display for NameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NameError::Empty => write!(f, "Type a name"),
            NameError::Invalid => write!(f, "Not a valid name"),
            NameError::Exists => write!(f, "Already exists"),
            NameError::NotAFolder(name) => write!(f, "“{name}” is a file"),
            NameError::IntoItself => write!(f, "A folder cannot go inside itself"),
            NameError::Io(err) => write!(f, "{err}"),
        }
    }
}

impl From<io::Error> for NameError {
    fn from(err: io::Error) -> Self {
        NameError::Io(err)
    }
}

/// One file or folder name: not empty, and not a step in place or up.
fn valid_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".."
}

/// Creates what was typed inside the folder `dir` (relative to `root`): a file, or a folder
/// when the text ends in `/`. `a/b/c.md` also creates the folders `a` and `a/b` on the way.
/// Returns the new item relative to `root`. Nothing that exists is ever replaced.
pub fn create(root: &Path, dir: &Path, typed: &str) -> Result<(PathBuf, EntryKind), NameError> {
    let typed = typed.trim();
    let (name, kind) = match typed.strip_suffix('/') {
        Some(name) => (name, EntryKind::Dir),
        None => (typed, EntryKind::File),
    };
    if name.is_empty() {
        return Err(NameError::Empty);
    }
    let parts: Vec<&str> = name.split('/').collect();
    if !parts.iter().all(|part| valid_name(part)) {
        return Err(NameError::Invalid);
    }
    // Name a file that stands where a folder should be, rather than the system's "File exists".
    let mut on_the_way = root.join(dir);
    for part in &parts[..parts.len() - 1] {
        on_the_way.push(part);
        if std::fs::symlink_metadata(&on_the_way).is_ok() && !on_the_way.is_dir() {
            return Err(NameError::NotAFolder(part.to_string()));
        }
    }
    let rel = dir.join(name);
    let abs = root.join(&rel);
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let made = match kind {
        EntryKind::Dir => std::fs::create_dir(&abs),
        // `create_new` checks and creates in one step, so a file that appears meanwhile is kept.
        EntryKind::File => std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&abs)
            .map(drop),
    };
    match made {
        Ok(()) => Ok((rel, kind)),
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => Err(NameError::Exists),
        Err(err) => Err(err.into()),
    }
}

/// Renames the item at `from` (relative to `root`) inside its folder and returns its new path.
/// Refuses a name another item has; changing only the case of the name is allowed.
pub fn rename(root: &Path, from: &Path, new_name: &str) -> Result<PathBuf, NameError> {
    use std::os::unix::fs::MetadataExt as _;
    let new_name = new_name.trim();
    if new_name.is_empty() {
        return Err(NameError::Empty);
    }
    if !valid_name(new_name) || new_name.contains('/') {
        return Err(NameError::Invalid);
    }
    let to = from.with_file_name(new_name);
    if to == from {
        return Ok(to);
    }
    let (abs_from, abs_to) = (root.join(from), root.join(&to));
    // `rename` would silently replace the other item, so look first. On a case-insensitive
    // disk `A.md` is `a.md` itself, which is no other item.
    if let Ok(there) = std::fs::symlink_metadata(&abs_to) {
        let here = std::fs::symlink_metadata(&abs_from)?;
        if (there.dev(), there.ino()) != (here.dev(), here.ino()) {
            return Err(NameError::Exists);
        }
    }
    std::fs::rename(&abs_from, &abs_to)?;
    Ok(to)
}

/// What F5 / F6's command line says, as the tree names paths: the folder to go into and, for a
/// single item, a new name. A trailing slash, or several items, name a folder only.
pub fn typed_target(typed: &str, several: bool) -> Result<(PathBuf, Option<String>), NameError> {
    let typed = typed.trim();
    if typed.is_empty() {
        return Err(NameError::Empty);
    }
    if typed.starts_with('/') {
        return Err(NameError::Invalid);
    }
    let (folder, name) = match typed.strip_suffix('/') {
        Some(folder) => (folder, None),
        None if several => (typed, None),
        None => match typed.rsplit_once('/') {
            Some((folder, name)) => (folder, Some(name)),
            None => ("", Some(typed)),
        },
    };
    let parts: Vec<&str> = folder.split('/').filter(|_| !folder.is_empty()).collect();
    if !parts.iter().all(|part| valid_name(part)) || name.is_some_and(|name| !valid_name(name)) {
        return Err(NameError::Invalid);
    }
    Ok((parts.iter().collect(), name.map(str::to_string)))
}

/// The name a copy of `name` takes in the folder `dir` without replacing anything: the name itself
/// when it is free, else Finder's `notes copy.md`, `notes copy 2.md`. A folder's name, or a name
/// with nothing before its dot (`.env`), has no extension to keep.
pub fn free_name(dir: &Path, name: &str, kind: EntryKind) -> String {
    let (stem, ext) = match name.rfind('.') {
        Some(dot) if dot > 0 && kind == EntryKind::File => name.split_at(dot),
        _ => (name, ""),
    };
    std::iter::once(name.to_string())
        .chain((1..).map(|n| match n {
            1 => format!("{stem} copy{ext}"),
            n => format!("{stem} copy {n}{ext}"),
        }))
        .find(|candidate| std::fs::symlink_metadata(dir.join(candidate)).is_err())
        .unwrap_or_else(|| name.to_string())
}

/// Copies the file or folder `from` into the folder `dir` as `name` (absolute paths) and returns
/// the copy. A folder is copied whole and a link as a link, so a link that loops is no trouble.
/// Nothing that exists is replaced, and a folder never goes inside itself.
pub fn copy_into(from: &Path, dir: &Path, name: &str) -> Result<PathBuf, NameError> {
    let to = destination(from, dir, name)?;
    if std::fs::symlink_metadata(&to).is_ok() {
        return Err(NameError::Exists);
    }
    copy_tree(from, &to)?;
    Ok(to)
}

/// Moves the file or folder `from` into the folder `dir` as `name` (absolute paths) and returns where
/// it went. Nothing that exists is replaced; moving to where it already is changes nothing. Across
/// disks it is copied, and then the original goes to the Trash rather than being deleted.
pub fn move_into(from: &Path, dir: &Path, name: &str) -> Result<PathBuf, NameError> {
    use std::os::unix::fs::MetadataExt as _;
    let to = destination(from, dir, name)?;
    if to == from {
        return Ok(to);
    }
    // On a case-insensitive disk `A.md` is `a.md` itself, which is no other item.
    if let Ok(there) = std::fs::symlink_metadata(&to) {
        let here = std::fs::symlink_metadata(from)?;
        if (there.dev(), there.ino()) != (here.dev(), here.ino()) {
            return Err(NameError::Exists);
        }
    }
    match std::fs::rename(from, &to) {
        Ok(()) => Ok(to),
        Err(err) if err.kind() == io::ErrorKind::CrossesDevices => {
            copy_tree(from, &to)?;
            move_to_trash(from)?;
            Ok(to)
        }
        Err(err) => Err(err.into()),
    }
}

/// Where `from` lands in `dir` as `name`, after checking the name and that a folder is not
/// going inside itself.
fn destination(from: &Path, dir: &Path, name: &str) -> Result<PathBuf, NameError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(NameError::Empty);
    }
    if !valid_name(name) || name.contains('/') {
        return Err(NameError::Invalid);
    }
    if dir.starts_with(from) {
        return Err(NameError::IntoItself);
    }
    Ok(dir.join(name))
}

/// Copies a file, a link, or a folder with everything in it, keeping permissions.
fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    let meta = std::fs::symlink_metadata(from)?;
    if meta.file_type().is_symlink() {
        std::os::unix::fs::symlink(std::fs::read_link(from)?, to)
    } else if meta.is_dir() {
        std::fs::create_dir(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_tree(&entry.path(), &to.join(entry.file_name()))?;
        }
        std::fs::set_permissions(to, meta.permissions())
    } else {
        std::fs::copy(from, to).map(drop)
    }
}

/// Moves an item of the open folder (`path`, relative to `root`) to the Trash.
pub fn trash(root: &Path, path: &Path) -> io::Result<()> {
    // The open folder itself is never a row, so an empty path can only be a mistake.
    if path.as_os_str().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the open folder cannot go to the Trash",
        ));
    }
    move_to_trash(&root.join(path))
}

fn move_to_trash(path: &Path) -> io::Result<()> {
    let mut trash = trash::TrashContext::default();
    // Finder's way asks for permission to control Finder and plays a sound; the system call
    // needs neither. Its cost: some macOS versions offer no "Put Back" for the item.
    #[cfg(target_os = "macos")]
    {
        use trash::macos::{DeleteMethod, TrashContextExtMacos as _};
        trash.set_delete_method(DeleteMethod::NsFileManager);
    }
    trash.delete(path).map_err(|err| {
        let message = match err {
            trash::Error::Os { description, .. } | trash::Error::Unknown { description } => {
                description
            }
            other => other.to_string(),
        };
        io::Error::other(message)
    })
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use super::*;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn names(entries: &[DirEntry]) -> Vec<&str> {
        entries.iter().map(|e| e.name.as_str()).collect()
    }

    fn entry<'a>(entries: &'a [DirEntry], name: &str) -> &'a DirEntry {
        entries.iter().find(|e| e.name == name).unwrap()
    }

    #[test]
    fn lists_folders_first_then_names_ignoring_case() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        for name in ["b.md", "C.md", "a.md", ".env", ".DS_Store", ".gitignore"] {
            write(&root.join(name), "");
        }
        for name in ["zeta", "Alpha", ".git", ".github"] {
            std::fs::create_dir(root.join(name)).unwrap();
        }
        let entries = list_dir(root, Path::new("")).unwrap();
        assert_eq!(
            names(&entries),
            [
                ".github",
                "Alpha",
                "zeta",
                ".env",
                ".gitignore",
                "a.md",
                "b.md",
                "C.md"
            ]
        );
        assert_eq!(entry(&entries, "zeta").kind, EntryKind::Dir);
        assert_eq!(entry(&entries, "a.md").kind, EntryKind::File);
    }

    #[test]
    fn names_equal_but_for_case_sort_by_exact_name() {
        // APFS is usually case-insensitive, so this case only arrives from other disks.
        let file = |name: &str| DirEntry {
            name: name.to_string(),
            kind: EntryKind::File,
            ignored: false,
            loops: false,
        };
        let mut entries = vec![file("b.md"), file("B.md"), file("a.md")];
        sort_entries(&mut entries);
        assert_eq!(names(&entries), ["a.md", "B.md", "b.md"]);
    }

    #[test]
    fn decomposed_hangul_names_list_composed() {
        let root = tempfile::tempdir().unwrap();
        let nfd = "\u{1112}\u{1161}\u{11ab}\u{1100}\u{1173}\u{11af}.md";
        write(&root.path().join(nfd), "");
        let entries = list_dir(root.path(), Path::new("")).unwrap();
        assert_eq!(names(&entries), ["\u{d55c}\u{ae00}.md"]);
    }

    #[test]
    fn marks_gitignored_entries_but_keeps_them() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        write(&root.join(".gitignore"), "target/\n*.log\n!keep.log\n");
        write(&root.join(".git/info/exclude"), "secret.txt\n");
        write(&root.join("docs/.gitignore"), "draft.md\n!important.log\n");
        for name in [
            "a.log",
            "keep.log",
            "secret.txt",
            "readme.md",
            "target/debug/leaf",
            "docs/draft.md",
            "docs/a.log",
            "docs/important.log",
            "docs/guide.md",
        ] {
            write(&root.join(name), "");
        }
        let ignored = |dir: &str| -> Vec<(String, bool)> {
            list_dir(root, Path::new(dir))
                .unwrap()
                .into_iter()
                .map(|e| (e.name, e.ignored))
                .collect()
        };
        let pairs = |list: &[(&str, bool)]| -> Vec<(String, bool)> {
            list.iter().map(|(n, i)| (n.to_string(), *i)).collect()
        };
        assert_eq!(
            ignored(""),
            pairs(&[
                ("docs", false),
                ("target", true),
                (".gitignore", false),
                ("a.log", true),
                ("keep.log", false),
                ("readme.md", false),
                ("secret.txt", true),
            ])
        );
        assert_eq!(
            ignored("docs"),
            pairs(&[
                (".gitignore", false),
                ("a.log", true),
                ("draft.md", true),
                ("guide.md", false),
                ("important.log", false),
            ])
        );
        // Everything inside an ignored folder is ignored, whatever its name.
        assert_eq!(ignored("target"), pairs(&[("debug", true)]));
        assert_eq!(ignored("target/debug"), pairs(&[("leaf", true)]));
    }

    #[test]
    fn symlinked_folders_that_lead_back_up_are_marked() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root = root.path();
        write(&root.join("a/note.md"), "");
        write(&root.join("b/note.md"), "");
        write(&outside.path().join("shared.md"), "");
        symlink(root, root.join("self")).unwrap();
        symlink("..", root.join("a/up")).unwrap();
        symlink("../b", root.join("a/to_b")).unwrap();
        symlink("../a", root.join("b/to_a")).unwrap();
        symlink("note.md", root.join("a/note_link.md")).unwrap();
        symlink(outside.path(), root.join("outside")).unwrap();

        let top = list_dir(root, Path::new("")).unwrap();
        assert!(entry(&top, "self").loops);
        assert!(!entry(&top, "outside").loops);
        assert_eq!(entry(&top, "outside").kind, EntryKind::Dir);
        assert!(!entry(&top, "a").loops);

        let a = list_dir(root, Path::new("a")).unwrap();
        assert!(entry(&a, "up").loops);
        assert!(!entry(&a, "to_b").loops);
        assert_eq!(entry(&a, "note_link.md").kind, EntryKind::File);
        assert!(!entry(&a, "note_link.md").loops);

        // `a/to_b/to_a` is `a` again: an ancestor in the tree, though not on disk.
        let through = list_dir(root, Path::new("a/to_b")).unwrap();
        assert!(entry(&through, "to_a").loops);
    }

    #[test]
    fn reads_lf_text_as_is() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.md");
        write(&path, "one\ntwo\n");
        let file = read_text(&path).unwrap();
        assert_eq!(file.text, "one\ntwo\n");
        assert_eq!(file.line_ending, LineEnding::Lf);
        assert!(!file.bom);
        assert!(!file.mixed_line_endings);
        assert_eq!(file.stamp, stamp(&path).unwrap());
    }

    #[test]
    fn reads_crlf_text_with_lf_inside() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        write(&path, "one\r\ntwo\r\nthree");
        let file = read_text(&path).unwrap();
        assert_eq!(file.text, "one\ntwo\nthree");
        assert_eq!(file.line_ending, LineEnding::Crlf);
        assert!(!file.mixed_line_endings);
    }

    #[test]
    fn text_without_a_newline_is_lf() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        write(&path, "one line");
        let file = read_text(&path).unwrap();
        assert_eq!(file.text, "one line");
        assert_eq!(file.line_ending, LineEnding::Lf);
    }

    #[test]
    fn mixed_line_endings_take_the_first_and_are_flagged() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        write(&path, "one\ntwo\r\nthree\n");
        let file = read_text(&path).unwrap();
        assert_eq!(file.text, "one\ntwo\nthree\n");
        assert_eq!(file.line_ending, LineEnding::Lf);
        assert!(file.mixed_line_endings);

        write(&path, "one\r\ntwo\n");
        let file = read_text(&path).unwrap();
        assert_eq!(file.line_ending, LineEnding::Crlf);
        assert!(file.mixed_line_endings);
    }

    #[test]
    fn strips_and_remembers_a_bom() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        write(&path, "\u{feff}hello\r\n");
        let file = read_text(&path).unwrap();
        assert_eq!(file.text, "hello\n");
        assert!(file.bom);
        assert_eq!(file.line_ending, LineEnding::Crlf);
    }

    #[test]
    fn a_nul_in_the_first_8_kib_means_binary() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.bin");
        std::fs::write(&path, b"PK\x03\x04\x00\x00").unwrap();
        assert!(matches!(read_text(&path), Err(ReadError::Binary)));

        let mut late = vec![b'a'; BINARY_PROBE_BYTES];
        late.push(0);
        std::fs::write(&path, &late).unwrap();
        assert_eq!(read_text(&path).unwrap().text.len(), BINARY_PROBE_BYTES + 1);
    }

    #[test]
    fn refuses_text_that_is_not_utf8() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("latin1.txt");
        std::fs::write(&path, b"caf\xe9\n").unwrap();
        assert!(matches!(read_text(&path), Err(ReadError::NotUtf8)));
    }

    #[test]
    fn refuses_files_over_the_open_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("huge.log");
        // A sparse file: the size is real, the disk use is not.
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(MAX_OPEN_BYTES + 1).unwrap();
        assert!(matches!(
            read_text(&path),
            Err(ReadError::TooLarge(len)) if len == MAX_OPEN_BYTES + 1
        ));
        file.set_len(MAX_OPEN_BYTES).unwrap();
        // Exactly at the limit still opens (and the zeros make it binary, not too large).
        assert!(matches!(read_text(&path), Err(ReadError::Binary)));
    }

    #[test]
    fn a_missing_file_is_an_io_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            read_text(&dir.path().join("gone.md")),
            Err(ReadError::Io(err)) if err.kind() == io::ErrorKind::NotFound
        ));
    }

    fn dir_names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn saves_with_the_files_own_line_ending_and_bom() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        write(&path, "\u{feff}one\r\ntwo\r\n");
        let file = read_text(&path).unwrap();
        let edited = format!("{}three\n", file.text);
        let stamp = write_text(&path, &edited, file.line_ending, file.bom).unwrap();
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"\xEF\xBB\xBFone\r\ntwo\r\nthree\r\n"
        );
        assert_eq!(stamp, super::stamp(&path).unwrap());

        write_text(&path, "no newline at the end", LineEnding::Lf, false).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"no newline at the end");
    }

    #[test]
    fn saving_leaves_no_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.md");
        write(&path, "old\n");
        write_text(&path, "new\n", LineEnding::Lf, false).unwrap();
        write_text(&dir.path().join("b.md"), "fresh\n", LineEnding::Lf, false).unwrap();
        assert_eq!(dir_names(dir.path()), ["a.md", "b.md"]);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new\n");
    }

    #[test]
    fn saving_keeps_permissions() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run.sh");
        write(&path, "echo old\n");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o750)).unwrap();
        write_text(&path, "echo new\n", LineEnding::Lf, false).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o750);
    }

    #[test]
    fn refuses_to_replace_a_read_only_file() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("locked.md");
        write(&path, "keep\n");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
        let err = write_text(&path, "lost\n", LineEnding::Lf, false).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "keep\n");
        assert_eq!(dir_names(dir.path()), ["locked.md"]);
    }

    #[test]
    fn saving_writes_through_a_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let notes = dir.path().join("notes");
        let docs = dir.path().join("docs");
        write(&notes.join("real.md"), "old\n");
        std::fs::create_dir(&docs).unwrap();
        let link = docs.join("link.md");
        symlink("../notes/real.md", &link).unwrap();

        write_text(&link, "new\n", LineEnding::Lf, false).unwrap();
        assert!(std::fs::symlink_metadata(&link).unwrap().is_symlink());
        assert_eq!(
            std::fs::read_to_string(notes.join("real.md")).unwrap(),
            "new\n"
        );
        // The temporary file went next to the target, and nothing was left behind.
        assert_eq!(dir_names(&docs), ["link.md"]);
        assert_eq!(dir_names(&notes), ["real.md"]);
    }

    fn created(root: &Path, dir: &str, typed: &str) -> Result<(String, EntryKind), String> {
        create(root, Path::new(dir), typed)
            .map(|(path, kind)| (path.to_str().unwrap().to_string(), kind))
            .map_err(|err| err.to_string())
    }

    #[test]
    fn creates_an_empty_file_in_the_folder() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("docs")).unwrap();
        assert_eq!(
            created(root.path(), "docs", "new.md"),
            Ok(("docs/new.md".into(), EntryKind::File))
        );
        assert_eq!(
            read_text(&root.path().join("docs/new.md")).unwrap().text,
            ""
        );
        // The open folder itself is the empty path.
        assert_eq!(
            created(root.path(), "", " top.md "),
            Ok(("top.md".into(), EntryKind::File))
        );
    }

    #[test]
    fn a_trailing_slash_creates_a_folder() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            created(root.path(), "", "notes/"),
            Ok(("notes".into(), EntryKind::Dir))
        );
        assert!(root.path().join("notes").is_dir());
    }

    #[test]
    fn creates_the_folders_on_the_way() {
        let root = tempfile::tempdir().unwrap();
        write(&root.path().join("a/keep.md"), "kept\n");
        assert_eq!(
            created(root.path(), "", "a/b/c.md"),
            Ok(("a/b/c.md".into(), EntryKind::File))
        );
        assert!(root.path().join("a/b/c.md").is_file());
        assert_eq!(dir_names(&root.path().join("a")), ["b", "keep.md"]);
        assert_eq!(
            created(root.path(), "a", "x/y/"),
            Ok(("a/x/y".into(), EntryKind::Dir))
        );
        assert!(root.path().join("a/x/y").is_dir());
    }

    #[test]
    fn never_replaces_what_is_there() {
        let root = tempfile::tempdir().unwrap();
        write(&root.path().join("a.md"), "mine\n");
        std::fs::create_dir(root.path().join("docs")).unwrap();
        assert_eq!(
            created(root.path(), "", "a.md"),
            Err("Already exists".into())
        );
        assert_eq!(
            std::fs::read_to_string(root.path().join("a.md")).unwrap(),
            "mine\n"
        );
        assert_eq!(
            created(root.path(), "", "docs/"),
            Err("Already exists".into())
        );
        assert_eq!(
            created(root.path(), "", "docs"),
            Err("Already exists".into())
        );
        assert_eq!(
            created(root.path(), "", "a.md/"),
            Err("Already exists".into())
        );
        assert_eq!(
            created(root.path(), "", "a.md/b.md"),
            Err("“a.md” is a file".into())
        );
        assert_eq!(dir_names(root.path()), ["a.md", "docs"]);
    }

    #[test]
    fn refuses_names_that_leave_or_skip_a_folder() {
        let root = tempfile::tempdir().unwrap();
        for typed in [
            "..",
            ".",
            "../out.md",
            "a/../b.md",
            "a//b.md",
            "/abs.md",
            "//",
        ] {
            assert_eq!(
                created(root.path(), "", typed),
                Err("Not a valid name".into()),
                "{typed}"
            );
        }
        for typed in ["", "   ", "/"] {
            assert_eq!(created(root.path(), "", typed), Err("Type a name".into()));
        }
        assert!(dir_names(root.path()).is_empty());
    }

    fn renamed(root: &Path, from: &str, name: &str) -> Result<String, String> {
        rename(root, Path::new(from), name)
            .map(|path| path.to_str().unwrap().to_string())
            .map_err(|err| err.to_string())
    }

    #[test]
    fn renames_inside_the_folder() {
        let root = tempfile::tempdir().unwrap();
        write(&root.path().join("docs/old.md"), "text\n");
        assert_eq!(
            renamed(root.path(), "docs/old.md", " new.md "),
            Ok("docs/new.md".into())
        );
        assert_eq!(dir_names(&root.path().join("docs")), ["new.md"]);
        assert_eq!(
            std::fs::read_to_string(root.path().join("docs/new.md")).unwrap(),
            "text\n"
        );
        // A folder takes what is inside it along.
        assert_eq!(renamed(root.path(), "docs", "notes"), Ok("notes".into()));
        assert!(root.path().join("notes/new.md").is_file());
    }

    #[test]
    fn a_rename_never_replaces_another_item() {
        let root = tempfile::tempdir().unwrap();
        write(&root.path().join("a.md"), "a\n");
        write(&root.path().join("b.md"), "b\n");
        assert_eq!(
            renamed(root.path(), "a.md", "b.md"),
            Err("Already exists".into())
        );
        assert_eq!(
            std::fs::read_to_string(root.path().join("a.md")).unwrap(),
            "a\n"
        );
        assert_eq!(
            std::fs::read_to_string(root.path().join("b.md")).unwrap(),
            "b\n"
        );
    }

    #[test]
    fn a_rename_may_change_only_the_case() {
        let root = tempfile::tempdir().unwrap();
        write(&root.path().join("readme.md"), "");
        assert_eq!(
            renamed(root.path(), "readme.md", "README.md"),
            Ok("README.md".into())
        );
        assert_eq!(dir_names(root.path()), ["README.md"]);
    }

    #[test]
    fn the_same_name_changes_nothing() {
        let root = tempfile::tempdir().unwrap();
        write(&root.path().join("a.md"), "");
        assert_eq!(renamed(root.path(), "a.md", "a.md"), Ok("a.md".into()));
        assert_eq!(dir_names(root.path()), ["a.md"]);
    }

    #[test]
    fn a_new_name_is_one_name() {
        let root = tempfile::tempdir().unwrap();
        write(&root.path().join("a.md"), "");
        for name in ["b/c.md", "..", ".", "/"] {
            assert_eq!(
                renamed(root.path(), "a.md", name),
                Err("Not a valid name".into()),
                "{name}"
            );
        }
        assert_eq!(
            renamed(root.path(), "a.md", "  "),
            Err("Type a name".into())
        );
        assert_eq!(dir_names(root.path()), ["a.md"]);
    }

    // Moving something to the Trash cannot be tested without filling the real Trash of whoever
    // runs the tests, so only the refusal is tested here.
    #[test]
    fn the_open_folder_never_goes_to_the_trash() {
        let root = tempfile::tempdir().unwrap();
        write(&root.path().join("a.md"), "");
        let err = trash(root.path(), Path::new("")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(dir_names(root.path()), ["a.md"]);
    }

    #[test]
    fn saving_through_a_dangling_symlink_recreates_the_target() {
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("link.md");
        symlink("gone.md", &link).unwrap();
        write_text(&link, "back\n", LineEnding::Lf, false).unwrap();
        assert!(std::fs::symlink_metadata(&link).unwrap().is_symlink());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("gone.md")).unwrap(),
            "back\n"
        );
    }

    #[test]
    fn a_free_name_follows_finder() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path();
        assert_eq!(free_name(dir, "notes.md", EntryKind::File), "notes.md");
        write(&dir.join("notes.md"), "");
        assert_eq!(free_name(dir, "notes.md", EntryKind::File), "notes copy.md");
        write(&dir.join("notes copy.md"), "");
        assert_eq!(
            free_name(dir, "notes.md", EntryKind::File),
            "notes copy 2.md"
        );
        write(&dir.join(".env"), "");
        assert_eq!(free_name(dir, ".env", EntryKind::File), ".env copy");
        std::fs::create_dir(dir.join("v1.2")).unwrap();
        assert_eq!(free_name(dir, "v1.2", EntryKind::Dir), "v1.2 copy");
    }

    #[test]
    fn a_dropped_picture_is_linked_where_it_is_or_copied_in() {
        let root = tempfile::tempdir().unwrap();
        let notes = root.path().join("notes");
        write(&notes.join("shots/a.png"), "a");
        write(&root.path().join("out/my pic.png"), "b");
        assert_eq!(
            place_picture(&notes, &notes.join("shots/a.png")).unwrap(),
            "shots/a.png"
        );
        let outside = root.path().join("out/my pic.png");
        assert_eq!(
            place_picture(&notes, &outside).unwrap(),
            "images/my pic.png"
        );
        assert_eq!(
            place_picture(&notes, &outside).unwrap(),
            "images/my pic copy.png"
        );
        assert_eq!(
            std::fs::read_to_string(notes.join("images/my pic.png")).unwrap(),
            "b"
        );
        assert!(outside.is_file());
    }

    #[test]
    fn a_new_file_takes_the_next_free_number() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("images");
        let first = write_new(&dir, "note", "png", b"one").unwrap();
        assert_eq!(first, dir.join("note-1.png"));
        write(&dir.join("note-2.png"), "kept");
        let next = write_new(&dir, "note", "png", b"two").unwrap();
        assert_eq!(next, dir.join("note-3.png"));
        assert_eq!(std::fs::read(&first).unwrap(), b"one");
        assert_eq!(
            std::fs::read_to_string(dir.join("note-2.png")).unwrap(),
            "kept"
        );
        assert_eq!(std::fs::read(&next).unwrap(), b"two");
    }

    #[test]
    fn copies_a_folder_whole_with_links_as_links() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        write(&root.join("docs/guide/a.md"), "a\n");
        // A link back up would never end if it were followed.
        symlink(root.join("docs"), root.join("docs/guide/up")).unwrap();
        std::fs::create_dir(root.join("out")).unwrap();
        let copy = copy_into(&root.join("docs"), &root.join("out"), "docs").unwrap();
        assert_eq!(copy, root.join("out/docs"));
        assert_eq!(
            std::fs::read_to_string(root.join("out/docs/guide/a.md")).unwrap(),
            "a\n"
        );
        assert!(
            std::fs::symlink_metadata(root.join("out/docs/guide/up"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        // The original stays.
        assert!(root.join("docs/guide/a.md").is_file());
    }

    #[test]
    fn copying_and_moving_never_replace() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        write(&root.join("a.md"), "mine\n");
        write(&root.join("docs/a.md"), "theirs\n");
        let from = root.join("a.md");
        let docs = root.join("docs");
        assert!(matches!(
            copy_into(&from, &docs, "a.md"),
            Err(NameError::Exists)
        ));
        assert!(matches!(
            move_into(&from, &docs, "a.md"),
            Err(NameError::Exists)
        ));
        assert_eq!(
            std::fs::read_to_string(docs.join("a.md")).unwrap(),
            "theirs\n"
        );
        assert!(from.is_file());
    }

    #[test]
    fn moves_a_file_into_a_folder() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        write(&root.join("a.md"), "a\n");
        std::fs::create_dir(root.join("docs")).unwrap();
        let moved = move_into(&root.join("a.md"), &root.join("docs"), "b.md").unwrap();
        assert_eq!(moved, root.join("docs/b.md"));
        assert_eq!(std::fs::read_to_string(&moved).unwrap(), "a\n");
        assert!(!root.join("a.md").exists());
        // Moving it where it already is changes nothing.
        assert_eq!(
            move_into(&moved, &root.join("docs"), "b.md").unwrap(),
            moved
        );
        assert!(moved.is_file());
    }

    #[test]
    fn a_folder_never_goes_inside_itself() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        write(&root.join("docs/guide/a.md"), "");
        let (docs, guide) = (root.join("docs"), root.join("docs/guide"));
        assert!(matches!(
            copy_into(&docs, &guide, "docs"),
            Err(NameError::IntoItself)
        ));
        assert!(matches!(
            move_into(&docs, &guide, "docs"),
            Err(NameError::IntoItself)
        ));
        assert!(matches!(
            copy_into(&docs, &docs, "docs copy"),
            Err(NameError::IntoItself)
        ));
        assert_eq!(dir_names(&guide), ["a.md"]);
    }

    #[test]
    fn a_typed_target_is_a_folder_and_maybe_a_name() {
        let target = |typed, several| {
            typed_target(typed, several)
                .map(|(dir, name)| (dir.to_string_lossy().into_owned(), name))
        };
        assert_eq!(
            target("docs/guide/a.md", false).unwrap(),
            ("docs/guide".into(), Some("a.md".into()))
        );
        assert_eq!(
            target("a.md", false).unwrap(),
            ("".into(), Some("a.md".into()))
        );
        assert_eq!(target("docs/", false).unwrap(), ("docs".into(), None));
        // Several items keep their names; what is typed is their folder.
        assert_eq!(
            target("docs/guide", true).unwrap(),
            ("docs/guide".into(), None)
        );
        assert_eq!(target(" src/ ", true).unwrap(), ("src".into(), None));
        assert!(matches!(target("", false), Err(NameError::Empty)));
        for bad in ["/etc/a.md", "../a.md", "docs//a.md", "docs/.."] {
            assert!(
                matches!(target(bad, false), Err(NameError::Invalid)),
                "{bad}"
            );
        }
    }
}
