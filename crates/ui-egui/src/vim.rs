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
    /// In a recorded macro: the text the cell editor committed after the key before it.
    Typed(String),
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
    PageDown,
    PageUp,
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

/// `Pv`, `Pf`, `Pt`, `Pc`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Paste {
    Values,
    Formats,
    Transposed,
    Formulas,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

/// Where `zt`, `zz` and `zb` put the cursor's row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scroll {
    Top,
    Middle,
    Bottom,
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
    DeleteCols(u32),
    YankCols(u32),
    /// `+r` / `+c`: insert lines at the cursor.
    InsertLines {
        cols: bool,
        count: u32,
    },
    Put {
        before: bool,
        count: u32,
    },
    PasteSpecial(Paste),
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
    Search {
        what: String,
        forward: bool,
    },
    SearchNext {
        forward: bool,
        count: u32,
    },
    /// `go` + address, sent after each key once the address is complete. `first` is the first
    /// jump of this prompt.
    GoTo {
        cell: CellRef,
        first: bool,
    },
    SetMark(char),
    JumpMark(char),
    /// `gl`: back to where the last jump started.
    JumpBack,
    Scroll(Scroll),
    Align(Align),
    /// `fh` / `fl` (columns) and `fj` / `fk` (rows): resize by this many steps.
    Resize {
        cols: bool,
        by: i32,
    },
    /// `Zr` / `Zc`.
    Hide {
        cols: bool,
    },
    /// `Sr` / `Sc`.
    Unhide {
        cols: bool,
    },
    /// `zh` / `zl`: scroll this many columns without moving the cursor.
    ScrollCols(i32),
    /// `Ctrl-a` / `Ctrl-x`.
    Increment(i64),
    /// `"a`: the next yank, delete or put uses register `a`.
    Register(char),
    /// `:` opens the palette as a command line.
    Palette,
    /// A mapping's `:command<CR>`.
    Ex(String),
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
    /// `+`: insert a row or column.
    Plus,
    /// `P`: a typed paste key may follow.
    Paste,
    Mark,
    Jump,
    /// `z`: scroll.
    Scroll,
    /// `Z`: hide.
    Hide,
    /// `f`: resize.
    Size,
    /// `S`: show (unhide).
    Show,
    /// `"`: a register name follows.
    Register,
    /// `q`: start recording into a register.
    Record,
    /// `@`: play a register.
    Play,
}

/// What a mapping's left-hand side turns into.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Rhs {
    Keys(Vec<Input>),
    /// `:sort<CR>`: a command line, run without opening the palette.
    Ex(String),
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
    /// The open prompt is `?` (backward), and the direction `n` searches in.
    search_back: bool,
    last_search_back: bool,
    /// The `go` address prompt while it is open, and whether it has moved the cursor yet.
    goto: Option<String>,
    goto_moved: bool,
    /// Inputs of the command being typed, for `.`.
    record: Vec<Input>,
    last_change: Option<Change>,
    replaying: Option<Option<String>>,
    awaiting_insert: bool,
    /// Executor state. The machine's mode runs ahead of the actions it returns, so the executor
    /// keeps the shape of the selection it is drawing.
    anchor: Option<CellRef>,
    shape: Option<Visual>,
    insert_cell: Option<(usize, CellRef)>,
    marks: std::collections::HashMap<char, MarkPos>,
    /// Sheet and cell before the last jump, for `gl`.
    last_jump: Option<(usize, CellRef)>,
    /// Named registers; `"` is the unnamed one every yank and delete writes.
    registers: std::collections::HashMap<char, Register>,
    next_register: Option<char>,
    /// Macro being recorded (register, keys so far), recorded macros and the last one played.
    recording: Option<(char, Vec<Input>)>,
    macros: std::collections::HashMap<char, Vec<Input>>,
    last_macro: Option<char>,
    macro_depth: u32,
    /// Key mappings, parsed from the saved ones in `source`, and keys waiting to complete one.
    maps: Vec<(Vec<Input>, Rhs)>,
    maps_source: std::collections::BTreeMap<String, String>,
    map_buffer: Vec<Input>,
}

#[derive(Clone, Copy, Debug)]
struct MarkPos {
    sheet: usize,
    range: RangeRef,
    active: CellRef,
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
            Some(Pending::Plus) => "+",
            Some(Pending::Paste) => "P",
            Some(Pending::Mark) => "m",
            Some(Pending::Jump) => "'",
            Some(Pending::Scroll) => "z",
            Some(Pending::Hide) => "Z",
            Some(Pending::Size) => "f",
            Some(Pending::Show) => "S",
            Some(Pending::Register) => "\"",
            Some(Pending::Record) => "q",
            Some(Pending::Play) => "@",
            None => "",
        });
        for input in &self.map_buffer {
            s.push_str(&key_name(input));
        }
        if let Some(address) = &self.goto {
            s.push_str("go");
            s.push_str(address);
        }
        s
    }

    pub fn reset(&mut self) {
        *self = Vim {
            registers: std::mem::take(&mut self.registers),
            last_change: self.last_change.take(),
            marks: std::mem::take(&mut self.marks),
            last_jump: self.last_jump.take(),
            macros: std::mem::take(&mut self.macros),
            last_macro: self.last_macro.take(),
            maps: std::mem::take(&mut self.maps),
            maps_source: std::mem::take(&mut self.maps_source),
            ..Vim::default()
        };
    }

    /// The register a macro is being recorded into.
    pub fn recording(&self) -> Option<char> {
        self.recording.as_ref().map(|(r, _)| *r)
    }

    /// Replaces the key mappings with `maps` (left-hand side to right-hand side, in key
    /// notation). Mappings that do not parse are left out.
    pub fn set_maps(&mut self, maps: &std::collections::BTreeMap<String, String>) {
        self.maps = maps
            .iter()
            .filter_map(|(lhs, rhs)| {
                let lhs = parse_keys(lhs);
                let rhs = parse_rhs(rhs)?;
                (!lhs.is_empty()).then_some((lhs, rhs))
            })
            .collect();
        self.maps_source = maps.clone();
        self.map_buffer.clear();
    }

    fn idle(&self) -> bool {
        self.pending.is_none() && self.search.is_none() && self.goto.is_none()
    }

    /// A key the user pressed: recorded into a macro, then through the key mappings.
    pub fn press(&mut self, input: Input) -> Vec<Action> {
        if self.macro_depth == 0
            && let Some((_, keys)) = self.recording.as_mut()
        {
            keys.push(input.clone());
        }
        let mut out = Vec::new();
        self.mapped(input, &mut out);
        out
    }

    fn mapped(&mut self, input: Input, out: &mut Vec<Action>) {
        let starts_map = self.idle() && self.maps.iter().any(|(lhs, _)| lhs.first() == Some(&input));
        if self.map_buffer.is_empty() && !starts_map {
            out.extend(self.feed(input));
            return;
        }
        self.map_buffer.push(input);
        let buffer = self.map_buffer.clone();
        // Wait while a longer mapping could still match (there is no timeout).
        if self.maps.iter().any(|(lhs, _)| lhs.len() > buffer.len() && lhs.starts_with(&buffer)) {
            return;
        }
        self.map_buffer.clear();
        let fired =
            (1..=buffer.len()).rev().find_map(|n| self.maps.iter().find(|(lhs, _)| lhs.as_slice() == &buffer[..n]).map(|(_, rhs)| (n, rhs.clone())));
        let rest = match fired {
            Some((n, rhs)) => {
                match rhs {
                    // Right-hand sides are not mapped again (vim's noremap).
                    Rhs::Keys(keys) => {
                        for key in keys {
                            out.extend(self.feed(key));
                        }
                    }
                    Rhs::Ex(line) => out.push(Action::Ex(line)),
                }
                &buffer[n..]
            }
            None => {
                if let Some(first) = buffer.first() {
                    out.extend(self.feed(first.clone()));
                }
                buffer.get(1..).unwrap_or_default()
            }
        };
        for key in rest.iter().cloned() {
            self.mapped(key, out);
        }
    }

    /// The last change entered insert mode and its edit has ended with `text` in the cell.
    pub fn inserted(&mut self, text: String) {
        self.awaiting_insert = false;
        if self.macro_depth == 0
            && let Some((_, keys)) = self.recording.as_mut()
        {
            keys.push(Input::Typed(text.clone()));
        }
        if let Some(change) = self.last_change.as_mut() {
            change.text = Some(text);
        }
    }

    pub fn feed(&mut self, input: Input) -> Vec<Action> {
        let mut out = Vec::new();
        if matches!(input, Input::Typed(_)) {
            return out;
        }
        if self.replaying.is_none() {
            self.record.push(input.clone());
        }
        if let Some(address) = self.goto.as_mut() {
            let letters = address.chars().take_while(char::is_ascii_alphabetic).count();
            let digits = address.len() - letters;
            match &input {
                Input::Char(c) if (c.is_ascii_alphabetic() && digits == 0 && letters < 3) || (c.is_ascii_digit() && letters > 0 && digits < 7) => {
                    address.push(c.to_ascii_uppercase());
                    if let Some(cell) = CellRef::parse(address) {
                        out.push(Action::GoTo { cell, first: !self.goto_moved });
                        self.goto_moved = true;
                    }
                    return out;
                }
                Input::Backspace => {
                    address.pop();
                    return out;
                }
                Input::Enter | Input::Esc => {
                    self.goto = None;
                    self.record.clear();
                    return out;
                }
                _ => {
                    // Any other key ends the address and then does its own job.
                    self.goto = None;
                    self.record.clear();
                    if self.replaying.is_none() {
                        self.record.push(input.clone());
                    }
                }
            }
        }
        if let Some(query) = self.search.as_mut() {
            match input {
                Input::Char(c) => query.push(c),
                Input::Backspace if !query.is_empty() => {
                    query.pop();
                }
                Input::Enter => {
                    if !query.is_empty() {
                        out.push(Action::Search { what: query.clone(), forward: !self.search_back });
                        self.last_search_back = self.search_back;
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
            Input::Char(']') => Motion::BlockDown,
            Input::Char('[') => Motion::BlockUp,
            Input::Char('0' | '^') => Motion::RowStart,
            Input::Char('$') => Motion::RowEnd,
            Input::Char('G') => Motion::Bottom,
            Input::Char('H') => Motion::ScreenTop,
            Input::Char('M') => Motion::ScreenMiddle,
            Input::Char('L') => Motion::ScreenBottom,
            Input::Ctrl('d') => Motion::HalfDown,
            Input::Ctrl('u') => Motion::HalfUp,
            Input::Ctrl('f') => Motion::PageDown,
            Input::Ctrl('b') => Motion::PageUp,
            _ => return None,
        })
    }

    /// Keys shared by normal and visual mode: counts, motions and prefixes.
    fn common(&mut self, input: &Input, out: &mut Vec<Action>) -> Option<Done> {
        if let Input::Char(c) = input
            && self.digit(*c)
        {
            return Some(Done::Pending);
        }
        if let Some(m) = Self::motion(input) {
            out.push(Action::Move(m, self.count.take()));
            return Some(Done::Other);
        }
        let prefix = match input {
            Input::Char('g') => Pending::G,
            Input::Char('z') => Pending::Scroll,
            Input::Char('\'' | '`') => Pending::Jump,
            Input::Char('m') => Pending::Mark,
            Input::Char('Z') => Pending::Hide,
            Input::Char('S') => Pending::Show,
            Input::Char('f') => Pending::Size,
            Input::Char('"') => Pending::Register,
            _ => return None,
        };
        self.pending = Some(prefix);
        Some(Done::Pending)
    }

    /// The key after a prefix or operator.
    fn prefixed(&mut self, pending: Pending, input: Input, out: &mut Vec<Action>) -> Done {
        if pending == Pending::Paste && !matches!(input, Input::Char('v' | 'f' | 't' | 'c')) {
            // A plain `P`, then the key does its own job (and is a command of its own for `.`).
            out.push(Action::Put { before: true, count: self.take_count() });
            if self.replaying.is_none() {
                let mut inputs = std::mem::take(&mut self.record);
                let key = inputs.pop();
                self.last_change = Some(Change { inputs, text: None });
                self.record.extend(key);
            }
            return match self.mode {
                Mode::Normal => self.normal(input, out),
                Mode::Visual(kind) => self.visual(kind, input, out),
            };
        }
        match (pending, &input) {
            (Pending::Register, Input::Char(c @ ('a'..='z' | '"'))) => {
                // The count and the command still to come stay pending.
                out.push(Action::Register(*c));
                return Done::Pending;
            }
            (Pending::Record, Input::Char(c @ 'a'..='z')) => {
                self.count = None;
                self.recording = Some((*c, Vec::new()));
                return Done::Other;
            }
            (Pending::Play, Input::Char(c @ ('a'..='z' | '@'))) => {
                let name = if *c == '@' { self.last_macro } else { Some(*c) };
                let times = self.take_count().min(100);
                if let Some(name) = name {
                    self.play(name, times, out);
                }
                return Done::Other;
            }
            _ => {}
        }
        let count = self.count.take();
        let n = count.unwrap_or(1);
        let Input::Char(c) = input else {
            if self.mode == Mode::Normal && input == Input::Esc {
                out.push(Action::Escape);
            }
            return Done::Other;
        };
        let by = n.min(i32::MAX as u32) as i32;
        let (action, done) = match (pending, c) {
            (Pending::G, 'g') => (Action::Move(Motion::Top, count), Done::Other),
            (Pending::G, 't') => (Action::SheetNext(n), Done::Other),
            (Pending::G, 'T') => (Action::SheetPrevious(n), Done::Other),
            (Pending::G, 'l') => (Action::JumpBack, Done::Other),
            (Pending::G, 'o') => {
                self.goto = Some(String::new());
                self.goto_moved = false;
                return Done::Pending;
            }
            (Pending::Scroll, 'z' | '.') => (Action::Scroll(Scroll::Middle), Done::Other),
            (Pending::Scroll, 't') => (Action::Scroll(Scroll::Top), Done::Other),
            (Pending::Scroll, 'b' | '-') => (Action::Scroll(Scroll::Bottom), Done::Other),
            (Pending::Scroll, 'h') => (Action::ScrollCols(-by), Done::Other),
            (Pending::Scroll, 'l') => (Action::ScrollCols(by), Done::Other),
            (Pending::Mark, 'a'..='z') => (Action::SetMark(c), Done::Other),
            (Pending::Jump, 'a'..='z') => (Action::JumpMark(c), Done::Other),
            (Pending::Delete, 'd' | 'r') => (Action::DeleteRows(n), Done::Change),
            (Pending::Delete, 'c') => (Action::DeleteCols(n), Done::Change),
            (Pending::Yank, 'y' | 'r') => (Action::YankRows(n), Done::Other),
            (Pending::Yank, 'c') => (Action::YankCols(n), Done::Other),
            (Pending::Change, 'c') => (Action::Edit(Edit::Substitute, self.repeat_text()), Done::Change),
            (Pending::Indent, '>') => (Action::Indent(by), Done::Change),
            (Pending::Outdent, '<') => (Action::Indent(-by), Done::Change),
            (Pending::Plus, 'r' | 'c') => (Action::InsertLines { cols: c == 'c', count: n }, Done::Change),
            (Pending::Paste, 'v') => (Action::PasteSpecial(Paste::Values), Done::Change),
            (Pending::Paste, 'f') => (Action::PasteSpecial(Paste::Formats), Done::Change),
            (Pending::Paste, 't') => (Action::PasteSpecial(Paste::Transposed), Done::Change),
            (Pending::Paste, 'c') => (Action::PasteSpecial(Paste::Formulas), Done::Change),
            (Pending::Hide, 'r' | 'c') => (Action::Hide { cols: c == 'c' }, Done::Change),
            (Pending::Show, 'r' | 'c') => (Action::Unhide { cols: c == 'c' }, Done::Change),
            (Pending::Size, 'h' | 'l') => (Action::Resize { cols: true, by: if c == 'l' { by } else { -by } }, Done::Change),
            (Pending::Size, 'j' | 'k') => (Action::Resize { cols: false, by: if c == 'j' { by } else { -by } }, Done::Change),
            _ => return Done::Other,
        };
        let ends_visual = matches!(action, Action::Hide { .. } | Action::Unhide { .. } | Action::Resize { .. });
        out.push(action);
        if ends_visual && matches!(self.mode, Mode::Visual(_)) {
            self.leave_visual(out);
        }
        done
    }

    fn normal(&mut self, input: Input, out: &mut Vec<Action>) -> Done {
        if let Some(pending) = self.pending.take() {
            return self.prefixed(pending, input, out);
        }
        if input == Input::Esc {
            self.count = None;
            out.push(Action::Escape);
            return Done::Other;
        }
        if let Some(done) = self.common(&input, out) {
            return done;
        }
        let edit = |kind| Action::Edit(kind, None);
        let (action, done) = match input {
            Input::Char('i' | 'I') => (edit(Edit::Insert), Done::Change),
            Input::Char('a' | 'A') | Input::Enter => (edit(Edit::Append), Done::Change),
            Input::Char('s') => (edit(Edit::Substitute), Done::Change),
            Input::Char('o') => (edit(Edit::OpenBelow), Done::Change),
            Input::Char('O') => (edit(Edit::OpenAbove), Done::Change),
            Input::Char('x') => (Action::Clear(self.take_count()), Done::Change),
            Input::Char('Y') => (Action::YankRows(self.take_count()), Done::Other),
            Input::Char('p') => (Action::Put { before: false, count: self.take_count() }, Done::Change),
            Input::Char('{') => (Action::Align(Align::Left), Done::Change),
            Input::Char('|') => (Action::Align(Align::Center), Done::Change),
            Input::Char('}') => (Action::Align(Align::Right), Done::Change),
            Input::Char('u') => (Action::Undo(self.take_count()), Done::Other),
            Input::Ctrl('r') => (Action::Redo(self.take_count()), Done::Other),
            Input::Char('n') => (Action::SearchNext { forward: !self.last_search_back, count: self.take_count() }, Done::Other),
            Input::Char('N') => (Action::SearchNext { forward: self.last_search_back, count: self.take_count() }, Done::Other),
            Input::Char(c @ ('/' | '?')) => {
                self.count = None;
                self.search = Some(String::new());
                self.search_back = c == '?';
                return Done::Pending;
            }
            Input::Ctrl('a') => (Action::Increment(i64::from(self.take_count())), Done::Change),
            Input::Ctrl('x') => (Action::Increment(-i64::from(self.take_count())), Done::Change),
            Input::Char(':') => {
                self.count = None;
                return self.palette(out);
            }
            Input::Char('q') if self.recording.is_some() => {
                self.count = None;
                if let Some((name, mut keys)) = self.recording.take() {
                    // The `q` that stops recording is not part of the macro.
                    keys.pop();
                    self.macros.insert(name, keys);
                }
                return Done::Other;
            }
            Input::Char('q') => {
                self.pending = Some(Pending::Record);
                return Done::Pending;
            }
            Input::Char('@') => {
                self.pending = Some(Pending::Play);
                return Done::Pending;
            }
            Input::Char('.') => {
                let count = self.take_count();
                self.repeat(count, out);
                return Done::Other;
            }
            Input::Char(c @ ('d' | 'y' | 'c' | '>' | '<' | '+' | 'P')) => {
                // The count stays for the second key.
                self.pending = Some(match c {
                    'd' => Pending::Delete,
                    'y' => Pending::Yank,
                    'c' => Pending::Change,
                    '>' => Pending::Indent,
                    '<' => Pending::Outdent,
                    '+' => Pending::Plus,
                    _ => Pending::Paste,
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
        if let Some(pending) = self.pending.take() {
            let done = self.prefixed(pending, input, out);
            // Still selecting: keep recording so `.` repeats the whole selection.
            return if matches!(self.mode, Mode::Visual(_)) { Done::Pending } else { done };
        }
        if let Some(done) = self.common(&input, out) {
            // Motions extend the selection; keep recording so `.` reselects the same shape.
            return if done == Done::Other && self.pending.is_none() { Done::Pending } else { done };
        }
        let op = match input {
            Input::Char('y' | 'Y') => Op::Yank,
            Input::Char('d' | 'x' | 'D' | 'X') => Op::Delete,
            Input::Char('c' | 's' | 'C') => Op::Change,
            Input::Char('>') => Op::Indent,
            Input::Char('<') => Op::Outdent,
            Input::Char('o') => {
                out.push(Action::VisualSwap);
                return Done::Pending;
            }
            Input::Char(':') => return self.palette(out),
            Input::Char(c @ ('{' | '|' | '}')) => {
                out.push(Action::Align(match c {
                    '{' => Align::Left,
                    '|' => Align::Center,
                    _ => Align::Right,
                }));
                self.leave_visual(out);
                return Done::Change;
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

    /// `:` keeps the selection (commands run on it) but leaves visual mode.
    fn palette(&mut self, out: &mut Vec<Action>) -> Done {
        self.mode = Mode::Normal;
        self.pending = None;
        out.push(Action::Palette);
        Done::Other
    }

    fn play(&mut self, name: char, times: u32, out: &mut Vec<Action>) {
        let Some(keys) = self.macros.get(&name).cloned() else { return };
        if self.macro_depth >= 10 {
            return;
        }
        self.last_macro = Some(name);
        self.macro_depth += 1;
        let saved = self.replaying.take();
        for _ in 0..times.max(1) {
            for (i, key) in keys.iter().enumerate() {
                if matches!(key, Input::Typed(_)) {
                    continue;
                }
                // An edit recorded with its text writes that text instead of opening the editor.
                let text = match keys.get(i + 1) {
                    Some(Input::Typed(t)) => Some(t.clone()),
                    _ => None,
                };
                self.replaying = Some(text);
                self.mapped(key.clone(), out);
            }
        }
        self.replaying = saved;
        self.macro_depth -= 1;
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
            // A plain `P` was recorded without the key that ended it.
            if self.pending == Some(Pending::Paste) {
                self.pending = None;
                out.push(Action::Put { before: true, count: self.take_count() });
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
                        Key::F => 'f',
                        Key::B => 'b',
                        Key::A => 'a',
                        Key::X => 'x',
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
    if let Some(ed) = &app.editor {
        return if ed.vim.is_normal() { "CELL" } else { "INSERT" };
    }
    match app.vim.mode() {
        Mode::Normal => "NORMAL",
        Mode::Visual(Visual::Cells) => "VISUAL",
        Mode::Visual(Visual::Rows) => "V-ROW",
        Mode::Visual(Visual::Cols) => "V-COLUMN",
    }
}

/// Status bar keys: the open prompt, or the count and prefix typed so far.
pub fn status_keys(app: &SheetApp) -> String {
    if let Some(ed) = &app.editor {
        return if ed.vim.is_normal() { ed.vim.pending_keys() } else { String::new() };
    }
    if let Some(query) = app.vim.search() {
        return format!("{}{query}", if app.vim.search_back { '?' } else { '/' });
    }
    let keys = app.vim.pending_keys();
    match app.vim.recording() {
        Some(r) if keys.is_empty() => format!("recording @{r}"),
        Some(r) => format!("recording @{r}  {keys}"),
        None => keys,
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
    if app.vim.maps_source != app.ui.vim_maps {
        let maps = app.ui.vim_maps.clone();
        app.vim.set_maps(&maps);
    }
    let mut inputs = take_inputs(ctx).into_iter();
    while let Some(input) = inputs.next() {
        let actions = app.vim.press(input);
        for action in actions {
            run(app, ctx, action);
        }
        // Keys typed after `:` in the same frame belong to the palette.
        if let Some(p) = app.palette.as_mut() {
            p.query.extend(inputs.by_ref().filter_map(|i| if let Input::Char(c) = i { Some(c) } else { None }));
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
    let uses_register = matches!(
        action,
        Action::Clear(_)
            | Action::DeleteRows(_)
            | Action::DeleteCols(_)
            | Action::YankRows(_)
            | Action::YankCols(_)
            | Action::Put { .. }
            | Action::PasteSpecial(_)
            | Action::VisualOp(_)
            | Action::Escape
    );
    act(app, ctx, action);
    if uses_register {
        app.vim.next_register = None;
    }
}

fn act(app: &mut SheetApp, ctx: &Context, action: Action) {
    match action {
        Action::Move(motion, count) => {
            if matches!(motion, Motion::Top | Motion::Bottom) {
                remember_jump(app);
            }
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
        Action::DeleteCols(count) => {
            let a = active(app);
            let range = cols_range(app, a.col, count);
            yank(app, ctx, range, Visual::Cols);
            app.run_or_alert("home.deleteColumns", json!({"cols": cols_a1(a.col, range.end.col)}));
            select_cell(app, a);
        }
        Action::YankCols(count) => {
            let range = cols_range(app, active(app).col, count);
            yank(app, ctx, range, Visual::Cols);
        }
        Action::InsertLines { cols, count } => {
            let a = active(app);
            let n = count.clamp(1, 1000) - 1;
            if cols {
                app.run_or_alert("home.insertColumns", json!({"cols": cols_a1(a.col, (a.col + n).min(MAX_COLS - 1))}));
            } else {
                app.run_or_alert("home.insertRows", json!({"rows": lines_a1(a.row, (a.row + n).min(MAX_ROWS - 1))}));
            }
            select_cell(app, a);
        }
        Action::Put { before, count } => put(app, before, count),
        Action::PasteSpecial(kind) => paste_special(app, kind),
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
        Action::Search { what, forward } => {
            remember_jump(app);
            if forward {
                find(app, json!({"what": what}));
            } else {
                find_previous(app, &json!({"what": what}));
            }
        }
        Action::SearchNext { forward, count } => {
            let Some(last) = app.session.last_find.clone() else {
                app.toast = Some(("No previous search".into(), crate::now_ms()));
                return;
            };
            remember_jump(app);
            for _ in 0..count.min(1000) {
                if forward {
                    find(app, last.clone());
                } else {
                    find_previous(app, &last);
                }
            }
        }
        Action::GoTo { cell, first } => {
            if first {
                remember_jump(app);
            }
            select_cell(app, cell);
            if let Some(kind) = app.vim.shape {
                select_visual(app, kind);
            }
            app.grid.ensure_visible = true;
        }
        Action::SetMark(name) => {
            if let Some(d) = app.session.active() {
                let mark = MarkPos { sheet: d.wb.active_sheet, range: d.selection.current(), active: d.selection.active };
                app.vim.marks.insert(name, mark);
            }
        }
        Action::JumpMark(name) => {
            let Some(mark) = app.vim.marks.get(&name).copied() else {
                app.toast = Some((format!("Mark '{name}' is not set"), crate::now_ms()));
                return;
            };
            remember_jump(app);
            if !go_to_sheet(app, mark.sheet) {
                return;
            }
            if let Some(kind) = app.vim.shape {
                select_cell(app, mark.active);
                select_visual(app, kind);
            } else if mark.range.is_single() {
                select_cell(app, mark.active);
            } else {
                let _ = app.session.run("selection.set", json!({"range": mark.range.a1(), "active": mark.active.a1()}));
            }
            app.grid.ensure_visible = true;
        }
        Action::JumpBack => {
            let Some((sheet, cell)) = app.vim.last_jump else {
                app.toast = Some(("No previous position".into(), crate::now_ms()));
                return;
            };
            remember_jump(app);
            if go_to_sheet(app, sheet) {
                select_cell(app, cell);
                app.grid.ensure_visible = true;
            }
        }
        Action::Scroll(to) => {
            app.grid.scroll_to = Some(match to {
                Scroll::Top => 0.0,
                Scroll::Middle => 0.5,
                Scroll::Bottom => 1.0,
            });
        }
        Action::Align(to) => {
            let id = match to {
                Align::Left => "home.alignLeft",
                Align::Center => "home.alignCenter",
                Align::Right => "home.alignRight",
            };
            // `on` keeps a second press from switching back to General.
            app.run_or_alert(id, json!({"on": true}));
        }
        Action::Resize { cols, by } => resize(app, cols, by),
        Action::Hide { cols } => app.run_or_alert(if cols { "home.hideColumns" } else { "home.hideRows" }, json!({})),
        Action::Unhide { cols } => unhide(app, cols),
        Action::ScrollCols(by) => app.grid.scroll_cols = Some(by),
        Action::Increment(by) => increment(app, by),
        Action::Register(name) => app.vim.next_register = Some(name),
        Action::Palette => {
            app.vim.anchor = None;
            app.vim.shape = None;
            app.palette = Some(crate::palette::Palette::new(true));
        }
        Action::Ex(line) => match crate::palette::parse_ex(&line) {
            Some(ex) => crate::palette::run_ex(app, ex),
            None => app.toast = Some((format!("Not a command: {line}"), crate::now_ms())),
        },
        Action::Escape => {
            app.session.clipboard = None;
            app.session.format_painter = None;
        }
    }
}

/// Column width and row height steps for `fh` / `fl` / `fj` / `fk`, in points.
const WIDTH_STEP: f32 = 8.0;
const HEIGHT_STEP: f32 = 4.0;

/// Resizes the selected columns or rows from the active cell's size.
fn resize(app: &mut SheetApp, cols: bool, by: i32) {
    let a = active(app);
    let Some(sh) = app.session.active().and_then(|d| d.wb.active()) else { return };
    let steps = by.clamp(-1000, 1000) as f32;
    if cols {
        let width = (sh.col_width(a.col) + steps * WIDTH_STEP).clamp(WIDTH_STEP, 1800.0);
        app.run_or_alert("home.columnWidth", json!({"width": width}));
    } else {
        let height = (sh.row_height(a.row) + steps * HEIGHT_STEP).clamp(HEIGHT_STEP, 409.0);
        app.run_or_alert("home.rowHeight", json!({"height": height}));
    }
}

/// Saves the sheet and cell for `gl` before a jump.
fn remember_jump(app: &mut SheetApp) {
    if let Some(d) = app.session.active() {
        app.vim.last_jump = Some((d.wb.active_sheet, d.selection.active));
    }
}

/// Activates sheet `index` if it is not the active one; false (with a toast) if it is gone.
fn go_to_sheet(app: &mut SheetApp, index: usize) -> bool {
    let Some(d) = app.session.active() else { return false };
    if d.wb.active_sheet == index {
        return true;
    }
    if app.run("sheet.activate", json!({"sheet": index})).is_err() {
        app.toast = Some(("That sheet no longer exists".into(), crate::now_ms()));
        return false;
    }
    true
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
        Motion::PageDown | Motion::PageUp => {
            let page = (bottom.saturating_sub(top).max(1)) as i64;
            let down = motion == Motion::PageDown;
            step(app, if down { page * n } else { -page * n }, 0, false);
            // As in vim: the new page starts at the cursor (Ctrl-f) or ends at it (Ctrl-b).
            app.grid.scroll_to = Some(if down { 0.0 } else { 1.0 });
        }
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

/// `count` columns from `col`, limited to the used rows.
fn cols_range(app: &SheetApp, col: u32, count: u32) -> RangeRef {
    let end = col.saturating_add(count.saturating_sub(1)).min(MAX_COLS - 1);
    RangeRef::new(CellRef::new(0, col), CellRef::new(used_end(app).row, end))
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
                let register = Register { clip, kind };
                if let Some(name) = app.vim.next_register.filter(|n| *n != '"') {
                    app.vim.registers.insert(name, register.clone());
                }
                app.vim.registers.insert('"', register);
            }
        }
        Err(e) => app.message = Some(("GridCraft".into(), crate::clean_error(&e))),
    }
}

fn put(app: &mut SheetApp, before: bool, count: u32) {
    let Some(reg) = register(app) else { return };
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

/// `Pv` / `Pf` / `Pt` / `Pc`: Paste Special from vim's register at the cursor (whole lines at
/// the start of the cursor's row or column).
fn paste_special(app: &mut SheetApp, kind: Paste) {
    let Some(reg) = register(app) else { return };
    let a = active(app);
    let at = match reg.kind {
        Visual::Cells => a,
        Visual::Rows => CellRef::new(a.row, 0),
        Visual::Cols => CellRef::new(0, a.col),
    };
    let (what, transpose) = match kind {
        Paste::Values => ("values", false),
        Paste::Formats => ("formats", false),
        Paste::Transposed => ("all", true),
        Paste::Formulas => ("formulas", false),
    };
    restore(app, &reg);
    app.run_or_alert("edit.pasteSpecial", json!({"what": what, "transpose": transpose, "at": at.a1()}));
    app.session.clipboard = None;
    app.grid.ensure_visible = true;
}

/// The register the next put reads (`"a`, else the unnamed one); a toast when it is empty.
fn register(app: &mut SheetApp) -> Option<Register> {
    let name = app.vim.next_register.unwrap_or('"');
    let reg = app.vim.registers.get(&name).cloned();
    if reg.is_none() {
        let msg = if name == '"' { "Nothing to put".to_string() } else { format!("Register {name} is empty") };
        app.toast = Some((msg, crate::now_ms()));
    }
    reg
}

/// `Ctrl-a` / `Ctrl-x`: a number (or date) cell changes by `by`; in text, the first number.
fn increment(app: &mut SheetApp, by: i64) {
    let a = active(app);
    let Some(sh) = app.session.active().and_then(|d| d.wb.active()) else { return };
    let input = sh.cell(a).map(|c| c.input_text()).unwrap_or_default();
    if input.starts_with('=') {
        app.toast = Some(("Ctrl-a and Ctrl-x change numbers, not formulas".into(), crate::now_ms()));
        return;
    }
    if let gridcraft_engine::core::Value::Number(n) = sh.value(a) {
        let value = n + by as f64;
        app.run_or_alert("range.setValues", json!({"range": a.a1(), "values": [[value]]}));
    } else if let Some(text) = bump_number(&input, by) {
        app.run_or_alert("cell.set", json!({"cell": a.a1(), "input": text}));
    } else {
        app.toast = Some(("No number in this cell".into(), crate::now_ms()));
    }
}

/// `text` with its first whole number changed by `by` (a `-` right before it counts).
pub fn bump_number(text: &str, by: i64) -> Option<String> {
    let start = text.find(|c: char| c.is_ascii_digit())?;
    let end = text[start..].find(|c: char| !c.is_ascii_digit()).map_or(text.len(), |e| start + e);
    let negative = text[..start].ends_with('-') && text[..start.saturating_sub(1)].chars().last().is_none_or(|c| !c.is_alphanumeric());
    let from = if negative { start - 1 } else { start };
    let n: i64 = text[start..end].parse().ok()?;
    let n = if negative { -n } else { n };
    Some(format!("{}{}{}", &text[..from], n.saturating_add(by), &text[end..]))
}

/// `Sr` / `Sc`: unhide the hidden rows or columns next to the cursor (in the selection in
/// visual mode).
fn unhide(app: &mut SheetApp, cols: bool) {
    let id = if cols { "home.unhideColumns" } else { "home.unhideRows" };
    if app.vim.shape.is_some() {
        app.run_or_alert(id, json!({}));
        return;
    }
    let a = active(app);
    let Some(sh) = app.session.active().and_then(|d| d.wb.active()) else { return };
    let hidden = |i: u32| if cols { sh.is_col_hidden(i) } else { sh.is_row_hidden(i) };
    let (at, max) = if cols { (a.col, MAX_COLS - 1) } else { (a.row, MAX_ROWS - 1) };
    let (mut first, mut last) = (at, at);
    while first > 0 && hidden(first - 1) && at - first < 100_000 {
        first -= 1;
    }
    while last < max && hidden(last + 1) && last - at < 100_000 {
        last += 1;
    }
    if first == at && last == at && !hidden(at) {
        let what = if cols { "columns" } else { "rows" };
        app.toast = Some((format!("No hidden {what} next to the cursor"), crate::now_ms()));
        return;
    }
    if cols {
        app.run_or_alert(id, json!({"cols": cols_a1(first, last)}));
    } else {
        app.run_or_alert(id, json!({"rows": lines_a1(first, last)}));
    }
}

/// Moves the cursor to `cell` as a jump (`gl` comes back).
pub(crate) fn jump_to(app: &mut SheetApp, cell: CellRef) {
    remember_jump(app);
    select_cell(app, cell);
    app.grid.ensure_visible = true;
}

/// The vim keys for a command, for the palette: built in, and mappings to `:<command><CR>`.
pub fn keys_for(id: &str, maps: &std::collections::BTreeMap<String, String>) -> Option<String> {
    let builtin = match id {
        "edit.undo" => "u",
        "edit.redo" => "Ctrl-r",
        "edit.copy" => "y",
        "edit.paste" => "p",
        "edit.pasteSpecial" => "Pv Pf Pc Pt",
        "edit.clearContents" => "x",
        "edit.find" => "/ ?",
        "edit.goTo" => "go",
        "home.alignLeft" => "{",
        "home.alignCenter" => "|",
        "home.alignRight" => "}",
        "home.increaseIndent" => ">>",
        "home.decreaseIndent" => "<<",
        "home.insertRows" => "+r",
        "home.insertColumns" => "+c",
        "home.deleteRows" => "dr",
        "home.deleteColumns" => "dc",
        "home.hideRows" => "Zr",
        "home.hideColumns" => "Zc",
        "home.unhideRows" => "Sr",
        "home.unhideColumns" => "Sc",
        "home.columnWidth" => "fh fl",
        "home.rowHeight" => "fk fj",
        "sheet.next" => "gt",
        "sheet.previous" => "gT",
        "file.save" => ":w",
        "file.close" => ":q",
        "file.open" => ":e",
        "data.sortAscending" => ":sort",
        "data.sortDescending" => ":sort!",
        "edit.fillDown" => ":fill",
        "edit.fillRight" => ":fill right",
        "view.freezePanes" => ":freeze",
        "view.unfreezePanes" => ":unfreeze",
        _ => "",
    };
    let mapped = maps.iter().filter(|(_, rhs)| parse_rhs(rhs) == Some(Rhs::Ex(id.to_string()))).map(|(lhs, _)| lhs.as_str());
    let all: Vec<&str> = mapped.chain((!builtin.is_empty()).then_some(builtin)).collect();
    (!all.is_empty()).then(|| all.join(" "))
}

/// Keys in vim notation: plain characters, `<CR>` `<Esc>` `<BS>` `<Space>` `<Bar>` `<lt>` and
/// `<C-x>`. An unknown `<…>` is taken literally.
pub fn parse_keys(text: &str) -> Vec<Input> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        if c == '<'
            && let Some(end) = rest.find('>')
            && let Some(key) = named_key(&rest[1..end])
        {
            out.push(key);
            rest = &rest[end + 1..];
            continue;
        }
        out.push(Input::Char(c));
        rest = &rest[c.len_utf8()..];
    }
    out
}

fn named_key(name: &str) -> Option<Input> {
    let lower = name.to_ascii_lowercase();
    Some(match lower.as_str() {
        "cr" | "enter" | "return" => Input::Enter,
        "esc" => Input::Esc,
        "bs" => Input::Backspace,
        "space" => Input::Char(' '),
        "bar" => Input::Char('|'),
        "lt" => Input::Char('<'),
        _ => {
            let c = lower.strip_prefix("c-")?;
            let mut chars = c.chars();
            let (Some(c), None) = (chars.next(), chars.next()) else { return None };
            Input::Ctrl(c)
        }
    })
}

fn parse_rhs(text: &str) -> Option<Rhs> {
    let lower = text.to_ascii_lowercase();
    if let Some(line) = text.strip_prefix(':')
        && (lower.ends_with("<cr>") || lower.ends_with("<enter>"))
        && let Some(end) = line.rfind('<')
    {
        return Some(Rhs::Ex(line[..end].trim().to_string()));
    }
    let keys = parse_keys(text);
    (!keys.is_empty()).then_some(Rhs::Keys(keys))
}

/// How a key looks in the status bar.
fn key_name(input: &Input) -> String {
    match input {
        Input::Char(' ') => "<Space>".into(),
        Input::Char(c) => c.to_string(),
        Input::Esc => "<Esc>".into(),
        Input::Enter => "<CR>".into(),
        Input::Backspace => "<BS>".into(),
        Input::Ctrl(c) => format!("<C-{c}>"),
        Input::Typed(_) => String::new(),
    }
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
        assert_eq!(vim.feed(Input::Enter), vec![Action::Search { what: "a".into(), forward: true }]);
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
        assert_eq!(feed(&mut vim, "5Q"), vec![]);
        assert_eq!(feed(&mut vim, "j"), vec![Action::Move(Motion::Down, None)]);
    }

    fn press(vim: &mut Vim, keys: &str) -> Vec<Action> {
        parse_keys(keys).into_iter().flat_map(|k| vim.press(k)).collect()
    }

    #[test]
    fn backward_search_flips_n() {
        let mut vim = Vim::default();
        feed(&mut vim, "?ab");
        assert_eq!(vim.feed(Input::Enter), vec![Action::Search { what: "ab".into(), forward: false }]);
        assert_eq!(feed(&mut vim, "n"), vec![Action::SearchNext { forward: false, count: 1 }]);
        assert_eq!(feed(&mut vim, "N"), vec![Action::SearchNext { forward: true, count: 1 }]);
    }

    #[test]
    fn named_registers_prefix_the_command() {
        let mut vim = Vim::default();
        assert_eq!(feed(&mut vim, "\"a"), vec![Action::Register('a')]);
        assert_eq!(vim.pending_keys(), "");
        assert_eq!(feed(&mut vim, "2yy"), vec![Action::YankRows(2)]);
        assert_eq!(feed(&mut vim, "\"b3"), vec![Action::Register('b')]);
        assert_eq!(feed(&mut vim, "dr"), vec![Action::DeleteRows(3)]);
        assert_eq!(feed(&mut vim, "\"1j"), vec![Action::Move(Motion::Down, None)]);
    }

    #[test]
    fn macros_record_and_replay_keys_and_typed_text() {
        let mut vim = Vim::default();
        assert_eq!(press(&mut vim, "qa"), vec![]);
        assert_eq!(vim.recording(), Some('a'));
        press(&mut vim, "jo");
        vim.inserted("hi".into());
        assert_eq!(press(&mut vim, "q"), vec![]);
        assert_eq!(vim.recording(), None);
        let once = vec![Action::Move(Motion::Down, None), Action::Edit(Edit::OpenBelow, Some("hi".into()))];
        assert_eq!(press(&mut vim, "@a"), once);
        assert_eq!(press(&mut vim, "2@@"), [once.clone(), once].concat());
        assert_eq!(press(&mut vim, "@z"), vec![]);
    }

    #[test]
    fn a_macro_that_plays_itself_stops() {
        let mut vim = Vim::default();
        press(&mut vim, "qaj@aq");
        let out = press(&mut vim, "@a");
        assert!(!out.is_empty() && out.len() <= 20, "{out:?}");
    }

    #[test]
    fn mappings_wait_for_longer_matches_and_replay_the_rest() {
        let mut vim = Vim::default();
        let maps = [("J", "5j"), ("gx", ":edit.copy<CR>"), ("gxx", "yy"), ("<C-s>", ":w<CR>")]
            .into_iter()
            .map(|(l, r)| (l.to_string(), r.to_string()))
            .collect();
        vim.set_maps(&maps);
        assert_eq!(press(&mut vim, "J"), vec![Action::Move(Motion::Down, Some(5))]);
        assert_eq!(press(&mut vim, "gx"), vec![]);
        assert_eq!(press(&mut vim, "x"), vec![Action::YankRows(1)]);
        assert_eq!(press(&mut vim, "gx"), vec![]);
        assert_eq!(press(&mut vim, "j"), vec![Action::Ex("edit.copy".into()), Action::Move(Motion::Down, None)]);
        assert_eq!(press(&mut vim, "<C-s>"), vec![Action::Ex("w".into())]);
        assert_eq!(press(&mut vim, "gt"), feed(&mut Vim::default(), "gt"));
        // A mapped key in the middle of a command is the plain key.
        assert_eq!(press(&mut vim, "dJ"), vec![]);
    }

    #[test]
    fn key_notation() {
        assert_eq!(
            parse_keys("a<CR><esc><C-x><lt><Space><Nope>"),
            vec![
                Input::Char('a'),
                Input::Enter,
                Input::Esc,
                Input::Ctrl('x'),
                Input::Char('<'),
                Input::Char(' '),
                Input::Char('<'),
                Input::Char('N'),
                Input::Char('o'),
                Input::Char('p'),
                Input::Char('e'),
                Input::Char('>'),
            ]
        );
        assert_eq!(parse_rhs(":sort <CR>"), Some(Rhs::Ex("sort".into())));
        assert_eq!(parse_rhs("dd"), Some(Rhs::Keys(vec![Input::Char('d'), Input::Char('d')])));
        assert_eq!(parse_rhs(""), None);
        let maps = [("<C-c>".to_string(), ":edit.copy<CR>".to_string())].into_iter().collect();
        assert_eq!(keys_for("edit.copy", &maps).as_deref(), Some("<C-c> y"));
        assert_eq!(keys_for("file.print", &maps), None);
    }

    #[test]
    fn scroll_increment_unhide_and_command_line_keys() {
        let mut vim = Vim::default();
        assert_eq!(vim.feed(Input::Ctrl('f')), vec![Action::Move(Motion::PageDown, None)]);
        assert_eq!(vim.feed(Input::Ctrl('b')), vec![Action::Move(Motion::PageUp, None)]);
        assert_eq!(feed(&mut vim, "3zl"), vec![Action::ScrollCols(3)]);
        assert_eq!(feed(&mut vim, "zh"), vec![Action::ScrollCols(-1)]);
        feed(&mut vim, "5");
        assert_eq!(vim.feed(Input::Ctrl('a')), vec![Action::Increment(5)]);
        assert_eq!(vim.feed(Input::Ctrl('x')), vec![Action::Increment(-1)]);
        assert_eq!(feed(&mut vim, "."), vec![Action::Increment(-1)]);
        assert_eq!(feed(&mut vim, "Sr"), vec![Action::Unhide { cols: false }]);
        assert_eq!(feed(&mut vim, "vSc"), vec![Action::Visual(Some(Visual::Cells)), Action::Unhide { cols: true }, Action::Visual(None)]);
        assert_eq!(feed(&mut vim, ":"), vec![Action::Palette]);
        assert_eq!(feed(&mut vim, "v:"), vec![Action::Visual(Some(Visual::Cells)), Action::Palette]);
        assert_eq!(vim.mode(), Mode::Normal);
    }

    #[test]
    fn bump_number_changes_the_first_number() {
        assert_eq!(bump_number("item 9 of 12", 1).as_deref(), Some("item 10 of 12"));
        assert_eq!(bump_number("x-3", 5).as_deref(), Some("x-8"));
        assert_eq!(bump_number(" -3", 5).as_deref(), Some(" 2"));
        assert_eq!(bump_number("v2", -5).as_deref(), Some("v-3"));
        assert_eq!(bump_number("none", 1), None);
    }

    #[test]
    fn row_and_column_operators() {
        let mut vim = Vim::default();
        assert_eq!(feed(&mut vim, "dr"), vec![Action::DeleteRows(1)]);
        assert_eq!(feed(&mut vim, "2dc"), vec![Action::DeleteCols(2)]);
        assert_eq!(feed(&mut vim, "yr"), vec![Action::YankRows(1)]);
        assert_eq!(feed(&mut vim, "3yc"), vec![Action::YankCols(3)]);
        assert_eq!(feed(&mut vim, "+r"), vec![Action::InsertLines { cols: false, count: 1 }]);
        assert_eq!(feed(&mut vim, "2+c"), vec![Action::InsertLines { cols: true, count: 2 }]);
        assert_eq!(feed(&mut vim, "+"), vec![]);
        assert_eq!(vim.pending_keys(), "+");
        assert_eq!(feed(&mut vim, "x"), vec![]);
        assert_eq!(feed(&mut vim, "."), vec![Action::InsertLines { cols: true, count: 2 }]);
    }

    #[test]
    fn typed_paste_and_plain_p() {
        let mut vim = Vim::default();
        assert_eq!(feed(&mut vim, "Pv"), vec![Action::PasteSpecial(Paste::Values)]);
        assert_eq!(feed(&mut vim, "Pf"), vec![Action::PasteSpecial(Paste::Formats)]);
        assert_eq!(feed(&mut vim, "Pt"), vec![Action::PasteSpecial(Paste::Transposed)]);
        assert_eq!(feed(&mut vim, "Pc"), vec![Action::PasteSpecial(Paste::Formulas)]);
        // Any other key: a plain `P`, then the key itself.
        assert_eq!(feed(&mut vim, "2Pj"), vec![Action::Put { before: true, count: 2 }, Action::Move(Motion::Down, None)]);
        assert_eq!(feed(&mut vim, "."), vec![Action::Put { before: true, count: 2 }]);
        assert_eq!(feed(&mut vim, "Px"), vec![Action::Put { before: true, count: 1 }, Action::Clear(1)]);
        assert_eq!(feed(&mut vim, "."), vec![Action::Clear(1)]);
        assert_eq!(feed(&mut vim, "P"), vec![]);
        assert_eq!(vim.feed(Input::Esc), vec![Action::Put { before: true, count: 1 }, Action::Escape]);
    }

    #[test]
    fn go_to_an_address() {
        let mut vim = Vim::default();
        assert_eq!(feed(&mut vim, "go"), vec![]);
        assert_eq!(vim.pending_keys(), "go");
        assert_eq!(feed(&mut vim, "a"), vec![]);
        assert_eq!(feed(&mut vim, "b2"), vec![Action::GoTo { cell: CellRef::new(1, 27), first: true }]);
        assert_eq!(vim.pending_keys(), "goAB2");
        assert_eq!(feed(&mut vim, "4"), vec![Action::GoTo { cell: CellRef::new(23, 27), first: false }]);
        // A letter after the digits ends the address and does its own job.
        assert_eq!(feed(&mut vim, "j"), vec![Action::Move(Motion::Down, None)]);
        assert_eq!(vim.pending_keys(), "");
        feed(&mut vim, "goc3");
        assert_eq!(vim.feed(Input::Enter), vec![]);
        assert_eq!(vim.pending_keys(), "");
        feed(&mut vim, "gox");
        vim.feed(Input::Backspace);
        assert_eq!(feed(&mut vim, "d1"), vec![Action::GoTo { cell: CellRef::new(0, 3), first: true }]);
        vim.feed(Input::Esc);
        assert_eq!(feed(&mut vim, "go5"), vec![]);
        assert_eq!(vim.pending_keys(), "5");
    }

    #[test]
    fn marks_scrolling_and_jump_back() {
        let mut vim = Vim::default();
        assert_eq!(feed(&mut vim, "ma"), vec![Action::SetMark('a')]);
        assert_eq!(feed(&mut vim, "'a"), vec![Action::JumpMark('a')]);
        assert_eq!(feed(&mut vim, "`b"), vec![Action::JumpMark('b')]);
        assert_eq!(feed(&mut vim, "m1"), vec![]);
        assert_eq!(feed(&mut vim, "zz"), vec![Action::Scroll(Scroll::Middle)]);
        assert_eq!(feed(&mut vim, "zt"), vec![Action::Scroll(Scroll::Top)]);
        assert_eq!(feed(&mut vim, "zb"), vec![Action::Scroll(Scroll::Bottom)]);
        assert_eq!(feed(&mut vim, "gl"), vec![Action::JumpBack]);
        // In visual mode they keep the selection going.
        feed(&mut vim, "v");
        assert_eq!(feed(&mut vim, "mbzz'a"), vec![Action::SetMark('b'), Action::Scroll(Scroll::Middle), Action::JumpMark('a')]);
        assert_eq!(vim.mode(), Mode::Visual(Visual::Cells));
    }

    #[test]
    fn formatting_keys() {
        let mut vim = Vim::default();
        assert_eq!(feed(&mut vim, "{"), vec![Action::Align(Align::Left)]);
        assert_eq!(feed(&mut vim, "|"), vec![Action::Align(Align::Center)]);
        assert_eq!(feed(&mut vim, "}"), vec![Action::Align(Align::Right)]);
        assert_eq!(feed(&mut vim, "]"), vec![Action::Move(Motion::BlockDown, None)]);
        assert_eq!(feed(&mut vim, "["), vec![Action::Move(Motion::BlockUp, None)]);
        assert_eq!(feed(&mut vim, "3fl"), vec![Action::Resize { cols: true, by: 3 }]);
        assert_eq!(feed(&mut vim, "fh"), vec![Action::Resize { cols: true, by: -1 }]);
        assert_eq!(feed(&mut vim, "fj"), vec![Action::Resize { cols: false, by: 1 }]);
        assert_eq!(feed(&mut vim, "2fk"), vec![Action::Resize { cols: false, by: -2 }]);
        assert_eq!(feed(&mut vim, "Zr"), vec![Action::Hide { cols: false }]);
        assert_eq!(feed(&mut vim, "Zc"), vec![Action::Hide { cols: true }]);
        // In visual mode they act on the selection and end it.
        assert_eq!(
            feed(&mut vim, "vj|"),
            vec![Action::Visual(Some(Visual::Cells)), Action::Move(Motion::Down, None), Action::Align(Align::Center), Action::Visual(None)]
        );
        assert_eq!(feed(&mut vim, "VZr"), vec![Action::Visual(Some(Visual::Rows)), Action::Hide { cols: false }, Action::Visual(None)]);
        assert_eq!(feed(&mut vim, "vfl"), vec![Action::Visual(Some(Visual::Cells)), Action::Resize { cols: true, by: 1 }, Action::Visual(None)]);
        assert_eq!(vim.mode(), Mode::Normal);
        // Escape after a prefix in visual mode only drops the prefix.
        feed(&mut vim, "vZ");
        assert_eq!(vim.feed(Input::Esc), vec![]);
        assert_eq!(vim.mode(), Mode::Visual(Visual::Cells));
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
