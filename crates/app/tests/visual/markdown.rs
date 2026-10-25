//! Markdown editing helpers: lists, indents, bold and italic.

use std::{fs, path::PathBuf};

use gpui_kit::AnyWindowHandle;

use crate::check::Check;

/// Types `text` into the focused editor, one character at a time.
fn type_text(check: &mut Check, window: AnyWindowHandle, text: &str) {
    check.cx.simulate_input(window, text);
}

/// Presses `keys` one at a time and checks the active tab's text afterwards.
fn expect_text(check: &mut Check, window: AnyWindowHandle, keys: &str, what: &str, want: &str) {
    check.keys(window, keys);
    let got = check.workspace(window, |w, cx| w.active_text(cx));
    check.expect_eq(what, got.as_deref(), Some(want));
}

pub fn run(check: &mut Check) {
    let dir = tempfile::tempdir().expect("temp dir");
    fs::write(dir.path().join("list.md"), "").expect("write");
    fs::write(dir.path().join("main.rs"), "").expect("write");
    let window = check.open(Some(dir.path()));

    check.keys(window, "down down enter");
    let active = check.workspace(window, |w, cx| w.active_file(cx));
    check.expect_eq(
        "enter opens list.md",
        active,
        Some(PathBuf::from("list.md")),
    );

    type_text(check, window, "- item");
    expect_text(
        check,
        window,
        "enter",
        "markdown: enter after a list item starts the next",
        "- item\n- ",
    );
    expect_text(
        check,
        window,
        "cmd-z",
        "markdown: one undo takes the new item back",
        "- item",
    );
    expect_text(
        check,
        window,
        "enter enter",
        "markdown: enter on an empty item ends the list",
        "- item\n",
    );

    type_text(check, window, "- 한글");
    expect_text(
        check,
        window,
        "tab",
        "markdown: tab indents a list item",
        "- item\n  - 한글",
    );
    expect_text(
        check,
        window,
        "shift-tab",
        "markdown: shift-tab outdents it",
        "- item\n- 한글",
    );

    check.keys(window, "enter");
    type_text(check, window, "bold");
    expect_text(
        check,
        window,
        "alt-shift-left cmd-b",
        "markdown: cmd-b wraps the selected word",
        "- item\n- 한글\n- **bold**",
    );
    expect_text(
        check,
        window,
        "cmd-b",
        "markdown: cmd-b again unwraps it",
        "- item\n- 한글\n- bold",
    );
    expect_text(
        check,
        window,
        "cmd-i",
        "markdown: cmd-i makes it italic",
        "- item\n- 한글\n- *bold*",
    );
    check.shot(window, "markdown-lists");

    // Off list lines, Enter and Tab fall through to gpui-kit: its newline keeps the indent, its
    // tab indents at the cursor.
    check.keys(window, "cmd-right enter enter");
    type_text(check, window, "  plain");
    let base = "- item\n- 한글\n- *bold*\n  plain";
    expect_text(
        check,
        window,
        "enter",
        "markdown: enter on a plain line is gpui-kit's newline",
        &format!("{base}\n  "),
    );
    expect_text(
        check,
        window,
        "tab",
        "markdown: tab on a plain line is gpui-kit's tab",
        // Enter kept the line's 2 spaces; Tab adds the default tab size (4).
        &format!("{base}\n      "),
    );
    expect_text(
        check,
        window,
        "shift-tab",
        "markdown: shift-tab on a plain line is gpui-kit's outdent",
        &format!("{base}\n  "),
    );

    // A code file keeps gpui-kit's keys even on a line that looks like a list item.
    check.keys(window, "cmd-1 down enter");
    let active = check.workspace(window, |w, cx| w.active_file(cx));
    check.expect_eq(
        "enter opens main.rs",
        active,
        Some(PathBuf::from("main.rs")),
    );
    type_text(check, window, "- item");
    expect_text(
        check,
        window,
        "enter",
        "code: enter is a plain newline",
        "- item\n",
    );
    expect_text(
        check,
        window,
        "tab",
        "code: tab indents at the cursor",
        "- item\n    ",
    );
    type_text(check, window, "word");
    expect_text(
        check,
        window,
        "alt-shift-left cmd-b",
        "code: cmd-b does nothing",
        "- item\n    word",
    );

    edges(check);
    backspace_indent(check);
}

/// Backspace in a list item's indent takes the whole indent unit at once; elsewhere one character.
fn backspace_indent(check: &mut Check) {
    let dir = tempfile::tempdir().expect("temp dir");
    fs::write(dir.path().join("list.md"), "- top\n    - nested").expect("write");
    let window = check.open(Some(dir.path()));
    check.keys(window, "down down enter");
    expect_text(
        check,
        window,
        "down right right right right backspace",
        "markdown: backspace in the indent takes a whole unit",
        "- top\n- nested",
    );
    expect_text(
        check,
        window,
        "end backspace",
        "markdown: backspace after text takes one character",
        "- top\n- neste",
    );
}

/// Down on the last line goes to the end of the text and Up on the first to its start; elsewhere
/// they keep the column.
fn edges(check: &mut Check) {
    let dir = tempfile::tempdir().expect("temp dir");
    fs::write(dir.path().join("edge.md"), "first\nlast line").expect("write");
    let window = check.open(Some(dir.path()));
    check.keys(window, "down down enter down");
    type_text(check, window, "A");
    let got = check.workspace(window, |w, cx| w.active_text(cx));
    check.expect_eq(
        "down keeps the column",
        got.as_deref(),
        Some("first\nAlast line"),
    );
    check.keys(window, "down");
    type_text(check, window, "X");
    let got = check.workspace(window, |w, cx| w.active_text(cx));
    check.expect_eq(
        "down on the last line goes to its end",
        got.as_deref(),
        Some("first\nAlast lineX"),
    );
    check.keys(window, "up up");
    type_text(check, window, "Y");
    let got = check.workspace(window, |w, cx| w.active_text(cx));
    check.expect_eq(
        "up on the first line goes to its start",
        got.as_deref(),
        Some("Yfirst\nAlast lineX"),
    );
}
