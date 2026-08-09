use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc;
use std::thread;

use native_windows_derive::NwgUi;
use native_windows_gui as nwg;
use nwg::stretch::geometry::Size;
use nwg::stretch::style::{Dimension as D, FlexDirection};
use nwg::NativeUi;

use crate::context_tabs;
use crate::db;
use crate::db::models::{CollectionRecord, ImageRecord};
use crate::explorer;
use crate::image_cache::ImageCache;
use crate::image_viewer_shortcuts::{self, ViewerAction};
use crate::keyboard;

/// How many decoded/fitted bitmaps the Image Viewer keeps around at once: the current photo
/// plus every photo `prefetch_targets` looks ahead/behind to. Sized to exactly fit that set (see
/// `prefetch_targets`) so a normal Prev/Next sequence never evicts a photo the user is about to
/// land on.
const IMAGE_CACHE_CAPACITY: usize = 5;

/// Height of the top row (status label, Prev/Next buttons) and the collection buttons row.
const TOP_ROW_HEIGHT: f32 = 28.0;
/// Width of the Prev/Next buttons.
const NAV_BUTTON_WIDTH: f32 = 60.0;
/// Width of the Toggle RAW button, wider than Prev/Next since its label is longer.
const TOGGLE_RAW_BUTTON_WIDTH: f32 = 90.0;
/// Width of one collection toggle button — just its shortcut letter, so this stays narrow.
const COLLECTION_BUTTON_WIDTH: f32 = 40.0;

/// Opens the Image Viewer on its own thread for one event's or collection's photos, starting
/// at `start_index`. The window's title is a snapshot of `title` at open time — unlike an
/// event tab's live title input, it's never re-synced if the title is edited afterwards.
/// `linked_images` maps a displayed photo's own `key` to its linked RAW/compressed counterpart
/// `ImageRecord` (when "Link RAW and compressed images" has paired it) — the Toggle RAW
/// button's data source, see `record_to_display`. `collections` is every collection in the
/// project (for one toggle button each) and `membership` maps a displayed photo's own `key` to
/// the collection ids it currently belongs to — both preloaded once, the same way
/// `linked_images` is. `db_path` is the project database's absolute path, used only when a
/// collection toggle button is actually clicked (see `toggle_collection`) — the viewer
/// otherwise never touches the database itself.
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
    title: String,
    images: Vec<ImageRecord>,
    linked_images: HashMap<String, ImageRecord>,
    start_index: usize,
    source_dir: PathBuf,
    collections: Vec<CollectionRecord>,
    membership: HashMap<String, HashSet<i64>>,
    db_path: PathBuf,
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
        viewer.window.set_text(&title);
        *viewer.images.borrow_mut() = images;
        *viewer.linked_images.borrow_mut() = linked_images;
        *viewer.source_dir.borrow_mut() = source_dir;
        *viewer.current_index.borrow_mut() = start_index;
        *viewer.collections.borrow_mut() = collections;
        *viewer.membership.borrow_mut() = membership;
        *viewer.db_path.borrow_mut() = db_path;
        *viewer.image_cache.borrow_mut() = ImageCache::new(IMAGE_CACHE_CAPACITY);

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
    /// Decoded/fitted bitmaps for recently-shown photos, keyed by `(index, showing_counterpart)`
    /// — capped at `IMAGE_CACHE_CAPACITY` (see `prefetch_targets`). `ImageFrame::set_bitmap`
    /// doesn't take ownership, it just points the control at whatever `Bitmap` is passed in, so
    /// the entry displayed right now must stay alive in here for as long as it's on screen; since
    /// `get` promotes it to most-recently-used on every `show_current_image` call, a plain
    /// Prev/Next sequence never evicts it.
    image_cache: RefCell<ImageCache<(usize, bool), nwg::Bitmap>>,
    /// Indices currently being decoded by a `schedule_prefetch` background thread, so a second
    /// thread is never spawned for the same photo while the first one is still running.
    prefetch_in_flight: RefCell<HashSet<usize>>,
    /// Sending half of the prefetch result channel, cloned into every background prefetch
    /// thread `schedule_prefetch` spawns. `None` until `setup` creates the channel.
    prefetch_tx: RefCell<Option<mpsc::Sender<PrefetchResult>>>,
    /// Receiving half of the same channel, drained by `apply_prefetch_results` on `OnNotice`.
    prefetch_rx: RefCell<Option<mpsc::Receiver<PrefetchResult>>>,
    /// Keeps the `OnKeyPress`/`OnButtonClick` subclass hook from `setup` alive for the
    /// window's lifetime. See that method's doc comment for why a plain
    /// `#[nwg_events(OnKeyPress: ...)]` on `window` wouldn't be enough.
    key_press_handler: RefCell<Option<nwg::EventHandler>>,
    /// Every collection in the project, preloaded once at open time — the source list
    /// `build_collection_buttons` builds one toggle button from.
    collections: RefCell<Vec<CollectionRecord>>,
    /// A displayed photo's own `key` -> the collection ids it currently belongs to, preloaded
    /// once at open time and kept in sync locally by `toggle_collection` on every click (so a
    /// button's state never needs a fresh database read to redraw). Membership is a property
    /// of the *photo*, unlike `showing_counterpart` — it must never be reset on Prev/Next.
    membership: RefCell<HashMap<String, HashSet<i64>>>,
    /// The project database's absolute path — opened fresh (and briefly) by
    /// `toggle_collection` each time a collection button is actually clicked; nothing else in
    /// this viewer touches the database.
    db_path: RefCell<PathBuf>,
    /// One `(checkbox, collection_id)` pair per collection, built once by
    /// `build_collection_buttons` — `setup`'s click dispatch matches a clicked handle against
    /// this to know which collection to toggle, and `refresh_collection_buttons` iterates it
    /// to redraw checked state for the current photo.
    collection_buttons: RefCell<Vec<(nwg::CheckBox, i64)>>,

    #[nwg_resource]
    decoder: nwg::ImageDecoder,

    #[nwg_control(size: (1000, 750), position: (250, 120), title: "", flags: "MAIN_WINDOW")]
    #[nwg_events(
        OnInit: [ImageViewer::setup(RC_SELF)],
        OnWindowClose: [ImageViewer::close],
        OnResize: [ImageViewer::on_resize],
    )]
    window: nwg::Window,

    /// Fires once per background prefetch decode that lands — see `schedule_prefetch`/
    /// `apply_prefetch_results`. Must be declared after `window`: native-windows-derive's
    /// auto-parent detection only looks *backward* through already-declared controls for one
    /// that can parent a child (see `AUTO_PARENT` in native-windows-derive's `ui.rs`), and a
    /// `Notice` with no parent found fails to build at all — `open`'s `.expect(...)` on
    /// `build_ui` would then panic on every single open, silently killing the viewer's thread
    /// before its window ever appears.
    #[nwg_control]
    #[nwg_events(OnNotice: [ImageViewer::apply_prefetch_results])]
    prefetch_notice: nwg::Notice,

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
    collection_row_layout: nwg::FlexboxLayout,
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
    /// Also binds the one `OnKeyPress` hook that makes Left/Right/Ctrl+W, and each collection's
    /// shortcut letter, work no matter which control has keyboard focus. A plain
    /// `#[nwg_events(OnKeyPress: ...)]` on `window` (as
    /// `App` uses for its own window) only fires when `window`'s own `HWND` has focus — the
    /// moment the user clicks Prev/Next, focus moves to that button and stops working. Dynamic
    /// Context Window tabs hit the same problem (see `app.rs`'s `build_event_tab_entry`) and
    /// solve it the same way: `full_bind_event_handler` recursively subclasses `window` and
    /// every control under it, and matching on the event alone (no handle check) means it
    /// fires regardless of which of those controls is focused. Needs `&Rc<Self>` (via
    /// `RC_SELF`) for the closure to stay `'static`.
    fn setup(app: &Rc<ImageViewer>) {
        app.build_collection_buttons();
        app.build_layout();

        let (tx, rx) = mpsc::channel();
        *app.prefetch_tx.borrow_mut() = Some(tx);
        *app.prefetch_rx.borrow_mut() = Some(rx);

        // One shared hook for both `OnKeyPress` (Left/Right/Ctrl+W, see this method's doc
        // comment above) and `OnButtonClick` on the collection toggle buttons — the latter are
        // built at runtime in `build_collection_buttons`, so (like the dynamic tabs in
        // `app.rs`'s `build_event_tab_entry`) they're invisible to `native_windows_derive`'s
        // one-time `#[nwg_events(...)]` dispatch and need their own subclass hook, matched by
        // handle against `collection_buttons` rather than declared per-control.
        let app_weak = Rc::downgrade(app);
        let handler = nwg::full_bind_event_handler(&app.window.handle, move |evt, evt_data, handle| {
            let Some(app) = app_weak.upgrade() else { return };
            match evt {
                nwg::Event::OnKeyPress => app.on_key_press(&evt_data),
                nwg::Event::OnButtonClick => {
                    let collection_id =
                        app.collection_buttons.borrow().iter().find(|(cb, _)| cb.handle == handle).map(|(_, id)| *id);
                    if let Some(collection_id) = collection_id {
                        app.toggle_collection(collection_id);
                    }
                }
                _ => {}
            }
        });
        *app.key_press_handler.borrow_mut() = Some(handler);

        app.show_current_image();
        app.window.set_visible(true);
    }

    /// Builds one `nwg::CheckBox` per collection, labeled with its shortcut — a `CheckBox`'s
    /// real checked/unchecked visual is reused as the "pressed" look for collection
    /// membership, the same idiom `app.rs` already uses for the File Types checkboxes, rather
    /// than inventing a new control style. The label's `&` mnemonic is the collection's own
    /// shortcut letter, so Alt+<shortcut> toggles it too — since these buttons are built at
    /// runtime (unknown at compile time) rather than declared with a `\t`-suffixed accelerator
    /// label, this is the natural per-`CLAUDE.md` way to give each one a keyboard shortcut
    /// without inventing an unrelated binding; collisions can't happen since shortcuts are
    /// enforced unique when a collection is created/edited.
    fn build_collection_buttons(&self) {
        let mut buttons = Vec::new();
        for collection in self.collections.borrow().iter() {
            let mut checkbox = nwg::CheckBox::default();
            nwg::CheckBox::builder()
                .parent(&self.window)
                .text(&format!("&{}", collection.shortcut))
                .build(&mut checkbox)
                .expect("Failed to build a collection toggle button");
            buttons.push((checkbox, collection.id));
        }

        let mut builder = nwg::FlexboxLayout::builder().parent(&self.window).flex_direction(FlexDirection::Row);
        for (checkbox, _) in &buttons {
            builder = builder
                .child(checkbox)
                .child_size(Size { width: D::Points(COLLECTION_BUTTON_WIDTH), height: D::Points(TOP_ROW_HEIGHT) });
        }
        builder.build_partial(&self.collection_row_layout).expect("Failed to build the Image Viewer's collection row layout");

        *self.collection_buttons.borrow_mut() = buttons;
    }

    /// Lays out the top row (status label flex-growing to the left, Prev/Next pinned right),
    /// the collection toggle buttons row (built by `build_collection_buttons`, called before
    /// this), and the image area filling the rest — the same "partial row layout nested into
    /// an outer column via `child_layout`" technique `app.rs`'s `build_event_tab_entry` uses
    /// for its Title row, which avoids needing an extra `Frame` just to group each row's
    /// controls.
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
            .child_layout(&self.collection_row_layout)
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
    /// decoded rather than the window's new size. Every cached/in-flight bitmap was fit to the
    /// *old* size, so both are dropped/abandoned first — `apply_prefetch_results` also guards
    /// against a stale in-flight decode landing after this by comparing frame sizes, in case one
    /// was already on its way back from a thread this clears from `prefetch_in_flight`.
    fn on_resize(&self) {
        self.image_cache.borrow_mut().clear();
        self.prefetch_in_flight.borrow_mut().clear();
        self.show_current_image();
    }

    fn on_key_press(&self, data: &nwg::EventData) {
        let ctrl = keyboard::key_down(winapi::um::winuser::VK_CONTROL);
        let key = data.on_key();
        match image_viewer_shortcuts::resolve(key, ctrl) {
            Some(ViewerAction::Close) => return self.close(),
            Some(ViewerAction::Prev) => return self.prev(),
            Some(ViewerAction::Next) => return self.next(),
            None => {}
        }

        let shortcuts: Vec<(char, i64)> = self
            .collections
            .borrow()
            .iter()
            .filter_map(|c| c.shortcut.chars().next().map(|ch| (ch, c.id)))
            .collect();
        if let Some(collection_id) = image_viewer_shortcuts::resolve_collection_shortcut(key, ctrl, &shortcuts) {
            self.toggle_collection(collection_id);
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

    /// Displays the current image, updates the status label (path plus "N of M"), and
    /// enables/disables Prev/Next for the new position. Serves the bitmap from `image_cache`
    /// when `schedule_prefetch` (from a previous navigation) already decoded it in the
    /// background; otherwise decodes it synchronously here, same as before prefetching existed —
    /// so the first-ever photo, or a jump the cache didn't anticipate, still always shows
    /// something, just without the speedup.
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
        let frame_size = self.image_frame.size();
        let cache_key = (index, showing_counterpart);

        let mut cache = self.image_cache.borrow_mut();
        if cache.get(cache_key).is_none() {
            if let Some(bitmap) = decode_and_fit(&self.decoder, &path, frame_size) {
                cache.insert(cache_key, bitmap);
            }
        }
        let bitmap = cache.get(cache_key);

        let suffix = if bitmap.is_some() { "" } else { " \u{2014} preview unavailable" };
        self.status_label.set_text(&format!("{} ({} of {}){}", record.path, index + 1, images.len(), suffix));
        self.image_frame.set_bitmap(bitmap);
        drop(cache);

        self.toggle_raw_button.set_enabled(has_counterpart);
        self.prev_button.set_enabled(index > 0);
        self.next_button.set_enabled(index + 1 < images.len());
        self.refresh_collection_buttons(&record.key);

        drop(source_dir);
        drop(linked_images);
        drop(images);
        self.schedule_prefetch();
    }

    /// Kicks off a background decode for every photo `prefetch_targets` says should be ready
    /// ahead of time (skipping one already cached or already being decoded), so a following
    /// Prev/Next lands on a warm `image_cache` entry instead of blocking on disk I/O and WIC
    /// decode/resize on the UI thread. Each spawned thread only touches plain owned data (a path
    /// and a target size) and reports back over `prefetch_tx`/`prefetch_notice` — never `self` —
    /// so it needs no synchronization with the UI thread beyond that channel.
    fn schedule_prefetch(&self) {
        let current = *self.current_index.borrow();
        let len = self.images.borrow().len();
        let frame_size = self.image_frame.size();
        if frame_size.0 == 0 || frame_size.1 == 0 {
            return;
        }
        let Some(tx) = self.prefetch_tx.borrow().clone() else { return };

        for index in prefetch_targets(current, len) {
            let cache_key = (index, false);
            if self.image_cache.borrow_mut().get(cache_key).is_some() {
                continue;
            }
            if !self.prefetch_in_flight.borrow_mut().insert(index) {
                continue;
            }

            let Some(record) = self.images.borrow().get(index).cloned() else { continue };
            let path = explorer::resolve_path(self.source_dir.borrow().as_path(), &record.path);
            let tx = tx.clone();
            let notice = self.prefetch_notice.sender();

            thread::spawn(move || {
                // Same reasoning as `image_viewer::open`'s own thread: WIC decoding needs COM
                // initialized on the calling thread, and this is a fresh one.
                unsafe {
                    winapi::um::combaseapi::CoInitializeEx(
                        std::ptr::null_mut(),
                        winapi::um::objbase::COINIT_APARTMENTTHREADED,
                    );
                }
                // A fresh `ImageDecoder` rather than reusing the viewer's own: its WIC factory
                // is a COM object bound to the thread that created it, so it can't be shared
                // with a background thread's own apartment.
                if let Ok(decoder) = nwg::ImageDecoder::new() {
                    if let Some(bitmap) = decode_and_fit(&decoder, &path, frame_size) {
                        let _ = tx.send(PrefetchResult { index, frame_size, bitmap: SendBitmap(bitmap) });
                        notice.notice();
                    }
                }
                unsafe {
                    winapi::um::combaseapi::CoUninitialize();
                }
            });
        }
    }

    /// Fired via `OnNotice` whenever a `schedule_prefetch` thread lands a decoded bitmap. Drains
    /// every result currently waiting (more than one can arrive between two pumps of the message
    /// loop) and files each into `image_cache` — unless the window was resized since that thread
    /// was spawned, in which case the bitmap was fit to a now-wrong size and is simply dropped;
    /// `on_resize` already cleared `prefetch_in_flight`/`image_cache` for the new size, and a
    /// fresh prefetch for the current size will follow from the next `show_current_image`.
    fn apply_prefetch_results(&self) {
        let current_frame_size = self.image_frame.size();
        let rx = self.prefetch_rx.borrow();
        let Some(rx) = rx.as_ref() else { return };
        while let Ok(result) = rx.try_recv() {
            self.prefetch_in_flight.borrow_mut().remove(&result.index);
            if result.frame_size == current_frame_size {
                self.image_cache.borrow_mut().insert((result.index, false), result.bitmap.0);
            }
        }
    }

    /// Sets every collection toggle button's checked state from `membership`'s entry for
    /// `image_key` — called at the end of `show_current_image` so this always reflects
    /// whichever photo (native or, if Toggle RAW is active, its counterpart) is actually being
    /// shown. Unlike `showing_counterpart`, membership is a property of the *photo*, not a
    /// per-navigation UI toggle, so it's never reset here — only freshly read for the new key.
    fn refresh_collection_buttons(&self, image_key: &str) {
        let membership = self.membership.borrow();
        let member_of = membership.get(image_key);
        for (checkbox, collection_id) in self.collection_buttons.borrow().iter() {
            let checked = member_of.map(|set| set.contains(collection_id)).unwrap_or(false);
            checkbox.set_check_state(if checked { nwg::CheckBoxState::Checked } else { nwg::CheckBoxState::Unchecked });
        }
    }

    /// A collection toggle button's click: flips whether the currently displayed photo belongs
    /// to that collection, in both the in-memory `membership` map (so the button's own state
    /// stays right without a database round trip) and the database — via a fresh, short-lived
    /// connection to `db_path`, the one place this viewer touches the database live, since
    /// everything else it shows was preloaded once at open time.
    fn toggle_collection(&self, collection_id: i64) {
        let images = self.images.borrow();
        let index = *self.current_index.borrow();
        let linked_images = self.linked_images.borrow();
        let showing_counterpart = *self.showing_counterpart.borrow();
        let Some(record) = record_to_display(&images, &linked_images, index, showing_counterpart) else { return };
        let key = record.key.clone();
        drop(linked_images);
        drop(images);

        let now_member = {
            let mut membership = self.membership.borrow_mut();
            let set = membership.entry(key.clone()).or_default();
            let now_member = !set.contains(&collection_id);
            if now_member {
                set.insert(collection_id);
            } else {
                set.remove(&collection_id);
            }
            now_member
        };

        let db_path = self.db_path.borrow().clone();
        if let Ok(db) = db::ProjectDb::open(&db_path) {
            let result = if now_member {
                db.add_image_to_collection(collection_id, &key)
            } else {
                db.remove_image_from_collection(collection_id, &key)
            };
            if let Err(err) = result {
                nwg::simple_message("PhotoMatic", &format!("Failed to update collection: {err}"));
            }
        }

        self.refresh_collection_buttons(&key);
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

/// A `schedule_prefetch` thread's result, sent back over `prefetch_tx`/`prefetch_notice`. Carries
/// the `image_frame` size the bitmap was fit to (not just the index) so `apply_prefetch_results`
/// can detect and discard one that finished after a resize made it the wrong size.
struct PrefetchResult {
    index: usize,
    frame_size: (u32, u32),
    bitmap: SendBitmap,
}

/// Wraps `nwg::Bitmap` so a freshly decoded one can cross the `mpsc::channel` from a prefetch
/// thread back to the UI thread. Safe because a GDI bitmap handle (`HBITMAP`), unlike a window
/// handle, has no thread affinity — Windows allows using or deleting it from any thread. Only the
/// *decoding* that produces it (WIC, backed by COM) needs its own apartment-threaded COM per
/// thread, which `schedule_prefetch`'s spawned closure already sets up the same way
/// `image_viewer::open`'s own thread does.
struct SendBitmap(nwg::Bitmap);
unsafe impl Send for SendBitmap {}

/// Decodes `path` and scales it to fit inside `frame_size`, preserving aspect ratio — the
/// synchronous path in `show_current_image` and every `schedule_prefetch` background thread both
/// go through this, each with their own `nwg::ImageDecoder` (an `ImageDecoder`'s WIC factory is a
/// COM object bound to whichever thread created it, so a background thread can't reuse the
/// viewer's own).
fn decode_and_fit(decoder: &nwg::ImageDecoder, path: &Path, frame_size: (u32, u32)) -> Option<nwg::Bitmap> {
    let source = decoder.from_filename(&path.to_string_lossy()).ok()?;
    let frame = source.frame(0).ok()?;
    match fitted_target_size(frame_size, frame.size()) {
        Some(target) => decoder.resize_image(&frame, target).ok()?.as_bitmap().ok(),
        None => frame.as_bitmap().ok(),
    }
}

/// The pixel size an `img`-sized frame should be resized to so it fits inside `frame_size`
/// without changing its aspect ratio. `nwg::ImageFrame` centers a bitmap smaller than the control
/// and crops one larger than it, but never scales (confirmed by reading `image_frame.rs`) —
/// without this, a full-resolution DSLR photo would only ever show its center crop. `None` when
/// either size has a zero dimension (nothing meaningful to fit to/from), in which case the caller
/// falls back to the frame's native size.
fn fitted_target_size(frame_size: (u32, u32), img: (u32, u32)) -> Option<[u32; 2]> {
    let (frame_w, frame_h) = frame_size;
    let (img_w, img_h) = img;
    if frame_w == 0 || frame_h == 0 || img_w == 0 || img_h == 0 {
        return None;
    }

    let scale = (frame_w as f64 / img_w as f64).min(frame_h as f64 / img_h as f64);
    Some([((img_w as f64 * scale).round() as u32).max(1), ((img_h as f64 * scale).round() as u32).max(1)])
}

/// Which photo indices `schedule_prefetch` should try to have decoded ahead of time once
/// `current` is showing: the next two (favoring the direction Next moves, since that's the more
/// common browsing direction and the user's explicit ask — buffering "the next next image") and
/// the one before. Bounds-checked against `len` and never includes `current` itself. Kept as a
/// free function, independent of `RefCell`/threading, so it's unit-testable on its own.
fn prefetch_targets(current: usize, len: usize) -> Vec<usize> {
    let mut targets = Vec::new();
    if current + 1 < len {
        targets.push(current + 1);
    }
    if current + 2 < len {
        targets.push(current + 2);
    }
    if current > 0 {
        targets.push(current - 1);
    }
    targets
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

    #[test]
    fn prefetch_targets_looks_two_ahead_and_one_behind_in_the_middle() {
        assert_eq!(prefetch_targets(5, 10), vec![6, 7, 4]);
    }

    #[test]
    fn prefetch_targets_omits_out_of_bounds_neighbors_at_the_start() {
        assert_eq!(prefetch_targets(0, 10), vec![1, 2]);
    }

    #[test]
    fn prefetch_targets_omits_out_of_bounds_neighbors_at_the_end() {
        assert_eq!(prefetch_targets(9, 10), vec![8]);
    }

    #[test]
    fn prefetch_targets_is_empty_for_a_single_image() {
        assert_eq!(prefetch_targets(0, 1), Vec::<usize>::new());
    }

    #[test]
    fn fitted_target_size_scales_down_preserving_aspect_ratio() {
        let target = fitted_target_size((100, 100), (200, 100)).unwrap();
        assert_eq!(target, [100, 50]);
    }

    #[test]
    fn fitted_target_size_scales_up_preserving_aspect_ratio() {
        let target = fitted_target_size((400, 400), (100, 200)).unwrap();
        assert_eq!(target, [200, 400]);
    }

    #[test]
    fn fitted_target_size_is_none_for_a_zero_sized_frame() {
        assert!(fitted_target_size((0, 100), (200, 100)).is_none());
    }
}
