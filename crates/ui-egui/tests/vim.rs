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
    h.input_mut().events.extend([key_event(key, true, modifiers), key_event(key, false, modifiers)]);
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
    typed(&mut h, "}");
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
fn insert_edits_and_escape_commits() {
    let mut h = harness(rows(), true);
    typed(&mut h, "a");
    assert!(h.state().editor.is_some());
    assert_eq!(gridcraft_ui_egui::vim::mode_label(h.state()), "INSERT");
    typed(&mut h, "x");
    esc(&mut h);
    assert!(h.state().editor.is_none());
    assert_eq!(text(&h, "A1"), "r1x", "a appends and Escape keeps the edit");
    assert_eq!(gridcraft_ui_egui::vim::mode_label(h.state()), "NORMAL");
    typed(&mut h, "i");
    typed(&mut h, "y");
    esc(&mut h);
    assert_eq!(text(&h, "A1"), "yr1x", "i puts the caret at the start");
    typed(&mut h, "s");
    typed(&mut h, "new");
    esc(&mut h);
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
    esc(&mut h);
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
    typed(&mut h, "99999<<9999.");
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
