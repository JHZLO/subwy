//! IntelliJ's line keys in a code tab: duplicate, delete, move and comment out lines, go to one,
//! select the next occurrence, change case, join and sort.

use std::fs;

use crate::{
    check::Check,
    markdown::{expect_text, type_text},
};

pub fn run(check: &mut Check) {
    let dir = tempfile::tempdir().expect("temp dir");
    fs::write(dir.path().join("f.rs"), "fn a() {}\nfn b() {}\nfn c() {}\n").expect("write");
    let window = check.open(Some(dir.path()));
    check.keys(window, "down down enter");
    expect_text(
        check,
        window,
        "cmd-d",
        "cmd-d duplicates the line",
        "fn a() {}\nfn a() {}\nfn b() {}\nfn c() {}\n",
    );
    expect_text(
        check,
        window,
        "cmd-backspace",
        "cmd-backspace deletes the line",
        "fn a() {}\nfn b() {}\nfn c() {}\n",
    );
    expect_text(
        check,
        window,
        "alt-shift-up",
        "alt-shift-up moves the line up",
        "fn b() {}\nfn a() {}\nfn c() {}\n",
    );
    expect_text(
        check,
        window,
        "alt-shift-down alt-shift-down",
        "alt-shift-down moves it down, the selection with it",
        "fn a() {}\nfn c() {}\nfn b() {}\n",
    );
    expect_text(
        check,
        window,
        "cmd-z",
        "one undo takes a move back",
        "fn a() {}\nfn b() {}\nfn c() {}\n",
    );
    expect_text(
        check,
        window,
        "cmd-up cmd-/ cmd-/",
        "cmd-/ comments out the line and goes down, so it repeats",
        "// fn a() {}\n// fn b() {}\nfn c() {}\n",
    );
    expect_text(
        check,
        window,
        "cmd-up cmd-/",
        "cmd-/ on a commented line takes the comment off",
        "fn a() {}\n// fn b() {}\nfn c() {}\n",
    );

    check.keys(window, "cmd-l");
    type_text(check, window, "3");
    check.shot(window, "go-to-line");
    check.keys(window, "enter");
    type_text(check, window, "X");
    let got = check.workspace(window, |w, cx| w.active_text(cx));
    check.expect_eq(
        "cmd-l :3 puts the caret at the start of line 3",
        got.as_deref(),
        Some("fn a() {}\n// fn b() {}\nXfn c() {}\n"),
    );
    check.keys(window, "cmd-l");
    type_text(check, window, "2:4");
    check.keys(window, "enter");
    type_text(check, window, "Y");
    let got = check.workspace(window, |w, cx| w.active_text(cx));
    check.expect_eq(
        "cmd-l :2:4 puts it at line 2, column 4",
        got.as_deref(),
        Some("fn a() {}\n// Yfn b() {}\nXfn c() {}\n"),
    );

    next_occurrence(check);
    case_join_sort(check);
    extends_selection(check);
}

/// Alt+Up grows the selection from the caret's word to the quotes around it; Alt+Down goes back.
fn extends_selection(check: &mut Check) {
    let dir = tempfile::tempdir().expect("temp dir");
    fs::write(dir.path().join("e.rs"), "let x = foo(a, \"b c\");\n").expect("write");
    let window = check.open(Some(dir.path()));
    check.keys(window, "down down enter");
    check.keys(window, &["right"; 17].join(" "));
    check.keys(window, "alt-up alt-up");
    type_text(check, window, "X");
    let text = check.workspace(window, |w, cx| w.active_text(cx));
    check.expect_eq(
        "alt-up twice selects inside the quotes",
        text.as_deref(),
        Some("let x = foo(a, \"X\");\n"),
    );
    check.keys(window, "cmd-z cmd-left");
    check.keys(window, &["right"; 17].join(" "));
    check.keys(window, "alt-up alt-up alt-down");
    type_text(check, window, "Y");
    let text = check.workspace(window, |w, cx| w.active_text(cx));
    check.expect_eq(
        "alt-down goes back to the word",
        text.as_deref(),
        Some("let x = foo(a, \"Y c\");\n"),
    );
}

/// Cmd+Shift+U raises the caret's word, Ctrl+Shift+J joins the next line, F9 sorts the lines.
fn case_join_sort(check: &mut Check) {
    let dir = tempfile::tempdir().expect("temp dir");
    fs::write(dir.path().join("h.rs"), "pear\n  apple\nfig\n").expect("write");
    let window = check.open(Some(dir.path()));
    check.keys(window, "down down enter");
    expect_text(
        check,
        window,
        "cmd-shift-u",
        "cmd-shift-u raises the word at the caret",
        "PEAR\n  apple\nfig\n",
    );
    expect_text(
        check,
        window,
        "ctrl-shift-j",
        "ctrl-shift-j joins the next line without its indent",
        "PEAR apple\nfig\n",
    );
    expect_text(
        check,
        window,
        "cmd-a f9",
        "f9 sorts the selected lines",
        "fig\nPEAR apple\n",
    );
}

/// Ctrl+G takes the caret's word, then the next place it is as a whole word, so typing changes both.
fn next_occurrence(check: &mut Check) {
    let dir = tempfile::tempdir().expect("temp dir");
    fs::write(dir.path().join("g.rs"), "let foo = foo + food;\n").expect("write");
    let window = check.open(Some(dir.path()));
    check.keys(
        window,
        "down down enter right right right right ctrl-g ctrl-g",
    );
    type_text(check, window, "bar");
    let got = check.workspace(window, |w, cx| w.active_text(cx));
    check.expect_eq(
        "ctrl-g selects the word and its next whole-word match",
        got.as_deref(),
        Some("let bar = bar + food;\n"),
    );
}
