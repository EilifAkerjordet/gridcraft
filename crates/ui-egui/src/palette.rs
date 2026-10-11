//! The command palette: every command, filtered as you type, with its ribbon place and its
//! shortcuts. ⌘⇧Space (Ctrl+Shift+Space) or the ribbon's search button open it, and `:` does in
//! vim mode, where the box is also vim's command line (`:w`, `:sort`, `:B7`, `:map`…, see [`Ex`]).

use egui::{Color32, Context, Key, Modifiers, Rect, Sense, vec2};
use gridcraft_engine::core::{CellRef, RangeRef};
use serde_json::{Value as Json, json};

use crate::SheetApp;
use crate::theme::{self, Tokens};

/// Recently run commands kept for an empty query.
const RECENT: usize = 12;

#[derive(Clone, Debug, Default)]
pub struct Palette {
    pub query: String,
    pub index: usize,
    /// Opened with `:` in vim mode: shown as a command line.
    pub vim: bool,
    focused: bool,
    last_query: String,
}

impl Palette {
    pub fn new(vim: bool) -> Palette {
        Palette { vim, ..Palette::default() }
    }
}

/// A command line typed into the palette.
#[derive(Clone, Debug, PartialEq)]
pub enum Ex {
    Write(Option<String>),
    Quit {
        force: bool,
    },
    WriteQuit,
    Edit(String),
    Sort {
        descending: bool,
    },
    Fill {
        right: bool,
    },
    Freeze,
    Unfreeze,
    /// `:42`, one-based.
    Row(u32),
    /// `:B7` or `:B7:C9`.
    Goto(String),
    Map {
        lhs: String,
        rhs: String,
    },
    Unmap(String),
    ListMaps,
    /// `:home.bold {"range": "A1"}`.
    Command {
        id: String,
        params: Json,
    },
}

pub fn parse_ex(text: &str) -> Option<Ex> {
    let s = text.trim().trim_start_matches(':').trim();
    if s.is_empty() {
        return None;
    }
    let (word, rest) = s.split_once(char::is_whitespace).map(|(w, r)| (w, r.trim())).unwrap_or((s, ""));
    let ex = match word {
        "w" | "write" => Ex::Write((!rest.is_empty()).then(|| rest.to_string())),
        "q" | "quit" | "close" if rest.is_empty() => Ex::Quit { force: false },
        "q!" | "quit!" if rest.is_empty() => Ex::Quit { force: true },
        "wq" | "x" | "xit" if rest.is_empty() => Ex::WriteQuit,
        "e" | "edit" | "o" | "open" if !rest.is_empty() => Ex::Edit(rest.to_string()),
        "sort" => match rest {
            "" | "asc" | "a" => Ex::Sort { descending: false },
            "desc" | "d" | "!" => Ex::Sort { descending: true },
            _ => return None,
        },
        "sort!" if rest.is_empty() => Ex::Sort { descending: true },
        "fill" => match rest {
            "" | "down" | "d" => Ex::Fill { right: false },
            "right" | "r" => Ex::Fill { right: true },
            _ => return None,
        },
        "freeze" if rest.is_empty() => Ex::Freeze,
        "unfreeze" | "nofreeze" if rest.is_empty() => Ex::Unfreeze,
        "map" | "nmap" | "noremap" | "nnoremap" => {
            if rest.is_empty() {
                Ex::ListMaps
            } else {
                let (lhs, rhs) = rest.split_once(char::is_whitespace)?;
                let rhs = rhs.trim();
                if rhs.is_empty() {
                    return None;
                }
                Ex::Map { lhs: lhs.to_string(), rhs: rhs.to_string() }
            }
        }
        "unmap" | "nunmap" if !rest.is_empty() => Ex::Unmap(rest.to_string()),
        _ if rest.is_empty() && !word.is_empty() && word.chars().all(|c| c.is_ascii_digit()) => Ex::Row(word.parse().ok().filter(|n| *n >= 1)?),
        _ if rest.is_empty() && (CellRef::parse(&word.to_ascii_uppercase()).is_some() || RangeRef::parse(&word.to_ascii_uppercase()).is_some()) => {
            Ex::Goto(word.to_ascii_uppercase())
        }
        _ if is_command_id(word) => {
            let params = if rest.is_empty() { json!({}) } else { serde_json::from_str(rest).ok().filter(Json::is_object)? };
            Ex::Command { id: word.to_string(), params }
        }
        _ => return None,
    };
    Some(ex)
}

fn is_command_id(word: &str) -> bool {
    let mut parts = word.split('.');
    parts.next().is_some_and(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphabetic()))
        && word.contains('.')
        && parts.all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric()))
}

/// What a command line will do, for its row.
pub fn describe(ex: &Ex) -> String {
    match ex {
        Ex::Write(None) => "Save".into(),
        Ex::Write(Some(p)) => format!("Save as {p}"),
        Ex::Quit { force: false } => "Close the workbook".into(),
        Ex::Quit { force: true } => "Close the workbook without saving".into(),
        Ex::WriteQuit => "Save and close the workbook".into(),
        Ex::Edit(p) => format!("Open {p}"),
        Ex::Sort { descending: false } => "Sort A to Z".into(),
        Ex::Sort { descending: true } => "Sort Z to A".into(),
        Ex::Fill { right: false } => "Fill Down".into(),
        Ex::Fill { right: true } => "Fill Right".into(),
        Ex::Freeze => "Freeze Panes at the cursor".into(),
        Ex::Unfreeze => "Unfreeze Panes".into(),
        Ex::Row(r) => format!("Go to row {r}"),
        Ex::Goto(a) => format!("Go to {a}"),
        Ex::Map { lhs, rhs } => format!("Map {lhs} to {rhs} in vim mode"),
        Ex::Unmap(lhs) => format!("Remove the mapping of {lhs}"),
        Ex::ListMaps => "List vim key mappings".into(),
        Ex::Command { id, params } => {
            if params.as_object().is_some_and(|o| o.is_empty()) {
                format!("Run {id}")
            } else {
                format!("Run {id} {params}")
            }
        }
    }
}

pub fn run_ex(app: &mut SheetApp, ex: Ex) {
    // File operations include the cell still being edited, as the File menu does.
    if matches!(ex, Ex::Write(_) | Ex::Quit { .. } | Ex::WriteQuit | Ex::Edit(_)) && !app.commit_edit(0, 0, false, false) {
        return;
    }
    match ex {
        Ex::Write(None) => app.run_or_alert("file.save", json!({})),
        Ex::Write(Some(path)) => app.run_or_alert("file.save", json!({"path": expand_home(&path)})),
        Ex::Quit { force } => app.run_or_alert("file.close", if force { json!({"force": true}) } else { json!({}) }),
        Ex::WriteQuit => {
            if app.run("file.save", json!({})).is_ok() {
                app.run_or_alert("file.close", json!({}));
            } else {
                app.run_or_alert("file.save", json!({}));
            }
        }
        Ex::Edit(path) => app.open_path(&expand_home(&path)),
        Ex::Sort { descending } => app.run_or_alert(if descending { "data.sortDescending" } else { "data.sortAscending" }, json!({})),
        Ex::Fill { right } => app.run_or_alert(if right { "edit.fillRight" } else { "edit.fillDown" }, json!({})),
        Ex::Freeze => app.run_or_alert("view.freezePanes", json!({})),
        Ex::Unfreeze => app.run_or_alert("view.unfreezePanes", json!({})),
        Ex::Row(row) => {
            let col = app.session.active().map(|d| d.selection.active.col).unwrap_or(0);
            crate::vim::jump_to(app, CellRef::new(row.saturating_sub(1).min(gridcraft_engine::core::MAX_ROWS - 1), col));
        }
        Ex::Goto(a1) => {
            if let Some(cell) = CellRef::parse(&a1) {
                crate::vim::jump_to(app, cell);
            } else {
                app.run_or_alert("selection.set", json!({"range": a1}));
                app.grid.ensure_visible = true;
            }
        }
        Ex::Map { lhs, rhs } => {
            if crate::vim::parse_keys(&lhs).is_empty() {
                app.toast = Some((format!("Cannot map {lhs}"), crate::now_ms()));
            } else {
                app.toast = Some((format!("Mapped {lhs} to {rhs}"), crate::now_ms()));
                app.ui.vim_maps.insert(lhs, rhs);
            }
        }
        Ex::Unmap(lhs) => {
            let msg = if app.ui.vim_maps.remove(&lhs).is_some() { format!("Removed the mapping of {lhs}") } else { format!("No mapping for {lhs}") };
            app.toast = Some((msg, crate::now_ms()));
        }
        Ex::ListMaps => {
            let list = if app.ui.vim_maps.is_empty() {
                "No key mappings. Add one with :map, e.g. :map Q dd".to_string()
            } else {
                app.ui.vim_maps.iter().map(|(l, r)| format!("{l}    {r}")).collect::<Vec<_>>().join("\n")
            };
            app.message = Some(("Vim key mappings".into(), list));
        }
        Ex::Command { id, params } => {
            remember(app, &id);
            app.run_or_alert(&id, params);
            app.grid.ensure_visible = true;
        }
    }
}

fn expand_home(path: &str) -> String {
    match (path.strip_prefix("~/"), std::env::var("HOME")) {
        (Some(rest), Ok(home)) => format!("{home}/{rest}"),
        _ => path.to_string(),
    }
}

/// How well `query` matches `text`: every query character in order, scored for word starts,
/// runs and a prefix. `None` when it does not match.
pub fn fuzzy(query: &str, text: &str) -> Option<i32> {
    let q: Vec<char> = query.chars().filter(|c| !c.is_whitespace()).flat_map(char::to_lowercase).collect();
    if q.is_empty() {
        return Some(0);
    }
    let t: Vec<char> = text.chars().collect();
    let lower: Vec<char> = t.iter().map(|c| c.to_lowercase().next().unwrap_or(*c)).collect();
    let word_start = |i: usize| i == 0 || !t[i - 1].is_alphanumeric() || (t[i].is_uppercase() && t[i - 1].is_lowercase());
    let mut best: Option<i32> = None;
    for start in (0..lower.len()).filter(|&i| lower[i] == q[0]) {
        let mut score = 0;
        let mut prev: Option<usize> = None;
        let mut i = start;
        let mut ok = true;
        for &qc in &q {
            // Continue a run, else take the next word start, else the next occurrence.
            let run = prev.map(|p| p + 1).filter(|&k| k < lower.len() && lower[k] == qc);
            let first = prev.is_none().then_some(start);
            let j = first
                .or(run)
                .or_else(|| (i..lower.len()).find(|&k| lower[k] == qc && word_start(k)))
                .or_else(|| (i..lower.len()).find(|&k| lower[k] == qc));
            let Some(j) = j else {
                ok = false;
                break;
            };
            score += 10;
            if word_start(j) {
                score += 15;
            }
            if let Some(p) = prev {
                if j == p + 1 {
                    score += 8;
                } else {
                    score -= ((j - p - 1) as i32).min(10);
                }
            }
            prev = Some(j);
            i = j + 1;
        }
        if !ok {
            continue;
        }
        if start == 0 {
            score += 20;
        }
        if lower.len() == q.len() {
            score += 30;
        }
        best = Some(best.map_or(score, |b| b.max(score)));
    }
    best
}

/// `Cmd+Shift+9` as shown on this platform: `⌘⇧9` on macOS, `Ctrl+Shift+9` elsewhere.
pub fn shortcut_label(shortcut: &str, mac: bool) -> String {
    if !mac {
        return shortcut.replace("Cmd", "Ctrl");
    }
    shortcut
        .split('+')
        .map(|part| match part {
            "Cmd" => "⌘",
            "Shift" => "⇧",
            "Alt" | "Option" => "⌥",
            "Ctrl" => "⌃",
            "PageDown" => "PgDn",
            "PageUp" => "PgUp",
            other => other,
        })
        .collect()
}

/// Commands whose settings live in a dialog: the palette opens the dialog instead.
const DIALOGS: &[(&str, &str)] = &[
    ("data.removeDuplicates", "removeDuplicates"),
    ("data.subtotal", "subtotal"),
    ("data.textToColumns", "textToColumns"),
    ("data.validation", "dataValidation"),
    ("data.sort", "sort"),
    ("data.goalSeek", "goalSeek"),
    ("edit.fillSeries", "series"),
    ("edit.goTo", "goTo"),
    ("edit.goToSpecial", "goToSpecial"),
    ("edit.pasteSpecial", "pasteSpecial"),
    ("edit.find", "find"),
    ("formulas.insertFunction", "insertFunction"),
    ("home.columnWidth", "columnWidth"),
    ("home.defaultWidth", "defaultWidth"),
    ("home.deleteCells", "deleteCells"),
    ("home.insertCells", "insertCells"),
    ("home.formatCells", "formatCells"),
    ("home.manageRules", "manageRules"),
    ("home.rowHeight", "rowHeight"),
    ("insert.link", "insertLink"),
    ("insert.sparkline", "sparkline"),
    ("pageLayout.headerFooter", "headerFooter"),
    ("pageLayout.margins", "pageSetup"),
    ("review.protectSheet", "protectSheet"),
    ("review.unprotectSheet", "unprotectSheet"),
    ("sheet.move", "moveSheet"),
    ("sheet.rename", "renameSheet"),
    ("sheet.unhide", "unhideSheet"),
    ("view.zoom", "zoom"),
];

/// One command row.
#[derive(Clone, Debug)]
pub struct Entry {
    pub id: &'static str,
    pub label: String,
    pub path: String,
    pub shortcut: Option<String>,
    pub keys: Option<String>,
}

/// A palette row: a command line, or a command.
#[derive(Clone, Debug)]
pub enum Row {
    Ex(Ex),
    Command(Entry),
}

/// The rows for `query`: a command line first when it parses as one (in vim mode, or after a
/// `:`), then matching commands (recent ones first when the query is empty).
pub fn rows(app: &SheetApp, query: &str, vim: bool, mac: bool) -> Vec<Row> {
    let colon = query.trim_start().starts_with(':');
    let query = query.trim().trim_start_matches(':').trim();
    let lang = app.ui.language;
    let mut entries: Vec<(i32, usize, Entry)> = Vec::new();
    for (order, c) in app.session.commands().into_iter().enumerate() {
        let keys = crate::vim::keys_for(c.id, &app.ui.vim_maps);
        if !c.enabled || (c.menu.is_empty() && c.shortcut.is_none() && keys.is_none()) {
            continue;
        }
        let label = lang.tr(c.label).trim_end_matches('…').to_string();
        let path = c.menu.iter().map(|m| lang.tr(m)).collect::<Vec<_>>().join(" › ");
        let score = [fuzzy(query, &label).map(|s| s + 10), fuzzy(query, c.label), fuzzy(query, c.id), fuzzy(query, &path).map(|s| s - 15)]
            .into_iter()
            .flatten()
            .max();
        let Some(mut score) = score else { continue };
        if let Some(rank) = app.ui.recent_commands.iter().position(|r| r == c.id) {
            score += if query.is_empty() { 1000 - rank as i32 } else { 6 };
        }
        let shortcut = c.shortcut.map(|s| shortcut_label(s, mac));
        entries.push((score, order, Entry { id: c.id, label, path, shortcut, keys }));
    }
    entries.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut rows: Vec<Row> = parse_ex(query).filter(|_| vim || colon).into_iter().map(Row::Ex).collect();
    rows.extend(entries.into_iter().take(200).map(|(.., e)| Row::Command(e)));
    rows
}

fn remember(app: &mut SheetApp, id: &str) {
    app.ui.recent_commands.retain(|r| r != id);
    app.ui.recent_commands.insert(0, id.to_string());
    app.ui.recent_commands.truncate(RECENT);
}

/// Runs a picked command: its dialog when it has one, otherwise the command on the selection.
pub fn run_command(app: &mut SheetApp, id: &str) {
    remember(app, id);
    if let Some((_, dialog)) = DIALOGS.iter().find(|(c, _)| *c == id) {
        app.open_dialog(dialog, json!({}));
        return;
    }
    if id.starts_with("file.") && !app.commit_edit(0, 0, false, false) {
        return;
    }
    app.run_or_alert(id, json!({}));
    app.grid.ensure_visible = true;
}

const ROW_H: f32 = 28.0;

pub fn show(app: &mut SheetApp, ctx: &Context) {
    let Some(mut p) = app.palette.take() else { return };
    let mac = ctx.os() == egui::os::OperatingSystem::Mac;
    let (up, down, enter, esc, back) = ctx.input_mut(|i| {
        let up = i.consume_key(Modifiers::NONE, Key::ArrowUp) || i.consume_key(Modifiers::CTRL, Key::P) || i.consume_key(Modifiers::CTRL, Key::K);
        let down = i.consume_key(Modifiers::NONE, Key::ArrowDown) || i.consume_key(Modifiers::CTRL, Key::N) || i.consume_key(Modifiers::CTRL, Key::J);
        let enter = i.consume_key(Modifiers::NONE, Key::Enter);
        let esc = i.consume_key(Modifiers::NONE, Key::Escape);
        let back = p.query.is_empty() && i.key_pressed(Key::Backspace);
        (up, down, enter, esc, back)
    });
    if esc || (back && p.vim) {
        return;
    }
    let rows = rows(app, &p.query, p.vim, mac);
    if p.query != p.last_query {
        p.index = 0;
        p.last_query = p.query.clone();
    }
    let moved = up || down;
    if down {
        p.index = (p.index + 1).min(rows.len().saturating_sub(1));
    }
    if up {
        p.index = p.index.saturating_sub(1);
    }
    p.index = p.index.min(rows.len().saturating_sub(1));
    let mut pick = enter.then_some(p.index);
    let t = Tokens::get(ctx);
    let screen = ctx.content_rect();
    let width = 620f32.min(screen.width() - 40.0).max(240.0);
    let area = egui::Area::new(egui::Id::new("gridcraft.palette"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::pos2(screen.center().x - width / 2.0, screen.top() + 80.0));
    let shown = area.show(ctx, |ui| {
        egui::Frame::popup(ui.style()).inner_margin(8.0).show(ui, |ui| {
            ui.set_width(width);
            ui.horizontal(|ui| {
                if p.vim {
                    ui.label(egui::RichText::new(":").font(egui::FontId::monospace(15.0)).strong().color(t.text));
                }
                let hint = if p.vim { "command, or search actions…" } else { "Search actions, or type a command like :sort…" };
                let r = ui.add(
                    egui::TextEdit::singleline(&mut p.query)
                        .id(egui::Id::new("gridcraft.palette.input"))
                        .font(theme::ui_font(15.0))
                        .frame(egui::Frame::NONE)
                        .desired_width(f32::INFINITY)
                        .hint_text(hint),
                );
                if !p.focused || !r.has_focus() {
                    r.request_focus();
                    p.focused = true;
                }
            });
            ui.separator();
            if rows.is_empty() {
                ui.label(egui::RichText::new("No matching actions").color(t.text_dim));
            }
            let list = (screen.height() * 0.6 - 60.0).clamp(140.0, 640.0).min(rows.len() as f32 * ROW_H);
            egui::ScrollArea::vertical().max_height(list).min_scrolled_height(list).auto_shrink([false, true]).show(ui, |ui| {
                for (i, row) in rows.iter().enumerate() {
                    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
                    if resp.clicked() {
                        pick = Some(i);
                    }
                    let selected = i == p.index;
                    if selected && moved {
                        ui.scroll_to_rect(rect, None);
                    }
                    paint_row(ui, rect, row, selected || resp.hovered(), &t);
                }
            });
        });
    });
    let outside = ctx.input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|pos| !shown.response.rect.contains(pos)));
    if outside {
        return;
    }
    if let Some(i) = pick {
        match rows.into_iter().nth(i) {
            Some(Row::Ex(ex)) => run_ex(app, ex),
            Some(Row::Command(e)) => run_command(app, e.id),
            None if !p.query.trim().is_empty() => {
                app.toast = Some((format!("Not a command: {}", p.query.trim()), crate::now_ms()));
            }
            None => {}
        }
        return;
    }
    app.palette = Some(p);
}

fn paint_row(ui: &egui::Ui, rect: Rect, row: &Row, highlight: bool, t: &Tokens) {
    let painter = ui.painter_at(rect);
    if highlight {
        painter.rect_filled(rect, 4.0, ui.visuals().selection.bg_fill.gamma_multiply(0.35));
    }
    let left = rect.left() + 8.0;
    let mid = rect.center().y;
    let font = theme::ui_font(13.5);
    let small = theme::ui_font(11.5);
    let (label, path, badges): (String, String, Vec<String>) = match row {
        Row::Ex(ex) => (describe(ex), "Command line".into(), vec!["⏎".into()]),
        Row::Command(e) => (e.label.clone(), e.path.clone(), e.keys.iter().chain(e.shortcut.iter()).cloned().collect()),
    };
    let g = painter.layout_no_wrap(label, font, t.text);
    let label_w = g.size().x;
    painter.galley(egui::pos2(left, mid - g.size().y / 2.0), g, t.text);
    // Badges from the right: vim keys, then the shortcut.
    let mut right = rect.right() - 8.0;
    for badge in badges.iter().rev() {
        let g = painter.layout_no_wrap(badge.clone(), egui::FontId::monospace(11.5), t.text);
        let size = g.size() + vec2(10.0, 4.0);
        let r = Rect::from_min_size(egui::pos2(right - size.x, mid - size.y / 2.0), size);
        painter.rect_filled(r, 3.0, t.input_bg);
        painter.rect_stroke(r, 3.0, egui::Stroke::new(1.0, t.text_dim.gamma_multiply(0.5)), egui::StrokeKind::Inside);
        painter.galley(r.min + vec2(5.0, 2.0), g, t.text);
        right = r.left() - 6.0;
    }
    let path_left = left + label_w + 12.0;
    if path_left < right - 20.0 {
        let g = painter.layout_no_wrap(path, small, t.text_dim);
        let clip = Rect::from_min_max(egui::pos2(path_left, rect.top()), egui::pos2(right - 8.0, rect.bottom()));
        ui.painter_at(clip).galley(egui::pos2(path_left, mid - g.size().y / 2.0), g, Color32::PLACEHOLDER);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_matches_in_order_and_ranks_word_starts() {
        assert!(fuzzy("frz", "Freeze Panes").is_some());
        assert!(fuzzy("fp", "Freeze Panes").is_some());
        assert!(fuzzy("zf", "Freeze Panes").is_none());
        assert!(fuzzy("", "anything") == Some(0));
        assert!(fuzzy("bold", "Bold") > fuzzy("bold", "Embolden"));
        assert!(fuzzy("fp", "Freeze Panes") > fuzzy("fp", "Info Map"));
        assert!(fuzzy("sort", "Sort A to Z") > fuzzy("sort", "Custom Sort"));
        assert!(fuzzy("hr", "home.hideRows") > fuzzy("hr", "home.chartRows"));
        assert_eq!(fuzzy("A", "a"), fuzzy("a", "A"));
    }

    #[test]
    fn shortcut_labels_follow_the_platform() {
        assert_eq!(shortcut_label("Cmd+Shift+9", true), "⌘⇧9");
        assert_eq!(shortcut_label("Cmd+Shift+9", false), "Ctrl+Shift+9");
        assert_eq!(shortcut_label("Ctrl+PageDown", true), "⌃PgDn");
        assert_eq!(shortcut_label("F2", true), "F2");
    }

    #[test]
    fn command_lines_parse() {
        assert_eq!(parse_ex("w"), Some(Ex::Write(None)));
        assert_eq!(parse_ex(":w ~/out.xlsx"), Some(Ex::Write(Some("~/out.xlsx".into()))));
        assert_eq!(parse_ex("q"), Some(Ex::Quit { force: false }));
        assert_eq!(parse_ex("q!"), Some(Ex::Quit { force: true }));
        assert_eq!(parse_ex("wq"), Some(Ex::WriteQuit));
        assert_eq!(parse_ex("e book.xlsx"), Some(Ex::Edit("book.xlsx".into())));
        assert_eq!(parse_ex("e"), None);
        assert_eq!(parse_ex("sort"), Some(Ex::Sort { descending: false }));
        assert_eq!(parse_ex("sort desc"), Some(Ex::Sort { descending: true }));
        assert_eq!(parse_ex("sort!"), Some(Ex::Sort { descending: true }));
        assert_eq!(parse_ex("sort sideways"), None);
        assert_eq!(parse_ex("fill right"), Some(Ex::Fill { right: true }));
        assert_eq!(parse_ex("freeze"), Some(Ex::Freeze));
        assert_eq!(parse_ex("unfreeze"), Some(Ex::Unfreeze));
        assert_eq!(parse_ex("42"), Some(Ex::Row(42)));
        assert_eq!(parse_ex("0"), None);
        assert_eq!(parse_ex("b7"), Some(Ex::Goto("B7".into())));
        assert_eq!(parse_ex("B7:C9"), Some(Ex::Goto("B7:C9".into())));
        assert_eq!(parse_ex("map Q dd"), Some(Ex::Map { lhs: "Q".into(), rhs: "dd".into() }));
        assert_eq!(parse_ex("nnoremap <Space>s :sort<CR>"), Some(Ex::Map { lhs: "<Space>s".into(), rhs: ":sort<CR>".into() }));
        assert_eq!(parse_ex("map Q"), None);
        assert_eq!(parse_ex("map"), Some(Ex::ListMaps));
        assert_eq!(parse_ex("unmap Q"), Some(Ex::Unmap("Q".into())));
        assert_eq!(parse_ex("home.bold"), Some(Ex::Command { id: "home.bold".into(), params: json!({}) }));
        assert_eq!(parse_ex(r#"home.bold {"range": "A1"}"#), Some(Ex::Command { id: "home.bold".into(), params: json!({"range": "A1"}) }));
        assert_eq!(parse_ex("home.bold {nope"), None);
        assert_eq!(parse_ex("bold"), None);
        assert_eq!(parse_ex("freeze panes"), None);
        assert_eq!(parse_ex(""), None);
        assert_eq!(parse_ex("   "), None);
    }
}
