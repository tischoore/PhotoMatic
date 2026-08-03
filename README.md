# PhotoMatic

A native Windows desktop application written in Rust, using [native-windows-gui](https://github.com/gabdube/native-windows-gui) for a classic Win32 look: a real menu bar, native dialogs, and no browser/canvas rendering underneath.

## Features

- **Windowed or full-screen** startup mode, based on a saved user preference.
- **Three-region layout** — a full-width Project Information strip (300px tall) across the top, with a Left Navigation panel (1/5 width) and Context Window (4/5 width) filling the rest. Project Information and Left Navigation use a darker gray background; the Context Window is lighter.
- **Source Directory** — an editable text box plus a `Browse...` folder picker, aligned to the top-left of the Project Information strip. The chosen (or typed) folder is stored in the project file and restored whenever a project is loaded.
- **File Types** — checkboxes for `*.jpg`, `*.CR2`, and `*.gif`, directly below Source Directory, controlling which file extensions are included from that folder. All are checked by default; the selection is stored in the project file and restored whenever a project is loaded.
- **Scan Directory** — a button pinned to the bottom-right of the Project Information strip. Recursively walks the Source Directory on a background thread (so the rest of the GUI stays responsive), counting files matching the enabled File Types. While scanning, the button is disabled and a marquee progress bar above it is shown. When the scan finishes, a line per enabled file type (with its count) plus the elapsed time is added to a 10-line, read-only scan log at the bottom of the Left Navigation panel.
- **File menu** — `New` (Ctrl+N) resets the current project; `Open...` (Ctrl+O) loads a project from disk; `Save` (Ctrl+S) and `Save As...` (Ctrl+Shift+S) write it back; `Exit` (Alt+F4) closes the application.
- **Edit menu** — `Settings...` opens a dialog to switch between Windowed and Full screen, with Accept/Cancel. Accept saves the choice to disk and applies it immediately; Cancel discards the change.
- **Help menu** — `About` shows the application name, author, and year.
- **Keyboard shortcuts** — every menu has an Alt-mnemonic access key (e.g. Alt+F for File), and the standard Windows accelerators above are wired for real, not just labeled.
- **Persisted settings** — stored at `%APPDATA%\Tischer\PhotoMatic\config.toml`.

## Prerequisites

- **Rust** (stable, MSVC toolchain) — install via [rustup](https://rustup.rs). The pinned toolchain is in `rust-toolchain.toml`, so `rustup` will fetch it automatically on first build.
- **MSVC linker** — install the "Desktop development with C++" workload from [Visual Studio Build Tools](https://visualstudio.microsoft.com/downloads/#build-tools-for-visual-studio). Without this, the build fails with `linker 'link.exe' not found`.

## Build and run

From a terminal, in the project root:

```powershell
cargo build          # compile
cargo run             # compile (if needed) and launch target\debug\photomatic.exe
cargo build --release # optimized build, output in target\release\photomatic.exe
```

### From VS Code

Open this folder in VS Code and press **F5** to build and launch under the debugger. This requires the [CodeLLDB](https://marketplace.visualstudio.com/items?itemName=vadimcn.vscode-lldb) extension (VS Code will prompt to install it if missing). Use `Ctrl+Shift+B` to just build without debugging.

## Testing

```powershell
cargo test
```

See `CLAUDE.md` for this project's testing convention (every feature must ship with its own independent test).

## Project layout

| File | Purpose |
|---|---|
| `src/main.rs` | Entry point: initializes NWG, loads config, builds the UI, runs the event loop. |
| `src/app.rs` | Main window, menu bar, three-region layout, Source Directory and File Types controls, and event routing. |
| `src/project.rs` | The project file (`.json`) format and its load/save. |
| `src/scan.rs` | Recursive image-file counting by extension, and scan-summary/log formatting — pure logic, unit-tested independently of the GUI. |
| `src/settings.rs` | Config load/save (`%APPDATA%\Tischer\PhotoMatic\config.toml`). |
| `src/settings_modal.rs` | The Settings dialog (runs on its own thread, per NWG's recommended dialog pattern). |
| `src/about_modal.rs` | The About message box. |
| `src/panel_background.rs` | Paints a solid background color on an `nwg::Frame` (used for the Project Information / Left Navigation / Context Window panels). |
| `src/shortcuts.rs` | Maps Ctrl-key combinations to menu actions (Ctrl+N/O/S/Shift+S), independent of NWG's event system. |
| `src/window_mode.rs` | Applies windowed vs. full-screen (maximized) mode to the main window. |
| `build.rs` | Embeds a Windows application manifest (required for native-windows-gui's control subclassing to work at all). |
