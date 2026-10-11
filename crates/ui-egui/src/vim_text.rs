//! Vim normal mode inside the cell editor (vim mode only).
//!
//! Escape while typing in a cell enters this mode instead of committing. [`TextVim::feed`] edits
//! the editor's text and caret (a char index) directly and tells the editor when to go back to
//! typing or to commit.

use crate::vim::Input;

const MAX_COUNT: u32 = 9_999;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Delete,
    Change,
    Yank,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FindSpec {
    forward: bool,
    /// `t` / `T`: stop one short of the character.
    till: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pending {
    Op(Op),
    Find(FindSpec, Option<Op>),
    /// `i` / `a` after an operator: a text object follows.
    Object(Op, bool),
    Replace,
}

/// What the editor does after a key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Continue,
    /// Back to typing at the caret.
    Insert,
    /// Commit the edit; `down` moves the cursor down like Enter.
    Commit {
        down: bool,
    },
}

#[derive(Clone, Debug, Default)]
pub struct TextVim {
    normal: bool,
    count: Option<u32>,
    op_count: Option<u32>,
    pending: Option<Pending>,
    last_find: Option<(FindSpec, char)>,
    register: String,
    undo: Vec<(String, usize)>,
}

/// A motion's target and whether an operator includes the character under it.
struct Target {
    to: usize,
    inclusive: bool,
}

impl TextVim {
    pub fn is_normal(&self) -> bool {
        self.normal
    }

    /// The count and operator typed so far.
    pub fn pending_keys(&self) -> String {
        let mut s = self.op_count.map(|c| c.to_string()).unwrap_or_default();
        let op = |op: Op| match op {
            Op::Delete => 'd',
            Op::Change => 'c',
            Op::Yank => 'y',
        };
        match self.pending {
            Some(Pending::Op(o)) => s.push(op(o)),
            Some(Pending::Object(o, inner)) => {
                s.push(op(o));
                s.push(if inner { 'i' } else { 'a' });
            }
            Some(Pending::Find(f, o)) => {
                if let Some(o) = o {
                    s.push(op(o));
                }
                s.push(match (f.forward, f.till) {
                    (true, false) => 'f',
                    (true, true) => 't',
                    (false, false) => 'F',
                    (false, true) => 'T',
                });
            }
            Some(Pending::Replace) => s.push('r'),
            None => {}
        }
        if let Some(c) = self.count {
            s.push_str(&c.to_string());
        }
        s
    }

    /// Leaves typing: the caret steps back onto the last typed character, as in vim.
    pub fn enter_normal(&mut self, text: &str, caret: &mut usize) {
        self.normal = true;
        self.count = None;
        self.op_count = None;
        self.pending = None;
        *caret = clamp_normal(text.chars().count(), caret.saturating_sub(1));
    }

    pub fn feed(&mut self, text: &mut String, caret: &mut usize, input: &Input) -> Outcome {
        if self.pending.is_none() && *input == Input::Char('u') {
            self.count = None;
            self.op_count = None;
            if let Some((old, at)) = self.undo.pop() {
                *caret = clamp_normal(old.chars().count(), at);
                *text = old;
            }
            return Outcome::Continue;
        }
        let mut chars: Vec<char> = text.chars().collect();
        let mut at = (*caret).min(chars.len());
        let before = (text.clone(), *caret);
        let outcome = self.key(&mut chars, &mut at, input);
        let new: String = chars.iter().collect();
        // Going back to typing saves the text too, so `u` takes back what is typed next.
        if (new != *text || outcome == Outcome::Insert) && self.undo.last() != Some(&before) {
            self.undo.push(before);
            if self.undo.len() > 100 {
                self.undo.remove(0);
            }
        }
        *text = new;
        *caret = if outcome == Outcome::Insert { at.min(chars.len()) } else { clamp_normal(chars.len(), at) };
        if outcome == Outcome::Insert {
            self.normal = false;
        }
        outcome
    }

    fn take_count(&mut self) -> usize {
        let n = self.op_count.take().unwrap_or(1).saturating_mul(self.count.take().unwrap_or(1));
        n.min(MAX_COUNT) as usize
    }

    fn key(&mut self, chars: &mut Vec<char>, at: &mut usize, input: &Input) -> Outcome {
        let c = match input {
            Input::Char(c) => *c,
            Input::Backspace => 'h',
            Input::Esc => {
                if self.pending.is_some() || self.count.is_some() || self.op_count.is_some() {
                    self.pending = None;
                    self.count = None;
                    self.op_count = None;
                    return Outcome::Continue;
                }
                return Outcome::Commit { down: false };
            }
            Input::Enter => return Outcome::Commit { down: true },
            Input::Ctrl('r') => {
                // Redo is the grid's job after the commit; inside a cell `u` is enough.
                return Outcome::Continue;
            }
            Input::Ctrl(_) => return Outcome::Continue,
        };
        match self.pending.take() {
            Some(Pending::Replace) => {
                let n = self.take_count();
                if *at + n <= chars.len() {
                    for ch in chars.iter_mut().skip(*at).take(n) {
                        *ch = c;
                    }
                    *at += n - 1;
                }
                return Outcome::Continue;
            }
            Some(Pending::Find(spec, op)) => {
                self.last_find = Some((spec, c));
                let n = self.take_count();
                let target = find(chars, *at, spec, c, n);
                return self.apply(chars, at, op, target);
            }
            Some(Pending::Object(op, inner)) => {
                self.count = None;
                self.op_count = None;
                if let Some((s, e)) = word_object(chars, *at, c, inner) {
                    return self.operate(chars, at, op, s, e);
                }
                return Outcome::Continue;
            }
            Some(Pending::Op(op)) => return self.after_operator(chars, at, op, c),
            None => {}
        }
        if self.digit(c) {
            return Outcome::Continue;
        }
        if let Some(target) = self.motion(chars, *at, c) {
            return self.apply(chars, at, None, Some(target));
        }
        let typed = self.count;
        let n = self.take_count();
        let len = chars.len();
        match c {
            'i' => Outcome::Insert,
            'a' => {
                *at = (*at + 1).min(len);
                Outcome::Insert
            }
            'I' => {
                *at = first_non_blank(chars);
                Outcome::Insert
            }
            'A' => {
                *at = len;
                Outcome::Insert
            }
            'x' => self.operate(chars, at, Op::Delete, *at, (*at + n).min(len)),
            'X' => {
                let s = at.saturating_sub(n);
                self.operate(chars, at, Op::Delete, s, *at)
            }
            's' => self.operate(chars, at, Op::Change, *at, (*at + n).min(len)),
            'S' => self.operate(chars, at, Op::Change, 0, len),
            'C' => self.operate(chars, at, Op::Change, *at, len),
            'D' => self.operate(chars, at, Op::Delete, *at, len),
            'Y' => self.operate(chars, at, Op::Yank, 0, len),
            'r' => {
                self.count = typed;
                self.pending = Some(Pending::Replace);
                Outcome::Continue
            }
            '~' => {
                let end = (*at + n).min(len);
                for ch in chars.iter_mut().take(end).skip(*at) {
                    *ch = toggle_case(*ch);
                }
                *at = end;
                Outcome::Continue
            }
            'p' | 'P' => {
                if self.register.is_empty() {
                    return Outcome::Continue;
                }
                let pos = if c == 'p' { (*at + 1).min(len) } else { *at };
                let put: Vec<char> = self.register.chars().cycle().take(self.register.chars().count() * n).collect();
                let added = put.len();
                chars.splice(pos..pos, put);
                *at = pos + added - 1;
                Outcome::Continue
            }
            'd' | 'c' | 'y' => {
                self.op_count = typed;
                self.pending = Some(Pending::Op(match c {
                    'd' => Op::Delete,
                    'c' => Op::Change,
                    _ => Op::Yank,
                }));
                Outcome::Continue
            }
            'f' | 't' | 'F' | 'T' => {
                self.count = typed;
                self.pending = Some(Pending::Find(FindSpec { forward: c.is_lowercase(), till: c.eq_ignore_ascii_case(&'t') }, None));
                Outcome::Continue
            }
            _ => Outcome::Continue,
        }
    }

    fn digit(&mut self, c: char) -> bool {
        let Some(d) = c.to_digit(10) else { return false };
        if d == 0 && self.count.is_none() {
            return false;
        }
        self.count = Some((self.count.unwrap_or(0).saturating_mul(10).saturating_add(d)).min(MAX_COUNT));
        true
    }

    fn after_operator(&mut self, chars: &mut Vec<char>, at: &mut usize, op: Op, c: char) -> Outcome {
        let op_key = match op {
            Op::Delete => 'd',
            Op::Change => 'c',
            Op::Yank => 'y',
        };
        if c == op_key {
            self.count = None;
            self.op_count = None;
            let len = chars.len();
            return self.operate(chars, at, op, 0, len);
        }
        if self.digit(c) {
            self.pending = Some(Pending::Op(op));
            return Outcome::Continue;
        }
        match c {
            'i' | 'a' => {
                self.pending = Some(Pending::Object(op, c == 'i'));
                Outcome::Continue
            }
            'f' | 't' | 'F' | 'T' => {
                self.pending = Some(Pending::Find(FindSpec { forward: c.is_lowercase(), till: c.eq_ignore_ascii_case(&'t') }, Some(op)));
                Outcome::Continue
            }
            // `cw` changes to the end of the word, like `ce`.
            'w' | 'W' if op == Op::Change && chars.get(*at).is_some_and(|ch| !ch.is_whitespace()) => {
                let target = self.motion(chars, *at, if c == 'w' { 'e' } else { 'E' });
                self.apply(chars, at, Some(op), target)
            }
            _ => {
                let target = self.motion(chars, *at, c);
                if target.is_none() {
                    self.count = None;
                    self.op_count = None;
                }
                self.apply(chars, at, Some(op), target)
            }
        }
    }

    /// Moves to `target`, or applies `op` from the caret to it.
    fn apply(&mut self, chars: &mut Vec<char>, at: &mut usize, op: Option<Op>, target: Option<Target>) -> Outcome {
        let Some(t) = target else { return Outcome::Continue };
        let Some(op) = op else {
            *at = t.to;
            return Outcome::Continue;
        };
        let (s, e) = if t.to < *at { (t.to, *at) } else { (*at, t.to) };
        let e = if t.inclusive { e + 1 } else { e };
        let len = chars.len();
        self.operate(chars, at, op, s.min(len), e.min(len))
    }

    fn operate(&mut self, chars: &mut Vec<char>, at: &mut usize, op: Op, s: usize, e: usize) -> Outcome {
        let Some(span) = chars.get(s..e) else { return Outcome::Continue };
        if !span.is_empty() {
            self.register = span.iter().collect();
        }
        if op != Op::Yank {
            chars.drain(s..e);
        }
        *at = s;
        if op == Op::Change { Outcome::Insert } else { Outcome::Continue }
    }

    fn motion(&mut self, chars: &[char], at: usize, c: char) -> Option<Target> {
        let len = chars.len();
        let n = |s: &mut Self| s.count.take().unwrap_or(1).min(MAX_COUNT) as usize * s.op_count.unwrap_or(1).min(MAX_COUNT) as usize;
        let excl = |to| Some(Target { to, inclusive: false });
        let incl = |to| Some(Target { to, inclusive: true });
        let target = match c {
            'h' => excl(at.saturating_sub(n(self))),
            'l' | ' ' => excl((at + n(self)).min(len)),
            '0' => excl(0),
            '^' => excl(first_non_blank(chars)),
            '$' => incl(len.saturating_sub(1)),
            'w' | 'W' => {
                let mut i = at;
                for _ in 0..n(self) {
                    i = next_word(chars, i, c == 'W');
                }
                excl(i)
            }
            'b' | 'B' => {
                let mut i = at;
                for _ in 0..n(self) {
                    i = prev_word(chars, i, c == 'B');
                }
                excl(i)
            }
            'e' | 'E' => {
                let mut i = at;
                for _ in 0..n(self) {
                    i = word_end(chars, i, c == 'E');
                }
                incl(i)
            }
            ';' | ',' => {
                let (spec, ch) = self.last_find?;
                let spec = if c == ',' { FindSpec { forward: !spec.forward, ..spec } } else { spec };
                let count = n(self);
                // Repeating `t` from just before the match would not move: start one further.
                let from = if spec.till { if spec.forward { at + 1 } else { at.saturating_sub(1) } } else { at };
                find(chars, from, spec, ch, count)
            }
            _ => return None,
        };
        self.op_count = None;
        target
    }
}

fn clamp_normal(len: usize, at: usize) -> usize {
    at.min(len.saturating_sub(1))
}

fn first_non_blank(chars: &[char]) -> usize {
    chars.iter().position(|c| !c.is_whitespace()).unwrap_or(0)
}

fn toggle_case(c: char) -> char {
    if c.is_lowercase() { c.to_uppercase().next().unwrap_or(c) } else { c.to_lowercase().next().unwrap_or(c) }
}

/// 0 blank, 1 word characters, 2 punctuation (`big`: everything not blank is one class).
fn class(c: char, big: bool) -> u8 {
    if c.is_whitespace() {
        0
    } else if big || c.is_alphanumeric() || c == '_' {
        1
    } else {
        2
    }
}

fn next_word(chars: &[char], mut i: usize, big: bool) -> usize {
    let len = chars.len();
    if i >= len {
        return len;
    }
    let k = class(chars[i], big);
    while i < len && k != 0 && class(chars[i], big) == k {
        i += 1;
    }
    while i < len && class(chars[i], big) == 0 {
        i += 1;
    }
    i
}

fn prev_word(chars: &[char], mut i: usize, big: bool) -> usize {
    i = i.min(chars.len());
    if i == 0 {
        return 0;
    }
    i -= 1;
    while i > 0 && class(chars[i], big) == 0 {
        i -= 1;
    }
    let k = class(chars[i], big);
    while i > 0 && class(chars[i - 1], big) == k {
        i -= 1;
    }
    i
}

fn word_end(chars: &[char], i: usize, big: bool) -> usize {
    let len = chars.len();
    if len == 0 {
        return 0;
    }
    let mut i = (i + 1).min(len - 1);
    while i + 1 < len && class(chars[i], big) == 0 {
        i += 1;
    }
    let k = class(chars[i], big);
    while i + 1 < len && class(chars[i + 1], big) == k {
        i += 1;
    }
    i
}

/// The `n`th `ch` from `at` in the direction of `spec`.
fn find(chars: &[char], at: usize, spec: FindSpec, ch: char, n: usize) -> Option<Target> {
    let mut i = at;
    for _ in 0..n.max(1) {
        i = if spec.forward { (i + 1..chars.len()).find(|&j| chars[j] == ch)? } else { (0..i.min(chars.len())).rev().find(|&j| chars[j] == ch)? };
    }
    let to = match (spec.till, spec.forward) {
        (false, _) => i,
        (true, true) => i - 1,
        (true, false) => i + 1,
    };
    Some(Target { to, inclusive: spec.forward })
}

/// `iw` / `aw` (and `iW` / `aW`) around `at` as a char range.
fn word_object(chars: &[char], at: usize, c: char, inner: bool) -> Option<(usize, usize)> {
    let big = match c {
        'w' => false,
        'W' => true,
        _ => return None,
    };
    let len = chars.len();
    if at >= len {
        return None;
    }
    let k = class(chars[at], big);
    let mut s = at;
    while s > 0 && class(chars[s - 1], big) == k {
        s -= 1;
    }
    let mut e = at + 1;
    while e < len && class(chars[e], big) == k {
        e += 1;
    }
    if !inner {
        let trailing = e;
        while e < len && class(chars[e], big) == 0 {
            e += 1;
        }
        if e == trailing {
            while s > 0 && class(chars[s - 1], big) == 0 {
                s -= 1;
            }
        }
    }
    Some((s, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs `keys` from normal mode with the caret at `at`; returns the text, caret and last outcome.
    fn run(text: &str, at: usize, keys: &str) -> (String, usize, Outcome) {
        let mut vim = TextVim { normal: true, ..TextVim::default() };
        let mut text = text.to_string();
        let mut caret = at;
        let mut outcome = Outcome::Continue;
        for c in keys.chars() {
            outcome = vim.feed(&mut text, &mut caret, &Input::Char(c));
        }
        (text, caret, outcome)
    }

    fn caret(text: &str, at: usize, keys: &str) -> usize {
        run(text, at, keys).1
    }

    #[test]
    fn enter_normal_steps_back() {
        let mut vim = TextVim::default();
        let mut caret = 5;
        vim.enter_normal("hello", &mut caret);
        assert!(vim.is_normal());
        assert_eq!(caret, 4);
        let mut caret = 0;
        vim.enter_normal("", &mut caret);
        assert_eq!(caret, 0);
    }

    #[test]
    fn character_and_line_motions() {
        assert_eq!(caret("hello", 0, "l"), 1);
        assert_eq!(caret("hello", 0, "3l"), 3);
        assert_eq!(caret("hello", 0, "9l"), 4);
        assert_eq!(caret("hello", 4, "2h"), 2);
        assert_eq!(caret("hello", 2, "$"), 4);
        assert_eq!(caret("  hi", 3, "0"), 0);
        assert_eq!(caret("  hi", 3, "^"), 2);
    }

    #[test]
    fn word_motions() {
        let s = "=SUM(A1, B2) foo";
        assert_eq!(caret(s, 0, "w"), 1);
        assert_eq!(caret(s, 1, "w"), 4);
        assert_eq!(caret(s, 1, "e"), 3);
        assert_eq!(caret(s, 0, "W"), 9);
        assert_eq!(caret(s, 15, "b"), 13);
        assert_eq!(caret(s, 13, "B"), 9);
        assert_eq!(caret(s, 0, "2E"), 11);
        assert_eq!(caret("ab", 1, "w"), 1);
    }

    #[test]
    fn find_motions_and_repeat() {
        let s = "a,b,c,d";
        assert_eq!(caret(s, 0, "f,"), 1);
        assert_eq!(caret(s, 0, "2f,"), 3);
        assert_eq!(caret(s, 0, "f,;"), 3);
        assert_eq!(caret(s, 0, "f,;,"), 1);
        assert_eq!(caret(s, 0, "t,"), 0);
        assert_eq!(caret(s, 0, "t,;"), 2);
        assert_eq!(caret(s, 6, "F,"), 5);
        assert_eq!(caret(s, 6, "T,"), 6);
        assert_eq!(caret(s, 0, "fz"), 0);
    }

    #[test]
    fn deletes_and_the_register() {
        assert_eq!(run("hello", 1, "x"), ("hllo".into(), 1, Outcome::Continue));
        assert_eq!(run("hello", 1, "3x").0, "ho");
        assert_eq!(run("hello", 4, "x"), ("hell".into(), 3, Outcome::Continue));
        assert_eq!(run("hello", 2, "X").0, "hllo");
        assert_eq!(run("hello", 2, "D"), ("he".into(), 1, Outcome::Continue));
        assert_eq!(run("one two three", 0, "dw").0, "two three");
        assert_eq!(run("one two three", 0, "2dw").0, "three");
        assert_eq!(run("one two three", 0, "d2w").0, "three");
        assert_eq!(run("one two three", 4, "de").0, "one  three");
        assert_eq!(run("one two three", 8, "db").0, "one three");
        assert_eq!(run("one two", 4, "d0").0, "two");
        assert_eq!(run("one two", 2, "dd").0, "");
        assert_eq!(run("a,b,c", 0, "df,").0, "b,c");
        assert_eq!(run("a,b,c", 0, "dt,").0, ",b,c");
        assert_eq!(run("hello", 0, "xp").0, "ehllo");
        assert_eq!(run("hello", 0, "dwP").0, "hello");
        assert_eq!(run("ab", 0, "yl2p").0, "aaab");
    }

    #[test]
    fn changes_enter_insert() {
        assert_eq!(run("one two", 0, "cw"), (" two".into(), 0, Outcome::Insert));
        assert_eq!(run("one two", 4, "C"), ("one ".into(), 4, Outcome::Insert));
        assert_eq!(run("one two", 4, "cc"), ("".into(), 0, Outcome::Insert));
        assert_eq!(run("one two", 4, "S").0, "");
        assert_eq!(run("one two", 1, "s"), ("oe two".into(), 1, Outcome::Insert));
        assert_eq!(run("=SUM(A1)", 2, "ciw"), ("=(A1)".into(), 1, Outcome::Insert));
        assert_eq!(run("one two three", 5, "daw").0, "one three");
        assert_eq!(run("one two", 5, "daw").0, "one");
        assert_eq!(run("one two three", 5, "diW").0, "one  three");
    }

    #[test]
    fn insert_positions() {
        assert_eq!(run("hello", 2, "i"), ("hello".into(), 2, Outcome::Insert));
        assert_eq!(run("hello", 2, "a").1, 3);
        assert_eq!(run("hello", 4, "a").1, 5);
        assert_eq!(run("  hi", 3, "I").1, 2);
        assert_eq!(run("hi", 0, "A").1, 2);
    }

    #[test]
    fn replace_and_toggle_case() {
        assert_eq!(run("hello", 0, "rj"), ("jello".into(), 0, Outcome::Continue));
        assert_eq!(run("hello", 0, "3rx"), ("xxxlo".into(), 2, Outcome::Continue));
        assert_eq!(run("hi", 0, "5rx").0, "hi");
        assert_eq!(run("hello", 0, "~~"), ("HEllo".into(), 2, Outcome::Continue));
    }

    #[test]
    fn undo_restores_text_and_caret() {
        assert_eq!(run("hello", 1, "xxu"), ("hllo".into(), 1, Outcome::Continue));
        assert_eq!(run("hello", 1, "xxuu"), ("hello".into(), 1, Outcome::Continue));
        assert_eq!(run("hello", 1, "u").0, "hello");
    }

    #[test]
    fn undo_takes_back_an_insert() {
        let mut vim = TextVim { normal: true, ..TextVim::default() };
        let mut text = "ab".to_string();
        let mut caret = 0;
        assert_eq!(vim.feed(&mut text, &mut caret, &Input::Char('A')), Outcome::Insert);
        text.push_str("cd");
        caret = 4;
        vim.enter_normal(&text, &mut caret);
        vim.feed(&mut text, &mut caret, &Input::Char('u'));
        assert_eq!(text, "ab");
    }

    #[test]
    fn escape_cancels_then_commits_and_enter_commits() {
        let mut vim = TextVim { normal: true, ..TextVim::default() };
        let mut text = "abc".to_string();
        let mut caret = 0;
        vim.feed(&mut text, &mut caret, &Input::Char('d'));
        assert_eq!(vim.pending_keys(), "d");
        assert_eq!(vim.feed(&mut text, &mut caret, &Input::Esc), Outcome::Continue);
        assert_eq!(vim.pending_keys(), "");
        assert_eq!(vim.feed(&mut text, &mut caret, &Input::Esc), Outcome::Commit { down: false });
        assert_eq!(vim.feed(&mut text, &mut caret, &Input::Enter), Outcome::Commit { down: true });
        assert_eq!(text, "abc");
    }

    #[test]
    fn unknown_keys_and_empty_text_are_harmless() {
        assert_eq!(run("", 0, "xXdwDp~rqe$bw"), ("".into(), 0, Outcome::Continue));
        assert_eq!(run("", 0, "cw"), ("".into(), 0, Outcome::Insert));
        assert_eq!(run("abc", 1, "dz").0, "abc");
        assert_eq!(run("abc", 1, "99999999x").0, "a");
        assert_eq!(run("héllo wörld", 3, "dw").0, "hélwörld");
    }
}
