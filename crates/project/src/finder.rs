//! How the finder shows a result: the file's name, and the folders on the way to it written as
//! the stations of a route, from the window's own folder. And the query that asks for a line.

use std::ops::Range;

/// One result's text, with the query's matched characters as byte ranges into each part.
#[derive(Debug, PartialEq, Eq)]
pub struct Route {
    pub name: String,
    pub name_marks: Vec<Range<usize>>,
    /// The folders on the way, the window's own first: the rings of the drawn route and the
    /// "N stops" count.
    pub stops: usize,
    /// "via a, b, c", or "direct from a" for a file at the top.
    pub via: String,
    pub via_marks: Vec<Range<usize>>,
}

/// The route to `path` (`/`-separated, as the finder lists it), with `positions` the matched
/// char indices. `window` is the folder name put in front: the window's one folder, or `None`
/// when the path already starts with its folder's name (a window of several folders).
pub fn route(path: &str, positions: &[usize], window: Option<&str>) -> Route {
    let name_start = path.rfind('/').map_or(0, |slash| slash + 1);
    let folders = path[..name_start].matches('/').count();
    let stops = folders + usize::from(window.is_some());
    let mut via = String::from(match stops {
        0 => "",
        1 => "direct from ",
        _ => "via ",
    });
    if let Some(window) = window {
        via.push_str(window);
        if folders > 0 {
            via.push_str(", ");
        }
    }
    let (mut name, mut name_marks, mut via_marks) = (String::new(), Vec::new(), Vec::new());
    let mut positions = positions.iter().peekable();
    for (i, (byte, ch)) in path.char_indices().enumerate() {
        let matched = positions.next_if_eq(&&i).is_some();
        // A matched `/` shows in neither part.
        if byte >= name_start {
            if matched {
                name_marks.push(name.len()..name.len() + ch.len_utf8());
            }
            name.push(ch);
        } else if ch == '/' {
            if byte + 1 < name_start {
                via.push_str(", ");
            }
        } else {
            if matched {
                via_marks.push(via.len()..via.len() + ch.len_utf8());
            }
            via.push(ch);
        }
    }
    Route {
        name,
        name_marks,
        stops,
        via,
        via_marks,
    }
}

/// A query for a line of the open file, as Cmd+L starts it (IntelliJ's Go to Line): `:12` or
/// `:12:5`, the line and the column counted from one. `None` for a query of a file, or for a line
/// still being typed.
pub fn line_query(query: &str) -> Option<(u32, u32)> {
    let rest = query.strip_prefix(':')?;
    let (line, column) = rest.split_once(':').unwrap_or((rest, ""));
    let line = line.trim().parse::<u32>().ok()?.max(1);
    let column = match column.trim() {
        "" => 1,
        column => column.parse::<u32>().ok()?.max(1),
    };
    Some((line, column))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_read_as_stations_from_the_window_folder() {
        // "docs/guide/keys.md" with d, g, /, k, s matched.
        let found = route("docs/guide/keys.md", &[0, 5, 10, 11, 14], Some("leaf"));
        assert_eq!(found.name, "keys.md");
        assert_eq!(found.name_marks, [0..1, 3..4]);
        assert_eq!(found.stops, 3);
        assert_eq!(found.via, "via leaf, docs, guide");
        // "d" of docs and "g" of guide, after "via leaf, ".
        assert_eq!(found.via_marks, [10..11, 16..17]);
    }

    #[test]
    fn a_file_at_the_top_is_direct_from_the_window_folder() {
        let found = route("PRD.md", &[0], Some("leaf"));
        assert_eq!(found.name, "PRD.md");
        assert_eq!(found.name_marks, vec![0..1]);
        assert_eq!(found.stops, 1);
        assert_eq!(found.via, "direct from leaf");
        assert!(found.via_marks.is_empty());
    }

    #[test]
    fn with_several_folders_the_path_starts_at_its_folder() {
        let found = route("notes/a.md", &[0], None);
        assert_eq!(found.stops, 1);
        assert_eq!(found.via, "direct from notes");
        assert_eq!(found.via_marks, vec![12..13]);
        let found = route("notes/queries/q.sql", &[], None);
        assert_eq!(found.stops, 2);
        assert_eq!(found.via, "via notes, queries");
    }

    #[test]
    fn a_colon_asks_for_a_line() {
        assert_eq!(line_query(":12"), Some((12, 1)));
        assert_eq!(line_query(":12:5"), Some((12, 5)));
        assert_eq!(line_query(": 3 "), Some((3, 1)));
        assert_eq!(line_query(":0"), Some((1, 1)));
        assert_eq!(line_query(":7:"), Some((7, 1)));
        assert_eq!(line_query(":"), None);
        assert_eq!(line_query(":x"), None);
        assert_eq!(line_query("12"), None);
    }

    #[test]
    fn marks_are_in_bytes() {
        let found = route("\u{d55c}/\u{ae00}.md", &[0, 2], Some("leaf"));
        assert_eq!(found.via, "via leaf, \u{d55c}");
        assert_eq!(found.via_marks, vec![10..13]);
        assert_eq!(found.name_marks, vec![0..3]);
    }
}
