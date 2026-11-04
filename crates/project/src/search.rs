//! Find in the folder: the lines of the finder's files (gitignored files left out) that contain
//! a query.
//!
//! The query is literal. It ignores case unless it has an upper-case letter (smart case), so
//! `path` finds `Path` and `PATH` while `Path` finds only `Path`.

use std::{
    collections::BTreeMap,
    ops::Range,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed},
        mpsc,
    },
    thread,
};

use unicode_normalization::UnicodeNormalization;

use crate::{folders::Folders, fs, markdown};

/// More matches than this stop the search: a list that long is for narrowing the query, not
/// for reading, and stopping keeps a one-letter query quick on a large folder.
pub const MAX_MATCHES: usize = 2_000;
/// Files are read and searched on this many threads at most. Reading is most of the cost, and
/// 20k files (240 MB) take about a second on one thread.
const MAX_THREADS: usize = 8;
/// A line is cut to this many chars for display: a minified file can hold megabytes in one line.
const MAX_LINE_CHARS: usize = 200;
/// The chars kept before the first match when it sits far along its line.
const CONTEXT_CHARS: usize = 40;
/// Marks a line whose start was cut off.
const CUT: &str = "\u{2026}";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineMatch {
    /// Zero-based.
    pub line: u32,
    /// The line as shown: without its indentation, cut around the first match, tabs as spaces.
    pub text: String,
    /// The matches as byte ranges into `text`; a match cut off the end is left out.
    pub ranges: Vec<Range<usize>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileMatches {
    /// Relative to the open folder, as the index lists it.
    pub path: String,
    pub lines: Vec<LineMatch>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Results {
    /// In the index's order.
    pub files: Vec<FileMatches>,
    /// Every match, two on one line counting as two.
    pub matches: usize,
    /// There were more than [`MAX_MATCHES`]; the search stopped there.
    pub capped: bool,
}

/// The lines of `paths` (relative to `root`, in the index's order) that contain `query`.
/// Binary, non-UTF-8 and unreadable files are skipped, as are files too large to open.
///
/// Returns `None` as soon as `cancel` is set: a newer query replaces this one, and a large
/// folder should not finish an old search first.
pub fn search(root: &Path, paths: &[String], query: &str, cancel: &AtomicBool) -> Option<Results> {
    search_with_limit(root, paths, query, cancel, MAX_MATCHES)
}

/// [`search`] over a window's folders: `paths` as the finder's index has them (see `folders`).
pub fn search_folders(
    folders: &Folders,
    paths: &[String],
    query: &str,
    cancel: &AtomicBool,
) -> Option<Results> {
    let resolve = |path: &str| folders.absolute(Path::new(path)).unwrap_or_default();
    search_inner(&resolve, paths, query, cancel, MAX_MATCHES)
}

/// Alt+F7 on a file: the lines of the Markdown files among `paths` (as the finder's index lists
/// them) that link to `target`, each link resolved from its own file's folder as following it
/// would be. The links are the matches.
pub fn backlinks(folders: &Folders, paths: &[String], target: &Path) -> Results {
    let target = markdown::Link::File(target.to_path_buf());
    let mut results = Results::default();
    for path in paths {
        let Some(abs) = folders.absolute(Path::new(path)) else {
            continue;
        };
        let Ok(file) = fs::read_text(&abs) else {
            continue;
        };
        if !file.text.contains("](") || !markdown::is_markdown(&abs, &file.text) {
            continue;
        }
        let dir = abs.parent().unwrap_or(Path::new(""));
        let mut lines = Vec::new();
        let mut start = 0;
        for (number, line) in file.text.split('\n').enumerate() {
            let ranges: Vec<Range<usize>> = markdown::links(line)
                .into_iter()
                .filter(|(_, to)| markdown::follow(dir, to).as_ref() == Some(&target))
                .map(|(range, _)| start + range.start..start + range.end)
                .collect();
            if !ranges.is_empty() {
                results.matches += ranges.len();
                let bounds = start..start + line.len();
                lines.push(line_match(&file.text, number as u32, bounds, &ranges));
            }
            start += line.len() + 1;
        }
        if !lines.is_empty() {
            results.files.push(FileMatches {
                path: path.clone(),
                lines,
            });
        }
    }
    results
}

fn search_with_limit(
    root: &Path,
    paths: &[String],
    query: &str,
    cancel: &AtomicBool,
    limit: usize,
) -> Option<Results> {
    search_inner(&|path: &str| root.join(path), paths, query, cancel, limit)
}

fn search_inner(
    resolve: &(dyn Fn(&str) -> PathBuf + Sync),
    paths: &[String],
    query: &str,
    cancel: &AtomicBool,
    limit: usize,
) -> Option<Results> {
    let mut results = Results::default();
    let Some(needle) = Needle::new(query) else {
        return Some(results);
    };
    let next = AtomicUsize::new(0);
    // Set once the results are complete (capped), so the threads stop reading.
    let done = AtomicBool::new(false);
    let threads = thread::available_parallelism().map_or(1, |n| n.get().min(MAX_THREADS));
    let (sender, receiver) = mpsc::channel();
    thread::scope(|scope| {
        for _ in 0..threads {
            let sender = sender.clone();
            let (next, done, needle) = (&next, &done, &needle);
            scope.spawn(move || {
                while !done.load(Relaxed) && !cancel.load(Relaxed) {
                    let i = next.fetch_add(1, Relaxed);
                    let Some(path) = paths.get(i) else {
                        break;
                    };
                    // The same rules as opening the file: what subwy cannot open, it does not search.
                    let found = match fs::read_text(&resolve(path)) {
                        Ok(file) => search_text(&file.text, needle, limit),
                        Err(_) => Found::default(),
                    };
                    if sender.send((i, found)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(sender);
        // Files finish out of order; they join the results in index order, so the cap keeps
        // the first matches whichever thread was quicker.
        let mut waiting = BTreeMap::new();
        let mut joined = 0;
        for (i, found) in receiver {
            waiting.insert(i, found);
            while let Some(found) = waiting.remove(&joined) {
                if add(&mut results, &paths[joined], found, limit) {
                    done.store(true, Relaxed);
                    return;
                }
                joined += 1;
            }
        }
    });
    (!cancel.load(Relaxed)).then_some(results)
}

/// Adds one file's lines, keeping to `limit` matches in all; true when that capped the results.
fn add(results: &mut Results, path: &str, found: Found, limit: usize) -> bool {
    let room = limit - results.matches;
    let mut lines = Vec::new();
    let mut taken = 0;
    let mut capped = found.more;
    for (mut line, count) in found.lines {
        if taken + count > room {
            // The shown ranges are the line's first matches, so these are the ones to keep.
            line.ranges.truncate(room - taken);
            if room > taken {
                lines.push(line);
            }
            taken = room;
            capped = true;
            break;
        }
        taken += count;
        lines.push(line);
    }
    results.matches += taken;
    results.capped = capped;
    if !lines.is_empty() {
        results.files.push(FileMatches {
            path: path.to_string(),
            lines,
        });
    }
    capped
}

/// The query as searched for.
struct Needle {
    /// NFC (what the macOS input methods type); folded when the search ignores case.
    text: String,
    fold: Fold,
}

/// How much of a file to fold before looking for the needle in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fold {
    /// Case matters, or the needle has no letters with case (digits, Hangul, symbols).
    None,
    /// ASCII letters only. Enough when the needle's other chars have no case: those match only
    /// themselves, and folding a whole file char by char costs more than reading it.
    Ascii,
    Full,
}

impl Needle {
    fn new(query: &str) -> Option<Self> {
        let text: String = query.nfc().collect();
        if text.is_empty() {
            return None;
        }
        if text.chars().any(char::is_uppercase) {
            return Some(Self {
                text,
                fold: Fold::None,
            });
        }
        let caseless = |c: char| {
            let (mut lower, mut upper) = (c.to_lowercase(), c.to_uppercase());
            lower.next() == Some(c)
                && lower.next().is_none()
                && upper.next() == Some(c)
                && upper.next().is_none()
        };
        let fold = if !text.chars().all(|c| c.is_ascii() || caseless(c)) {
            Fold::Full
        } else if text.chars().any(|c| c.is_ascii_alphabetic()) {
            Fold::Ascii
        } else {
            Fold::None
        };
        Some(Self {
            text: self::fold(&text),
            fold,
        })
    }
}

/// Lower case that keeps every char's UTF-8 length, so a byte offset into the folded text is
/// the same offset into the original. The few chars whose lower case is longer or shorter (the
/// Kelvin sign, a dotted capital I) stay as they are, and match only themselves.
fn fold(text: &str) -> String {
    if text.is_ascii() {
        return text.to_ascii_lowercase();
    }
    text.chars()
        .map(|c| {
            let mut lower = c.to_lowercase();
            match (lower.next(), lower.next()) {
                (Some(l), None) if l.len_utf8() == c.len_utf8() => l,
                _ => c,
            }
        })
        .collect()
}

#[derive(Default)]
struct Found {
    /// Each line with the number of matches on it; a line cut for display shows fewer.
    lines: Vec<(LineMatch, usize)>,
    matches: usize,
    /// A match past `limit` was left out.
    more: bool,
}

/// The lines of `text` that contain the needle, with at most `limit` matches.
fn search_text(text: &str, needle: &Needle, limit: usize) -> Found {
    let folded;
    let haystack = match needle.fold {
        Fold::None => text,
        Fold::Ascii => {
            folded = text.to_ascii_lowercase();
            &folded
        }
        Fold::Full => {
            folded = fold(text);
            &folded
        }
    };
    let mut found = Found::default();
    // The line being collected (its number and its bytes in `text`) and its matches.
    let mut current: Option<(u32, Range<usize>)> = None;
    let mut ranges = Vec::new();
    let mut line = 0u32;
    // Newlines before this byte are counted in `line`.
    let mut counted = 0;
    for (start, _) in haystack.match_indices(needle.text.as_str()) {
        if found.matches == limit {
            found.more = true;
            break;
        }
        found.matches += 1;
        if !current
            .as_ref()
            .is_some_and(|(_, bounds)| start < bounds.end)
        {
            if let Some((line, bounds)) = current.take() {
                let shown = line_match(text, line, bounds, &ranges);
                found.lines.push((shown, ranges.len()));
                ranges.clear();
            }
            line += count_newlines(&text[counted..start]);
            counted = start;
            let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
            let line_end = text[start..].find('\n').map_or(text.len(), |i| start + i);
            current = Some((line, line_start..line_end));
        }
        ranges.push(start..start + needle.text.len());
    }
    if let Some((line, bounds)) = current {
        let shown = line_match(text, line, bounds, &ranges);
        found.lines.push((shown, ranges.len()));
    }
    found
}

fn count_newlines(text: &str) -> u32 {
    text.bytes().filter(|&b| b == b'\n').count() as u32
}

/// One matching line, cut for display. `bounds` and `ranges` are byte ranges into `text`.
fn line_match(text: &str, line: u32, bounds: Range<usize>, ranges: &[Range<usize>]) -> LineMatch {
    let first = ranges[0].start;
    let indent = text[bounds.clone()].len() - text[bounds.clone()].trim_start().len();
    // A query of spaces can match inside the indentation; the match stays visible.
    let mut start = (bounds.start + indent).min(first);
    let mut prefix = "";
    if text[start..first].chars().nth(CONTEXT_CHARS).is_some() {
        start = text[..first]
            .char_indices()
            .rev()
            .nth(CONTEXT_CHARS - 1)
            .map_or(start, |(i, _)| i);
        prefix = CUT;
    }
    let end = text[start..bounds.end]
        .char_indices()
        .nth(MAX_LINE_CHARS)
        .map_or(bounds.end, |(i, _)| start + i);
    // Tabs and spaces are both one byte, so the ranges still line up.
    let shown = format!("{prefix}{}", text[start..end].replace('\t', " "));
    let shift = |offset: usize| offset - start + prefix.len();
    let ranges = ranges
        .iter()
        .filter(|range| range.start < end)
        .map(|range| shift(range.start)..shift(range.end.min(end)))
        .collect();
    LineMatch {
        line,
        text: shown,
        ranges,
    }
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::single_range_in_vec_init,
        reason = "a line with one match has one range"
    )]

    use super::*;

    fn write(root: &Path, path: &str, text: &[u8]) {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn backlinks_are_the_markdown_links_to_the_file() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        write(root, "docs/b.md", b"# B\n");
        write(
            root,
            "notes/a.md",
            b"intro\nsee [b](../docs/b.md) and [web](https://x.kr)\n",
        );
        write(root, "notes/c.md", b"[other](other.md)\n");
        write(root, "notes/d.txt", b"[b](../docs/b.md)\n");
        let paths: Vec<String> = ["docs/b.md", "notes/a.md", "notes/c.md", "notes/d.txt"]
            .iter()
            .map(|p| p.to_string())
            .collect();
        let folders = Folders::single(root.to_path_buf());
        let found = backlinks(&folders, &paths, &root.join("docs/b.md"));
        assert_eq!(
            hits(&found),
            [("notes/a.md".to_string(), 1, "[b](../docs/b.md)".to_string())]
        );
    }

    fn run(root: &Path, paths: &[&str], query: &str) -> Results {
        let paths: Vec<String> = paths.iter().map(|p| p.to_string()).collect();
        search(root, &paths, query, &AtomicBool::new(false)).unwrap()
    }

    /// Each match as (path, line, the matched text) for compact asserts.
    fn hits(results: &Results) -> Vec<(String, u32, String)> {
        let mut hits = Vec::new();
        for file in &results.files {
            for line in &file.lines {
                for range in &line.ranges {
                    hits.push((
                        file.path.clone(),
                        line.line,
                        line.text[range.clone()].to_string(),
                    ));
                }
            }
        }
        hits
    }

    fn hit(path: &str, line: u32, text: &str) -> (String, u32, String) {
        (path.to_string(), line, text.to_string())
    }

    #[test]
    fn finds_lines_in_index_order() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        write(root, "b.md", b"one\ntwo path\n");
        write(root, "a/z.md", b"path first\n\nlast path");
        write(root, "c.md", b"nothing here\n");
        let results = run(root, &["a/z.md", "b.md", "c.md"], "path");
        assert_eq!(
            hits(&results),
            [
                hit("a/z.md", 0, "path"),
                hit("a/z.md", 2, "path"),
                hit("b.md", 1, "path"),
            ]
        );
        assert_eq!(results.files.len(), 2);
        assert_eq!(results.matches, 3);
        assert!(!results.capped);
        assert_eq!(results.files[0].lines[1].text, "last path");
        assert_eq!(results.files[0].lines[1].ranges, [5..9]);
    }

    #[test]
    fn lower_case_ignores_case_and_upper_case_does_not() {
        let root = tempfile::tempdir().unwrap();
        write(root.path(), "a.rs", b"Path\npath\nPATH\n");
        let lines = |query: &str| -> Vec<u32> {
            let results = run(root.path(), &["a.rs"], query);
            hits(&results)
                .into_iter()
                .map(|(_, line, _)| line)
                .collect()
        };
        assert_eq!(lines("path"), [0, 1, 2]);
        assert_eq!(lines("Path"), [0]);
        assert_eq!(lines("PATH"), [2]);
    }

    #[test]
    fn the_query_is_literal() {
        let root = tempfile::tempdir().unwrap();
        write(root.path(), "a.rs", b"a.b\naxb\n(a|b)\n");
        let results = run(root.path(), &["a.rs"], "a.b");
        assert_eq!(hits(&results), [hit("a.rs", 0, "a.b")]);
        let results = run(root.path(), &["a.rs"], "(a|b)");
        assert_eq!(hits(&results), [hit("a.rs", 2, "(a|b)")]);
    }

    #[test]
    fn several_matches_on_a_line_make_one_line_and_count_apart() {
        let root = tempfile::tempdir().unwrap();
        write(root.path(), "a.md", b"leaf, leaf and LEAF\nno\n");
        let results = run(root.path(), &["a.md"], "leaf");
        assert_eq!(results.matches, 3);
        let lines = &results.files[0].lines;
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].ranges, [0..4, 6..10, 15..19]);
    }

    #[test]
    fn skips_binary_non_utf8_missing_and_oversized_files() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        write(root, "bin", b"leaf\0leaf\n");
        write(root, "latin1.txt", b"leaf \xE9t\xE9\n");
        write(root, "ok.txt", b"leaf\n");
        let big = std::fs::File::create(root.join("big.txt")).unwrap();
        big.set_len(fs::MAX_OPEN_BYTES + 1).unwrap();
        let results = run(
            root,
            &["big.txt", "bin", "gone.txt", "latin1.txt", "ok.txt"],
            "leaf",
        );
        assert_eq!(hits(&results), [hit("ok.txt", 0, "leaf")]);
    }

    #[test]
    fn crlf_and_bom_keep_lines_and_columns() {
        let root = tempfile::tempdir().unwrap();
        write(
            root.path(),
            "a.txt",
            b"\xEF\xBB\xBFleaf\r\nnone\r\n  leaf\r\n",
        );
        let results = run(root.path(), &["a.txt"], "leaf");
        let lines = &results.files[0].lines;
        assert_eq!((lines[0].line, lines[0].ranges.clone()), (0, vec![0..4]));
        assert_eq!(lines[1].line, 2);
        assert_eq!(lines[1].text, "leaf");
    }

    #[test]
    fn non_ascii_text_matches_by_bytes_of_the_original() {
        let root = tempfile::tempdir().unwrap();
        write(
            root.path(),
            "a.md",
            "\u{d55c}\u{ae00} \u{c78e} \u{c78e}\n\u{c900}\u{d0dd} \u{c2a4}\u{d1a0}\u{c5b4} \u{c2a4}\u{d1a0}\u{c5b4}\n"
                .as_bytes(),
        );
        // "잎" (leaf) twice on line 0.
        let results = run(root.path(), &["a.md"], "\u{c78e}");
        assert_eq!(results.files[0].lines[0].ranges, [7..10, 11..14]);
        // Accented letters fold too; the ranges point into the original text.
        write(
            root.path(),
            "b.md",
            "\u{c0}BC \u{e0}bc \u{c0}bc\n".as_bytes(),
        );
        let results = run(root.path(), &["b.md"], "\u{e0}bc");
        assert_eq!(
            hits(&results),
            [
                hit("b.md", 0, "\u{c0}BC"),
                hit("b.md", 0, "\u{e0}bc"),
                hit("b.md", 0, "\u{c0}bc"),
            ]
        );
        let results = run(root.path(), &["b.md"], "\u{c0}bc");
        assert_eq!(hits(&results), [hit("b.md", 0, "\u{c0}bc")]);
    }

    #[test]
    fn decomposed_queries_match_composed_text() {
        let root = tempfile::tempdir().unwrap();
        write(root.path(), "a.md", "\u{d55c}\u{ae00}\n".as_bytes());
        let nfd = "\u{1112}\u{1161}\u{11ab}";
        let results = run(root.path(), &["a.md"], nfd);
        assert_eq!(hits(&results), [hit("a.md", 0, "\u{d55c}")]);
    }

    #[test]
    fn stops_at_the_limit_and_says_so() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        write(root, "a.md", b"x x\nx\n");
        write(root, "b.md", b"x\n");
        let paths = ["a.md".to_string(), "b.md".to_string()];
        let search = |limit| search_with_limit(root, &paths, "x", &AtomicBool::new(false), limit);
        let results = search(2).unwrap();
        assert_eq!(results.matches, 2);
        assert!(results.capped);
        assert_eq!(results.files.len(), 1);
        assert_eq!(results.files[0].lines.len(), 1);
        // Exactly at the limit when the next match lies in the next file.
        let results = search(3).unwrap();
        assert_eq!((results.matches, results.capped), (3, true));
        assert_eq!(results.files.len(), 1);
        let results = search(4).unwrap();
        assert_eq!((results.matches, results.capped), (4, false));
        assert_eq!(results.files.len(), 2);
    }

    #[test]
    fn the_cap_keeps_the_first_files_in_index_order() {
        let root = tempfile::tempdir().unwrap();
        let paths: Vec<String> = (0..100).map(|i| format!("f{i:03}.md")).collect();
        for path in &paths {
            write(root.path(), path, b"x\n");
        }
        let results =
            search_with_limit(root.path(), &paths, "x", &AtomicBool::new(false), 10).unwrap();
        let found: Vec<&str> = results.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(found, paths[..10]);
        assert!(results.capped);
        let results = run(root.path(), &[], "x");
        assert_eq!(results, Results::default());
        let all = search(root.path(), &paths, "x", &AtomicBool::new(false)).unwrap();
        let found: Vec<&str> = all.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(found, paths);
    }

    #[test]
    fn folds_only_as_much_as_the_query_needs() {
        let fold = |query: &str| Needle::new(query).unwrap().fold;
        assert_eq!(fold("path"), Fold::Ascii);
        assert_eq!(fold("Path"), Fold::None);
        assert_eq!(fold("12 -> 3"), Fold::None);
        assert_eq!(fold("\u{d55c}\u{ae00}"), Fold::None);
        assert_eq!(fold("\u{d55c}\u{ae00} path"), Fold::Ascii);
        assert_eq!(fold("\u{e0}bc"), Fold::Full);
        assert_eq!(fold("stra\u{df}e"), Fold::Full);
    }

    #[test]
    fn a_cancelled_search_returns_nothing() {
        let root = tempfile::tempdir().unwrap();
        write(root.path(), "a.md", b"leaf\n");
        let paths = ["a.md".to_string()];
        assert_eq!(
            search(root.path(), &paths, "leaf", &AtomicBool::new(true)),
            None
        );
    }

    #[test]
    fn an_empty_query_finds_nothing() {
        let root = tempfile::tempdir().unwrap();
        write(root.path(), "a.md", b"leaf\n");
        assert_eq!(run(root.path(), &["a.md"], ""), Results::default());
    }

    #[test]
    fn shown_lines_drop_indentation_and_tabs() {
        let root = tempfile::tempdir().unwrap();
        write(root.path(), "a.rs", b"\t\tlet leaf =\t1;\n");
        let line = &run(root.path(), &["a.rs"], "leaf").files[0].lines[0];
        assert_eq!(line.text, "let leaf = 1;");
        assert_eq!(line.ranges, [4..8]);
        // A query of spaces matched inside the indentation stays visible.
        write(root.path(), "b.rs", b"    x\n");
        let line = &run(root.path(), &["b.rs"], "  ").files[0].lines[0];
        assert_eq!(line.text, "    x");
        assert_eq!(line.ranges, [0..2, 2..4]);
    }

    #[test]
    fn long_lines_are_cut_around_the_first_match() {
        let root = tempfile::tempdir().unwrap();
        let text = format!("{}leaf{}\n", "a".repeat(100), "b".repeat(500));
        write(root.path(), "min.js", text.as_bytes());
        let line = &run(root.path(), &["min.js"], "leaf").files[0].lines[0];
        let want = format!("{CUT}{}leaf{}", "a".repeat(40), "b".repeat(156));
        assert_eq!(line.text, want);
        assert_eq!(line.ranges, [43..47]);
        // A match past the cut is counted but not shown.
        let text = format!("leaf{}leaf\n", "c".repeat(300));
        write(root.path(), "long.txt", text.as_bytes());
        let results = run(root.path(), &["long.txt"], "leaf");
        assert_eq!(results.matches, 2);
        assert_eq!(results.files[0].lines[0].ranges, [0..4]);
        assert_eq!(results.files[0].lines[0].text.chars().count(), 200);
    }
}
