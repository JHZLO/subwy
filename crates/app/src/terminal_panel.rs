//! The terminal under the editor (IntelliJ's Terminal, Alt+F12): the user's shell in the open
//! folder, painted cell by cell on subwy's paper. The shell and its screen live in
//! `subwy_project::terminal`; this view paints the screen and turns keys, text and the mouse into
//! what the shell reads.

use std::{cell::Cell, ops::Range, path::Path, rc::Rc};

use gpui_kit::*;
use subwy_project::terminal::{
    self, CursorShape, Event, EventListener, Frame, GridSize, Mods, Palette, Rgb, SelectionType,
    Shell, Side,
};

use crate::{
    keymap::{CopySelection, Paste, SendBackTab, SendTab, TERMINAL},
    theme::{self, c, size, z},
};

/// Carries the shell's events from its reader thread to the panel.
#[derive(Clone)]
struct Events(smol::channel::Sender<Event>);

impl EventListener for Events {
    /// Called with the terminal locked: it only queues.
    fn send_event(&self, event: Event) {
        let _ = self.0.try_send(event);
    }
}

/// A shell that has started, before its panel exists (starting can fail; making a view cannot).
pub struct Started {
    shell: Shell<Events>,
    events: smol::channel::Receiver<Event>,
}

pub enum TerminalEvent {
    /// The shell exited; the panel has nothing left to show.
    Exited,
    /// The panel took the keyboard.
    Focused,
}

/// Where the grid sat in the last paint, for the mouse and the input method.
#[derive(Clone, Copy, Default)]
struct Metrics {
    origin: Point<Pixels>,
    cell_width: Pixels,
    line_height: Pixels,
    cursor: Option<(usize, usize)>,
}

pub struct TerminalPanel {
    shell: Shell<Events>,
    focus_handle: FocusHandle,
    metrics: Rc<Cell<Metrics>>,
    /// Text the input method is composing (Hangul before the syllable is done). Drawn at the
    /// cursor, never sent: the shell gets the finished text.
    marked: Option<String>,
    /// Scroll distance not yet a whole line.
    scroll_rest: f32,
    _events: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<TerminalEvent> for TerminalPanel {}

impl Focusable for TerminalPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

/// The terminal's colors for the current appearance.
fn palette(cx: &App) -> Palette {
    let p = theme::colors(cx);
    Palette::new(p.ansi, p.ink, p.paper, p.accent)
}

fn color(value: Rgb) -> Hsla {
    rgb(((value.r as u32) << 16) | ((value.g as u32) << 8) | value.b as u32).into()
}

impl TerminalPanel {
    /// Starts `shell` in `cwd`, or the login shell. `SUBWY_SHELL` names another program, for the UI
    /// checks. The panel sizes the grid when it first paints.
    pub fn start(cwd: &Path, shell: Option<&str>) -> std::io::Result<Started> {
        let (tx, events) = smol::channel::unbounded();
        let program = std::env::var("SUBWY_SHELL")
            .ok()
            .or_else(|| shell.map(str::to_string));
        let size = GridSize {
            cols: 80,
            rows: 24,
            cell_width: 8,
            cell_height: 16,
        };
        let shell = Shell::spawn(
            cwd,
            program.as_deref().map(|p| (p, &[][..])),
            size,
            Events(tx),
        )?;
        Ok(Started { shell, events })
    }

    pub fn new(started: Started, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let Started { shell, events: rx } = started;
        // The first event redraws at once; whatever else queued up meanwhile goes with it, so a
        // flood of output redraws once a frame rather than once a read.
        let events = cx.spawn(async move |this, cx| {
            while let Ok(first) = rx.recv().await {
                let mut batch = vec![first];
                while let Ok(more) = rx.try_recv() {
                    batch.push(more);
                }
                let alive = this.update(cx, |this, cx| {
                    for event in batch {
                        this.handle(event, cx);
                    }
                    cx.notify();
                });
                if alive.is_err() {
                    break;
                }
            }
        });
        let focus_handle = cx.focus_handle();
        let subscriptions = vec![
            cx.on_focus_in(&focus_handle, window, |this, _, cx| {
                this.focus_changed(true, cx)
            }),
            cx.on_blur(&focus_handle, window, |this, _, cx| {
                this.focus_changed(false, cx)
            }),
        ];
        Self {
            shell,
            focus_handle,
            metrics: Rc::new(Cell::new(Metrics::default())),
            marked: None,
            scroll_rest: 0.,
            _events: events,
            _subscriptions: subscriptions,
        }
    }

    /// The folder the shell is in now, for the session.
    pub fn cwd(&self) -> Option<std::path::PathBuf> {
        self.shell.cwd()
    }

    /// The text on screen, one string per line, for the UI checks.
    pub fn visible_lines(&self, cx: &App) -> Vec<String> {
        self.shell
            .frame(&palette(cx))
            .lines
            .iter()
            .map(|line| {
                let mut text = String::new();
                for span in line {
                    let pad = span.col.saturating_sub(text.chars().count());
                    text.extend(std::iter::repeat_n(' ', pad));
                    text.push_str(&span.text);
                }
                text
            })
            .collect()
    }

    fn handle(&mut self, event: Event, cx: &mut Context<Self>) {
        let Some(event) = self.shell.answer(event, &palette(cx)) else {
            return;
        };
        match event {
            Event::ClipboardStore(_, text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text))
            }
            Event::ClipboardLoad(_, format) => {
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    self.shell.write(format(&text).into_bytes());
                }
            }
            Event::ChildExit(_) | Event::Exit => cx.emit(TerminalEvent::Exited),
            _ => {}
        }
    }

    fn focus_changed(&mut self, focused: bool, cx: &mut Context<Self>) {
        if self.shell.set_focused(focused) {
            self.shell.write(if focused {
                &b"\x1b[I"[..]
            } else {
                &b"\x1b[O"[..]
            });
        }
        if focused {
            cx.emit(TerminalEvent::Focused);
        }
        cx.notify();
    }

    /// Keys that are not text: Enter, arrows, Ctrl combinations. Text (and Hangul) comes in
    /// through the input handler below; Cmd keys are subwy's.
    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        if keystroke.modifiers.platform {
            return;
        }
        let mods = Mods {
            shift: keystroke.modifiers.shift,
            alt: keystroke.modifiers.alt,
            ctrl: keystroke.modifiers.control,
        };
        if let Some(bytes) = terminal::encode_key(&keystroke.key, mods, self.shell.mode()) {
            self.shell.input(bytes);
            cx.stop_propagation();
            cx.notify();
        }
    }

    fn copy(&mut self, _: &CopySelection, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.shell.selection_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.paste_text(&text, cx);
        }
    }

    /// Hands `text` to the shell as a paste (bracketed when the shell asks for it).
    pub fn paste_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.shell
            .input(terminal::paste_bytes(text, self.shell.mode()));
        cx.notify();
    }

    /// The screen cell under `position`, and which half of it.
    fn cell_at(&self, position: Point<Pixels>) -> (usize, usize, Side) {
        let m = self.metrics.get();
        let x = (position.x - m.origin.x).as_f32().max(0.);
        let y = (position.y - m.origin.y).as_f32().max(0.);
        let width = m.cell_width.as_f32().max(1.);
        let height = m.line_height.as_f32().max(1.);
        let side = if x % width > width / 2. {
            Side::Right
        } else {
            Side::Left
        };
        ((y / height) as usize, (x / width) as usize, side)
    }

    /// A click starts a selection: one click by character, two by word, three by line.
    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_handle.focus(window, cx);
        let (line, col, side) = self.cell_at(event.position);
        let kind = match event.click_count {
            1 => SelectionType::Simple,
            2 => SelectionType::Semantic,
            _ => SelectionType::Lines,
        };
        self.shell.select(kind, line, col, side);
        cx.notify();
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if event.pressed_button != Some(MouseButton::Left) {
            return;
        }
        let (line, col, side) = self.cell_at(event.position);
        self.shell.extend_selection(line, col, side);
        cx.notify();
    }

    /// The wheel scrolls the history; in a full-screen program (less, vim) it sends arrow keys,
    /// as other terminals do.
    fn scroll(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let height = self.metrics.get().line_height;
        if height <= px(0.) {
            return;
        }
        self.scroll_rest += event.delta.pixel_delta(height).y.as_f32();
        let lines = (self.scroll_rest / height.as_f32()) as i32;
        if lines == 0 {
            return;
        }
        self.scroll_rest -= lines as f32 * height.as_f32();
        let mode = self.shell.mode();
        if mode.contains(terminal::TermMode::ALT_SCREEN | terminal::TermMode::ALTERNATE_SCROLL) {
            let key: &[u8] = if lines > 0 { b"\x1bOA" } else { b"\x1bOB" };
            self.shell.write(key.repeat(lines.unsigned_abs() as usize));
        } else {
            self.shell.scroll(lines);
        }
        cx.notify();
    }
}

/// The text and cursor of one frame, measured, ready to paint.
struct Prepared {
    frame: Frame,
    font: Font,
    font_size: Pixels,
    cell_width: Pixels,
    line_height: Pixels,
}

impl Render for TerminalPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = theme::colors(cx);
        let entity = cx.entity();
        let focused = self.focus_handle.is_focused(window);
        let marked = self.marked.clone();
        let metrics = self.metrics.clone();
        let focus_handle = self.focus_handle.clone();
        div()
            .key_context(TERMINAL)
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::key_down))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(|this, _: &SendTab, _, cx| {
                this.shell.input(&b"\t"[..]);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &SendBackTab, _, cx| {
                this.shell.input(&b"\x1b[Z"[..]);
                cx.notify();
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_scroll_wheel(cx.listener(Self::scroll))
            .size_full()
            .bg(c(p.paper))
            .px(z(10., cx))
            .py(z(6., cx))
            .child(
                canvas(
                    {
                        let entity = entity.clone();
                        move |bounds, window, cx| {
                            let font_size = px(theme::terminal_text(cx) * theme::zoom(cx));
                            let font = font(theme::editor_font(cx));
                            let text = window.text_system();
                            let id = text.resolve_font(&font);
                            let cell_width = text
                                .advance(id, font_size, 'm')
                                .map(|a| a.width)
                                .unwrap_or(font_size * 0.6);
                            let line_height = (font_size * size::TERMINAL_LINE).round();
                            let cols = (bounds.size.width / cell_width).floor().max(2.) as u16;
                            let rows = (bounds.size.height / line_height).floor().max(1.) as u16;
                            let panel = entity.read(cx);
                            panel.shell.resize(GridSize {
                                cols,
                                rows,
                                cell_width: cell_width.as_f32() as u16,
                                cell_height: line_height.as_f32() as u16,
                            });
                            Prepared {
                                frame: panel.shell.frame(&palette(cx)),
                                font,
                                font_size,
                                cell_width,
                                line_height,
                            }
                        }
                    },
                    move |bounds, prepared, window, cx| {
                        paint(bounds, &prepared, focused, marked.as_deref(), window, cx);
                        let cursor = prepared.frame.cursor.as_ref().map(|c| (c.line, c.col));
                        metrics.set(Metrics {
                            origin: bounds.origin,
                            cell_width: prepared.cell_width,
                            line_height: prepared.line_height,
                            cursor,
                        });
                        window.handle_input(
                            &focus_handle,
                            ElementInputHandler::new(bounds, entity),
                            cx,
                        );
                    },
                )
                .size_full(),
            )
    }
}

/// Paints the spans line by line, each run of characters pinned to the cell grid, then the
/// cursor and any text the input method is composing.
fn paint(
    bounds: Bounds<Pixels>,
    prepared: &Prepared,
    focused: bool,
    marked: Option<&str>,
    window: &mut Window,
    cx: &mut App,
) {
    let p = theme::colors(cx);
    let Prepared {
        frame,
        font,
        font_size,
        cell_width,
        line_height,
    } = prepared;
    let (cell_width, line_height, font_size) = (*cell_width, *line_height, *font_size);
    let at = |line: usize, col: usize| {
        point(
            bounds.origin.x + cell_width * col as f32,
            bounds.origin.y + line_height * line as f32,
        )
    };
    let shape = |text: &str, look: Font, ink: Hsla, wide: bool, window: &mut Window| {
        let run = TextRun {
            len: text.len(),
            font: look,
            color: ink,
            ..Default::default()
        };
        // Narrow characters each take one cell; a wide one is shaped as it is.
        let force = (!wide).then_some(cell_width);
        window
            .text_system()
            .shape_line(text.to_string().into(), font_size, &[run], force)
    };
    for (index, line) in frame.lines.iter().enumerate() {
        for span in line {
            let origin = at(index, span.col);
            let width = cell_width * span.cells as f32;
            let background = if span.selected {
                Some(c(p.sel_bg))
            } else {
                span.bg.map(color)
            };
            if let Some(background) = background {
                window.paint_quad(fill(
                    Bounds::new(origin, size(width, line_height)),
                    background,
                ));
            }
            if span.text.trim().is_empty() && !span.underline && !span.strike {
                continue;
            }
            let ink = if span.selected {
                c(p.sel_ink)
            } else {
                color(span.fg)
            };
            let mut look = font.clone();
            if span.bold {
                look = look.bold();
            }
            if span.italic {
                look = look.italic();
            }
            let mut run = TextRun {
                len: span.text.len(),
                font: look,
                color: ink,
                ..Default::default()
            };
            if span.underline {
                run.underline = Some(UnderlineStyle {
                    thickness: px(1.),
                    color: Some(ink),
                    wavy: false,
                });
            }
            if span.strike {
                run.strikethrough = Some(StrikethroughStyle {
                    thickness: px(1.),
                    color: Some(ink),
                });
            }
            let force = (!span.wide).then_some(cell_width);
            let shaped =
                window
                    .text_system()
                    .shape_line(span.text.clone().into(), font_size, &[run], force);
            let _ = shaped.paint(origin, line_height, TextAlign::Left, None, window, cx);
        }
    }
    let Some(cursor) = &frame.cursor else {
        return;
    };
    let origin = at(cursor.line, cursor.col);
    let cells = if cursor.wide { 2. } else { 1. };
    let accent = c(p.accent);
    if let Some(marked) = marked.filter(|m| !m.is_empty()) {
        // The composing text covers what is under it and carries an underline, as macOS draws it.
        let run = TextRun {
            len: marked.len(),
            font: font.clone(),
            color: c(p.ink),
            underline: Some(UnderlineStyle {
                thickness: px(1.),
                color: Some(accent),
                wavy: false,
            }),
            ..Default::default()
        };
        let shaped =
            window
                .text_system()
                .shape_line(marked.to_string().into(), font_size, &[run], None);
        window.paint_quad(fill(
            Bounds::new(origin, size(shaped.width, line_height)),
            c(p.paper),
        ));
        let _ = shaped.paint(origin, line_height, TextAlign::Left, None, window, cx);
        return;
    }
    let cell = Bounds::new(origin, size(cell_width * cells, line_height));
    match (cursor.shape, focused) {
        (CursorShape::Beam, _) => {
            window.paint_quad(fill(Bounds::new(origin, size(px(2.), line_height)), accent));
        }
        (CursorShape::Underline, _) => {
            window.paint_quad(fill(
                Bounds::new(
                    point(origin.x, origin.y + line_height - px(2.)),
                    size(cell_width * cells, px(2.)),
                ),
                accent,
            ));
        }
        (CursorShape::Block, true) => {
            window.paint_quad(fill(cell, accent));
            if !cursor.text.trim().is_empty() {
                let shaped = shape(&cursor.text, font.clone(), c(p.paper), cursor.wide, window);
                let _ = shaped.paint(origin, line_height, TextAlign::Left, None, window, cx);
            }
        }
        // Without focus, and when a program asks for it, the block is an outline.
        _ => window.paint_quad(outline(cell, accent, BorderStyle::default())),
    }
}

/// The input method's view of the terminal: nothing is editable, so the only text is what is
/// being composed, and it sits at the cursor.
impl EntityInputHandler for TerminalPanel {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let units: Vec<u16> = self
            .marked
            .as_deref()
            .unwrap_or("")
            .encode_utf16()
            .collect();
        let end = range.end.min(units.len());
        let start = range.start.min(end);
        *adjusted = Some(start..end);
        Some(String::from_utf16_lossy(&units[start..end]))
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let len = self
            .marked
            .as_deref()
            .map_or(0, |m| m.encode_utf16().count());
        Some(UTF16Selection {
            range: len..len,
            reversed: false,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked
            .as_deref()
            .map(|marked| 0..marked.encode_utf16().count())
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.marked = None;
        cx.notify();
    }

    /// Finished text: typed characters, a composed syllable, an Option character, dictation.
    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked = None;
        if !text.is_empty() {
            self.shell.input(text.as_bytes().to_vec());
        }
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked = (!text.is_empty()).then(|| text.to_string());
        cx.notify();
    }

    /// Where the input method puts its candidate window: at the cursor.
    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let m = self.metrics.get();
        let (line, col) = m.cursor?;
        Some(Bounds::new(
            point(
                m.origin.x + m.cell_width * col as f32,
                m.origin.y + m.line_height * line as f32,
            ),
            size(m.cell_width, m.line_height),
        ))
    }

    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}
