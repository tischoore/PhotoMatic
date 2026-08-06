use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;

use native_windows_derive::NwgUi;
use native_windows_gui as nwg;
use nwg::stretch::geometry::{Rect, Size};
use nwg::stretch::style::{AlignItems, Dimension as D, FlexDirection, JustifyContent};

use crate::db;
use crate::exif;
use crate::explorer;
use crate::nav_tree;
use crate::panel_background;
use crate::project::{self, FileExtensions, ProjectFile};
use crate::scan;
use crate::settings::{self, AppConfig};
use crate::settings_modal;
use crate::shortcuts::{self, ShortcutAction};
use crate::window_mode;

const PROJECT_FILTER: &str = "PhotoMatic Project(*.json)";

/// Height of the Project Information strip, in points (`FlexboxLayout` units).
const PROJECT_INFO_HEIGHT: f32 = 150.0;
/// Width of the Left Navigation panel, as a fraction of the space below Project Information.
const NAV_WIDTH_PERCENT: f32 = 0.2;
/// Width of the Context Window, as a fraction of the space below Project Information.
const CONTEXT_WIDTH_PERCENT: f32 = 0.8;

/// Background color shared by Project Information and Left Navigation.
const DARK_PANEL_COLOR: [u8; 3] = [214, 214, 214];
/// Background color of the Context Window.
const LIGHT_PANEL_COLOR: [u8; 3] = [245, 245, 245];

/// Sizes for the Source Directory row's controls. `stretch` (the flex engine `FlexboxLayout`
/// is built on) has no way to measure a Win32 control's natural size, so every leaf control in
/// a flex layout needs an explicit width *and* height — a `Dimension::Auto` on a control with
/// no children and no measure function resolves to zero, not to its natural/content size.
const SOURCE_DIR_ROW_HEIGHT: f32 = 24.0;
const SOURCE_DIR_LABEL_WIDTH: f32 = 110.0;
const SOURCE_DIR_INPUT_WIDTH: f32 = 500.0;
const SOURCE_DIR_BROWSE_WIDTH: f32 = 90.0;

/// Sizes for the File Types row's controls, directly below the Source Directory row.
const FILE_TYPES_ROW_HEIGHT: f32 = 24.0;
const FILE_TYPES_LABEL_WIDTH: f32 = 110.0;
const FILE_TYPE_CHECKBOX_WIDTH: f32 = 80.0;
/// Vertical gap between the Source Directory and File Types rows.
const PROJECT_INFO_ROW_GAP: f32 = 8.0;

/// Sizes for the Scan Directory area, pinned to the bottom-right of Project Information.
const SCAN_BUTTON_WIDTH: f32 = 160.0;
const SCAN_BUTTON_HEIGHT: f32 = 30.0;
const SCAN_PROGRESS_HEIGHT: f32 = 8.0;
/// Vertical gap between the progress bar and the Scan Directory/Generate MetaData button.
const SCAN_AREA_GAP: f32 = 6.0;
/// Width of the Generate MetaData button, next to Scan Directory.
const METADATA_BUTTON_WIDTH: f32 = SCAN_BUTTON_WIDTH;
/// Horizontal gap between the Scan Directory and Generate MetaData columns.
const SCAN_AREA_COLUMN_GAP: f32 = 12.0;

/// Whether the given virtual key (e.g. `VK_CONTROL`) is currently held down.
fn key_down(vk: winapi::ctypes::c_int) -> bool {
    unsafe { winapi::um::winuser::GetKeyState(vk) & 0x8000u16 as i16 != 0 }
}

/// Finds the tree item under the current cursor position, if any. `nwg::TreeView`
/// exposes no "item under a point" query of its own, so this sends `TVM_HITTEST`
/// directly to the control's `HWND` — the same low-level-winapi-call pattern
/// `key_down` above uses for modifier keys, needed here because right-clicking a
/// Win32 tree view doesn't move its selection the way a left click does.
fn tree_hit_test_at_cursor(tree: &nwg::TreeView) -> Option<nwg::TreeItem> {
    use winapi::shared::windef::POINT;
    use winapi::um::commctrl::{TVHITTESTINFO, TVHT_ONITEM, TVM_HITTEST};

    let hwnd = tree.handle.hwnd()?;
    let (x, y) = nwg::GlobalCursor::local_position(tree, None);

    let mut hit_test = TVHITTESTINFO { pt: POINT { x, y }, flags: 0, hItem: std::ptr::null_mut() };
    unsafe {
        winapi::um::winuser::SendMessageW(
            hwnd,
            TVM_HITTEST,
            0,
            &mut hit_test as *mut TVHITTESTINFO as isize,
        );
    }

    if hit_test.hItem.is_null() || hit_test.flags & TVHT_ONITEM == 0 {
        None
    } else {
        Some(nwg::TreeItem { handle: hit_test.hItem })
    }
}

#[derive(Default, NwgUi)]
pub struct App {
    config: RefCell<AppConfig>,
    settings_thread: RefCell<Option<thread::JoinHandle<Option<bool>>>>,
    project: RefCell<ProjectFile>,
    current_project_path: RefCell<Option<PathBuf>>,
    scan_thread: RefCell<Option<thread::JoinHandle<(scan::ScanResult, Option<db::ProjectDb>)>>>,
    metadata_thread: RefCell<Option<thread::JoinHandle<Option<db::ProjectDb>>>>,
    db: RefCell<Option<db::ProjectDb>>,
    db_open_thread: RefCell<Option<thread::JoinHandle<Result<db::ProjectDb, db::DbError>>>>,
    /// The top-level directory right-clicked in `nav_tree`, remembered between
    /// `nav_tree_right_click` (which shows `nav_tree_menu`) and
    /// `open_selected_dir_in_explorer` (the menu's only item).
    nav_tree_context_dir: RefCell<Option<String>>,

    #[nwg_control(title: "PhotoMatic", flags: "MAIN_WINDOW")]
    #[nwg_events(OnWindowClose: [App::exit], OnKeyPress: [App::on_key_press(SELF, EVT_DATA)])]
    window: nwg::Window,

    #[nwg_control]
    #[nwg_events(OnNotice: [App::settings_dialog_closed])]
    settings_notice: nwg::Notice,

    #[nwg_control]
    #[nwg_events(OnNotice: [App::scan_finished])]
    scan_notice: nwg::Notice,

    #[nwg_control]
    #[nwg_events(OnNotice: [App::generate_metadata_finished])]
    metadata_notice: nwg::Notice,

    #[nwg_control]
    #[nwg_events(OnNotice: [App::db_open_finished])]
    db_notice: nwg::Notice,

    #[nwg_control(parent: window, text: "&File")]
    file_menu: nwg::Menu,

    #[nwg_control(parent: file_menu, text: "&New\tCtrl+N")]
    #[nwg_events(OnMenuItemSelected: [App::file_new])]
    file_new: nwg::MenuItem,

    #[nwg_control(parent: file_menu, text: "&Open...\tCtrl+O")]
    #[nwg_events(OnMenuItemSelected: [App::file_load])]
    file_load: nwg::MenuItem,

    #[nwg_control(parent: file_menu, text: "&Save\tCtrl+S")]
    #[nwg_events(OnMenuItemSelected: [App::file_save])]
    file_save: nwg::MenuItem,

    #[nwg_control(parent: file_menu, text: "Save &As...\tCtrl+Shift+S")]
    #[nwg_events(OnMenuItemSelected: [App::file_save_as])]
    file_save_as: nwg::MenuItem,

    #[nwg_control(parent: file_menu)]
    file_menu_sep: nwg::MenuSeparator,

    #[nwg_control(parent: file_menu, text: "E&xit\tAlt+F4")]
    #[nwg_events(OnMenuItemSelected: [App::exit])]
    file_exit: nwg::MenuItem,

    #[nwg_control(parent: window, text: "&Edit")]
    edit_menu: nwg::Menu,

    #[nwg_control(parent: edit_menu, text: "&Settings...")]
    #[nwg_events(OnMenuItemSelected: [App::open_settings])]
    edit_settings: nwg::MenuItem,

    #[nwg_control(parent: window, text: "&Help")]
    help_menu: nwg::Menu,

    #[nwg_control(parent: help_menu, text: "&About")]
    #[nwg_events(OnMenuItemSelected: [App::show_about])]
    help_about: nwg::MenuItem,

    #[nwg_control(parent: window, flags: "VISIBLE")]
    project_info_frame: nwg::Frame,

    #[nwg_control(parent: window, flags: "VISIBLE")]
    nav_frame: nwg::Frame,

    #[nwg_control(parent: window, flags: "VISIBLE")]
    context_frame: nwg::Frame,

    #[nwg_control(parent: project_info_frame, text: "Source Directory:")]
    source_dir_label: nwg::Label,

    #[nwg_control(parent: project_info_frame, text: "")]
    source_dir_input: nwg::TextInput,

    #[nwg_control(parent: project_info_frame, text: "&Browse...")]
    #[nwg_events(OnButtonClick: [App::browse_source_directory])]
    source_dir_browse: nwg::Button,

    #[nwg_control(parent: project_info_frame, text: "File Types:")]
    file_types_label: nwg::Label,

    #[nwg_control(parent: project_info_frame, text: "*.&jpg", check_state: nwg::CheckBoxState::Checked)]
    #[nwg_events(OnButtonClick: [App::sync_file_extensions_from_checkboxes])]
    file_type_jpg: nwg::CheckBox,

    #[nwg_control(parent: project_info_frame, text: "*.C&R2", check_state: nwg::CheckBoxState::Checked)]
    #[nwg_events(OnButtonClick: [App::sync_file_extensions_from_checkboxes])]
    file_type_cr2: nwg::CheckBox,

    #[nwg_control(parent: project_info_frame, text: "*.&gif", check_state: nwg::CheckBoxState::Unchecked)]
    #[nwg_events(OnButtonClick: [App::sync_file_extensions_from_checkboxes])]
    file_type_gif: nwg::CheckBox,

    #[nwg_control(parent: project_info_frame, flags: "MARQUEE", marquee: true, marquee_update: 30)]
    scan_progress: nwg::ProgressBar,

    #[nwg_control(parent: project_info_frame, text: "&Scan Directory")]
    #[nwg_events(OnButtonClick: [App::start_scan])]
    scan_button: nwg::Button,

    #[nwg_control(parent: project_info_frame, flags: "MARQUEE", marquee: true, marquee_update: 30)]
    metadata_progress: nwg::ProgressBar,

    #[nwg_control(parent: project_info_frame, text: "&Generate MetaData")]
    #[nwg_events(OnButtonClick: [App::start_generate_metadata])]
    metadata_button: nwg::Button,

    #[nwg_control(parent: nav_frame, flags: "VISIBLE")]
    #[nwg_events(OnTreeViewRightClick: [App::nav_tree_right_click])]
    nav_tree: nwg::TreeView,

    #[nwg_control(parent: window, popup: true)]
    nav_tree_menu: nwg::Menu,

    #[nwg_control(parent: nav_tree_menu, text: "&Open in Explorer")]
    #[nwg_events(OnMenuItemSelected: [App::open_selected_dir_in_explorer])]
    nav_tree_menu_open_explorer: nwg::MenuItem,

    body_layout: nwg::FlexboxLayout,
    root_layout: nwg::FlexboxLayout,
    project_info_layout: nwg::FlexboxLayout,
    source_dir_layout: nwg::FlexboxLayout,
    file_types_layout: nwg::FlexboxLayout,
    scan_area_layout: nwg::FlexboxLayout,
    scan_column_layout: nwg::FlexboxLayout,
    metadata_column_layout: nwg::FlexboxLayout,
    nav_layout: nwg::FlexboxLayout,
}

impl App {
    pub fn init(&self, config: AppConfig) {
        window_mode::apply(&self.window, config.fullscreen);
        *self.config.borrow_mut() = config;
        self.build_layout();
        panel_background::paint(&self.project_info_frame, DARK_PANEL_COLOR);
        panel_background::paint(&self.nav_frame, DARK_PANEL_COLOR);
        panel_background::paint(&self.context_frame, LIGHT_PANEL_COLOR);
        self.refresh_metadata_button_enabled();
        self.window.set_visible(true);
    }

    /// Builds the three-region layout (Project Information / Left Navigation / Context Window)
    /// and the Source Directory control row inside Project Information. See `flexbox_sub_layout`
    /// in the `native-windows-gui` examples for the nesting pattern this follows: a sub-layout
    /// is attached with `child_layout` and targets the same parent as the layout that nests it —
    /// nesting is a relationship between `FlexboxLayout`s, not between the windows they position.
    fn build_layout(&self) {
        nwg::FlexboxLayout::builder()
            .parent(&self.window)
            .flex_direction(FlexDirection::Row)
            .child(&self.nav_frame)
            .child_size(Size { width: D::Percent(NAV_WIDTH_PERCENT), height: D::Auto })
            .child(&self.context_frame)
            .child_size(Size { width: D::Percent(CONTEXT_WIDTH_PERCENT), height: D::Auto })
            .build_partial(&self.body_layout)
            .expect("Failed to build the nav/context layout");

        nwg::FlexboxLayout::builder()
            .parent(&self.window)
            .flex_direction(FlexDirection::Column)
            .child(&self.project_info_frame)
            .child_size(Size { width: D::Auto, height: D::Points(PROJECT_INFO_HEIGHT) })
            .child_layout(&self.body_layout)
            .child_size(Size { width: D::Auto, height: D::Auto })
            .child_flex_grow(1.0)
            .build(&self.root_layout)
            .expect("Failed to build the root layout");

        let row_margin = Rect { start: D::Points(0.0), end: D::Points(8.0), top: D::Points(0.0), bottom: D::Points(0.0) };
        nwg::FlexboxLayout::builder()
            .parent(&self.project_info_frame)
            .flex_direction(FlexDirection::Row)
            .justify_content(JustifyContent::FlexStart)
            .align_items(AlignItems::FlexStart)
            .child(&self.source_dir_label)
            .child_size(Size { width: D::Points(SOURCE_DIR_LABEL_WIDTH), height: D::Points(SOURCE_DIR_ROW_HEIGHT) })
            .child_margin(row_margin)
            .child(&self.source_dir_input)
            .child_size(Size { width: D::Points(SOURCE_DIR_INPUT_WIDTH), height: D::Points(SOURCE_DIR_ROW_HEIGHT) })
            .child_margin(row_margin)
            .child(&self.source_dir_browse)
            .child_size(Size { width: D::Points(SOURCE_DIR_BROWSE_WIDTH), height: D::Points(SOURCE_DIR_ROW_HEIGHT) })
            .build(&self.source_dir_layout)
            .expect("Failed to build the Source Directory row layout");

        nwg::FlexboxLayout::builder()
            .parent(&self.project_info_frame)
            .flex_direction(FlexDirection::Row)
            .justify_content(JustifyContent::FlexStart)
            .align_items(AlignItems::FlexStart)
            .child(&self.file_types_label)
            .child_size(Size { width: D::Points(FILE_TYPES_LABEL_WIDTH), height: D::Points(FILE_TYPES_ROW_HEIGHT) })
            .child_margin(row_margin)
            .child(&self.file_type_jpg)
            .child_size(Size { width: D::Points(FILE_TYPE_CHECKBOX_WIDTH), height: D::Points(FILE_TYPES_ROW_HEIGHT) })
            .child_margin(row_margin)
            .child(&self.file_type_cr2)
            .child_size(Size { width: D::Points(FILE_TYPE_CHECKBOX_WIDTH), height: D::Points(FILE_TYPES_ROW_HEIGHT) })
            .child_margin(row_margin)
            .child(&self.file_type_gif)
            .child_size(Size { width: D::Points(FILE_TYPE_CHECKBOX_WIDTH), height: D::Points(FILE_TYPES_ROW_HEIGHT) })
            .build(&self.file_types_layout)
            .expect("Failed to build the File Types row layout");

        // Progress bar above each button, both columns pinned to the bottom-right corner
        // of Project Information via `justify_content`/`align_items: FlexEnd`.
        let scan_area_margin = Rect { start: D::Points(0.0), end: D::Points(0.0), top: D::Points(0.0), bottom: D::Points(SCAN_AREA_GAP) };
        nwg::FlexboxLayout::builder()
            .parent(&self.project_info_frame)
            .flex_direction(FlexDirection::Column)
            .justify_content(JustifyContent::FlexEnd)
            .align_items(AlignItems::FlexEnd)
            .child(&self.scan_progress)
            .child_size(Size { width: D::Points(SCAN_BUTTON_WIDTH), height: D::Points(SCAN_PROGRESS_HEIGHT) })
            .child_margin(scan_area_margin)
            .child(&self.scan_button)
            .child_size(Size { width: D::Points(SCAN_BUTTON_WIDTH), height: D::Points(SCAN_BUTTON_HEIGHT) })
            .build_partial(&self.scan_column_layout)
            .expect("Failed to build the Scan Directory column layout");

        nwg::FlexboxLayout::builder()
            .parent(&self.project_info_frame)
            .flex_direction(FlexDirection::Column)
            .justify_content(JustifyContent::FlexEnd)
            .align_items(AlignItems::FlexEnd)
            .child(&self.metadata_progress)
            .child_size(Size { width: D::Points(METADATA_BUTTON_WIDTH), height: D::Points(SCAN_PROGRESS_HEIGHT) })
            .child_margin(scan_area_margin)
            .child(&self.metadata_button)
            .child_size(Size { width: D::Points(METADATA_BUTTON_WIDTH), height: D::Points(SCAN_BUTTON_HEIGHT) })
            .build_partial(&self.metadata_column_layout)
            .expect("Failed to build the Generate MetaData column layout");

        let scan_area_column_margin =
            Rect { start: D::Points(0.0), end: D::Points(SCAN_AREA_COLUMN_GAP), top: D::Points(0.0), bottom: D::Points(0.0) };
        nwg::FlexboxLayout::builder()
            .parent(&self.project_info_frame)
            .flex_direction(FlexDirection::Row)
            .justify_content(JustifyContent::FlexEnd)
            .align_items(AlignItems::FlexEnd)
            .child_layout(&self.scan_column_layout)
            .child_size(Size { width: D::Points(SCAN_BUTTON_WIDTH), height: D::Auto })
            .child_margin(scan_area_column_margin)
            .child_layout(&self.metadata_column_layout)
            .child_size(Size { width: D::Points(METADATA_BUTTON_WIDTH), height: D::Auto })
            .build_partial(&self.scan_area_layout)
            .expect("Failed to build the Scan Directory area layout");

        let rows_margin = Rect { start: D::Points(0.0), end: D::Points(0.0), top: D::Points(0.0), bottom: D::Points(PROJECT_INFO_ROW_GAP) };
        nwg::FlexboxLayout::builder()
            .parent(&self.project_info_frame)
            .flex_direction(FlexDirection::Column)
            .padding(Rect { start: D::Points(12.0), end: D::Points(12.0), top: D::Points(12.0), bottom: D::Points(12.0) })
            .child_layout(&self.source_dir_layout)
            .child_size(Size { width: D::Auto, height: D::Points(SOURCE_DIR_ROW_HEIGHT) })
            .child_margin(rows_margin)
            .child_layout(&self.file_types_layout)
            .child_size(Size { width: D::Auto, height: D::Points(FILE_TYPES_ROW_HEIGHT) })
            .child_margin(rows_margin)
            .child_layout(&self.scan_area_layout)
            .child_size(Size { width: D::Auto, height: D::Auto })
            .child_flex_grow(1.0)
            .build(&self.project_info_layout)
            .expect("Failed to build the Project Information column layout");

        // Directory tree fills the entire Left Navigation panel.
        nwg::FlexboxLayout::builder()
            .parent(&self.nav_frame)
            .padding(Rect { start: D::Points(8.0), end: D::Points(8.0), top: D::Points(8.0), bottom: D::Points(8.0) })
            .child(&self.nav_tree)
            .child_size(Size { width: D::Percent(1.0), height: D::Percent(1.0) })
            .build(&self.nav_layout)
            .expect("Failed to build the Left Navigation layout");
    }

    fn exit(&self) {
        nwg::stop_thread_dispatch();
    }

    /// Dispatches the Ctrl-key accelerators declared in the File menu (Ctrl+N/O/S/Shift+S).
    /// The virtual-key code comes from the event; modifier state isn't part of `OnKeyPress`, so
    /// it's read directly from Win32.
    fn on_key_press(&self, data: &nwg::EventData) {
        let ctrl = key_down(winapi::um::winuser::VK_CONTROL);
        let shift = key_down(winapi::um::winuser::VK_SHIFT);

        match shortcuts::resolve(data.on_key(), ctrl, shift) {
            Some(ShortcutAction::New) => self.file_new(),
            Some(ShortcutAction::Open) => self.file_load(),
            Some(ShortcutAction::Save) => self.file_save(),
            Some(ShortcutAction::SaveAs) => self.file_save_as(),
            None => {}
        }
    }

    fn file_new(&self) {
        *self.project.borrow_mut() = ProjectFile::default();
        *self.current_project_path.borrow_mut() = None;
        *self.db.borrow_mut() = None;
        self.source_dir_input.set_text("");
        self.set_file_type_checkboxes(&FileExtensions::default());
        self.nav_tree.clear();
        self.refresh_metadata_button_enabled();
    }

    fn file_load(&self) {
        let mut dialog = nwg::FileDialog::default();
        if !self.build_project_dialog(&mut dialog, "Load PhotoMatic Project", nwg::FileDialogAction::Open) {
            return;
        }
        if !dialog.run(Some(&self.window)) {
            return;
        }

        let path = match dialog.get_selected_item() {
            Ok(item) => PathBuf::from(item),
            Err(_) => return,
        };

        match project::load(&path) {
            Ok(loaded) => {
                let source_dir = loaded.source_directory.clone();
                let database_path = loaded.database_path.clone();
                self.set_file_type_checkboxes(&loaded.file_extensions);
                *self.project.borrow_mut() = loaded;
                *self.current_project_path.borrow_mut() = Some(path.clone());
                self.source_dir_input.set_text(
                    source_dir.map(|dir| dir.to_string_lossy().into_owned()).as_deref().unwrap_or(""),
                );
                self.remember_recent_path(path.clone());

                *self.db.borrow_mut() = None;
                if let Some(relative) = database_path {
                    let absolute = path.parent().map(|parent| parent.join(&relative)).unwrap_or(relative);
                    self.start_db_open(absolute);
                }
            }
            Err(err) => {
                nwg::simple_message("PhotoMatic", &format!("Failed to load project: {err}"));
            }
        }
    }

    /// Opens (and migrates) the project database at `database_path` on a background
    /// thread, so a large migration can't stall the GUI thread. Disallows a second
    /// concurrent open, mirroring `start_scan`'s guard against overlapping scans.
    fn start_db_open(&self, database_path: PathBuf) {
        if self.db_open_thread.borrow().is_some() {
            return;
        }

        let sender = self.db_notice.sender();
        let handle = thread::spawn(move || {
            let result = db::ProjectDb::open(&database_path);
            sender.notice();
            result
        });
        *self.db_open_thread.borrow_mut() = Some(handle);
    }

    /// Fired via `OnNotice` once the background database-open thread finishes (started
    /// by `start_db_open` from `file_load`). Stores the opened `ProjectDb` and, if its
    /// post-migration schema version differs from the JSON's cached value, updates and
    /// re-saves the JSON so the cache never lags a migration that already ran. Failure
    /// is reported without blocking use of the rest of the loaded project.
    fn db_open_finished(&self) {
        let Some(handle) = self.db_open_thread.borrow_mut().take() else {
            return;
        };
        let Ok(result) = handle.join() else {
            return;
        };

        match result {
            Ok(db) => {
                let cached_version = self.project.borrow().database_schema_version;
                if db.schema_version().ok() != cached_version {
                    self.sync_database_metadata(&db);
                }
                *self.db.borrow_mut() = Some(db);
                self.refresh_nav_tree();
                self.refresh_metadata_button_enabled();
            }
            Err(err) => {
                nwg::simple_message("PhotoMatic", &format!("Failed to open project database: {err}"));
            }
        }
    }

    /// Rebuilds `nav_tree` from the database's current `directories`/`images` rows:
    /// one root node per top-level directory, sorted, each with fixed `jpg`/`cr2`/`gif`
    /// count children. Called whenever the database changes under the tree — after a
    /// project load and after a scan — so the tree never drifts from what's stored.
    fn refresh_nav_tree(&self) {
        self.nav_tree.clear();

        let db = self.db.borrow();
        let Some(db) = db.as_ref() else { return };
        let (Ok(dirs), Ok(counts)) = (db.list_directories(), db.directory_type_counts()) else { return };

        for node in nav_tree::build(&dirs, &counts) {
            let dir_item = self.nav_tree.insert_item(&node.dir_name, None, nwg::TreeInsert::Sort);
            for (ext, count) in &node.type_counts {
                self.nav_tree.insert_item(&format!("{ext} ({count})"), Some(&dir_item), nwg::TreeInsert::Last);
            }
            // Expanded by default so the type counts are visible without an extra click —
            // that's the whole point of putting them in the tree.
            self.nav_tree.set_expand_state(&dir_item, nwg::ExpandState::Expand);
        }
    }

    /// Fired via `OnTreeViewRightClick`. `nwg::TreeView` doesn't change selection on a
    /// right click and has no "item under cursor" query, so the item is found with a
    /// manual `TVM_HITTEST` at the current cursor position. Only top-level directory
    /// nodes (no parent) get the context menu — the `jpg (n)`/`cr2 (n)`/`gif (n)`
    /// children don't have a folder of their own to open.
    fn nav_tree_right_click(&self) {
        let Some(item) = tree_hit_test_at_cursor(&self.nav_tree) else { return };
        if self.nav_tree.parent(&item).is_some() {
            return;
        }

        *self.nav_tree_context_dir.borrow_mut() = self.nav_tree.item_text(&item);
        let (x, y) = nwg::GlobalCursor::position();
        self.nav_tree_menu.popup(x, y);
    }

    /// The `nav_tree_menu`'s only item: opens Windows Explorer at the top-level
    /// directory last right-clicked in `nav_tree` (recorded by `nav_tree_right_click`).
    fn open_selected_dir_in_explorer(&self) {
        let Some(dir_name) = self.nav_tree_context_dir.borrow_mut().take() else { return };
        let Some(source_dir) = self.project.borrow().source_directory.clone() else { return };

        let path = explorer::resolve_path(&source_dir, &dir_name);
        if let Err(err) = explorer::open(&path) {
            nwg::simple_message("PhotoMatic", &format!("Failed to open Explorer: {err}"));
        }
    }

    /// Refreshes the project's cached `database_schema_version`/`database_last_modified`
    /// fields from `db`'s current state and re-saves the project file. Only called once a
    /// `ProjectDb` exists, which only happens after the project has been saved at least
    /// once, so `current_project_path` is always set here.
    fn sync_database_metadata(&self, db: &db::ProjectDb) {
        {
            let mut project = self.project.borrow_mut();
            project.database_schema_version = db.schema_version().ok();
            project.database_last_modified = Some(chrono::Utc::now());
        }
        if let Some(path) = self.current_project_path.borrow().clone() {
            if let Err(err) = project::save(&path, &self.project.borrow()) {
                nwg::simple_message("PhotoMatic", &format!("Failed to save project: {err}"));
            }
        }
    }

    fn file_save(&self) {
        let path = self.current_project_path.borrow().clone();
        match path {
            Some(path) => self.save_project_to(&path),
            None => self.file_save_as(),
        }
    }

    fn file_save_as(&self) {
        let mut dialog = nwg::FileDialog::default();
        if !self.build_project_dialog(&mut dialog, "Save PhotoMatic Project As", nwg::FileDialogAction::Save) {
            return;
        }
        if !dialog.run(Some(&self.window)) {
            return;
        }

        let path = match dialog.get_selected_item() {
            Ok(item) => PathBuf::from(item),
            Err(_) => return,
        };

        self.save_project_to(&path);
    }

    fn save_project_to(&self, path: &Path) {
        self.sync_source_directory_from_input();
        self.ensure_database_provisioned(path);

        match project::save(path, &self.project.borrow()) {
            Ok(()) => {
                *self.current_project_path.borrow_mut() = Some(path.to_path_buf());
                self.remember_recent_path(path.to_path_buf());
            }
            Err(err) => {
                nwg::simple_message("PhotoMatic", &format!("Failed to save project: {err}"));
            }
        }
    }

    /// Creates the project's sibling `.sqlite3` database on first save (same stem as
    /// `path`, `.sqlite3` extension), migrating it to the latest schema and caching its
    /// version/timestamp on the project. Subsequent saves reuse the existing
    /// `database_path`, since this is a no-op once it's `Some`.
    fn ensure_database_provisioned(&self, path: &Path) {
        if self.project.borrow().database_path.is_some() {
            return;
        }

        let db_path = path.with_extension("sqlite3");
        match db::ProjectDb::open(&db_path) {
            Ok(db) => {
                let version = db.schema_version().ok();
                let relative = path
                    .parent()
                    .and_then(|parent| db_path.strip_prefix(parent).ok())
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| db_path.clone());

                let mut project = self.project.borrow_mut();
                project.database_path = Some(relative);
                project.database_schema_version = version;
                project.database_last_modified = Some(chrono::Utc::now());
                drop(project);

                *self.db.borrow_mut() = Some(db);
            }
            Err(err) => {
                nwg::simple_message("PhotoMatic", &format!("Failed to create project database: {err}"));
            }
        }
    }

    /// Builds a `.json`-filtered file dialog, defaulting to the recent project's
    /// directory when it still exists on disk. Returns `false` if the dialog
    /// couldn't be built at all (rare — e.g. a race where that directory just
    /// vanished), in which case the caller does nothing.
    fn build_project_dialog(&self, dialog: &mut nwg::FileDialog, title: &str, action: nwg::FileDialogAction) -> bool {
        let mut builder = nwg::FileDialog::builder().title(title).action(action).filters(PROJECT_FILTER);
        if let Some(dir) = self.recent_dir() {
            builder = builder.default_folder(dir.to_string_lossy().to_string());
        }
        builder.build(dialog).is_ok()
    }

    /// Opens a folder picker for the Source Directory field, defaulting to whatever's
    /// currently typed there (falling back to the recent project's directory).
    fn browse_source_directory(&self) {
        let mut dialog = nwg::FileDialog::default();
        let mut builder = nwg::FileDialog::builder()
            .title("Select Source Directory")
            .action(nwg::FileDialogAction::OpenDirectory);

        let current = self.source_dir_input.text();
        if !current.is_empty() {
            builder = builder.default_folder(current);
        } else if let Some(dir) = self.recent_dir() {
            builder = builder.default_folder(dir.to_string_lossy().to_string());
        }

        if builder.build(&mut dialog).is_err() {
            return;
        }
        if !dialog.run(Some(&self.window)) {
            return;
        }

        let Ok(item) = dialog.get_selected_item() else {
            return;
        };
        self.source_dir_input.set_text(&item.to_string_lossy());
        self.sync_source_directory_from_input();
    }

    /// Writes the Source Directory text box's current value into `self.project`, treating an
    /// empty box as "unset" rather than as a literal empty path.
    fn sync_source_directory_from_input(&self) {
        let text = self.source_dir_input.text();
        self.project.borrow_mut().source_directory =
            if text.is_empty() { None } else { Some(PathBuf::from(text)) };
    }

    /// Writes the File Types checkboxes' current state into `self.project`.
    fn sync_file_extensions_from_checkboxes(&self) {
        self.project.borrow_mut().file_extensions = FileExtensions {
            jpg: self.file_type_jpg.check_state() == nwg::CheckBoxState::Checked,
            cr2: self.file_type_cr2.check_state() == nwg::CheckBoxState::Checked,
            gif: self.file_type_gif.check_state() == nwg::CheckBoxState::Checked,
        };
    }

    /// Sets the File Types checkboxes to reflect `extensions` — used when starting a new
    /// project or loading one, the reverse direction of `sync_file_extensions_from_checkboxes`.
    fn set_file_type_checkboxes(&self, extensions: &FileExtensions) {
        let state = |checked: bool| if checked { nwg::CheckBoxState::Checked } else { nwg::CheckBoxState::Unchecked };
        self.file_type_jpg.set_check_state(state(extensions.jpg));
        self.file_type_cr2.set_check_state(state(extensions.cr2));
        self.file_type_gif.set_check_state(state(extensions.gif));
    }

    /// Directory of the most recently loaded/saved project, if that directory
    /// still exists — used to default the Load and Save As dialogs.
    fn recent_dir(&self) -> Option<PathBuf> {
        let config = self.config.borrow();
        let dir = config.recent_path.as_ref()?.parent()?;
        dir.exists().then(|| dir.to_path_buf())
    }

    fn remember_recent_path(&self, path: PathBuf) {
        let mut config = self.config.borrow_mut();
        config.recent_path = Some(path);
        if let Err(err) = settings::save(&config) {
            nwg::simple_message("PhotoMatic", &format!("Failed to save settings: {err}"));
        }
    }

    fn show_about(&self) {
        crate::about_modal::show();
    }

    /// Opens the Settings dialog. Disallows a second one while one is already open, since
    /// `settings_thread` only has room for a single in-flight dialog.
    fn open_settings(&self) {
        if self.settings_thread.borrow().is_some() {
            return;
        }

        let fullscreen = self.config.borrow().fullscreen;
        let handle = settings_modal::open(fullscreen, self.settings_notice.sender());
        *self.settings_thread.borrow_mut() = Some(handle);
    }

    /// Fired via `OnNotice` once the Settings dialog thread finishes (Accept, Cancel, or the
    /// dialog's own close button all route here through `settings_modal`'s thread join).
    fn settings_dialog_closed(&self) {
        let handle = match self.settings_thread.borrow_mut().take() {
            Some(handle) => handle,
            None => return,
        };

        let Ok(Some(fullscreen)) = handle.join() else {
            return;
        };

        self.config.borrow_mut().fullscreen = fullscreen;
        if let Err(err) = settings::save(&self.config.borrow()) {
            nwg::simple_message("PhotoMatic", &format!("Failed to save settings: {err}"));
        }
        window_mode::apply(&self.window, fullscreen);
    }

    /// Starts a recursive directory scan on a background thread, so the GUI stays responsive.
    /// Disallows a second concurrent scan while one is already running.
    fn start_scan(&self) {
        if self.scan_thread.borrow().is_some() {
            return;
        }

        self.sync_source_directory_from_input();
        let project = self.project.borrow();
        let Some(root) = project.source_directory.clone() else {
            nwg::simple_message("PhotoMatic", "Please choose a Source Directory before scanning.");
            return;
        };
        let extensions = project.file_extensions.clone();
        drop(project);

        // Moved into the closure alongside `root`/`extensions`, same ownership-transfer
        // idiom used above — `rusqlite::Connection` isn't `Sync`, so only one thread
        // touches it at a time, and folding the DB writes into the scan thread (one
        // `apply_scan_unit` call per unit as the walk discovers it, so records land
        // while the scan is still running) avoids a second thread hop. `db` is `None`
        // until the project has been saved once; the checks above are then a no-op,
        // so scanning before first save still works.
        let db = self.db.borrow_mut().take();

        self.scan_button.set_enabled(false);
        self.scan_progress.set_visible(true);

        let sender = self.scan_notice.sender();
        let handle = thread::spawn(move || {
            let mut db = db;
            let result = scan::scan_directory(&root, &extensions, |unit| {
                if let Some(db) = db.as_mut() {
                    let _ = db.apply_scan_unit(unit);
                }
            });
            if let Some(db) = db.as_mut() {
                let _ = db.finish_scan();
            }
            sender.notice();
            (result, db)
        });
        *self.scan_thread.borrow_mut() = Some(handle);
    }

    /// Fired via `OnNotice` once the scan thread finishes. Joining is effectively
    /// non-blocking here since `notice()` is only sent after the thread's work is done.
    fn scan_finished(&self) {
        let Some(handle) = self.scan_thread.borrow_mut().take() else {
            return;
        };
        let Ok((_result, db)) = handle.join() else {
            return;
        };

        self.scan_progress.set_visible(false);
        self.scan_button.set_enabled(true);

        if let Some(db) = db {
            self.sync_database_metadata(&db);
            *self.db.borrow_mut() = Some(db);
            self.refresh_nav_tree();
        }
        self.refresh_metadata_button_enabled();
    }

    /// Enables `metadata_button` only once a scan has populated the database —
    /// `project_settings.last_scan` (stamped by `finish_scan` after every completed
    /// scan) is used as the "has scan run" signal. Disabled (including while `db`
    /// is `None`, e.g. before the project's first save) otherwise, since there's
    /// nothing to iterate before a scan has run.
    fn refresh_metadata_button_enabled(&self) {
        let enabled = self
            .db
            .borrow()
            .as_ref()
            .and_then(|db| db.project_settings().ok())
            .map(|settings| settings.last_scan.is_some())
            .unwrap_or(false);
        self.metadata_button.set_enabled(enabled);
    }

    /// Extracts EXIF metadata for every image that doesn't have it yet, on a small
    /// pool of reader threads, while keeping all database access on this one
    /// orchestrating thread. Disallows a second concurrent run, and won't run
    /// alongside a scan — both need exclusive ownership of `self.db`, the same
    /// reason `rusqlite::Connection` (`Send` but not `Sync`) is moved into a single
    /// thread rather than shared, as `start_scan` already does.
    fn start_generate_metadata(&self) {
        if self.metadata_thread.borrow().is_some() || self.scan_thread.borrow().is_some() {
            return;
        }

        let Some(db) = self.db.borrow_mut().take() else {
            return;
        };
        let Some(source_dir) = self.project.borrow().source_directory.clone() else {
            *self.db.borrow_mut() = Some(db);
            return;
        };

        self.metadata_button.set_enabled(false);
        self.metadata_progress.set_visible(true);

        let sender = self.metadata_notice.sender();
        let handle = thread::spawn(move || {
            let db = db;
            let Ok(pending) = db.list_images_pending_metadata() else {
                sender.notice();
                return Some(db);
            };

            // Shared work queue: reader threads pop the next record (brief lock, no
            // I/O held under it), read its file, and send the result back; this one
            // thread is the only one that ever touches `db`.
            let queue = Arc::new(Mutex::new(VecDeque::from(pending)));
            let (tx, rx) = mpsc::channel::<(String, exif::ImageMetadata)>();

            let reader_count = thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
            let readers: Vec<_> = (0..reader_count)
                .map(|_| {
                    let queue = Arc::clone(&queue);
                    let reader_tx = tx.clone();
                    let source_dir = source_dir.clone();
                    thread::spawn(move || loop {
                        let record = queue.lock().unwrap().pop_front();
                        let Some(record) = record else { break };
                        let path = explorer::resolve_path(&source_dir, &record.path);
                        let metadata = exif::read_metadata(&path);
                        if reader_tx.send((record.key, metadata)).is_err() {
                            break;
                        }
                    })
                })
                .collect();
            drop(tx);

            for (key, metadata) in rx {
                let _ = db.update_image_metadata(&key, &metadata);
            }

            for reader in readers {
                let _ = reader.join();
            }

            sender.notice();
            Some(db)
        });
        *self.metadata_thread.borrow_mut() = Some(handle);
    }

    /// Fired via `OnNotice` once the Generate MetaData thread finishes.
    fn generate_metadata_finished(&self) {
        let Some(handle) = self.metadata_thread.borrow_mut().take() else {
            return;
        };
        let Ok(db) = handle.join() else {
            return;
        };

        self.metadata_progress.set_visible(false);
        self.metadata_button.set_enabled(true);
        *self.db.borrow_mut() = db;
        self.refresh_metadata_button_enabled();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_info_height_is_150_points() {
        assert_eq!(PROJECT_INFO_HEIGHT, 150.0);
    }

    #[test]
    fn nav_and_context_widths_split_the_body_evenly() {
        assert_eq!(NAV_WIDTH_PERCENT, 0.2);
        assert_eq!(CONTEXT_WIDTH_PERCENT, 0.8);
        assert_eq!(NAV_WIDTH_PERCENT + CONTEXT_WIDTH_PERCENT, 1.0);
    }
}
