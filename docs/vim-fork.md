# Vim mode and command palette (fork maintenance)

This fork adds **vim mode** (`crates/ui-egui/src/vim.rs`, `vim_text.rs`) and the **command
palette** (`crates/ui-egui/src/palette.rs`). Upstream command names and dialogs can change;
`cargo test -p gridcraft-ui-egui --test fork_drift` catches most drift after you merge
`upstream/main`.

## After merging upstream

1. Run `cargo xtask ci` (and `cargo test -p gridcraft-ui-egui --test fork_drift`).
2. If **fork_drift** fails:
   - **`vim::VIM_KEYS`**: each `(command id, key hint)` must match a real `CommandSpec` id. Add or
     remove rows in `vim.rs` when upstream renames commands. User mappings to `:id<CR>` are
     discovered automatically in the palette.
   - **`palette::DIALOGS`**: each `(command id, dialog name, default JSON)` must reference a real
     command and a dialog opened in `dialogs::Dialog::open`. Add a row when a palette-visible
     command needs a dialog and does not open one from the engine alone.
3. Try the palette: `:` → type a few renamed commands; fix `run_ex` aliases (`:sort`, `:w`, …) if
   file or view commands moved.
4. Update **README.md** vim table if keys or behaviour changed.
5. Rebuild **gridcraft-eilif** (`~/.local/bin/gridcraft-eilif-build`).

## What updates automatically

- The palette list is built from `Session::commands()` every frame: new commands, labels, menu
  paths and ribbon shortcuts appear without editing `palette.rs`.
- `:any.engine.id` and `:any.engine.id {"key": "value"}` work for any id that parses as a command.

## Manual tables (do not forget)

| Location | Purpose |
|----------|---------|
| `vim::VIM_KEYS` | Shortcut hints shown in the palette next to commands |
| `palette::DIALOGS` | Open a settings dialog instead of running bare `command` |
| `palette::parse_ex` / `run_ex` | Vim-style `:w`, `:sort`, `:map`, cell addresses |
| `palette::Ex` | Command-line verbs not equal to a single engine id |

## UI entry points

- Vim: `view.vimMode` → `UiState.vim_mode`; maps in `UiState.vim_maps`; recents in
  `recent_commands`.
- Palette: `SheetApp.palette`, ⌘⇧Space in `grid.rs`, `:` → `Action::Palette`, ribbon search →
  `open_dialog("commandSearch")` routes to `Palette::new(false)`.
- Cheat sheet: `vim_cheatsheet.rs`, `?` → `Action::CheatSheet`; static list in `BINDINGS` plus user
  `:map` entries. Update when adding keys.

## Tests worth running

```bash
cargo test -p gridcraft-ui-egui --test fork_drift
cargo test -p gridcraft-ui-egui --test palette --test vim
```
