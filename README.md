# PhotoMatic

A native Windows desktop application written in Rust, using [native-windows-gui](https://github.com/gabdube/native-windows-gui) for a classic Win32 look: a real menu bar, native dialogs, and no browser/canvas rendering underneath.

## Features

- **Windowed or full-screen** startup mode, based on a saved user preference.
- **Three-region layout** — a full-width Project Information strip (300px tall) across the top, with a Left Navigation panel (1/5 width) and Context Window (4/5 width) filling the rest. Project Information and Left Navigation use a darker gray background; the Context Window is lighter.
- **Source Directory** — an editable text box plus a `Browse...` folder picker, aligned to the top-left of the Project Information strip. The chosen (or typed) folder is stored in the project file and restored whenever a project is loaded.
- **File Types** — checkboxes for `*.jpg`, `*.CR2`, and `*.gif`, directly below Source Directory, controlling which file extensions are included from that folder. All are checked by default; the selection is stored in the project file and restored whenever a project is loaded.
- **Scan Directory** — a button pinned to the bottom-right of the Project Information strip. Recursively walks the Source Directory on a background thread (so the rest of the GUI stays responsive), counting files matching the enabled File Types. While scanning, the button is disabled and a marquee progress bar above it is shown.
- **Generate MetaData** — a button next to Scan Directory, disabled until a scan has populated the database. Reads each scanned image's EXIF data (date taken, width, height, exposure time, ISO, focal length, GPS coordinates and altitude) on a small pool of background reader threads and writes it to the corresponding `images` row, so a later cataloguing/browsing UI has it available without re-reading files from disk. Only images that haven't been processed yet are read, so re-running it after adding more images (via another Scan Directory) only picks up the new ones; a rescan never wipes previously generated metadata.
- **Event Gaps / Generate Events** — three small numeric inputs (Event Gaps: burst seconds, session minutes, multi-hour hours; default 10/60/8) plus a **Generate Events** button next to Generate MetaData, enabled under the same condition. Clusters every image in the project by `date_taken` at three nested granularities — **Tight Burst**, **Session**, **Multi-hour** — by sorting photos chronologically and starting a new group wherever the gap since the previous photo exceeds that tier's threshold; a photo can belong to up to three groups at once (its burst, its session, and its multi-hour block). Only groups of 2 or more photos become an event; a photo with no neighbor within a tier's threshold simply has no event for that tier. Every run clears and fully rebuilds the results (unlike Generate MetaData's incremental behavior), and runs synchronously since it only touches already-cached data. The Event Gaps values are stored in the project file and restored whenever a project is loaded.
- **Project database** — alongside the JSON project file, a SQLite database (`<name>.sqlite3` next to `<name>.json`) is created on first save and migrated forward automatically every time it's opened. A Scan Directory run writes to it incrementally as the scan progresses (one top-level directory, or the batch of files sitting directly in the Source Directory, at a time) rather than only once the whole tree has been walked. The `directories` table holds only top-level directories (one level below the Source Directory); each `images` row is keyed by a stable hash of its path, carries a foreign key to the top-level directory it lives under (`NULL` for images placed directly in the Source Directory), and (once Generate MetaData has run) its EXIF-derived date taken/width/height/exposure time/ISO/focal length/GPS latitude/longitude/altitude. Rescanning never duplicates existing rows or overwrites any per-directory or per-image metadata a later editing UI adds. Generate Events populates two more tables: `events` (one row per detected group, with its tier and a `notes` column reserved for a later manual-editing UI) and `event_images`, a join table linking each event to its photos.
- **Left Navigation directory tree** — filling the entire Left Navigation panel, a tree view lists every top-level directory from the project database, each expandable into an `ext (n)` child item for every currently-enabled File Type showing that directory's image count (0 when present but empty). A File Type unchecked in the Project Information strip has no child item at all, even if the database still holds images of that type from an earlier scan. It's read entirely from the database and the File Types selection, and rebuilt whenever either changes: when a project with an existing database finishes loading, after every Scan Directory run, and immediately on every File Types checkbox toggle. Right-clicking a directory node or one of its File Type children (`ext (n)`) shows a two-item context menu: **Open in Explorer** (opens that directory in Windows Explorer) and **Image List** (opens/switches to a Context Window tab for it — see below).
- **Context Window tabs — Image List** — choosing **Image List** from the Left Navigation tree's context menu opens a tab in the Context Window, or switches to it if already open. The tab is named after the directory (e.g. `50D`) or, for a File Type child, `directory/type` (e.g. `50D/jpg`). Its content is a table of that directory's images from the database — Path, Date taken, Width, Height, Focal length, ISO, Exposure time, Location, Altitude — with a native vertical scrollbar. Tabs are native Win32 tabs with no per-tab close "x"; close the active tab with Ctrl+W, or right-click any tab's header for a one-item context menu, **Close Tab**, which closes that tab whether or not it's the active one.
- **File menu** — `New` (Ctrl+N) resets the current project; `Open...` (Ctrl+O) loads a project from disk; `Save` (Ctrl+S) and `Save As...` (Ctrl+Shift+S) write it back; `Exit` (Alt+F4) closes the application.
- **Edit menu** — `Settings...` opens a dialog to switch between Windowed and Full screen, with Accept/Cancel. Accept saves the choice to disk and applies it immediately; Cancel discards the change.
- **Help menu** — `About` shows the application name, author, and year.
- **Keyboard shortcuts** — every menu has an Alt-mnemonic access key (e.g. Alt+F for File), and the standard Windows accelerators above are wired for real, not just labeled.
- **Persisted settings** — stored at `%APPDATA%\Tischer\PhotoMatic\config.toml`.

## Prerequisites

- **Rust** (stable, MSVC toolchain) — install via [rustup](https://rustup.rs). The pinned toolchain is in `rust-toolchain.toml`, so `rustup` will fetch it automatically on first build.
- **MSVC linker** — install the "Desktop development with C++" workload from [Visual Studio Build Tools](https://visualstudio.microsoft.com/downloads/#build-tools-for-visual-studio). Without this, the build fails with `linker 'link.exe' not found`.
- **SQLite** — statically bundled into the executable via `rusqlite`'s `bundled` feature, so there's nothing separate to install. The first build compiles SQLite from source using the same MSVC toolchain, so expect a slower first build.

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
| `src/app.rs` | Main window, menu bar, three-region layout, Source Directory/File Types/Event Gaps controls, the Left Navigation tree's context menu (Open in Explorer / Image List), Context Window tab management (open-or-switch, close via Ctrl+W or a right-click tab context menu), Generate Events button wiring, and event routing. |
| `src/project.rs` | The project file (`.json`) format and its load/save, including the `EventThresholds` (Event Gaps) values. Also carries the project database's path, cached schema version, and last-modified timestamp. |
| `src/scan.rs` | Recursive image-file collection grouped into per-top-level-directory `ScanUnit`s (reported via a callback as the walk progresses) and per-extension counting — pure logic, unit-tested independently of the GUI and with no knowledge of the database. |
| `src/exif.rs` | `read_metadata()`: reads an image's EXIF data (date taken, width, height, exposure time, ISO, focal length, GPS latitude/longitude/altitude) off disk into an `ImageMetadata` — pure file I/O, no knowledge of the GUI or the database; unreadable files or files with no EXIF simply come back all-`None`. |
| `src/events.rs` | `cluster_by_time_gap()`: groups images by `date_taken`, starting a new group wherever the gap since the previous photo exceeds a given threshold — pure logic, unit-tested independently of the GUI and the database, reused by `src/db/events.rs` at all three Generate Events tiers. |
| `src/db/mod.rs` | `ProjectDb`: opens/migrates the project's SQLite database and folds each `ScanUnit` into it as the scan discovers it (`apply_scan_unit`), stamping `last_scan` once the scan finishes (`finish_scan`); also lists images still missing EXIF metadata (`list_images_pending_metadata`), writes it back (`update_image_metadata`), lists images by directory/type for Context Window tabs (`list_images_by_directory`), and clears/rebuilds the events tables (`regenerate_events`). |
| `src/db/migrations.rs` | The embedded migration list and `apply()`, run via `rusqlite_migration` against `PRAGMA user_version`. |
| `src/db/models.rs` | `ProjectSettings`, `ImageRecord` (including its `toplevel_dir` foreign key and EXIF metadata columns — date/dimensions/exposure/ISO/focal length plus GPS latitude/longitude/altitude — and `metadata_read_at` marking whether extraction has been attempted), `DirectoryRecord`, `EventType`/`EventRecord` — plain structs mirroring the database's tables. |
| `src/db/project_settings.rs` | Get/update queries for the singleton `project_settings` row. |
| `src/db/images.rs` | Upsert/list queries for the `images` table (including its EXIF metadata columns, GPS latitude/longitude/altitude among them), the pending-metadata selection query, the metadata update query, `list_images_by_directory()` (by top-level directory and optional File Type, for Context Window tabs), and the pure `image_key()` (xxh3) hash function. |
| `src/db/directories.rs` | Upsert/list queries for the `directories` table, plus `update_directory_metadata()` for a later editing UI. |
| `src/db/events.rs` | `regenerate()`: clears the `events`/`event_images` tables and rebuilds them from the current images, clustered at all three tiers via `crate::events::cluster_by_time_gap` — backs the Generate Events button. |
| `src/nav_tree.rs` | Builds the Left Navigation tree's data (one node per top-level directory, with a count per currently-enabled File Type) from `list_directories()`/`directory_type_counts()` output plus the project's `FileExtensions` selection, plus `parse_type_label()` (extracts a File Type node's extension back out of its `"ext (n)"` label) — pure logic, unit-tested independently of NWG and SQLite. |
| `src/context_tabs.rs` | Pure Context Window tab logic: `tab_title()` (directory or `"dir/type"` tab name) and `image_row()` (an `ImageRecord` as the 9 display strings shown in an Image List tab's table, including formatted GPS location/altitude) — unit-tested independently of NWG and SQLite. |
| `src/explorer.rs` | Resolves a `directories.dir_name` back to a native path under the Source Directory, and launches Windows Explorer at it (used by the tree's "Open in Explorer" context menu item). |
| `src/settings.rs` | Config load/save (`%APPDATA%\Tischer\PhotoMatic\config.toml`). |
| `src/settings_modal.rs` | The Settings dialog (runs on its own thread, per NWG's recommended dialog pattern). |
| `src/about_modal.rs` | The About message box. |
| `src/panel_background.rs` | Paints a solid background color on an `nwg::Frame` (used for the Project Information / Left Navigation / Context Window panels). |
| `src/shortcuts.rs` | Maps Ctrl-key combinations to menu actions (Ctrl+N/O/S/Shift+S) and to closing the active Context Window tab (Ctrl+W), independent of NWG's event system. |
| `src/window_mode.rs` | Applies windowed vs. full-screen (maximized) mode to the main window. |
| `build.rs` | Embeds a Windows application manifest (required for native-windows-gui's control subclassing to work at all). |
