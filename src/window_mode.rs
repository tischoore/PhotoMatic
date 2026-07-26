use native_windows_gui as nwg;

const WINDOWED_SIZE: (u32, u32) = (1024, 768);
const WINDOWED_POSITION: (i32, i32) = (100, 100);

/// Applies windowed or full-screen mode to the main window.
///
/// "Full screen" uses the native maximize behavior (`Window::maximize`) rather than a
/// borderless/exclusive mode: NWG has no supported way to change a window's border/title-bar
/// style after creation, so maximizing is the robust option that needs no window rebuild.
pub fn apply(window: &nwg::Window, fullscreen: bool) {
    if fullscreen {
        window.maximize();
    } else {
        window.restore();
        window.set_position(WINDOWED_POSITION.0, WINDOWED_POSITION.1);
        window.set_size(WINDOWED_SIZE.0, WINDOWED_SIZE.1);
    }
}
