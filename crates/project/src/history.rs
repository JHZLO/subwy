//! Local history: the texts subwy saved of each file, kept outside its folder, to compare with the
//! file now and to go back to. Each kept text is a station on the file's line.
//!
//! A save rides on the latest station when that one began less than [`RIDE`] ago and is what the
//! file held, so a burst of auto-saves leaves one station, not hundreds. A text the history has not
//! seen (the file before subwy first saved it, or as another program left it) gets a station of its
//! own before it is written over, so nothing on disk is lost to a save.

use std::{
    io,
    os::unix::ffi::OsStrExt as _,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use similar::{ChangeTag, DiffTag, TextDiff};

/// How long saves keep riding on the station they began.
pub const RIDE: Duration = Duration::from_secs(5 * 60);
/// Stations older than this go when the file is next saved.
const KEEP_FOR: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// The most stations a file keeps.
const MOST: usize = 100;
/// Unchanged lines shown around each change.
const CONTEXT: usize = 3;

/// `~/Library/Application Support/subwy/history`, or `SUBWY_HISTORY_PATH` so tests never touch it.
pub fn default_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("SUBWY_HISTORY_PATH") {
        return PathBuf::from(path);
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join("Library/Application Support/subwy/history")
}

/// A kept text of a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Station {
    /// When it began, in milliseconds since 1970; it names the station's file and orders them.
    id: u64,
    file: PathBuf,
    /// When its text was last written to the file.
    pub saved: SystemTime,
}

impl Station {
    pub fn read(&self) -> io::Result<String> {
        std::fs::read_to_string(&self.file)
    }
}

/// `file`'s stations, newest first.
pub fn stations(dir: &Path, file: &Path) -> Vec<Station> {
    let mut stations = listed(&folder(dir, file));
    stations.reverse();
    stations
}

/// Records a save of `file` (absolute): `before` is the text the save replaced and when it was
/// last changed, `None` for a new file; `written` is the text now there.
pub fn keep(
    dir: &Path,
    file: &Path,
    before: Option<(&str, SystemTime)>,
    written: &str,
    now: SystemTime,
) -> io::Result<()> {
    let folder = folder(dir, file);
    let stations = listed(&folder);
    let latest = stations.last();
    let latest_text = latest.and_then(|station| station.read().ok());
    if latest_text.as_deref() == Some(written) {
        return Ok(());
    }
    std::fs::create_dir_all(&folder)?;
    let now_ms = millis(now);
    let mut next = latest.map_or(now_ms, |station| now_ms.max(station.id + 1));
    // The file held what the latest station holds: this save goes on from it.
    let seen = before.map(|(text, _)| text) == latest_text.as_deref();
    if let Some((text, modified)) = before
        && !seen
        && text != written
    {
        put(&folder, next, text, modified)?;
        next += 1;
    }
    match latest {
        Some(station) if seen && now_ms.saturating_sub(station.id) < RIDE.as_millis() as u64 => {
            put(&folder, station.id, written, now)?;
        }
        _ => put(&folder, next, written, now)?,
    }
    prune(&folder, now);
    Ok(())
}

/// Where a file's stations are kept: a folder named for the file's path.
fn folder(dir: &Path, file: &Path) -> PathBuf {
    dir.join(format!("{:016x}", fnv1a(file.as_os_str().as_bytes())))
}

/// A stable hash (64-bit FNV-1a); std's may change between Rust releases, losing the folders.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// The stations in `folder`, oldest first.
fn listed(folder: &Path) -> Vec<Station> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut stations: Vec<Station> = entries
        .flatten()
        .filter_map(|entry| {
            let file = entry.path();
            let id = file.file_stem()?.to_str()?.parse().ok()?;
            let saved = entry.metadata().ok()?.modified().ok()?;
            Some(Station { id, file, saved })
        })
        .collect();
    stations.sort_by_key(|station| station.id);
    stations
}

/// Writes station `id`'s text, dated `saved`.
fn put(folder: &Path, id: u64, text: &str, saved: SystemTime) -> io::Result<()> {
    let file = folder.join(format!("{id}.txt"));
    std::fs::write(&file, text)?;
    std::fs::File::options()
        .write(true)
        .open(&file)?
        .set_modified(saved)
}

/// Drops stations past [`KEEP_FOR`], then the oldest past [`MOST`].
fn prune(folder: &Path, now: SystemTime) {
    let stations = listed(folder);
    let stale = stations.len().saturating_sub(MOST);
    for (index, station) in stations.iter().enumerate() {
        let old = now
            .duration_since(station.saved)
            .is_ok_and(|age| age > KEEP_FOR);
        if old || index < stale {
            let _ = std::fs::remove_file(&station.file);
        }
    }
}

fn millis(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64)
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// When a station was saved, as its row says it: "Just now", "3 min ago", "Today 14:20",
/// "Yesterday 18:42", "Nov 17 09:10".
pub fn when(saved: SystemTime, now: SystemTime) -> String {
    let ago = now.duration_since(saved).unwrap_or_default();
    if ago < Duration::from_secs(60) {
        return "Just now".to_string();
    }
    if ago < Duration::from_secs(60 * 60) {
        return format!("{} min ago", ago.as_secs() / 60);
    }
    let at = local(saved);
    let time = format!("{:02}:{:02}", at.tm_hour, at.tm_min);
    let day = |tm: &libc::tm| (tm.tm_year, tm.tm_yday);
    let today = local(now);
    if day(&at) == day(&today) {
        format!("Today {time}")
    } else if day(&local(saved + Duration::from_secs(24 * 60 * 60))) == day(&today) {
        format!("Yesterday {time}")
    } else {
        let month = MONTHS[at.tm_mon.clamp(0, 11) as usize];
        format!("{month} {} {time}", at.tm_mday)
    }
}

/// `time` on the Mac's clock and calendar.
fn local(time: SystemTime) -> libc::tm {
    let secs = millis(time) / 1000;
    // Safe: localtime_r writes only into `tm`, which is plain data.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&(secs as libc::time_t), &mut tm);
        tm
    }
}

/// A station's text beside the file's: the changed lines, a few unchanged ones around each, and
/// how many lines went and came.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Comparison {
    pub rows: Vec<Row>,
    /// Lines of the station the file no longer has.
    pub gone: usize,
    /// Lines of the file the station does not have.
    pub added: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Row {
    /// A line, with its number in the station's text (one-based) unless only the file has it.
    Line {
        change: Change,
        line: Option<usize>,
        text: String,
    },
    /// This many unchanged lines left out.
    Skipped(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Same,
    Gone,
    Added,
}

/// How `old` (a station) differs from `new` (the file now), line by line.
pub fn compare(old: &str, new: &str) -> Comparison {
    let diff = TextDiff::from_lines(old, new);
    let mut comparison = Comparison::default();
    for op in diff.ops() {
        let (tag, old_range, new_range) = op.as_tag_tuple();
        match tag {
            DiffTag::Equal => {}
            DiffTag::Delete => comparison.gone += old_range.len(),
            DiffTag::Insert => comparison.added += new_range.len(),
            DiffTag::Replace => {
                comparison.gone += old_range.len();
                comparison.added += new_range.len();
            }
        }
    }
    let old_lines = diff.old_slices().len();
    let mut next = 0;
    for group in diff.grouped_ops(CONTEXT) {
        let Some(first) = group.first() else {
            continue;
        };
        let start = first.old_range().start;
        if start > next {
            comparison.rows.push(Row::Skipped(start - next));
        }
        for op in &group {
            for change in diff.iter_changes(op) {
                let text = change.value().trim_end_matches(['\n', '\r']).to_string();
                let (change_kind, line) = match change.tag() {
                    ChangeTag::Equal => (Change::Same, change.old_index()),
                    ChangeTag::Delete => (Change::Gone, change.old_index()),
                    ChangeTag::Insert => (Change::Added, None),
                };
                comparison.rows.push(Row::Line {
                    change: change_kind,
                    line: line.map(|index| index + 1),
                    text,
                });
            }
            next = op.old_range().end;
        }
    }
    if !comparison.rows.is_empty() && next < old_lines {
        comparison.rows.push(Row::Skipped(old_lines - next));
    }
    comparison
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1_800_000_000 + secs)
    }

    fn texts(dir: &Path, file: &Path) -> Vec<String> {
        stations(dir, file)
            .iter()
            .map(|station| station.read().expect("read"))
            .collect()
    }

    #[test]
    fn saves_close_together_ride_on_one_station() {
        let dir = tempfile::tempdir().expect("dir");
        let file = Path::new("/notes/a.md");
        keep(dir.path(), file, None, "one", at(0)).expect("keep");
        keep(dir.path(), file, Some(("one", at(0))), "two", at(60)).expect("keep");
        keep(dir.path(), file, Some(("two", at(60))), "three", at(120)).expect("keep");
        assert_eq!(texts(dir.path(), file), ["three"]);
        // Past the ride, the next save starts a station.
        keep(dir.path(), file, Some(("three", at(120))), "four", at(400)).expect("keep");
        assert_eq!(texts(dir.path(), file), ["four", "three"]);
    }

    #[test]
    fn a_text_not_seen_is_kept_before_it_is_written_over() {
        let dir = tempfile::tempdir().expect("dir");
        let file = Path::new("/notes/a.md");
        // The file as another program wrote it, saved over by subwy for the first time.
        keep(dir.path(), file, Some(("theirs", at(0))), "mine", at(100)).expect("keep");
        assert_eq!(texts(dir.path(), file), ["mine", "theirs"]);
        let first = stations(dir.path(), file).pop().expect("station");
        assert_eq!(first.saved, at(0));
        // Changed outside since: that text gets a station too, though the ride has not ended.
        keep(
            dir.path(),
            file,
            Some(("edited", at(110))),
            "mine again",
            at(120),
        )
        .expect("keep");
        assert_eq!(
            texts(dir.path(), file),
            ["mine again", "edited", "mine", "theirs"]
        );
    }

    #[test]
    fn saving_the_same_text_adds_nothing() {
        let dir = tempfile::tempdir().expect("dir");
        let file = Path::new("/notes/a.md");
        keep(dir.path(), file, Some(("same", at(0))), "same", at(10)).expect("keep");
        keep(dir.path(), file, Some(("same", at(10))), "same", at(1000)).expect("keep");
        assert_eq!(texts(dir.path(), file), ["same"]);
    }

    #[test]
    fn each_file_has_its_own_line() {
        let dir = tempfile::tempdir().expect("dir");
        keep(dir.path(), Path::new("/a.md"), None, "a", at(0)).expect("keep");
        keep(dir.path(), Path::new("/b.md"), None, "b", at(0)).expect("keep");
        assert_eq!(texts(dir.path(), Path::new("/a.md")), ["a"]);
        assert_eq!(texts(dir.path(), Path::new("/b.md")), ["b"]);
    }

    #[test]
    fn old_stations_go_on_the_next_save() {
        let dir = tempfile::tempdir().expect("dir");
        let file = Path::new("/notes/a.md");
        keep(dir.path(), file, None, "old", at(0)).expect("keep");
        let later = at(0) + KEEP_FOR + Duration::from_secs(60);
        keep(dir.path(), file, Some(("old", at(0))), "new", later).expect("keep");
        assert_eq!(texts(dir.path(), file), ["new"]);
    }

    #[test]
    fn a_file_keeps_the_latest_stations() {
        let dir = tempfile::tempdir().expect("dir");
        let file = Path::new("/notes/a.md");
        let mut before: Option<String> = None;
        for n in 0..MOST as u64 + 3 {
            let text = n.to_string();
            let now = at(n * 600);
            keep(
                dir.path(),
                file,
                before.as_deref().map(|b| (b, now)),
                &text,
                now,
            )
            .expect("keep");
            before = Some(text);
        }
        let kept = texts(dir.path(), file);
        assert_eq!(kept.len(), MOST);
        assert_eq!(kept.first().map(String::as_str), Some("102"));
    }

    #[test]
    fn times_read_as_the_row_says_them() {
        let now = at(100_000);
        assert_eq!(when(now - Duration::from_secs(20), now), "Just now");
        assert_eq!(
            when(now - Duration::from_secs(3 * 60 + 5), now),
            "3 min ago"
        );
        let long_ago = now - Duration::from_secs(40 * 24 * 60 * 60);
        let tm = local(long_ago);
        assert_eq!(
            when(long_ago, now),
            format!(
                "{} {} {:02}:{:02}",
                MONTHS[tm.tm_mon as usize], tm.tm_mday, tm.tm_hour, tm.tm_min
            )
        );
    }

    #[test]
    fn a_comparison_shows_changes_with_lines_around_them() {
        let old: String = (1..=20).map(|n| format!("line {n}\n")).collect();
        let new = old
            .replace("line 10\n", "line ten\n")
            .replace("line 11\n", "");
        let comparison = compare(&old, &new);
        assert_eq!((comparison.gone, comparison.added), (2, 1));
        let line = |change, line: Option<usize>, text: &str| Row::Line {
            change,
            line,
            text: text.to_string(),
        };
        assert_eq!(
            comparison.rows,
            [
                Row::Skipped(6),
                line(Change::Same, Some(7), "line 7"),
                line(Change::Same, Some(8), "line 8"),
                line(Change::Same, Some(9), "line 9"),
                line(Change::Gone, Some(10), "line 10"),
                line(Change::Gone, Some(11), "line 11"),
                line(Change::Added, None, "line ten"),
                line(Change::Same, Some(12), "line 12"),
                line(Change::Same, Some(13), "line 13"),
                line(Change::Same, Some(14), "line 14"),
                Row::Skipped(6),
            ]
        );
        assert_eq!(compare(&old, &old), Comparison::default());
    }
}
