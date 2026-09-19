use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;

use chrono::Duration;
use native_windows_derive::NwgUi;
use native_windows_gui as nwg;
use nwg::NativeUi;

use crate::color_correction;
use crate::db::models::ImageRecord;
use crate::explorer;
use crate::image_viewer;
use crate::time_correction::{self, LaneSelection};

const THUMBNAIL_SIZE: f32 = 72.0;
const THUMB_BORDER: f32 = 3.0;
const THUMB_CELL: f32 = THUMBNAIL_SIZE + 2.0 * THUMB_BORDER;
const THUMB_GAP: f32 = 4.0;
const THUMBS_VISIBLE_PER_CARD: usize = 4;
const CARD_STRIP_VIEWPORT_WIDTH: f32 =
    THUMBS_VISIBLE_PER_CARD as f32 * THUMB_CELL + (THUMBS_VISIBLE_PER_CARD as f32 - 1.0) * THUMB_GAP;
const CARD_STRIP_HEIGHT: f32 = THUMB_CELL;
const SCROLLBAR_THICKNESS: f32 = 17.0;
const CARD_PADDING: f32 = 8.0;
const CARD_HEADER_HEIGHT: f32 = 22.0;
const CARD_STATUS_HEIGHT: f32 = 18.0;
const CARD_INNER_GAP: f32 = 4.0;
const CARD_WIDTH: f32 = CARD_STRIP_VIEWPORT_WIDTH + 2.0 * CARD_PADDING;
const CARD_HEIGHT: f32 = 2.0 * CARD_PADDING
    + CARD_HEADER_HEIGHT
    + CARD_INNER_GAP
    + CARD_STRIP_HEIGHT
    + CARD_INNER_GAP
    + SCROLLBAR_THICKNESS
    + CARD_INNER_GAP
    + CARD_STATUS_HEIGHT;
const CARDS_PER_ROW: usize = 2;
const CARD_GAP: f32 = 12.0;
const WINDOW_PADDING: f32 = 12.0;
const GRID_CONTENT_WIDTH: f32 = CARDS_PER_ROW as f32 * CARD_WIDTH + (CARDS_PER_ROW as f32 - 1.0) * CARD_GAP;
const DIALOG_WIDTH: f32 = 2.0 * WINDOW_PADDING + GRID_CONTENT_WIDTH + SCROLLBAR_THICKNESS;
const MAX_VISIBLE_ROWS: usize = 3;
const GRID_VIEWPORT_MAX_HEIGHT: f32 = MAX_VISIBLE_ROWS as f32 * CARD_HEIGHT + (MAX_VISIBLE_ROWS as f32 - 1.0) * CARD_GAP;
const FOOTER_HEIGHT: f32 = 30.0;
const FOOTER_BUTTON_WIDTH: f32 = 90.0;
/// How far one scrollbar arrow click moves the page-level grid: one card row plus its gap.
const GRID_LINE_STEP: i32 = (CARD_HEIGHT + CARD_GAP) as i32;
/// How far one scrollbar arrow click moves a card's thumbnail strip: one thumbnail plus its gap.
const STRIP_LINE_STEP: i32 = (THUMB_CELL + THUMB_GAP) as i32;

/// Handler id for the raw `WM_VSCROLL` subclass bound to `window.handle` (the page-level grid
/// scrollbar's parent). Must be `> 0xFFFF` (ids at or below that are reserved by NWG itself, see
/// `nwg::bind_raw_event_handler`'s doc comment) — matches `panel_background.rs`'s own starting
/// point for the same reason.
const VSCROLL_HANDLER_ID: usize = 0x1_0000;
/// Handler id for the raw `WM_HSCROLL` subclass bound to `grid_content.handle` (every card's
/// thumbnail-strip scrollbar shares this one parent, so one handler dispatches all of them —
/// see `init`'s doc comment).
const HSCROLL_HANDLER_ID: usize = 0x1_0001;

/// Opens the Set Time Correction dialog on its own thread and returns a handle whose `join()`
/// yields `Some(corrections)` — one `(toplevel_dir, offset)` pair per non-baseline lane, ready
/// for `ProjectDb::apply_time_corrections` — if the user hit Accept, or `None` on Cancel/close.
/// Same thread-per-dialog pattern as `settings_modal.rs`/`collection_modal.rs` (NWG only allows
/// one message loop per thread), plus `export_modal.rs`'s `CoInitializeEx`/`CoUninitialize`
/// bracket: this dialog decodes thumbnails via `nwg::ImageDecoder`, which needs COM initialized
/// on the thread that uses it.
///
/// `images` should be `ProjectDb::list_images_for_event_generation`'s result (already
/// RAW/compressed-linking aware) — grouped into lanes by `TimeCorrectionDialog::init`, fired via
/// `OnInit` once `dispatch`'s message loop starts, by which point `images`/`source_dir` below are
/// already populated (see `init`'s doc comment). `init` immediately shows a small "Generating
/// Thumbnails" spinner dialog and kicks off `generate_thumbnails` on a second, inner background
/// thread rather than decoding synchronously — `images` is the *entire* project's event-eligible
/// list, not scoped to one event, so decoding every photo before the window can even appear would
/// otherwise freeze it (invisibly, with no feedback) for as long as that takes.
pub fn open(
    images: Vec<ImageRecord>,
    source_dir: PathBuf,
    sender: nwg::NoticeSender,
) -> thread::JoinHandle<Option<Vec<(Option<String>, Duration)>>> {
    thread::spawn(move || {
        unsafe {
            winapi::um::combaseapi::CoInitializeEx(std::ptr::null_mut(), winapi::um::objbase::COINIT_APARTMENTTHREADED);
        }

        let dialog =
            TimeCorrectionDialog::build_ui(Default::default()).expect("Failed to build the Set Time Correction dialog");
        *dialog.images.borrow_mut() = images;
        *dialog.source_dir.borrow_mut() = source_dir;

        dispatch();

        unsafe {
            winapi::um::combaseapi::CoUninitialize();
        }

        sender.notice();
        dialog.result.take()
    })
}

/// A custom `GetMessageW` loop instead of plain `nwg::dispatch_thread_events()` — that call
/// routes every message through `IsDialogMessageW`, Win32's dialog-navigation helper, which
/// consumes Enter/Escape key-downs as default-button/cancel dialog navigation before
/// `OnKeyPress` would ever see them. `image_viewer.rs`'s message loop hits the identical problem
/// for Left/Right arrows and fixes it the same way: divert the keys this dialog needs straight
/// to dispatch (`bypasses_dialog_navigation`), leave every other message (Tab, mnemonics, etc.)
/// routed through `IsDialogMessageW` exactly as before, so normal dialog-style focus navigation
/// between Accept/Cancel is unaffected.
fn dispatch() {
    unsafe {
        let mut msg: winapi::um::winuser::MSG = std::mem::zeroed();
        while winapi::um::winuser::GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) != 0 {
            let consumed_by_dialog_navigation = !bypasses_dialog_navigation(msg.message, msg.wParam)
                && winapi::um::winuser::IsDialogMessageW(
                    winapi::um::winuser::GetAncestor(msg.hwnd, winapi::um::winuser::GA_ROOT),
                    &mut msg,
                ) != 0;
            if !consumed_by_dialog_navigation {
                winapi::um::winuser::TranslateMessage(&msg);
                winapi::um::winuser::DispatchMessageW(&msg);
            }
        }
    }
}

/// A `generate_thumbnails` background thread's result, sent back over a channel plus
/// `thumbnails_notice`. `cancelled` is set when `cancel_flag` was observed before every photo
/// finished decoding, in which case `bitmaps` is incomplete and must be discarded rather than
/// used to build cards.
struct ThumbnailBatchResult {
    cancelled: bool,
    bitmaps: Vec<Vec<Option<SendBitmap>>>,
}

/// Wraps `nwg::Bitmap` so a freshly decoded one can cross the channel from `generate_thumbnails`
/// back to the UI thread. Safe because a GDI bitmap handle (`HBITMAP`), unlike a window handle,
/// has no thread affinity — Windows allows using or deleting it from any thread. Only the
/// *decoding* that produces it (WIC, backed by COM) needs its own apartment-threaded COM per
/// thread — the same reasoning `image_viewer.rs`'s identical `SendBitmap` documents for
/// `schedule_prefetch`.
struct SendBitmap(nwg::Bitmap);
unsafe impl Send for SendBitmap {}

/// Decodes every lane's every photo into a small thumbnail on a background thread, so
/// `TimeCorrectionDialog::init` can show a spinner immediately instead of blocking the window's
/// first appearance on however long the *entire* project's photo count takes to decode. Checks
/// `cancel_flag` between photos and stops as soon as it's set — decoding a thumbnail has no
/// partial-write concern (unlike `export_progress_modal::run_export`'s "finish the current file"
/// rule), so cancellation is immediate. Uses its own `nwg::ImageDecoder` (and its own COM
/// apartment via `CoInitializeEx`) rather than sharing one with another thread, the same
/// reasoning `image_viewer::schedule_prefetch` documents.
fn generate_thumbnails(
    groups: Vec<time_correction::LaneImages>,
    source_dir: PathBuf,
    cancel_flag: Arc<AtomicBool>,
    tx: mpsc::Sender<ThumbnailBatchResult>,
    notice: nwg::NoticeSender,
) {
    unsafe {
        winapi::um::combaseapi::CoInitializeEx(std::ptr::null_mut(), winapi::um::objbase::COINIT_APARTMENTTHREADED);
    }

    let mut bitmaps: Vec<Vec<Option<SendBitmap>>> = Vec::with_capacity(groups.len());
    let mut cancelled = false;
    'lanes: for group in &groups {
        let mut lane_bitmaps = Vec::with_capacity(group.photos.len());
        let Ok(decoder) = nwg::ImageDecoder::new() else {
            bitmaps.push(group.photos.iter().map(|_| None).collect());
            continue;
        };
        for photo in &group.photos {
            if cancel_flag.load(Ordering::Relaxed) {
                cancelled = true;
                break 'lanes;
            }

            let path = explorer::resolve_path(&source_dir, &photo.path);
            let rotation = photo.rotation.unwrap_or(0);
            let color_params = color_correction::from_record(photo);
            let bitmap = image_viewer::decode_and_fit(
                &decoder,
                &path,
                (THUMBNAIL_SIZE as u32, THUMBNAIL_SIZE as u32),
                rotation,
                color_params.as_ref(),
            );
            lane_bitmaps.push(bitmap.map(SendBitmap));
        }
        bitmaps.push(lane_bitmaps);
    }

    unsafe {
        winapi::um::combaseapi::CoUninitialize();
    }

    let _ = tx.send(ThumbnailBatchResult { cancelled, bitmaps });
    notice.notice();
}

/// One photo's clickable cell inside a lane's thumbnail strip: `outline` is a `WS_BORDER` frame
/// slightly larger than `image`, toggled via `set_visible` to mark the lane's currently selected
/// photo — deliberately *not* a painted/colored highlight (see `select_thumbnail`'s doc comment
/// for why). Both must be kept alive: dropping either destroys its HWND out from under the
/// still-visible card.
struct ThumbnailCell {
    outline: nwg::Frame,
    image: nwg::ImageFrame,
}

/// One directory's card in the dialog's grid: its ordered photo list, which one is currently
/// selected (`current_index`), and the controls displaying it — a bold/larger directory-name
/// header, every photo as a small clickable `ThumbnailCell` inside a horizontally-scrollable
/// strip (`strip_viewport` clips, `strip_content` is the oversized child moved by
/// `strip_scrollbar`), and a status label. Built at runtime by `build_card` (the number of lanes
/// — one per top-level directory with at least one event-eligible photo — is only known once the
/// project's images are grouped), so these can't be static `#[nwg_control]` fields on
/// `TimeCorrectionDialog`; see `App::build_event_tab_entry` in `app.rs` for the same
/// runtime-controls-in-a-placeholder-frame technique this mirrors.
struct Lane {
    toplevel_dir: Option<String>,
    photos: Vec<ImageRecord>,
    /// Which photo is currently selected, or `None` if the user has deselected the lane entirely
    /// (clicked its selected thumbnail a second time) — a deselected lane shows no outline on any
    /// thumbnail and is excluded from the correction (see `select_thumbnail`).
    current_index: Cell<Option<usize>>,
    /// Shows the directory name plus its live offset from the baseline lane, or "not selected"
    /// while the lane is deselected (see `refresh_lane_headers`) — text is refreshed on every
    /// thumbnail click, in this lane or any other, since a click in the baseline lane shifts every
    /// other lane's displayed offset too.
    dir_label: nwg::Label,
    status_label: nwg::Label,
    thumbnails: Vec<ThumbnailCell>,
    /// Never read after `build_card` populates it, but must be kept alive (see `dir_label`).
    #[allow(dead_code)]
    strip_viewport: nwg::Frame,
    strip_content: nwg::Frame,
    /// Current horizontal scroll offset, in pixels, of `strip_content` within `strip_viewport`.
    /// Tracked independently of `strip_scrollbar`'s own internal position so the raw `WM_HSCROLL`
    /// handler in `init` never needs to read it back from Win32 — see that handler's doc comment.
    strip_offset: Cell<i32>,
    /// `strip_content`'s full width in pixels — always `>= CARD_STRIP_VIEWPORT_WIDTH`, so the
    /// strip never needs to scroll past its own start even when it has few photos.
    strip_content_width: i32,
    strip_scrollbar: nwg::ScrollBar,
}

#[derive(Default, NwgUi)]
pub struct TimeCorrectionDialog {
    images: RefCell<Vec<ImageRecord>>,
    source_dir: RefCell<PathBuf>,
    lanes: RefCell<Vec<Lane>>,
    result: RefCell<Option<Vec<(Option<String>, Duration)>>>,
    /// Set before `generate_thumbnails` is spawned, read by `on_progress_window_close` to decide
    /// whether closing `progress_window` must be treated as Cancel (still generating) or allowed
    /// through normally (already finished — `thumbnails_ready` closes it programmatically itself).
    thumbnails_pending: Cell<bool>,
    /// Shared with the `generate_thumbnails` background thread; set by `cancel_thumbnail_generation`
    /// or `on_progress_window_close`, checked by the worker between photos.
    cancel_flag: Arc<AtomicBool>,
    /// `time_correction::group_lanes`'s result, stashed by `init` and consumed by
    /// `thumbnails_ready` once decoding finishes — kept separate from the raw `images`/`source_dir`
    /// above since grouping (unlike decoding) is cheap enough to do once, up front, on the UI thread.
    pending_groups: RefCell<Vec<time_correction::LaneImages>>,
    /// Receiving half of the one-shot channel `generate_thumbnails` reports back over. `None`
    /// until `init` creates it.
    thumbnails_rx: RefCell<Option<mpsc::Receiver<ThumbnailBatchResult>>>,
    /// The bold/larger font applied to every card's `dir_label`. Kept alive on the dialog (rather
    /// than as a local in `init`) so it outlives every label using it for as long as the dialog
    /// is open — `nwg::Font` has no `Drop` impl in this NWG version (its `HFONT` is intentionally
    /// never deleted, the same accepted one-object-per-open cost `panel_background.rs` documents
    /// for its brushes), so this is a hygiene choice rather than a strict safety requirement.
    header_font: RefCell<Option<nwg::Font>>,
    /// Current vertical scroll offset, in pixels, of `grid_content` within `grid_viewport`.
    grid_offset: Cell<i32>,
    /// `grid_content`'s full height in pixels, computed once in `init` from the lane count.
    grid_content_height: Cell<i32>,
    /// `grid_viewport`'s actual height in pixels — `min(grid_content_height, GRID_VIEWPORT_MAX_HEIGHT)`.
    grid_viewport_height: Cell<i32>,
    /// Holds the `full_bind_event_handler` subclass `init` binds for thumbnail clicks and
    /// Enter/Escape — never read again, but must be kept alive for the same reason
    /// `image_viewer.rs`'s `key_press_handler` and `app.rs`'s `context_tab_event_handlers` are:
    /// nothing unbinds it before the window closes.
    #[allow(dead_code)]
    event_handler: RefCell<Option<nwg::EventHandler>>,
    /// Holds the raw `WM_VSCROLL` subclass bound to `window.handle` for the page-level grid
    /// scrollbar. Kept alive for the same reason as `event_handler`.
    #[allow(dead_code)]
    vscroll_handler: RefCell<Option<nwg::RawEventHandler>>,
    /// Holds the raw `WM_HSCROLL` subclass bound to `grid_content.handle`, shared by every
    /// card's thumbnail-strip scrollbar. Kept alive for the same reason as `event_handler`.
    #[allow(dead_code)]
    hscroll_handler: RefCell<Option<nwg::RawEventHandler>>,

    #[nwg_control(size: (DIALOG_WIDTH as i32, 400), position: (420, 150), title: "Set Time Correction", flags: "WINDOW")]
    #[nwg_events(
        OnInit: [TimeCorrectionDialog::init(RC_SELF)],
        OnWindowClose: [TimeCorrectionDialog::exit],
    )]
    window: nwg::Window,

    /// A small standalone spinner dialog shown immediately (before `window`, which stays hidden
    /// until thumbnails are ready) so the user gets feedback right away instead of a click that
    /// appears to do nothing while `generate_thumbnails` works through the whole project's photos.
    #[nwg_control(size: (300, 120), position: (420, 150), title: "Generating Thumbnails", flags: "WINDOW|VISIBLE")]
    #[nwg_events(OnWindowClose: [TimeCorrectionDialog::on_progress_window_close(SELF, EVT_DATA)])]
    progress_window: nwg::Window,

    #[nwg_control(parent: progress_window, text: "Generating thumbnails\u{2026}", position: (20, 15), size: (260, 20))]
    progress_label: nwg::Label,

    #[nwg_control(parent: progress_window, position: (20, 40), size: (260, 24), flags: "VISIBLE|MARQUEE", marquee: true, marquee_update: 30)]
    progress_bar: nwg::ProgressBar,

    #[nwg_control(parent: progress_window, text: "&Cancel", position: (105, 75), size: (90, 30))]
    #[nwg_events(OnButtonClick: [TimeCorrectionDialog::cancel_thumbnail_generation])]
    progress_cancel_btn: nwg::Button,

    /// Fires once `generate_thumbnails` sends its result. Declared after `window`/`progress_window`
    /// since native-windows-derive's auto-parent detection for a `Notice` with no explicit parent
    /// only looks *backward* through already-declared controls — see `image_viewer.rs`'s
    /// `prefetch_notice` for the same requirement spelled out in full.
    #[nwg_control]
    #[nwg_events(OnNotice: [TimeCorrectionDialog::thumbnails_ready(RC_SELF)])]
    thumbnails_notice: nwg::Notice,

    /// The fixed-size, clipping viewport for the card grid. Only as tall as `GRID_VIEWPORT_MAX_HEIGHT`
    /// allows; `grid_content` (its child) can be taller, in which case `grid_scrollbar` (a sibling,
    /// not a child — it must never scroll along with the content it controls) lets the user pan it.
    #[nwg_control(parent: window, flags: "VISIBLE", size: (100, 100), position: (0, 0))]
    grid_viewport: nwg::Frame,

    /// Holds every card, stacked into rows of `CARDS_PER_ROW`. Resized to its true (possibly
    /// overflowing) height in `init`, then repositioned vertically by the `WM_VSCROLL` handler.
    #[nwg_control(parent: grid_viewport, flags: "VISIBLE", size: (100, 100), position: (0, 0))]
    grid_content: nwg::Frame,

    /// The page-level vertical scrollbar. Only made visible in `init` when the grid's full content
    /// height exceeds `GRID_VIEWPORT_MAX_HEIGHT` — default `ScrollBar` flags are already
    /// `VISIBLE | VERTICAL`, which is exactly what's wanted once shown, so no explicit `flags` override.
    #[nwg_control(parent: window, size: (100, 100), position: (0, 0))]
    grid_scrollbar: nwg::ScrollBar,

    #[nwg_control(parent: window, text: "&Accept")]
    #[nwg_events(OnButtonClick: [TimeCorrectionDialog::accept])]
    accept_btn: nwg::Button,

    #[nwg_control(parent: window, text: "&Cancel")]
    #[nwg_events(OnButtonClick: [TimeCorrectionDialog::cancel])]
    cancel_btn: nwg::Button,
}

impl TimeCorrectionDialog {
    /// Fired once via `OnInit`, after `open` has already populated `images`/`source_dir`
    /// (posted, so it only runs once the message loop starts pumping — see `dispatch`'s doc
    /// comment and `image_viewer.rs`'s identical `setup`/`OnInit` ordering guarantee).
    ///
    /// `progress_window` is already visible by this point (its own `flags` include `VISIBLE`, so
    /// it appeared as soon as `dispatch`'s message loop started pumping paint messages — before
    /// this handler even runs). Groups `images` into lanes (`time_correction::group_lanes`,
    /// cheap — no decoding), stashes them for `thumbnails_ready`, then spawns
    /// `generate_thumbnails` on a background thread to do the actual (potentially slow) decode
    /// work. `window` itself — the real card grid — stays hidden until that finishes.
    fn init(dialog: &Rc<TimeCorrectionDialog>) {
        let images = dialog.images.borrow().clone();
        let source_dir = dialog.source_dir.borrow().clone();
        let groups = time_correction::group_lanes(images);
        let groups_for_worker = groups.clone();
        *dialog.pending_groups.borrow_mut() = groups;

        let mut header_font = nwg::Font::default();
        nwg::Font::builder().size(18).weight(700).build(&mut header_font).expect("Failed to build the card header font");
        *dialog.header_font.borrow_mut() = Some(header_font);

        let (tx, rx) = mpsc::channel();
        *dialog.thumbnails_rx.borrow_mut() = Some(rx);
        dialog.thumbnails_pending.set(true);
        let cancel_flag = dialog.cancel_flag.clone();
        let notice = dialog.thumbnails_notice.sender();
        thread::spawn(move || generate_thumbnails(groups_for_worker, source_dir, cancel_flag, tx, notice));
    }

    /// Fired via `OnNotice` once `generate_thumbnails` sends its result. On cancel, discards
    /// everything and closes both windows (yielding `None` from `open`, the same as any other
    /// Cancel). Otherwise builds every lane as a fixed-size card (`build_card`) positioned into a
    /// `CARDS_PER_ROW`-wide grid inside `grid_content` — every position is computed directly in
    /// Rust (not via a flex/wrap layout engine) since the scrollable content height must be known
    /// *before* `grid_viewport`/the window are sized, the same reason the original single-column
    /// version of this dialog computed `lanes_frame_height` by hand instead of querying a layout
    /// engine. Sizes `grid_viewport`/`grid_scrollbar` (showing the scrollbar only if the grid
    /// doesn't fit `GRID_VIEWPORT_MAX_HEIGHT`), the window, then binds one `full_bind_event_handler`
    /// (thumbnail clicks + Enter/Escape, mirroring the by-handle lookup idiom `ImageViewer::setup`
    /// uses for its runtime-built collection toggle buttons) plus two raw scroll handlers — see
    /// `VSCROLL_HANDLER_ID`/`HSCROLL_HANDLER_ID`'s doc comments for why there are only two despite
    /// there being one scrollbar per card. Finally closes `progress_window` and shows `window`,
    /// avoiding a flash of unlaid-out controls.
    fn thumbnails_ready(dialog: &Rc<TimeCorrectionDialog>) {
        dialog.thumbnails_pending.set(false);
        let Some(rx) = dialog.thumbnails_rx.borrow_mut().take() else { return };
        let Ok(result) = rx.try_recv() else { return };

        if result.cancelled {
            dialog.progress_window.close();
            dialog.window.close();
            return;
        }

        let groups = std::mem::take(&mut *dialog.pending_groups.borrow_mut());
        let lane_count = groups.len();
        let header_font_ref = dialog.header_font.borrow();
        let header_font_ref = header_font_ref.as_ref().expect("Header font was built in init");

        let mut lanes = Vec::with_capacity(lane_count);
        for (index, (group, bitmaps)) in groups.into_iter().zip(result.bitmaps.into_iter()).enumerate() {
            let row = index / CARDS_PER_ROW;
            let col = index % CARDS_PER_ROW;
            let position = (
                (col as f32 * (CARD_WIDTH + CARD_GAP)) as i32,
                (row as f32 * (CARD_HEIGHT + CARD_GAP)) as i32,
            );
            let bitmaps: Vec<Option<nwg::Bitmap>> = bitmaps.into_iter().map(|b| b.map(|sb| sb.0)).collect();
            let lane = build_card(&dialog.grid_content, header_font_ref, group, bitmaps, position);
            lanes.push(lane);
        }
        refresh_lane_headers(&lanes);
        *dialog.lanes.borrow_mut() = lanes;

        let rows = row_count(lane_count, CARDS_PER_ROW);
        let grid_content_height = if rows == 0 { 0 } else { (rows as f32 * CARD_HEIGHT + (rows as f32 - 1.0) * CARD_GAP) as i32 };
        let grid_viewport_height = grid_content_height.min(GRID_VIEWPORT_MAX_HEIGHT as i32);
        dialog.grid_content_height.set(grid_content_height);
        dialog.grid_viewport_height.set(grid_viewport_height);

        dialog.grid_viewport.set_position(WINDOW_PADDING as i32, WINDOW_PADDING as i32);
        dialog.grid_viewport.set_size(GRID_CONTENT_WIDTH as u32, grid_viewport_height.max(0) as u32);
        dialog.grid_content.set_position(0, 0);
        dialog.grid_content.set_size(GRID_CONTENT_WIDTH as u32, grid_content_height.max(0) as u32);

        let needs_grid_scroll = grid_content_height > grid_viewport_height;
        dialog.grid_scrollbar.set_position((WINDOW_PADDING + GRID_CONTENT_WIDTH) as i32, WINDOW_PADDING as i32);
        dialog.grid_scrollbar.set_size(SCROLLBAR_THICKNESS as u32, grid_viewport_height.max(0) as u32);
        dialog.grid_scrollbar.set_range(0..(grid_content_height - grid_viewport_height).max(0) as usize);
        dialog.grid_scrollbar.set_pos(0);
        dialog.grid_scrollbar.set_visible(needs_grid_scroll);

        let footer_y = WINDOW_PADDING as i32 + grid_viewport_height + WINDOW_PADDING as i32;
        dialog.accept_btn.set_position(WINDOW_PADDING as i32, footer_y);
        dialog.accept_btn.set_size(FOOTER_BUTTON_WIDTH as u32, FOOTER_HEIGHT as u32);
        dialog.cancel_btn.set_position(WINDOW_PADDING as i32 + FOOTER_BUTTON_WIDTH as i32, footer_y);
        dialog.cancel_btn.set_size(FOOTER_BUTTON_WIDTH as u32, FOOTER_HEIGHT as u32);

        let window_height = 3.0 * WINDOW_PADDING + grid_viewport_height as f32 + FOOTER_HEIGHT;
        dialog.window.set_size(DIALOG_WIDTH as u32, window_height as u32);

        let dialog_weak = Rc::downgrade(dialog);
        let click_handler_weak = dialog_weak.clone();
        let handler = nwg::full_bind_event_handler(&dialog.window.handle, move |evt, evt_data, handle| {
            let Some(dialog) = click_handler_weak.upgrade() else { return };
            match evt {
                nwg::Event::OnKeyPress => match resolve_dialog_key(evt_data.on_key()) {
                    Some(DialogAction::Accept) => dialog.accept(),
                    Some(DialogAction::Cancel) => dialog.cancel(),
                    None => {}
                },
                nwg::Event::OnImageFrameClick => {
                    let lanes = dialog.lanes.borrow();
                    for lane in lanes.iter() {
                        if let Some(index) = lane.thumbnails.iter().position(|cell| cell.image.handle == handle) {
                            select_thumbnail(lane, index);
                            break;
                        }
                    }
                    refresh_lane_headers(&lanes);
                }
                _ => {}
            }
        });
        *dialog.event_handler.borrow_mut() = Some(handler);

        // The page-level vertical scrollbar's own WM_VSCROLL is sent to *its* parent (`window`,
        // since `grid_scrollbar` is a sibling of `grid_viewport`, not a child — see that field's
        // doc comment). Only one such scrollbar exists, so no by-handle lookup is needed.
        let vscroll_weak = dialog_weak.clone();
        let vscroll_handler = nwg::bind_raw_event_handler(&dialog.window.handle, VSCROLL_HANDLER_ID, move |_hwnd, msg, w, _l| {
            use winapi::shared::minwindef::{HIWORD, LOWORD};
            use winapi::um::winuser::WM_VSCROLL;

            if msg != WM_VSCROLL {
                return None;
            }
            let Some(dialog) = vscroll_weak.upgrade() else { return None };

            let next = scroll_offset_after(
                dialog.grid_offset.get(),
                dialog.grid_content_height.get(),
                dialog.grid_viewport_height.get(),
                LOWORD(w as u32),
                HIWORD(w as u32),
                GRID_LINE_STEP,
            );
            dialog.grid_offset.set(next);
            dialog.grid_content.set_position(0, -next);
            dialog.grid_scrollbar.set_pos(next as usize);
            None
        })
        .expect("Failed to bind the Set Time Correction grid's vertical scroll handler");
        *dialog.vscroll_handler.borrow_mut() = Some(vscroll_handler);

        // Every card's thumbnail-strip scrollbar is parented to `grid_content` (so it scrolls
        // vertically along with its own card when the page scrolls), which means WM_HSCROLL for
        // *all* of them arrives here — `lParam` carries the specific scrollbar's HWND, so one
        // handler dispatches to the right lane by matching it, the same by-handle lookup idiom
        // the click handler above and the old Prev/Next dispatch both used.
        let hscroll_weak = dialog_weak;
        let hscroll_handler = nwg::bind_raw_event_handler(&dialog.grid_content.handle, HSCROLL_HANDLER_ID, move |_hwnd, msg, w, l| {
            use winapi::shared::minwindef::{HIWORD, LOWORD};
            use winapi::shared::windef::HWND;
            use winapi::um::winuser::WM_HSCROLL;

            if msg != WM_HSCROLL {
                return None;
            }
            let Some(dialog) = hscroll_weak.upgrade() else { return None };

            let scrollbar_hwnd = l as HWND;
            let lanes = dialog.lanes.borrow();
            for lane in lanes.iter() {
                if lane.strip_scrollbar.handle.hwnd() != Some(scrollbar_hwnd) {
                    continue;
                }
                let next = scroll_offset_after(
                    lane.strip_offset.get(),
                    lane.strip_content_width,
                    CARD_STRIP_VIEWPORT_WIDTH as i32,
                    LOWORD(w as u32),
                    HIWORD(w as u32),
                    STRIP_LINE_STEP,
                );
                lane.strip_offset.set(next);
                lane.strip_content.set_position(-next, 0);
                lane.strip_scrollbar.set_pos(next as usize);
                break;
            }
            None
        })
        .expect("Failed to bind the Set Time Correction card thumbnail strip scroll handler");
        *dialog.hscroll_handler.borrow_mut() = Some(hscroll_handler);

        dialog.progress_window.close();
        dialog.window.set_visible(true);
    }

    /// Only sets `cancel_flag` — `generate_thumbnails` is checked between photos, not files, so
    /// there's no "let the current one finish" concern the way `export_progress_modal::abort`
    /// has. The window stays open, showing the spinner, until `thumbnails_ready` closes it for
    /// real once the worker acknowledges the cancellation.
    fn cancel_thumbnail_generation(&self) {
        self.cancel_flag.store(true, Ordering::Relaxed);
        self.progress_cancel_btn.set_enabled(false);
    }

    /// Covers both the Windows system-menu/Alt+F4 close and `thumbnails_ready`'s own
    /// `progress_window.close()`. While thumbnails are still generating (`thumbnails_pending`),
    /// treat this exactly like Cancel — request cancellation but veto the close via
    /// `WindowCloseData::close(false)` so the message loop (and the worker thread it's tracking)
    /// stays alive until the worker actually finishes; only then does `thumbnails_ready`'s own
    /// `close()` call (with `thumbnails_pending` already cleared) pass through for real. Mirrors
    /// `export_progress_modal::ExportingDialog::on_window_close` exactly.
    fn on_progress_window_close(&self, data: &nwg::EventData) {
        if self.thumbnails_pending.get() {
            self.cancel_flag.store(true, Ordering::Relaxed);
            if let nwg::EventData::OnWindowClose(close_data) = data {
                close_data.close(false);
            }
        }
    }

    /// Gathers each lane's currently-selected photo (and whether it's deselected entirely) and
    /// hands it to the pure `time_correction::compute_offsets` — on success, stores the result
    /// and closes; on failure (fewer than two lanes *included*, or an included lane's selection
    /// has no `corrected_date_taken` yet), shows the error and leaves the dialog open so the user
    /// can fix it, the same validate-then-close-or-explain shape `collection_modal::accept` uses.
    fn accept(&self) {
        let selections: Vec<LaneSelection> = self
            .lanes
            .borrow()
            .iter()
            .map(|lane| LaneSelection {
                toplevel_dir: lane.toplevel_dir.clone(),
                photo_count: lane.photos.len(),
                selected_corrected_date_taken: lane.current_index.get().and_then(|i| lane.photos.get(i)).and_then(|p| p.corrected_date_taken),
                included: lane.current_index.get().is_some(),
            })
            .collect();

        match time_correction::compute_offsets(&selections) {
            Ok(corrections) => {
                *self.result.borrow_mut() = Some(corrections);
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

    fn exit(&self) {
        nwg::stop_thread_dispatch();
    }
}

/// Builds one lane's card (parented to `parent`, the placeholder `grid_content`) at `position`
/// (top-left, in `grid_content`-local coordinates): a bold/larger directory-name header, every
/// photo in `group` — already decoded by `generate_thumbnails` into `bitmaps` (same length/order
/// as `group.photos`) — set onto its own small `ThumbnailCell`, laid out in a single row inside a
/// horizontally-scrollable strip, and a status label. The strip's own scrollbar is only shown
/// when the lane has more photos than fit in `THUMBS_VISIBLE_PER_CARD` — every card still
/// reserves the space for it, so every card in a grid row stays the same height regardless of
/// photo count.
fn build_card(
    parent: &nwg::Frame,
    header_font: &nwg::Font,
    group: time_correction::LaneImages,
    bitmaps: Vec<Option<nwg::Bitmap>>,
    position: (i32, i32),
) -> Lane {
    let (card_x, card_y) = position;
    let dir_text = group.toplevel_dir.clone().unwrap_or_else(|| "(root)".to_string());

    let mut dir_label = nwg::Label::default();
    nwg::Label::builder()
        .parent(parent)
        .text(&dir_text)
        .font(Some(header_font))
        .position((card_x + CARD_PADDING as i32, card_y + CARD_PADDING as i32))
        .size((CARD_STRIP_VIEWPORT_WIDTH as i32, CARD_HEADER_HEIGHT as i32))
        .build(&mut dir_label)
        .expect("Failed to build a card's directory label");

    let strip_y = card_y + CARD_PADDING as i32 + CARD_HEADER_HEIGHT as i32 + CARD_INNER_GAP as i32;
    let mut strip_viewport = nwg::Frame::default();
    nwg::Frame::builder()
        .parent(parent)
        .flags(nwg::FrameFlags::VISIBLE)
        .position((card_x + CARD_PADDING as i32, strip_y))
        .size((CARD_STRIP_VIEWPORT_WIDTH as i32, CARD_STRIP_HEIGHT as i32))
        .build(&mut strip_viewport)
        .expect("Failed to build a card's thumbnail strip viewport");

    let strip_content_width = (group.photos.len() as f32 * THUMB_CELL + (group.photos.len().saturating_sub(1)) as f32 * THUMB_GAP)
        .max(CARD_STRIP_VIEWPORT_WIDTH);
    let mut strip_content = nwg::Frame::default();
    nwg::Frame::builder()
        .parent(&strip_viewport)
        .flags(nwg::FrameFlags::VISIBLE)
        .position((0, 0))
        .size((strip_content_width as i32, CARD_STRIP_HEIGHT as i32))
        .build(&mut strip_content)
        .expect("Failed to build a card's thumbnail strip content");

    let mut thumbnails = Vec::with_capacity(group.photos.len());
    for (index, bitmap) in bitmaps.into_iter().enumerate() {
        let cell_x = (index as f32 * (THUMB_CELL + THUMB_GAP)) as i32;

        let mut outline = nwg::Frame::default();
        nwg::Frame::builder()
            .parent(&strip_content)
            .flags(nwg::FrameFlags::VISIBLE | nwg::FrameFlags::BORDER)
            .position((cell_x, 0))
            .size((THUMB_CELL as i32, THUMB_CELL as i32))
            .build(&mut outline)
            .expect("Failed to build a thumbnail's selection outline");
        outline.set_visible(index == 0);

        let mut image = nwg::ImageFrame::default();
        nwg::ImageFrame::builder()
            .parent(&strip_content)
            .flags(nwg::ImageFrameFlags::VISIBLE)
            .background_color(Some([255, 255, 255]))
            .position((cell_x + THUMB_BORDER as i32, THUMB_BORDER as i32))
            .size((THUMBNAIL_SIZE as i32, THUMBNAIL_SIZE as i32))
            .build(&mut image)
            .expect("Failed to build a thumbnail's image frame");
        image.set_bitmap(bitmap.as_ref());

        thumbnails.push(ThumbnailCell { outline, image });
    }

    let scrollbar_y = strip_y + CARD_STRIP_HEIGHT as i32 + CARD_INNER_GAP as i32;
    let mut strip_scrollbar = nwg::ScrollBar::default();
    nwg::ScrollBar::builder()
        .parent(parent)
        .flags(nwg::ScrollBarFlags::VISIBLE | nwg::ScrollBarFlags::HORIZONTAL)
        .position((card_x + CARD_PADDING as i32, scrollbar_y))
        .size((CARD_STRIP_VIEWPORT_WIDTH as i32, SCROLLBAR_THICKNESS as i32))
        .range(Some(0..(strip_content_width - CARD_STRIP_VIEWPORT_WIDTH).max(0.0) as usize))
        .pos(Some(0))
        .build(&mut strip_scrollbar)
        .expect("Failed to build a card's thumbnail strip scrollbar");
    strip_scrollbar.set_visible(group.photos.len() > THUMBS_VISIBLE_PER_CARD);

    let status_y = scrollbar_y + SCROLLBAR_THICKNESS as i32 + CARD_INNER_GAP as i32;
    let mut status_label = nwg::Label::default();
    nwg::Label::builder()
        .parent(parent)
        .text(&lane_status_text(&group.photos, Some(0)))
        .position((card_x + CARD_PADDING as i32, status_y))
        .size((CARD_STRIP_VIEWPORT_WIDTH as i32, CARD_STATUS_HEIGHT as i32))
        .build(&mut status_label)
        .expect("Failed to build a card's status label");

    Lane {
        toplevel_dir: group.toplevel_dir,
        photos: group.photos,
        current_index: Cell::new(Some(0)),
        dir_label,
        status_label,
        thumbnails,
        strip_viewport,
        strip_content,
        strip_offset: Cell::new(0),
        strip_content_width: strip_content_width as i32,
        strip_scrollbar,
    }
}

/// Handles a click on `lane`'s `clicked_index` thumbnail: if it's already the lane's selected
/// photo, this is the *second* click on it, so the lane is deselected entirely (`new_index` is
/// `None` — no thumbnail shows an outline, and the lane is excluded from the correction).
/// Otherwise `clicked_index` becomes the new selection, whether the lane was previously deselected
/// or had a different photo selected. Toggles the old and new `ThumbnailCell`'s outline visibility
/// and updates the status label. No decode work at all — every photo's bitmap was already decoded
/// into its own `ThumbnailCell` by `build_card`, so selecting is just two visibility toggles and a
/// text update, unlike the old Prev/Next design's per-click re-decode.
///
/// The outline is a plain `WS_BORDER` frame rather than a painted/colored highlight deliberately:
/// this dialog can be opened repeatedly and a lane can hold hundreds of photos, and both
/// `panel_background::paint` and `ImageFrame::background_color` allocate a GDI brush plus a raw
/// event handler with no cleanup path — fine for the handful of permanent, process-lifetime
/// frames they're otherwise used for, but here it would leak one GDI brush per thumbnail per
/// dialog open, eventually exhausting the process's GDI object quota app-wide. `set_visible` has
/// no such cost.
fn select_thumbnail(lane: &Lane, clicked_index: usize) {
    let old_index = lane.current_index.get();
    if let Some(cell) = old_index.and_then(|index| lane.thumbnails.get(index)) {
        cell.outline.set_visible(false);
    }
    let new_index = if old_index == Some(clicked_index) { None } else { Some(clicked_index) };
    if let Some(cell) = new_index.and_then(|index| lane.thumbnails.get(index)) {
        cell.outline.set_visible(true);
    }
    lane.current_index.set(new_index);
    lane.status_label.set_text(&lane_status_text(&lane.photos, new_index));
}

/// Recomputes and redraws every card's directory-name header with its live offset from the
/// baseline lane (`0` for the baseline itself, signed `HH:MM:SS` for the rest, a call-out when a
/// selection has no `corrected_date_taken` yet, or "not selected" for a deselected lane) — called
/// once `lanes` is fully built and again after every thumbnail click, since a click in the
/// baseline lane changes every other lane's offset, not just the clicked one. Builds the same
/// `LaneSelection` shape `accept` does, from each lane's currently selected photo (or lack of one).
fn refresh_lane_headers(lanes: &[Lane]) {
    let selections: Vec<LaneSelection> = lanes
        .iter()
        .map(|lane| LaneSelection {
            toplevel_dir: lane.toplevel_dir.clone(),
            photo_count: lane.photos.len(),
            selected_corrected_date_taken: lane.current_index.get().and_then(|i| lane.photos.get(i)).and_then(|p| p.corrected_date_taken),
            included: lane.current_index.get().is_some(),
        })
        .collect();

    for (lane, offset) in lanes.iter().zip(time_correction::lane_offsets(&selections)) {
        let dir_text = lane.toplevel_dir.clone().unwrap_or_else(|| "(root)".to_string());
        lane.dir_label.set_text(&format!("{dir_text}   {}", time_correction::format_lane_offset(offset)));
    }
}

/// The status label text under a lane's thumbnail strip: filename, position within the lane, and
/// the selected photo's `corrected_date_taken` (or a call-out that it's missing, since that also
/// means `compute_offsets` will reject this lane's current selection if left there) — or, when
/// `index` is `None` (the lane has been deselected), a call-out that it's excluded from the
/// correction entirely. Pure so it's unit-testable without a window.
fn lane_status_text(photos: &[ImageRecord], index: Option<usize>) -> String {
    let Some(index) = index else { return "Not part of the time correction".to_string() };
    let Some(photo) = photos.get(index) else { return String::new() };
    let filename = photo.path.rsplit('/').next().unwrap_or(photo.path.as_str());
    match photo.corrected_date_taken {
        Some(date_taken) => format!("{filename}  \u{2014}  {} of {}  \u{2014}  {date_taken}", index + 1, photos.len()),
        None => format!("{filename}  \u{2014}  {} of {}  \u{2014}  no date yet", index + 1, photos.len()),
    }
}

/// How many grid rows `lane_count` cards need at `columns` cards per row (ceiling division).
/// Pure arithmetic, kept alongside the other small pure helpers in this file since it has no
/// NWG/db dependency of its own despite feeding the grid layout.
fn row_count(lane_count: usize, columns: usize) -> usize {
    if lane_count == 0 {
        0
    } else {
        (lane_count + columns - 1) / columns
    }
}

/// Clamps a scroll offset (in pixels) to the valid `0..=(content_size - viewport_size)` range —
/// when `content_size <= viewport_size` there's nothing to scroll, so the only valid offset is 0.
fn clamp_scroll_offset(offset: i32, content_size: i32, viewport_size: i32) -> i32 {
    let max_offset = (content_size - viewport_size).max(0);
    offset.clamp(0, max_offset)
}

/// Computes the next scroll offset from a raw `WM_VSCROLL`/`WM_HSCROLL` message's `wParam`,
/// already split into `low_word` (the `SB_*` scroll code — shared between the vertical and
/// horizontal code sets, e.g. `SB_LINEUP == SB_LINELEFT`, so this one function serves both the
/// page-level vertical scroll and every card's horizontal strip scroll) and `high_word` (the
/// live thumb position, only meaningful for `SB_THUMBTRACK`/`SB_THUMBPOSITION`). `line_step` is
/// how far one arrow-click line-scroll moves; a page-scroll moves by `viewport_size`. Always
/// clamped via `clamp_scroll_offset`. Pure and NWG-independent so every scroll code and both
/// clamp ends are directly unit-testable.
fn scroll_offset_after(current: i32, content_size: i32, viewport_size: i32, low_word: u16, high_word: u16, line_step: i32) -> i32 {
    use winapi::um::winuser::{SB_BOTTOM, SB_LINEDOWN, SB_LINEUP, SB_PAGEDOWN, SB_PAGEUP, SB_THUMBPOSITION, SB_THUMBTRACK, SB_TOP};

    let code = low_word as isize;
    let next = match code {
        SB_LINEUP => current - line_step,
        SB_LINEDOWN => current + line_step,
        SB_PAGEUP => current - viewport_size,
        SB_PAGEDOWN => current + viewport_size,
        SB_THUMBTRACK | SB_THUMBPOSITION => high_word as i32,
        SB_TOP => 0,
        SB_BOTTOM => content_size - viewport_size,
        _ => current,
    };

    clamp_scroll_offset(next, content_size, viewport_size)
}

/// A dialog-level action reachable via a standard Windows OK/Cancel accelerator.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum DialogAction {
    Accept,
    Cancel,
}

/// Maps Enter/Escape to Accept/Cancel — the standard Windows modal-dialog convention. Kept
/// independent of NWG so it's unit-testable without a window, the same way `shortcuts::resolve`
/// and `image_viewer_shortcuts::resolve` are.
fn resolve_dialog_key(key: u32) -> Option<DialogAction> {
    match key {
        nwg::keys::RETURN => Some(DialogAction::Accept),
        nwg::keys::ESCAPE => Some(DialogAction::Cancel),
        _ => None,
    }
}

/// Whether a raw Win32 message is an Enter/Escape key-down that must bypass `IsDialogMessageW`'s
/// dialog-navigation handling so it reaches `OnKeyPress` (see `dispatch`'s doc comment) — the
/// same problem, and same fix, `image_viewer_shortcuts::bypasses_dialog_navigation` documents
/// for Left/Right arrows.
fn bypasses_dialog_navigation(message: u32, virtual_key: usize) -> bool {
    message == winapi::um::winuser::WM_KEYDOWN
        && (virtual_key == nwg::keys::RETURN as usize || virtual_key == nwg::keys::ESCAPE as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDateTime;

    fn photo(path: &str, corrected_date_taken: Option<&str>) -> ImageRecord {
        ImageRecord {
            path: path.to_string(),
            corrected_date_taken: corrected_date_taken.map(|s| NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").unwrap()),
            ..ImageRecord::default()
        }
    }

    #[test]
    fn lane_status_text_includes_filename_position_and_date() {
        let photos = vec![photo("50D/a.jpg", Some("2026-01-01 10:00:00")), photo("50D/b.jpg", Some("2026-01-01 10:05:00"))];

        let text = lane_status_text(&photos, Some(1));

        assert!(text.contains("b.jpg"));
        assert!(text.contains("2 of 2"));
        assert!(text.contains("2026-01-01 10:05:00"));
    }

    #[test]
    fn lane_status_text_calls_out_a_missing_corrected_date() {
        let photos = vec![photo("50D/a.jpg", None)];

        let text = lane_status_text(&photos, Some(0));

        assert!(text.contains("no date yet"));
    }

    #[test]
    fn lane_status_text_calls_out_a_deselected_lane() {
        let photos = vec![photo("50D/a.jpg", Some("2026-01-01 10:00:00"))];

        let text = lane_status_text(&photos, None);

        assert!(text.contains("Not part of the time correction"));
    }

    #[test]
    fn resolve_dialog_key_maps_return_and_escape() {
        assert_eq!(resolve_dialog_key(nwg::keys::RETURN), Some(DialogAction::Accept));
        assert_eq!(resolve_dialog_key(nwg::keys::ESCAPE), Some(DialogAction::Cancel));
    }

    #[test]
    fn resolve_dialog_key_ignores_unrelated_keys() {
        assert_eq!(resolve_dialog_key(nwg::keys::_A), None);
    }

    #[test]
    fn return_and_escape_keydown_bypass_dialog_navigation() {
        assert!(bypasses_dialog_navigation(winapi::um::winuser::WM_KEYDOWN, nwg::keys::RETURN as usize));
        assert!(bypasses_dialog_navigation(winapi::um::winuser::WM_KEYDOWN, nwg::keys::ESCAPE as usize));
    }

    #[test]
    fn unrelated_keydown_does_not_bypass_dialog_navigation() {
        assert!(!bypasses_dialog_navigation(winapi::um::winuser::WM_KEYDOWN, nwg::keys::_A as usize));
    }

    #[test]
    fn non_keydown_message_does_not_bypass_dialog_navigation_even_for_return() {
        assert!(!bypasses_dialog_navigation(winapi::um::winuser::WM_CHAR, nwg::keys::RETURN as usize));
    }

    #[test]
    fn row_count_is_a_ceiling_division() {
        assert_eq!(row_count(0, 2), 0);
        assert_eq!(row_count(1, 2), 1);
        assert_eq!(row_count(2, 2), 1);
        assert_eq!(row_count(3, 2), 2);
        assert_eq!(row_count(4, 2), 2);
        assert_eq!(row_count(5, 2), 3);
    }

    #[test]
    fn clamp_scroll_offset_keeps_an_in_range_offset_unchanged() {
        assert_eq!(clamp_scroll_offset(50, 500, 200), 50);
    }

    #[test]
    fn clamp_scroll_offset_clamps_a_negative_offset_to_zero() {
        assert_eq!(clamp_scroll_offset(-10, 500, 200), 0);
    }

    #[test]
    fn clamp_scroll_offset_clamps_an_over_max_offset() {
        assert_eq!(clamp_scroll_offset(1000, 500, 200), 300);
    }

    #[test]
    fn clamp_scroll_offset_is_always_zero_when_content_fits_the_viewport() {
        assert_eq!(clamp_scroll_offset(50, 200, 500), 0);
    }

    #[test]
    fn scroll_offset_after_line_up_and_down() {
        let up = winapi::um::winuser::SB_LINEUP as u16;
        let down = winapi::um::winuser::SB_LINEDOWN as u16;
        assert_eq!(scroll_offset_after(100, 1000, 200, down, 0, 20), 120);
        assert_eq!(scroll_offset_after(100, 1000, 200, up, 0, 20), 80);
    }

    #[test]
    fn scroll_offset_after_line_up_clamps_at_zero() {
        let up = winapi::um::winuser::SB_LINEUP as u16;
        assert_eq!(scroll_offset_after(10, 1000, 200, up, 0, 20), 0);
    }

    #[test]
    fn scroll_offset_after_page_up_and_down() {
        let page_up = winapi::um::winuser::SB_PAGEUP as u16;
        let page_down = winapi::um::winuser::SB_PAGEDOWN as u16;
        assert_eq!(scroll_offset_after(100, 1000, 200, page_down, 0, 20), 300);
        assert_eq!(scroll_offset_after(300, 1000, 200, page_up, 0, 20), 100);
    }

    #[test]
    fn scroll_offset_after_thumb_track_and_position_use_the_high_word() {
        let track = winapi::um::winuser::SB_THUMBTRACK as u16;
        let position = winapi::um::winuser::SB_THUMBPOSITION as u16;
        assert_eq!(scroll_offset_after(0, 1000, 200, track, 450, 20), 450);
        assert_eq!(scroll_offset_after(0, 1000, 200, position, 450, 20), 450);
    }

    #[test]
    fn scroll_offset_after_thumb_track_clamps_to_the_max_offset() {
        let track = winapi::um::winuser::SB_THUMBTRACK as u16;
        assert_eq!(scroll_offset_after(0, 1000, 200, track, 9000, 20), 800);
    }

    #[test]
    fn scroll_offset_after_top_and_bottom() {
        let top = winapi::um::winuser::SB_TOP as u16;
        let bottom = winapi::um::winuser::SB_BOTTOM as u16;
        assert_eq!(scroll_offset_after(500, 1000, 200, top, 0, 20), 0);
        assert_eq!(scroll_offset_after(0, 1000, 200, bottom, 0, 20), 800);
    }

    #[test]
    fn scroll_offset_after_ignores_an_unrecognized_code() {
        assert_eq!(scroll_offset_after(100, 1000, 200, 0xFFFF, 0, 20), 100);
    }
}
