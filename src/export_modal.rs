use std::cell::RefCell;
use std::path::PathBuf;
use std::thread;

use native_windows_derive::NwgUi;
use native_windows_gui as nwg;
use nwg::NativeUi;

use crate::export::{ExportOptions, Recompression, RescaleTarget};

/// Opens the Export dialog on its own thread and returns a handle whose `join()` yields
/// `Some(ExportOptions)` if the user hit Export, or `None` on Cancel/close — same
/// dialog-per-thread pattern as `settings_modal.rs`/`collection_modal.rs` (NWG only allows one
/// message loop per thread). Unlike those two, this dialog's Browse button uses
/// `nwg::FileDialog`, which needs COM initialized on the thread that uses it (see `main.rs`) —
/// this is a fresh thread that has never done so, so `open` brackets the dialog with its own
/// `CoInitializeEx`/`CoUninitialize`, same as `image_viewer.rs`'s prefetch threads.
pub fn open(sender: nwg::NoticeSender) -> thread::JoinHandle<Option<ExportOptions>> {
    thread::spawn(move || {
        unsafe {
            winapi::um::combaseapi::CoInitializeEx(
                std::ptr::null_mut(),
                winapi::um::objbase::COINIT_APARTMENTTHREADED,
            );
        }

        let dialog = ExportDialog::build_ui(Default::default()).expect("Failed to build the Export dialog");
        dialog.update_rescale_controls();

        nwg::dispatch_thread_events();

        unsafe {
            winapi::um::combaseapi::CoUninitialize();
        }

        sender.notice();
        dialog.result.take()
    })
}

#[derive(Default, NwgUi)]
pub struct ExportDialog {
    result: RefCell<Option<ExportOptions>>,

    #[nwg_control(size: (360, 320), position: (600, 260), title: "Export", flags: "WINDOW|VISIBLE")]
    #[nwg_events(OnWindowClose: [ExportDialog::exit])]
    window: nwg::Window,

    #[nwg_control(parent: window, text: "Recompression:", position: (20, 16), size: (320, 20))]
    recompression_label: nwg::Label,

    #[nwg_control(parent: window, text: "&None", position: (30, 40), size: (90, 24), flags: "VISIBLE|GROUP")]
    #[nwg_events(OnButtonClick: [ExportDialog::update_rescale_controls])]
    recompression_none: nwg::RadioButton,

    #[nwg_control(parent: window, text: "&Large (80%)", position: (130, 40), size: (100, 24), flags: "VISIBLE")]
    #[nwg_events(OnButtonClick: [ExportDialog::update_rescale_controls])]
    recompression_large: nwg::RadioButton,

    #[nwg_control(parent: window, text: "&Small (50%)", position: (240, 40), size: (100, 24), flags: "VISIBLE")]
    #[nwg_events(OnButtonClick: [ExportDialog::update_rescale_controls])]
    recompression_small: nwg::RadioButton,

    #[nwg_control(parent: window, text: "&Rescale", position: (20, 76), size: (100, 24))]
    #[nwg_events(OnButtonClick: [ExportDialog::update_rescale_controls])]
    rescale_checkbox: nwg::CheckBox,

    #[nwg_control(parent: window, text: "&Width:", position: (40, 106), size: (60, 20))]
    width_label: nwg::Label,

    #[nwg_control(parent: window, text: "", position: (100, 104), size: (70, 24))]
    width_input: nwg::TextInput,

    #[nwg_control(parent: window, text: "&Height:", position: (180, 106), size: (60, 20))]
    height_label: nwg::Label,

    #[nwg_control(parent: window, text: "", position: (240, 104), size: (70, 24))]
    height_input: nwg::TextInput,

    #[nwg_control(parent: window, text: "Leave one blank to scale by the other; aspect ratio is always kept.", position: (20, 130), size: (320, 18))]
    rescale_hint_label: nwg::Label,

    #[nwg_control(parent: window, text: "Output &Directory:", position: (20, 154), size: (320, 20))]
    output_dir_label: nwg::Label,

    #[nwg_control(parent: window, text: "", position: (20, 176), size: (240, 24))]
    output_dir_input: nwg::TextInput,

    #[nwg_control(parent: window, text: "&Browse...", position: (270, 175), size: (70, 26))]
    #[nwg_events(OnButtonClick: [ExportDialog::browse])]
    browse_btn: nwg::Button,

    #[nwg_control(parent: window, text: "E&xport", position: (160, 260), size: (90, 30))]
    #[nwg_events(OnButtonClick: [ExportDialog::export])]
    export_btn: nwg::Button,

    #[nwg_control(parent: window, text: "&Cancel", position: (260, 260), size: (90, 30))]
    #[nwg_events(OnButtonClick: [ExportDialog::cancel])]
    cancel_btn: nwg::Button,
}

impl ExportDialog {
    fn exit(&self) {
        nwg::stop_thread_dispatch();
    }

    fn selected_recompression(&self) -> Recompression {
        if self.recompression_large.check_state() == nwg::RadioButtonState::Checked {
            Recompression::Large
        } else if self.recompression_small.check_state() == nwg::RadioButtonState::Checked {
            Recompression::Small
        } else {
            Recompression::None
        }
    }

    /// Wires the Rescale row's enable/reveal state to the current Recompression choice, and the
    /// Width/Height inputs' visibility to Rescale itself — called after every radio/checkbox
    /// click so the form always reflects a consistent state. Recompression = None disables and
    /// force-clears Rescale (resizing without recompressing was never offered, so a stale check
    /// from a previous selection must not silently survive).
    fn update_rescale_controls(&self) {
        let can_rescale = self.selected_recompression() != Recompression::None;
        self.rescale_checkbox.set_enabled(can_rescale);
        if !can_rescale {
            self.rescale_checkbox.set_check_state(nwg::CheckBoxState::Unchecked);
        }

        let show_dimensions = can_rescale && self.rescale_checkbox.check_state() == nwg::CheckBoxState::Checked;
        self.width_label.set_visible(show_dimensions);
        self.width_input.set_visible(show_dimensions);
        self.height_label.set_visible(show_dimensions);
        self.height_input.set_visible(show_dimensions);
        self.rescale_hint_label.set_visible(show_dimensions);
    }

    /// Opens a folder picker for the output directory, defaulting to whatever's currently typed
    /// — same shape as `App::browse_source_directory` (`app.rs`).
    fn browse(&self) {
        let mut dialog = nwg::FileDialog::default();
        let mut builder = nwg::FileDialog::builder().title("Select Output Directory").action(nwg::FileDialogAction::OpenDirectory);

        let current = self.output_dir_input.text();
        if !current.is_empty() {
            builder = builder.default_folder(current);
        }

        if builder.build(&mut dialog).is_err() {
            return;
        }
        if !dialog.run(Some(&self.window)) {
            return;
        }
        let Ok(item) = dialog.get_selected_item() else { return };
        self.output_dir_input.set_text(&item.to_string_lossy());
    }

    /// Export button: validates the form, warns (but doesn't block) if the chosen directory
    /// isn't empty, then closes the dialog with a result.
    fn export(&self) {
        let recompression = self.selected_recompression();
        let rescale_checked = self.rescale_checkbox.check_state() == nwg::CheckBoxState::Checked;
        match validate(recompression, rescale_checked, &self.width_input.text(), &self.height_input.text(), &self.output_dir_input.text()) {
            Err(message) => {
                nwg::modal_error_message(&self.window, "PhotoMatic", &message);
            }
            Ok(options) => {
                if crate::export::directory_is_non_empty(&options.output_dir) {
                    let choice = nwg::modal_message(
                        &self.window,
                        &nwg::MessageParams {
                            title: "PhotoMatic",
                            content: "The output directory is not empty. Continue anyway?",
                            buttons: nwg::MessageButtons::YesNo,
                            icons: nwg::MessageIcons::Warning,
                        },
                    );
                    if choice != nwg::MessageChoice::Yes {
                        return;
                    }
                }
                *self.result.borrow_mut() = Some(options);
                self.window.close();
            }
        }
    }

    fn cancel(&self) {
        self.window.close();
    }
}

/// Validates the Export form's fields into an `ExportOptions`. The output directory must be
/// non-empty once trimmed. Width/Height are each optional — a blank field means "derive this
/// dimension to keep the aspect ratio" (see `export::scaled_output_size`) — but when recompressing
/// with Rescale checked, at least one of the two must be a valid positive integer; a field that
/// isn't blank must still parse as one. Ignored entirely when Rescale isn't in effect (the UI
/// already hides/disables the fields in that state, so this is a defensive fallback, not expected
/// to trigger from normal use). Kept independent of NWG so it's unit-testable without a window,
/// per `CLAUDE.md`'s testing convention.
fn validate(
    recompression: Recompression,
    rescale_checked: bool,
    width_text: &str,
    height_text: &str,
    output_dir_text: &str,
) -> Result<ExportOptions, String> {
    let output_dir = output_dir_text.trim();
    if output_dir.is_empty() {
        return Err("Please choose an output directory.".to_string());
    }

    let rescale = if recompression != Recompression::None && rescale_checked {
        let width = parse_positive(width_text, "width")?;
        let height = parse_positive(height_text, "height")?;
        if width.is_none() && height.is_none() {
            return Err("Please enter a width, a height, or both.".to_string());
        }
        Some(RescaleTarget { width, height })
    } else {
        None
    };

    Ok(ExportOptions { recompression, rescale, output_dir: PathBuf::from(output_dir) })
}

/// Parses `text` as an optional positive size: blank (once trimmed) means "not provided"
/// (`Ok(None)`) — the corresponding dimension is derived instead, to keep the aspect ratio.
/// Anything else must parse to a whole number greater than zero.
fn parse_positive(text: &str, field_name: &str) -> Result<Option<u32>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    match text.parse::<u32>() {
        Ok(value) if value > 0 => Ok(Some(value)),
        _ => Err(format!("Please enter a whole number greater than zero for {field_name}.")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_an_empty_output_directory() {
        assert!(validate(Recompression::None, false, "", "", "  ").is_err());
    }

    #[test]
    fn none_recompression_ignores_rescale_inputs_entirely() {
        let options = validate(Recompression::None, true, "not a number", "", "C:\\out").unwrap();
        assert_eq!(options.rescale, None);
    }

    #[test]
    fn recompress_with_rescale_rejects_invalid_or_zero_dimensions() {
        assert!(validate(Recompression::Large, true, "abc", "600", "C:\\out").is_err());
        assert!(validate(Recompression::Large, true, "800", "0", "C:\\out").is_err());
    }

    #[test]
    fn recompress_with_rescale_requires_at_least_one_dimension() {
        assert!(validate(Recompression::Large, true, "", "", "C:\\out").is_err());
    }

    #[test]
    fn recompress_with_rescale_accepts_width_only() {
        let options = validate(Recompression::Small, true, "800", "", "C:\\out").unwrap();
        assert_eq!(options.rescale, Some(RescaleTarget { width: Some(800), height: None }));
    }

    #[test]
    fn recompress_with_rescale_accepts_height_only() {
        let options = validate(Recompression::Small, true, "", "600", "C:\\out").unwrap();
        assert_eq!(options.rescale, Some(RescaleTarget { width: None, height: Some(600) }));
    }

    #[test]
    fn recompress_with_rescale_accepts_both_dimensions() {
        let options = validate(Recompression::Small, true, "800", "600", "C:\\out").unwrap();
        assert_eq!(options.rescale, Some(RescaleTarget { width: Some(800), height: Some(600) }));
        assert_eq!(options.recompression, Recompression::Small);
    }

    #[test]
    fn recompress_without_rescale_checked_ignores_dimensions() {
        let options = validate(Recompression::Large, false, "", "", "C:\\out").unwrap();
        assert_eq!(options.rescale, None);
    }

    #[test]
    fn trims_the_output_directory() {
        let options = validate(Recompression::None, false, "", "", "  C:\\out  ").unwrap();
        assert_eq!(options.output_dir, PathBuf::from("C:\\out"));
    }
}
