//! IntelliJ's line keys: duplicate, delete and move the lines under the selection. Pure, like
//! [`crate::markdown`]: the whole text and the selection in, an [`Edit`] out; the app applies it.
//! Offsets are bytes, as the editor counts them, and lines end in `\n` (files load with LF).

use std::ops::Range;

use crate::markdown::Edit;

/// Where the line holding `offset` starts.
fn line_start(text: &str, offset: usize) -> usize {
    text[..offset].rfind('\n').map_or(0, |i| i + 1)
}

/// Where the line holding `offset` ends, before its newline.
fn line_end(text: &str, offset: usize) -> usize {
    text[offset..].find('\n').map_or(text.len(), |i| offset + i)
}

/// The whole lines under `selection`, without the last one's newline. A selection that ends at
/// the start of a line leaves that line out, as in IntelliJ.
fn lines(text: &str, selection: &Range<usize>) -> Range<usize> {
    let end = match selection.end > selection.start && text[..selection.end].ends_with('\n') {
        true => selection.end - 1,
        false => selection.end,
    };
    line_start(text, selection.start)..line_end(text, end)
}

/// The offset `column` characters into `line`, or its end when it is shorter.
fn at_column(line: &str, column: usize) -> usize {
    line.char_indices()
        .nth(column)
        .map_or(line.len(), |(i, _)| i)
}

/// Cmd+D: a copy of the selection right after it, selected; without one, a copy of the caret's
/// line below it, with the caret on the copy.
pub fn duplicate(text: &str, selection: Range<usize>) -> Edit {
    if selection.is_empty() {
        let line = lines(text, &selection);
        let copy = format!("\n{}", &text[line.clone()]);
        let caret = selection.start + copy.len();
        return Edit {
            range: line.end..line.end,
            text: copy,
            selection: caret..caret,
        };
    }
    let copy = text[selection.clone()].to_string();
    let end = selection.end + copy.len();
    Edit {
        range: selection.end..selection.end,
        text: copy,
        selection: selection.end..end,
    }
}

/// Cmd+Backspace: the lines under the selection go, with one newline. The caret keeps its column
/// on the line that takes their place, the one above when they were the last.
pub fn delete(text: &str, selection: Range<usize>) -> Edit {
    let lines = lines(text, &selection);
    let column = text[lines.start..selection.start].chars().count();
    if lines.end < text.len() {
        let next = &text[lines.end + 1..line_end(text, lines.end + 1)];
        let caret = lines.start + at_column(next, column);
        Edit {
            range: lines.start..lines.end + 1,
            text: String::new(),
            selection: caret..caret,
        }
    } else if lines.start > 0 {
        let above = line_start(text, lines.start - 1);
        let caret = above + at_column(&text[above..lines.start - 1], column);
        Edit {
            range: lines.start - 1..lines.end,
            text: String::new(),
            selection: caret..caret,
        }
    } else {
        Edit {
            range: lines,
            text: String::new(),
            selection: 0..0,
        }
    }
}

/// Alt+Shift+Up and Down: the lines under the selection trade places with the line above or
/// below, and the selection moves with them. `None` at the top or the bottom. The empty line after
/// a final newline is neither moved nor moved past, so the file still ends in one.
pub fn move_lines(text: &str, selection: Range<usize>, up: bool) -> Option<Edit> {
    let lines = lines(text, &selection);
    if lines.start == text.len() {
        return None;
    }
    let moved = &text[lines.clone()];
    if up {
        if lines.start == 0 {
            return None;
        }
        let above = line_start(text, lines.start - 1)..lines.start - 1;
        let shift = above.len() + 1;
        Some(Edit {
            range: above.start..lines.end,
            text: format!("{moved}\n{}", &text[above.clone()]),
            selection: selection.start - shift..selection.end - shift,
        })
    } else {
        if lines.end + 1 >= text.len() {
            return None;
        }
        let below = lines.end + 1..line_end(text, lines.end + 1);
        let shift = below.len() + 1;
        // A selection that ended at the start of the line below ends at the text's end when no
        // line follows the moved ones.
        let end = (selection.end + shift).min(text.len());
        Some(Edit {
            range: lines.start..below.end,
            text: format!("{}\n{moved}", &text[below.clone()]),
            selection: selection.start + shift..end,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `marked` as text and selection: the caret at `|`, or a selection between `[` and `]`.
    fn parse(marked: &str) -> (String, Range<usize>) {
        if let Some(at) = marked.find('|') {
            return (marked.replace('|', ""), at..at);
        }
        let start = marked.find('[').expect("caret or selection");
        let end = marked.find(']').expect("selection end") - 1;
        (marked.replace(['[', ']'], ""), start..end)
    }

    /// Applies `edit` to `text` and marks the selection it leaves, as [`parse`] reads it.
    fn apply(text: &str, edit: Edit) -> String {
        let mut out = text.to_string();
        out.replace_range(edit.range, &edit.text);
        let Range { start, end } = edit.selection;
        if start == end {
            out.insert(start, '|');
        } else {
            out.insert(end, ']');
            out.insert(start, '[');
        }
        out
    }

    fn duplicated(marked: &str) -> String {
        let (text, selection) = parse(marked);
        apply(&text, duplicate(&text, selection))
    }

    fn deleted(marked: &str) -> String {
        let (text, selection) = parse(marked);
        apply(&text, delete(&text, selection))
    }

    fn moved(marked: &str, up: bool) -> Option<String> {
        let (text, selection) = parse(marked);
        move_lines(&text, selection, up).map(|edit| apply(&text, edit))
    }

    #[test]
    fn duplicate_copies_the_line_or_the_selection() {
        assert_eq!(duplicated("a\nb|c\nd"), "a\nbc\nb|c\nd");
        assert_eq!(duplicated("last|"), "last\nlast|");
        assert_eq!(duplicated("|"), "\n|");
        assert_eq!(duplicated("한|글"), "한글\n한|글");
        assert_eq!(duplicated("x [ab] y"), "x ab[ab] y");
        assert_eq!(duplicated("[a\nb]\nc"), "a\nb[a\nb]\nc");
    }

    #[test]
    fn delete_takes_whole_lines_and_keeps_the_column() {
        assert_eq!(deleted("one\ntw|o\nthree"), "one\nth|ree");
        // A shorter line takes the caret to its end.
        assert_eq!(deleted("one\nlonger|\nab"), "one\nab|");
        // The last line takes the newline before it, and the caret goes up.
        assert_eq!(deleted("one\ntw|o"), "on|e");
        assert_eq!(deleted("on|ly"), "|");
        // A final newline stays.
        assert_eq!(deleted("a\nb|\n"), "a\n|");
        assert_eq!(deleted("[a\nb]\nc"), "|c");
        // A selection that ends at a line's start leaves that line.
        assert_eq!(deleted("[a\n]b\nc"), "|b\nc");
    }

    #[test]
    fn move_trades_places_with_the_next_line() {
        assert_eq!(moved("a\nb|b\nc", true).as_deref(), Some("b|b\na\nc"));
        assert_eq!(moved("a\nb|b\nc", false).as_deref(), Some("a\nc\nb|b"));
        assert_eq!(moved("a\n[b\nc]\nd", true).as_deref(), Some("[b\nc]\na\nd"));
        assert_eq!(
            moved("a\n[b\nc]\nd", false).as_deref(),
            Some("a\nd\n[b\nc]")
        );
        // A selection that ends at a line's start moves the lines above it only.
        assert_eq!(
            moved("a\n[b\n]c\nd", false).as_deref(),
            Some("a\nc\n[b\n]d")
        );
        assert_eq!(moved("a\n[b\n]c", false).as_deref(), Some("a\nc\n[b]"));
        assert_eq!(moved("|a\nb", true), None);
        assert_eq!(moved("a\n|b", false), None);
    }

    #[test]
    fn move_keeps_the_final_newline() {
        assert_eq!(moved("a\nb|\n", false), None);
        assert_eq!(moved("a\nb\n|", true), None);
        assert_eq!(moved("a|\nb\n", false).as_deref(), Some("b\na|\n"));
    }
}
