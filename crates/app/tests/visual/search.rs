//! Find in the folder (Cmd+Shift+F).

use std::{path::PathBuf, time::Duration};

use gpui_kit::{
    component::{Theme, ThemeMode},
    *,
};

use crate::check::{Check, fixture};

/// The center of the panel's list row `row` at 100% zoom, in a 1280-wide window: below the title
/// bar (38), the gap (8), the border (1), the line's band (3), the header (34), the query between
/// its lines (1 + 34 + 1) and the list's padding (4).
fn row_center(row: usize) -> Point<Pixels> {
    point(
        px(640.),
        px(38. + 8. + 1. + 3. + 34. + 1. + 34. + 1. + 4. + 24. * row as f32 + 12.),
    )
}

/// Opens the search from the tree, types `query` a key at a time and waits out the pause before
/// it searches. From the tree: in the editor, gpui-kit binds Cmd+Shift+F to its own replace bar.
fn find(check: &mut Check, window: AnyWindowHandle, query: &str) {
    check.keys(window, "cmd-shift-f");
    let keys: Vec<String> = query
        .chars()
        .map(|c| match c {
            ' ' => "space".to_string(),
            c if c.is_uppercase() => format!("shift-{}", c.to_lowercase()),
            c => c.to_string(),
        })
        .collect();
    check.keys(window, &keys.join(" "));
    check.cx.advance_clock(Duration::from_millis(200));
    check.cx.run_until_parked();
}

/// Checks the active file and its cursor line as Cmd+Alt+Shift+C copies them (`path:line`,
/// one-based), then gives the tree the keys for the next search.
fn expect_reference(check: &mut Check, window: AnyWindowHandle, what: &str, want: &str) {
    check.keys(window, "cmd-alt-shift-c");
    let copied = check.clipboard();
    check.expect_eq(what, copied.as_deref(), Some(want));
    check.keys(window, "cmd-1");
}

/// Alt+F7 lists the lines that link to the active file; Enter opens one at its line.
fn backlinks(check: &mut Check) {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(dir.path().join("a.md"), "intro\nsee [b](b.md)\n").expect("write");
    std::fs::write(dir.path().join("b.md"), "# B\n").expect("write");
    let window = check.open(Some(dir.path()));
    check.keys(window, "down down down enter alt-f7");
    check.wait_until(|check| {
        check
            .workspace(window, |w, cx| w.search_summary(cx))
            .is_some()
    });
    let summary = check.workspace(window, |w, cx| w.search_summary(cx));
    check.expect_eq(
        "alt-f7 finds the link to the file",
        summary.as_deref(),
        Some("1 match in 1 file"),
    );
    check.keys(window, "enter");
    expect_reference(check, window, "enter opens the linking line", "a.md:2");
}

pub fn run(check: &mut Check) {
    backlinks(check);
    let fixture = fixture();
    let window = check.open(Some(fixture.path()));

    // "leaf" is on four lines of three files (gitignored target/ left out): paths.md line 3,
    // README.md line 1, main.rs lines 3 and 6. Enter opens the first, the cursor on its line.
    find(check, window, "leaf");
    check.shot(window, "search-results");
    // Under the list, the selected match's file with the match selected.
    let previewed = |check: &mut Check| check.workspace(window, |w, cx| w.search_preview(cx));
    let first = Some(("docs/guide/paths.md".to_string(), "leaf".to_string()));
    let shown = check.wait_until(|check| previewed(check) == first);
    check.expect_eq("the preview shows the first match", shown, true);
    check.keys(window, "down down");
    let later = Some(("src/main.rs".to_string(), "leaf".to_string()));
    let shown = check.wait_until(|check| previewed(check) == later);
    check.shot(window, "search-preview");
    check.expect_eq("the preview follows the selection", shown, true);
    check.keys(window, "up up enter");
    let active = check.workspace(window, |w, cx| w.active_file(cx));
    check.expect_eq(
        "enter opens the first match's file",
        active,
        Some(PathBuf::from("docs/guide/paths.md")),
    );
    expect_reference(
        check,
        window,
        "the cursor is on the first match's line",
        "docs/guide/paths.md:3",
    );

    // Down steps to the next match and stops at the last; Up goes back.
    find(check, window, "leaf");
    check.keys(window, "down down down down enter");
    expect_reference(
        check,
        window,
        "down stops at the last match",
        "src/main.rs:6",
    );
    find(check, window, "leaf");
    check.keys(window, "down down down up enter");
    expect_reference(check, window, "up goes back a line", "src/main.rs:3");

    // Smart case: lower case finds "paths" first, an upper-case letter only "Path".
    find(check, window, "path");
    check.keys(window, "enter");
    expect_reference(
        check,
        window,
        "a lower-case query ignores case",
        "docs/guide/paths.md:1",
    );
    find(check, window, "Path");
    check.keys(window, "down enter");
    expect_reference(
        check,
        window,
        "an upper-case letter makes case matter",
        "src/main.rs:5",
    );

    // A file already open moves its cursor to the line.
    find(check, window, "absolute");
    check.keys(window, "enter");
    expect_reference(
        check,
        window,
        "an open file moves its cursor to the match",
        "docs/guide/paths.md:8",
    );

    // A click on a line opens it: README.md's, the second.
    find(check, window, "leaf");
    check
        .cx
        .simulate_click(window, row_center(1), Modifiers::default());
    expect_reference(
        check,
        window,
        "a click opens the clicked line",
        "README.md:1",
    );

    // No match: Enter opens nothing; Escape closes and gives the keys back to the tree.
    find(check, window, "zzz");
    check.shot(window, "search-no-matches");
    check.keys(window, "enter");
    let active = check.workspace(window, |w, cx| w.active_file(cx));
    check.expect_eq(
        "enter with no match opens nothing",
        active,
        Some(PathBuf::from("README.md")),
    );
    check.keys(window, "escape up");
    let selected = check.workspace(window, |w, cx| w.tree_selection(cx));
    check.expect_eq(
        "escape gives focus back to the tree",
        selected,
        Some(PathBuf::from(".gitignore")),
    );

    // Cmd+Shift+F closes it again and the tree keeps walking.
    find(check, window, "leaf");
    check.keys(window, "cmd-shift-f down");
    let selected = check.workspace(window, |w, cx| w.tree_selection(cx));
    check.expect_eq(
        "cmd-shift-f gives focus back to the tree",
        selected,
        Some(PathBuf::from("README.md")),
    );

    long_list(check);
    in_folder(check);

    // On light paper and zoomed: every size scales.
    let was = check.cx.update(|cx| Theme::global(cx).mode);
    check
        .cx
        .update(|cx| Theme::change(ThemeMode::Light, None, cx));
    check.keys(window, "cmd-=");
    find(check, window, "the");
    check.shot(window, "search-light-zoomed");
    check.keys(window, "escape cmd-0");
    check.cx.update(|cx| Theme::change(was, None, cx));
}

/// Cmd+Shift+F on a folder in the tree searches that folder only (IntelliJ's Directory scope).
fn in_folder(check: &mut Check) {
    let fixture = fixture();
    let window = check.open(Some(fixture.path()));
    // Rows: the folder, docs. "leaf" is in docs/guide/paths.md, README.md and src/main.rs.
    check.keys(window, "down down");
    find(check, window, "leaf");
    check.shot(window, "search-in-folder");
    check.keys(window, "down down down enter");
    expect_reference(
        check,
        window,
        "a folder's search finds only what it holds",
        "docs/guide/paths.md:3",
    );
}

/// More matches than fit: the list scrolls to keep the selected line in view.
fn long_list(check: &mut Check) {
    let dir = tempfile::tempdir().expect("temp dir");
    let text: String = (1..=40).map(|i| format!("item {i}\n")).collect();
    std::fs::write(dir.path().join("many.md"), text).expect("write");
    let window = check.open(Some(dir.path()));
    find(check, window, "item");
    check.keys(window, &["down"; 20].join(" "));
    check.shot(window, "search-scrolled");
    check.keys(window, "enter");
    expect_reference(
        check,
        window,
        "down walks past the shown rows",
        "many.md:21",
    );
}
