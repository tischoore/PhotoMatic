use std::cell::RefCell;
use std::thread;

use native_windows_derive::NwgUi;
use native_windows_gui as nwg;
use nwg::NativeUi;

/// Opens the Settings dialog on its own thread and returns a handle whose `join()` yields
/// `Some(fullscreen)` if the user hit Accept, or `None` if they hit Cancel / closed the window.
///
/// NWG only allows one message-loop per thread (see `app.rs`), so a real dialog can't be a
/// blocking function call on the caller's thread the way `ShowDialog()` works elsewhere — this
/// is the pattern the NWG docs recommend for exactly that reason. The caller is notified that
/// the dialog finished via `sender` (an `nwg::Notice` bound to `OnNotice` on the main window).
pub fn open(initial_fullscreen: bool, sender: nwg::NoticeSender) -> thread::JoinHandle<Option<bool>> {
    thread::spawn(move || {
        let dialog = SettingsDialog::build_ui(Default::default())
            .expect("Failed to build the Settings dialog");

        let (windowed_state, fullscreen_state) = if initial_fullscreen {
            (nwg::RadioButtonState::Unchecked, nwg::RadioButtonState::Checked)
        } else {
            (nwg::RadioButtonState::Checked, nwg::RadioButtonState::Unchecked)
        };
        dialog.radio_windowed.set_check_state(windowed_state);
        dialog.radio_fullscreen.set_check_state(fullscreen_state);

        nwg::dispatch_thread_events();

        sender.notice();
        dialog.result.take()
    })
}

#[derive(Default, NwgUi)]
pub struct SettingsDialog {
    result: RefCell<Option<bool>>,

    #[nwg_control(size: (260, 160), position: (650, 300), title: "Settings", flags: "WINDOW|VISIBLE")]
    #[nwg_events(OnWindowClose: [SettingsDialog::exit])]
    window: nwg::Window,

    #[nwg_control(parent: window, text: "Windowed", position: (20, 20), size: (200, 25), flags: "VISIBLE|GROUP")]
    radio_windowed: nwg::RadioButton,

    #[nwg_control(parent: window, text: "Full screen", position: (20, 55), size: (200, 25), flags: "VISIBLE")]
    radio_fullscreen: nwg::RadioButton,

    #[nwg_control(parent: window, text: "Accept", position: (30, 105), size: (90, 30))]
    #[nwg_events(OnButtonClick: [SettingsDialog::accept])]
    accept_btn: nwg::Button,

    #[nwg_control(parent: window, text: "Cancel", position: (140, 105), size: (90, 30))]
    #[nwg_events(OnButtonClick: [SettingsDialog::cancel])]
    cancel_btn: nwg::Button,
}

impl SettingsDialog {
    fn exit(&self) {
        nwg::stop_thread_dispatch();
    }

    fn accept(&self) {
        let fullscreen = self.radio_fullscreen.check_state() == nwg::RadioButtonState::Checked;
        *self.result.borrow_mut() = Some(fullscreen);
        self.window.close();
    }

    fn cancel(&self) {
        self.window.close();
    }
}
