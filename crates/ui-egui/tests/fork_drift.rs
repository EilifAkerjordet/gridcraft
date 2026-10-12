//! Catches drift between upstream GridCraft commands and the vim-mode / palette fork.
//!
//! Run with `cargo test -p gridcraft-ui-egui --test fork_drift`. Fails when upstream renames or
//! removes a command id referenced in `palette::DIALOGS` or `vim::VIM_KEYS`, or when a dialog name
//! no longer exists. See `docs/vim-fork.md` for what to update after merging upstream.

use gridcraft_engine::Session;
use gridcraft_ui_egui::palette::{DIALOGS, choices};
use gridcraft_ui_egui::vim::VIM_KEYS;

fn command_ids() -> std::collections::HashSet<&'static str> {
    let mut s = Session::new();
    s.new_workbook();
    s.commands().into_iter().map(|c| c.id).collect()
}

fn dialog_names() -> std::collections::HashSet<&'static str> {
    [
        "about",
        "columnWidth",
        "dataValidation",
        "defaultWidth",
        "deleteCells",
        "find",
        "formatCells",
        "goTo",
        "goToSpecial",
        "goalSeek",
        "headerFooter",
        "insertCells",
        "insertFunction",
        "insertLink",
        "manageRules",
        "moveSheet",
        "nameManager",
        "pageSetup",
        "pasteSpecial",
        "protectSheet",
        "removeDuplicates",
        "renameSheet",
        "rowHeight",
        "saveCopy",
        "series",
        "sort",
        "sparkline",
        "subtotal",
        "textToColumns",
        "unhideSheet",
        "unprotectSheet",
        "zoom",
    ]
    .into_iter()
    .collect()
}

#[test]
fn vim_keys_reference_real_commands() {
    let ids = command_ids();
    let missing: Vec<_> = VIM_KEYS.iter().filter(|(id, _)| !ids.contains(id)).map(|(id, keys)| format!("{id} ({keys})")).collect();
    assert!(missing.is_empty(), "vim::VIM_KEYS ids missing from the engine (update vim.rs or remove the row):\n{}", missing.join("\n"));
}

#[test]
fn palette_dialogs_reference_real_commands_and_dialogs() {
    let ids = command_ids();
    let dialogs = dialog_names();
    let mut bad_cmd = Vec::new();
    let mut bad_dialog = Vec::new();
    for (cmd, dialog, _params) in DIALOGS {
        if !ids.contains(cmd) {
            bad_cmd.push(format!("{cmd} → dialog {dialog}"));
        }
        if !dialogs.contains(dialog) {
            bad_dialog.push(format!("{cmd} → unknown dialog `{dialog}` (check dialogs::Dialog::open in dialogs.rs)"));
        }
        if serde_json::from_str::<serde_json::Value>(_params).is_err() {
            bad_dialog.push(format!("{cmd}: invalid default params JSON: {_params}"));
        }
    }
    assert!(bad_cmd.is_empty(), "palette::DIALOGS command ids missing from the engine:\n{}", bad_cmd.join("\n"));
    assert!(bad_dialog.is_empty(), "palette::DIALOGS dialog problems:\n{}", bad_dialog.join("\n"));
}

#[test]
fn ex_command_aliases_still_exist() {
    let ids = command_ids();
    for id in [
        "file.save",
        "file.close",
        "file.open",
        "data.sortAscending",
        "data.sortDescending",
        "edit.fillDown",
        "edit.fillRight",
        "view.freezePanes",
        "view.unfreezePanes",
    ] {
        assert!(ids.contains(id), "Ex command line alias target `{id}` is missing; update palette::run_ex");
    }
}

#[test]
fn choice_lists_parse_from_engine_descriptions() {
    let mut s = Session::new();
    s.new_workbook();
    let fmt = s.commands().into_iter().find(|c| c.id == "home.numberFormat").map(|c| c.params).expect("home.numberFormat");
    let ch = choices(fmt, "format");
    assert!(ch.len() >= 5, "expected General|Number|… in numberFormat params, got {ch:?}");
    let go = s.commands().into_iter().find(|c| c.id == "edit.goToSpecial").map(|c| c.params).expect("edit.goToSpecial");
    assert!(!choices(go, "kind").is_empty());
}
