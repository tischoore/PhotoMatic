use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

use native_windows_derive::NwgUi;
use native_windows_gui as nwg;
use nwg::NativeUi;

use crate::color_correction;
use crate::db::models::ImageRecord;
use crate::explorer;
use crate::export::{self, ExportOptions, ExportOutcome};

/// Opens the Exporting dialog on its own thread, which immediately spawns a second, inner
/// worker thread to do the actual copying/encoding (see the module doc). Returns a handle whose
/// `join()` yields the `ExportOutcome` once the dialog closes — unlike the other modals in this
/// app, there's no `Option<T>`/Cancel here: Abort is itself a reported outcome, not "nothing to
/// report", so this always yields a real result.
pub fn open(
    images: Vec<ImageRecord>,
    source_dir: PathBuf,
    options: ExportOptions,
    sender: nwg::NoticeSender,
) -> thread::JoinHandle<ExportOutcome> {
    thread::spawn(move || {
        let dialog = ExportingDialog::build_ui(Default::default()).expect("Failed to build the Exporting dialog");

        let abort_flag = dialog.abort_flag.clone();
        let work_done_sender = dialog.work_done_notice.sender();
        let copy_handle = thread::spawn(move || run_export(images, source_dir, options, abort_flag, work_done_sender));
        *dialog.copy_thread.borrow_mut() = Some(copy_handle);

        nwg::dispatch_thread_events();

        sender.notice();
        dialog.result.take().unwrap_or_default()
    })
}

/// The inner worker thread's body: copies (and, when recompressing, re-encodes) every picture in
/// order, checking `abort_flag` between files only — so a request to abort always lets the file
/// currently being written finish rather than leaving a partial one behind. Per-file failures are
/// collected into `ExportOutcome::errors` rather than stopping the run, matching this codebase's
/// existing style of not letting one bad file abort an otherwise-working batch operation (e.g.
/// `scan::scan_directory`).
fn run_export(
    images: Vec<ImageRecord>,
    source_dir: PathBuf,
    options: ExportOptions,
    abort_flag: Arc<AtomicBool>,
    work_done_sender: nwg::NoticeSender,
) -> ExportOutcome {
    unsafe {
        winapi::um::combaseapi::CoInitializeEx(
            std::ptr::null_mut(),
            winapi::um::objbase::COINIT_APARTMENTTHREADED,
        );
    }

    let _ = std::fs::create_dir_all(&options.output_dir);
    let digit_count = export::digit_count(images.len());

    let mut copied = 0;
    let mut errors = Vec::new();
    let mut aborted = false;
    for (index, record) in images.iter().enumerate() {
        if abort_flag.load(Ordering::Relaxed) {
            aborted = true;
            break;
        }

        let src = explorer::resolve_path(&source_dir, &record.path);
        let file_name = Path::new(&record.path).file_name().and_then(|n| n.to_str()).unwrap_or(&record.path);
        let dest = options.output_dir.join(export::indexed_file_name(index + 1, digit_count, file_name, options.recompression));

        let color_params = color_correction::from_record(record);
        match export::export_one(&src, &dest, record.rotation.unwrap_or(0), color_params, options.recompression, options.rescale) {
            Ok(()) => copied += 1,
            Err(err) => errors.push(format!("{}: {err}", record.path)),
        }
    }

    unsafe {
        winapi::um::combaseapi::CoUninitialize();
    }

    let outcome = ExportOutcome { copied, total: images.len(), aborted, errors };
    work_done_sender.notice();
    outcome
}

#[derive(Default, NwgUi)]
pub struct ExportingDialog {
    result: RefCell<Option<ExportOutcome>>,
    copy_thread: RefCell<Option<thread::JoinHandle<ExportOutcome>>>,
    abort_flag: Arc<AtomicBool>,

    #[nwg_control(size: (280, 130), position: (650, 300), title: "Exporting", flags: "WINDOW|VISIBLE")]
    #[nwg_events(OnWindowClose: [ExportingDialog::on_window_close(SELF, EVT_DATA)])]
    window: nwg::Window,

    #[nwg_control(parent: window, position: (20, 30), size: (240, 24), flags: "VISIBLE|MARQUEE", marquee: true, marquee_update: 30)]
    progress: nwg::ProgressBar,

    #[nwg_control(parent: window, text: "&Abort", position: (95, 75), size: (90, 30))]
    #[nwg_events(OnButtonClick: [ExportingDialog::abort])]
    abort_btn: nwg::Button,

    #[nwg_control]
    #[nwg_events(OnNotice: [ExportingDialog::work_finished])]
    work_done_notice: nwg::Notice,
}

impl ExportingDialog {
    /// Only sets the abort flag — doesn't close the window itself, since the worker thread must
    /// finish its current file first (no partial files). The window stays open, showing the
    /// spinner, until `work_finished` closes it for real.
    fn abort(&self) {
        self.abort_flag.store(true, Ordering::Relaxed);
        self.abort_btn.set_enabled(false);
    }

    /// Fired via `OnNotice` once the worker thread (`run_export`) returns. Joining is
    /// effectively non-blocking here since the notice is only sent after the thread's work is
    /// done.
    fn work_finished(&self) {
        let Some(handle) = self.copy_thread.borrow_mut().take() else { return };
        *self.result.borrow_mut() = Some(handle.join().unwrap_or_default());
        self.window.close();
    }

    /// Covers both the Windows system-menu/Alt+F4 close and `work_finished`'s own
    /// `window.close()`. While the worker is still running (`copy_thread` is still `Some`),
    /// treat this exactly like Abort — request cancellation but veto the close via
    /// `WindowCloseData::close(false)`, so the message loop (and the worker thread it's tracking)
    /// stays alive until the worker actually finishes; only then does `work_finished`'s own
    /// `close()` call pass through to actually stop the dialog's message loop.
    fn on_window_close(&self, data: &nwg::EventData) {
        if self.copy_thread.borrow().is_some() {
            self.abort_flag.store(true, Ordering::Relaxed);
            if let nwg::EventData::OnWindowClose(close_data) = data {
                close_data.close(false);
            }
            return;
        }
        nwg::stop_thread_dispatch();
    }
}
