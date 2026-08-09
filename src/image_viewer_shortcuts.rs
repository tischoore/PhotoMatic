use native_windows_gui as nwg;

/// A keyboard action reachable inside the Image Viewer window.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum ViewerAction {
    Close,
    Prev,
    Next,
}

/// Maps a virtual-key code and Ctrl state to the Image Viewer action it represents (Left/Right
/// arrows to navigate, Ctrl+W to close, mirroring the Context Window tab convention). Kept
/// independent of NWG's event system so it can be unit-tested without a window or message loop,
/// the same way `crate::shortcuts::resolve` is for the main window.
pub fn resolve(key: u32, ctrl: bool) -> Option<ViewerAction> {
    match key {
        nwg::keys::LEFT => Some(ViewerAction::Prev),
        nwg::keys::RIGHT => Some(ViewerAction::Next),
        nwg::keys::_W if ctrl => Some(ViewerAction::Close),
        _ => None,
    }
}

/// Whether a raw Win32 message is a Left/Right arrow key-down that must bypass
/// `IsDialogMessageW`'s dialog-navigation handling so it reaches `OnKeyPress` (see
/// `ImageViewer::open`'s message loop) — `IsDialogMessageW` otherwise consumes arrow keys
/// to cycle keyboard focus between the viewer's buttons/checkboxes before `resolve` ever
/// sees them, which is why Left/Right silently did nothing despite `resolve` already
/// mapping them correctly.
pub fn bypasses_dialog_navigation(message: u32, virtual_key: usize) -> bool {
    message == winapi::um::winuser::WM_KEYDOWN
        && (virtual_key == nwg::keys::LEFT as usize || virtual_key == nwg::keys::RIGHT as usize)
}

/// Maps a plain (non-Ctrl) key press to the id of the collection whose shortcut it matches.
/// Ctrl is excluded so a collection shortcut can never shadow a Ctrl-combo action (e.g. Ctrl+W
/// close) — only a bare letter press toggles a collection, matching how the checkbox's own `&`
/// mnemonic is Alt-based rather than Ctrl-based. Case-insensitive since shortcuts are stored
/// lowercase but virtual-key codes for letters arrive as uppercase ASCII. Takes the shortcut set
/// as data (rather than a compile-time enum like `ViewerAction`) since collections, and their
/// shortcuts, are defined at runtime.
pub fn resolve_collection_shortcut(key: u32, ctrl: bool, shortcuts: &[(char, i64)]) -> Option<i64> {
    if ctrl {
        return None;
    }
    let pressed = char::from_u32(key)?.to_ascii_lowercase();
    shortcuts.iter().find(|(ch, _)| ch.to_ascii_lowercase() == pressed).map(|(_, id)| *id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn left_arrow_is_prev_regardless_of_ctrl() {
        assert_eq!(resolve(nwg::keys::LEFT, false), Some(ViewerAction::Prev));
        assert_eq!(resolve(nwg::keys::LEFT, true), Some(ViewerAction::Prev));
    }

    #[test]
    fn right_arrow_is_next_regardless_of_ctrl() {
        assert_eq!(resolve(nwg::keys::RIGHT, false), Some(ViewerAction::Next));
        assert_eq!(resolve(nwg::keys::RIGHT, true), Some(ViewerAction::Next));
    }

    #[test]
    fn ctrl_w_is_close() {
        assert_eq!(resolve(nwg::keys::_W, true), Some(ViewerAction::Close));
    }

    #[test]
    fn plain_w_without_ctrl_does_nothing() {
        assert_eq!(resolve(nwg::keys::_W, false), None);
    }

    #[test]
    fn unrelated_key_resolves_to_none() {
        assert_eq!(resolve(nwg::keys::_A, true), None);
    }

    #[test]
    fn left_and_right_arrow_keydown_bypass_dialog_navigation() {
        assert!(bypasses_dialog_navigation(winapi::um::winuser::WM_KEYDOWN, nwg::keys::LEFT as usize));
        assert!(bypasses_dialog_navigation(winapi::um::winuser::WM_KEYDOWN, nwg::keys::RIGHT as usize));
    }

    #[test]
    fn unrelated_keydown_does_not_bypass_dialog_navigation() {
        assert!(!bypasses_dialog_navigation(winapi::um::winuser::WM_KEYDOWN, nwg::keys::_W as usize));
    }

    #[test]
    fn non_keydown_message_does_not_bypass_dialog_navigation_even_for_arrow_keys() {
        assert!(!bypasses_dialog_navigation(winapi::um::winuser::WM_CHAR, nwg::keys::LEFT as usize));
    }

    #[test]
    fn collection_shortcut_matches_its_letter() {
        let shortcuts = [('d', 1), ('f', 2)];
        assert_eq!(resolve_collection_shortcut(nwg::keys::_D, false, &shortcuts), Some(1));
        assert_eq!(resolve_collection_shortcut(nwg::keys::_F, false, &shortcuts), Some(2));
    }

    #[test]
    fn collection_shortcut_is_case_insensitive() {
        let shortcuts = [('D', 1)];
        assert_eq!(resolve_collection_shortcut(nwg::keys::_D, false, &shortcuts), Some(1));
    }

    #[test]
    fn collection_shortcut_ignores_unmatched_key() {
        let shortcuts = [('d', 1)];
        assert_eq!(resolve_collection_shortcut(nwg::keys::_A, false, &shortcuts), None);
    }

    #[test]
    fn collection_shortcut_is_suppressed_when_ctrl_is_held() {
        let shortcuts = [('d', 1)];
        assert_eq!(resolve_collection_shortcut(nwg::keys::_D, true, &shortcuts), None);
    }
}
