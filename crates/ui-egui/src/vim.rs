//! Opt-in vim-style modal keys for the worksheet grid (`view.vimMode`).
//!
//! [`Vim`] is a pure state machine: keys in, [`Action`]s out. [`process`] feeds it the frame's
//! keys before the grid sees them and runs the actions through existing commands. Insert mode is
//! the cell editor; with vim mode on, Escape in the editor commits instead of cancelling.

use egui::{Context, Event, Key, os::OperatingSystem};
use gridcraft_engine::Clipboard;
use gridcraft_engine::core::{CellRef, MAX_COLS, MAX_ROWS, RangeRef};
use serde_json::{Value as Json, json};

use crate::SheetApp;

const MAX_COUNT: u32 = 99_999;

/// What a visual selection covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Visual {
    Cells,
    Rows,
    Cols,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Normal,
    Visual(Visual),
}

/// A key as the state machine sees it: typed text, or one of the few named keys it owns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Input {
    Char(char),
    Esc,
    Enter,
    Backspace,
    Ctrl(char),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    Left,
    Right,
    Up,
    Down,
    /// Next / previous filled cell in the row (Ctrl+Right / Ctrl+Left).
    NextFilled,
    PrevFilled,
    /// The same down / up the column (Ctrl+Down / Ctrl+Up).
    BlockDown,
    BlockUp,
    RowStart,
    RowEnd,
    Top,
    Bottom,
    HalfDown,
    HalfUp,
    ScreenTop,
    ScreenMiddle,
    ScreenBottom,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edit {
    /// Edit the cell with the caret at the start.
    Insert,
    /// Edit the cell with the caret at the end.
    Append,
    /// Replace the cell's content.
    Substitute,
    OpenBelow,
    OpenAbove,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Yank,
    Delete,
    Change,
    Indent,
    Outdent,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// `count` is `None` when no count was typed (`gg` and `G` treat a count as a row number).
    Move(Motion, Option<u32>),
    /// `text` is the committed text when `.` repeats an edit; `None` opens the editor.
    Edit(Edit, Option<String>),
    /// `x`: yank and clear this many cells from the active one rightwards.
    Clear(u32),
    DeleteRows(u32),
    YankRows(u32),
    Put {
        before: bool,
        count: u32,
    },
    Indent(i32),
    Undo(u32),
    Redo(u32),
    /// The new visual state; `None` leaves visual mode.
    Visual(Option<Visual>),
    VisualOp(Op),
    /// `o` in visual mode: the cursor moves to the other end.
    VisualSwap,
    SheetNext(u32),
    SheetPrevious(u32),
    Search(String),
    SearchNext {
        forward: bool,
        count: u32,
    },
    Escape,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pending {
    G,
    Delete,
    Yank,
    Change,
    Indent,
    Outdent,
}

#[derive(Clone, Debug, Default)]
struct Change {
    inputs: Vec<Input>,
    /// What the editor committed, for changes that entered insert mode.
    text: Option<String>,
}

#[derive(Default)]
pub struct Vim {
    mode: Mode,
    count: Option<u32>,
    pending: Option<Pending>,
    search: Option<String>,
    /// Inputs of the command being typed, for `.`.
    record: Vec<Input>,
    last_change: Option<Change>,
    replaying: Option<Option<String>>,
    awaiting_insert: bool,
    /// Executor state. The machine's mode runs ahead of the actions it returns, so the executor
    /// keeps the shape of the selection it is drawing.
    anchor: Option<CellRef>,
    shape: Option<Visual>,
    register: Option<Register>,
    insert_cell: Option<(usize, CellRef)>,
}

#[derive(Clone, Debug)]
struct Register {
    clip: Clipboard,
    kind: Visual,
}

impl Vim {
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// The `/` prompt while it is open.
    pub fn search(&self) -> Option<&str> {
        self.search.as_deref()
    }

    /// The count and operator typed so far (vim's showcmd).
    pub fn pending_keys(&self) -> String {
        let mut s = self.count.map(|c| c.to_string()).unwrap_or_default();
        s.push_str(match self.pending {
            Some(Pending::G) => "g",
            Some(Pending::Delete) => "d",
            Some(Pending::Yank) => "y",
            Some(Pending::Change) => "c",
            Some(Pending::Indent) => ">",
            Some(Pending::Outdent) => "<",
            None => "",
        });
        s
    }

    pub fn reset(&mut self) {
        *self = Vim { register: self.register.take(), last_change: self.last_change.take(), ..Vim::default() };
    }

    /// The last change entered insert mode and its edit has ended with `text` in the cell.
    pub fn inserted(&mut self, text: String) {
        self.awaiting_insert = false;
        if let Some(change) = self.last_change.as_mut() {
            change.text = Some(text);
        }
    }

    pub fn feed(&mut self, input: Input) -> Vec<Action> {
        let mut out = Vec::new();
        if self.replaying.is_none() {
            self.record.push(input.clone());
        }
        if let Some(query) = self.search.as_mut() {
            match input {
                Input::Char(c) => query.push(c),
                Input::Backspace if !query.is_empty() => {
                    query.pop();
                }
                Input::Enter => {
                    if !query.is_empty() {
                        out.push(Action::Search(query.clone()));
                    }
                    self.search = None;
                }
                _ => self.search = None,
            }
            if self.search.is_none() {
                self.record.clear();
            }
            return out;
        }
        let change = match self.mode {
            Mode::Normal => self.normal(input, &mut out),
            Mode::Visual(kind) => self.visual(kind, input, &mut out),
        };
        if self.replaying.is_none() {
            match change {
                Done::Change => {
                    self.awaiting_insert = out.iter().any(|a| matches!(a, Action::Edit(..)));
                    self.last_change = Some(Change { inputs: std::mem::take(&mut self.record), text: None });
                }
                Done::Other => self.record.clear(),
                Done::Pending => {}
            }
        }
        out
    }

    fn take_count(&mut self) -> u32 {
        self.count.take().unwrap_or(1)
    }

    fn digit(&mut self, c: char) -> bool {
        let Some(d) = c.to_digit(10) else { return false };
        if d == 0 && self.count.is_none() {
            return false;
        }
        self.count = Some((self.count.unwrap_or(0).saturating_mul(10).saturating_add(d)).min(MAX_COUNT));
        true
    }

    fn motion(c: &Input) -> Option<Motion> {
        Some(match c {
            Input::Char('h') | Input::Backspace => Motion::Left,
            Input::Char('l' | ' ') => Motion::Right,
            Input::Char('j') => Motion::Down,
            Input::Char('k') => Motion::Up,
            Input::Char('w') => Motion::NextFilled,
            Input::Char('b') => Motion::PrevFilled,
            Input::Char('}') => Motion::BlockDown,
            Input::Char('{') => Motion::BlockUp,
            Input::Char('0' | '^') => Motion::RowStart,
            Input::Char('$') => Motion::RowEnd,
            Input::Char('G') => Motion::Bottom,
            Input::Char('H') => Motion::ScreenTop,
            Input::Char('M') => Motion::ScreenMiddle,
            Input::Char('L') => Motion::ScreenBottom,
            Input::Ctrl('d') => Motion::HalfDown,
            Input::Ctrl('u') => Motion::HalfUp,
            _ => return None,
        })
    }

    /// Keys shared by normal and visual mode: counts, motions and `g` prefixes.
    fn common(&mut self, input: &Input, out: &mut Vec<Action>) -> Option<Done> {
        if self.pending == Some(Pending::G) {
            self.pending = None;
            let count = self.count.take();
            match input {
                Input::Char('g') => out.push(Action::Move(Motion::Top, count)),
                Input::Char('t') => out.push(Action::SheetNext(count.unwrap_or(1))),
                Input::Char('T') => out.push(Action::SheetPrevious(count.unwrap_or(1))),
                _ => {}
            }
            return Some(Done::Other);
        }
        if let Input::Char(c) = input
            && self.digit(*c)
        {
            return Some(Done::Pending);
        }
        if let Some(m) = Self::motion(input) {
            out.push(Action::Move(m, self.count.take()));
            return Some(Done::Other);
        }
        match input {
            Input::Char('g') => {
                self.pending = Some(Pending::G);
                Some(Done::Pending)
            }
            _ => None,
        }
    }

    fn normal(&mut self, input: Input, out: &mut Vec<Action>) -> Done {
        if input == Input::Esc {
            self.count = None;
            self.pending = None;
            out.push(Action::Escape);
            return Done::Other;
        }
        if let Some(op) = self.pending.filter(|p| *p != Pending::G) {
            self.pending = None;
            let count = self.take_count();
            return match (op, &input) {
                (Pending::Delete, Input::Char('d')) => {
                    out.push(Action::DeleteRows(count));
                    Done::Change
                }
                (Pending::Yank, Input::Char('y')) => {
                    out.push(Action::YankRows(count));
                    Done::Other
                }
                (Pending::Change, Input::Char('c')) => {
                    out.push(Action::Edit(Edit::Substitute, self.repeat_text()));
                    Done::Change
                }
                (Pending::Indent, Input::Char('>')) => {
                    out.push(Action::Indent(count as i32));
                    Done::Change
                }
                (Pending::Outdent, Input::Char('<')) => {
                    out.push(Action::Indent(-(count as i32)));
                    Done::Change
                }
                _ => Done::Other,
            };
        }
        if let Some(done) = self.common(&input, out) {
            return done;
        }
        let edit = |kind| Action::Edit(kind, None);
        let (action, done) = match input {
            Input::Char('i') => (edit(Edit::Insert), Done::Change),
            Input::Char('a') | Input::Enter => (edit(Edit::Append), Done::Change),
            Input::Char('s' | 'S') => (edit(Edit::Substitute), Done::Change),
            Input::Char('o') => (edit(Edit::OpenBelow), Done::Change),
            Input::Char('O') => (edit(Edit::OpenAbove), Done::Change),
            Input::Char('x') => (Action::Clear(self.take_count()), Done::Change),
            Input::Char('Y') => (Action::YankRows(self.take_count()), Done::Other),
            Input::Char('p') => (Action::Put { before: false, count: self.take_count() }, Done::Change),
            Input::Char('P') => (Action::Put { before: true, count: self.take_count() }, Done::Change),
            Input::Char('u') => (Action::Undo(self.take_count()), Done::Other),
            Input::Ctrl('r') => (Action::Redo(self.take_count()), Done::Other),
            Input::Char('n') => (Action::SearchNext { forward: true, count: self.take_count() }, Done::Other),
            Input::Char('N') => (Action::SearchNext { forward: false, count: self.take_count() }, Done::Other),
            Input::Char('/') => {
                self.count = None;
                self.search = Some(String::new());
                return Done::Pending;
            }
            Input::Char('.') => {
                let count = self.take_count();
                self.repeat(count, out);
                return Done::Other;
            }
            Input::Char(c @ ('d' | 'y' | 'c' | '>' | '<')) => {
                self.pending = Some(match c {
                    'd' => Pending::Delete,
                    'y' => Pending::Yank,
                    'c' => Pending::Change,
                    '>' => Pending::Indent,
                    _ => Pending::Outdent,
                });
                return Done::Pending;
            }
            Input::Char('v') => return self.enter_visual(Visual::Cells, out),
            Input::Char('V') => return self.enter_visual(Visual::Rows, out),
            Input::Ctrl('v' | 'q') => return self.enter_visual(Visual::Cols, out),
            _ => {
                self.count = None;
                return Done::Other;
            }
        };
        let action = match action {
            Action::Edit(kind, None) => Action::Edit(kind, self.repeat_text()),
            other => other,
        };
        out.push(action);
        done
    }

    fn enter_visual(&mut self, kind: Visual, out: &mut Vec<Action>) -> Done {
        self.count = None;
        self.mode = Mode::Visual(kind);
        out.push(Action::Visual(Some(kind)));
        // Recording continues until an operator ends the selection (see `visual`).
        Done::Pending
    }

    fn leave_visual(&mut self, out: &mut Vec<Action>) {
        self.mode = Mode::Normal;
        self.count = None;
        self.pending = None;
        out.push(Action::Visual(None));
    }

    fn visual(&mut self, kind: Visual, input: Input, out: &mut Vec<Action>) -> Done {
        if let Some(done) = self.common(&input, out) {
            // Motions extend the selection; keep recording so `.` reselects the same shape.
            return if done == Done::Other && self.pending.is_none() { Done::Pending } else { done };
        }
        let op = match input {
            Input::Char('y' | 'Y') => Op::Yank,
            Input::Char('d' | 'x' | 'D' | 'X') => Op::Delete,
            Input::Char('c' | 's' | 'C' | 'S') => Op::Change,
            Input::Char('>') => Op::Indent,
            Input::Char('<') => Op::Outdent,
            Input::Char('o') => {
                out.push(Action::VisualSwap);
                return Done::Pending;
            }
            Input::Char('v') | Input::Char('V') | Input::Ctrl('v' | 'q') => {
                let next = match input {
                    Input::Char('v') => Visual::Cells,
                    Input::Char('V') => Visual::Rows,
                    _ => Visual::Cols,
                };
                if next == kind {
                    self.leave_visual(out);
                    return Done::Other;
                }
                self.mode = Mode::Visual(next);
                out.push(Action::Visual(Some(next)));
                return Done::Pending;
            }
            Input::Esc => {
                self.leave_visual(out);
                return Done::Other;
            }
            _ => {
                self.count = None;
                return Done::Pending;
            }
        };
        out.push(Action::VisualOp(op));
        self.leave_visual(out);
        if op == Op::Change {
            out.push(Action::Edit(Edit::Substitute, self.repeat_text()));
        }
        if op == Op::Yank { Done::Other } else { Done::Change }
    }

    fn repeat_text(&self) -> Option<String> {
        self.replaying.clone().flatten()
    }

    fn repeat(&mut self, count: u32, out: &mut Vec<Action>) {
        let Some(change) = self.last_change.clone() else { return };
        self.replaying = Some(change.text.clone());
        for _ in 0..count.min(1000) {
            for input in &change.inputs {
                out.extend(self.feed(input.clone()));
            }
        }
        self.replaying = None;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Done {
    /// A complete change, which `.` repeats.
    Change,
    /// A complete command that is not a change.
    Other,
    /// More keys to come.
    Pending,
}

fn printable(key: Key) -> bool {
    match key {
        Key::ArrowDown | Key::ArrowLeft | Key::ArrowRight | Key::ArrowUp => false,
        Key::Space => true,
        _ => key.symbol_or_name().chars().count() == 1,
    }
}

/// Removes the keys vim owns from this frame's events and returns them. Arrows, Tab, Delete,
/// F-keys, Cmd/Ctrl shortcuts and clipboard events stay for the grid. Ctrl+V stays paste outside
/// macOS (Ctrl+Q starts column visual there).
pub fn take_inputs(ctx: &Context) -> Vec<Input> {
    let mac = ctx.os() == OperatingSystem::Mac;
    let mut inputs = Vec::new();
    ctx.input_mut(|i| {
        i.events.retain(|event| match event {
            Event::Text(text) => {
                inputs.extend(text.chars().filter(|c| !c.is_control()).map(Input::Char));
                false
            }
            Event::Key { key, pressed, modifiers: m, .. } => {
                if m.ctrl && !m.alt && !m.shift && !m.mac_cmd {
                    let c = match key {
                        Key::D => 'd',
                        Key::U => 'u',
                        Key::R => 'r',
                        Key::V if mac => 'v',
                        Key::Q => 'q',
                        _ => return true,
                    };
                    if *pressed {
                        inputs.push(Input::Ctrl(c));
                    }
                    return false;
                }
                if m.ctrl || m.command || m.mac_cmd || m.alt {
                    return true;
                }
                let named = match key {
                    Key::Escape => Some(Input::Esc),
                    Key::Enter => Some(Input::Enter),
                    Key::Backspace => Some(Input::Backspace),
                    _ => None,
                };
                if let Some(input) = named {
                    if *pressed {
                        inputs.push(input);
                    }
                    return false;
                }
                // The text arrives as its own event.
                !printable(*key)
            }
            _ => true,
        });
    });
    inputs
}

/// Status bar label for the current mode.
pub fn mode_label(app: &SheetApp) -> &'static str {
    if app.editor.is_some() {
        return "INSERT";
    }
    match app.vim.mode() {
        Mode::Normal => "NORMAL",
        Mode::Visual(Visual::Cells) => "VISUAL",
        Mode::Visual(Visual::Rows) => "V-ROW",
        Mode::Visual(Visual::Cols) => "V-COLUMN",
    }
}

/// Runs vim for one frame. `enabled` is false when anything but the grid owns the keyboard.
pub fn process(app: &mut SheetApp, ctx: &Context, enabled: bool) {
    if !app.ui.vim_mode {
        if app.vim.mode() != Mode::Normal || app.vim.search().is_some() {
            app.vim.reset();
        }
        return;
    }
    if app.vim.awaiting_insert && app.editor.is_none() {
        let text = app
            .vim
            .insert_cell
            .take()
            .and_then(|(sheet, cell)| app.session.active().and_then(|d| d.wb.sheet(sheet)).and_then(|sh| sh.cell(cell)).map(|c| c.input_text()))
            .unwrap_or_default();
        app.vim.inserted(text);
    }
    if !enabled {
        return;
    }
    if matches!(app.vim.mode(), Mode::Visual(_)) && ctx.input(|i| i.pointer.any_pressed()) {
        app.vim.mode = Mode::Normal;
        app.vim.anchor = None;
        app.vim.shape = None;
    }
    for input in take_inputs(ctx) {
        let actions = app.vim.feed(input);
        for action in actions {
            run(app, ctx, action);
        }
    }
}

fn active(app: &SheetApp) -> CellRef {
    app.session.active().map(|d| d.selection.active).unwrap_or_default()
}

fn select_cell(app: &mut SheetApp, cell: CellRef) {
    let _ = app.session.run("selection.set", json!({"cell": cell.a1()}));
}

/// Last used column and row of the active sheet (0 when empty).
fn used_end(app: &SheetApp) -> CellRef {
    app.session.active().and_then(|d| d.wb.active()).and_then(|sh| sh.used_range()).map(|u| u.end).unwrap_or_default()
}

fn run(app: &mut SheetApp, ctx: &Context, action: Action) {
    match action {
        Action::Move(motion, count) => {
            move_cursor(app, motion, count);
            if let Some(kind) = app.vim.shape {
                select_visual(app, kind);
            }
            app.grid.ensure_visible = true;
        }
        Action::Edit(kind, text) => edit(app, kind, text),
        Action::Clear(count) => {
            let a = active(app);
            let range = RangeRef::new(a, a.offset_clamped(0, count.saturating_sub(1) as i64));
            yank(app, ctx, range, Visual::Cells);
            app.run_or_alert("edit.clearContents", json!({"range": range.a1()}));
        }
        Action::YankRows(count) => {
            let range = rows_range(app, active(app).row, count);
            yank(app, ctx, range, Visual::Rows);
        }
        Action::DeleteRows(count) => {
            let a = active(app);
            let range = rows_range(app, a.row, count);
            yank(app, ctx, range, Visual::Rows);
            app.run_or_alert("home.deleteRows", json!({"rows": lines_a1(a.row, range.end.row)}));
            select_cell(app, CellRef::new(a.row, a.col));
        }
        Action::Put { before, count } => put(app, before, count),
        Action::Indent(by) => {
            let id = if by > 0 { "home.increaseIndent" } else { "home.decreaseIndent" };
            for _ in 0..by.unsigned_abs().min(15) {
                app.run_or_alert(id, json!({}));
            }
        }
        Action::Undo(count) => repeat_command(app, "edit.undo", count),
        Action::Redo(count) => repeat_command(app, "edit.redo", count),
        Action::Visual(Some(kind)) => {
            if app.vim.anchor.is_none() {
                app.vim.anchor = Some(active(app));
            }
            app.vim.shape = Some(kind);
            select_visual(app, kind);
        }
        Action::Visual(None) => {
            app.vim.anchor = None;
            app.vim.shape = None;
            let a = active(app);
            select_cell(app, a);
        }
        Action::VisualSwap => {
            if let (Some(anchor), Some(kind)) = (app.vim.anchor, app.vim.shape) {
                app.vim.anchor = Some(active(app));
                select_cell(app, anchor);
                select_visual(app, kind);
                app.grid.ensure_visible = true;
            }
        }
        Action::VisualOp(op) => visual_op(app, ctx, op),
        Action::SheetNext(count) => repeat_command(app, "sheet.next", count),
        Action::SheetPrevious(count) => repeat_command(app, "sheet.previous", count),
        Action::Search(what) => find(app, json!({"what": what})),
        Action::SearchNext { forward, count } => {
            let Some(last) = app.session.last_find.clone() else {
                app.toast = Some(("No previous search".into(), crate::now_ms()));
                return;
            };
            for _ in 0..count.min(1000) {
                if forward {
                    find(app, last.clone());
                } else {
                    find_previous(app, &last);
                }
            }
        }
        Action::Escape => {
            app.session.clipboard = None;
            app.session.format_painter = None;
        }
    }
}

fn repeat_command(app: &mut SheetApp, id: &str, count: u32) {
    for _ in 0..count.min(1000) {
        if app.run(id, json!({})).is_err() {
            break;
        }
    }
    app.grid.ensure_visible = true;
}

fn move_cursor(app: &mut SheetApp, motion: Motion, count: Option<u32>) {
    let n = count.unwrap_or(1) as i64;
    let a = active(app);
    let step = |app: &mut SheetApp, dr: i64, dc: i64, jump: bool| {
        let _ = app.session.run("selection.move", json!({"dr": dr, "dc": dc, "jump": jump}));
    };
    let jump = |app: &mut SheetApp, dr: i64, dc: i64| {
        for _ in 0..n.min(1000) {
            step(app, dr, dc, true);
        }
    };
    let (top, bottom) = app.grid.visible_rows.unwrap_or((a.row, a.row));
    let half = (((bottom.saturating_sub(top)) / 2).max(1)) as i64;
    match motion {
        Motion::Left => step(app, 0, -n, false),
        Motion::Right => step(app, 0, n, false),
        Motion::Up => step(app, -n, 0, false),
        Motion::Down => step(app, n, 0, false),
        Motion::NextFilled => jump(app, 0, 1),
        Motion::PrevFilled => jump(app, 0, -1),
        Motion::BlockDown => jump(app, 1, 0),
        Motion::BlockUp => jump(app, -1, 0),
        Motion::RowStart => select_cell(app, CellRef::new(a.row, 0)),
        Motion::RowEnd => {
            // From the sheet's right edge, Ctrl+Left lands on the row's last filled cell.
            select_cell(app, CellRef::new(a.row, MAX_COLS - 1));
            step(app, 0, -1, true);
        }
        Motion::Top => select_cell(app, CellRef::new(count.map_or(0, |c| c.saturating_sub(1)), a.col)),
        Motion::Bottom => {
            let row = count.map_or_else(|| used_end(app).row, |c| c.saturating_sub(1));
            select_cell(app, CellRef::new(row, a.col));
        }
        Motion::HalfDown => step(app, half * n, 0, false),
        Motion::HalfUp => step(app, -half * n, 0, false),
        Motion::ScreenTop => select_cell(app, CellRef::new(top, a.col)),
        Motion::ScreenMiddle => select_cell(app, CellRef::new(top + (bottom.saturating_sub(top)) / 2, a.col)),
        Motion::ScreenBottom => select_cell(app, CellRef::new(bottom, a.col)),
    }
}

/// Selects anchor..cursor in the shape of `kind`, keeping the cursor as the active cell.
fn select_visual(app: &mut SheetApp, kind: Visual) {
    let cursor = active(app);
    let anchor = app.vim.anchor.unwrap_or(cursor);
    let range = match kind {
        Visual::Cells => RangeRef::new(anchor, cursor),
        Visual::Rows => RangeRef::rows(anchor.row.min(cursor.row), anchor.row.max(cursor.row)),
        Visual::Cols => RangeRef::cols(anchor.col.min(cursor.col), anchor.col.max(cursor.col)),
    };
    let _ = app.session.run("selection.set", json!({"range": range.a1(), "active": cursor.a1()}));
}

/// `count` rows from `row`, limited to the used columns (a whole-row copy would hold 16,384 cells).
fn rows_range(app: &SheetApp, row: u32, count: u32) -> RangeRef {
    let end = row.saturating_add(count.saturating_sub(1)).min(MAX_ROWS - 1);
    RangeRef::new(CellRef::new(row, 0), CellRef::new(end, used_end(app).col))
}

fn lines_a1(first: u32, last: u32) -> String {
    format!("{}:{}", first + 1, last + 1)
}

fn cols_a1(first: u32, last: u32) -> String {
    RangeRef::cols(first, last).a1()
}

/// Copies `range` into vim's register and the system clipboard, without marching ants.
fn yank(app: &mut SheetApp, ctx: &Context, range: RangeRef, kind: Visual) {
    match app.run("edit.copy", json!({"range": range.a1()})) {
        Ok(result) => {
            if let Some(text) = result.get("text").and_then(Json::as_str) {
                ctx.copy_text(text.to_string());
            }
            if let Some(clip) = app.session.clipboard.take() {
                app.vim.register = Some(Register { clip, kind });
            }
        }
        Err(e) => app.message = Some(("GridCraft".into(), crate::clean_error(&e))),
    }
}

fn put(app: &mut SheetApp, before: bool, count: u32) {
    let Some(reg) = app.vim.register.clone() else {
        app.toast = Some(("Nothing to put".into(), crate::now_ms()));
        return;
    };
    let a = active(app);
    let src = reg.clip.range;
    restore(app, &reg);
    match reg.kind {
        Visual::Cells => app.run_or_alert("edit.paste", json!({"at": a.a1()})),
        Visual::Rows => {
            let height = src.height().saturating_mul(count.clamp(1, 1000));
            let at = if before { a.row } else { a.row + 1 };
            if app.run("home.insertRows", json!({"rows": lines_a1(at, at + height - 1)})).is_ok() {
                for i in 0..count.clamp(1, 1000) {
                    restore(app, &reg);
                    let row = at + i * src.height();
                    app.run_or_alert("edit.paste", json!({"at": CellRef::new(row, 0).a1()}));
                }
                select_cell(app, CellRef::new(at, a.col));
            }
        }
        Visual::Cols => {
            let width = src.width().saturating_mul(count.clamp(1, 1000));
            let at = if before { a.col } else { a.col + 1 };
            if app.run("home.insertColumns", json!({"cols": cols_a1(at, at + width - 1)})).is_ok() {
                for i in 0..count.clamp(1, 1000) {
                    restore(app, &reg);
                    let col = at + i * src.width();
                    app.run_or_alert("edit.paste", json!({"at": CellRef::new(0, col).a1()}));
                }
                select_cell(app, CellRef::new(a.row, at));
            }
        }
    }
    app.session.clipboard = None;
    app.grid.ensure_visible = true;
}

/// Structural edits drop the engine clipboard; put it back before each paste.
fn restore(app: &mut SheetApp, reg: &Register) {
    app.session.clipboard = Some(Clipboard { cut: false, ..reg.clip.clone() });
}

fn visual_op(app: &mut SheetApp, ctx: &Context, op: Op) {
    let Some(kind) = app.vim.shape else { return };
    let Some(sel) = app.session.active().map(|d| d.selection.current()) else { return };
    let used = used_end(app);
    // Whole rows / columns are limited to the used area for the register.
    let copy = match kind {
        Visual::Cells => sel,
        Visual::Rows => RangeRef::new(CellRef::new(sel.start.row, 0), CellRef::new(sel.end.row, used.col)),
        Visual::Cols => RangeRef::new(CellRef::new(0, sel.start.col), CellRef::new(used.row, sel.end.col)),
    };
    let start = match kind {
        Visual::Cells => sel.start,
        Visual::Rows => CellRef::new(sel.start.row, active(app).col),
        Visual::Cols => CellRef::new(active(app).row, sel.start.col),
    };
    match op {
        Op::Yank => yank(app, ctx, copy, kind),
        Op::Delete => {
            yank(app, ctx, copy, kind);
            match kind {
                Visual::Cells => app.run_or_alert("edit.clearContents", json!({"range": sel.a1()})),
                Visual::Rows => app.run_or_alert("home.deleteRows", json!({"rows": lines_a1(sel.start.row, sel.end.row)})),
                Visual::Cols => app.run_or_alert("home.deleteColumns", json!({"cols": cols_a1(sel.start.col, sel.end.col)})),
            }
        }
        Op::Change => {
            yank(app, ctx, copy, kind);
            app.run_or_alert("edit.clearContents", json!({"range": sel.a1()}));
        }
        Op::Indent => app.run_or_alert("home.increaseIndent", json!({})),
        Op::Outdent => app.run_or_alert("home.decreaseIndent", json!({})),
    }
    // Leaving visual collapses the selection to the active cell: make that the start.
    select_cell(app, start);
}

fn edit(app: &mut SheetApp, kind: Edit, text: Option<String>) {
    let a = active(app);
    let target = match kind {
        Edit::OpenBelow | Edit::OpenAbove => {
            let row = if kind == Edit::OpenBelow { a.row + 1 } else { a.row };
            if let Err(e) = app.run("home.insertRows", json!({"rows": lines_a1(row, row)})) {
                app.message = Some(("GridCraft".into(), crate::clean_error(&e)));
                return;
            }
            let cell = CellRef::new(row, a.col);
            select_cell(app, cell);
            app.grid.ensure_visible = true;
            cell
        }
        _ => a,
    };
    if let Some(text) = text {
        app.run_or_alert("cell.set", json!({"cell": target.a1(), "input": text}));
        return;
    }
    match kind {
        Edit::Insert | Edit::Append => app.begin_edit(None, false),
        Edit::Substitute | Edit::OpenBelow | Edit::OpenAbove => app.begin_edit(Some(String::new()), false),
    }
    if kind == Edit::Insert
        && let Some(ed) = app.editor.as_mut()
    {
        ed.caret = 0;
        ed.caret_to_end = false;
        ed.sync_caret = true;
    }
    if let Some(ed) = app.editor.as_ref() {
        app.vim.insert_cell = Some((ed.sheet, ed.cell));
    }
}

fn find(app: &mut SheetApp, params: Json) {
    if let Err(e) = app.run("edit.find", params) {
        app.toast = Some((crate::clean_error(&e), crate::now_ms()));
    }
    app.grid.ensure_visible = true;
}

/// `edit.find` only searches forward: list every hit and take the one before the cursor.
fn find_previous(app: &mut SheetApp, last: &Json) {
    let mut params = last.clone();
    if let Some(p) = params.as_object_mut() {
        p.insert("all".into(), Json::Bool(true));
    }
    let result = app.session.run("edit.find", params);
    app.session.last_find = Some(last.clone());
    let result = match result {
        Ok(r) => r,
        Err(e) => {
            app.toast = Some((crate::clean_error(&e), crate::now_ms()));
            return;
        }
    };
    let by_cols = last.get("byColumns").and_then(Json::as_bool).unwrap_or(false);
    let Some((sheet_name, a)) = app.session.active().and_then(|d| d.wb.active().map(|sh| (sh.name.clone(), d.selection.active))) else {
        return;
    };
    let key = |c: CellRef| if by_cols { (c.col, c.row) } else { (c.row, c.col) };
    let hits: Vec<CellRef> = result
        .get("results")
        .and_then(Json::as_array)
        .map(|list| {
            list.iter()
                .filter(|h| h.get("sheet").and_then(Json::as_str).is_none_or(|s| s == sheet_name))
                .filter_map(|h| h.get("cell").and_then(Json::as_str).and_then(CellRef::parse))
                .collect()
        })
        .unwrap_or_default();
    let target = hits.iter().copied().filter(|c| key(*c) < key(a)).max_by_key(|c| key(*c)).or_else(|| hits.iter().copied().max_by_key(|c| key(*c)));
    if let Some(cell) = target {
        select_cell(app, cell);
        app.grid.ensure_visible = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(vim: &mut Vim, keys: &str) -> Vec<Action> {
        keys.chars().flat_map(|c| vim.feed(Input::Char(c))).collect()
    }

    #[test]
    fn counts_apply_to_motions() {
        let mut vim = Vim::default();
        assert_eq!(feed(&mut vim, "5j"), vec![Action::Move(Motion::Down, Some(5))]);
        assert_eq!(feed(&mut vim, "12l"), vec![Action::Move(Motion::Right, Some(12))]);
        assert_eq!(feed(&mut vim, "k"), vec![Action::Move(Motion::Up, None)]);
    }

    #[test]
    fn zero_is_a_motion_unless_it_continues_a_count() {
        let mut vim = Vim::default();
        assert_eq!(feed(&mut vim, "0"), vec![Action::Move(Motion::RowStart, None)]);
        assert_eq!(feed(&mut vim, "10j"), vec![Action::Move(Motion::Down, Some(10))]);
    }

    #[test]
    fn g_prefixes() {
        let mut vim = Vim::default();
        assert_eq!(feed(&mut vim, "gg"), vec![Action::Move(Motion::Top, None)]);
        assert_eq!(feed(&mut vim, "7gg"), vec![Action::Move(Motion::Top, Some(7))]);
        assert_eq!(feed(&mut vim, "G"), vec![Action::Move(Motion::Bottom, None)]);
        assert_eq!(feed(&mut vim, "gt"), vec![Action::SheetNext(1)]);
        assert_eq!(feed(&mut vim, "2gT"), vec![Action::SheetPrevious(2)]);
        assert_eq!(feed(&mut vim, "gq"), vec![]);
        assert_eq!(vim.pending_keys(), "");
    }

    #[test]
    fn operators_wait_for_their_second_key() {
        let mut vim = Vim::default();
        assert_eq!(feed(&mut vim, "3d"), vec![]);
        assert_eq!(vim.pending_keys(), "3d");
        assert_eq!(feed(&mut vim, "d"), vec![Action::DeleteRows(3)]);
        assert_eq!(feed(&mut vim, "yy"), vec![Action::YankRows(1)]);
        assert_eq!(feed(&mut vim, ">>"), vec![Action::Indent(1)]);
        assert_eq!(feed(&mut vim, "2<<"), vec![Action::Indent(-2)]);
        assert_eq!(feed(&mut vim, "cc"), vec![Action::Edit(Edit::Substitute, None)]);
        // A different second key cancels the operator.
        assert_eq!(feed(&mut vim, "dj"), vec![]);
        assert_eq!(vim.pending_keys(), "");
    }

    #[test]
    fn escape_cancels_counts_and_operators() {
        let mut vim = Vim::default();
        feed(&mut vim, "4d");
        assert_eq!(vim.feed(Input::Esc), vec![Action::Escape]);
        assert_eq!(vim.pending_keys(), "");
        assert_eq!(feed(&mut vim, "j"), vec![Action::Move(Motion::Down, None)]);
    }

    #[test]
    fn visual_mode_extends_then_operates() {
        let mut vim = Vim::default();
        assert_eq!(feed(&mut vim, "v"), vec![Action::Visual(Some(Visual::Cells))]);
        assert_eq!(vim.mode(), Mode::Visual(Visual::Cells));
        assert_eq!(feed(&mut vim, "2j"), vec![Action::Move(Motion::Down, Some(2))]);
        assert_eq!(feed(&mut vim, "V"), vec![Action::Visual(Some(Visual::Rows))]);
        assert_eq!(feed(&mut vim, "d"), vec![Action::VisualOp(Op::Delete), Action::Visual(None)]);
        assert_eq!(vim.mode(), Mode::Normal);
    }

    #[test]
    fn visual_toggles_off_with_its_own_key_and_escape() {
        let mut vim = Vim::default();
        feed(&mut vim, "v");
        assert_eq!(feed(&mut vim, "v"), vec![Action::Visual(None)]);
        vim.feed(Input::Ctrl('v'));
        assert_eq!(vim.mode(), Mode::Visual(Visual::Cols));
        assert_eq!(vim.feed(Input::Esc), vec![Action::Visual(None)]);
        vim.feed(Input::Ctrl('q'));
        assert_eq!(vim.mode(), Mode::Visual(Visual::Cols));
    }

    #[test]
    fn visual_change_clears_and_edits() {
        let mut vim = Vim::default();
        assert_eq!(
            feed(&mut vim, "vlc"),
            vec![
                Action::Visual(Some(Visual::Cells)),
                Action::Move(Motion::Right, None),
                Action::VisualOp(Op::Change),
                Action::Visual(None),
                Action::Edit(Edit::Substitute, None),
            ]
        );
    }

    #[test]
    fn dot_repeats_the_last_change() {
        let mut vim = Vim::default();
        feed(&mut vim, "2x");
        feed(&mut vim, "j");
        assert_eq!(feed(&mut vim, "."), vec![Action::Clear(2)]);
        assert_eq!(feed(&mut vim, "3."), vec![Action::Clear(2), Action::Clear(2), Action::Clear(2)]);
        // Yanks and motions are not changes.
        feed(&mut vim, "yy");
        assert_eq!(feed(&mut vim, "."), vec![Action::Clear(2)]);
    }

    #[test]
    fn dot_repeats_a_visual_change_with_its_motions() {
        let mut vim = Vim::default();
        feed(&mut vim, "vjd");
        assert_eq!(
            feed(&mut vim, "."),
            vec![Action::Visual(Some(Visual::Cells)), Action::Move(Motion::Down, None), Action::VisualOp(Op::Delete), Action::Visual(None)]
        );
    }

    #[test]
    fn dot_replays_inserted_text() {
        let mut vim = Vim::default();
        assert_eq!(feed(&mut vim, "o"), vec![Action::Edit(Edit::OpenBelow, None)]);
        vim.inserted("hello".into());
        assert_eq!(feed(&mut vim, "."), vec![Action::Edit(Edit::OpenBelow, Some("hello".into()))]);
    }

    #[test]
    fn search_prompt_collects_text() {
        let mut vim = Vim::default();
        assert_eq!(feed(&mut vim, "/ab"), vec![]);
        assert_eq!(vim.search(), Some("ab"));
        vim.feed(Input::Backspace);
        assert_eq!(vim.search(), Some("a"));
        assert_eq!(vim.feed(Input::Enter), vec![Action::Search("a".into())]);
        assert_eq!(vim.search(), None);
        assert_eq!(feed(&mut vim, "n"), vec![Action::SearchNext { forward: true, count: 1 }]);
        assert_eq!(feed(&mut vim, "2N"), vec![Action::SearchNext { forward: false, count: 2 }]);
        feed(&mut vim, "/x");
        assert_eq!(vim.feed(Input::Esc), vec![]);
        assert_eq!(vim.search(), None);
    }

    #[test]
    fn unknown_keys_do_nothing_and_drop_the_count() {
        let mut vim = Vim::default();
        assert_eq!(feed(&mut vim, "5z"), vec![]);
        assert_eq!(feed(&mut vim, "j"), vec![Action::Move(Motion::Down, None)]);
    }

    #[test]
    fn huge_counts_are_capped() {
        let mut vim = Vim::default();
        assert_eq!(feed(&mut vim, "99999999j"), vec![Action::Move(Motion::Down, Some(MAX_COUNT))]);
    }

    #[test]
    fn take_inputs_keeps_grid_keys() {
        let ctx = Context::default();
        let key = |key: Key, modifiers: egui::Modifiers| Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers };
        let events = vec![
            key(Key::J, egui::Modifiers::NONE),
            Event::Text("j".into()),
            key(Key::ArrowDown, egui::Modifiers::NONE),
            key(Key::D, egui::Modifiers::CTRL),
            key(Key::C, egui::Modifiers::COMMAND),
            key(Key::Escape, egui::Modifiers::NONE),
            key(Key::Tab, egui::Modifiers::NONE),
        ];
        let mut inputs = Vec::new();
        let mut remaining = Vec::new();
        let mut output = ctx.run_ui(egui::RawInput { events, ..Default::default() }, |ui| {
            inputs = take_inputs(ui.ctx());
            remaining = ui.input(|i| i.events.clone());
        });
        output.textures_delta.clear();
        assert_eq!(inputs, vec![Input::Char('j'), Input::Ctrl('d'), Input::Esc]);
        assert_eq!(
            remaining,
            vec![key(Key::ArrowDown, egui::Modifiers::NONE), key(Key::C, egui::Modifiers::COMMAND), key(Key::Tab, egui::Modifiers::NONE)]
        );
    }
}
