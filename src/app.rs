use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::thread;

use native_windows_derive::NwgUi;
use native_windows_gui as nwg;
use nwg::stretch::geometry::{Rect, Size};
use nwg::stretch::style::{AlignItems, Dimension as D, FlexDirection, JustifyContent};

use crate::panel_background;
use crate::project::{self, FileExtensions, ProjectFile};
use crate::settings::{self, AppConfig};
use crate::settings_modal;
use crate::shortcuts::{self, ShortcutAction};
use crate::window_mode;

const PROJECT_FILTER: &str = "PhotoMatic Project(*.json)";

/// Height of the Project Information strip, in points (`FlexboxLayout` units).
const PROJECT_INFO_HEIGHT: f32 = 300.0;
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

/// Whether the given virtual key (e.g. `VK_CONTROL`) is currently held down.
fn key_down(vk: winapi::ctypes::c_int) -> bool {
    unsafe { winapi::um::winuser::GetKeyState(vk) & 0x8000u16 as i16 != 0 }
}

#[derive(Default, NwgUi)]
pub struct App {
    config: RefCell<AppConfig>,
    settings_thread: RefCell<Option<thread::JoinHandle<Option<bool>>>>,
    project: RefCell<ProjectFile>,
    current_project_path: RefCell<Option<PathBuf>>,

    #[nwg_control(title: "PhotoMatic", flags: "MAIN_WINDOW")]
    #[nwg_events(OnWindowClose: [App::exit], OnKeyPress: [App::on_key_press(SELF, EVT_DATA)])]
    window: nwg::Window,

    #[nwg_control]
    #[nwg_events(OnNotice: [App::settings_dialog_closed])]
    settings_notice: nwg::Notice,

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

    body_layout: nwg::FlexboxLayout,
    root_layout: nwg::FlexboxLayout,
    project_info_layout: nwg::FlexboxLayout,
    source_dir_layout: nwg::FlexboxLayout,
    file_types_layout: nwg::FlexboxLayout,
}

impl App {
    pub fn init(&self, config: AppConfig) {
        window_mode::apply(&self.window, config.fullscreen);
        *self.config.borrow_mut() = config;
        self.build_layout();
        panel_background::paint(&self.project_info_frame, DARK_PANEL_COLOR);
        panel_background::paint(&self.nav_frame, DARK_PANEL_COLOR);
        panel_background::paint(&self.context_frame, LIGHT_PANEL_COLOR);
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
            .build(&self.project_info_layout)
            .expect("Failed to build the Project Information column layout");
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
        self.source_dir_input.set_text("");
        self.set_file_type_checkboxes(&FileExtensions::default());
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
                self.set_file_type_checkboxes(&loaded.file_extensions);
                *self.project.borrow_mut() = loaded;
                *self.current_project_path.borrow_mut() = Some(path.clone());
                self.source_dir_input.set_text(
                    source_dir.map(|dir| dir.to_string_lossy().into_owned()).as_deref().unwrap_or(""),
                );
                self.remember_recent_path(path);
            }
            Err(err) => {
                nwg::simple_message("PhotoMatic", &format!("Failed to load project: {err}"));
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_info_height_is_300_points() {
        assert_eq!(PROJECT_INFO_HEIGHT, 300.0);
    }

    #[test]
    fn nav_and_context_widths_split_the_body_evenly() {
        assert_eq!(NAV_WIDTH_PERCENT, 0.2);
        assert_eq!(CONTEXT_WIDTH_PERCENT, 0.8);
        assert_eq!(NAV_WIDTH_PERCENT + CONTEXT_WIDTH_PERCENT, 1.0);
    }
}
