# PhotoMatic

A native Windows desktop application written in Rust, using [native-windows-gui](https://github.com/gabdube/native-windows-gui) for a classic Win32 look: a real menu bar, native dialogs, and no browser/canvas rendering underneath.

## Features

- **Windowed or full-screen** startup mode, based on a saved user preference.
- **File menu** — `Exit` closes the application.
- **Edit menu** — `Settings` opens a dialog to switch between Windowed and Full screen, with Accept/Cancel. Accept saves the choice to disk and applies it immediately; Cancel discards the change.
- **Help menu** — `About` shows the application name, author, and year.
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
| `src/app.rs` | Main window, menu bar, and event routing. |
| `src/settings.rs` | Config load/save (`%APPDATA%\Tischer\PhotoMatic\config.toml`). |
| `src/settings_modal.rs` | The Settings dialog (runs on its own thread, per NWG's recommended dialog pattern). |
| `src/about_modal.rs` | The About message box. |
| `src/window_mode.rs` | Applies windowed vs. full-screen (maximized) mode to the main window. |
| `build.rs` | Embeds a Windows application manifest (required for native-windows-gui's control subclassing to work at all). |
