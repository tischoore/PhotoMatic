use native_windows_gui as nwg;

/// A menu action reachable through a Ctrl-key accelerator.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum ShortcutAction {
    New,
    Open,
    Save,
    SaveAs,
    CloseTab,
}

/// Maps a virtual-key code and modifier state to the shortcut it represents, following the
/// standard Windows accelerators (Ctrl+N/O/S, Ctrl+Shift+S). Kept independent of NWG's event
/// system so it can be unit-tested without a window or message loop.
pub fn resolve(key: u32, ctrl: bool, shift: bool) -> Option<ShortcutAction> {
    if !ctrl {
        return None;
    }

    match key {
        nwg::keys::_N => Some(ShortcutAction::New),
        nwg::keys::_O => Some(ShortcutAction::Open),
        nwg::keys::_S if shift => Some(ShortcutAction::SaveAs),
        nwg::keys::_S => Some(ShortcutAction::Save),
        nwg::keys::_W => Some(ShortcutAction::CloseTab),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignores_keys_without_ctrl_held() {
        assert_eq!(resolve(nwg::keys::_N, false, false), None);
    }

    #[test]
    fn ctrl_n_is_new() {
        assert_eq!(resolve(nwg::keys::_N, true, false), Some(ShortcutAction::New));
    }

    #[test]
    fn ctrl_o_is_open() {
        assert_eq!(resolve(nwg::keys::_O, true, false), Some(ShortcutAction::Open));
    }

    #[test]
    fn ctrl_s_is_save() {
        assert_eq!(resolve(nwg::keys::_S, true, false), Some(ShortcutAction::Save));
    }

    #[test]
    fn ctrl_shift_s_is_save_as() {
        assert_eq!(resolve(nwg::keys::_S, true, true), Some(ShortcutAction::SaveAs));
    }

    #[test]
    fn ctrl_w_is_close_tab() {
        assert_eq!(resolve(nwg::keys::_W, true, false), Some(ShortcutAction::CloseTab));
    }

    #[test]
    fn unrelated_key_resolves_to_none() {
        assert_eq!(resolve(nwg::keys::_A, true, false), None);
    }
}
