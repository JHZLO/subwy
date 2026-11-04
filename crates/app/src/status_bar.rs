//! The status bar. Its left end is the stem: the path you would copy, and the place copying is confirmed.

use std::time::{Duration, Instant};

use gpui_kit::{prelude::FluentBuilder as _, *};

use crate::theme::{self, c, size, z};

/// How long the copied state stays before it fades back.
pub const COPIED_FOR: Duration = Duration::from_millis(1500);
/// The fade back: the only motion in subwy (design doc, interaction rules).
pub const FADE: Duration = Duration::from_millis(200);

/// What was just copied, shown on the stem until [`COPIED_FOR`] has passed.
#[derive(Clone, Debug)]
pub struct Copied {
    pub text: String,
    /// The format, as shown after the stem (`Copied relative path and line`).
    pub label: &'static str,
    pub at: Instant,
    /// Restarts the fade when something is copied again.
    pub seq: u64,
}

/// How the status bar names a language; the editor knows it by grammar name or file extension.
pub fn language_label(language: &str) -> String {
    let label = match language {
        "markdown" | "md" => "Markdown",
        "rust" | "rs" => "Rust",
        "kotlin" | "kt" | "kts" => "Kotlin",
        "java" => "Java",
        "typescript" | "ts" => "TypeScript",
        "tsx" => "TSX",
        "javascript" | "js" | "jsx" => "JavaScript",
        "json" => "JSON",
        "yaml" | "yml" => "YAML",
        "toml" => "TOML",
        "sql" => "SQL",
        "bash" | "sh" | "zsh" => "Shell",
        "python" | "py" => "Python",
        "go" => "Go",
        "html" | "htm" => "HTML",
        "css" | "scss" => "CSS",
        "text" | "txt" | "" => "Plain Text",
        other => return other.to_uppercase(),
    };
    label.to_string()
}

/// A Markdown tab's words and characters, of the selection when there is one.
pub fn counts_label(words: usize, chars: usize, selected: bool) -> String {
    let plural = |n: usize| if n == 1 { "" } else { "s" };
    let mut label = format!(
        "{words} word{}, {chars} char{}",
        plural(words),
        plural(chars)
    );
    if selected {
        label.push_str(" selected");
    }
    label
}

pub struct Status<'a> {
    /// The stem: the relative path (and cursor line) of what has focus.
    pub stem: Option<&'a str>,
    pub copied: Option<&'a Copied>,
    /// Words and characters, in a Markdown tab.
    pub counts: Option<String>,
    pub language: Option<&'a str>,
    pub line_ending: Option<&'a str>,
    pub zoom: u32,
}

pub fn render(status: Status<'_>, cx: &App) -> impl IntoElement {
    let p = theme::colors(cx);
    let elapsed = status.copied.map(|copied| copied.at.elapsed());
    let holding = status
        .copied
        .filter(|_| elapsed.is_some_and(|e| e < COPIED_FOR));
    let fading = status
        .copied
        .filter(|_| elapsed.is_some_and(|e| e >= COPIED_FOR && e < COPIED_FOR + FADE));
    // While holding, the stem shows exactly what was copied, one line after another; the fade
    // returns to the plain stem.
    let stem_text = holding
        .map(|copied| copied.text.replace('\n', " ↵ "))
        .or_else(|| status.stem.map(str::to_string))
        .unwrap_or_default();
    let base = div()
        .px_2()
        .rounded(px(4.))
        .font_family(theme::MONO_FONT)
        .text_size(z(11.5, cx))
        .text_color(c(if holding.is_some() { p.sel_ink } else { p.ink }))
        .when(holding.is_some(), |el| el.bg(c(p.sel_bg)))
        .child(stem_text);
    let sel_bg = c(p.sel_bg);
    let stem = match fading {
        Some(copied) => base
            .with_animation(
                ElementId::NamedInteger("stem-fade".into(), copied.seq),
                Animation::new(FADE),
                move |el, delta| el.bg(sel_bg.opacity(1. - delta)),
            )
            .into_any_element(),
        None => base.into_any_element(),
    };
    let label = holding.or(fading).map(|copied| {
        let label = div().text_color(c(p.accent_text)).child(copied.label);
        match fading {
            Some(_) => label
                .with_animation(
                    ElementId::NamedInteger("label-fade".into(), copied.seq),
                    Animation::new(FADE),
                    |el, delta| el.opacity(1. - delta),
                )
                .into_any_element(),
            None => label.into_any_element(),
        }
    });
    div()
        .h(z(size::STATUS_BAR, cx))
        .flex_none()
        .flex()
        .items_center()
        .justify_between()
        .pl_2()
        .pr_3()
        // On the desk under the islands, like the title bar.
        .bg(c(p.desk))
        .text_size(z(12., cx))
        .text_color(c(p.sub))
        .child(
            div()
                .flex()
                .items_center()
                .gap_2p5()
                .child(stem)
                .children(label),
        )
        .child(
            div()
                .flex()
                .gap_4()
                .children(status.counts)
                .children(status.language.map(str::to_string))
                .children(status.line_ending.map(str::to_string))
                .children(status.language.map(|_| "UTF-8"))
                .child(format!("{}%", status.zoom)),
        )
}
