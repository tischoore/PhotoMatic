use native_windows_gui as nwg;

/// Shows the About box. Backed by a native `MessageBoxW` (via `nwg::simple_message`), which is
/// genuinely OS-modal — unlike the Settings dialog (see `settings_modal.rs`), this call blocks
/// the calling thread's message loop until the user dismisses it.
pub fn show() {
    nwg::simple_message("About PhotoMatic", "PhotoMatic - Tischer 2026");
}
