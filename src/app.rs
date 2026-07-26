use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::thread;

use native_windows_derive::NwgUi;
use native_windows_gui as nwg;

use crate::project::{self, ProjectFile};
use crate::settings::{self, AppConfig};
use crate::settings_modal;
use crate::shortcuts::{self, ShortcutAction};
use crate::window_mode;

const PROJECT_FILTER: &str = "PhotoMatic Project(*.json)";

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
}

impl App {
    pub fn init(&self, config: AppConfig) {
        window_mode::apply(&self.window, config.fullscreen);
        *self.config.borrow_mut() = config;
        self.window.set_visible(true);
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
                *self.project.borrow_mut() = loaded;
                *self.current_project_path.borrow_mut() = Some(path.clone());
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
