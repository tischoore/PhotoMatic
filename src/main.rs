#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod about_modal;
mod app;
mod settings;
mod settings_modal;
mod window_mode;

use native_windows_gui as nwg;
use nwg::NativeUi;

fn main() {
    nwg::init().expect("Failed to init Native Windows GUI");
    nwg::Font::set_global_family("Segoe UI").expect("Failed to set default font");

    let config = settings::load();
    let app = app::App::build_ui(Default::default()).expect("Failed to build UI");
    app.init(config);

    nwg::dispatch_thread_events();
}
