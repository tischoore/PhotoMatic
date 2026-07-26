use std::cell::RefCell;
use std::thread;

use native_windows_derive::NwgUi;
use native_windows_gui as nwg;

use crate::settings::{self, AppConfig};
use crate::settings_modal;
use crate::window_mode;

#[derive(Default, NwgUi)]
pub struct App {
    config: RefCell<AppConfig>,
    settings_thread: RefCell<Option<thread::JoinHandle<Option<bool>>>>,

    #[nwg_control(title: "PhotoMatic", flags: "MAIN_WINDOW")]
    #[nwg_events(OnWindowClose: [App::exit])]
    window: nwg::Window,

    #[nwg_control]
    #[nwg_events(OnNotice: [App::settings_dialog_closed])]
    settings_notice: nwg::Notice,

    #[nwg_control(parent: window, text: "File")]
    file_menu: nwg::Menu,

    #[nwg_control(parent: file_menu, text: "Exit")]
    #[nwg_events(OnMenuItemSelected: [App::exit])]
    file_exit: nwg::MenuItem,

    #[nwg_control(parent: window, text: "Edit")]
    edit_menu: nwg::Menu,

    #[nwg_control(parent: edit_menu, text: "Settings")]
    #[nwg_events(OnMenuItemSelected: [App::open_settings])]
    edit_settings: nwg::MenuItem,

    #[nwg_control(parent: window, text: "Help")]
    help_menu: nwg::Menu,

    #[nwg_control(parent: help_menu, text: "About")]
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
