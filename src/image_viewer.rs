use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::thread;

use native_windows_derive::NwgUi;
use native_windows_gui as nwg;
use nwg::stretch::geometry::Size;
use nwg::stretch::style::{Dimension as D, FlexDirection};
use nwg::NativeUi;

use crate::context_tabs;
use crate::db::models::ImageRecord;
use crate::explorer;
use crate::image_viewer_shortcuts::{self, ViewerAction};
use crate::keyboard;

/// Height of the top row (status label, Prev/Next buttons).
const TOP_ROW_HEIGHT: f32 = 28.0;
/// Width of the Prev/Next buttons.
const NAV_BUTTON_WIDTH: f32 = 60.0;
/// Width of the Toggle RAW button, wider than Prev/Next since its label is longer.
const TOGGLE_RAW_BUTTON_WIDTH: f32 = 90.0;

/// Opens the Image Viewer on its own thread for one event's photos, starting at `start_index`.
/// The window's title is a snapshot of `event_title` at open time — unlike an event tab's live
/// title input, it's never re-synced if the title is edited afterwards. `linked_images` maps a
/// displayed photo's own `key` to its linked RAW/compressed counterpart `ImageRecord` (when
/// "Link RAW and compressed images" has paired it) — the Toggle RAW button's data source, see
/// `record_to_display`.
///
/// Runs on its own thread rather than a separate process (see `CLAUDE.md`/the design plan):
/// NWG requires one message loop per thread, so a second *window* already can't share the main
/// thread's loop — `settings_modal.rs` established this same thread-per-window pattern. All the
/// data the viewer needs (the event's `ImageRecord`s, already carrying every EXIF field the
/// metadata dialog shows, plus the project's Source Directory) is already loaded, `Clone`-able
/// Rust data, so a thread closure can just take ownership of it — no IPC a separate process
/// would need is involved. Unlike the Settings dialog, nothing needs to flow back to `App` when
/// this window closes, so it's fire-and-forget: no `nwg::Notice`/`JoinHandle` bookkeeping.
pub fn open(
    event_title: String,
    images: Vec<ImageRecord>,
    linked_images: HashMap<String, ImageRecord>,
    start_index: usize,
    source_dir: PathBuf,
) {
    thread::spawn(move || {
        // `nwg::ImageDecoder` creates its WIC factory via `CoCreateInstance`, which requires COM
        // initialized on the calling thread. `nwg::init()` (main thread only) initializes COM as
        // a side effect, but this is a *different* thread, so it needs its own initialization —
        // the same call `main.rs` makes before building the main window.
        unsafe {
            winapi::um::combaseapi::CoInitializeEx(
                std::ptr::null_mut(),
                winapi::um::objbase::COINIT_APARTMENTTHREADED,
            );
        }

        let viewer =
            ImageViewer::build_ui(Default::default()).expect("Failed to build the Image Viewer window");
        viewer.window.set_text(&event_title);
        *viewer.images.borrow_mut() = images;
        *viewer.linked_images.borrow_mut() = linked_images;
        *viewer.source_dir.borrow_mut() = source_dir;
        *viewer.current_index.borrow_mut() = start_index;

        nwg::dispatch_thread_events();
    });
}

#[derive(Default, NwgUi)]
pub struct ImageViewer {
    images: RefCell<Vec<ImageRecord>>,
    /// Every displayed photo's linked RAW/compressed counterpart, keyed by the photo's own
    /// `key` (`images[i].key`) — populated once at open time by `App::open_image_viewer`. See
    /// `record_to_display`.
    linked_images: RefCell<HashMap<String, ImageRecord>>,
    source_dir: RefCell<PathBuf>,
    current_index: RefCell<usize>,
    /// Whether the Toggle RAW button's counterpart view is active for `current_index`. Reset to
    /// `false` on every Prev/Next, so a toggled RAW view never silently carries over to the
    /// next photo.
    showing_counterpart: RefCell<bool>,
    /// Must be kept alive as long as it's shown — `ImageFrame::set_bitmap` doesn't take
    /// ownership, it just points the control at whatever `Bitmap` is passed in.
    loaded_bitmap: RefCell<Option<nwg::Bitmap>>,
    /// Keeps the `OnKeyPress` subclass hook from `setup` alive for the window's lifetime. See
    /// that method's doc comment for why a plain `#[nwg_events(OnKeyPress: ...)]` on `window`
    /// wouldn't be enough.
    key_press_handler: RefCell<Option<nwg::EventHandler>>,

    #[nwg_resource]
    decoder: nwg::ImageDecoder,

    #[nwg_control(size: (1000, 750), position: (250, 120), title: "", flags: "MAIN_WINDOW")]
    #[nwg_events(
        OnInit: [ImageViewer::setup(RC_SELF)],
        OnWindowClose: [ImageViewer::close],
        OnResize: [ImageViewer::on_resize],
    )]
    window: nwg::Window,

    #[nwg_control(parent: window, text: "&File")]
    file_menu: nwg::Menu,

    #[nwg_control(parent: file_menu, text: "&Close\tCtrl+W")]
    #[nwg_events(OnMenuItemSelected: [ImageViewer::close])]
    file_close: nwg::MenuItem,

    #[nwg_control(parent: window, text: "&Edit")]
    edit_menu: nwg::Menu,

    #[nwg_control(parent: edit_menu, text: "&Meta Data...")]
    #[nwg_events(OnMenuItemSelected: [ImageViewer::show_metadata])]
    edit_metadata: nwg::MenuItem,

    #[nwg_control(parent: window, text: "&Help")]
    help_menu: nwg::Menu,

    #[nwg_control(parent: help_menu, text: "&About")]
    #[nwg_events(OnMenuItemSelected: [ImageViewer::show_about])]
    help_about: nwg::MenuItem,

    #[nwg_control(parent: window, text: "")]
    status_label: nwg::Label,

    #[nwg_control(parent: window, text: "&Prev")]
    #[nwg_events(OnButtonClick: [ImageViewer::prev])]
    prev_button: nwg::Button,

    #[nwg_control(parent: window, text: "Ne&xt")]
    #[nwg_events(OnButtonClick: [ImageViewer::next])]
    next_button: nwg::Button,

    /// Enabled only when the current photo has a linked RAW/compressed counterpart. A static
    /// label rather than state-describing text ("Show RAW"/"Show JPG") so its mnemonic never
    /// needs to change on toggle. No accelerator per `CLAUDE.md`: there's no Windows-standard
    /// key combination for this action, so none is invented — mnemonic only.
    #[nwg_control(parent: window, text: "Toggle &RAW")]
    #[nwg_events(OnButtonClick: [ImageViewer::toggle_raw_compressed])]
    toggle_raw_button: nwg::Button,

    #[nwg_control(parent: window, flags: "VISIBLE", background_color: Some([255, 255, 255]))]
    image_frame: nwg::ImageFrame,

    top_row_layout: nwg::FlexboxLayout,
    root_layout: nwg::FlexboxLayout,
}

impl ImageViewer {
    /// Fired once via `OnInit`, after `open` has already populated `images`/`source_dir`/
    /// `current_index` (posted, so it only runs once `nwg::dispatch_thread_events` starts
    /// pumping messages — by then that state is already set).
    ///
    /// Builds the layout, shows the first image, then makes the window visible — avoiding a
    /// flash of unlaid-out controls, the same "build while hidden, `set_visible(true)` last"
    /// order `App::init` uses for the main window.
    ///
    /// Also binds the one `OnKeyPress` hook that makes Left/Right/Ctrl+W work no matter which
    /// control has keyboard focus. A plain `#[nwg_events(OnKeyPress: ...)]` on `window` (as
    /// `App` uses for its own window) only fires when `window`'s own `HWND` has focus — the
    /// moment the user clicks Prev/Next, focus moves to that button and stops working. Dynamic
    /// Context Window tabs hit the same problem (see `app.rs`'s `build_event_tab_entry`) and
    /// solve it the same way: `full_bind_event_handler` recursively subclasses `window` and
    /// every control under it, and matching on the event alone (no handle check) means it
    /// fires regardless of which of those controls is focused. Needs `&Rc<Self>` (via
    /// `RC_SELF`) for the closure to stay `'static`.
    fn setup(app: &Rc<ImageViewer>) {
        app.build_layout();

        let app_weak = Rc::downgrade(app);
        let handler = nwg::full_bind_event_handler(&app.window.handle, move |evt, evt_data, _handle| {
            if evt == nwg::Event::OnKeyPress {
                if let Some(app) = app_weak.upgrade() {
                    app.on_key_press(&evt_data);
                }
            }
        });
        *app.key_press_handler.borrow_mut() = Some(handler);

        app.show_current_image();
        app.window.set_visible(true);
    }

    /// Lays out the top row (status label flex-growing to the left, Prev/Next pinned right) and
    /// the image area filling the rest — the same "partial row layout nested into an outer
    /// column via `child_layout`" technique `app.rs`'s `build_event_tab_entry` uses for its
    /// Title row, which avoids needing an extra `Frame` just to group the row's controls.
    fn build_layout(&self) {
        nwg::FlexboxLayout::builder()
            .parent(&self.window)
            .flex_direction(FlexDirection::Row)
            .child(&self.status_label)
            .child_size(Size { width: D::Auto, height: D::Points(TOP_ROW_HEIGHT) })
            .child_flex_grow(1.0)
            .child(&self.prev_button)
            .child_size(Size { width: D::Points(NAV_BUTTON_WIDTH), height: D::Points(TOP_ROW_HEIGHT) })
            .child(&self.next_button)
            .child_size(Size { width: D::Points(NAV_BUTTON_WIDTH), height: D::Points(TOP_ROW_HEIGHT) })
            .child(&self.toggle_raw_button)
            .child_size(Size { width: D::Points(TOGGLE_RAW_BUTTON_WIDTH), height: D::Points(TOP_ROW_HEIGHT) })
            .build_partial(&self.top_row_layout)
            .expect("Failed to build the Image Viewer's top row layout");

        nwg::FlexboxLayout::builder()
            .parent(&self.window)
            .flex_direction(FlexDirection::Column)
            .child_layout(&self.top_row_layout)
            .child_size(Size { width: D::Percent(1.0), height: D::Points(TOP_ROW_HEIGHT) })
            .child(&self.image_frame)
            .child_size(Size { width: D::Percent(1.0), height: D::Auto })
            .child_flex_grow(1.0)
            .build(&self.root_layout)
            .expect("Failed to build the Image Viewer's root layout");
    }

    fn close(&self) {
        nwg::stop_thread_dispatch();
    }

    /// Re-fits the current image whenever the window is resized — `ImageFrame::set_bitmap`
    /// displays a bitmap at its own pixel size (centered, never scaled; see `show_current_image`),
    /// so without this the image would stay sized for whatever the window was when it was last
    /// decoded rather than the window's new size.
    fn on_resize(&self) {
        self.show_current_image();
    }

    fn on_key_press(&self, data: &nwg::EventData) {
        let ctrl = keyboard::key_down(winapi::um::winuser::VK_CONTROL);
        match image_viewer_shortcuts::resolve(data.on_key(), ctrl) {
            Some(ViewerAction::Close) => self.close(),
            Some(ViewerAction::Prev) => self.prev(),
            Some(ViewerAction::Next) => self.next(),
            None => {}
        }
    }

    /// No wraparound, unlike an event tab's looping Prev/Next — a no-op on the first image.
    /// The button being disabled already prevents a click here, but the Left-arrow path needs
    /// the same guard.
    fn prev(&self) {
        let mut index = self.current_index.borrow_mut();
        if *index == 0 {
            return;
        }
        *index -= 1;
        drop(index);
        *self.showing_counterpart.borrow_mut() = false;
        self.show_current_image();
    }

    /// A no-op on the last image — see `prev`.
    fn next(&self) {
        let len = self.images.borrow().len();
        let mut index = self.current_index.borrow_mut();
        if *index + 1 >= len {
            return;
        }
        *index += 1;
        drop(index);
        *self.showing_counterpart.borrow_mut() = false;
        self.show_current_image();
    }

    /// Toggle RAW's click: flips between the current photo and its linked RAW/compressed
    /// counterpart (if any — the button is disabled otherwise, see `show_current_image`).
    fn toggle_raw_compressed(&self) {
        let mut showing = self.showing_counterpart.borrow_mut();
        *showing = !*showing;
        drop(showing);
        self.show_current_image();
    }

    /// Decodes and displays the current image, updates the status label (path plus "N of M"),
    /// and enables/disables Prev/Next for the new position.
    ///
    /// WIC (the decoder backing `nwg::ImageDecoder`) has no built-in RAW codec, so a `.CR2` (or
    /// other RAW) file fails to decode — rather than treat that as fatal, this just leaves
    /// `image_frame` blank and appends "preview unavailable" to the status label, so browsing
    /// and the Meta Data dialog keep working for it.
    fn show_current_image(&self) {
        let images = self.images.borrow();
        let index = *self.current_index.borrow();
        let Some(native) = images.get(index) else { return };
        let linked_images = self.linked_images.borrow();
        let has_counterpart = linked_images.contains_key(&native.key);
        let showing_counterpart = *self.showing_counterpart.borrow() && has_counterpart;
        let Some(record) = record_to_display(&images, &linked_images, index, showing_counterpart) else { return };

        let source_dir = self.source_dir.borrow();
        let path = explorer::resolve_path(source_dir.as_path(), &record.path);

        let bitmap = self
            .decoder
            .from_filename(&path.to_string_lossy())
            .and_then(|source| source.frame(0))
            .and_then(|frame| self.fit_frame_to_view(&frame))
            .ok();

        let suffix = if bitmap.is_some() { "" } else { " \u{2014} preview unavailable" };
        self.status_label.set_text(&format!("{} ({} of {}){}", record.path, index + 1, images.len(), suffix));

        *self.loaded_bitmap.borrow_mut() = bitmap;
        self.image_frame.set_bitmap(self.loaded_bitmap.borrow().as_ref());

        self.toggle_raw_button.set_enabled(has_counterpart);
        self.prev_button.set_enabled(index > 0);
        self.next_button.set_enabled(index + 1 < images.len());
    }

    /// Scales a decoded frame down or up to fit `image_frame`'s current size, preserving aspect
    /// ratio. `nwg::ImageFrame` centers a bitmap smaller than the control and crops one larger
    /// than it, but never scales (confirmed by reading `image_frame.rs`) — without this, a
    /// full-resolution DSLR photo would only ever show its center crop.
    fn fit_frame_to_view(&self, frame: &nwg::ImageData) -> Result<nwg::Bitmap, nwg::NwgError> {
        let (frame_w, frame_h) = self.image_frame.size();
        let (img_w, img_h) = frame.size();
        if frame_w == 0 || frame_h == 0 || img_w == 0 || img_h == 0 {
            return frame.as_bitmap();
        }

        let scale = (frame_w as f64 / img_w as f64).min(frame_h as f64 / img_h as f64);
        let target = [
            ((img_w as f64 * scale).round() as u32).max(1),
            ((img_h as f64 * scale).round() as u32).max(1),
        ];
        self.decoder.resize_image(frame, target)?.as_bitmap()
    }

    /// Edit > Meta Data...: every database field for the current photo, in a dialog locked to
    /// this window (`nwg::modal_info_message`, unlike `nwg::simple_message`, takes a parent and
    /// disables it for the duration — a genuine modal). All of it is already sitting in the
    /// `ImageRecord` fetched when the viewer opened, so this needs no new database query.
    fn show_metadata(&self) {
        let images = self.images.borrow();
        let index = *self.current_index.borrow();
        let linked_images = self.linked_images.borrow();
        let showing_counterpart = *self.showing_counterpart.borrow();
        let Some(record) = record_to_display(&images, &linked_images, index, showing_counterpart) else { return };
        nwg::modal_info_message(&self.window, "Image Metadata", &format_metadata(record));
    }

    fn show_about(&self) {
        nwg::simple_message("About PhotoMatic Image Viewer", "PhotoMatic image viewer. Tischer 2026");
    }
}

/// Every `ImageRecord` field as a label:value line, for the Meta Data dialog. Reuses
/// `context_tabs`'s exposure time/GPS formatting so the same photo reads identically here and
/// in the photo table.
fn format_metadata(record: &ImageRecord) -> String {
    let date_taken = record.date_taken.map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string()).unwrap_or_default();
    let metadata_read_at =
        record.metadata_read_at.map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string()).unwrap_or_default();

    format!(
        "Path: {}\nType: {}\nDirectory: {}\nDate taken: {}\nWidth: {}\nHeight: {}\nExposure time: {}\nISO: {}\nFocal length: {}\nLocation: {}\nAltitude: {}\nMetadata read at: {}",
        record.path,
        record.image_type,
        record.toplevel_dir.as_deref().unwrap_or(""),
        date_taken,
        record.width.map(|w| w.to_string()).unwrap_or_default(),
        record.height.map(|h| h.to_string()).unwrap_or_default(),
        context_tabs::format_exposure_time(record.exposure_time),
        record.iso.map(|i| i.to_string()).unwrap_or_default(),
        record.focal_length.map(|f| format!("{f:.1}mm")).unwrap_or_default(),
        context_tabs::format_gps_coordinates(record.gps_latitude, record.gps_longitude),
        context_tabs::format_gps_altitude(record.gps_altitude),
        metadata_read_at,
    )
}

/// The `ImageRecord` `show_current_image`/`show_metadata` should render for `index`: the
/// linked counterpart from `linked_images` when `showing_counterpart` is set and one exists
/// for the photo at `index`, otherwise the photo at `index` itself. `None` only when `index`
/// is out of `images`' bounds. Kept as a free function, independent of NWG's controls/`RefCell`
/// borrows, so it's unit-testable without a window.
fn record_to_display<'a>(
    images: &'a [ImageRecord],
    linked_images: &'a HashMap<String, ImageRecord>,
    index: usize,
    showing_counterpart: bool,
) -> Option<&'a ImageRecord> {
    let native = images.get(index)?;
    if showing_counterpart {
        Some(linked_images.get(&native.key).unwrap_or(native))
    } else {
        Some(native)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(key: &str, path: &str) -> ImageRecord {
        ImageRecord { key: key.to_string(), path: path.to_string(), image_type: "jpg".to_string(), ..ImageRecord::default() }
    }

    #[test]
    fn record_to_display_returns_native_when_not_toggled() {
        let images = vec![image("a", "a.jpg")];
        let linked = HashMap::from([("a".to_string(), image("a-raw", "a.cr2"))]);

        let record = record_to_display(&images, &linked, 0, false).unwrap();

        assert_eq!(record.key, "a");
    }

    #[test]
    fn record_to_display_returns_counterpart_when_toggled_and_present() {
        let images = vec![image("a", "a.jpg")];
        let linked = HashMap::from([("a".to_string(), image("a-raw", "a.cr2"))]);

        let record = record_to_display(&images, &linked, 0, true).unwrap();

        assert_eq!(record.key, "a-raw");
    }

    #[test]
    fn record_to_display_falls_back_to_native_when_toggled_but_no_counterpart() {
        let images = vec![image("a", "a.jpg")];
        let linked = HashMap::new();

        let record = record_to_display(&images, &linked, 0, true).unwrap();

        assert_eq!(record.key, "a");
    }

    #[test]
    fn record_to_display_returns_none_for_out_of_bounds_index() {
        let images = vec![image("a", "a.jpg")];
        let linked = HashMap::new();

        assert!(record_to_display(&images, &linked, 5, false).is_none());
    }
}
