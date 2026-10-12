//! Vim mode through the complete egui input and UI path.

use egui::{Event, Key, Modifiers};
use gridcraft_engine::Session;
use gridcraft_engine::core::CellRef;
use gridcraft_ui_egui::SheetApp;
use gridcraft_ui_egui::vim::{Mode, Visual};
use serde_json::json;

type Harness = egui_kittest::Harness<'static, SheetApp>;

fn harness(session: Session, vim: bool) -> Harness {
    let mut app = SheetApp::new(session, Default::default());
    app.ui.vim_mode = vim;
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_ui_state(
        |ui, app: &mut SheetApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(4);
    h
}

/// A sheet with A1:A5 = r1..r5 and B1:B5 = 10..50, cursor on A1.
fn rows() -> Session {
    let mut s = Session::new();
    s.new_workbook();
    for r in 1..=5 {
        s.execute("cell.set", json!({"cell": format!("A{r}"), "input": format!("r{r}")})).unwrap();
        s.execute("cell.set", json!({"cell": format!("B{r}"), "input": format!("{}", r * 10)})).unwrap();
    }
    s.execute("selection.set", json!({"cell": "A1"})).unwrap();
    s
}

fn key_event(key: Key, pressed: bool, modifiers: Modifiers) -> Event {
    Event::Key { key, physical_key: None, pressed, repeat: false, modifiers }
}

fn press(h: &mut Harness, key: Key, modifiers: Modifiers) {
    h.input_mut().events.extend([
        Event::ModifiersChanged(modifiers),
        key_event(key, true, modifiers),
        key_event(key, false, modifiers),
        Event::ModifiersChanged(Modifiers::NONE),
    ]);
    h.run_steps(2);
}

/// Types each character as a key press with its text, one frame each, like a keyboard.
fn typed(h: &mut Harness, keys: &str) {
    for c in keys.chars() {
        let key = Key::from_name(&c.to_ascii_uppercase().to_string());
        let mods = if c.is_ascii_uppercase() { Modifiers::SHIFT } else { Modifiers::NONE };
        let mut events = Vec::new();
        if let Some(k) = key {
            events.push(key_event(k, true, mods));
        }
        events.push(Event::Text(c.to_string()));
        if let Some(k) = key {
            events.push(key_event(k, false, mods));
        }
        h.input_mut().events.extend(events);
        h.run_steps(2);
    }
}

fn esc(h: &mut Harness) {
    press(h, Key::Escape, Modifiers::NONE);
}

/// Escape to cell normal mode, then Escape again to save.
fn save(h: &mut Harness) {
    esc(h);
    esc(h);
}

fn text(h: &Harness, cell: &str) -> String {
    h.state().session.active().unwrap().wb.active().unwrap().cell(CellRef::parse(cell).unwrap()).map(|c| c.input_text()).unwrap_or_default()
}

fn active(h: &Harness) -> String {
    h.state().session.active().unwrap().selection.active.a1()
}

fn selected(h: &Harness) -> String {
    h.state().session.active().unwrap().selection.current().a1()
}

#[test]
fn off_by_default_letters_still_start_an_edit() {
    let mut h = harness(rows(), false);
    typed(&mut h, "j");
    assert!(h.state().editor.is_some(), "without vim mode typing edits the cell, as in Excel");
}

#[test]
fn view_vim_mode_toggles_and_is_saved_with_ui_preferences() {
    let mut h = harness(rows(), false);
    assert_eq!(h.state_mut().run("view.vimMode", json!({})).unwrap(), json!({"on": true}));
    assert!(h.state().ui.vim_mode);
    let saved = serde_json::to_value(&h.state().ui).unwrap();
    assert_eq!(saved["vimMode"], json!(true));
    let restored: gridcraft_ui_egui::UiState = serde_json::from_value(saved).unwrap();
    assert!(restored.vim_mode);
    assert_eq!(h.state_mut().run("view.vimMode", json!({"on": false})).unwrap(), json!({"on": false}));
}

#[test]
fn motions_move_the_cursor_without_editing() {
    let mut h = harness(rows(), true);
    typed(&mut h, "3j");
    assert_eq!(active(&h), "A4");
    typed(&mut h, "l");
    assert_eq!(active(&h), "B4");
    typed(&mut h, "k");
    assert_eq!(active(&h), "B3");
    typed(&mut h, "0");
    assert_eq!(active(&h), "A3");
    typed(&mut h, "$");
    assert_eq!(active(&h), "B3", "$ goes to the row's last filled cell");
    typed(&mut h, "G");
    assert_eq!(active(&h), "B5", "G goes to the last used row");
    typed(&mut h, "gg");
    assert_eq!(active(&h), "B1");
    typed(&mut h, "4G");
    assert_eq!(active(&h), "B4");
    typed(&mut h, "]");
    assert_eq!(active(&h), "B5");
    typed(&mut h, "h");
    assert_eq!(active(&h), "A5");
    assert!(h.state().editor.is_none());
    assert_eq!(text(&h, "A1"), "r1", "motion letters must not be typed into cells");
}

#[test]
fn half_page_and_screen_motions_use_the_visible_rows() {
    let mut h = harness(rows(), true);
    let (top, bottom) = h.state().grid.visible_rows.unwrap();
    assert_eq!(top, 0);
    press(&mut h, Key::D, Modifiers::CTRL);
    let half = (bottom - top) / 2;
    assert_eq!(active(&h), CellRef::new(half, 0).a1());
    press(&mut h, Key::U, Modifiers::CTRL);
    assert_eq!(active(&h), "A1");
    typed(&mut h, "L");
    assert_eq!(active(&h), CellRef::new(bottom, 0).a1());
    typed(&mut h, "H");
    assert_eq!(active(&h), "A1");
}

#[test]
fn insert_edits_and_escape_twice_commits() {
    let mut h = harness(rows(), true);
    typed(&mut h, "a");
    assert!(h.state().editor.is_some());
    assert_eq!(gridcraft_ui_egui::vim::mode_label(h.state()), "INSERT");
    typed(&mut h, "x");
    esc(&mut h);
    assert!(h.state().editor.is_some(), "the first Escape stays in the cell");
    assert_eq!(gridcraft_ui_egui::vim::mode_label(h.state()), "CELL");
    esc(&mut h);
    assert!(h.state().editor.is_none());
    assert_eq!(text(&h, "A1"), "r1x", "a appends and Escape keeps the edit");
    assert_eq!(gridcraft_ui_egui::vim::mode_label(h.state()), "NORMAL");
    typed(&mut h, "i");
    typed(&mut h, "y");
    save(&mut h);
    assert_eq!(text(&h, "A1"), "yr1x", "i puts the caret at the start");
    typed(&mut h, "s");
    typed(&mut h, "new");
    save(&mut h);
    assert_eq!(text(&h, "A1"), "new", "s replaces the content");
    typed(&mut h, "u");
    assert_eq!(text(&h, "A1"), "yr1x", "u undoes the committed edit");
    press(&mut h, Key::R, Modifiers::CTRL);
    assert_eq!(text(&h, "A1"), "new");
}

#[test]
fn x_clears_and_p_puts_it_back() {
    let mut h = harness(rows(), true);
    typed(&mut h, "x");
    assert_eq!(text(&h, "A1"), "");
    typed(&mut h, "jjp");
    assert_eq!(text(&h, "A3"), "r1", "x yanks before clearing");
    assert!(h.state().session.clipboard.is_none(), "no marching ants after vim yanks and puts");
}

#[test]
fn dd_deletes_rows_and_p_puts_them_below() {
    let mut h = harness(rows(), true);
    typed(&mut h, "j2dd");
    assert_eq!(active(&h), "A2");
    assert_eq!(text(&h, "A2"), "r4");
    assert_eq!(text(&h, "B2"), "40");
    typed(&mut h, "p");
    assert_eq!(text(&h, "A3"), "r2", "p inserts the deleted rows below the cursor row");
    assert_eq!(text(&h, "A4"), "r3");
    assert_eq!(text(&h, "B4"), "30");
    assert_eq!(text(&h, "A5"), "r5");
    assert_eq!(active(&h), "A3");
}

#[test]
fn yy_and_capital_p_put_above() {
    let mut h = harness(rows(), true);
    typed(&mut h, "4Gyy");
    assert_eq!(text(&h, "A4"), "r4", "yy does not change the sheet");
    typed(&mut h, "ggP");
    assert_eq!(text(&h, "A1"), "r1", "P waits for a typed paste key");
    esc(&mut h);
    assert_eq!(text(&h, "A1"), "r4");
    assert_eq!(text(&h, "B1"), "40");
    assert_eq!(text(&h, "A2"), "r1");
}

#[test]
fn o_opens_a_row_below_and_dot_repeats_it() {
    let mut h = harness(rows(), true);
    typed(&mut h, "o");
    assert!(h.state().editor.is_some());
    assert_eq!(active(&h), "A2");
    typed(&mut h, "hey");
    save(&mut h);
    assert_eq!(text(&h, "A2"), "hey");
    assert_eq!(text(&h, "A3"), "r2");
    typed(&mut h, ".");
    assert!(h.state().editor.is_none(), ". writes the remembered text without opening the editor");
    assert_eq!(text(&h, "A3"), "hey");
    assert_eq!(text(&h, "A4"), "r2");
}

#[test]
fn visual_selects_and_deletes_a_block() {
    let mut h = harness(rows(), true);
    typed(&mut h, "vjl");
    assert_eq!(h.state().vim.mode(), Mode::Visual(Visual::Cells));
    assert_eq!(selected(&h), "A1:B2");
    assert_eq!(gridcraft_ui_egui::vim::mode_label(h.state()), "VISUAL");
    typed(&mut h, "d");
    assert_eq!(h.state().vim.mode(), Mode::Normal);
    assert_eq!(selected(&h), "A1");
    assert_eq!([text(&h, "A1"), text(&h, "B2"), text(&h, "A3")], [String::new(), String::new(), "r3".into()]);
}

#[test]
fn visual_rows_and_escape() {
    let mut h = harness(rows(), true);
    typed(&mut h, "jVj");
    assert_eq!(selected(&h), "2:3");
    esc(&mut h);
    assert_eq!(h.state().vim.mode(), Mode::Normal);
    assert_eq!(selected(&h), "A3");
    typed(&mut h, "Vkd");
    assert_eq!(text(&h, "A2"), "r4", "V then d deletes whole rows");
    assert_eq!(active(&h), "A2");
}

#[test]
fn visual_columns_with_ctrl_q() {
    let mut h = harness(rows(), true);
    press(&mut h, Key::Q, Modifiers::CTRL);
    assert_eq!(h.state().vim.mode(), Mode::Visual(Visual::Cols));
    typed(&mut h, "y");
    typed(&mut h, "p");
    assert_eq!(text(&h, "B1"), "r1", "column put inserts the yanked column to the right");
    assert_eq!(text(&h, "C1"), "10");
}

#[test]
fn visual_indent() {
    let mut h = harness(rows(), true);
    typed(&mut h, "vj>");
    let d = h.state().session.active().unwrap();
    let sh = d.wb.active().unwrap();
    for c in ["A1", "A2"] {
        assert_eq!(d.wb.styles.get(sh.style_id(CellRef::parse(c).unwrap())).align.indent, 1);
    }
}

#[test]
fn search_jumps_forward_and_back() {
    let mut s = rows();
    s.execute("cell.set", json!({"cell": "C2", "input": "needle"})).unwrap();
    s.execute("cell.set", json!({"cell": "C4", "input": "needle"})).unwrap();
    let mut h = harness(s, true);
    typed(&mut h, "/need");
    assert_eq!(h.state().vim.search(), Some("need"));
    assert!(h.state().editor.is_none());
    press(&mut h, Key::Enter, Modifiers::NONE);
    assert_eq!(active(&h), "C2");
    typed(&mut h, "n");
    assert_eq!(active(&h), "C4");
    typed(&mut h, "n");
    assert_eq!(active(&h), "C2", "n wraps around");
    typed(&mut h, "N");
    assert_eq!(active(&h), "C4", "N goes back, wrapping");
    typed(&mut h, "N");
    assert_eq!(active(&h), "C2");
}

#[test]
fn gt_switches_sheets() {
    let mut s = rows();
    s.execute("home.insertSheet", json!({})).unwrap();
    s.execute("sheet.next", json!({})).unwrap_or_default();
    s.execute("sheet.previous", json!({})).unwrap();
    let first = s.active().unwrap().wb.active_sheet;
    let mut h = harness(s, true);
    typed(&mut h, "gt");
    let second = h.state().session.active().unwrap().wb.active_sheet;
    assert_ne!(first, second);
    typed(&mut h, "gT");
    assert_eq!(h.state().session.active().unwrap().wb.active_sheet, first);
}

#[test]
fn arrows_and_command_shortcuts_still_reach_the_grid() {
    let mut h = harness(rows(), true);
    press(&mut h, Key::ArrowDown, Modifiers::NONE);
    assert_eq!(active(&h), "A2");
    press(&mut h, Key::Delete, Modifiers::NONE);
    assert_eq!(text(&h, "A2"), "");
}

#[test]
fn w_and_b_jump_between_filled_cells() {
    let mut s = rows();
    s.execute("cell.set", json!({"cell": "E1", "input": "far"})).unwrap();
    let mut h = harness(s, true);
    typed(&mut h, "w");
    assert_eq!(active(&h), "B1");
    typed(&mut h, "w");
    assert_eq!(active(&h), "E1");
    typed(&mut h, "2b");
    assert_eq!(active(&h), "A1");
}

#[test]
fn dot_repeats_a_visual_delete_from_the_new_cursor() {
    let mut h = harness(rows(), true);
    typed(&mut h, "vjd");
    assert_eq!([text(&h, "A1"), text(&h, "A2")], [String::new(), String::new()]);
    typed(&mut h, "3G.");
    assert_eq!([text(&h, "A3"), text(&h, "A4"), text(&h, "A5")], [String::new(), String::new(), "r5".into()]);
}

#[test]
fn huge_counts_neither_crash_nor_hang() {
    let mut h = harness(rows(), true);
    typed(&mut h, "99999j");
    assert_eq!(active(&h), CellRef::new(99_999, 0).a1());
    typed(&mut h, "gg99999x");
    assert_eq!(text(&h, "B1"), "");
    typed(&mut h, "99999dd");
    assert_eq!(text(&h, "A2"), "");
    typed(&mut h, "u");
    assert_eq!(text(&h, "A2"), "r2");
    typed(&mut h, "99999<9999>");
    assert!(h.state().editor.is_none());
}

#[test]
fn turning_vim_off_restores_excel_typing() {
    let mut h = harness(rows(), true);
    typed(&mut h, "v");
    h.state_mut().run("view.vimMode", json!({"on": false})).unwrap();
    h.run_steps(2);
    assert_eq!(h.state().vim.mode(), Mode::Normal);
    typed(&mut h, "z");
    assert!(h.state().editor.is_some());
}

fn editor_text(h: &Harness) -> String {
    h.state().editor.as_ref().map(|e| e.text.clone()).unwrap_or_default()
}

fn halign(h: &Harness, cell: &str) -> gridcraft_engine::model::HAlign {
    let d = h.state().session.active().unwrap();
    let sh = d.wb.active().unwrap();
    d.wb.styles.get(sh.style_id(CellRef::parse(cell).unwrap())).align.h
}

#[test]
fn cell_normal_mode_edits_the_text() {
    let mut s = rows();
    s.execute("cell.set", json!({"cell": "A1", "input": "one two three"})).unwrap();
    let mut h = harness(s, true);
    typed(&mut h, "A");
    esc(&mut h);
    assert_eq!(gridcraft_ui_egui::vim::mode_label(h.state()), "CELL");
    typed(&mut h, "0w");
    typed(&mut h, "cw");
    assert_eq!(gridcraft_ui_egui::vim::mode_label(h.state()), "INSERT", "cw goes back to typing");
    typed(&mut h, "2");
    assert_eq!(editor_text(&h), "one 2 three");
    esc(&mut h);
    typed(&mut h, "$x");
    assert_eq!(editor_text(&h), "one 2 thre", "motion and edit letters are not typed");
    typed(&mut h, "d");
    assert_eq!(gridcraft_ui_egui::vim::status_keys(h.state()), "d");
    typed(&mut h, "0");
    assert_eq!(editor_text(&h), "e");
    typed(&mut h, "u");
    assert_eq!(editor_text(&h), "one 2 thre");
    press(&mut h, Key::Enter, Modifiers::NONE);
    assert!(h.state().editor.is_none(), "Enter saves from cell normal mode");
    assert_eq!(text(&h, "A1"), "one 2 thre");
    assert_eq!(active(&h), "A2");
}

#[test]
fn enter_saves_while_typing_and_off_mode_escape_still_cancels() {
    let mut h = harness(rows(), true);
    typed(&mut h, "s");
    typed(&mut h, "fresh");
    press(&mut h, Key::Enter, Modifiers::NONE);
    assert_eq!(text(&h, "A1"), "fresh");
    let mut h = harness(rows(), false);
    typed(&mut h, "x");
    esc(&mut h);
    assert!(h.state().editor.is_none());
    assert_eq!(text(&h, "A1"), "r1", "without vim mode Escape still cancels");
}

#[test]
fn ctrl_hjkl_point_at_cells_in_a_formula() {
    let mut h = harness(rows(), true);
    typed(&mut h, "jjjs");
    typed(&mut h, "=SUM(");
    press(&mut h, Key::K, Modifiers::CTRL);
    assert_eq!(editor_text(&h), "=SUM(A3");
    press(&mut h, Key::K, Modifiers::CTRL | Modifiers::SHIFT);
    press(&mut h, Key::K, Modifiers::CTRL | Modifiers::SHIFT);
    assert_eq!(editor_text(&h), "=SUM(A1:A3", "the range keeps its first cell as the anchor");
    press(&mut h, Key::L, Modifiers::CTRL | Modifiers::SHIFT);
    assert_eq!(editor_text(&h), "=SUM(A1:B3");
    typed(&mut h, ")");
    press(&mut h, Key::Enter, Modifiers::NONE);
    assert_eq!(text(&h, "A4"), "=SUM(A1:B3)");
    // Outside a place that takes a reference, Ctrl+L does not insert one.
    typed(&mut h, "kA");
    press(&mut h, Key::L, Modifiers::CTRL);
    assert_eq!(editor_text(&h), "=SUM(A1:B3)");
}

#[test]
fn insert_delete_and_yank_rows_and_columns() {
    let mut h = harness(rows(), true);
    typed(&mut h, "j+r");
    assert_eq!(text(&h, "A2"), "");
    assert_eq!(text(&h, "A3"), "r2", "+r inserts a row at the cursor");
    typed(&mut h, "dr");
    assert_eq!(text(&h, "A2"), "r2");
    typed(&mut h, "l2+c");
    assert_eq!(text(&h, "D1"), "10", "2+c inserts two columns");
    typed(&mut h, "2dc");
    assert_eq!(text(&h, "B1"), "10");
    typed(&mut h, "ycP");
    esc(&mut h);
    assert_eq!(text(&h, "B1"), "10");
    assert_eq!(text(&h, "C1"), "10", "yc then P puts the column back before the cursor");
    typed(&mut h, "dc");
    assert_eq!(text(&h, "C1"), "");
    typed(&mut h, "hdcp");
    assert_eq!(text(&h, "A1"), "10", "dc then p moves a column");
    assert_eq!(text(&h, "B1"), "r1");
    typed(&mut h, "yr4Gp");
    assert_eq!(text(&h, "A5"), "20", "yr then p puts the row below");
    assert_eq!(text(&h, "B5"), "r2");
}

#[test]
fn typed_paste_keys() {
    let mut s = rows();
    s.execute("cell.set", json!({"cell": "C1", "input": "=B1*2"})).unwrap();
    s.execute("selection.set", json!({"cell": "C1"})).unwrap();
    let mut h = harness(s, true);
    typed(&mut h, "vyjPv");
    assert_eq!(text(&h, "C2"), "20", "Pv pastes the value");
    typed(&mut h, "jPc");
    assert_eq!(text(&h, "C3"), "=B3*2", "Pc pastes the formula, references adjusted");
    typed(&mut h, "ggvjyllPt");
    assert_eq!([text(&h, "E1"), text(&h, "F1"), text(&h, "E2")], ["=D1*2".to_string(), "20".to_string(), String::new()], "Pt pastes transposed");
    typed(&mut h, "Pj");
    assert_eq!(text(&h, "E2"), "20");
    assert_eq!(active(&h), "E2", "P then another key is a plain P, then the key");
}

#[test]
fn go_marks_and_jump_back() {
    let mut h = harness(rows(), true);
    typed(&mut h, "goab24");
    assert_eq!(active(&h), "AB24", "go jumps as the address is typed");
    typed(&mut h, "j");
    assert_eq!(active(&h), "AB25", "another key ends the address and runs");
    typed(&mut h, "ma");
    typed(&mut h, "gg0");
    typed(&mut h, "'a");
    assert_eq!(active(&h), "AB25");
    typed(&mut h, "gl");
    assert_eq!(active(&h), "A1", "gl goes back to where the jump started");
    typed(&mut h, "gl");
    assert_eq!(active(&h), "AB25");
    typed(&mut h, "gg0vjlmb");
    esc(&mut h);
    typed(&mut h, "G`b");
    assert_eq!(selected(&h), "A1:B2", "a mark set in visual mode reselects the range");
    typed(&mut h, "'q");
    assert_eq!(selected(&h), "A1:B2", "an unset mark does nothing");
}

#[test]
fn z_scrolls_the_cursor_row() {
    let mut h = harness(rows(), true);
    typed(&mut h, "200G");
    let (top, bottom) = h.state().grid.visible_rows.unwrap();
    assert!(top <= 199 && 199 <= bottom);
    typed(&mut h, "zt");
    assert_eq!(h.state().grid.visible_rows.unwrap().0, 199, "zt puts the row at the top");
    typed(&mut h, "zb");
    assert_eq!(h.state().grid.visible_rows.unwrap().1, 199, "zb puts the row at the bottom");
    typed(&mut h, "zz");
    let (top, bottom) = h.state().grid.visible_rows.unwrap();
    assert!((199 - top).abs_diff(bottom - 199) <= 1, "zz centres the row: {top}..{bottom}");
    assert_eq!(active(&h), "A200");
}

#[test]
fn align_resize_and_hide() {
    use gridcraft_engine::model::HAlign;
    let mut h = harness(rows(), true);
    typed(&mut h, "}");
    assert_eq!(halign(&h, "A1"), HAlign::Right);
    typed(&mut h, "}");
    assert_eq!(halign(&h, "A1"), HAlign::Right, "a second press keeps the alignment");
    typed(&mut h, "vj|");
    assert_eq!([halign(&h, "A1"), halign(&h, "A2")], [HAlign::Center, HAlign::Center]);
    typed(&mut h, "{");
    assert_eq!(halign(&h, "A2"), HAlign::Left, "the cursor stays where the selection ended");
    let width = |h: &Harness, c| h.state().session.active().unwrap().wb.active().unwrap().col_width(c);
    let height = |h: &Harness, r| h.state().session.active().unwrap().wb.active().unwrap().row_height(r);
    // The cursor is on A2.
    let (w, rh) = (width(&h, 0), height(&h, 1));
    typed(&mut h, "2fl");
    assert_eq!(width(&h, 0), w + 16.0);
    typed(&mut h, "fh");
    assert_eq!(width(&h, 0), w + 8.0);
    typed(&mut h, "fj");
    assert_eq!(height(&h, 1), rh + 4.0);
    assert_eq!(height(&h, 0), rh, "only the cursor's row");
    typed(&mut h, "u");
    assert_eq!(height(&h, 1), rh, "a resize is one undo step");
    typed(&mut h, "jZr");
    let sh = |h: &Harness| h.state().session.active().unwrap().wb.active().unwrap().clone();
    assert!(sh(&h).is_row_hidden(2));
    assert!(!sh(&h).is_row_hidden(1));
    typed(&mut h, "klZc");
    assert!(sh(&h).is_col_hidden(1));
    assert!(!sh(&h).is_col_hidden(0));
}

#[test]
fn named_registers_keep_their_own_yanks() {
    let mut h = harness(rows(), true);
    typed(&mut h, "\"ayyjyy");
    typed(&mut h, "G\"ap");
    assert_eq!(text(&h, "A6"), "r1", "\"ap puts register a, not the last yank");
    typed(&mut h, "p");
    assert_eq!(text(&h, "A7"), "r2", "plain p puts the last yank");
    typed(&mut h, "\"zp");
    assert_eq!(text(&h, "A8"), "", "an empty register puts nothing");
}

#[test]
fn macros_replay_keys_and_edits() {
    let mut h = harness(rows(), true);
    typed(&mut h, "qaxjq");
    assert_eq!(text(&h, "A1"), "");
    assert_eq!(active(&h), "A2");
    typed(&mut h, "2@a");
    assert_eq!(text(&h, "A2"), "");
    assert_eq!(text(&h, "A3"), "");
    assert_eq!(active(&h), "A4");
    typed(&mut h, "qbcc");
    typed(&mut h, "new");
    save(&mut h);
    typed(&mut h, "jq");
    assert_eq!(text(&h, "A4"), "new");
    typed(&mut h, "@b");
    assert_eq!(text(&h, "A5"), "new", "the macro writes the text typed while recording");
    assert!(h.state().editor.is_none());
    typed(&mut h, "@@");
    assert_eq!(text(&h, "A6"), "new");
}

#[test]
fn ctrl_a_and_ctrl_x_change_numbers() {
    let mut h = harness(rows(), true);
    typed(&mut h, "l");
    press(&mut h, Key::A, Modifiers::CTRL);
    assert_eq!(text(&h, "B1"), "11");
    typed(&mut h, "5");
    press(&mut h, Key::X, Modifiers::CTRL);
    assert_eq!(text(&h, "B1"), "6");
    typed(&mut h, "h");
    press(&mut h, Key::A, Modifiers::CTRL);
    assert_eq!(text(&h, "A1"), "r2", "in text the first number changes");
    typed(&mut h, "j.");
    assert_eq!(text(&h, "A2"), "r3", ". repeats it");
}

#[test]
fn capital_s_unhides_next_to_the_cursor() {
    let mut h = harness(rows(), true);
    typed(&mut h, "jZr");
    let hidden = |h: &Harness, r: u32| h.state().session.active().unwrap().wb.active().unwrap().is_row_hidden(r);
    assert!(hidden(&h, 1));
    typed(&mut h, "ggSr");
    assert!(!hidden(&h, 1));
    typed(&mut h, "Sr");
    assert!(h.state().toast.is_some(), "nothing hidden: a toast, no change");
}

#[test]
fn question_mark_searches_backward() {
    let mut h = harness(rows(), true);
    typed(&mut h, "3G?r");
    press(&mut h, Key::Enter, Modifiers::NONE);
    assert_eq!(active(&h), "A2");
    typed(&mut h, "n");
    assert_eq!(active(&h), "A1");
    typed(&mut h, "N");
    assert_eq!(active(&h), "A2");
}

#[test]
fn page_and_column_scrolling() {
    let mut h = harness(rows(), true);
    let (top, bottom) = h.state().grid.visible_rows.unwrap();
    press(&mut h, Key::F, Modifiers::CTRL);
    let row = h.state().session.active().unwrap().selection.active.row;
    assert_eq!(row, bottom - top);
    assert_eq!(h.state().grid.visible_rows.unwrap().0, row, "Ctrl-f starts the page at the cursor");
    press(&mut h, Key::B, Modifiers::CTRL);
    assert_eq!(active(&h), "A1");
    typed(&mut h, "3zl");
    assert_eq!(h.state().grid.visible_cols.unwrap().0, 3);
    assert_eq!(active(&h), "A1", "zl scrolls without moving the cursor");
    typed(&mut h, "zh");
    assert_eq!(h.state().grid.visible_cols.unwrap().0, 2);
}
