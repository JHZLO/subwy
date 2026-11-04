//! IntelliJ's line keys: duplicate, delete, move and comment out the lines under the selection,
//! and select the next occurrence. Pure, like [`crate::markdown`]: the whole text and the selection in, an [`Edit`] out; the app
//! applies it. Offsets are bytes, as the editor counts them, and lines end in `\n` (files load
//! with LF).

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

/// How a tab's language (its name, or its file's extension) comments out a line: what opens the
/// comment, and what closes it where the line's end does not.
pub fn comment_for(language: &str) -> Option<(&'static str, &'static str)> {
    Some(match language {
        "rs" | "kt" | "kts" | "java" | "typescript" | "tsx" | "javascript" | "jsx" | "go" | "c"
        | "h" | "cpp" | "cc" | "hpp" | "swift" | "scala" | "dart" | "groovy" | "gradle"
        | "scss" | "jsonc" => ("//", ""),
        "py" | "sh" | "bash" | "zsh" | "toml" | "yaml" | "rb" | "r" | "pl" => ("#", ""),
        "sql" | "lua" | "hs" => ("--", ""),
        "markdown" | "html" | "htm" | "xml" | "svg" => ("<!--", "-->"),
        "css" => ("/*", "*/"),
        _ => return None,
    })
}

/// Cmd+/: comments out the lines under the selection, or takes the comments off when every line
/// with text has one. Blank lines among others stay as they are. The comment opens at the lines'
/// least indent, so the openers line up. A lone caret then goes to the next line, as in IntelliJ,
/// so the key repeats down a block.
pub fn toggle_comment(text: &str, selection: Range<usize>, open: &str, close: &str) -> Edit {
    let lines = lines(text, &selection);
    let block = &text[lines.clone()];
    let filled = |line: &&str| !line.trim().is_empty();
    let all_blank = !block.split('\n').any(|line| filled(&line));
    let touched = |line: &&str| all_blank || filled(line);
    let commented = |line: &str| {
        let line = line.trim();
        line.starts_with(open) && line.ends_with(close)
    };
    let uncomment = !all_blank && block.split('\n').filter(touched).all(commented);
    let indent = block
        .split('\n')
        .filter(touched)
        .map(|line| line.len() - line.trim_start_matches([' ', '\t']).len())
        .min()
        .unwrap_or(0);
    let changed = block
        .split('\n')
        .map(|line| match (touched(&line), uncomment) {
            (false, _) => line.to_string(),
            (true, true) => uncomment_line(line, open, close),
            (true, false) => comment_line(line, indent, open, close),
        })
        .collect::<Vec<_>>()
        .join("\n");
    let end = lines.start + changed.len();
    let selection = if !selection.is_empty() {
        lines.start..end
    } else if lines.end < text.len() {
        let column = text[lines.start..selection.start].chars().count();
        let next = &text[lines.end + 1..line_end(text, lines.end + 1)];
        let caret = end + 1 + at_column(next, column);
        caret..caret
    } else {
        end..end
    };
    Edit {
        range: lines,
        text: changed,
        selection,
    }
}

/// `line` commented out from `indent`, a run of spaces and tabs every touched line starts with.
fn comment_line(line: &str, indent: usize, open: &str, close: &str) -> String {
    let (lead, body) = line.split_at(indent);
    match (body.trim().is_empty(), close.is_empty()) {
        (true, true) => format!("{lead}{open}"),
        (true, false) => format!("{lead}{open} {close}"),
        (false, true) => format!("{lead}{open} {body}"),
        (false, false) => format!("{lead}{open} {body} {close}"),
    }
}

/// `line` without its comment: the opener with a space after it, the closer with one before it.
fn uncomment_line(line: &str, open: &str, close: &str) -> String {
    let body = line.trim_start();
    let lead = &line[..line.len() - body.len()];
    let body = body.strip_prefix(open).unwrap_or(body);
    let body = body.strip_prefix(' ').unwrap_or(body);
    let body = match close.is_empty() {
        true => body,
        false => {
            let body = body.trim_end();
            let body = body.strip_suffix(close).unwrap_or(body);
            body.strip_suffix(' ').unwrap_or(body)
        }
    };
    format!("{lead}{body}")
}

/// Ctrl+G: where the text at `from` comes again after it, going round to the start; `None` when
/// it is nowhere else. With `whole_word` (Ctrl+G took a word at the caret), a match inside a
/// longer word does not count, as in IntelliJ.
pub fn next_occurrence(text: &str, from: Range<usize>, whole_word: bool) -> Option<Range<usize>> {
    let needle = &text[from.clone()];
    if needle.is_empty() {
        return None;
    }
    let word = |c: char| c.is_alphanumeric() || c == '_';
    let fits = |start: &usize| {
        let end = start + needle.len();
        !whole_word
            || !(text[..*start].chars().next_back().is_some_and(word)
                || text[end..].chars().next().is_some_and(word))
    };
    let after = text[from.end..]
        .match_indices(needle)
        .map(|(i, _)| from.end + i);
    let before = text[..from.start].match_indices(needle).map(|(i, _)| i);
    after
        .chain(before)
        .find(fits)
        .map(|start| start..start + needle.len())
}

/// Cmd+Shift+U (IntelliJ's Toggle Case): the selection, or the word at the caret, in capitals,
/// or in small letters when it has none to raise. What changed stays selected.
pub fn toggle_case(text: &str, selection: Range<usize>) -> Option<Edit> {
    let range = match selection.is_empty() {
        true => word_at(text, selection.start)?,
        false => selection,
    };
    let old = &text[range.clone()];
    let new = match old.chars().any(char::is_lowercase) {
        true => old.to_uppercase(),
        false => old.to_lowercase(),
    };
    (new != old).then(|| Edit {
        selection: range.start..range.start + new.len(),
        range,
        text: new,
    })
}

/// The letters, digits and `_` around `offset`, if there are any.
fn word_at(text: &str, offset: usize) -> Option<Range<usize>> {
    let word = |c: char| c.is_alphanumeric() || c == '_';
    let start = text[..offset]
        .char_indices()
        .rev()
        .take_while(|&(_, c)| word(c))
        .last()
        .map_or(offset, |(i, _)| i);
    let end = text[offset..]
        .char_indices()
        .find(|&(_, c)| !word(c))
        .map_or(text.len(), |(i, _)| offset + i);
    (start < end).then_some(start..end)
}

/// Alt+Up (IntelliJ's Extend Selection): the smallest stretch around the selection that is larger
/// than it: the word at the caret, inside a pair of quotes or brackets, the pair with them, the
/// line without its indent, the whole line, the paragraph, all the text. None when all is selected.
pub fn extend_selection(text: &str, selection: Range<usize>) -> Option<Range<usize>> {
    let line = line_start(text, selection.start)..line_end(text, selection.end);
    let mut candidates: Vec<Range<usize>> = word_at(text, selection.start).into_iter().collect();
    for (open, close) in [('(', ')'), ('[', ']'), ('{', '}')] {
        if let Some(pair) = enclosing_pair(text, &selection, open, close) {
            candidates.push(pair.start + 1..pair.end - 1);
            candidates.push(pair);
        }
    }
    for quote in ['"', '`'] {
        if let Some(pair) = enclosing_quotes(text, &line, &selection, quote) {
            candidates.push(pair.start + 1..pair.end - 1);
            candidates.push(pair);
        }
    }
    let shown = &text[line.clone()];
    let indent = shown.len() - shown.trim_start().len();
    candidates.push(line.start + indent..line.start + shown.trim_end().len());
    candidates.push(paragraph(text, &line));
    candidates.push(line);
    candidates.push(0..text.len());
    candidates
        .into_iter()
        .filter(|c| {
            c.start <= selection.start && c.end >= selection.end && c.len() > selection.len()
        })
        .min_by_key(|c| c.len())
}

/// The innermost `open` and `close` around `selection`, both included, nested pairs skipped.
fn enclosing_pair(
    text: &str,
    selection: &Range<usize>,
    open: char,
    close: char,
) -> Option<Range<usize>> {
    let mut depth = 0usize;
    let (start, _) = text[..selection.start]
        .char_indices()
        .rev()
        .find(|&(_, c)| {
            if c == close {
                depth += 1;
            } else if c == open {
                if depth == 0 {
                    return true;
                }
                depth -= 1;
            }
            false
        })?;
    let mut depth = 0usize;
    let (end, _) = text[selection.end..].char_indices().find(|&(_, c)| {
        if c == open {
            depth += 1;
        } else if c == close {
            if depth == 0 {
                return true;
            }
            depth -= 1;
        }
        false
    })?;
    Some(start..selection.end + end + close.len_utf8())
}

/// The quotes around `selection` on its `line`, both included: the last one before it that opens
/// (an even number of them comes before it on the line) and the first one after it.
fn enclosing_quotes(
    text: &str,
    line: &Range<usize>,
    selection: &Range<usize>,
    quote: char,
) -> Option<Range<usize>> {
    let before = &text[line.start..selection.start];
    let open = before.rfind(quote)?;
    if before[..open].matches(quote).count() % 2 == 1 {
        return None;
    }
    let close = text[selection.end..line.end].find(quote)? + selection.end;
    Some(line.start + open..close + quote.len_utf8())
}

/// The lines around `line` up to the blank lines on either side.
fn paragraph(text: &str, line: &Range<usize>) -> Range<usize> {
    let mut start = line.start;
    while start > 0 {
        let above = line_start(text, start - 1);
        if text[above..start - 1].trim().is_empty() {
            break;
        }
        start = above;
    }
    let mut end = line.end;
    while end < text.len() {
        let below = line_end(text, end + 1);
        if text[end + 1..below].trim().is_empty() {
            break;
        }
        end = below;
    }
    start..end
}

/// Ctrl+Shift+J (IntelliJ's Join Lines): the next line comes up onto the caret's, its indent
/// dropped and one space between; a selection over several lines joins them all. The caret goes
/// to the start of the last line joined. None on the last line.
pub fn join_lines(text: &str, selection: Range<usize>) -> Option<Edit> {
    let mut lines = lines(text, &selection);
    if text[lines.clone()].find('\n').is_none() {
        if lines.end == text.len() {
            return None;
        }
        lines.end = line_end(text, lines.end + 1);
    }
    let mut parts = text[lines.clone()].split('\n');
    let mut joined = parts.next().unwrap_or_default().to_string();
    let mut caret = joined.len();
    for next in parts {
        joined.truncate(joined.trim_end().len());
        let next = next.trim_start();
        if !joined.is_empty() && !next.is_empty() {
            joined.push(' ');
        }
        caret = joined.len();
        joined.push_str(next);
    }
    let caret = lines.start + caret;
    Some(Edit {
        range: lines,
        text: joined,
        selection: caret..caret,
    })
}

/// F9 (Sublime Text's key; IntelliJ's Sort Lines has none): the lines under the selection in
/// order, letter case aside, and selected. None for one line or lines already in order.
pub fn sort_lines(text: &str, selection: Range<usize>) -> Option<Edit> {
    let lines = lines(text, &selection);
    let mut sorted: Vec<&str> = text[lines.clone()].split('\n').collect();
    if sorted.len() < 2 {
        return None;
    }
    sorted.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()).then(a.cmp(b)));
    let sorted = sorted.join("\n");
    (sorted != text[lines.clone()]).then(|| Edit {
        selection: lines.clone(),
        range: lines,
        text: sorted,
    })
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

    fn commented(marked: &str, open: &str, close: &str) -> String {
        let (text, selection) = parse(marked);
        apply(&text, toggle_comment(&text, selection, open, close))
    }

    #[test]
    fn comment_toggles_lines_at_their_least_indent() {
        // A lone caret goes down a line, so the key repeats down a block.
        assert_eq!(
            commented("fn a() {\n    b|();\n}", "//", ""),
            "fn a() {\n    // b();\n}|"
        );
        assert_eq!(commented("    // b|();\nc", "//", ""), "    b();\nc|");
        // A selection: every line it touches but the blank ones, still selected.
        assert_eq!(
            commented("[  a\n\n    b]\nc", "//", ""),
            "[  // a\n\n  //   b]\nc"
        );
        // Off again once every line with text has one.
        assert_eq!(
            commented("[  // a\n\n  //   b]\nc", "//", ""),
            "[  a\n\n    b]\nc"
        );
        assert_eq!(commented("|", "//", ""), "//|");
    }

    #[test]
    fn comment_closes_where_the_language_needs_it() {
        assert_eq!(commented("a|\nb", "<!--", "-->"), "<!-- a -->\nb|");
        assert_eq!(commented("<!-- a -->|", "<!--", "-->"), "a|");
        assert_eq!(comment_for("rs"), Some(("//", "")));
        assert_eq!(comment_for("py"), Some(("#", "")));
        assert_eq!(comment_for("markdown"), Some(("<!--", "-->")));
        assert_eq!(comment_for("json"), None);
    }

    #[test]
    fn next_occurrence_goes_round_and_keeps_to_words() {
        let text = "foo bar foo food foo";
        assert_eq!(next_occurrence(text, 0..3, false), Some(8..11));
        // A word from the caret skips "food"; a selected "foo" does not.
        assert_eq!(next_occurrence(text, 8..11, true), Some(17..20));
        assert_eq!(next_occurrence(text, 8..11, false), Some(12..15));
        assert_eq!(next_occurrence(text, 17..20, true), Some(0..3));
        assert_eq!(next_occurrence("only once", 0..4, false), None);
    }

    #[test]
    fn move_keeps_the_final_newline() {
        assert_eq!(moved("a\nb|\n", false), None);
        assert_eq!(moved("a\nb\n|", true), None);
        assert_eq!(moved("a|\nb\n", false).as_deref(), Some("b\na|\n"));
    }

    fn cased(marked: &str) -> Option<String> {
        let (text, selection) = parse(marked);
        toggle_case(&text, selection).map(|edit| apply(&text, edit))
    }

    #[test]
    fn toggle_case_raises_then_lowers() {
        assert_eq!(cased("let max_size|;").as_deref(), Some("let [MAX_SIZE];"));
        assert_eq!(cased("let [MAX_SIZE];").as_deref(), Some("let [max_size];"));
        assert_eq!(cased("[Mixed Case]").as_deref(), Some("[MIXED CASE]"));
        assert_eq!(cased("a | b"), None);
        assert_eq!(cased("[123]"), None);
    }

    fn joined(marked: &str) -> Option<String> {
        let (text, selection) = parse(marked);
        join_lines(&text, selection).map(|edit| apply(&text, edit))
    }

    #[test]
    fn join_lines_pulls_the_next_line_up() {
        assert_eq!(
            joined("let a =|  \n    1;\nnext").as_deref(),
            Some("let a = |1;\nnext")
        );
        assert_eq!(
            joined("[one\n  two\n  thr]ee").as_deref(),
            Some("one two |three")
        );
        assert_eq!(joined("text|\n\nmore").as_deref(), Some("text|\nmore"));
        assert_eq!(joined("last|"), None);
    }

    fn sorted(marked: &str) -> Option<String> {
        let (text, selection) = parse(marked);
        sort_lines(&text, selection).map(|edit| apply(&text, edit))
    }

    #[test]
    fn sort_lines_orders_the_selected_lines() {
        assert_eq!(
            sorted("top\n[pear\nApple\nbanana\n]end").as_deref(),
            Some("top\n[Apple\nbanana\npear]\nend")
        );
        assert_eq!(sorted("[a\nb]"), None);
        assert_eq!(sorted("on|ly"), None);
    }

    fn extended(marked: &str) -> Option<String> {
        let (text, selection) = parse(marked);
        extend_selection(&text, selection).map(|range| {
            let edit = Edit {
                range: 0..0,
                text: String::new(),
                selection: range,
            };
            apply(&text, edit)
        })
    }

    #[test]
    fn extend_selection_grows_step_by_step() {
        let steps = [
            "let x = foo(a, \"b| c\");",
            "let x = foo(a, \"[b] c\");",
            "let x = foo(a, \"[b c]\");",
            "let x = foo(a, [\"b c\"]);",
            "let x = foo([a, \"b c\"]);",
            "let x = foo[(a, \"b c\")];",
            "[let x = foo(a, \"b c\");]",
        ];
        for pair in steps.windows(2) {
            assert_eq!(
                extended(pair[0]).as_deref(),
                Some(pair[1]),
                "from {}",
                pair[0]
            );
        }
        assert_eq!(extended(steps[6]), None);
        // Then the line with its indent, and the paragraph up to the blank line.
        assert_eq!(
            extended("top\n  [mid]\nlow\n\nnext").as_deref(),
            Some("top\n[  mid]\nlow\n\nnext")
        );
        assert_eq!(
            extended("top\n[  mid]\nlow\n\nnext").as_deref(),
            Some("[top\n  mid\nlow]\n\nnext")
        );
    }
}
