//! Searchable vim-mode key index (`?` in NORMAL / VISUAL).

use egui::{Context, Key, Modifiers, Rect, Sense, vec2};
use std::collections::BTreeMap;

use crate::SheetApp;
use crate::palette::fuzzy;
use crate::theme::{self, Tokens};

const ROW_H: f32 = 28.0;

#[derive(Clone, Debug, Default)]
pub struct CheatSheet {
    pub query: String,
    pub index: usize,
    focused: bool,
    last_query: String,
}

#[derive(Clone, Debug)]
struct Row {
    keys: String,
    desc: String,
    category: String,
}

/// Every built-in binding (keep in sync with `vim.rs` and README vim table).
const BINDINGS: &[(&str, &str, &str)] = &[
    ("?", "Open this cheat sheet (searchable)", "Help"),
    ("h j k l", "Move left / down / up / right", "Movement"),
    ("w b", "Next / previous filled cell in the row", "Movement"),
    ("] [", "Next / previous filled cell in the column", "Movement"),
    ("0 ^ $", "Start of row / first filled / last filled in row", "Movement"),
    ("gg G", "First / last used row (12G → row 12)", "Movement"),
    ("Ctrl-d Ctrl-u", "Half screen down / up", "Movement"),
    ("Ctrl-f Ctrl-b", "Page down / up", "Movement"),
    ("H M L", "Top / middle / bottom row on screen", "Movement"),
    ("go …", "Go to address as you type (goB7)", "Movement"),
    ("gl", "Back before the last jump", "Movement"),
    ("zt zz zb", "Scroll cursor row to top / middle / bottom", "Movement"),
    ("zh zl", "Scroll one column left / right", "Movement"),
    ("i a Enter", "Edit cell (start / end / default)", "Edit"),
    ("s cc", "Replace cell contents", "Edit"),
    ("o O", "Open row below / above and edit", "Edit"),
    ("x", "Cut cell contents (yank then clear)", "Edit"),
    ("dd dr dc", "Delete rows / rows at cursor / columns", "Rows & columns"),
    ("yy yr yc", "Copy rows / rows / columns", "Rows & columns"),
    ("+r +c", "Insert rows / columns", "Rows & columns"),
    ("p P", "Put below / above (rows) or right / left (cols)", "Clipboard"),
    ("Pv Pf Pc Pt", "Paste values / formats / formulas / transposed", "Clipboard"),
    ("\"a …", "Use register a–z for next yank/put (\"ayy)", "Registers"),
    ("y (visual)", "Yank selection", "Visual"),
    ("d c (visual)", "Delete / change selection", "Visual"),
    ("v V Ctrl-v", "Cell / row / column visual", "Visual"),
    ("o (visual)", "Swap active end of selection", "Visual"),
    ("v> v<", "Indent / outdent selection", "Visual"),
    ("{ | }", "Align left / centre / right", "Format"),
    ("fh fl fk fj", "Column narrower/wider; row shorter/taller", "Format"),
    ("Zr Zc", "Hide row / column", "Rows & columns"),
    ("Sr Sc", "Unhide rows / columns next to cursor", "Rows & columns"),
    ("Ctrl-a Ctrl-x", "Add / subtract from number or first number in text", "Edit"),
    ("> <", "Next / previous sheet", "Sheets"),
    ("gt gT", "Next / previous sheet (alternate)", "Sheets"),
    ("u Ctrl-r", "Undo / redo", "Edit"),
    (".", "Repeat last change", "Edit"),
    ("/ #", "Find forward / backward in sheet", "Search"),
    ("n N", "Next / previous find match", "Search"),
    ("m a", "Set mark a–z on cell or selection", "Marks"),
    ("'a `a", "Jump to mark a", "Marks"),
    ("q a … q", "Record macro a", "Macros"),
    ("@a @@", "Play macro / play last again", "Macros"),
    (":", "Command palette & command line", "Command"),
    (":w :q :e …", "Save, close, open, sort, map, …", "Command"),
    ("⌘⇧Space", "Command palette (vim off too)", "App"),
    ("Esc", "Cancel prefix; leave visual; clear register prefix", "Other"),
    ("Esc Esc (editing)", "CELL mode then save cell", "Cell mode"),
    ("Ctrl-hjkl (formula)", "Point at adjacent cells / extend range", "Cell mode"),
];

pub fn rows(query: &str, maps: &BTreeMap<String, String>) -> Vec<Row> {
    let q = query.trim();
    let mut out: Vec<(i32, usize, Row)> = Vec::new();
    for (order, (keys, desc, cat)) in BINDINGS.iter().enumerate() {
        let score = [fuzzy(q, keys), fuzzy(q, desc), fuzzy(q, cat), fuzzy(q, &format!("{cat} {desc} {keys}"))].into_iter().flatten().max();
        if let Some(score) = score {
            out.push((score, order, Row { keys: (*keys).into(), desc: (*desc).into(), category: (*cat).into() }));
        }
    }
    let base = BINDINGS.len();
    for (order, (lhs, rhs)) in maps.iter().enumerate() {
        let keys = lhs.clone();
        let desc = format!("→ {rhs}");
        let cat = "Your mappings".to_string();
        let score = [fuzzy(q, &keys), fuzzy(q, &rhs), fuzzy(q, &cat)].into_iter().flatten().max();
        if let Some(score) = score {
            out.push((score, base + order, Row { keys, desc, category: cat }));
        }
    }
    out.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    out.into_iter().map(|(.., r)| r).take(200).collect()
}

pub fn show(app: &mut SheetApp, ctx: &Context) {
    let Some(mut sheet) = app.cheat_sheet.take() else { return };
    let (up, down, esc, back) = ctx.input_mut(|i| {
        let up = i.consume_key(Modifiers::NONE, Key::ArrowUp) || i.consume_key(Modifiers::CTRL, Key::P) || i.consume_key(Modifiers::CTRL, Key::K);
        let down = i.consume_key(Modifiers::NONE, Key::ArrowDown) || i.consume_key(Modifiers::CTRL, Key::N) || i.consume_key(Modifiers::CTRL, Key::J);
        let esc = i.consume_key(Modifiers::NONE, Key::Escape);
        let back = sheet.query.is_empty() && i.key_pressed(Key::Backspace);
        (up, down, esc, back)
    });
    if esc || back {
        return;
    }
    let list = rows(&sheet.query, &app.ui.vim_maps);
    if sheet.query != sheet.last_query {
        sheet.index = 0;
        sheet.last_query = sheet.query.clone();
    }
    if down {
        sheet.index = (sheet.index + 1).min(list.len().saturating_sub(1));
    }
    if up {
        sheet.index = sheet.index.saturating_sub(1);
    }
    sheet.index = sheet.index.min(list.len().saturating_sub(1));

    let t = Tokens::get(ctx);
    let screen = ctx.content_rect();
    let width = 720f32.min(screen.width() - 40.0).max(280.0);
    let list_h = (screen.height() * 0.65 - 80.0).clamp(200.0, 560.0);
    let area = egui::Area::new(egui::Id::new("gridcraft.vim.cheatsheet"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::pos2(screen.center().x - width / 2.0, screen.top() + 48.0));
    let shown = area.show(ctx, |ui| {
        egui::Frame::popup(ui.style()).inner_margin(10.0).show(ui, |ui| {
            ui.set_width(width);
            ui.label(egui::RichText::new("Vim keys").font(theme::ui_font(17.0)).strong().color(t.text));
            ui.label(egui::RichText::new("Filter bindings — Esc to close").color(t.text_dim).size(12.0));
            ui.add_space(4.0);
            let r = ui.add(
                egui::TextEdit::singleline(&mut sheet.query)
                    .id(egui::Id::new("gridcraft.vim.cheatsheet.input"))
                    .font(theme::ui_font(15.0))
                    .frame(egui::Frame::NONE)
                    .desired_width(f32::INFINITY)
                    .hint_text("search keys or actions…"),
            );
            if !sheet.focused || !r.has_focus() {
                r.request_focus();
                sheet.focused = true;
            }
            ui.separator();
            if list.is_empty() {
                ui.label(egui::RichText::new("No matching bindings").color(t.text_dim));
            }
            egui::ScrollArea::vertical().max_height(list_h).auto_shrink([false, true]).show(ui, |ui| {
                for (i, row) in list.iter().enumerate() {
                    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
                    if i == sheet.index || resp.hovered() {
                        ui.painter_at(rect).rect_filled(rect, 4.0, ui.visuals().selection.bg_fill.gamma_multiply(0.35));
                    }
                    paint_row(ui, rect, row, &t);
                }
            });
        });
    });
    let outside = ctx.input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|pos| !shown.response.rect.contains(pos)));
    if outside {
        return;
    }
    app.cheat_sheet = Some(sheet);
}

fn paint_row(ui: &egui::Ui, rect: Rect, row: &Row, t: &Tokens) {
    let painter = ui.painter_at(rect);
    let left = rect.left() + 8.0;
    let mid = rect.center().y;
    let keys_font = egui::FontId::monospace(13.0);
    let gk = painter.layout_no_wrap(row.keys.clone(), keys_font, t.text);
    let keys_w = gk.size().x;
    painter.galley(egui::pos2(left, mid - gk.size().y / 2.0), gk, t.text);
    let path_left = left + keys_w + 14.0;
    let cat = format!("{} · ", row.category);
    let gc = painter.layout_no_wrap(cat, theme::ui_font(11.5), t.text_dim);
    let cat_w = gc.size().x;
    painter.galley(egui::pos2(path_left, mid - gc.size().y / 2.0), gc, t.text_dim);
    let desc_left = path_left + cat_w;
    let g = painter.layout_no_wrap(row.desc.clone(), theme::ui_font(13.0), t.text);
    painter.galley(egui::pos2(desc_left, mid - g.size().y / 2.0), g, t.text);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_bindings_by_keyword() {
        let list = rows("sheet", &BTreeMap::new());
        assert!(list.iter().any(|r| r.keys.contains('>')));
        assert!(rows("macro", &BTreeMap::new()).iter().any(|r| r.keys.contains('@')));
    }
}
