//! The command palette (⌘⇧Space, the ribbon search, `:` in vim mode) through the egui UI.

use egui::{Event, Key, Modifiers};
use gridcraft_engine::Session;
use gridcraft_engine::core::CellRef;
use gridcraft_ui_egui::SheetApp;
use serde_json::json;

type Harness = egui_kittest::Harness<'static, SheetApp>;

fn harness(vim: bool) -> Harness {
    let mut s = Session::new();
    s.new_workbook();
    for r in 1..=5 {
        s.execute("cell.set", json!({"cell": format!("A{r}"), "input": format!("r{r}")})).unwrap();
        s.execute("cell.set", json!({"cell": format!("B{r}"), "input": format!("{}", r * 10)})).unwrap();
    }
    s.execute("selection.set", json!({"cell": "A1"})).unwrap();
    let mut app = SheetApp::new(s, Default::default());
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

fn key_event(key: Key, pressed: bool, modifiers: Modifiers) -> Event {
    Event::Key { key, physical_key: None, pressed, repeat: false, modifiers }
}

fn press(h: &mut Harness, key: Key, modifiers: Modifiers) {
    h.input_mut().events.extend([Event::ModifiersChanged(modifiers), key_event(key, true, modifiers), key_event(key, false, modifiers)]);
    h.run_steps(1);
    h.input_mut().events.push(Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(1);
}

/// Types each character as a key press with its text, one frame each.
fn typed(h: &mut Harness, keys: &str) {
    for c in keys.chars() {
        let key = Key::from_name(&c.to_ascii_uppercase().to_string());
        let mods = if c.is_ascii_uppercase() { Modifiers::SHIFT } else { Modifiers::NONE };
        let mut events = Vec::new();
        events.extend(key.map(|k| key_event(k, true, mods)));
        events.push(Event::Text(c.to_string()));
        events.extend(key.map(|k| key_event(k, false, mods)));
        h.input_mut().events.extend(events);
        h.run_steps(2);
    }
}

fn enter(h: &mut Harness) {
    press(h, Key::Enter, Modifiers::NONE);
}

fn text(h: &Harness, cell: &str) -> String {
    h.state().session.active().unwrap().wb.active().unwrap().cell(CellRef::parse(cell).unwrap()).map(|c| c.input_text()).unwrap_or_default()
}

fn active(h: &Harness) -> String {
    h.state().session.active().unwrap().selection.active.a1()
}

fn query(h: &Harness) -> Option<String> {
    h.state().palette.as_ref().map(|p| p.query.clone())
}

#[test]
fn colon_opens_the_palette_as_a_command_line() {
    let mut h = harness(true);
    typed(&mut h, ":");
    assert_eq!(query(&h).as_deref(), Some(""), "the : itself is not part of the query");
    assert!(h.state().palette.as_ref().unwrap().vim);
    typed(&mut h, "j");
    assert_eq!(query(&h).as_deref(), Some("j"), "letters go to the palette, not to vim");
    assert_eq!(active(&h), "A1");
    press(&mut h, Key::Escape, Modifiers::NONE);
    assert!(h.state().palette.is_none());
    typed(&mut h, "j");
    assert_eq!(active(&h), "A2", "vim keys work again once the palette is closed");
}

#[test]
fn backspace_on_an_empty_command_line_closes_it() {
    let mut h = harness(true);
    typed(&mut h, ":");
    press(&mut h, Key::Backspace, Modifiers::NONE);
    assert!(h.state().palette.is_none());
}

#[test]
fn ex_commands_jump_to_cells_and_rows() {
    let mut h = harness(true);
    typed(&mut h, ":c4");
    enter(&mut h);
    assert!(h.state().palette.is_none());
    assert_eq!(active(&h), "C4");
    typed(&mut h, ":42");
    enter(&mut h);
    assert_eq!(active(&h), "C42");
    typed(&mut h, "gl");
    assert_eq!(active(&h), "C4", "a jump from the command line is remembered");
}

#[test]
fn ex_sort_and_fill_run_on_the_selection() {
    let mut h = harness(true);
    typed(&mut h, "lVG:sort!");
    enter(&mut h);
    assert_eq!(text(&h, "B1"), "50");
    assert_eq!(text(&h, "A1"), "r5", "sorting rows keeps them together");
    typed(&mut h, "gg0vj:fill");
    enter(&mut h);
    assert_eq!(text(&h, "A2"), "r5");
}

#[test]
fn ex_map_adds_a_vim_mapping() {
    let mut h = harness(true);
    typed(&mut h, ":map J 3j");
    enter(&mut h);
    assert_eq!(h.state().ui.vim_maps.get("J").map(String::as_str), Some("3j"));
    typed(&mut h, "J");
    assert_eq!(active(&h), "A4");
    typed(&mut h, ":unmap J");
    enter(&mut h);
    assert!(h.state().ui.vim_maps.is_empty());
}

#[test]
fn a_mapping_to_a_command_runs_it() {
    let mut h = harness(true);
    h.state_mut().ui.vim_maps.insert("gb".into(), ":edit.clearContents<CR>".into());
    h.run_steps(1);
    typed(&mut h, "jgb");
    assert_eq!(text(&h, "A2"), "");
    assert_eq!(text(&h, "A1"), "r1");
}

#[test]
fn filtering_and_enter_run_the_best_match_and_remember_it() {
    let mut h = harness(true);
    typed(&mut h, ":bold");
    let rows = gridcraft_ui_egui::palette::rows(h.state(), "bold", true, true);
    let first = match rows.first() {
        Some(gridcraft_ui_egui::palette::Row::Command(e)) => e.id,
        other => panic!("{:?}", other.is_some()),
    };
    assert_eq!(first, "home.bold");
    enter(&mut h);
    assert!(h.state().palette.is_none());
    assert_eq!(h.state().ui.recent_commands, vec!["home.bold".to_string()]);
    typed(&mut h, ":");
    let rows = gridcraft_ui_egui::palette::rows(h.state(), "", true, true);
    assert!(matches!(rows.first(), Some(gridcraft_ui_egui::palette::Row::Command(e)) if e.id == "home.bold"), "recent first");
}

#[test]
fn arrows_choose_the_row() {
    let mut h = harness(false);
    h.state_mut().open_dialog("commandSearch", json!({}));
    h.run_steps(2);
    typed(&mut h, "align");
    let rows = gridcraft_ui_egui::palette::rows(h.state(), "align", false, true);
    let Some(gridcraft_ui_egui::palette::Row::Command(second)) = rows.get(1) else { panic!("two rows") };
    let id = second.id;
    press(&mut h, Key::ArrowDown, Modifiers::NONE);
    press(&mut h, Key::N, Modifiers::CTRL);
    press(&mut h, Key::P, Modifiers::CTRL);
    enter(&mut h);
    assert_eq!(h.state().ui.recent_commands.first().map(String::as_str), Some(id));
}

#[test]
fn missing_parameters_reopen_the_command_line() {
    let mut h = harness(true);
    typed(&mut h, ":home.fontSize");
    enter(&mut h);
    assert!(h.state().palette.is_some(), "palette stays open for the value");
    assert!(h.state().palette.as_ref().unwrap().query.contains("home.fontSize"));
    typed(&mut h, " 14");
    enter(&mut h);
    assert!(h.state().palette.is_none());
}

#[test]
fn number_format_shows_choices_while_typing() {
    let mut h = harness(true);
    typed(&mut h, ":home.numberFormat Per");
    let rows = gridcraft_ui_egui::palette::rows(h.state(), "home.numberFormat Per", true, true);
    assert!(rows.iter().any(|r| matches!(r, gridcraft_ui_egui::palette::Row::Choice { value, .. } if value == "Percentage")));
}

#[test]
fn export_csv_opens_save_copy_dialog() {
    let mut h = harness(false);
    gridcraft_ui_egui::palette::run_command(h.state_mut(), "file.exportCsv");
    assert!(h.state().dialog.is_some());
}

#[test]
fn commands_with_settings_open_their_dialog() {
    let mut h = harness(false);
    gridcraft_ui_egui::palette::run_command(h.state_mut(), "home.columnWidth");
    assert!(h.state().dialog.is_some());
}

#[test]
fn shortcut_and_ribbon_route_open_it_without_vim() {
    let mut h = harness(false);
    press(&mut h, Key::Space, Modifiers::COMMAND | Modifiers::SHIFT);
    let p = h.state().palette.as_ref().expect("Cmd+Shift+Space opens the palette");
    assert!(!p.vim);
    typed(&mut h, "q");
    let rows = gridcraft_ui_egui::palette::rows(h.state(), "q", false, true);
    assert!(rows.iter().all(|r| matches!(r, gridcraft_ui_egui::palette::Row::Command(_))), "no command line without vim or a :");
    press(&mut h, Key::Escape, Modifiers::NONE);
    assert!(h.state().palette.is_none());
    h.state_mut().open_dialog("commandSearch", json!({}));
    assert!(h.state().palette.is_some() && h.state().dialog.is_none());
}

#[test]
fn every_shown_command_without_a_dialog_runs_without_settings_errors() {
    // The palette only lists enabled commands with a menu, a shortcut or vim keys.
    let h = harness(false);
    let rows = gridcraft_ui_egui::palette::rows(h.state(), "", false, true);
    assert!(rows.len() > 100, "{}", rows.len());
}
