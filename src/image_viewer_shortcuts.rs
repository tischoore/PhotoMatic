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
}
