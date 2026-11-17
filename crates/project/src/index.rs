//! The list of files under the open folder that the finder searches (gitignored files left out).

use std::{
    cmp::Ordering,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering::Relaxed},
    },
};

use ignore::{WalkBuilder, WalkState};

use crate::folders::Folders;

/// More files than this stops the walk: a folder that big is not one subwy is meant for, and
/// the finder should still answer instead of reading the disk for minutes.
pub const MAX_FILES: usize = 200_000;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileIndex {
    /// Paths relative to the open folder, `/`-separated, NFC, in [`compare_paths`] order.
    pub paths: Vec<String>,
    /// The walk stopped at [`MAX_FILES`]; `paths` holds the files found until then.
    pub capped: bool,
}

/// Every file under `root` the finder offers: what the tree shows, minus the gitignored files.
pub fn build(root: &Path) -> FileIndex {
    build_with_limit(&Folders::single(root.to_path_buf()), MAX_FILES)
}

/// The finder's files across a window's folders: each folder's own, with its name in front when
/// there are several (the paths the rest of subwy uses, see `folders`).
pub fn build_folders(folders: &Folders) -> FileIndex {
    build_with_limit(folders, MAX_FILES)
}

/// Every folder's own files go in before any checkout inside them (a folder with a `.git` file:
/// a git worktree or submodule). Those are mostly copies of a repository the window already
/// holds, and a folder of many worktrees filled `limit` alone, leaving the repository out.
fn build_with_limit(folders: &Folders, limit: usize) -> FileIndex {
    let mut index = FileIndex::default();
    let several = folders.several();
    let roots = folders.roots().iter().zip(folders.keys());
    let mut later = Vec::new();
    for (root, key) in roots.clone() {
        let mut found = walk(root, limit - index.paths.len(), None);
        later.push(std::mem::take(&mut found.checkouts));
        index.add(found, several.then_some(key.as_str()));
    }
    for ((root, key), checkouts) in roots.zip(later) {
        if !checkouts.is_empty() {
            let found = walk(root, limit - index.paths.len(), Some(Arc::new(checkouts)));
            index.add(found, several.then_some(key.as_str()));
        }
    }
    index.paths.sort_unstable_by(|a, b| compare_paths(a, b));
    index
}

impl FileIndex {
    /// One folder's files, under `key` when the window has several.
    fn add(&mut self, found: Walk, key: Option<&str>) {
        self.capped |= found.capped;
        match key {
            Some(key) => self
                .paths
                .extend(found.paths.into_iter().map(|path| format!("{key}/{path}"))),
            None => self.paths.extend(found.paths),
        }
    }
}

struct Walk {
    /// Relative to the walked folder, unsorted.
    paths: Vec<String>,
    capped: bool,
    /// The checkouts a first walk met and left for later.
    checkouts: Vec<PathBuf>,
}

/// Lists up to `limit` files of `root`: its own, leaving out the checkouts it meets, or with
/// `into` only those in the checkouts it names. Both walk from `root`, so the same ignore files
/// apply either way.
fn walk(root: &Path, limit: usize, into: Option<Arc<Vec<PathBuf>>>) -> Walk {
    let checkouts = Arc::new(Mutex::new(Vec::new()));
    let met = checkouts.clone();
    let filter = move |entry: &ignore::DirEntry| {
        if entry.file_name() == ".git" {
            return false;
        }
        let path = entry.path();
        if let Some(into) = &into {
            // The way down to each checkout, and everything in it.
            return into
                .iter()
                .any(|dir| path.starts_with(dir) || dir.starts_with(path));
        }
        let checkout = entry.depth() > 0
            && entry.file_type().is_some_and(|t| t.is_dir())
            && path.join(".git").is_file();
        if checkout {
            let mut met = met.lock().unwrap_or_else(|e| e.into_inner());
            met.push(path.to_path_buf());
        }
        !checkout
    };
    // The same rules as the tree (fs.rs): `.gitignore` files from `root` down and
    // `.git/info/exclude`, with or without a git repository, nothing from above `root`
    // or from the global git config. Hidden files stay; the tree shows them too.
    let walk = WalkBuilder::new(root)
        .hidden(false)
        .parents(false)
        .ignore(false)
        .git_global(false)
        .git_ignore(true)
        .git_exclude(true)
        .require_git(false)
        // The tree opens symlinked folders; the walker skips the ones that loop.
        .follow_links(true)
        .filter_entry(filter)
        // In parallel: following links costs a stat per entry, and 90k files then take
        // about a third of the time they take on one thread (measured on ~/.cargo/registry).
        .build_parallel();
    let paths = Mutex::new(Vec::new());
    let capped = AtomicBool::new(false);
    walk.run(|| {
        Box::new(|entry| {
            // Unreadable folders and symlink loops are left out, as the tree leaves them closed.
            let Ok(entry) = entry else {
                return WalkState::Continue;
            };
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                return WalkState::Continue;
            }
            let name = entry.file_name().to_string_lossy();
            // `.subwy-save-*.tmp` lives only for the instant of an atomic save.
            if name == ".DS_Store" || name.starts_with(".subwy-save-") {
                return WalkState::Continue;
            }
            let Ok(rel) = entry.path().strip_prefix(root) else {
                return WalkState::Continue;
            };
            let rel = crate::paths::relative(rel);
            let mut paths = paths.lock().unwrap_or_else(|e| e.into_inner());
            if paths.len() == limit {
                capped.store(true, Relaxed);
                return WalkState::Quit;
            }
            paths.push(rel);
            WalkState::Continue
        })
    });
    let checkouts = std::mem::take(&mut *checkouts.lock().unwrap_or_else(|e| e.into_inner()));
    Walk {
        paths: paths.into_inner().unwrap_or_else(|e| e.into_inner()),
        capped: capped.into_inner(),
        checkouts,
    }
}

/// Case-insensitive, then exact, so the order never depends on the order the disk returned.
pub fn compare_paths(a: &str, b: &str) -> Ordering {
    // Folding byte by byte when both are ASCII keeps a 200k sort quick.
    let folded = if a.is_ascii() && b.is_ascii() {
        let a = a.bytes().map(|c| c.to_ascii_lowercase());
        a.cmp(b.bytes().map(|c| c.to_ascii_lowercase()))
    } else {
        let a = a.chars().flat_map(char::to_lowercase);
        a.cmp(b.chars().flat_map(char::to_lowercase))
    };
    folded.then_with(|| a.cmp(b))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use super::*;

    fn write(root: &Path, path: &str) {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "").unwrap();
    }

    #[test]
    fn several_folders_index_together_under_their_names() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        write(a.path(), "docs/x.md");
        write(b.path(), "y.sql");
        let folders = Folders::new(vec![a.path().to_path_buf(), b.path().to_path_buf()]);
        let (ka, kb) = (&folders.keys()[0], &folders.keys()[1]);
        let mut want = vec![format!("{ka}/docs/x.md"), format!("{kb}/y.sql")];
        want.sort_by(|x, y| compare_paths(x, y));
        assert_eq!(build_folders(&folders).paths, want);
        // One folder indexes as before, without its name.
        let one = Folders::single(a.path().to_path_buf());
        assert_eq!(build_folders(&one).paths, ["docs/x.md"]);
    }

    #[test]
    fn lists_every_file_sorted_ignoring_case() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        for path in [
            "src/main.rs",
            "README.md",
            "docs/guide/keys.md",
            "docs/Notes.md",
            ".github/ci.yml",
            ".env",
            "empty/.keep",
        ] {
            write(root, path);
        }
        std::fs::create_dir(root.join("nothing")).unwrap();
        let index = build(root);
        assert_eq!(
            index.paths,
            [
                ".env",
                ".github/ci.yml",
                "docs/guide/keys.md",
                "docs/Notes.md",
                "empty/.keep",
                "README.md",
                "src/main.rs",
            ]
        );
        assert!(!index.capped);
    }

    #[test]
    fn leaves_out_git_and_gitignored_files() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        std::fs::write(root.join(".gitignore"), "target/\n*.log\n!keep.log\n").unwrap();
        write(root, ".git/info/exclude");
        std::fs::write(root.join(".git/info/exclude"), "secret.txt\n").unwrap();
        write(root, "docs/.gitignore");
        std::fs::write(root.join("docs/.gitignore"), "draft.md\n").unwrap();
        for path in [
            ".git/HEAD",
            "target/debug/leaf",
            "a.log",
            "keep.log",
            "secret.txt",
            "readme.md",
            "docs/draft.md",
            "docs/guide.md",
            ".DS_Store",
            ".subwy-save-1.tmp",
        ] {
            write(root, path);
        }
        assert_eq!(
            build(root).paths,
            [
                ".gitignore",
                "docs/.gitignore",
                "docs/guide.md",
                "keep.log",
                "readme.md",
            ]
        );
    }

    #[test]
    fn decomposed_hangul_paths_come_back_composed() {
        let root = tempfile::tempdir().unwrap();
        let nfd = "\u{1112}\u{1161}\u{11ab}\u{1100}\u{1173}\u{11af}";
        write(root.path(), &format!("{nfd}/{nfd}.md"));
        assert_eq!(
            build(root.path()).paths,
            ["\u{d55c}\u{ae00}/\u{d55c}\u{ae00}.md"]
        );
    }

    #[test]
    fn follows_symlinks_but_not_loops() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root = root.path();
        write(root, "a/note.md");
        write(outside.path(), "shared.md");
        symlink(outside.path(), root.join("outside")).unwrap();
        symlink("..", root.join("a/up")).unwrap();
        symlink("note.md", root.join("a/link.md")).unwrap();
        assert_eq!(
            build(root).paths,
            ["a/link.md", "a/note.md", "outside/shared.md"]
        );
    }

    #[test]
    fn stops_at_the_limit_and_says_so() {
        let root = tempfile::tempdir().unwrap();
        for i in 0..5 {
            write(root.path(), &format!("f{i}.md"));
        }
        let folders = Folders::single(root.path().to_path_buf());
        let index = build_with_limit(&folders, 3);
        assert_eq!(index.paths.len(), 3);
        assert!(index.capped);
        let index = build_with_limit(&folders, 5);
        assert_eq!(index.paths.len(), 5);
        assert!(!index.capped);
    }

    #[test]
    fn worktrees_wait_until_every_folder_is_listed() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        write(a.path(), "own.md");
        // Two linked worktrees: a `.git` file, not a folder.
        for name in ["one", "two"] {
            write(a.path(), &format!("copies/{name}/.git"));
            write(a.path(), &format!("copies/{name}/{name}.md"));
        }
        write(b.path(), "b.md");
        let folders = Folders::new(vec![a.path().to_path_buf(), b.path().to_path_buf()]);
        let (ka, kb) = (&folders.keys()[0], &folders.keys()[1]);

        // Room for one worktree file: both folders' own files are in all the same.
        let index = build_with_limit(&folders, 3);
        assert!(index.capped);
        assert_eq!(index.paths.len(), 3);
        assert!(index.paths.contains(&format!("{ka}/own.md")));
        assert!(index.paths.contains(&format!("{kb}/b.md")));

        let mut want = vec![
            format!("{ka}/copies/one/one.md"),
            format!("{ka}/copies/two/two.md"),
            format!("{ka}/own.md"),
            format!("{kb}/b.md"),
        ];
        want.sort_by(|x, y| compare_paths(x, y));
        let index = build_with_limit(&folders, 10);
        assert_eq!(index.paths, want);
        assert!(!index.capped);
        // A window opened on a worktree lists it as its own.
        assert_eq!(build(&a.path().join("copies/one")).paths, ["one.md"]);
    }

    #[test]
    fn paths_equal_but_for_case_sort_by_exact_path() {
        let mut paths = vec!["b.md", "B.md", "a/z.md", "A/y.md", "\u{c544}.md", "Z.md"];
        paths.sort_by(|a, b| compare_paths(a, b));
        assert_eq!(
            paths,
            ["A/y.md", "a/z.md", "B.md", "b.md", "Z.md", "\u{c544}.md"]
        );
    }
}
