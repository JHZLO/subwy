//! IntelliJ's line keys in a code tab: duplicate, delete, move and comment out lines.

use std::fs;

use crate::{check::Check, markdown::expect_text};

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
}
