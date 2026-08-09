use std::cell::RefCell;
use std::collections::HashSet;
use std::thread;

use native_windows_derive::NwgUi;
use native_windows_gui as nwg;
use nwg::NativeUi;

use crate::db::models::CollectionRecord;

/// Opens the Add/Edit Collection dialog on its own thread and returns a handle whose
/// `join()` yields `Some((title, notes, shortcut))` if the user hit Accept, or `None` on
/// Cancel/close — same thread-per-dialog pattern as `settings_modal.rs` (NWG only allows one
/// message loop per thread). `existing` prefills the fields and switches the window title to
/// "Edit Collection" (vs "Add Collection" for `None`); `other_shortcuts` is every *other*
/// collection's shortcut (the one being edited, if any, excluded), which Accept validates the
/// entered shortcut against so two collections can never end up sharing one.
pub fn open(
    existing: Option<CollectionRecord>,
    other_shortcuts: HashSet<String>,
    sender: nwg::NoticeSender,
) -> thread::JoinHandle<Option<(String, String, String)>> {
    thread::spawn(move || {
        let dialog =
            CollectionDialog::build_ui(Default::default()).expect("Failed to build the Collection dialog");

        dialog.window.set_text(if existing.is_some() { "Edit Collection" } else { "Add Collection" });
        if let Some(record) = &existing {
            dialog.title_input.set_text(&record.name);
            dialog.notes_input.set_text(&record.description);
            dialog.shortcut_input.set_text(&record.shortcut);
        }
        *dialog.other_shortcuts.borrow_mut() = other_shortcuts;

        nwg::dispatch_thread_events();

        sender.notice();
        dialog.result.take()
    })
}

#[derive(Default, NwgUi)]
pub struct CollectionDialog {
    result: RefCell<Option<(String, String, String)>>,
    /// Every other collection's current shortcut — set once by `open` right after the
    /// window is built, read back by `accept`'s validation.
    other_shortcuts: RefCell<HashSet<String>>,

    #[nwg_control(size: (320, 280), position: (600, 260), title: "Add Collection", flags: "WINDOW|VISIBLE")]
    #[nwg_events(OnWindowClose: [CollectionDialog::exit])]
    window: nwg::Window,

    #[nwg_control(parent: window, text: "Title:", position: (20, 16), size: (280, 20))]
    title_label: nwg::Label,

    #[nwg_control(parent: window, text: "", position: (20, 38), size: (280, 24))]
    title_input: nwg::TextInput,

    #[nwg_control(parent: window, text: "Notes:", position: (20, 72), size: (280, 20))]
    notes_label: nwg::Label,

    #[nwg_control(parent: window, text: "", position: (20, 94), size: (280, 90), flags: "VISIBLE|VSCROLL|AUTOVSCROLL")]
    notes_input: nwg::TextBox,

    #[nwg_control(parent: window, text: "Shortcut key:", position: (20, 194), size: (280, 20))]
    shortcut_label: nwg::Label,

    #[nwg_control(parent: window, text: "", position: (20, 216), size: (50, 24))]
    shortcut_input: nwg::TextInput,

    #[nwg_control(parent: window, text: "Accept", position: (110, 214), size: (90, 30))]
    #[nwg_events(OnButtonClick: [CollectionDialog::accept])]
    accept_btn: nwg::Button,

    #[nwg_control(parent: window, text: "Cancel", position: (210, 214), size: (90, 30))]
    #[nwg_events(OnButtonClick: [CollectionDialog::cancel])]
    cancel_btn: nwg::Button,
}

impl CollectionDialog {
    fn exit(&self) {
        nwg::stop_thread_dispatch();
    }

    /// Validates the fields before allowing the dialog to close — on failure, a modal error
    /// box explains why and the dialog stays open so the user can correct it, rather than
    /// silently discarding what they typed.
    fn accept(&self) {
        let title = self.title_input.text();
        let shortcut = self.shortcut_input.text();
        let validated = validate(&title, &shortcut, &self.other_shortcuts.borrow());
        match validated {
            Ok((title, shortcut)) => {
                *self.result.borrow_mut() = Some((title, self.notes_input.text(), shortcut));
                self.window.close();
            }
            Err(message) => {
                nwg::modal_error_message(&self.window, "PhotoMatic", &message);
            }
        }
    }

    fn cancel(&self) {
        self.window.close();
    }
}

/// Validates the Title/Shortcut fields: the title must be non-empty once trimmed, and the
/// shortcut must trim down to exactly one non-whitespace character not already used by
/// another collection. Returns the trimmed `(title, shortcut)` on success. Kept independent
/// of NWG so it's unit-testable without a window, per `CLAUDE.md`'s testing convention.
fn validate(title: &str, shortcut: &str, other_shortcuts: &HashSet<String>) -> Result<(String, String), String> {
    let title = title.trim().to_string();
    if title.is_empty() {
        return Err("Please enter a title for the collection.".to_string());
    }

    let shortcut = shortcut.trim().to_string();
    if shortcut.chars().count() != 1 {
        return Err("The shortcut must be exactly one character.".to_string());
    }

    if other_shortcuts.contains(&shortcut) {
        return Err(format!("The shortcut \"{shortcut}\" is already used by another collection."));
    }

    Ok((title, shortcut))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_an_empty_or_whitespace_only_title() {
        assert!(validate("", "d", &HashSet::new()).is_err());
        assert!(validate("   ", "d", &HashSet::new()).is_err());
    }

    #[test]
    fn rejects_a_shortcut_that_is_not_exactly_one_character() {
        assert!(validate("Trip", "", &HashSet::new()).is_err());
        assert!(validate("Trip", "ab", &HashSet::new()).is_err());
    }

    #[test]
    fn rejects_a_shortcut_already_used_by_another_collection() {
        let other = HashSet::from(["d".to_string()]);
        assert!(validate("Trip", "d", &other).is_err());
    }

    #[test]
    fn accepts_a_trimmed_title_and_single_character_shortcut() {
        let (title, shortcut) = validate("  Trip  ", " x ", &HashSet::new()).unwrap();
        assert_eq!(title, "Trip");
        assert_eq!(shortcut, "x");
    }

    #[test]
    fn a_shortcut_not_used_by_anyone_else_is_accepted_even_if_it_was_the_editing_collections_own() {
        // `other_shortcuts` is expected to exclude the collection currently being edited, so
        // re-submitting its own unchanged shortcut must not be rejected as a "duplicate".
        let other = HashSet::from(["a".to_string(), "b".to_string()]);
        assert!(validate("Trip", "c", &other).is_ok());
    }
}
