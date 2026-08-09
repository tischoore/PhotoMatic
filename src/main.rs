#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod about_modal;
mod app;
mod collection_modal;
mod collection_tree;
mod context_tabs;
mod db;
mod event_tree;
mod events;
mod exif;
mod explorer;
mod image_viewer;
mod image_viewer_shortcuts;
mod keyboard;
mod nav_tree;
mod panel_background;
mod project;
mod raw_linking;
mod scan;
mod settings;
mod settings_modal;
mod shortcuts;
mod window_mode;

use native_windows_gui as nwg;
use nwg::NativeUi;

fn main() {
    // The File > Load / Save As dialogs use the Windows shell's IFileDialog (COM), which
    // requires COM to be initialized on the thread that uses it — native-windows-gui doesn't
    // do this itself. Without it, `nwg::FileDialog` fails (or crashes) when opened.
    unsafe {
        winapi::um::combaseapi::CoInitializeEx(
            std::ptr::null_mut(),
            winapi::um::objbase::COINIT_APARTMENTTHREADED,
        );
    }

    nwg::init().expect("Failed to init Native Windows GUI");
    nwg::Font::set_global_family("Segoe UI").expect("Failed to set default font");

    let config = settings::load();
    let app = app::App::build_ui(Default::default()).expect("Failed to build UI");
    app.init(config);

    nwg::dispatch_thread_events();
}
